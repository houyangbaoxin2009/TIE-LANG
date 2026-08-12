# scripts/run-interp-tests.ps1 —— 自举 v2 T4.4：interp 行为测试 runner
#
# 遍历 compiler/tests/interp/*.tie（排除 _probe* 探针），逐个用 tie-llvm（Rust 种子
# 编译器）编译 + 运行，统计 PASS/FAIL。测试内部 main 用 compiler/interp/interp.tie
# 的 interp.eval() 求值并断言，退出码 0 = 通过。
#
# 分类：
#   - 正例：断言 eval 结果与 golden 一致（算术/变量/控制流/表/映射等）
#   - 负例：断言 eval 返回的错误文本与 Rust 一致（错误对齐）
#   - SKIP：标注 "SKIP" 的输出行（依赖 T4.2 或已知 interp 缺陷），不计数失败
#
# 用法：
#   .\scripts\run-interp-tests.ps1                # 运行全部 interp 测试
#   .\scripts\run-interp-tests.ps1 -Filter map    # 只跑名字含 map 的
#   .\scripts\run-interp-tests.ps1 --help         # 用法
#
# 退出码：0 = 全部通过 / 1 = 存在 FAIL

param(
    [Parameter(Position = 0)]
    [string]$Filter = "",
    [switch]$Keep
)

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
$TestDir = Join-Path $Root "compiler\tests\interp"
$Tiec = Join-Path $Root "target\release\tie-llvm.exe"
$Work = Join-Path $env:TEMP "opencode\interp-tests"
New-Item -ItemType Directory -Path $Work -Force | Out-Null

if (-not (Test-Path $Tiec)) {
    Write-Host "[run-interp-tests] 未找到编译器: $Tiec（先 cargo build --release）" -ForegroundColor Red
    exit 1
}

Write-Host "== tie interp 行为测试 runner（T4.4）==" -ForegroundColor Cyan
Write-Host "编译器: $Tiec"
Write-Host "测试目录: $TestDir"
Write-Host ""

$files = Get-ChildItem -Path $TestDir -Filter "*.tie" | Where-Object {
    $_.Name -notlike "_*" -and $_.Name -ne "runtests.tie"
} | Sort-Object Name

if ($Filter) {
    $files = $files | Where-Object { $_.Name -like "*$Filter*" }
}

if ($files.Count -eq 0) {
    Write-Host "未找到匹配的测试文件。" -ForegroundColor Yellow
    exit 0
}

$totalPass = 0
$totalFail = 0
$totalSkip = 0
$totalNeg = 0
$failList = @()

foreach ($f in $files) {
    $exe = Join-Path $Work ($f.BaseName + ".exe")
    $testName = $f.BaseName
    Write-Host ("  [{0}] 编译..." -f $testName) -NoNewline

    # 编译（tie-llvm 链接 interp 静态库）
    $compileOut = & $Tiec $f.FullName -o $exe 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host " 编译失败" -ForegroundColor Red
        $failList += $testName
        $totalFail++
        continue
    }

    # 运行
    $runOut = & $exe 2>&1
    $rc = $LASTEXITCODE

    # 统计输出中的 PASS / FAIL / SKIP 行
    $passLines = @($runOut | Where-Object { $_ -match "^PASS" })
    $failLines = @($runOut | Where-Object { $_ -match "^FAIL" })
    $skipLines = @($runOut | Where-Object { $_ -match "^SKIP" })
    # 负例（错误文本断言）单独计数：check_err 输出 "PASS  err[...]"
    $negLines = @($runOut | Where-Object { $_ -match "^PASS\s+err\[" })

    if ($rc -eq 0 -and $failLines.Count -eq 0) {
        Write-Host " 通过（正例=$($passLines.Count - $negLines.Count) 负例=$($negLines.Count) SKIP=$($skipLines.Count)）" -ForegroundColor Green
        $totalPass++
        $totalSkip += $skipLines.Count
        $totalNeg += $negLines.Count
    } else {
        Write-Host " 失败（FAIL=$($failLines.Count) PASS=$($passLines.Count)）" -ForegroundColor Red
        foreach ($fl in $failLines) { Write-Host "    $fl" -ForegroundColor DarkYellow }
        # 失败详情：打印 got/want 两行
        for ($i = 0; $i -lt $runOut.Count; $i++) {
            if ($runOut[$i] -match "^FAIL") {
                Write-Host "    $($runOut[$i])" -ForegroundColor DarkYellow
                if ($i + 1 -lt $runOut.Count -and $runOut[$i + 1] -match "^      ") {
                    Write-Host "    $($runOut[$i + 1])" -ForegroundColor DarkYellow
                }
            }
        }
        $failList += $testName
        $totalFail++
        $totalSkip += $skipLines.Count
        $totalNeg += $negLines.Count
    }

    if (-not $Keep) {
        Remove-Item $exe -Force -ErrorAction SilentlyContinue
    }
}

# ---- 汇总表格 ----
Write-Host ""
Write-Host "== 汇总 ==" -ForegroundColor Cyan
Write-Host ("  测试文件总数 : {0}" -f $files.Count)
Write-Host ("  通过 (PASS)  : {0}" -f $totalPass) -ForegroundColor Green
if ($totalFail -gt 0) {
    Write-Host ("  失败 (FAIL)  : {0}" -f $totalFail) -ForegroundColor Red
} else {
    Write-Host ("  失败 (FAIL)  : {0}" -f $totalFail) -ForegroundColor Green
}
Write-Host ("  负例断言总数 : {0}" -f $totalNeg) -ForegroundColor Cyan
Write-Host ("  SKIP 标注行  : {0}" -f $totalSkip) -ForegroundColor DarkYellow

if ($failList.Count -gt 0) {
    Write-Host ""
    Write-Host "失败清单:" -ForegroundColor Red
    foreach ($n in $failList) { Write-Host "  - $n" -ForegroundColor Red }
    Write-Host ""
    Write-Host "SKIP 说明: 依赖 T4.2（env/正则/标签/字符字面量等桥函数或已知 interp 缺陷）" -ForegroundColor DarkYellow
    exit 1
}

Write-Host ""
Write-Host "全部测试通过（SKIP 项见各测试内 SKIP 标注）。" -ForegroundColor Green
exit 0
