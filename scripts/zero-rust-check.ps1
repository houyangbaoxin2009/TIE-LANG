# tie 0-Rust 构建/运行证明脚本（阶段 4，T4.6 G3 闸门）——骨架占位
#
# 职责（待阶段 4 填充）：证明工具链构建路径不含 cargo/target/Rust 产物：
#   - 从干净树只使用 tiec + opt/clang/lld 构建 tiec 与 repl
#   - 运行完整 REPL parity 与 interp 套件
#   - grep 运行时栈，无 Rust 符号（tie_* C ABI 符号由 tie 自写 staticlib 提供，
#     不再链接 tie_interp.lib）
#
# 用法：
#   .\scripts\zero-rust-check.ps1          # 运行 G3 证明（阶段 4 后可用）
#   .\scripts\zero-rust-check.ps1 --help   # 打印用法
#
# 退出码：0 = 通过（0-Rust 成立）/ 1 = 失败

param(
    [Parameter(Position = 0)]
    [string]$Command = ""
)

$ErrorActionPreference = "Stop"

# 仓库根目录（脚本所在目录的上一级）
$Root = Split-Path -Parent $PSScriptRoot

if ($Command -in @("--help", "-h", "-?", "/?")) {
    Write-Host @"
tie 0-Rust 构建/运行证明脚本（阶段 4，T4.6 G3 闸门）

用法:
  .\scripts\zero-rust-check.ps1            # 运行 G3 证明
  .\scripts\zero-rust-check.ps1 --help     # 打印本帮助

G3 断言（待阶段 4 填充）:
  干净树只用 tiec + opt/clang/lld 构建成功（无 cargo/target/Rust 产物）；
  REPL parity diff 为空；运行时栈无 Rust 符号。
"@
    exit 0
}

# TODO(阶段4 T4.6): 实现 G3 0-Rust 证明：
#   1. 干净树：仅用 compiler/tiec.exe + opt/clang/lld 构建 tiec、repl
#   2. 断言构建过程与产物不含 cargo/target/Rust 链接（TIE_INTERP_LIB 不再需要）
#   3. 跑完整 REPL parity（scripts/repl-parity.ps1）与 interp 套件
#   4. grep 运行时栈 Rust 符号；结果写入 docs/bench/zero-rust.md
#   失败预案（R3）：保留 Rust interp 作为可用回退，直到 parity 完全闭合

# 当前为骨架：检测 tiec 是否已构建
$TiecExe = Join-Path $Root "compiler\tiec.exe"
if (-not (Test-Path $TiecExe)) {
    Write-Host "[zero-rust-check] 未找到 tiec: $TiecExe" -ForegroundColor DarkYellow
    Write-Host "[zero-rust-check] 阶段 4 尚未开始，0-Rust 证明逻辑待填充（当前为骨架）。" -ForegroundColor DarkGray
    exit 0
}

Write-Host "[zero-rust-check] 检测到 tiec，0-Rust 证明逻辑待阶段 4（T4.6）填充。" -ForegroundColor Yellow
exit 0
