//! JSON-RPC 2.0 消息分帧编解码。
//!
//! 职责：LSP 基于 JSON-RPC 2.0 over stdio，消息以「Content-Length 头 + 空行 + JSON 体」分帧。
//! 本模块负责：
//! - [read_message]：从任意 [std::io::BufRead] 读取一条分帧消息（EOF 返回 `None`）。
//! - [write_message]：向任意 [std::io::Write] 写入一条分帧消息并 flush。
//!
//! 说明：消息体统一使用 [serde_json::Value] 表示，协议层不关心具体方法语义，
//! 只保证「写出去的帧能原样读回来」，语义分发交给 [crate::server]。
//!
//! 帧格式（LSP 规范）：
//! ```text
//! Content-Length: <JSON 体的 UTF-8 字节数>\r\n
//! <其他头，忽略>\r\n
//! \r\n
//! <JSON 体，字节数 = Content-Length 声明>
//! ```

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// 读取一条分帧消息时的错误类型。
///
/// 三种来源：
/// - [FrameError::Io]：底层 IO 失败（stdin 断流等）。
/// - [FrameError::Malformed]：帧头损坏（缺 Content-Length、长度非法或超大）。
/// - [FrameError::Json]：JSON 体解析失败。
#[derive(Debug)]
pub enum FrameError {
    /// 底层 IO 失败
    Io(io::Error),
    /// 帧头格式错误（缺 Content-Length / 长度非法）
    Malformed(String),
    /// JSON 体解析失败
    Json(serde_json::Error),
}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

impl From<serde_json::Error> for FrameError {
    fn from(e: serde_json::Error) -> Self {
        FrameError::Json(e)
    }
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "IO 错误：{e}"),
            FrameError::Malformed(m) => write!(f, "帧头损坏：{m}"),
            FrameError::Json(e) => write!(f, "JSON 解析错误：{e}"),
        }
    }
}

/// 单帧 JSON 体的最大字节数（防御异常大的 Content-Length，避免一次性吃光内存）。
const MAX_FRAME_BYTES: u64 = 64 * 1024 * 1024;

/// 从 reader 读取一条分帧消息。
///
/// 流程：逐行读帧头直到空行 → 解析 `Content-Length` → 读恰好 N 字节 JSON 体。
/// 返回 `Ok(None)` 表示输入流正常结束（EOF），调用方应退出服务循环。
pub fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>, FrameError> {
    // ---- 读帧头：逐行直到空行 ----
    let mut content_length: Option<u64> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            // 读到 EOF：若一帧都没开始就是正常结束，否则是半截帧
            return if content_length.is_none() { Ok(None) } else { Err(FrameError::Malformed("帧头未以空行结束就 EOF".into())) };
        }
        // 去掉行尾 \r\n（read_line 会保留）
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // 空行 = 帧头结束
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            let raw = rest.trim();
            let len = raw
                .parse::<u64>()
                .map_err(|_| FrameError::Malformed(format!("Content-Length 非法：'{raw}'")))?;
            if len > MAX_FRAME_BYTES {
                return Err(FrameError::Malformed(format!("Content-Length 过大：{len}")));
            }
            content_length = Some(len);
        }
        // 其他头（Content-Type 等）直接忽略
    }

    // ---- 读 JSON 体：恰好 Content-Length 声明的字节数 ----
    let Some(len) = content_length else {
        return Err(FrameError::Malformed("帧头缺少 Content-Length".into()));
    };
    let buf_len = usize::try_from(len)
        .map_err(|_| FrameError::Malformed(format!("Content-Length 超出平台 usize：{len}")))?;
    let mut buf = vec![0u8; buf_len];
    reader.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

/// 向 writer 写入一条分帧消息并 flush。
///
/// Content-Length 声明的是 JSON 体的 UTF-8 字节数（`serde_json::to_vec` 的产物）。
pub fn write_message<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    // 序列化失败只可能是 Value 本身不合法（如非有限浮点数），映射为 IO 错误上报
    let body = serde_json::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("JSON 序列化失败：{e}")))?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 写→读往返：写出的帧能原样读回，且 JSON 值逐字段一致。
    #[test]
    fn 分帧写入读取往返() {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"processId": 42, "rootUri": "file:///proj"}
        });
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).expect("写入应成功");
        let mut cursor = Cursor::new(&buf);
        let back = read_message(&mut cursor).expect("读取应成功").expect("不应 EOF");
        assert_eq!(back, msg, "往返后的消息应与原消息一致");
    }

    /// Content-Length 声明的是 UTF-8 字节数：含中文的消息必须按字节数而非字符数分帧。
    #[test]
    fn 中文内容按字节数分帧() {
        // "变量类型不匹配" 等中文各占 3 字节 UTF-8，若按字符数会截断
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {"message": "变量类型不匹配：标注 i64，表达式推导为 string"}
        });
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).expect("写入应成功");
        // 解析帧头，取出 Content-Length 声明值
        let head_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("帧头应以空行结束")
            + 4;
        let head = String::from_utf8(buf[..head_end].to_vec()).expect("帧头应为 ASCII");
        let declared = head
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length:").map(|s| s.trim().parse::<usize>().expect("长度应为数字")))
            .expect("帧头应有 Content-Length");
        assert_eq!(declared, buf[head_end..].len(), "声明长度应等于 JSON 体的实际字节数");
        // 读回验证内容无损
        let mut cursor = Cursor::new(&buf);
        assert_eq!(read_message(&mut cursor).expect("读取应成功").expect("不应 EOF"), msg);
    }

    /// 连续多条消息：依次写入两条，应能依次读出且顺序不变。
    #[test]
    fn 连续多条消息顺序读取() {
        let a = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
        let b = serde_json::json!({"jsonrpc": "2.0", "method": "initialized"});
        let mut buf = Vec::new();
        write_message(&mut buf, &a).expect("写入 a 应成功");
        write_message(&mut buf, &b).expect("写入 b 应成功");
        let mut cursor = Cursor::new(&buf);
        let back_a = read_message(&mut cursor).expect("读 a 应成功").expect("a 不应 EOF");
        let back_b = read_message(&mut cursor).expect("读 b 应成功").expect("b 不应 EOF");
        let tail = read_message(&mut cursor).expect("读尾部应成功");
        assert_eq!(back_a, a, "第一条消息应一致");
        assert_eq!(back_b, b, "第二条消息应一致");
        assert!(tail.is_none(), "读完后应返回 EOF");
    }

    /// 帧头可含其他字段（如 Content-Type），应忽略并正常解析。
    #[test]
    fn 帧头含其他字段被忽略() {
        let body = r#"{"jsonrpc":"2.0","id":7,"method":"shutdown"}"#;
        let mut frame = format!("Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n", body.len()).into_bytes();
        frame.extend_from_slice(body.as_bytes());
        let mut cursor = Cursor::new(&frame);
        let msg = read_message(&mut cursor).expect("读取应成功").expect("不应 EOF");
        assert_eq!(msg["method"], "shutdown");
    }

    /// 帧头缺 Content-Length：应报帧头损坏错误。
    #[test]
    fn 帧头缺内容长度报错() {
        let frame = b"Content-Type: text/plain\r\n\r\n{}".to_vec();
        let mut cursor = Cursor::new(&frame);
        let err = read_message(&mut cursor).expect_err("缺少 Content-Length 应报错");
        assert!(matches!(err, FrameError::Malformed(_)), "应为帧头损坏：{err}");
    }

    /// 空输入流：立即返回 EOF（None）。
    #[test]
    fn 空输入流返回文件结束() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let msg = read_message(&mut cursor).expect("空流读取不应报错");
        assert!(msg.is_none(), "空流应返回 EOF");
    }
}
