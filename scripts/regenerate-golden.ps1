# tie 错误消息 golden 语料再生成脚本（阶段 2，T2.7）——骨架占位
#
# 职责（待阶段 2 填充）：用 Rust seed 编译器对 compiler/tests/errors/golden/*.tie
# 逐个运行，重生成对应的 *.stderr golden 文件（Rust 错误消息为逐字对齐契约）。
# 仅在确认 Rust 消息契约未变时使用；任何消息漂移都应先经 scripts/test-errors.ps1
# 发现，再人工确认后重生成。
#
# 用法：
#   .\scripts\regenerate-golden.ps1            # 重生成全部 golden（阶段 2 后可用）
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

if ($Command -in @("--help", "-h", "-?", "/?")) {
    Write-Host @"
tie 错误消息 golden 语料再生成脚本（阶段 2，T2.7）

用法:
  .\scripts\regenerate-golden.ps1            # 重生成全部 golden
  .\scripts\regenerate-golden.ps1 --help     # 打印本帮助

说明（待阶段 2 填充）:
  对 compiler/tests/errors/golden/*.tie 逐条运行 Rust seed（tie-frontend --check），
  将其 stderr 写为同名 *.stderr golden；tiec 侧错误文本必须与其逐字节一致。
"@
    exit 0
}

# TODO(阶段2 T2.7): 实现 golden 再生成：
#   1. 枚举 compiler/tests/errors/golden/*.tie（190+ 语义/词法/语法错误触发器）
#   2. 逐个跑 target/release/tie-frontend.exe <f> --check，捕获 stderr
#   3. 写为 <f>.stderr（UTF-8 无 BOM），Rust seed 消息即逐字对齐契约
#   4. 配套 scripts/test-errors.ps1：跑 tiec vs seed 逐字节 diff

# 当前为骨架：检测 Rust seed 前端是否已构建
$FrontendExe = Join-Path $Root "target\release\tie-frontend.exe"
if (-not (Test-Path $FrontendExe)) {
    Write-Host "[regenerate-golden] 未找到 tie-frontend: $FrontendExe" -ForegroundColor DarkYellow
    Write-Host "[regenerate-golden] 请先 cargo build --release -p tie-frontend" -ForegroundColor DarkGray
    exit 1
}

Write-Host "[regenerate-golden] 检测到 tie-frontend，golden 再生成逻辑待阶段 2（T2.7）填充。" -ForegroundColor Yellow
exit 0
