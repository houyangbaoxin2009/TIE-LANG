//! tie-frontend 集成：三阶段错误 → LSP 诊断；hover / 跳转定义 / 自动补全查询。
//!
//! 职责：把 tie-frontend 的三阶段分析（词法/语法/语义）接入 LSP：
//! - [diagnostics_for_source]：对源码执行词法 → 语法 → 语义，任一阶段首个错误
//!   转成一条 LSP 诊断（v1 fail-fast，这是 tie-frontend 现有 API 的限制）。
//! - [hover_markdown]：给定 0-based 位置，命中标识符后到语义结果查询函数/类签名。
//! - [definition]：给定 0-based 位置，定位光标处标识符并返回其定义位置（函数/类/
//!   方法/变量），基于 AST 声明节点的 span（语义结果不含定义位置，故遍历 AST）。
//! - [completion]：给定 0-based 位置，返回适合该上下文的补全项（关键词/类型/内置
//!   函数/顶层函数/类名；`类名.` 场景只补该类成员）。
//! - [span_to_range] / [ty_to_str]：1-based Span → 0-based Range、TypeSpec → 文本。
//!
//! 说明：tie-frontend 不导出 serde 结构，这里统一转换为本 crate 的
//! [crate::lsp] 类型，不修改 tie-frontend 源码。

use std::collections::HashMap;

use tie_frontend::ast::{Program, Stmt, TypeSpec};
use tie_frontend::lexer::{tokenize, Span, Token, TokenKind};
use tie_frontend::parser::parse_program;
use tie_frontend::semantic::analyze;

use crate::lsp::{CompletionItem, Diagnostic, Position, Range};

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

// ==================== 跳转定义（textDocument/definition） ====================

/// 定义表：从 AST 声明节点收集的「名字 → 定义处名字 token 的 span」。
///
/// 语义结果（[analyze]）只带签名不含定义位置 span，因此跳转定义直接遍历
/// AST 声明节点，再用声明关键字的 span 到 token 流中定位真正的名字 token
/// （`func add` 的 span 指向 `func`，名字 `add` 在其后一个 token）。
#[derive(Debug, Default)]
struct DefMap {
    /// 函数名 → 定义处名字位置（顶层 `func name(...)`）
    funcs: HashMap<String, Span>,
    /// 类名 → 定义处名字位置（顶层 `class Name`）
    classes: HashMap<String, Span>,
    /// 方法名 → 定义处名字位置（`method name(...)` / `static method name(...)`）
    methods: HashMap<String, Span>,
    /// 类字段名 → 定义处名字位置（`var field`）
    fields: HashMap<String, Span>,
    /// 局部变量名 → 定义处名字位置（按源码顺序收集，同名取最近的）
    vars: Vec<(String, Span)>,
}

/// 跳转定义：给定 0-based 位置，返回光标处标识符的定义位置（0-based Range）。
///
/// 流程：
/// 1. [tokenize] 后定位覆盖光标位置的 `Ident` token（与 hover 相同的命中规则）；
/// 2. 命中后 [parse_program] 并遍历 AST 收集定义表（语义失败也可用，编辑中常用）；
/// 3. 按场景判定：
///    - 光标 token 前一个是 `.`（`obj.field` / `obj.method()`）→ 方法定义，其次字段定义；
///    - 名字是类名（构造 `Point(...)` / 类型标注）→ 类定义；
///    - 名字是函数名（调用 `f(...)`）→ 函数定义；
///    - 其余 → 最近的同名局部变量声明。
/// 找不到返回 `None`（响应 result 为 null）。
pub fn definition(source: &str, line: u32, character: u32) -> Option<Range> {
    // 词法定位：命中光标处的 Ident token（并保留其在 token 流中的下标做场景判断）
    let tokens = tokenize(source).ok()?;
    let (ident, idx) = ident_token_at(&tokens, line, character)?;
    let TokenKind::Ident(word) = &ident.kind else { unreachable!() };
    let word = word.clone();

    // 语法分析 → 构建定义表
    let program = parse_program(&tokens).ok()?;
    let defs = collect_defs(&tokens, &program);

    // 场景一：成员访问 `obj.field` / `obj.method()`——光标 token 前一个是 `.`
    if idx > 0 && matches!(tokens[idx - 1].kind, TokenKind::Dot) {
        // 方法优先（`.m(...)` 调用），其次字段（`.f` 读取）
        if let Some(span) = defs.methods.get(&word) {
            return Some(name_span_to_range(&tokens, *span));
        }
        if let Some(span) = defs.fields.get(&word) {
            return Some(name_span_to_range(&tokens, *span));
        }
        return None;
    }

    // 场景二：类名引用（构造 `Point(...)` / 类型标注）→ 类定义
    if let Some(span) = defs.classes.get(&word) {
        return Some(name_span_to_range(&tokens, *span));
    }

    // 场景三：函数调用 `f(...)` → 函数定义
    if let Some(span) = defs.funcs.get(&word) {
        return Some(name_span_to_range(&tokens, *span));
    }

    // 场景四：变量引用 → 最近的同名 VarDecl 定义（收集顺序即源码顺序）
    if let Some((_, span)) = defs.vars.iter().filter(|(n, _)| n == &word).last() {
        return Some(name_span_to_range(&tokens, *span));
    }

    None
}

/// 遍历程序 AST，收集全部「名字 → 定义位置」映射。
fn collect_defs(tokens: &[Token], program: &Program) -> DefMap {
    let mut map = DefMap::default();
    for stmt in &program.stmts {
        match stmt {
            Stmt::FnDef(f) => {
                // 函数名：`func` 关键字之后的第一个 Ident；函数体内继续收集局部变量
                if let Some(span) = name_span_after(tokens, f.span) {
                    map.funcs.insert(f.name.clone(), span);
                }
                collect_stmt_defs(tokens, &f.body, &mut map);
            }
            Stmt::Class(c) => {
                // 类名：`class` 关键字之后的第一个 Ident
                if let Some(span) = name_span_after(tokens, c.span) {
                    map.classes.insert(c.name.clone(), span);
                }
                // 字段：`var` 关键字之后的第一个 Ident
                for f in &c.fields {
                    if let Some(span) = name_span_after(tokens, f.span) {
                        map.fields.insert(f.name.clone(), span);
                    }
                }
                // 方法：`method`/`static` 关键字之后的第一个 Ident；方法体内继续收集局部变量
                for m in &c.methods {
                    if let Some(span) = name_span_after(tokens, m.span) {
                        map.methods.insert(m.name.clone(), span);
                    }
                    collect_stmt_defs(tokens, &m.body, &mut map);
                }
            }
            // 顶层其他语句（import 等）：不产生定义
            _ => collect_stmt_defs(tokens, std::slice::from_ref(stmt), &mut map),
        }
    }
    map
}

/// 递归收集语句列表内的局部变量声明（函数体/方法体/各控制流块）。
fn collect_stmt_defs(tokens: &[Token], stmts: &[Stmt], map: &mut DefMap) {
    for stmt in stmts {
        match stmt {
            Stmt::VarDecl(v) => {
                // 变量名：`var`/`const` 关键字之后的第一个 Ident
                if let Some(span) = name_span_after(tokens, v.span) {
                    map.vars.push((v.name.clone(), span));
                }
            }
            Stmt::If(i) => {
                collect_stmt_defs(tokens, &i.then_branch, map);
                collect_stmt_defs(tokens, &i.else_branch, map);
            }
            Stmt::While(w) => collect_stmt_defs(tokens, &w.body, map),
            Stmt::For(f) => collect_stmt_defs(tokens, &f.body, map),
            Stmt::Switch(s) => {
                for c in &s.cases {
                    collect_stmt_defs(tokens, &c.body, map);
                }
                collect_stmt_defs(tokens, &s.default_body, map);
            }
            // 其余语句（表达式/赋值/return/嵌套声明）不含局部变量声明
            _ => {}
        }
    }
}

/// 定位覆盖给定 0-based 位置的 Ident token，返回 token 与下标。
///
/// 命中规则（与 hover 一致）：0-based 位置还原为 1-based 与 token.span 比较，
/// 名字长度取 UTF-8 字节数（v1 接受中文标识符的字节近似）。
fn ident_token_at(tokens: &[Token], line: u32, character: u32) -> Option<(&Token, usize)> {
    let target_line = line.saturating_add(1);
    let target_col = character.saturating_add(1);
    tokens
        .iter()
        .enumerate()
        .find(|(_, t)| {
            let TokenKind::Ident(name) = &t.kind else { return false };
            let len = u32::try_from(name.len()).unwrap_or(u32::MAX);
            t.span.line == target_line
                && t.span.col <= target_col
                && t.span.col.saturating_add(len) > target_col
        })
        .map(|(idx, t)| (t, idx))
}

/// 在 token 流中定位给定 span 对应的 token 下标（取最后一个匹配）。
///
/// 用最后一个匹配而非第一个：ASI 补出的分号可能与下一行行首真实 token 同位置
/// （如上一行语句行尾补分号在 `(line,1)`，而该行第一个 token 也在 `(line,1)`），
/// 分号先入流，取最后一个才能命中真实 token。
fn token_index_of(tokens: &[Token], span: Span) -> Option<usize> {
    tokens.iter().rposition(|t| t.span == span)
}

/// 声明关键字 span → 其后第一个 Ident token 的 span（即声明名字）。
///
/// 兼容普通与带 `static` 前缀的方法定义：从关键字起向后找第一个 Ident，
/// 中间只会是 `method`/`static` 等关键字。
fn name_span_after(tokens: &[Token], span: Span) -> Option<Span> {
    let idx = token_index_of(tokens, span)?;
    tokens[idx + 1..]
        .iter()
        .find_map(|t| match &t.kind {
            TokenKind::Ident(_) => Some(t.span),
            _ => None,
        })
}

/// 定义名字的 span → 0-based Range（覆盖整个名字，便于编辑器高亮）。
fn name_span_to_range(tokens: &[Token], span: Span) -> Range {
    // 名字长度从 token 流中的 Ident token 取（与 hover 的字节近似一致）
    let len = tokens
        .iter()
        .find(|t| t.span == span)
        .and_then(|t| match &t.kind {
            TokenKind::Ident(n) => u32::try_from(n.len()).ok(),
            _ => None,
        })
        .unwrap_or(0);
    let line = span.line.saturating_sub(1);
    let start = span.col.saturating_sub(1);
    Range {
        start: Position { line, character: start },
        end: Position { line, character: start.saturating_add(len) },
    }
}

// ==================== 自动补全（textDocument/completion） ====================

/// 补全项种类（LSP `CompletionItemKind`：2=Method、3=Function、5=Field、7=Class、14=Keyword）。
const KIND_METHOD: u32 = 2;
const KIND_FUNCTION: u32 = 3;
const KIND_FIELD: u32 = 5;
const KIND_CLASS: u32 = 7;
const KIND_KEYWORD: u32 = 14;

/// 关键词补全列表（tie 语言关键字）。
const KEYWORDS: &[&str] = &[
    "func", "var", "const", "if", "else", "while", "for", "return", "import", "class", "method",
    "static", "extends", "switch", "case", "default", "in", "this",
];

/// 类型名补全列表（tie 类型关键字）。
const TYPE_NAMES: &[&str] = &[
    "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "bool", "char", "string",
    "void", "num", "text", "misc", "table",
];

/// 内置函数：名字 → 签名说明（detail 用）。
const BUILTIN_FUNCS: &[(&str, &str)] = &[
    ("println", "内置函数 println(...)：打印一行"),
    ("print", "内置函数 print(...)：打印不换行"),
    ("len", "内置函数 len(s: string) -> i64"),
    ("read_line", "内置函数 read_line() -> string"),
    ("eval", "内置函数 eval(code: string) -> string"),
];

/// 自动补全：给定 0-based 位置，返回适合该上下文的补全项列表。
///
/// 策略：
/// - `类名.` 场景（光标前是 `.` 且其前是已定义类名）→ 只补该类字段与方法；
/// - 其余场景返回全集：关键词 + 类型名 + 内置函数 + 顶层函数（detail 填签名）+
///   类名（detail 填 `class`），按 label 排序去重。
pub fn completion(source: &str, line: u32, character: u32) -> Vec<CompletionItem> {
    // 点场景：`类名.` → 只补该类成员；receiver 非类名（变量/this）时回退全集
    if let Some(class_name) = member_receiver(source, line, character) {
        let tokens = tokenize(source).ok();
        let program = tokens.as_deref().and_then(|t| parse_program(t).ok());
        if let Some(program) = &program
            && program
                .stmts
                .iter()
                .any(|s| matches!(s, Stmt::Class(c) if c.name == class_name))
        {
            return member_completions(program, &class_name);
        }
    }
    all_completions(source)
}

/// 定位光标前的成员访问接收者：源码该位置之前的文本以 `.` 结尾 → 返回 `.` 前
/// 最后一个标识符（`obj.` → `obj`）；否则返回 `None`。
fn member_receiver(source: &str, line: u32, character: u32) -> Option<String> {
    let line_text = source.lines().nth(line as usize)?;
    // 取光标前的字符（按字符数截断，避免 UTF-8 字节切片越界）
    let prefix: String = line_text.chars().take(character as usize).collect();
    let prefix = prefix.trim_end();
    if !prefix.ends_with('.') {
        return None;
    }
    // 去掉末尾 `.`，再取最后一个标识符（`.` 是 ASCII 单字节，切片安全）
    let before = prefix[..prefix.len() - 1].trim_end();
    let mut word = String::new();
    for ch in before.chars().rev() {
        if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
        } else {
            break;
        }
    }
    if word.is_empty() {
        None
    } else {
        Some(word.chars().rev().collect())
    }
}

/// 类成员补全：给定类名，返回其字段（kind=Field，detail 填类型）与方法
/// （kind=Method，detail 填签名）。
fn member_completions(program: &Program, class_name: &str) -> Vec<CompletionItem> {
    let Some(class_def) = program
        .stmts
        .iter()
        .find_map(|s| match s {
            Stmt::Class(c) if c.name == class_name => Some(c),
            _ => None,
        })
    else {
        return Vec::new();
    };
    let mut items = Vec::new();
    // 字段：detail 填类型文本
    for f in &class_def.fields {
        let detail = f.ty.as_ref().map(ty_to_str).unwrap_or_default();
        items.push(CompletionItem {
            label: f.name.clone(),
            kind: Some(KIND_FIELD),
            detail: Some(detail),
        });
    }
    // 方法：detail 填签名 `method name(params) -> Ret`
    for m in &class_def.methods {
        let params = m
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name, ty_to_str(&p.ty)))
            .collect::<Vec<_>>()
            .join(", ");
        items.push(CompletionItem {
            label: m.name.clone(),
            kind: Some(KIND_METHOD),
            detail: Some(format!("method {}({params}) -> {}", m.name, ty_to_str(&m.ret_ty))),
        });
    }
    items
}

/// 全集补全：关键词 + 类型名 + 内置函数 + 顶层函数 + 类名。
///
/// 顶层函数 / 类名来自语义结果（需源码合法）；关键词、类型名与内置函数
/// 恒返回（编辑中的半成品源码也能补全）。结果按 label 排序去重。
fn all_completions(source: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = Vec::new();
    // 关键词（无 detail）
    for kw in KEYWORDS {
        items.push(CompletionItem {
            label: (*kw).into(),
            kind: Some(KIND_KEYWORD),
            detail: None,
        });
    }
    // 类型名（detail 标记为类型）
    for t in TYPE_NAMES {
        items.push(CompletionItem {
            label: (*t).into(),
            kind: Some(KIND_KEYWORD),
            detail: Some("类型".into()),
        });
    }
    // 内置函数（detail 填签名说明）
    for (name, detail) in BUILTIN_FUNCS {
        items.push(CompletionItem {
            label: (*name).into(),
            kind: Some(KIND_FUNCTION),
            detail: Some((*detail).into()),
        });
    }
    // 顶层函数与类名：需要语义结果
    let tokens = tokenize(source).ok();
    let program = tokens.as_deref().and_then(|t| parse_program(t).ok());
    let sem = program.as_ref().and_then(|p| analyze(p).ok());
    if let (Some(program), Some(sem)) = (&program, &sem) {
        // 顶层函数：detail 填签名（参数名取自 AST 同名函数定义，与 hover 一致）
        for (name, sig) in &sem.funcs {
            let param_names: Vec<String> = program
                .stmts
                .iter()
                .find_map(|s| match s {
                    Stmt::FnDef(f) if f.name == *name => {
                        Some(f.params.iter().map(|p| p.name.clone()).collect())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            let params = sig
                .param_tys
                .iter()
                .enumerate()
                .map(|(i, ty)| match param_names.get(i) {
                    Some(n) => format!("{n}: {}", ty_to_str(ty)),
                    None => format!("arg{i}: {}", ty_to_str(ty)),
                })
                .collect::<Vec<_>>()
                .join(", ");
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(KIND_FUNCTION),
                detail: Some(format!("func {name}({params}) -> {}", ty_to_str(&sig.ret_ty))),
            });
        }
        // 类名：detail 填 `class`
        for name in sem.classes.keys() {
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(KIND_CLASS),
                detail: Some("class".into()),
            });
        }
    }
    // 排序去重（label 唯一，保留先出现的项）
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label);
    items
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

    /// 跳转定义：函数调用 `add(...)` → `func add` 定义处名字位置（line 10、character 5）。
    #[test]
    fn 跳转定义函数调用返回函数定义() {
        let r = definition(定义源码(), 16, 12).expect("add 调用应命中函数定义");
        assert_eq!(r.start, Position { line: 10, character: 5 }, "函数名位置");
        assert!(r.end.character > r.start.character, "range 应覆盖整个名字");
    }

    /// 跳转定义：类构造 `Point(0, 0)` → `class Point` 定义处名字位置（line 0、character 6）。
    #[test]
    fn 跳转定义类构造返回类定义() {
        let r = definition(定义源码(), 4, 15).expect("Point 构造应命中类定义");
        assert_eq!(r.start, Position { line: 0, character: 6 }, "类名位置");
    }

    /// 跳转定义：实例方法调用 `p.dist()` → `method dist` 定义处名字位置（line 6、character 11）。
    #[test]
    fn 跳转定义方法调用返回方法定义() {
        let r = definition(定义源码(), 17, 14).expect("dist 调用应命中方法定义");
        assert_eq!(r.start, Position { line: 6, character: 11 }, "方法名位置");
    }

    /// 跳转定义：静态方法调用 `Point.create()` → 同名方法定义（line 3、character 18）。
    #[test]
    fn 跳转定义静态方法调用返回方法定义() {
        let r = definition(定义源码(), 15, 18).expect("create 调用应命中方法定义");
        assert_eq!(r.start, Position { line: 3, character: 18 }, "静态方法名位置");
    }

    /// 跳转定义：变量引用 `count` → `var count` 声明处名字位置（line 14、character 8）。
    #[test]
    fn 跳转定义变量引用返回声明位置() {
        let r = definition(定义源码(), 16, 16).expect("count 应命中变量声明");
        assert_eq!(r.start, Position { line: 14, character: 8 }, "变量名位置");
    }

    /// 跳转定义：字段访问 `this.x` → `var x` 字段声明处（line 1、character 8）。
    #[test]
    fn 跳转定义字段访问返回字段声明() {
        let r = definition(定义源码(), 7, 20).expect("x 字段应命中声明");
        assert_eq!(r.start, Position { line: 1, character: 8 }, "字段名位置");
    }

    /// 跳转定义：光标在关键字/空白处（无 Ident token）→ None。
    #[test]
    fn 跳转定义未命中返回空() {
        // line 10 的 character 0 是 `func` 关键字（非 Ident）
        assert!(definition(定义源码(), 10, 0).is_none(), "关键字不应命中");
        // 行首空白
        assert!(definition(定义源码(), 14, 0).is_none(), "行首空白不应命中");
    }

    /// 补全全集：包含关键词 func/var、内置函数 println、类型 i64、顶层函数 add、类名 Point。
    #[test]
    fn 补全全集包含关键词类型与函数() {
        let items = completion(定义源码(), 14, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"func"), "应含关键词 func：{labels:?}");
        assert!(labels.contains(&"var"), "应含关键词 var：{labels:?}");
        assert!(labels.contains(&"println"), "应含内置函数 println：{labels:?}");
        assert!(labels.contains(&"i64"), "应含类型 i64：{labels:?}");
        assert!(labels.contains(&"add"), "应含顶层函数 add：{labels:?}");
        assert!(labels.contains(&"Point"), "应含类名 Point：{labels:?}");
        // 顶层函数 detail 填签名
        let add = items.iter().find(|i| i.label == "add").expect("应有 add");
        assert_eq!(
            add.detail.as_deref(),
            Some("func add(a: i64, b: i64) -> i64"),
            "函数 detail 应为签名"
        );
        // 类名 detail 填 class
        let cls = items.iter().find(|i| i.label == "Point").expect("应有 Point");
        assert_eq!(cls.detail.as_deref(), Some("class"));
        // 排序去重：label 唯一且有序
        assert!(items.windows(2).all(|w| w[0].label <= w[1].label), "应按 label 排序");
        let mut unique: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), items.len(), "不应有重复 label");
    }

    /// 补全点场景：`Point.` → 只补该类成员（字段 x/y + 方法 create/dist），不含关键词。
    #[test]
    fn 补全类名点后返回类成员() {
        // line 15 `    var p = Point.create()`：光标在 `Point.` 之后（character 18）
        let items = completion(定义源码(), 15, 18);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"x"), "应含字段 x：{labels:?}");
        assert!(labels.contains(&"y"), "应含字段 y：{labels:?}");
        assert!(labels.contains(&"create"), "应含静态方法 create：{labels:?}");
        assert!(labels.contains(&"dist"), "应含实例方法 dist：{labels:?}");
        assert!(!labels.contains(&"func"), "点场景不应含关键词：{labels:?}");
        // 方法 detail 填签名
        let dist = items.iter().find(|i| i.label == "dist").expect("应有 dist");
        assert_eq!(dist.detail.as_deref(), Some("method dist() -> i64"));
    }

    /// 补全点场景：receiver 是变量（非类名）→ 回退全集。
    #[test]
    fn 补全变量点后回退全集() {
        // line 17 `    println(p.dist())`：光标在 `p.` 之后（character 14）
        let items = completion(定义源码(), 17, 14);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"func"), "变量 p 不是类名，应回退全集：{labels:?}");
    }
}
