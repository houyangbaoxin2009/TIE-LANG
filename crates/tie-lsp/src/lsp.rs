//! LSP 协议类型定义（serde 结构）。
//!
//! 职责：定义本服务器用到的最小 LSP 类型子集，供 [crate::server] 解析客户端请求、
//! 构造服务器响应/通知时使用。
//!
//! 说明：
//! - 只覆盖 v1 范围（初始化、文档同步、诊断、hover），其余方法一律返回 MethodNotFound。
//! - 字段命名遵循 LSP 规范（camelCase），通过 `#[serde(rename)]` 映射。
//! - tie-frontend 不导出 serde 结构，所有转换在本 crate 内完成，不侵入其源码。
//! - 解析客户端请求时对可选/可变字段使用 `#[serde(default)]`，避免缺字段直接失败。

use serde::{Deserialize, Serialize};

// ==================== 基础位置类型 ====================

/// LSP 位置：0-based 行与字符。
///
/// 与 tie-frontend 的 [tie_frontend::lexer::Span]（1-based line/col）的换算
/// 见 [crate::diagnostics::span_to_range]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// 0-based 行号
    pub line: u32,
    /// 0-based 字符偏移
    pub character: u32,
}

/// LSP 范围：起止两个 [Position]。
///
/// v1 诊断只有错误起点，因此诊断的 range 是零宽度点（start == end）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    /// 起点
    pub start: Position,
    /// 终点
    pub end: Position,
}

/// 文档标识符（只含 uri）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    /// 文档 uri（如 `file:///path/to/a.tie`）
    pub uri: String,
}

// ==================== 客户端 → 服务器（请求/通知参数） ====================

/// `textDocument/didOpen` 通知参数。
#[derive(Debug, Clone, Deserialize)]
pub struct DidOpenTextDocumentParams {
    /// 打开的文档 (LSP 字段名 `textDocument`)
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentItem,
}

/// 已打开的文档（didOpen 的载荷）。
///
/// 说明：`languageId` / `version` 字段 v1 不使用（serde 默认忽略未知字段，
/// 客户端照常发送也无妨），故不定义，避免 dead_code 警告。
#[derive(Debug, Clone, Deserialize)]
pub struct TextDocumentItem {
    /// 文档 uri
    pub uri: String,
    /// 全文
    pub text: String,
}

/// `textDocument/didChange` 通知参数。
#[derive(Debug, Clone, Deserialize)]
pub struct DidChangeTextDocumentParams {
    /// 变更的文档（只取 uri，`version` 字段 v1 不使用）
    #[serde(rename = "textDocument")]
    pub text_document: VersionedTextDocumentIdentifier,
    /// 变更内容列表（v1 取最后一个的全量文本同步；LSP 字段名 `contentChanges`）
    #[serde(rename = "contentChanges")]
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

/// 带版本的文档标识符（v1 只使用 uri，`version` 字段不定义）。
#[derive(Debug, Clone, Deserialize)]
pub struct VersionedTextDocumentIdentifier {
    /// 文档 uri
    pub uri: String,
}

/// 单次内容变更事件。
///
/// LSP 中 `range` 可选：无 range 表示全量替换。v1 只支持全量同步，故只取 `text`。
#[derive(Debug, Clone, Deserialize)]
pub struct TextDocumentContentChangeEvent {
    /// 变更后的文档全文（全量同步时）
    pub text: String,
}

/// `textDocument/didClose` 通知参数。
#[derive(Debug, Clone, Deserialize)]
pub struct DidCloseTextDocumentParams {
    /// 关闭的文档
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
}

/// `textDocument/hover` 请求参数。
#[derive(Debug, Clone, Deserialize)]
pub struct HoverParams {
    /// 目标文档
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
    /// 悬停位置（0-based）
    pub position: Position,
}

// ==================== 服务器 → 客户端（响应/通知） ====================

/// `initialize` 请求的响应 result。
#[derive(Debug, Clone, Serialize)]
pub struct InitializeResult {
    /// 服务器能力声明
    pub capabilities: ServerCapabilities,
    /// 服务器自身信息（`serverInfo`）
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

/// 服务器能力声明（v1 子集：全文同步 + hover）。
#[derive(Debug, Clone, Serialize)]
pub struct ServerCapabilities {
    /// 文本同步方式：1 = 全量同步（Full）
    #[serde(rename = "textDocumentSync")]
    pub text_document_sync: u32,
    /// 是否支持 hover
    #[serde(rename = "hoverProvider")]
    pub hover_provider: bool,
}

/// 服务器自身信息。
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    /// 服务器名称
    pub name: String,
    /// 服务器版本
    pub version: String,
}

/// 单条诊断（publishDiagnostics 的载荷）。
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// 诊断范围（v1 为零宽度点）
    pub range: Range,
    /// 严重程度：1 = 错误（Error）
    pub severity: u32,
    /// 诊断来源（`tie`）
    pub source: String,
    /// 诊断消息（沿用 tie-frontend 原始 message）
    pub message: String,
}

/// `textDocument/publishDiagnostics` 通知参数。
#[derive(Debug, Clone, Serialize)]
pub struct PublishDiagnosticsParams {
    /// 文档 uri
    pub uri: String,
    /// 诊断列表
    pub diagnostics: Vec<Diagnostic>,
}

/// `textDocument/hover` 请求的响应 result。
#[derive(Debug, Clone, Serialize)]
pub struct Hover {
    /// Markdown 内容
    pub contents: MarkupContent,
}

/// Markdown 内容（hover 响应体）。
#[derive(Debug, Clone, Serialize)]
pub struct MarkupContent {
    /// 内容类型（固定 `markdown`）
    pub kind: String,
    /// Markdown 文本
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// InitializeResult 序列化字段名符合 LSP 规范（camelCase：serverInfo 等）。
    #[test]
    fn initialize结果字段名符合规范() {
        let result = InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: 1,
                hover_provider: true,
            },
            server_info: ServerInfo {
                name: "tie-lsp".into(),
                version: "0.1.0".into(),
            },
        };
        let value = serde_json::to_value(&result).expect("序列化应成功");
        assert_eq!(value["capabilities"]["textDocumentSync"], serde_json::json!(1));
        assert_eq!(value["capabilities"]["hoverProvider"], serde_json::json!(true));
        assert_eq!(value["serverInfo"]["name"], "tie-lsp");
    }

    /// Position/Range 序列化字段名符合 LSP 规范（line/character/start/end）。
    #[test]
    fn 位置类型字段名符合规范() {
        let pos = Position { line: 2, character: 4 };
        let v = serde_json::to_value(&pos).expect("序列化应成功");
        assert_eq!(v, serde_json::json!({"line": 2, "character": 4}));
        let range = Range { start: pos, end: pos };
        let v = serde_json::to_value(&range).expect("序列化应成功");
        assert_eq!(v["start"]["character"], serde_json::json!(4));
    }

    /// HoverParams 反序列化：标准 hover 请求参数应解析成功且位置正确。
    #[test]
    fn hover参数反序列化() {
        let raw = serde_json::json!({
            "textDocument": {"uri": "file:///a.tie"},
            "position": {"line": 0, "character": 5}
        });
        let params: HoverParams = serde_json::from_value(raw).expect("解析应成功");
        assert_eq!(params.text_document.uri, "file:///a.tie");
        assert_eq!(params.position.line, 0);
        assert_eq!(params.position.character, 5);
    }
}
