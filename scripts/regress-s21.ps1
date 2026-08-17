# S2.1 字符串模型回归验证脚本（tie-s21 worktree）
# 用法: pwsh scripts/regress-s21.ps1 <tiec路径>
param(
    [Parameter(Mandatory = $true)][string]$Tiec
)
$ErrorActionPreference = "Continue"
$Root = Split-Path -Parent $PSScriptRoot
$env:TIE_INTERP_LIB = 'F:\Projects\tie\target\release\tie_interp.lib'
$pass = 0; $fail = 0; $known = 0

function Compile-Run($src, $runIt) {
    $exe = $src -replace '\.tie$', '.exe'
    & $Tiec $src -o $exe *> $null
    if ($LASTEXITCODE -ne 0) { return "COMPILE_FAIL" }
    if ($runIt) {
        $out = & $exe 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0) { return "RUN_FAIL rc=$LASTEXITCODE out=$out" }
        return "OK $out"
    }
    return "OK"
}

Write-Host "=== 1. S2.1 探针（编译+运行） ==="
$probes = @(
    'tests\s21_probe\probe1_binary.tie',
    'tests\s21_probe\probe2_len.tie',
    'tests\s21_probe\probe3_chars.tie',
    'tests\s21_probe\probe4_ffi.tie',
    'tests\s21_probe\probe5_sb.tie'
)
foreach ($p in $probes) {
    $r = Compile-Run (Join-Path $Root $p) $true
    if ($r.StartsWith("OK")) { Write-Host "PASS $p"; $pass++ }
    else { Write-Host "FAIL $p -> $r"; $fail++ }
}

Write-Host "=== 2. S2.2 探针（编译+运行） ==="
Get-ChildItem (Join-Path $Root 'tests\s22_probe') -Filter '*.tie' | ForEach-Object {
    $r = Compile-Run $_.FullName $true
    if ($r.StartsWith("OK")) { Write-Host "PASS $($_.Name)"; $pass++ }
    else { Write-Host "FAIL $($_.Name) -> $r"; $fail++ }
}

Write-Host "=== 3. S2.3 探针（编译+运行） ==="
Get-ChildItem (Join-Path $Root 'tests\s23_probe') -Filter '*.tie' | ForEach-Object {
    $r = Compile-Run $_.FullName $true
    if ($r.StartsWith("OK")) { Write-Host "PASS $($_.Name)"; $pass++ }
    else { Write-Host "FAIL $($_.Name) -> $r"; $fail++ }
}

Write-Host "=== 4. tests/language 正例（编译） ==="
Get-ChildItem (Join-Path $Root 'tests\language') -Filter '*.tie' | Where-Object {
    $_.Name -notlike '*_neg*' -and $_.Name -notlike '*.ir.tie'
} | ForEach-Object {
    if ($_.Name -in @('extern_decl.tie', 'std_fs_path.tie')) {
        Write-Host "SKIP(基线已知) $($_.Name)"; $known++
        return
    }
    $r = Compile-Run $_.FullName $false
    if ($r -eq "OK") { Write-Host "PASS $($_.Name)"; $pass++ }
    else { Write-Host "FAIL $($_.Name) -> $r"; $fail++ }
}

Write-Host "=== 5. tests/language 负例（应拒绝） ==="
Get-ChildItem (Join-Path $Root 'tests\language') -Filter '*_neg*.tie' | ForEach-Object {
    & $Tiec $_.FullName -o (Join-Path $Root 'compiler\neg_tmp.exe') *> $null
    if ($LASTEXITCODE -ne 0) { Write-Host "PASS(拒) $($_.Name)"; $pass++ }
    else { Write-Host "FAIL(未拒) $($_.Name)"; $fail++ }
}

Write-Host "=== 6. S3.1 config_smoke（编译+运行） ==="
$r = Compile-Run (Join-Path $Root 'tests\s31\config_smoke.tie') $true
if ($r.StartsWith("OK")) { Write-Host "PASS config_smoke"; $pass++ }
else { Write-Host "FAIL config_smoke -> $r"; $fail++ }

Write-Host ""
Write-Host "=== 汇总: PASS=$pass FAIL=$fail SKIP(已知基线)=$known ==="
exit ($fail -gt 0 ? 1 : 0)
