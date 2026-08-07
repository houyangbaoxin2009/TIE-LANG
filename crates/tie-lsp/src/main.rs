//! tie-lsp 独立二进制入口：调用库入口 [tie_lsp::run_server]。
//!
//! 职责：作为可单独执行的二进制，启动语言服务器主循环。
//! 主循环逻辑在库入口（[tie_lsp::run_server]），与 tie 主命令的
//! `tie --lsp` 复用同一份实现（单点维护）。

use std::process::ExitCode;

/// 启动语言服务器（stdio 分帧读写），退出码透传库入口结果。
fn main() -> ExitCode {
    tie_lsp::run_server()
}
