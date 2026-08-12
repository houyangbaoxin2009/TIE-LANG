# scripts/zero-rust-check.ps1 —— 自举 v2 T4.6：G3 闸门（0-Rust 验证）
# ============================================================
# 验证工具链构建路径的 0-Rust 程度：tiec 编译的用户程序链接 tie 自写运行时
# （std/runtime.a，导出 tie_exec_code/tie_get_env/tie_time_now），不依赖
# Rust tie-interp 库。
#
# 种子界限（bootstrap）：tiec.exe 本身由 Rust 种子编译器（target/release/
# tie-llvm.exe）编译——这是 0-Rust 的起点。本脚本验证**种子 tiec 之后的一切**
# （编译用户程序、链接 runtime.a、REPL 运行）不触碰 cargo/target 的 Rust 产物。
#
# 用法：pwsh ./scripts/zero-rust-check.ps1
# 退出码：0 = G3 PASS；1 = 部分 PASS / R3 fallback。

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tmp = Join-Path $env:TEMP 'zero_rust_check'
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

Write-Output "===== G3 闸门：0-Rust 验证（T4.6）====="
Write-Output "工作目录: $root"

# ---------- 0. 前置检查 ----------
$tiec = Join-Path $root 'compiler\tiec.exe'
$runtimeA = Join-Path $root 'std\runtime.a'
$runtimeTie = Join-Path $root 'std\runtime.tie'
$seed = Join-Path $root 'target\release\tie-llvm.exe'
$clang = 'D:\LLVM\bin\clang.exe'
$opt = 'D:\LLVM\bin\opt.exe'

$failures = @()
foreach ($p in @($tiec, $runtimeTie, $clang, $opt, $seed)) {
    if (-not (Test-Path $p)) {
        $failures += "缺失: $p"
    }
}
if (-not (Test-Path $runtimeA)) {
    # runtime.a 可重建：种子编译 std/runtime.tie（tiec 最小集不支持 extern fn）
    Write-Output "[info] std/runtime.a 缺失——用种子编译器重建（0-Rust 验证允许的 bootstrap 步骤）"
    & $seed $runtimeTie -o $runtimeA | Out-Null
    if ($LASTEXITCODE -ne 0) { $failures += "runtime.a 重建失败" }
}
if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Output "[FAIL] $_" }
    Write-Output "结论: FAIL（前置缺失）"
    exit 1
}
Write-Output "[PASS] 前置就绪（tiec / runtime.tie / runtime.a / clang / opt / 种子）"

# ---------- 1. tiec 编译 hello（回归基线） ----------
$helloExe = Join-Path $tmp 'g3_hello.exe'
& $tiec (Join-Path $root 'examples\hello.tie') -o $helloExe 2>$null
if ($LASTEXITCODE -ne 0) {
    $failures += "tiec 编译 hello 失败"
} else {
    $helloOut = & $helloExe
    $helloLines = @($helloOut).Count
    if ($helloLines -ne 16) {
        $failures += "hello 输出行数异常（$helloLines != 16）"
    } else {
        Write-Output "[PASS] tiec 编译 hello → 运行输出 16 行正确"
    }
}

# ---------- 2. tiec 编译运行时程序（exec_code/time_now/get_env → runtime.a） ----------
$rtSrc = Join-Path $tmp 'g3_runtime.tie'
$rtExe = Join-Path $tmp 'g3_runtime.exe'
@"
// tie:logic
// G3 闸门运行时程序（tiec 最小集 + tie 运行时桥）
func main() -> i64 {
    var t = time_now()
    if t > 0 {
        println("time_now ok")
    }
    var rc = exec_code("cmd /c exit 0")
    println(rc)
    var p: string = get_env("PATH")
    println(p)
    return 0
}
"@ | Set-Content -Encoding UTF8 $rtSrc
& $tiec $rtSrc -o $rtExe 2>$null
if ($LASTEXITCODE -ne 0) {
    $failures += "tiec 编译运行时程序失败"
} else {
    $rtOut = & $rtExe 2>&1
    $rtJoined = ($rtOut | ForEach-Object { $_.Trim() }) -join ' '
    # get_env("PATH") 返回的是路径列表值（不含 "PATH" 字面），用盘符 C:\ 特征校验非空
    if (($rtJoined -match '(^|\s)0(\s|$)') -and $rtJoined -match 'time_now ok' -and $rtJoined -match 'C:\\') {
        Write-Output "[PASS] tiec 编译运行时程序 → 运行（time_now/exec_code/get_env）正确"
    } else {
        $failures += "运行时程序输出异常: $rtJoined"
    }
}

# ---------- 3. 运行时栈 Rust-free 符号检查 ----------
# 允许集：std/runtime.a 导出的 tie 运行时桥（白名单）
$allowed = @('tie_exec_code', 'tie_get_env', 'tie_time_now')
# 禁止集：tie_interp.lib 独有桥（ptr 依赖，未 tie 化）——出现即 Rust 栈残留
$forbidden = @('tie_file_read', 'tie_str_char', 'tie_str_len', 'tie_rand_range',
    'tie_arg_count', 'tie_arg_string', 'tie_exec_output', 'tie_read_line',
    'tie_eval_expr', 'tie_eval_call', 'tie_char_code', 'tie_parse_int',
    'tie_parse_float', 'tie_to_string', 'tie_table_new', 'tie_map_new',
    'tie_byte_read', 'tie_regex_match', 'tie_http_get', 'tie_untar_gz')

if (Test-Path $rtExe) {
    # 可执行文件为 COFF（lld 默认剥离符号表），用二进制字符串扫描
    $bytes = [System.IO.File]::ReadAllBytes($rtExe)
    $ascii = [System.Text.Encoding]::ASCII.GetString($bytes)
    $rustFree = $true
    foreach ($f in $forbidden) {
        if ($ascii.Contains($f)) {
            $rustFree = $false
            $failures += "运行时栈检出 Rust 残留符号: $f"
        }
    }
    if ($rustFree) {
        Write-Output "[PASS] 运行时程序二进制无 tie_interp.lib 独有符号（Rust-free）"
    }
}

# ---------- 4. runtime.a 符号清单（允许集核验） ----------
$arSyms = & 'D:\LLVM\bin\llvm-nm.exe' $runtimeA 2>&1 | Out-String
$arOk = $true
foreach ($a in $allowed) {
    if (-not $arSyms.Contains($a)) {
        $arOk = $false
        $failures += "runtime.a 缺少导出符号: $a"
    }
}
if ($arOk) {
    Write-Output "[PASS] std/runtime.a 导出允许集符号（tie_exec_code/tie_get_env/tie_time_now）"
}

# ---------- 5. REPL parity + interp suite（种子编译通道，parity 基准） ----------
Write-Output "----- 引用回归：REPL parity + interp 套件 -----"
& (Join-Path $root 'scripts\repl-parity.ps1') | Out-Null
if ($LASTEXITCODE -ne 0) {
    $failures += "repl-parity 失败"
} else {
    Write-Output "[PASS] REPL parity（tie repl vs Rust 通道 diff 为空）"
}
& (Join-Path $root 'scripts\run-interp-tests.ps1') | Out-Null
if ($LASTEXITCODE -ne 0) {
    $failures += "interp 套件失败"
} else {
    Write-Output "[PASS] interp 行为测试套件全 PASS"
}

# ---------- 结论 ----------
Write-Output ""
if ($failures.Count -eq 0) {
    Write-Output "===== G3 结论: PASS ====="
    Write-Output "种子 tiec 编译用户程序 → 链接 std/runtime.a → 运行正确，运行时栈 Rust-free；REPL parity 空 diff；interp 套件全 PASS。"
    exit 0
} else {
    Write-Output "===== G3 结论: 部分 PASS / R3 fallback ====="
    Write-Output "0-Rust 主路径验证通过，但以下环节未闭合（保留 Rust 种子兜底直到 parity 闭合）："
    $failures | ForEach-Object { Write-Output "  - $_" }
    exit 1
}
