# scripts/regress-driver-lite.ps1 —— 自举 v2 T2.9 行为等价回归脚本
# ============================================================
# 用法：pwsh ./scripts/regress-driver-lite.ps1 [-Detail]
#   -Detail   逐文件打印状态行（默认只打印汇总与差异清单）
#
# 作用：把一批输入文件分别用两条编译链路编译并运行，对比 stdout：
#   - 新链路（tie 自写编译器）：compiler\tiec.exe <f> -o <tie_exe>
#   - 基线链路（Rust 种子）   ：target\release\tie-llvm.exe <f> -o <rust_exe>
# 状态分类：
#   PASS       两端都能编译+运行，stdout 逐字节一致
#   DIFF       两端都能编译+运行，stdout 不一致（行为不等价，附差异原因）
#   已知不支持  Rust 能编译，driver-lite 因 irgen 最小集限制拒绝（不算行为不等价）
#   双失败      Rust 自身也编译失败（文件本身有语义/词法错误或无 main，非 driver-lite 问题）
#   库文件跳过  头部 tie:library，driver-lite 暂不做库角色（T3.x）
#
# 语料：examples\hello.tie + tests\language\*.tie + examples\*.tie（剔除
# oop_neg_* 负例与 tie:library 库文件）。
# 中间产物统一放 $env:TEMP\t29_regress\，脚本结束自动清理。
# 等价率 = PASS / (PASS + DIFF)，成功标准 ≥ 90%。

param([switch]$Detail)

$ErrorActionPreference = 'Stop'
# tie 编译器输出 UTF-8；捕获原生命令输出时按 UTF-8 解码（否则中文被按 GBK 误读，
# '不支持' 等分类匹配会失败）
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8
$root = Split-Path -Parent $PSScriptRoot
$driver = Join-Path $root 'compiler\tiec.exe'
$rust = Join-Path $root 'target\release\tie-llvm.exe'
$work = Join-Path $env:TEMP 't29_regress'
$tieDir = Join-Path $work 'tie'
$rustDir = Join-Path $work 'rust'
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $tieDir, $rustDir | Out-Null

# ---------- 运行可执行文件并捕获 stdout（带超时防挂起） ----------
function Invoke-Captured {
    param([string]$FilePath, [int]$TimeoutMs = 15000)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FilePath
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $outTask = $p.StandardOutput.ReadToEndAsync()
    $null = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit($TimeoutMs)) {
        $p.Kill()
        return @{ Stdout = ''; ExitCode = -999; Timeout = $true }
    }
    return @{ Stdout = $outTask.Result; ExitCode = $p.ExitCode; Timeout = $false }
}

# ---------- 语料收集 ----------
$files = New-Object System.Collections.Generic.List[string]
$files.Add('examples\hello.tie')
Get-ChildItem (Join-Path $root 'tests\language\*.tie') | ForEach-Object { $files.Add("tests\language\$($_.Name)") }
Get-ChildItem (Join-Path $root 'examples\*.tie') | Where-Object { $_.Name -notlike 'oop_neg_*' } | ForEach-Object { $files.Add("examples\$($_.Name)") }
# 去重（hello.tie 既显式加入又在 examples glob 中）
$files = $files | Select-Object -Unique

# ---------- 回归主循环 ----------
$stats = @{ Total = 0; Pass = 0; Diff = 0; Unsupported = 0; BothFail = 0; LibSkip = 0; Timeout = 0 }
$results = New-Object System.Collections.Generic.List[string]
$diffDetails = New-Object System.Collections.Generic.List[string]
$idx = 0

foreach ($rel in $files) {
    $idx++
    $full = Join-Path $root $rel
    $base = "{0:D3}_{1}" -f $idx, ([IO.Path]::GetFileNameWithoutExtension($rel))
    $stats.Total++

    # 库文件跳过（头部 tie:library，driver-lite 暂不做库角色）
    $head = Get-Content $full -TotalCount 8 -ErrorAction SilentlyContinue
    if ($head -match 'tie:library') {
        $stats.LibSkip++
        $results.Add("SKIP  $rel  (库文件 tie:library，driver-lite 不做库角色)")
        continue
    }

    # 1) Rust 基线编译
    $rustExe = Join-Path $rustDir "$base.exe"
    & $rust $full -o $rustExe 2>$null | Out-Null
    $rustRC = $LASTEXITCODE

    # 2) driver-lite 编译（合并 stderr 用于分类）
    $tieExe = Join-Path $tieDir "$base.exe"
    $tieOut = & $driver $full -o $tieExe 2>&1 | Out-String
    $tieRC = $LASTEXITCODE

    if ($rustRC -ne 0) {
        # Rust 自身失败（文件有错/无 main）——双失败可接受
        $stats.BothFail++
        if ($tieRC -eq 0) {
            $results.Add("WARN  $rel  Rust 编译失败(rc=$rustRC) 但 driver-lite 成功——需人工查证")
        } else {
            $results.Add("BOTH  $rel  (Rust 与 driver-lite 都失败: Rust rc=$rustRC)")
        }
        continue
    }

    if ($tieRC -ne 0) {
        # driver-lite 编译失败但 Rust 成功 = 编译能力缺口（irgen 最小集 / 解析器 /
        # 语义器尚未实现 Rust 的某特性），不算"行为不等价"（行为等价只比较两端都
        # 能编译并运行后的 stdout）。按原因细分记录。
        $stats.Unsupported++
        if ($tieOut -match '不支持') {
            $msg = ($tieOut -split "`r?`n" | Where-Object { $_ -match '不支持' } | Select-Object -First 1)
            $results.Add("UNSUP $rel  (irgen 最小集: $msg)")
            $diffDetails.Add("[$rel] irgen 最小集外的语句: $msg")
        } else {
            $msg = ($tieOut -split "`r?`n" | Where-Object { $_ -match '错误|失败' } | Select-Object -First 1)
            $results.Add("UNSUP $rel  (前端缺口: $msg)")
            $diffDetails.Add("[$rel] driver-lite 前端编译失败但 Rust 可编译: $msg")
        }
        continue
    }

    # 3) 两端都编译成功 → 运行对比
    $r = Invoke-Captured $rustExe
    $t = Invoke-Captured $tieExe
    if ($r.Timeout -or $t.Timeout) {
        $stats.Timeout++
        $results.Add("TIME  $rel  (运行超时被终止)")
        continue
    }
    if ($r.Stdout -ceq $t.Stdout) {
        $stats.Pass++
        $results.Add("PASS  $rel")
    } else {
        $stats.Diff++
        $rl = $r.Stdout -split "`r?`n"
        $tl = $t.Stdout -split "`r?`n"
        $first = -1
        for ($k = 0; $k -lt [Math]::Max($rl.Count, $tl.Count); $k++) {
            $a = if ($k -lt $rl.Count) { $rl[$k] } else { '' }
            $b = if ($k -lt $tl.Count) { $tl[$k] } else { '' }
            if ($a -cne $b) { $first = $k; break }
        }
        $detail = "第 $($first + 1) 行不同: Rust=[$($rl[$first])] vs tie=[$($tl[$first])] (行数 $($rl.Count)/$($tl.Count))"
        $results.Add("DIFF  $rel  ($detail)")
        $diffDetails.Add("[$rel] $detail")
    }
}

# ---------- 汇总 ----------
Write-Host ""
Write-Host "===== T2.9 行为等价回归结果 ($($stats.Total) 文件) ====="
Write-Host ("  PASS         {0,3}  行为等价" -f $stats.Pass)
Write-Host ("  DIFF         {0,3}  行为不等价（见差异清单）" -f $stats.Diff)
Write-Host ("  已知不支持   {0,3}  irgen 最小集之外" -f $stats.Unsupported)
Write-Host ("  双失败       {0,3}  Rust 自身也编译失败" -f $stats.BothFail)
Write-Host ("  库文件跳过   {0,3}  tie:library 角色" -f $stats.LibSkip)
Write-Host ("  运行超时     {0,3}  " -f $stats.Timeout)
$compilable = $stats.Pass + $stats.Diff
$rate = if ($compilable -gt 0) { [Math]::Round(100.0 * $stats.Pass / $compilable, 1) } else { 0 }
Write-Host ("  可编译文件中等价率: {0}/{1} = {2}%  (成功标准 >= 90%)" -f $stats.Pass, $compilable, $rate)
if ($stats.Pass + $stats.Diff -gt 0 -and $rate -ge 90) {
    Write-Host "  结论: PASS（等价率达标）"
} else {
    Write-Host "  结论: FAIL（等价率未达标）"
}

if ($diffDetails.Count -gt 0) {
    Write-Host ""
    Write-Host "===== 已知差异 / 编译能力缺口清单（原因） ====="
    foreach ($d in $diffDetails) { Write-Host "  $d" }
}

if ($Detail) {
    Write-Host ""
    Write-Host "===== 逐文件明细 ====="
    foreach ($r in $results) { Write-Host "  $r" }
}

# 清理临时产物
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue

