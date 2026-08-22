# scripts/regress-m5-dynlib.ps1 —— M5 动态库编译回归（dev33 批次 12）
# 用法: pwsh scripts/regress-m5-dynlib.ps1 [tiec路径]
# 覆盖：.dll 编译 / 导出面（pub 导出、私有不导出）/ C 冒烟（LoadLibrary+GetProcAddress）
#       / 边界负例（--shared + 表/struct 违例应拒）/ --shared+logic 角色应拒 /
#       / 静态库编译无回归。
param(
    [string]$Tiec = ""
)
$ErrorActionPreference = "Continue"
$Root = Split-Path -Parent $PSScriptRoot
if ($Tiec -eq "") { $Tiec = Join-Path $Root 'compiler\tiec.exe' }
$script:pass = 0; $script:fail = 0
function Report($ok, $msg) {
    if ($ok) { Write-Host "PASS $msg"; $script:pass++ }
    else { Write-Host "FAIL $msg"; $script:fail++ }
}

# 找 llvm 工具
$ro = 'D:\LLVM\bin\llvm-readobj.exe'
if (-not (Test-Path $ro)) { $ro = 'llvm-readobj' }

$Lib = Join-Path $Root 'examples\lib_math_dyn'
$Dll = Join-Path $Lib 'lib_math_dyn.dll'
$Tmp = Join-Path $env:TEMP 'm5_dynlib'
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
$TmpDll = Join-Path $Tmp 'lib_math_dyn.dll'
if (Test-Path $TmpDll) { Remove-Item $TmpDll }

Write-Host "=== M5 动态库回归（tiec=$Tiec）==="

# ---- 1. .dll 编译（-o .dll 扩展名触发动态库模式）----
& $Tiec (Join-Path $Lib 'lib_math_dyn.tie') -o $TmpDll *> $null
Report ($LASTEXITCODE -eq 0) "步骤1：tie 库编译为 .dll（rc=$LASTEXITCODE）"

# ---- 2. 导出面检查（pub 6 符号导出；私有函数不导出）----
if (Test-Path $TmpDll) {
    $syms = & $ro --coff-exports $TmpDll 2>&1 | Out-String
    $pub = @('mathdyn$add', 'mathdyn$mul', 'mathdyn$sub', 'mathdyn$max2', 'mathdyn$neg', 'mathdyn$use_private')
    $allPub = $true
    foreach ($s in $pub) { if (-not $syms.Contains($s)) { $allPub = $false } }
    $privHidden = -not $syms.Contains('mathdyn$private_helper')
    Report ($allPub -and $privHidden) "步骤2：6 个 pub 符号导出、私有函数未导出"
} else {
    Report $false "步骤2：无 .dll 可检查导出面"
}

# ---- 3. C 冒烟（LoadLibrary + GetProcAddress）----
if (Test-Path $TmpDll) {
    $mainExe = Join-Path $Tmp 'main.exe'
    if (Test-Path $mainExe) { Remove-Item $mainExe }
    & clang (Join-Path $Lib 'main.c') -o $mainExe *> $null
    if (Test-Path $mainExe) {
        $out = & $mainExe $TmpDll 2>&1 | Out-String
        Report ($LASTEXITCODE -eq 0 -and $out.Contains('=== C 冒烟全部通过 ===')) "步骤3：C LoadLibrary/GetProcAddress 调用全部通过"
    } else {
        Report $false "步骤3：main.c 编译失败（clang 不可用？）"
    }
} else {
    Report $false "步骤3：无 .dll 可做 C 冒烟"
}

# ---- 4. 边界负例：--shared + 导出表参数/struct 返回应拒 ----
& $Tiec (Join-Path $Root 'tests\m5_dynlib\dynlib_boundary_neg.tie') --shared -o (Join-Path $Tmp 'bnd.dll') *> $null
Report ($LASTEXITCODE -ne 0) "步骤4：边界负例（导出 table 参数/struct 返回）被拒绝"

# ---- 5. --shared + logic 角色应拒 ----
& $Tiec (Join-Path $Root 'examples\hello.tie') --shared -o (Join-Path $Tmp 'x.dll') *> $null
Report ($LASTEXITCODE -ne 0) "步骤5：--shared 非 library 角色被拒绝"

# ---- 6. 静态库编译无回归（无 --shared 走 .a）----
$TmpA = Join-Path $Tmp 'lib_math_dyn.a'
if (Test-Path $TmpA) { Remove-Item $TmpA }
& $Tiec (Join-Path $Lib 'lib_math_dyn.tie') -o $TmpA *> $null
Report ($LASTEXITCODE -eq 0) "步骤6：静态库 .a 编译无回归（rc=$LASTEXITCODE）"

Write-Host ""
Write-Host "=== 汇总: PASS=$pass FAIL=$fail ==="
exit ($fail -gt 0 ? 1 : 0)
