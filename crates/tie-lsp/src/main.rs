//! tie-lsp 入口：stdio 分帧读写循环。
//!
//! 职责：以 JSON-RPC 2.0 over stdio 方式与编辑器（如 VSCode）通信：
//! - 从 stdin 按 Content-Length 分帧读取消息；
//! - 交给 [crate::server::handle_message] 处理（纯函数，可测）；
//! - 把返回的响应/通知分帧写到 stdout；
//! - 收到 `exit` 通知（或 stdin EOF）后退出进程。
//!
//! 说明：v1 退出码从简——无论是否收到 shutdown，exit 后统一以 0 退出。

mod diagnostics;
mod jsonrpc;
mod lsp;
mod server;

use std::io::{self, BufReader, BufWriter};
use std::process::ExitCode;

use server::ServerState;

/// 服务主循环。
///
/// 每轮：读一帧 → 分发处理 → 写出所有输出消息 → 检查退出标记。
/// 读取失败或 EOF 即结束循环（EOF 表示编辑器已关闭管道）。
fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();
    // 注：Windows 上 Rust 的 stdio 默认启用二进制模式，\r\n 不会被转换，分帧安全
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut state = ServerState::default();

    loop {
        match jsonrpc::read_message(&mut reader) {
            // 正常读到一条消息：分发并写出
            Ok(Some(msg)) => {
                for out in server::handle_message(&mut state, msg) {
                    if let Err(e) = jsonrpc::write_message(&mut writer, &out) {
                        // stdout 已断（编辑器退出）：直接结束
                        eprintln!("tie-lsp：写出消息失败：{e}");
                        break;
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
                break;
            }
        }
    }
    ExitCode::from(0)
}
