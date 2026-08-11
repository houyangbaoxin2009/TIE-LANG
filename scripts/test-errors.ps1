# tie 错误消息 golden 回归测试脚本（阶段 2，T2.7）
# ============================================================
# 职责：运行 tie 语义分析器（error_driver）对 tests/errors/golden/*.tie，
# 与 Rust tie-frontend 生成的 *.stderr golden 逐字节对比。
#
# 用法：
#   .\scripts\test-errors.ps1            # 全量对比
#   .\scripts\test-errors.ps1 --verbose  # 显示每个差异详情
#   .\scripts\test-errors.ps1 --regenerate  # 先重新生成 golden 再对比
#
# 退出码：0 = 0 diffs（全部一致）/ 1 = 有差异
# 先决：cargo build --release（tie-frontend + tie-llvm）

param(
    [string]$Command = "",
    [switch]$Verbose
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$GoldenDir = Join-Path $Root "tests\errors\golden"
$DriverSrc = Join-Path $Root "compiler\frontend\error_driver.tie"
$TieLlvm = Join-Path $Root "target\release\tie-llvm.exe"
$Frontend = Join-Path $Root "target\release\tie-frontend.exe"
$DriverExe = Join-Path $env:TEMP "tie-error-driver.exe"

if (-not (Test-Path $GoldenDir)) {
    Write-Host "[test-errors] 未找到 golden 目录: $GoldenDir" -ForegroundColor Red
    exit 1
}

if ($Command -eq "--regenerate") {
    & (Join-Path $PSScriptRoot "regenerate-golden.ps1")
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[test-errors] golden 再生成失败" -ForegroundColor Red
        exit 1
    }
}

if (-not (Test-Path $TieLlvm)) {
    Write-Host "[test-errors] 未找到 tie-llvm: $TieLlvm（先 cargo build --release）" -ForegroundColor Red
    exit 1
}

# 编译 tie 错误驱动（error_driver.tie → exe）
Write-Host "[test-errors] 编译 error_driver ..."
& $TieLlvm $DriverSrc -o $DriverExe 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Host "[test-errors] error_driver 编译失败" -ForegroundColor Red
    exit 1
}

# 收集 golden 文件（*.tie 且存在对应 .stderr），按文件名排序
$TieFiles = Get-ChildItem $GoldenDir -Filter "*.tie" | Where-Object {
    $stderr = Join-Path $GoldenDir ($_.BaseName + ".tie.stderr")
    Test-Path $stderr
} | Sort-Object Name

Write-Host "[test-errors] 共 $($TieFiles.Count) 个错误触发用例"

# 运行 error_driver 全量输出（无参数：逐文件打印一行，顺序 = 文件枚举顺序）
# 关键：native 程序输出 UTF-8 字节，PowerShell 重定向需显式 UTF-8 解码避免乱码
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8
$TieOut = Join-Path $env:TEMP "tie-error-out.txt"
& $DriverExe > $TieOut 2>&1
$tieLines = @(Get-Content $TieOut -Encoding utf8)

$diffCount = 0
$okCount = 0
$diffList = @()

for ($i = 0; $i -lt $TieFiles.Count; $i++) {
    $tf = $TieFiles[$i]
    $stderrPath = Join-Path $GoldenDir ($tf.BaseName + ".tie.stderr")
    $rustText = ((Get-Content $stderrPath -Raw -Encoding utf8) -replace "`r`n", "`n").Trim()

    $tieLine = ""
    if ($i -lt $tieLines.Count) {
        $tieLine = ($tieLines[$i] -replace "`r`n", "`n").Trim()
    }

    if ($tieLine -eq $rustText) {
        $okCount++
    } else {
        $diffCount++
        $diffList += $tf.Name
        if ($Verbose) {
            Write-Host "DIFF $($tf.Name):" -ForegroundColor Yellow
            Write-Host "  tie : $tieLine"
            Write-Host "  rust: $rustText"
        }
    }
}

Write-Host "[test-errors] 一致: $okCount/$($TieFiles.Count)  差异: $diffCount"
if ($diffList.Count -gt 0) {
    Write-Host "差异文件: $($diffList -join ', ')" -ForegroundColor Yellow
}

if ($diffCount -eq 0) {
    Write-Host "[test-errors] 通过：0 diffs" -ForegroundColor Green
    exit 0
} else {
    Write-Host "[test-errors] 存在 $diffCount 个差异" -ForegroundColor Red
    exit 1
}
