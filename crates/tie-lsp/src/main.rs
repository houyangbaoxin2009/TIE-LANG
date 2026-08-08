//! tie-lsp 独立二进制入口：调用库入口 [tie_lsp::run_server]。
//!
//! 职责：作为可单独执行的二进制，启动语言服务器主循环。
//! 主循环逻辑在库入口（[tie_lsp::run_server]），与 tie 主命令的
//! `tie --lsp` 复用同一份实现（单点维护）。
//!
//! 额外支持 `--version` 与 `--help`：LSP 模式通过 stdio 通信，
//! 这些选项用于命令行直接调用时（不污染 stdio 协议流）。

use std::env;
use std::process::ExitCode;

/// 内部代号（架构代号）：与主入口 tie 保持一致。
const CODENAME: &str = "Harbor";

/// 正式发行版号（年份.修订号）：与主入口 tie 保持一致。
const RELEASE_VERSION: &str = "2026.1";

/// 使用说明。
const USAGE: &str = "\
tie 语言服务器（LSP over stdio，供编辑器接入）

用法:
  tie-lsp               启动语言服务器（读 stdin 的 LSP 消息、写 stdout）
  tie-lsp --version     显示版本号与内部代号
  tie-lsp -h | --help   显示本帮助
";

/// 启动语言服务器（stdio 分帧读写），退出码透传库入口结果。
fn main() -> ExitCode {
    // 启动即把 Windows 控制台切到 UTF-8，保证中文输出不乱码
    tie_frontend::init_console_utf8();

    // 命令行选项处理（仅 --version / --help，其余一律视为启动 LSP）
    let arg = env::args().nth(1);
    match arg.as_deref() {
        Some("-V" | "--version") => {
            // 组件版本号（x.y.z）+ 发行版号（年份.修订号）+ 内部代号
            println!(
                "tie-lsp {} (发行版 {} \"{}\")",
                env!("CARGO_PKG_VERSION"),
                RELEASE_VERSION,
                CODENAME
            );
            ExitCode::SUCCESS
        }
        Some("-h" | "--help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => tie_lsp::run_server(),
    }
}
