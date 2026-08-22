# examples/lib_math_dyn/run.ps1 —— M5 动态库 C 冒烟测试（dev33 批次 12）
# 用法: pwsh run.ps1 [tiec路径]   （默认使用仓库 compiler\tiec.exe）
# 流程：tie 库 → .dll（tiec）→ C 调用方（clang）→ 运行 + 断言
param(
    [string]$Tiec = ""
)
$ErrorActionPreference = "Stop"
$Dir = $PSScriptRoot
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if ($Tiec -eq "") { $Tiec = Join-Path $Root 'compiler\tiec.exe' }
if (-not (Test-Path $Tiec)) {
    Write-Host "未找到 tiec: $Tiec" -ForegroundColor Red
    exit 1
}

Write-Host "=== 1. tie 库编译为动态库（.dll）==="
$dll = Join-Path $Dir 'lib_math_dyn.dll'
& $Tiec (Join-Path $Dir 'lib_math_dyn.tie') -o $dll *> $null
if ($LASTEXITCODE -ne 0) { Write-Host "FAIL: tiec 编译 .dll 失败" -ForegroundColor Red; exit 1 }
Write-Host "PASS: 生成 $dll"

Write-Host "=== 2. 符号导出检查（llvm-readobj --coff-exports）==="
$ro = 'D:\LLVM\bin\llvm-readobj.exe'
if (-not (Test-Path $ro)) { $ro = 'llvm-readobj' }
$syms = & $ro --coff-exports $dll 2>&1 | Out-String
$pubSyms = @('mathdyn$add', 'mathdyn$mul', 'mathdyn$sub', 'mathdyn$max2', 'mathdyn$neg', 'mathdyn$use_private')
$nmOk = $true
foreach ($s in $pubSyms) {
    if (-not $syms.Contains($s)) { Write-Host "FAIL: 缺导出符号 $s" -ForegroundColor Red; $nmOk = $false }
}
if ($syms.Contains('mathdyn$private_helper')) {
    Write-Host "FAIL: 私有函数不应导出" -ForegroundColor Red
    $nmOk = $false
}
if ($nmOk) { Write-Host "PASS: 6 个 pub 符号导出、私有函数未导出" }

Write-Host "=== 3. C 调用方编译 + 运行 ==="
$mainExe = Join-Path $Dir 'main.exe'
$cfile = Join-Path $Dir 'main.c'
if (Test-Path $mainExe) { Remove-Item $mainExe }
& clang $cfile -o $mainExe *> $null
if ($LASTEXITCODE -ne 0) {
    Write-Host "clang 编译 main.c 失败，尝试用 TIE_LLVM_HOME 的 clang" -ForegroundColor Yellow
    $home = $env:TIE_LLVM_HOME
    if ($home -ne $null -and (Test-Path (Join-Path $home 'bin\clang.exe'))) {
        & (Join-Path $home 'bin\clang.exe') $cfile -o $mainExe *> $null
    } else {
        Write-Host "无可用 clang" -ForegroundColor Red
        exit 1
    }
}
if (-not (Test-Path $mainExe)) { Write-Host "FAIL: main.exe 未生成" -ForegroundColor Red; exit 1 }
$out = & $mainExe $dll 2>&1 | Out-String
Write-Host $out
if ($LASTEXITCODE -ne 0) { Write-Host "FAIL: C 冒烟运行失败 rc=$LASTEXITCODE" -ForegroundColor Red; exit 1 }
Write-Host "=== 冒烟测试全部通过 ==="
