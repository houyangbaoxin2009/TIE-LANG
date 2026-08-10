# tie 自举闭环闸门脚本（阶段 3，T3.4 G2 闸门）——骨架占位
#
# 职责（待阶段 3 填充）：验证自举闭环——tiec1 = seed 编译的 compiler/driver.tie，
# tiec2 = tiec1 编译的同一份 driver.tie，断言：
#   (a) tiec1 --emit-ir 与 tiec2 --emit-ir 在 101 语料 + compiler/**/*.tie 上逐字节一致；
#   (b) tiec1 / tiec2 对语料产出 sha 一致的 .exe；
#   (c) tiec2 运行全部正例语料，行为与 Rust 编译产物一致。
#
# 用法：
#   .\scripts\selfhost-gate.ps1          # 运行 G2 闸门（阶段 3 后可用）
#   .\scripts\selfhost-gate.ps1 --help   # 打印用法
#
# 退出码：0 = 通过 / 1 = 失败

param(
    [Parameter(Position = 0)]
    [string]$Command = ""
)

$ErrorActionPreference = "Stop"

# 仓库根目录（脚本所在目录的上一级）
$Root = Split-Path -Parent $PSScriptRoot

if ($Command -in @("--help", "-h", "-?", "/?")) {
    Write-Host @"
tie 自举闭环闸门脚本（阶段 3，T3.4 G2 闸门）

用法:
  .\scripts\selfhost-gate.ps1            # 运行 G2 闸门
  .\scripts\selfhost-gate.ps1 --help     # 打印本帮助

G2 闸门断言（待阶段 3 填充）:
  (a) tiec1 --emit-ir == tiec2 --emit-ir（逐字节，101 语料 + compiler/**/*.tie）
  (b) tiec1 / tiec2 产出 sha 一致的 .exe
  (c) tiec2 运行全部正例语料，行为与 Rust 编译产物一致
"@
    exit 0
}

# TODO(阶段3 T3.4): 实现 G2 闸门：
#   1. 用 seed（target/release/tie-llvm.exe）编译 compiler/driver.tie → compiler/tiec1.exe
#   2. 用 tiec1.exe 再次编译 compiler/driver.tie → compiler/tiec2.exe
#   3. 两轮 --emit-ir 输出逐字节 diff；两轮 -o exe 比对 sha256
#   4. tiec2 全语料行为回归（stdout diff vs Rust 编译产物）
#   5. 结果写入 docs/bench/selfhost.md；失败时输出确定性排查线索
#      （排查顺序：map 顺序 → 时间戳 → 链接器参数 → env/path 泄漏）

# 当前为骨架：检测 tiec 是否已构建，未构建则提示阶段 3 尚未开始
$TiecExe = Join-Path $Root "compiler\tiec.exe"
if (-not (Test-Path $TiecExe)) {
    Write-Host "[selfhost-gate] 未找到 tiec: $TiecExe" -ForegroundColor DarkYellow
    Write-Host "[selfhost-gate] 阶段 3 尚未开始，G2 闸门逻辑待填充（当前为骨架）。" -ForegroundColor DarkGray
    exit 0
}

Write-Host "[selfhost-gate] 检测到 tiec，G2 闸门逻辑待阶段 3（T3.4）填充。" -ForegroundColor Yellow
exit 0
