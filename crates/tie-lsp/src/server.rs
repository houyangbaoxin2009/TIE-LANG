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
//! - `textDocument/definition`：跳转定义
//! - `textDocument/completion`：自动补全
//! - `textDocument/publishDiagnostics`：服务器主动推送的诊断通知
//! 其余方法返回 MethodNotFound（-32601）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::diagnostics;
use crate::lsp::{
    CompletionList, CompletionOptions, CompletionParams, DefinitionParams,
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
        "textDocument/definition" => definition(state, id, &params),
        "textDocument/completion" => completion(state, id, &params),
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
            definition_provider: true,
            completion_provider: CompletionOptions { trigger_characters: vec![".".into()] },
        },
        server_info: ServerInfo {
            name: SERVER_NAME.into(),
            version: SERVER_VERSION.into(),
        },
    };
    let result = serde_json::to_value(result).expect("InitializeResult 序列化不应失败");
    vec![success_response(id, result)]
}

/// 从文档 uri（`file:///...`）提取源码所在目录（import 展开用）。
///
/// - Windows 路径：`file:///C:/dir/file.tie` → `C:/dir`（由 `Path` 统一处理分隔符）；
/// - 其他平台：`file:///dir/file.tie` → `/dir`；
/// - 无法解析（非 file 协议 / 无目录部分）返回 `None`，
///   调用方据此跳过 import 展开（仅做单文件分析）。
fn uri_base_dir(uri: &str) -> Option<PathBuf> {
    // 去掉 `file://` 前缀；Windows 盘符路径多一个前导 `/`（`file:///C:/...`）
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    // 文件 uri 应指向具体文件，取父目录作为 base_dir
    let path = Path::new(rest);
    path.parent().map(|p| p.to_path_buf())
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
    let result = match diagnostics::hover_markdown(
        source,
        params.position.line,
        params.position.character,
        uri_base_dir(&uri).as_deref(),
    ) {
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

/// `textDocument/definition`：跳转定义，未命中 result 为 null。
fn definition(state: &ServerState, id: Option<Value>, params: &Value) -> Vec<Value> {
    let Ok(params) = serde_json::from_value::<DefinitionParams>(params.clone()) else {
        return vec![error_response(id, ERR_INVALID_PARAMS, "无效参数：textDocument/definition")];
    };
    let uri = params.text_document.uri;
    let Some(source) = state.documents.get(&uri) else {
        // 文档不在内存：返回 null
        return vec![success_response(id, Value::Null)];
    };
    let result = match diagnostics::definition(
        source,
        params.position.line,
        params.position.character,
        uri_base_dir(&uri).as_deref(),
    ) {
        Some(range) => {
            // 定义位置：uri 与请求文档相同（v1 单文档，不做跨文件跳转）
            let loc = crate::lsp::Location { uri: uri.clone(), range };
            serde_json::to_value(loc).expect("Location 序列化不应失败")
        }
        None => Value::Null,
    };
    vec![success_response(id, result)]
}

/// `textDocument/completion`：返回补全列表（文档未打开时为空列表）。
fn completion(state: &ServerState, id: Option<Value>, params: &Value) -> Vec<Value> {
    let Ok(params) = serde_json::from_value::<CompletionParams>(params.clone()) else {
        return vec![error_response(id, ERR_INVALID_PARAMS, "无效参数：textDocument/completion")];
    };
    let uri = params.text_document.uri;
    let Some(source) = state.documents.get(&uri) else {
        // 文档不在内存：返回空列表
        let list = CompletionList { is_incomplete: false, items: Vec::new() };
        let result = serde_json::to_value(list).expect("CompletionList 序列化不应失败");
        return vec![success_response(id, result)];
    };
    let items = diagnostics::completion(
        source,
        params.position.line,
        params.position.character,
        uri_base_dir(&uri).as_deref(),
    );
    let list = CompletionList { is_incomplete: false, items };
    let result = serde_json::to_value(list).expect("CompletionList 序列化不应失败");
    vec![success_response(id, result)]
}

/// 构造 `textDocument/publishDiagnostics` 通知（对当前全文跑三阶段诊断）。
///
/// 传 base_dir（从 uri 提取）做 import 展开，跨文件命名空间调用不再误报。
fn publish_diagnostics(uri: &str, source: &str) -> Value {
    let params = PublishDiagnosticsParams {
        uri: uri.into(),
        diagnostics: diagnostics::diagnostics_for_source(source, uri_base_dir(uri).as_deref()),
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

    /// initialize 请求 → capabilities 响应（textDocumentSync=1、hoverProvider=true、
    /// definitionProvider=true、completionProvider.triggerCharacters=["."]、serverInfo）。
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
        assert_eq!(resp["result"]["capabilities"]["definitionProvider"], json!(true));
        assert_eq!(
            resp["result"]["capabilities"]["completionProvider"]["triggerCharacters"],
            json!(["."]),
            "补全触发字符应为点"
        );
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

    /// 未支持的方法（如 rename）：MethodNotFound 错误（-32601）。
    #[test]
    fn 未支持方法返回方法未找到() {
        let mut state = ServerState::default();
        let out = handle_message(&mut state, 请求(6, "textDocument/rename", json!({})));
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

    /// 定义测试源码：类（字段/静态方法/实例方法）+ 顶层函数 + main 中的变量与调用。
    fn 定义源码() -> &'static str {
        r#"class Point {
    var x: i64
    var y: i64
    static method create() -> Point {
        return Point(0, 0)
    }
    method dist() -> i64 {
        return this.x
    }
}
func add(a: i64, b: i64) -> i64 {
    return a + b
}
func main() {
    var count = 1
    var p = Point.create()
    println(add(count, 2))
    println(p.dist())
}
"#
    }

    /// definition 命中函数调用 add（line 16、character 12）→ 返回 add 定义处位置
    /// （line 10、character 5），且 uri 与请求文档一致。
    #[test]
    fn 跳转定义命中函数调用返回位置() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", 定义源码()));
        let out = handle_message(
            &mut state,
            请求(20, "textDocument/definition", json!({
                "textDocument": {"uri": "file:///a.tie"},
                "position": {"line": 16, "character": 12}
            })),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], json!(20));
        assert_eq!(out[0]["result"]["uri"], "file:///a.tie");
        assert_eq!(out[0]["result"]["range"]["start"]["line"], json!(10));
        assert_eq!(out[0]["result"]["range"]["start"]["character"], json!(5));
    }

    /// definition 命中类构造 Point（line 4、character 15）→ 返回 class Point 定义处
    /// （line 0、character 6）。
    #[test]
    fn 跳转定义命中类构造返回位置() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", 定义源码()));
        let out = handle_message(
            &mut state,
            请求(21, "textDocument/definition", json!({
                "textDocument": {"uri": "file:///a.tie"},
                "position": {"line": 4, "character": 15}
            })),
        );
        assert_eq!(out[0]["result"]["range"]["start"]["line"], json!(0));
        assert_eq!(out[0]["result"]["range"]["start"]["character"], json!(6));
    }

    /// definition 命中变量 count（line 16、character 16）→ 返回 var count 声明处
    /// （line 14、character 8）。
    #[test]
    fn 跳转定义命中变量返回声明位置() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", 定义源码()));
        let out = handle_message(
            &mut state,
            请求(22, "textDocument/definition", json!({
                "textDocument": {"uri": "file:///a.tie"},
                "position": {"line": 16, "character": 16}
            })),
        );
        assert_eq!(out[0]["result"]["range"]["start"]["line"], json!(14));
        assert_eq!(out[0]["result"]["range"]["start"]["character"], json!(8));
    }

    /// definition 未命中（光标在关键字 func 上，line 10、character 0）→ result 为 null。
    #[test]
    fn 跳转定义未命中返回null() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", 定义源码()));
        let out = handle_message(
            &mut state,
            请求(23, "textDocument/definition", json!({
                "textDocument": {"uri": "file:///a.tie"},
                "position": {"line": 10, "character": 0}
            })),
        );
        assert!(out[0]["result"].is_null(), "关键字处应返回 null");
    }

    /// definition 文档未打开 → result 为 null。
    #[test]
    fn 跳转定义文档未打开返回null() {
        let mut state = ServerState::default();
        let out = handle_message(
            &mut state,
            请求(24, "textDocument/definition", json!({
                "textDocument": {"uri": "file:///ghost.tie"},
                "position": {"line": 0, "character": 5}
            })),
        );
        assert!(out[0]["result"].is_null(), "文档未打开应返回 null");
    }

    /// completion 请求 → 补全列表（isIncomplete=false）含 func/var/println/i64。
    #[test]
    fn 补全请求返回全集列表() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", 定义源码()));
        let out = handle_message(
            &mut state,
            请求(25, "textDocument/completion", json!({
                "textDocument": {"uri": "file:///a.tie"},
                "position": {"line": 14, "character": 0}
            })),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], json!(25));
        assert_eq!(out[0]["result"]["isIncomplete"], json!(false));
        let items = out[0]["result"]["items"].as_array().expect("items 应为数组");
        let labels: Vec<&str> = items
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        assert!(labels.contains(&"func"), "应含关键词 func：{labels:?}");
        assert!(labels.contains(&"var"), "应含关键词 var：{labels:?}");
        assert!(labels.contains(&"println"), "应含内置函数 println：{labels:?}");
        assert!(labels.contains(&"i64"), "应含类型 i64：{labels:?}");
    }

    /// completion 点场景：`Point.`（line 15、character 18）→ 只补该类成员（x/y/create/dist）。
    #[test]
    fn 补全类名点后返回类成员() {
        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档("file:///a.tie", 定义源码()));
        let out = handle_message(
            &mut state,
            请求(26, "textDocument/completion", json!({
                "textDocument": {"uri": "file:///a.tie"},
                "position": {"line": 15, "character": 18}
            })),
        );
        let items = out[0]["result"]["items"].as_array().expect("items 应为数组");
        let labels: Vec<&str> = items.iter().filter_map(|i| i["label"].as_str()).collect();
        assert!(labels.contains(&"x"), "应含字段 x：{labels:?}");
        assert!(labels.contains(&"create"), "应含方法 create：{labels:?}");
        assert!(labels.contains(&"dist"), "应含方法 dist：{labels:?}");
        assert!(!labels.contains(&"func"), "点场景不应含关键词：{labels:?}");
    }

    /// completion 文档未打开 → 返回空列表（isIncomplete=false）。
    #[test]
    fn 补全文档未打开返回空列表() {
        let mut state = ServerState::default();
        let out = handle_message(
            &mut state,
            请求(27, "textDocument/completion", json!({
                "textDocument": {"uri": "file:///ghost.tie"},
                "position": {"line": 0, "character": 0}
            })),
        );
        assert_eq!(out[0]["result"]["isIncomplete"], json!(false));
        let items = out[0]["result"]["items"].as_array().expect("items 应为数组");
        assert!(items.is_empty(), "文档未打开应返回空列表");
    }

    // ==================== 真实文件（import 展开端到端） ====================

    /// 真实文件端到端：didOpen `examples/csv_demo.tie`（导入 std/csv 等命名空间库），
    /// 诊断应为空——验证跨文件命名空间调用（`str.str_split` / `csv.csv_read` 等）
    /// 经 import 展开后不再误报「未声明变量」。
    ///
    /// 路径：`CARGO_MANIFEST_DIR`（crates/tie-lsp）→ 上两级到仓库根 → examples/。
    #[test]
    fn 真实文件打开导入库示例诊断为空() {
        // 定位仓库根：crates/tie-lsp → 上两级
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("CARGO_MANIFEST_DIR 应有父目录");
        let demo = root.join("examples").join("csv_demo.tie");
        if !demo.exists() {
            eprintln!("跳过：示例文件不存在 {demo:?}");
            return;
        }
        let text = std::fs::read_to_string(&demo).expect("读取示例文件失败");
        // uri 用绝对路径（Windows 盘符形式 file:///F:/Projects/tie/...）
        let path_str = demo.to_string_lossy().replace('\\', "/");
        let uri = format!("file:///{path_str}");

        let mut state = ServerState::default();
        let out = handle_message(&mut state, 打开文档(&uri, &text));
        assert_eq!(out.len(), 1, "应推送 1 条诊断通知");
        assert_eq!(out[0]["method"], "textDocument/publishDiagnostics");
        let diags = out[0]["params"]["diagnostics"].as_array().expect("诊断应为数组");
        assert!(
            diags.is_empty(),
            "csv_demo.tie 经 import 展开后不应有诊断：{diags:?}"
        );
    }

    /// hover 端到端：对真实导入库的示例文件，hover 命中 `str.str_split` 的
    /// 第二个标识符（str_split）→ 应返回签名（跨文件函数可见）。
    #[test]
    fn 真实文件hover命名空间函数返回签名() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("CARGO_MANIFEST_DIR 应有父目录");
        let demo = root.join("examples").join("csv_demo.tie");
        if !demo.exists() {
            eprintln!("跳过：示例文件不存在 {demo:?}");
            return;
        }
        let text = std::fs::read_to_string(&demo).expect("读取示例文件失败");
        let path_str = demo.to_string_lossy().replace('\\', "/");
        let uri = format!("file:///{path_str}");

        // 找到 `str.str_split(` 实际调用位置（注释里的 `str.str_split 基础` 不参与，
        // 避免命中注释导致无 Ident token）。hover 命中第二个标识符 str_split：
        // str 占 3 字符、`.` 占 1，str_split 从 c+4 起
        let (line, col) = text
            .lines()
            .enumerate()
            .find_map(|(i, l)| {
                l.find("str.str_split(")
                    .map(|c| (i as u32, c as u32 + 4))
            })
            .unwrap_or((0, 0));

        let mut state = ServerState::default();
        handle_message(&mut state, 打开文档(&uri, &text));
        let out = handle_message(
            &mut state,
            请求(28, "textDocument/hover", json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": col}
            })),
        );
        let value = out[0]["result"]["contents"]["value"].as_str();
        assert!(
            value.is_some_and(|v| v.contains("func ")),
            "hover 应命中跨文件函数签名，实际：{value:?}"
        );
    }
}
