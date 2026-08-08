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

/// `textDocument/definition` 请求参数（跳转定义）。
#[derive(Debug, Clone, Deserialize)]
pub struct DefinitionParams {
    /// 目标文档
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
    /// 光标位置（0-based）
    pub position: Position,
}

/// `textDocument/completion` 请求参数（自动补全）。
#[derive(Debug, Clone, Deserialize)]
pub struct CompletionParams {
    /// 目标文档
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
    /// 补全触发位置（0-based）
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

/// 服务器能力声明（v1 子集：全文同步 + hover + 跳转定义 + 补全 + 语义高亮）。
#[derive(Debug, Clone, Serialize)]
pub struct ServerCapabilities {
    /// 文本同步方式：1 = 全量同步（Full）
    #[serde(rename = "textDocumentSync")]
    pub text_document_sync: u32,
    /// 是否支持 hover
    #[serde(rename = "hoverProvider")]
    pub hover_provider: bool,
    /// 是否支持跳转定义（`textDocument/definition`）
    #[serde(rename = "definitionProvider")]
    pub definition_provider: bool,
    /// 补全能力（`textDocument/completion`），`triggerCharacters: ["."]` 触发成员补全
    #[serde(rename = "completionProvider")]
    pub completion_provider: CompletionOptions,
    /// 语义高亮能力（`textDocument/semanticTokens/full`）
    #[serde(rename = "semanticTokensProvider")]
    pub semantic_tokens_provider: SemanticTokensOptions,
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

/// `textDocument/definition` 请求的响应 result（LSP `Location`）。
///
/// 字段：uri + 定义处范围（覆盖整个名字，便于编辑器高亮）。
#[derive(Debug, Clone, Serialize)]
pub struct Location {
    /// 定义所在文档的 uri（v1 单文档：与请求同一 uri）
    pub uri: String,
    /// 定义处范围（0-based）
    pub range: Range,
}

/// 单个补全项（LSP `CompletionItem` 子集）。
///
/// `kind` / `detail` 可选：`#[serde(skip_serializing_if)]` 使缺省字段不输出
/// （VSCode 等客户端对缺失字段宽容）。
#[derive(Debug, Clone, Serialize)]
pub struct CompletionItem {
    /// 补全文本（插入到光标处的标识符）
    pub label: String,
    /// 补全项种类（LSP `CompletionItemKind`：3=Function、7=Class、14=Keyword 等）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<u32>,
    /// 补充说明（如函数签名 `func add(a: i64, b: i64) -> i64`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `textDocument/completion` 请求的响应 result（LSP `CompletionList`）。
#[derive(Debug, Clone, Serialize)]
pub struct CompletionList {
    /// 是否不完整：false = 当前列表即全部结果
    #[serde(rename = "isIncomplete")]
    pub is_incomplete: bool,
    /// 补全项列表
    pub items: Vec<CompletionItem>,
}

/// 补全能力选项（LSP `CompletionOptions`）。
///
/// `triggerCharacters`：无需 Ctrl+Space 即自动弹出的字符（`.` 触发成员补全）。
#[derive(Debug, Clone, Serialize)]
pub struct CompletionOptions {
    /// 触发字符列表
    #[serde(rename = "triggerCharacters")]
    pub trigger_characters: Vec<String>,
}

/// 语义高亮类型索引声明（LSP `SemanticTokensLegend`）。
///
/// 顺序即 `data` 中 tokenType 的数字下标（与 [crate::diagnostics] 的 st 模块
/// 常量一一对应）。只声明标识符类——关键字/类型由 TextMate 语法着色。
#[derive(Debug, Clone, Serialize)]
pub struct SemanticTokensLegend {
    /// token 类型名称列表（按下标 0 起）
    #[serde(rename = "tokenTypes")]
    pub token_types: Vec<String>,
    /// token 修饰符名称列表（v1 恒为空）
    #[serde(rename = "tokenModifiers")]
    pub token_modifiers: Vec<String>,
}

/// 语义高亮能力选项（LSP `SemanticTokensOptions` 子集）。
///
/// `full` 为 true 表示支持 `textDocument/semanticTokens/full`（全量请求；
/// 增量 delta 请求 v1 不做）。
#[derive(Debug, Clone, Serialize)]
pub struct SemanticTokensOptions {
    /// 类型图例
    pub legend: SemanticTokensLegend,
    /// 是否支持全量请求（true = 支持 `semanticTokens/full`）
    pub full: bool,
}

/// `textDocument/semanticTokens/full` 请求参数。
#[derive(Debug, Clone, Deserialize)]
pub struct SemanticTokensParams {
    /// 目标文档
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
}

/// `textDocument/semanticTokens/full` 请求的响应 result（LSP `SemanticTokens`）。
///
/// `data` 采用增量编码：每 5 个 u32 一组
/// `(deltaLine, deltaStartChar, length, tokenType, tokenModifiers)`。
#[derive(Debug, Clone, Serialize)]
pub struct SemanticTokens {
    /// 编码后的 token 数据
    pub data: Vec<u32>,
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
                definition_provider: true,
                completion_provider: CompletionOptions { trigger_characters: vec![".".into()] },
                semantic_tokens_provider: SemanticTokensOptions {
                    legend: SemanticTokensLegend {
                        token_types: vec![
                            "namespace".into(),
                            "class".into(),
                            "function".into(),
                            "method".into(),
                            "property".into(),
                            "variable".into(),
                            "parameter".into(),
                        ],
                        token_modifiers: Vec::new(),
                    },
                    full: true,
                },
            },
            server_info: ServerInfo {
                name: "tie-lsp".into(),
                version: "0.1.0".into(),
            },
        };
        let value = serde_json::to_value(&result).expect("序列化应成功");
        assert_eq!(value["capabilities"]["textDocumentSync"], serde_json::json!(1));
        assert_eq!(value["capabilities"]["hoverProvider"], serde_json::json!(true));
        assert_eq!(value["capabilities"]["definitionProvider"], serde_json::json!(true));
        assert_eq!(
            value["capabilities"]["completionProvider"]["triggerCharacters"],
            serde_json::json!(["."])
        );
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

    /// DefinitionParams / CompletionParams 反序列化：与 hover 同构（textDocument + position）。
    #[test]
    fn 定义与补全参数反序列化() {
        let raw = serde_json::json!({
            "textDocument": {"uri": "file:///a.tie"},
            "position": {"line": 3, "character": 12}
        });
        let params: DefinitionParams = serde_json::from_value(raw.clone()).expect("definition 解析应成功");
        assert_eq!(params.text_document.uri, "file:///a.tie");
        assert_eq!(params.position, Position { line: 3, character: 12 });
        let params: CompletionParams = serde_json::from_value(raw).expect("completion 解析应成功");
        assert_eq!(params.position.line, 3);
    }

    /// Location / CompletionList 序列化字段名符合 LSP 规范（uri/range/isIncomplete）。
    #[test]
    fn 定义与补全响应字段名符合规范() {
        let loc = Location {
            uri: "file:///a.tie".into(),
            range: Range {
                start: Position { line: 1, character: 5 },
                end: Position { line: 1, character: 8 },
            },
        };
        let v = serde_json::to_value(&loc).expect("序列化应成功");
        assert_eq!(v["uri"], "file:///a.tie");
        assert_eq!(v["range"]["start"]["line"], serde_json::json!(1));
        assert_eq!(v["range"]["end"]["character"], serde_json::json!(8));

        let list = CompletionList {
            is_incomplete: false,
            items: vec![CompletionItem {
                label: "func".into(),
                kind: Some(14),
                detail: None,
            }],
        };
        let v = serde_json::to_value(&list).expect("序列化应成功");
        assert_eq!(v["isIncomplete"], serde_json::json!(false));
        assert_eq!(v["items"][0]["label"], "func");
        assert_eq!(v["items"][0]["kind"], serde_json::json!(14));
        assert!(
            v["items"][0].get("detail").is_none(),
            "detail 为 None 时不应输出该字段"
        );
    }
}
