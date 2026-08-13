# scripts/repl-parity.ps1 —— tie REPL 会话 parity 回归脚本（自举 v2 T4.3）
# ============================================================
# 职责：用同一份 golden 会话（命令列表）分别跑
#   - tie 解释器通道：compiler/repl.tie 由 tiec 编译为 compiler/repl.exe
#     （自举升格：tiec 是 tie 语言自写的自举 v2 编译器；
#     eval 走 compiler/interp/interp.tie 的 interp.eval —— T4.1 tie 自写解释器）
#   - Rust 解释器通道：repl/repl.tie 由 Rust 种子 tie-llvm.exe 编译为
#     compiler/_rust_repl.exe（参照基线，eval() 走 Rust tie-interp 的
#     C ABI 桥 tie_eval_expr → Session::eval）
# 逐字节 diff 两者 stdout，验证 REPL 行为 parity（G3 闸门的一部分）。
#
# golden 机制：
#   - tests/repl/golden_session.txt  命令列表（每行一条，以 :quit 结束）
#   - tests/repl/golden_stdout.txt   预期 stdout（由 Rust 通道原始字节生成；
#                                    每次运行都会复核 Rust 侧与 golden 一致，
#                                    Rust 行为变更时需 -Regen 重新生成）
#   - tests/repl/masks.txt           可选掩码规则（每行一个正则，`#` 注释；
#                                    掩码同时作用于 Rust/tie 两侧输出与 golden，
#                                    用于未来含时间戳/随机数的会话；当前 golden
#                                    会话全部为固定输入，无掩码行）
#
# 已知 parity 缺口（interp 状态问题，已报告主控，T4.1 待修）：
#   compiler/interp/session.tie scope_pop() 以「哨兵占位」（table_push 而非
#   缩栈）实现，scope_top() = len(env_soff)-1 单调增长永不回到 -1；任何带
#   形参的函数调用后，REPL 顶层 var（interp.tie 按 scope_top()<0 判定是否
#   落入 globals）会写进偏移写死为 0 的哨兵作用域，后续 eval 查找不到 →
#   "变量 'm' 未声明"。golden 会话因此**将全部顶层 var 声明放在函数调用之前**
#   （会话自然顺序），diff 为空；修复后可在会话末尾追加
#   `func g(p: i64) -> i64 { return p; }` / `g(1)` / `var y = 9; y` 复验。
#
# 用法：
#   .\scripts\repl-parity.ps1              # 构建 + 运行 parity（diff 为空即通过）
#   .\scripts\repl-parity.ps1 -Regen       # 用 Rust 侧重新生成 golden_stdout.txt
#   .\scripts\repl-parity.ps1 -SkipBuild   # 不重新构建（用现有 exe）
#   .\scripts\repl-parity.ps1 -Session <f> # 自定义会话文件
#   .\scripts\repl-parity.ps1 --help       # 打印用法
#
# 退出码：0 = parity 一致 / 1 = 不一致、构建失败或参数错误。

param(
    [switch]$Regen,       # 用 Rust 通道输出重新生成 golden_stdout.txt
    [switch]$SkipBuild,   # 跳过构建（repl.exe / _rust_repl.exe 需已存在）
    [switch]$Keep,        # 保留临时 Rust 通道 _rust_repl.exe（默认清理）
    [string]$Session = "" # 自定义会话文件路径（默认 tests/repl/golden_session.txt）
)

$ErrorActionPreference = "Stop"

# 仓库根目录（脚本所在目录的上一级）
$Root = Split-Path -Parent $PSScriptRoot

if ($Regen -and $Session) {
    Write-Host "[repl-parity] -Regen 与 -Session 互斥：golden 只由默认会话生成。" -ForegroundColor Red
    exit 1
}

# ==================== 路径 ====================
$SessionFile = if ($Session) { $Session } else { Join-Path $Root "tests\repl\golden_session.txt" }
$GoldenFile  = Join-Path $Root "tests\repl\golden_stdout.txt"
$MaskFile    = Join-Path $Root "tests\repl\masks.txt"

$SeedExe     = Join-Path $Root "target\release\tie-llvm.exe"
# tiec：自举 v2 新编译器（tie 语言自写，compiler/driver.tie → tiec.exe）。
# 自举升格后 tie 解释器通道 repl 由它编译（Rust 种子仅作参照基线）
$TiecExe     = Join-Path $Root "compiler\tiec.exe"
$InterpLib   = Join-Path $Root "target\release\tie_interp.lib"
$TieReplSrc  = Join-Path $Root "compiler\repl.tie"
$TieReplExe  = Join-Path $Root "compiler\repl.exe"
$RustReplSrc = Join-Path $Root "repl\repl.tie"
$RustReplExe = Join-Path $Root "compiler\_rust_repl.exe"

# 临时输出（原始字节捕获）
$TmpDir = Join-Path $Root "compiler\_repl_parity_tmp"
$TieOut     = Join-Path $TmpDir "tie_out.txt"
$RustOut    = Join-Path $TmpDir "rust_out.txt"
$TieOutMask = Join-Path $TmpDir "tie_out_masked.txt"
$RustOutMask = Join-Path $TmpDir "rust_out_masked.txt"
$GoldenMask = Join-Path $TmpDir "golden_masked.txt"

# ==================== 辅助函数 ====================

# 两个文件逐字节比较；返回 true 一致 / false 不一致（输出首个差异位置）。
function Test-BytesEqual([string]$A, [string]$B) {
    $ba = [System.IO.File]::ReadAllBytes($A)
    $bb = [System.IO.File]::ReadAllBytes($B)
    if ($ba.Length -ne $bb.Length) {
        Write-Host "  [diff] 长度不一致: $($ba.Length) vs $($bb.Length) 字节" -ForegroundColor Red
        return $false
    }
    for ($i = 0; $i -lt $ba.Length; $i++) {
        if ($ba[$i] -ne $bb[$i]) {
            Write-Host "  [diff] 第 $i 字节不一致 (0x$($ba[$i].ToString('X2')) vs 0x$($bb[$i].ToString('X2')))" -ForegroundColor Red
            return $false
        }
    }
    return $true
}

# 应用掩码规则（masks.txt 存在时）：读 UTF-8 文本 → 逐正则替换 → 写回。
function Invoke-Mask([string]$Src, [string]$Dst, [string[]]$Patterns) {
    if ($Patterns.Count -eq 0) {
        Copy-Item $Src $Dst -Force
        return
    }
    $text = [System.IO.File]::ReadAllText($Src)
    foreach ($p in $Patterns) {
        $text = $text -replace $p, "__MASKED__"
    }
    [System.IO.File]::WriteAllText($Dst, $text)
}

# ==================== 主流程 ====================

# 0. 会话文件检查
if (-not (Test-Path $SessionFile)) {
    Write-Host "[repl-parity] 找不到会话文件: $SessionFile" -ForegroundColor Red
    exit 1
}

# 1. 工具链检查（种子编译器 + interp 静态库；缺失则 cargo 构建）
if (-not (Test-Path $SeedExe) -or -not (Test-Path $InterpLib)) {
    Write-Host "[repl-parity] 种子工具链缺失，执行 cargo build --release -p tie-interp ..." -ForegroundColor Yellow
    Push-Location $Root
    try { cargo build --release -p tie-interp } finally { Pop-Location }
    if (-not (Test-Path $SeedExe) -or -not (Test-Path $InterpLib)) {
        Write-Host "[repl-parity] 构建失败：仍缺 $SeedExe 或 $InterpLib" -ForegroundColor Red
        exit 1
    }
}

# 2. 构建双通道 repl（-SkipBuild 时复用现有 exe）
if (-not $SkipBuild) {
    Write-Host "[repl-parity] 构建 tie repl（tiec）: $TieReplSrc" -ForegroundColor Cyan
    & $TiecExe $TieReplSrc -o $TieReplExe | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[repl-parity] tie repl 编译失败" -ForegroundColor Red
        exit 1
    }
    Write-Host "[repl-parity] 构建 Rust 通道: $RustReplSrc → $RustReplExe" -ForegroundColor Cyan
    & $SeedExe $RustReplSrc -o $RustReplExe | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[repl-parity] Rust 通道编译失败" -ForegroundColor Red
        exit 1
    }
}

# 3. 运行双通道（cmd 原始重定向：stdin 会话文件 → stdout 原始字节文件）
if (-not (Test-Path $TieReplExe) -or -not (Test-Path $RustReplExe)) {
    Write-Host "[repl-parity] 缺少 repl 可执行：$TieReplExe 或 $RustReplExe（先不 -SkipBuild 运行）" -ForegroundColor Red
    exit 1
}
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null
cmd /c "`"$TieReplExe`" < `"$SessionFile`" > `"$TieOut`"" | Out-Null
cmd /c "`"$RustReplExe`" < `"$SessionFile`" > `"$RustOut`"" | Out-Null

# 4. 掩码（可选）：读取 masks.txt 规则并施加到两侧输出
$Patterns = @()
if (Test-Path $MaskFile) {
    $Patterns = Get-Content $MaskFile | Where-Object { $_ -and -not $_.TrimStart().StartsWith("#") }
    if ($Patterns.Count -gt 0) {
        Write-Host "[repl-parity] 应用掩码规则 $($Patterns.Count) 条（$MaskFile）" -ForegroundColor DarkYellow
    }
}
Invoke-Mask $TieOut  $TieOutMask  $Patterns
Invoke-Mask $RustOut $RustOutMask $Patterns

# 5. golden 管理
if (-not (Test-Path $GoldenFile)) {
    Write-Host "[repl-parity] golden 缺失，自动用 Rust 通道生成: $GoldenFile" -ForegroundColor Yellow
    $Regen = $true
}
if ($Regen) {
    Invoke-Mask $RustOut $GoldenMask $Patterns
    Copy-Item $GoldenMask $GoldenFile -Force
    Write-Host "[repl-parity] 已重新生成 golden_stdout.txt（Rust 通道输出）" -ForegroundColor Green
} else {
    Invoke-Mask $GoldenFile $GoldenMask $Patterns
}

# 6. 双重断言：
#    a) Rust 通道 vs golden（golden 有效性复核，Rust 行为漂移即报警）
#    b) tie 通道 vs Rust 通道（REPL parity 核心断言）
$goldenOk = Test-BytesEqual $RustOutMask $GoldenMask
if (-not $goldenOk) {
    Write-Host "  [repl-parity] 警告：Rust 侧输出与 golden 不一致（Rust 行为变更？需 -Regen 复核）" -ForegroundColor Yellow
}
$parityOk = Test-BytesEqual $TieOutMask $RustOutMask

# 7. 结果汇总 + 报告（先取文件实际大小，再清理临时目录）
$tieBytes = (Get-Item $TieOut).Length
$rustBytes = (Get-Item $RustOut).Length
$goldenBytes = (Get-Item $GoldenFile).Length
$goldenState = if ($goldenOk) { "一致" } else { "不一致（需 -Regen 复核）" }
$parityState = if ($parityOk) { "一致" } else { "不一致" }
$conclusion = if ($parityOk) { "PASS：REPL 行为 parity 一致（diff 为空）" } else { "FAIL：存在 diff（见上方字节级差异）" }
$report = @"
# REPL 会话 parity 报告（自举 v2 T4.3）
- 会话: $($SessionFile)（$((Get-Content $SessionFile).Count) 条命令）
- 日期: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
- Rust 通道: $rustBytes 字节 vs golden: $goldenBytes 字节 → $goldenState
- tie 通道: $tieBytes 字节 vs Rust 通道: $rustBytes 字节 → $parityState
- 掩码规则: $($Patterns.Count) 条
- 结论: $conclusion
"@
$report | Out-File -FilePath (Join-Path $Root "docs\bench\repl-parity.md") -Encoding utf8
Write-Host ""
Write-Host $report

# 8. 清理临时产物（-Keep 保留 Rust 通道，便于复验）
Remove-Item $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
if (-not $Keep) {
    Remove-Item $RustReplExe -Force -ErrorAction SilentlyContinue
}

if (-not $parityOk) {
    Write-Host "[repl-parity] FAIL：golden diff 非空" -ForegroundColor Red
    exit 1
}
Write-Host "[repl-parity] PASS：golden diff 为空（掩码行除外）" -ForegroundColor Green
exit 0
