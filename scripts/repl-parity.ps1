# tie REPL 会话 parity 回归脚本（阶段 4，T4.3）——骨架占位
#
# 职责（待阶段 4 填充）：用同一份 golden 会话（命令 + 期望 stdout）分别跑
#   - Rust 解释器（target/release/tie-interp.exe，或 repl/repl.exe）
#   - tie 解释器（compiler/repl.exe / tiec 内嵌 interp）
# 逐字节 diff 输出，验证 REPL 行为 parity（G3 闸门的一部分）。
# 注意：golden 中环境/随机相关行需按文档掩码（env/rand 非确定性行）。
#
# 用法：
#   .\scripts\repl-parity.ps1          # 运行 REPL parity（阶段 4 后可用）
#   .\scripts\repl-parity.ps1 --help   # 打印用法
#
# 退出码：0 = parity 一致 / 1 = 不一致或失败

param(
    [Parameter(Position = 0)]
    [string]$Command = ""
)

$ErrorActionPreference = "Stop"

# 仓库根目录（脚本所在目录的上一级）
$Root = Split-Path -Parent $PSScriptRoot

if ($Command -in @("--help", "-h", "-?", "/?")) {
    Write-Host @"
tie REPL 会话 parity 回归脚本（阶段 4，T4.3）

用法:
  .\scripts\repl-parity.ps1            # 运行 REPL parity
  .\scripts\repl-parity.ps1 --help     # 打印本帮助

parity 断言（待阶段 4 填充）:
  golden 会话（命令 + 期望 stdout）分别经 Rust 解释器与 tie 解释器运行，
  逐字节 diff 输出（env/rand 非确定性行按文档掩码）。
"@
    exit 0
}

# TODO(阶段4 T4.3): 实现 REPL parity：
#   1. 定义 golden 会话：一组 repl 输入行 + 期望 stdout（tests/ 下 golden 文件）
#   2. Rust 侧: 跑 target/release/tie-interp.exe（或 repl/repl.exe），捕获 stdout
#   3. tie 侧: 跑 compiler/repl.exe，捕获 stdout
#   4. 逐字节 diff（掩码 env/rand 行），diff 为空 = parity 通过
#   5. 结果写入 docs/bench/repl-parity.md

# 当前为骨架：检测 repl 是否已构建
$ReplExe = Join-Path $Root "compiler\repl.exe"
if (-not (Test-Path $ReplExe)) {
    Write-Host "[repl-parity] 未找到 tie repl: $ReplExe" -ForegroundColor DarkYellow
    Write-Host "[repl-parity] 阶段 4 尚未开始，parity 逻辑待填充（当前为骨架）。" -ForegroundColor DarkGray
    exit 0
}

Write-Host "[repl-parity] 检测到 tie repl，parity 逻辑待阶段 4（T4.3）填充。" -ForegroundColor Yellow
exit 0
