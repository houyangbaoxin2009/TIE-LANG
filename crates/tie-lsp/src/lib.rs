//! tie 语言服务器（LSP）库入口。
//!
//! 职责：把 stdio 分帧主循环提炼为可复用函数 [run_server]，供
//! tie 主命令（`tie --lsp`）与独立二进制（`tie-lsp`）共同调用。
//!
//! 模块划分：
//! - [jsonrpc]：JSON-RPC 2.0 分帧编解码（Content-Length 头 + JSON 体）
//! - [lsp]：LSP 协议类型定义（serde 结构）
//! - [diagnostics]：tie-frontend 三阶段分析 → 诊断；hover / 跳转定义 / 自动补全查询
//! - [server]：服务器状态与请求分发（纯函数，可测）

pub mod diagnostics;
pub mod jsonrpc;
pub mod lsp;
pub mod server;

use std::io::{self, BufReader, BufWriter};
use std::process::ExitCode;

/// 运行语言服务器主循环（stdio 分帧读写），直到收到 `exit` 通知或 stdin EOF。
///
/// 每轮：读一帧 → 交给 [server::handle_message] 分发 → 把返回的响应/通知
/// 分帧写出 → 检查退出标记。调用方（tie 主命令或 tie-lsp 独立二进制）
/// 直接调用本函数即可启动完整的 LSP 服务。
pub fn run_server() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();
    // 注：Windows 上 Rust 的 stdio 默认启用二进制模式，\r\n 不会被转换，分帧安全
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut state = server::ServerState::default();

    loop {
        match jsonrpc::read_message(&mut reader) {
            // 正常读到一条消息：分发并写出
            Ok(Some(msg)) => {
                for out in server::handle_message(&mut state, msg) {
                    if let Err(e) = jsonrpc::write_message(&mut writer, &out) {
                        // stdout 已断（编辑器退出）：直接结束
                        eprintln!("tie-lsp：写出消息失败：{e}");
                        return ExitCode::FAILURE;
                    }
                }
                if state.exit_requested {
                    break; // 收到 exit 通知
                }
            }
            // 输入流正常结束（EOF）：退出
            Ok(None) => break,
            // 帧损坏：记录并退出（v1 从简，不尝试恢复）
            Err(e) => {
                eprintln!("tie-lsp：读取消息失败：{e}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::from(0)
}
