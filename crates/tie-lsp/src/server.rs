//! 服务器状态与请求分发（核心逻辑，可测纯函数）。
//!
//! 职责：维护服务器状态（文档内存、生命周期标记），把每条 JSON-RPC 消息
//! 分发到对应处理方法，返回「要发给客户端的消息列表」（响应与通知），
//! 分帧写出交给 [crate::main]。
//!
//! 设计：核心入口 [handle_message] 是纯函数风格的——不直接碰 stdio，
//! 输入 [serde_json::Value]、输出消息列表，因此单元测试无需真实 IO。
//!
//! 支持的方法（v1 范围）：
//! - `initialize` / `initialized` / `shutdown` / `exit`：生命周期
//! - `textDocument/didOpen` / `didChange` / `didClose`：文档同步（全量）
//! - `textDocument/hover`：hover 查询
//! - `textDocument/publishDiagnostics`：服务器主动推送的诊断通知
//! 其余方法返回 MethodNotFound（-32601）。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::diagnostics;
use crate::lsp::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    HoverParams, InitializeResult, PublishDiagnosticsParams, ServerCapabilities, ServerInfo,
};

/// JSON-RPC 错误码：方法未找到。
const ERR_METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC 错误码：无效请求。
const ERR_INVALID_REQUEST: i64 = -32600;
/// JSON-RPC 错误码：无效参数。
const ERR_INVALID_PARAMS: i64 = -32602;

/// 服务器名称（initialize 响应与 serverInfo 使用）。
const SERVER_NAME: &str = "tie-lsp";
/// 服务器版本（与 Cargo.toml 同步）。
const SERVER_VERSION: &str = "0.1.0";

/// 文本同步方式：1 = 全量同步（Full）。
const TEXT_DOCUMENT_SYNC_FULL: u32 = 1;

/// 服务器状态。
#[derive(Default)]
pub struct ServerState {
    /// 文档内存：uri → 全文（v1 单文档也用它，多文档天然支持）
    pub documents: HashMap<String, String>,
    /// 是否收到 `exit` 通知（main 循环据此退出进程）
    pub exit_requested: bool,
    /// 是否收到 `shutdown` 请求（v1 仅记录，退出码从简）
    pub shutdown_requested: bool,
}

/// 处理一条 JSON-RPC 消息，返回要发送给客户端的消息列表。
///
/// 规则：
/// - 请求（含 id）→ 返回响应（成功或错误）；
/// - 通知（无 id）→ 返回空列表或推送的通知（如 publishDiagnostics）；
/// - `exit` 通知 → 置 `state.exit_requested`，由 main 循环退出。
pub fn handle_message(state: &mut ServerState, msg: Value) -> Vec<Value> {
    // 提取 method；缺失视为无效请求
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return vec![error_response(None, ERR_INVALID_REQUEST, "无效请求：缺少 method")];
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let id = msg.get("id").cloned();

    match method {
        "initialize" => initialize(state, id),
        "initialized" => Vec::new(), // 忽略
        "shutdown" => {
            state.shutdown_requested = true;
            vec![success_response(id, Value::Null)]
        }
        "exit" => {
            state.exit_requested = true;
            Vec::new()
        }
        "textDocument/didOpen" => did_open(state, &params),
        "textDocument/didChange" => did_change(state, &params),
        "textDocument/didClose" => did_close(state, &params),
        "textDocument/hover" => hover(state, id, &params),
        _ => vec![error_response(id, ERR_METHOD_NOT_FOUND, &format!("方法未找到：{method}"))],
    }
}

/// `initialize`：返回能力声明与服务器信息。
fn initialize(state: &mut ServerState, id: Option<Value>) -> Vec<Value> {
    // 记录初始化（v1 不做内容校验，能力固定）
    let _ = state;
    let result = InitializeResult {
        capabilities: ServerCapabilities {
            text_document_sync: TEXT_DOCUMENT_SYNC_FULL,
            hover_provider: true,
        },
        server_info: ServerInfo {
            name: SERVER_NAME.into(),
            version: SERVER_VERSION.into(),
        },
    };
    let result = serde_json::to_value(result).expect("InitializeResult 序列化不应失败");
    vec![success_response(id, result)]
}

/// `textDocument/didOpen`：全文存入内存，并立即推送诊断。
fn did_open(state: &mut ServerState, params: &Value) -> Vec<Value> {
    let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(params.clone()) else {
        return Vec::new(); // 参数损坏：v1 忽略，不推送
    };
    let uri = params.text_document.uri;
    let text = params.text_document.text;
    state.documents.insert(uri.clone(), text.clone());
    vec![publish_diagnostics(&uri, &text)]
}

/// `textDocument/didChange`：全量同步（取最后一个 contentChanges[i].text 替换全文）。
fn did_change(state: &mut ServerState, params: &Value) -> Vec<Value> {
    let Ok(params) = serde_json::from_value::<DidChangeTextDocumentParams>(params.clone()) else {
        return Vec::new();
    };
    let uri = params.text_document.uri;
    let Some(last) = params.content_changes.last() else {
        return Vec::new(); // 无变更内容：v1 忽略
    };
    let text = last.text.clone();
    state.documents.insert(uri.clone(), text.clone());
    vec![publish_diagnostics(&uri, &text)]
}

/// `textDocument/didClose`：清除文档内存，并推送空诊断（清掉旧错误标记）。
fn did_close(state: &mut ServerState, params: &Value) -> Vec<Value> {
    let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(params.clone()) else {
        return Vec::new();
    };
    let uri = params.text_document.uri;
    state.documents.remove(&uri);
    vec![empty_diagnostics(&uri)]
}

/// `textDocument/hover`：命中函数名/类名返回 Markdown 签名，否则 result 为 null。
fn hover(state: &ServerState, id: Option<Value>, params: &Value) -> Vec<Value> {
    let Ok(params) = serde_json::from_value::<HoverParams>(params.clone()) else {
        return vec![error_response(id, ERR_INVALID_PARAMS, "无效参数：textDocument/hover")];
    };
    let uri = params.text_document.uri;
    let Some(source) = state.documents.get(&uri) else {
        // 文档不在内存：返回 null
        return vec![success_response(id, Value::Null)];
    };
    let result = match diagnostics::hover_markdown(source, params.position.line, params.position.character) {
        Some(value) => {
            // 命中：返回 markdown 内容
            let hover = crate::lsp::Hover {
                contents: crate::lsp::MarkupContent { kind: "markdown".into(), value },
            };
            serde_json::to_value(hover).expect("Hover 序列化不应失败")
        }
        None => Value::Null,
    };
    vec![success_response(id, result)]
}

/// 构造 `textDocument/publishDiagnostics` 通知（对当前全文跑三阶段诊断）。
fn publish_diagnostics(uri: &str, source: &str) -> Value {
    let params = PublishDiagnosticsParams {
        uri: uri.into(),
        diagnostics: diagnostics::diagnostics_for_source(source),
    };
    let params = serde_json::to_value(params).expect("诊断通知序列化不应失败");
    json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": params})
}

/// 构造空诊断通知（didClose 时清除旧错误标记）。
fn empty_diagnostics(uri: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {"uri": uri, "diagnostics": []}
    })
}

/// 构造成功响应：`{"jsonrpc":"2.0","id":<id>,"result":<result>}`。
fn success_response(id: Option<Value>, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// 构造错误响应：`{"jsonrpc":"2.0","id":<id>,"error":{"code":<code>,"message":<msg>}}`。
fn error_response(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条带 id 的请求。
    fn 请求(id: u64, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    /// 构造一条无 id 的通知。
    fn 通知(method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "method": method, "params": params})
    }

    /// didOpen 通知参数。
    fn 打开文档(uri: &str, text: &str) -> Value {
        通知(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "tie", "version": 1, "text": text}}),
        )
    }

    /// 合法源码（func add + main）。
    fn 合法源码() -> &'static str {
        "func add(a: i64, b: i64) -> i64 {\n    return a + b\n}\nfunc main() {\n    println(add(1, 2))\n}\n"
    }

    /// initialize 请求 → capabilities 响应（textDocumentSync=1、hoverProvider=true、serverInfo）。
    #[test]
    fn initialize请求返回capabilities() {
        let mut state = ServerState::default();
        let out = handle_message(&mut state, 请求(1, "initialize", json!({})));
        assert_eq!(out.len(), 1, "应返回 1 条响应");
        let resp = &out[0];
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], json!(1));
        assert_eq!(resp["result"]["capabilities"]["textDocumentSync"], json!(1));
        assert_eq!(resp["result"]["capabilities"]["hoverProvider"], json!(true));
        assert_eq!(resp["result"]["serverInfo"]["name"], "tie-lsp");
        assert_eq!(resp["result"]["serverInfo"]["version"], "0.1.0");
    }

    /// initialized 通知：忽略，不产生任何输出。
    #[test]
    fn initialized通知被忽略() {
        let mut state = ServerState::default();
        let out = handle_message(&mut state, 通知("initialized", json!({})));
        assert!(out.is_empty(), "initialized 不应有输出");
    }

    /// shutdown 请求 → result: null。
    #[test]
    fn shutdown返回null() {
        let mut state = ServerState::default();
        let out = handle_message(&mut state, 请求(2, "shutdown", json!({})));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], json!(2));
        assert!(out[0]["result"].is_null(), "shutdown 响应 result 应为 null");
        assert!(state.shutdown_requested, "应记录 shutdown");
    }

    /// exit 通知：置 exit_requested，无输出。
    #[test]
    fn exit通知请求退出() {
        let mut state = ServerState::default();
        let out = handle_message(&mut state, 通知("exit", json!({})));
        assert!(out.is_empty(), "exit 不应有输出");
        assert!(state.exit_requested, "应请求退出");
    }

    /// didOpen 含语法错误源码 → publishDiagnostics 通知含 1 条诊断（severity 1、source tie）。
    #[test]
    fn 打开文档语法错误推送诊断() {
        let mut state = ServerState::default();
        let out = handle_message(&mut state, 打开文档("file:///bad.tie", "var x = 1"));
        assert_eq!(out.len(), 1, "应推送 1 条通知");
        let notif = &out[0];
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        assert_eq!(notif["params"]["uri"], "file:///bad.tie");
        let diags = notif["params"]["diagnostics"].as_array().expect("诊断应为数组");
        assert_eq!(diags.len(), 1, "语法错误应 1 条诊断");
        assert_eq!(diags[0]["severity"], json!(1));
        assert_eq!(diags[0]["source"], "tie");
        let msg = diags[0]["message"].as_str().expect("消息应为字符串");
        assert!(msg.contains("顶层只允许"), "消息应沿用原始 message：{msg}");
    }

    /// didOpen 合法源码 → publishDiagnostics 通知（空诊断数组）。
    #[test]
    fn 打开文档合法源码诊断为空() {
        let mut state = ServerState::default();
        let out = handle_message(&mut state, 打开文档("file:///good.tie", 合法源码()));
        assert_eq!(out.len(), 1);
        let notif = &out[0];
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        assert!(notif["params"]["diagnostics"].as_array().expect("诊断应为数组").is_empty(),
            "合法源码不应有诊断");
    }

    /// didChange 全量替换：内容更新后重新推送诊断。
    #[test]
    fn 变更文档全量替换并推送诊断() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", 合法源码()));
        // 改成错误源码：取最后一个 contentChanges 的 text
        let out = handle_message(
            &mut state,
            通知(
                "textDocument/didChange",
                json!({
                    "textDocument": {"uri": "file:///a.tie", "version": 2},
                    "contentChanges": [
                        {"text": "func main() {\n    println(1)\n}\n"},
                        {"text": "var x = 1"}
                    ]
                }),
            ),
        );
        assert_eq!(out.len(), 1);
        let diags = out[0]["params"]["diagnostics"].as_array().expect("诊断应为数组");
        assert_eq!(diags.len(), 1, "应使用最后一个变更内容的诊断");
        assert_eq!(state.documents["file:///a.tie"], "var x = 1", "内存应替换为最后一个变更文本");
    }

    /// didClose：清除文档内存并推送空诊断。
    #[test]
    fn 关闭文档清除内容并推送空诊断() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", "var x = 1"));
        assert!(state.documents.contains_key("file:///a.tie"));
        let out = handle_message(
            &mut state,
            通知("textDocument/didClose", json!({"textDocument": {"uri": "file:///a.tie"}})),
        );
        assert!(state.documents.is_empty(), "关闭后应清除文档");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["method"], "textDocument/publishDiagnostics");
        assert!(out[0]["params"]["diagnostics"].as_array().expect("诊断应为数组").is_empty(),
            "关闭应推送空诊断");
    }

    /// hover 命中函数名：返回签名 Markdown。
    #[test]
    fn hover命中函数名返回签名() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", 合法源码()));
        // "func add" 中 add 在 line 0、character 5
        let out = handle_message(
            &mut state,
            请求(3, "textDocument/hover", json!({
                "textDocument": {"uri": "file:///a.tie"},
                "position": {"line": 0, "character": 5}
            })),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], json!(3));
        let value = out[0]["result"]["contents"]["value"].as_str().expect("应为 markdown 文本");
        assert_eq!(out[0]["result"]["contents"]["kind"], "markdown");
        assert!(value.contains("**函数签名**"), "应含签名标记：{value}");
        assert!(value.contains("func add(a: i64, b: i64) -> i64"), "签名格式不符：{value}");
    }

    /// hover 未命中（关键字/空白处）：result 为 null。
    #[test]
    fn hover未命中返回null() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", 合法源码()));
        let out = handle_message(
            &mut state,
            请求(4, "textDocument/hover", json!({
                "textDocument": {"uri": "file:///a.tie"},
                "position": {"line": 0, "character": 0}
            })),
        );
        assert!(out[0]["result"].is_null(), "未命中应返回 null");
    }

    /// hover 文档不存在（未 didOpen）：result 为 null。
    #[test]
    fn hover文档未打开返回null() {
        let mut state = ServerState::default();
        let out = handle_message(
            &mut state,
            请求(5, "textDocument/hover", json!({
                "textDocument": {"uri": "file:///ghost.tie"},
                "position": {"line": 0, "character": 5}
            })),
        );
        assert!(out[0]["result"].is_null(), "文档未打开应返回 null");
    }

    /// 未支持的方法（如 goto-definition）：MethodNotFound 错误（-32601）。
    #[test]
    fn 未支持方法返回方法未找到() {
        let mut state = ServerState::default();
        let out = handle_message(&mut state, 请求(6, "textDocument/definition", json!({})));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], json!(6));
        assert_eq!(out[0]["error"]["code"], json!(-32601));
        assert!(out[0]["result"].is_null(), "错误响应不应有 result");
    }

    /// 缺 method 的消息：无效请求错误（-32600）。
    #[test]
    fn 缺method返回无效请求() {
        let mut state = ServerState::default();
        let out = handle_message(&mut state, json!({"jsonrpc": "2.0", "id": 7}));
        assert_eq!(out[0]["error"]["code"], json!(-32600));
    }
}
