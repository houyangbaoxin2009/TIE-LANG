# tie 错误消息 golden 语料再生成脚本（阶段 2，T2.7）
# ============================================================
# 职责：用 Rust seed 编译器对 tests/errors/golden/*.tie 逐个运行
# tie-frontend --check，捕获 stderr 重生成对应的 *.stderr golden 文件
# （Rust 错误消息为逐字对齐契约）。
#
# 用法：
#   .\scripts\regenerate-golden.ps1            # 重生成全部 golden
#   .\scripts\regenerate-golden.ps1 --help     # 打印用法
#
# 退出码：0 = 成功 / 1 = 失败

param(
    [Parameter(Position = 0)]
    [string]$Command = ""
)

$ErrorActionPreference = "Stop"

# 仓库根目录（脚本所在目录的上一级）
$Root = Split-Path -Parent $PSScriptRoot
$GoldenDir = Join-Path $Root "tests\errors\golden"
$FrontendExe = Join-Path $Root "target\release\tie-frontend.exe"

if ($Command -in @("--help", "-h", "-?", "/?")) {
    Write-Host @"
tie 错误消息 golden 语料再生成脚本（阶段 2，T2.7）

用法:
  .\scripts\regenerate-golden.ps1            # 重生成全部 golden
  .\scripts\regenerate-golden.ps1 --help     # 打印本帮助

说明:
  对 tests/errors/golden/*.tie 逐条运行 Rust seed（tie-frontend --check），
  将其 stderr 写为同名 *.tie.stderr golden；tiec 侧错误文本必须与其逐字节一致。
  仅在确认 Rust 消息契约未变时使用；任何消息漂移都应先经 scripts/test-errors.ps1
  发现，再人工确认后重生成。
"@
    exit 0
}

if (-not (Test-Path $FrontendExe)) {
    Write-Host "[regenerate-golden] 未找到 tie-frontend: $FrontendExe" -ForegroundColor DarkYellow
    Write-Host "[regenerate-golden] 请先 cargo build --release -p tie-frontend" -ForegroundColor DarkGray
    exit 1
}

if (-not (Test-Path $GoldenDir)) {
    Write-Host "[regenerate-golden] 未找到 golden 目录: $GoldenDir" -ForegroundColor Red
    exit 1
}

# 枚举全部触发文件（*.tie 且不是 *.stderr）
$TieFiles = Get-ChildItem $GoldenDir -Filter "*.tie" | Where-Object {
    $_.Name -notmatch "\.stderr$"
} | Sort-Object Name

Write-Host "[regenerate-golden] 共 $($TieFiles.Count) 个错误触发文件"

$okCount = 0
$noErrCount = 0

# 关键：native 程序输出 UTF-8 字节，PowerShell 捕获需显式 UTF-8 解码避免乱码
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

foreach ($tf in $TieFiles) {
    $stderrPath = Join-Path $GoldenDir ($tf.Name + ".stderr")
    # 运行 Rust seed：--check 输出错误到 stderr，成功则无输出
    $err = & $FrontendExe $tf.FullName --check 2>&1 | Out-String
    $errTrim = ($err -replace "`r`n", "`n").Trim()

    if ([string]::IsNullOrWhiteSpace($errTrim)) {
        $noErrCount++
        Write-Host "[regenerate-golden] 跳过（无错误输出）: $($tf.Name)" -ForegroundColor DarkYellow
        continue
    }

    # 写 golden（UTF-8 无 BOM，含末尾换行，与 test-errors.ps1 读法一致）
    [System.IO.File]::WriteAllText($stderrPath, $errTrim + "`n", (New-Object System.Text.UTF8Encoding($false)))
    $okCount++
}

Write-Host "[regenerate-golden] 生成 $okCount 个 golden（$noErrCount 个无错误输出跳过）"
if ($noErrCount -gt 0) {
    Write-Host "[regenerate-golden] 警告：有 $noErrCount 个触发文件未产生错误，请检查触发源" -ForegroundColor Yellow
}
Write-Host "[regenerate-golden] 完成" -ForegroundColor Green
exit 0
