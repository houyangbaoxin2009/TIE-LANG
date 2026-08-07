//! tie-frontend 集成：三阶段错误 → LSP 诊断；hover 查询。
//!
//! 职责：把 tie-frontend 的三阶段分析（词法/语法/语义）接入 LSP：
//! - [diagnostics_for_source]：对源码执行词法 → 语法 → 语义，任一阶段首个错误
//!   转成一条 LSP 诊断（v1 fail-fast，这是 tie-frontend 现有 API 的限制）。
//! - [hover_markdown]：给定 0-based 位置，命中标识符后到语义结果查询函数/类签名。
//! - [span_to_range] / [ty_to_str]：1-based Span → 0-based Range、TypeSpec → 文本。
//!
//! 说明：tie-frontend 不导出 serde 结构，这里统一转换为本 crate 的
//! [crate::lsp] 类型，不修改 tie-frontend 源码。

use tie_frontend::ast::{Stmt, TypeSpec};
use tie_frontend::lexer::{tokenize, Span, TokenKind};
use tie_frontend::parser::parse_program;
use tie_frontend::semantic::analyze;

use crate::lsp::{Diagnostic, Position, Range};

/// 诊断严重程度：错误（LSP 规范 1 = Error）。
const SEVERITY_ERROR: u32 = 1;

/// 诊断来源：`tie`。
const SOURCE_NAME: &str = "tie";

/// 对源码执行三阶段分析（fail-fast），生成 LSP 诊断列表。
///
/// 流程（任一阶段失败即停止收集）：
/// 1. [tokenize] 词法 → `LexError` 时生成 1 条诊断；
/// 2. [parse_program] 语法 → `ParseError` 时生成 1 条诊断；
/// 3. [analyze] 语义 → `SemanticError` 时生成 1 条诊断。
///
/// 全部成功返回空列表（无诊断）。错误消息沿用 tie-frontend 原始 message 文本。
pub fn diagnostics_for_source(source: &str) -> Vec<Diagnostic> {
    // 阶段一：词法分析
    let tokens = match tokenize(source) {
        Ok(tokens) => tokens,
        Err(err) => return vec![one_diagnostic(err.span, err.message)],
    };
    // 阶段二：语法分析
    let program = match parse_program(&tokens) {
        Ok(program) => program,
        Err(err) => return vec![one_diagnostic(err.span, err.message)],
    };
    // 阶段三：语义分析
    if let Err(err) = analyze(&program) {
        return vec![one_diagnostic(err.span, err.message)];
    }
    // 全部通过：无诊断
    Vec::new()
}

/// 用单个错误位置与消息构造一条零宽度诊断。
fn one_diagnostic(span: Span, message: String) -> Diagnostic {
    Diagnostic {
        range: span_to_range(span),
        severity: SEVERITY_ERROR,
        source: SOURCE_NAME.into(),
        message,
    }
}

/// 位置转换：tie-frontend 的 1-based Span → LSP 的 0-based Range。
///
/// 换算：LSP line = span.line - 1，LSP character = span.col - 1。
/// v1 错误只有起点，故 range 是零宽度点（start == end）。
/// 用 saturating_sub 防御 0 值（正常输入 line/col ≥ 1，不会触发）。
pub fn span_to_range(span: Span) -> Range {
    let pos = Position {
        line: span.line.saturating_sub(1),
        character: span.col.saturating_sub(1),
    };
    Range { start: pos, end: pos }
}

/// 类型转文本：把 TypeSpec 转成 tie 语法中的类型写法。
///
/// - `Named(TyKw)` → 类型关键字文本（如 `i64`、`string`）；
/// - `Class(name)` → 类名；
/// - `Tuple(fields)` → `(T1, T2)`，命名字段为 `(x: T1)`。
pub fn ty_to_str(ty: &TypeSpec) -> String {
    match ty {
        TypeSpec::Named(kw) => kw.as_str().to_string(),
        TypeSpec::Class(name) => name.clone(),
        TypeSpec::Tuple(fields) => {
            let inner = fields
                .iter()
                .map(|f| match &f.name {
                    Some(n) => format!("{n}: {}", ty_to_str(&f.ty)),
                    None => ty_to_str(&f.ty),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
    }
}

/// hover 查询：给定 0-based 位置，命中函数名/类名时返回 Markdown 签名。
///
/// 流程：
/// 1. 重新词法分析，找出与位置匹配的 `Ident` token（span 换算回 1-based 比较，
///    token 长度取名字的 UTF-8 字节数，v1 接受中文标识符的字节近似）；
/// 2. 命中后执行语义分析：
///    - `sem.funcs` 含该名字 → `**函数签名**：func name(param: Ty, ...) -> Ret`
///      （参数名取自 AST 同名函数定义，类型来自 `FuncSig.param_tys`）；
///    - `sem.classes` 含该名字 → `**类**：class Name`（有 extends 则追加）；
///    - 都没命中返回 `None`（hover result 为 null）。
pub fn hover_markdown(source: &str, line: u32, character: u32) -> Option<String> {
    // 命中标识符：0-based 位置还原为 1-based 再与 token.span 比较
    let tokens = tokenize(source).ok()?;
    let target_line = line.saturating_add(1);
    let target_col = character.saturating_add(1);
    let ident = tokens.iter().find(|t| {
        let TokenKind::Ident(name) = &t.kind else { return false };
        // 字节长度转 u32（64 位平台恒成功；异常时按最大处理）
        let len = u32::try_from(name.len()).unwrap_or(u32::MAX);
        t.span.line == target_line
            && t.span.col <= target_col
            && t.span.col.saturating_add(len) > target_col
    })?;
    let TokenKind::Ident(name) = &ident.kind else { unreachable!() };
    let name = name.clone();

    // 语义查询：函数签名 / 类信息
    let program = parse_program(&tokens).ok()?;
    let sem = analyze(&program).ok()?;

    if let Some(sig) = sem.funcs.get(&name) {
        // 参数名从 AST 同名函数定义取（FuncSig 只携带参数类型）
        let param_names: Vec<&str> = program
            .stmts
            .iter()
            .find_map(|s| match s {
                Stmt::FnDef(f) if f.name == name => Some(f.params.iter().map(|p| p.name.as_str()).collect()),
                _ => None,
            })
            .unwrap_or_default();
        let params = sig
            .param_tys
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                // 参数名取自 AST；FuncSig 只带类型，取不到时用占位名 argN
                let pname = match param_names.get(i) {
                    Some(n) => (*n).to_string(),
                    None => format!("arg{i}"),
                };
                format!("{pname}: {}", ty_to_str(ty))
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Some(format!("**函数签名**：func {name}({params}) -> {}", ty_to_str(&sig.ret_ty)));
    }

    if let Some(cls) = sem.classes.get(&name) {
        let mut text = format!("**类**：class {name}");
        if let Some(parent) = &cls.parent {
            text.push_str(&format!(" extends {parent}"));
        }
        return Some(text);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三阶段全通过的合法源码（func + main）：0 条诊断。
    #[test]
    fn 合法源码无诊断() {
        let src = r#"
func add(a: i64, b: i64) -> i64 {
    return a + b
}
func main() {
    println(add(1, 2))
}
"#;
        let diags = diagnostics_for_source(src);
        assert!(diags.is_empty(), "合法源码不应有诊断：{diags:?}");
    }

    /// 词法错误（字符串未闭合）：1 条诊断，位置为开引号处（1-based col 9 → 0-based character 8）。
    #[test]
    fn 词法错误生成一条诊断且位置正确() {
        let src = "var x = \"abc"; // 字符串未闭合，引号在 col 9
        let diags = diagnostics_for_source(src);
        assert_eq!(diags.len(), 1, "词法错误应恰好 1 条诊断");
        let d = &diags[0];
        assert_eq!(d.severity, 1, "严重程度应为错误");
        assert_eq!(d.source, "tie", "来源应为 tie");
        assert!(d.message.contains("未闭合"), "消息应沿用原始 message：{}", d.message);
        assert_eq!(d.range.start.line, 0, "第 1 行 → LSP line 0");
        assert_eq!(d.range.start.character, 8, "col 9 → character 8");
        assert_eq!(d.range.start, d.range.end, "v1 诊断应为零宽度点");
    }

    /// 语法错误（顶层 var 语句）：1 条诊断，消息沿用原始 message。
    #[test]
    fn 语法错误生成一条诊断() {
        let src = "var x = 1"; // 顶层只允许函数/import/类
        let diags = diagnostics_for_source(src);
        assert_eq!(diags.len(), 1, "语法错误应恰好 1 条诊断");
        let d = &diags[0];
        assert!(d.message.contains("顶层只允许"), "消息应沿用原始 message：{}", d.message);
        assert_eq!(d.range.start.line, 0, "第 1 行 → LSP line 0");
    }

    /// 语义错误（i64 变量赋 string 值）：1 条诊断，消息含「类型不匹配」。
    #[test]
    fn 语义错误生成一条诊断() {
        let src = r#"
func main() {
    var x: i64 = "hello"
}
"#;
        let diags = diagnostics_for_source(src);
        assert_eq!(diags.len(), 1, "语义错误应恰好 1 条诊断");
        let d = &diags[0];
        assert!(d.message.contains("类型不匹配"), "消息应沿用原始 message：{}", d.message);
        // var 在第 3 行（col 5，1-based）→ LSP line 2、character 4
        assert_eq!(d.range.start.line, 2, "var 在第 3 行 → LSP line 2");
        assert_eq!(d.range.start.character, 4, "var 在 col 5 → LSP character 4");
    }

    /// 位置转换：1-based Span → 0-based Range（起止相同，零宽度）。
    #[test]
    fn span转换为一基到零基() {
        let r = span_to_range(Span { line: 1, col: 1 });
        assert_eq!(r.start, Position { line: 0, character: 0 });
        assert_eq!(r.end, Position { line: 0, character: 0 });
        let r = span_to_range(Span { line: 3, col: 5 });
        assert_eq!(r.start, Position { line: 2, character: 4 });
    }

    /// 类型转文本：Named / Class / Tuple 三种形态。
    #[test]
    fn 类型转为文本() {
        use tie_frontend::ast::TupleField;
        use tie_frontend::lexer::TyKw;
        assert_eq!(ty_to_str(&TypeSpec::Named(TyKw::I64)), "i64");
        assert_eq!(ty_to_str(&TypeSpec::Named(TyKw::Str)), "string");
        assert_eq!(ty_to_str(&TypeSpec::Class("Point".into())), "Point");
        let tuple = TypeSpec::Tuple(vec![
            TupleField { name: None, ty: TypeSpec::Named(TyKw::I64) },
            TupleField { name: None, ty: TypeSpec::Named(TyKw::Str) },
        ]);
        assert_eq!(ty_to_str(&tuple), "(i64, string)");
        let named = TypeSpec::Tuple(vec![TupleField {
            name: Some("x".into()),
            ty: TypeSpec::Named(TyKw::I64),
        }]);
        assert_eq!(ty_to_str(&named), "(x: i64)");
    }

    /// hover 命中函数名：返回 Markdown 签名（含参数类型与返回类型）。
    #[test]
    fn hover命中函数名返回签名() {
        let src = r#"
func add(a: i64, b: i64) -> i64 {
    return a + b
}
func main() {
    println(add(1, 2))
}
"#;
        // "func add" 中 add 起始位置：第 2 行（LSP line 1），col 6 → character 5
        let md = hover_markdown(src, 1, 5).expect("命中 add 应返回签名");
        assert!(md.contains("**函数签名**"), "应含函数签名标记：{md}");
        assert!(md.contains("func add(a: i64, b: i64) -> i64"), "签名格式不符：{md}");
    }

    /// hover 命中类名：返回类信息（含 extends 父类）。
    #[test]
    fn hover命中类名返回类信息() {
        let src = r#"
class Animal {
    var name: string
    var age: i64
    method speak() -> string {
        return this.name + " makes a sound"
    }
}
class Dog extends Animal {
    var breed: string
}
func main() {
    println(1)
}
"#;
        // "class Animal" 中 Animal 在 LSP line 1、character 6（无继承）
        let md = hover_markdown(src, 1, 6).expect("命中 Animal 应返回类信息");
        assert!(md.contains("**类**：class Animal"), "应为类信息：{md}");
        assert!(!md.contains("extends"), "Animal 无父类：{md}");
        // "class Dog extends Animal" 中 Dog 在 LSP line 8、character 6（有继承）
        let md = hover_markdown(src, 8, 6).expect("命中 Dog 应返回类信息");
        assert!(md.contains("**类**：class Dog"), "应为类信息：{md}");
        assert!(md.contains("extends Animal"), "应含父类：{md}");
    }

    /// hover 未命中（关键字 func 上）：返回 None（result 为 null）。
    #[test]
    fn hover未命中返回空() {
        let src = "func main() {\n    println(1)\n}\n";
        // 字符 0 是 `func` 关键字（TokenKind::Func，非 Ident）→ 不命中
        assert!(hover_markdown(src, 0, 0).is_none(), "关键字不应命中");
        // 空白位置也不命中
        assert!(hover_markdown(src, 1, 0).is_none(), "行首空白不应命中");
    }

    /// hover 命中变量名（既非函数也非类）：返回 None。
    #[test]
    fn hover命中普通变量返回空() {
        let src = "func main() {\n    var count = 1\n    println(count)\n}\n";
        assert!(hover_markdown(src, 1, 8).is_none(), "普通变量不应命中");
    }

    /// hover 位置在函数名中间（如 add 的第三个字符）也应命中（长度判断）。
    #[test]
    fn hover命中函数名中间位置() {
        let src = "func add(a: i64, b: i64) -> i64 {\n    return a + b\n}\nfunc main() {\n    println(add(1, 2))\n}\n";
        let md = hover_markdown(src, 0, 7).expect("add 中间位置应命中");
        assert!(md.contains("func add"), "应命中 add：{md}");
    }
}
