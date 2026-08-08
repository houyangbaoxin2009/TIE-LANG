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

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tie_frontend::ast::{Program, Stmt, TypeSpec};
use tie_frontend::imports::expand_imports;
use tie_frontend::lexer::{tokenize, Span, Token, TokenKind};
use tie_frontend::parser::parse_program;
use tie_frontend::semantic::{SemanticResult, analyze};

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
/// 3. [expand_imports] import 展开 → `ImportError` 时生成 1 条诊断
///    （`base_dir` 为 None 时跳过展开，仅分析单文件）；
/// 4. [analyze] 语义 → `SemanticError` 时生成 1 条诊断。
///
/// 全部成功返回空列表（无诊断）。错误消息沿用 tie-frontend 原始 message 文本。
///
/// `base_dir`：源码所在目录。跨文件命名空间调用（`str.str_split` 等）依赖
/// import 展开后才能通过语义分析（被导入文件的函数定义已内联进程序），
/// 否则会误报「未声明变量 'str'」。
pub fn diagnostics_for_source(source: &str, base_dir: Option<&Path>) -> Vec<Diagnostic> {
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
    // 阶段三：import 展开（有 base_dir 才做；无路径上下文时保持单文件分析）
    let program = match expand_with_base(program, base_dir) {
        Ok(program) => program,
        Err(err) => return vec![one_diagnostic(err.span, err.message)],
    };
    // 阶段四：语义分析
    if let Err(err) = analyze(&program) {
        return vec![one_diagnostic(err.span, err.message)];
    }
    // 全部通过：无诊断
    Vec::new()
}

/// 按 `base_dir` 展开 import：有目录则递归内联被导入文件，无目录时原样返回。
fn expand_with_base(
    program: Program,
    base_dir: Option<&Path>,
) -> Result<Program, tie_frontend::imports::ImportError> {
    match base_dir {
        Some(dir) => expand_imports(program, dir),
        None => Ok(program),
    }
}

/// 命名空间调用查询名：光标命中的 Ident 若构成 `receiver.name`（其前一个 token
/// 是 `.`，再前一个是 Ident），返回语义层的全名 `receiver::name`（命名空间函数
/// 以全名注册）；否则返回裸名。
/// 从光标 token 向左收集命名空间链段：`Ident (.|::) Ident` 反复出现时拼接。
/// 如 `tcmsg.error.no_file` → `["tcmsg", "error", "no_file"]`。
fn ns_chain(tokens: &[Token], idx: usize) -> Vec<String> {
    let TokenKind::Ident(name) = &tokens[idx].kind else { return Vec::new() };
    let mut segs = vec![name.clone()];
    let mut i = idx;
    // 前一个 token 是 `.` 或 `::`、再前一个是 Ident 时继续向左收集
    while i >= 2 && matches!(tokens[i - 1].kind, TokenKind::Dot | TokenKind::DoubleColon) {
        if let TokenKind::Ident(prev) = &tokens[i - 2].kind {
            segs.push(prev.clone());
            i -= 2;
        } else {
            break;
        }
    }
    segs.reverse();
    segs
}

/// 查询全名：命名空间链（`tcmsg.error.no_file`）→ `tcmsg::error::no_file`；
/// 无链（裸名）→ 原名。与语义层命名空间函数注册的全名格式一致。
fn ns_query_name(tokens: &[Token], idx: usize, name: &str) -> String {
    let _ = name; // 链从 token 流取（与调用方传入的 name 一致），保留参数供调用方语义清晰
    ns_chain(tokens, idx).join("::")
}

/// 从程序 AST 查找函数定义（含命名空间内函数），返回参数名列表。
///
/// `query_name` 为全名（命名空间函数 `math::abs`）或裸名（顶层函数 `add`）。
/// FuncSig 只携带参数类型，参数名需回 AST 取。
fn func_param_names(program: &Program, query_name: &str) -> Vec<String> {
    func_param_names_inner(&program.stmts, &[], query_name)
}

/// 递归收集函数参数名的实际实现。
///
/// `ns_prefix` 是当前遍历所在的命名空间路径段（空 = 顶层），用于拼接全名比对。
fn func_param_names_inner(stmts: &[Stmt], ns_prefix: &[String], query_name: &str) -> Vec<String> {
    for stmt in stmts {
        match stmt {
            Stmt::FnDef(f) => {
                // 全名 = 命名空间路径段 + 函数名；顶层函数全名即裸名
                let mut segs = ns_prefix.to_vec();
                segs.push(f.name.clone());
                if segs.join("::") == query_name {
                    return f.params.iter().map(|p| p.name.clone()).collect();
                }
            }
            Stmt::Namespace(ns) => {
                // 嵌套命名空间：路径拼接后递归
                let mut segs = ns_prefix.to_vec();
                segs.extend(ns.path.iter().cloned());
                let found = func_param_names_inner(&ns.body, &segs, query_name);
                if !found.is_empty() {
                    return found;
                }
            }
            _ => {}
        }
    }
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
/// 2. 命中后执行语义分析（含 import 展开，跨文件函数/类也能命中）：
///    - `sem.funcs` 含该名字 → `**函数签名**：func name(param: Ty, ...) -> Ret`
///      （参数名取自 AST 同名函数定义，类型来自 `FuncSig.param_tys`）；
///    - `sem.classes` 含该名字 → `**类**：class Name`（有 extends 则追加）；
///    - 都没命中返回 `None`（hover result 为 null）。
///
/// `base_dir`：源码所在目录（import 展开用），None 时仅分析单文件。
pub fn hover_markdown(
    source: &str,
    line: u32,
    character: u32,
    base_dir: Option<&Path>,
) -> Option<String> {
    // 命中标识符：0-based 位置还原为 1-based 再与 token.span 比较
    let tokens = tokenize(source).ok()?;
    let target_line = line.saturating_add(1);
    let target_col = character.saturating_add(1);
    let (idx, ident) = tokens.iter().enumerate().find(|(_, t)| {
        let TokenKind::Ident(name) = &t.kind else { return false };
        // 字节长度转 u32（64 位平台恒成功；异常时按最大处理）
        let len = u32::try_from(name.len()).unwrap_or(u32::MAX);
        t.span.line == target_line
            && t.span.col <= target_col
            && t.span.col.saturating_add(len) > target_col
    })?;
    let TokenKind::Ident(name) = &ident.kind else { unreachable!() };
    let name = name.clone();

    // 语义查询：函数签名 / 类信息（先语法分析，再按 base_dir 展开 import）
    let program = parse_program(&tokens).ok()?;
    let program = expand_with_base(program, base_dir).ok()?;
    let sem = analyze(&program).ok()?;

    // 查询名：命名空间调用（`math.abs`，token 前是 `.` 且再前是 Ident）→ 全名
    // `math::abs`；其余场景用裸名。语义层命名空间函数以全名注册。
    let query_name = ns_query_name(&tokens, idx, &name);
    if let Some(sig) = sem.funcs.get(&query_name) {
        // 参数名从 AST 同名函数定义取（FuncSig 只携带参数类型；含命名空间内函数）
        let param_names = func_param_names(&program, &query_name);
        let params = sig
            .param_tys
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                // 参数名取自 AST；FuncSig 只带类型，取不到时用占位名 argN
                let pname = match param_names.get(i) {
                    Some(n) => n.clone(),
                    None => format!("arg{i}"),
                };
                format!("{pname}: {}", ty_to_str(ty))
            })
            .collect::<Vec<_>>()
            .join(", ");
        // 显示名用裸名（`math::abs` → `abs`），与源码调用形式一致
        let display = query_name.rsplit("::").next().unwrap_or(&query_name);
        return Some(format!("**函数签名**：func {display}({params}) -> {}", ty_to_str(&sig.ret_ty)));
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
///
/// `base_dir`：源码所在目录（import 展开用）。展开后跨文件函数/类也能跳转
/// 到定义（定义 span 取自被导入文件的 AST，位置正确）。
pub fn definition(
    source: &str,
    line: u32,
    character: u32,
    base_dir: Option<&Path>,
) -> Option<Range> {
    // 词法定位：命中光标处的 Ident token（并保留其在 token 流中的下标做场景判断）
    let tokens = tokenize(source).ok()?;
    let (ident, idx) = ident_token_at(&tokens, line, character)?;
    let TokenKind::Ident(word) = &ident.kind else { unreachable!() };
    let word = word.clone();

    // 语法分析 → 构建定义表（含 import 展开：跨文件定义也可命中）
    let program = parse_program(&tokens).ok()?;
    let program = expand_with_base(program, base_dir).ok()?;
    let defs = collect_defs(&tokens, &program);

    // 场景一：成员访问 `obj.field` / `obj.method()` / `tcmsg.error.no_file(...)`——
    // 光标 token 前一个是 `.`。命名空间函数调用（链式如 `tcmsg.error.no_file`）
    // 的完整全名在 funcs 表，先按全名查；方法/字段（裸名）其次。
    if idx > 0 && matches!(tokens[idx - 1].kind, TokenKind::Dot) {
        // 命名空间函数：`a.b.c` 链拍平为 `a::b::c` 查 funcs（顶层函数裸名不受影响）
        let full = ns_query_name(&tokens, idx, &word);
        if let Some(span) = defs.funcs.get(&full) {
            return Some(name_span_to_range(&tokens, *span));
        }
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
    collect_defs_inner(tokens, &program.stmts, &[], &mut map);
    map
}

/// 递归收集定义的实现：`ns_prefix` 为当前命名空间路径段（空 = 顶层）。
///
/// 命名空间内函数以全名（`math::abs`）为 key 注册，供命名空间调用跳转。
fn collect_defs_inner(tokens: &[Token], stmts: &[Stmt], ns_prefix: &[String], map: &mut DefMap) {
    for stmt in stmts {
        match stmt {
            Stmt::FnDef(f) => {
                // 函数名：`func` 关键字之后的第一个 Ident；key 用全名（顶层即裸名）
                if let Some(span) = name_span_after(tokens, f.span) {
                    let mut segs = ns_prefix.to_vec();
                    segs.push(f.name.clone());
                    map.funcs.insert(segs.join("::"), span);
                }
                // 参数：跳转场景四（变量引用）可命中参数名
                for p in &f.params {
                    map.vars.push((p.name.clone(), p.span));
                }
                collect_stmt_defs(tokens, &f.body, map);
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
                    // 方法参数：跳转场景四（变量引用）可命中参数名
                    for p in &m.params {
                        map.vars.push((p.name.clone(), p.span));
                    }
                    collect_stmt_defs(tokens, &m.body, map);
                }
            }
            Stmt::Namespace(ns) => {
                // 命名空间：路径拼接后递归（函数以全名注册）
                let mut segs = ns_prefix.to_vec();
                segs.extend(ns.path.iter().cloned());
                collect_defs_inner(tokens, &ns.body, &segs, map);
            }
            // 顶层其他语句（import 等）：不产生定义
            _ => collect_stmt_defs(tokens, std::slice::from_ref(stmt), map),
        }
    }
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

/// 补全项种类（LSP `CompletionItemKind`：2=Method、3=Function、5=Field、7=Class、
/// 9=Namespace、14=Keyword）。
const KIND_METHOD: u32 = 2;
const KIND_FUNCTION: u32 = 3;
const KIND_FIELD: u32 = 5;
const KIND_CLASS: u32 = 7;
const KIND_NAMESPACE: u32 = 9;
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
/// - `命名空间名.` 场景（receiver 是命名空间）→ 只补该命名空间内函数（裸名）；
/// - 其余场景返回全集：关键词 + 类型名 + 内置函数 + 顶层函数（detail 填签名）+
///   类名（detail 填 `class`），按 label 排序去重。
///
/// `base_dir`：源码所在目录（import 展开用）。展开后跨文件导入的命名空间
/// 函数（`math.abs` 等）在点场景与全集都能补全。
pub fn completion(
    source: &str,
    line: u32,
    character: u32,
    base_dir: Option<&Path>,
) -> Vec<CompletionItem> {
    // 点场景：`类名.` / `命名空间名.` → 只补对应成员；receiver 非类/命名空间时回退全集
    if let Some(receiver) = member_receiver(source, line, character) {
        let tokens = tokenize(source).ok();
        let program = tokens.as_deref().and_then(|t| parse_program(t).ok());
        let program = program.and_then(|p| expand_with_base(p, base_dir).ok());
        let sem = program.as_ref().and_then(|p| analyze(p).ok());
        if let (Some(program), Some(sem)) = (&program, &sem) {
            // 场景一：类名. → 类成员补全
            if program
                .stmts
                .iter()
                .any(|s| matches!(s, Stmt::Class(c) if c.name == receiver))
            {
                return member_completions(program, &receiver);
            }
            // 场景二：命名空间. → 命名空间函数补全（裸名）
            let ns_items = ns_member_completions(program, sem, &receiver);
            if !ns_items.is_empty() {
                return ns_items;
            }
        }
    }
    all_completions(source, base_dir)
}

/// 命名空间成员补全：给定命名空间路径（如 `math` 或 `tcmsg::error`），返回该
/// 命名空间内的直接成员。label 规则：
/// - 函数成员（`{ns}::abs`）→ label 裸名 `abs`，detail 填签名；
/// - 子命名空间（`{ns}::error::no_file`）→ label 只取第一段 `error`，detail 填
///   `namespace`（选择后继续 `.` 可进入下一级）。
///
/// 用于 `math.` / `tcmsg.error.` 点场景。
fn ns_member_completions(program: &Program, sem: &SemanticResult, ns: &str) -> Vec<CompletionItem> {
    let prefix = format!("{ns}::");
    // 去重：同一 label（如多个子命名空间函数都映射到 error）只保留一个
    let mut seen: HashSet<String> = HashSet::new();
    let mut items = Vec::new();
    for (full, sig) in &sem.funcs {
        let Some(rest) = full.strip_prefix(&prefix) else { continue };
        // 只取直接成员：剩余含 `::` 说明是更深层子命名空间，取第一段
        let (label, is_ns) = match rest.split_once("::") {
            Some((seg, _)) => (seg.to_string(), true),
            None => (rest.to_string(), false),
        };
        if !seen.insert(label.clone()) {
            continue;
        }
        let item = if is_ns {
            // 子命名空间：detail 标 `namespace`，选中后继续 `.` 深入
            CompletionItem { label, kind: Some(KIND_NAMESPACE), detail: Some("namespace".into()) }
        } else {
            // 函数成员：参数名取自 AST（命名空间内函数定义）
            let param_names = func_param_names(program, full);
            let params = sig
                .param_tys
                .iter()
                .enumerate()
                .map(|(i, ty)| {
                    let pname = param_names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("arg{i}"));
                    format!("{pname}: {}", ty_to_str(ty))
                })
                .collect::<Vec<_>>()
                .join(", ");
            CompletionItem {
                label,
                kind: Some(KIND_FUNCTION),
                detail: Some(format!("func {}({params}) -> {}", rest, ty_to_str(&sig.ret_ty))),
            }
        };
        items.push(item);
    }
    items
}

/// 定位光标前的成员访问接收者：源码该位置之前的文本以 `.` 结尾 → 返回 `.` 前
/// 的完整链（`obj.` → `obj`；`tcmsg.error.` → `tcmsg::error`，用 `::` 连接以
/// 匹配语义层命名空间全名）；否则返回 `None`。
///
/// 兼容点分命名空间：`math.` / `tcmsg.error.` 都命中（链段均收集）。
fn member_receiver(source: &str, line: u32, character: u32) -> Option<String> {
    let line_text = source.lines().nth(line as usize)?;
    // 取光标前的字符（按字符数截断，避免 UTF-8 字节切片越界）
    let prefix: String = line_text.chars().take(character as usize).collect();
    let prefix = prefix.trim_end();
    if !prefix.ends_with('.') {
        return None;
    }
    // 去掉末尾 `.`，向前收集 `ident(.ident)*` 链（`.` 是 ASCII 单字节，切片安全）
    let before = prefix[..prefix.len() - 1].trim_end();
    let mut segs: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut prev_dot = false;
    for ch in before.chars().rev() {
        if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
            prev_dot = false;
        } else if ch == '.' && !prev_dot {
            // 一次 `.` 结束当前段；`.` 前无段（前一个字符非标识符）时中断
            if word.is_empty() {
                break;
            }
            segs.push(word.chars().rev().collect());
            word.clear();
            prev_dot = true;
        } else {
            break;
        }
    }
    // 收尾：最后一段（若链仅一层，word 即完整 receiver）
    if !word.is_empty() {
        segs.push(word.chars().rev().collect());
    }
    if segs.is_empty() {
        None
    } else {
        // 反向收集后反转回源码顺序，再以 `::` 连接成链
        segs.reverse();
        Some(segs.join("::"))
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
/// 顶层函数 / 类名来自语义结果（需源码合法，含 import 展开后的跨文件定义）；
/// 关键词、类型名与内置函数恒返回（编辑中的半成品源码也能补全）。
/// 结果按 label 排序去重。
fn all_completions(source: &str, base_dir: Option<&Path>) -> Vec<CompletionItem> {
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
    // 顶层函数与类名：需要语义结果（含 import 展开）
    let tokens = tokenize(source).ok();
    let program = tokens.as_deref().and_then(|t| parse_program(t).ok());
    let program = program.and_then(|p| expand_with_base(p, base_dir).ok());
    let sem = program.as_ref().and_then(|p| analyze(p).ok());
    if let (Some(program), Some(sem)) = (&program, &sem) {
        // 顶层函数：detail 填签名（参数名取自 AST 同名函数定义，与 hover 一致）。
        // 跳过命名空间函数全名（含 `::`，如 `math::abs`）——它们只在
        // `math.` 点场景补全（[ns_member_completions]），全集只列裸名函数。
        for (name, sig) in &sem.funcs {
            if name.contains("::") {
                continue;
            }
            let param_names = func_param_names(program, name);
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

// ==================== 语义高亮（textDocument/semanticTokens/full） ====================

/// 语义 token 类型索引（对应 [crate::lsp::semantic_token_types] 的下标）。
///
/// 关键字/类型关键字由 TextMate 语法着色，这里只覆盖标识符类；
/// 故 TYPE/KEYWORD 不在其中（避免声明未使用）。
mod st {
    /// 命名空间（`tcmsg` / `math`）
    pub const NAMESPACE: u32 = 0;
    /// 类名（定义与引用）
    pub const CLASS: u32 = 1;
    /// 函数（定义与调用）
    pub const FUNCTION: u32 = 2;
    /// 方法（定义与调用）
    pub const METHOD: u32 = 3;
    /// 属性/字段（`p.x` 的 x）
    pub const PROPERTY: u32 = 4;
    /// 变量（声明与引用）
    pub const VARIABLE: u32 = 5;
    /// 参数（形参声明与引用）
    pub const PARAMETER: u32 = 6;
}

/// 语义 token 类型名称列表（LSP `SemanticTokensLegend.tokenTypes`）。
///
/// 下标与 [st] 模块常量一一对应；供 [crate::server] 构造 initialize 能力声明。
pub fn semantic_token_types() -> Vec<String> {
    ["namespace", "class", "function", "method", "property", "variable", "parameter"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 语义高亮：`textDocument/semanticTokens/full` 请求的结果数据（增量编码）。
///
/// 对源码逐 token 分类（仅标识符；关键字/字面量/运算符交给 TextMate 着色）：
/// - 命名空间声明路径段 / 命名空间调用链段（`tcmsg.error.` 的 tcmsg/error）→ NAMESPACE；
/// - 函数定义名 / 命名空间函数末段 / 函数调用名 → FUNCTION；
/// - 方法定义名 / 实例方法调用（`p.dist()`）→ METHOD；
/// - 类名（定义与引用）→ CLASS；
/// - 字段访问（`p.x`）→ PROPERTY；
/// - 形参（声明处 span 精确匹配）→ PARAMETER；
/// - 其余标识符（局部变量等）→ VARIABLE。
///
/// 编码采用 LSP 增量格式：每 5 个 u32 一组
/// `(deltaLine, deltaStartChar, length, tokenType, tokenModifiers)`，
/// 相对上一 token 编码（v1 只声明类型，修饰符恒 0）。
///
/// `base_dir`：源码所在目录（import 展开用）。展开后命名空间函数全名
/// （`tcmsg::error::no_file`）参与链段识别。
pub fn semantic_tokens(source: &str, base_dir: Option<&Path>) -> Vec<u32> {
    // 词法失败（编辑中途）→ 空结果（客户端保留上一帧）
    let Ok(tokens) = tokenize(source) else { return Vec::new() };

    // 语义结果（尽力而为：语法/语义失败也能用词法规则分类）
    let program = parse_program(&tokens).ok();
    let program = program.and_then(|p| expand_with_base(p, base_dir).ok());
    let sem = program.as_ref().and_then(|p| analyze(p).ok());

    // 形参声明 span 集合：形参名 token 的精确位置（用于 PARAMETER 分类）
    let param_spans = program
        .as_ref()
        .map(|p| collect_param_spans(&p.stmts))
        .unwrap_or_default();

    let mut out: Vec<u32> = Vec::new();
    // 上一 token 的 0-based 行/列（增量编码基准）
    let mut prev_line = 0u32;
    let mut prev_char = 0u32;

    for (i, t) in tokens.iter().enumerate() {
        // 只输出标识符：关键字/类型/字面量/运算符交给 TextMate 语法着色
        let TokenKind::Ident(name) = &t.kind else { continue };
        let ty = classify_ident(&tokens, i, name, sem.as_ref(), &param_spans);
        // 1-based span → 0-based 位置（与 span_to_range 一致）
        let line = t.span.line.saturating_sub(1);
        let char = t.span.col.saturating_sub(1);
        let len = u32::try_from(name.len()).unwrap_or(u32::MAX);
        out.push(line - prev_line); // deltaLine
        out.push(if line == prev_line { char - prev_char } else { char }); // deltaStartChar
        out.push(len);
        out.push(ty);
        out.push(0); // tokenModifiers：v1 恒无修饰
        prev_line = line;
        prev_char = char;
    }
    out
}

/// 递归收集全部形参声明位置（函数/方法参数名 token 的 span）。
///
/// 用 Vec + 线性查找：Span 不实现 Hash（tie-frontend 零侵入设计），
/// 且参数数量有限，线性扫描开销可忽略。
fn collect_param_spans(stmts: &[Stmt]) -> Vec<Span> {
    let mut spans = Vec::new();
    collect_param_spans_inner(stmts, &mut spans);
    spans
}

/// [collect_param_spans] 的递归实现。
fn collect_param_spans_inner(stmts: &[Stmt], spans: &mut Vec<Span>) {
    for stmt in stmts {
        match stmt {
            Stmt::FnDef(f) => {
                for p in &f.params {
                    spans.push(p.span);
                }
                collect_param_spans_inner(&f.body, spans);
            }
            Stmt::Class(c) => {
                for m in &c.methods {
                    for p in &m.params {
                        spans.push(p.span);
                    }
                    collect_param_spans_inner(&m.body, spans);
                }
            }
            Stmt::Namespace(ns) => collect_param_spans_inner(&ns.body, spans),
            Stmt::If(i) => {
                collect_param_spans_inner(&i.then_branch, spans);
                collect_param_spans_inner(&i.else_branch, spans);
            }
            Stmt::While(w) => collect_param_spans_inner(&w.body, spans),
            Stmt::For(f) => collect_param_spans_inner(&f.body, spans),
            Stmt::Switch(s) => {
                for c in &s.cases {
                    collect_param_spans_inner(&c.body, spans);
                }
                collect_param_spans_inner(&s.default_body, spans);
            }
            _ => {}
        }
    }
}

/// 单个标识符的语义类型分类（见 [semantic_tokens] 的优先级说明）。
///
/// 判定顺序：
/// 1. 定义名（前面是 func/method/class/namespace）→ 对应类型；
/// 2. 命名空间链（[ns_chain]）：
///    - 完整链在语义层 funcs（命名空间函数）→ FUNCTION；
///    - 当前段前缀存在（`tcmsg::error` 的 tcmsg/error 段）→ NAMESPACE；
/// 3. 实例成员访问（`p.dist(` 前 `.`）→ 后跟 `(` 为 METHOD，否则 PROPERTY；
/// 4. 形参声明（span 精确匹配）→ PARAMETER；
/// 5. 类名（语义表）→ CLASS；
/// 6. 函数调用（后跟 `(`）→ FUNCTION；
/// 7. 其余标识符 → VARIABLE。
fn classify_ident(
    tokens: &[Token],
    idx: usize,
    name: &str,
    sem: Option<&SemanticResult>,
    param_spans: &[Span],
) -> u32 {
    // 1. 定义名：`func`/`method`/`class`/`namespace` 之后的第一个标识符
    if idx > 0 {
        match &tokens[idx - 1].kind {
            TokenKind::Func => return st::FUNCTION,
            TokenKind::Method => return st::METHOD,
            TokenKind::Class => return st::CLASS,
            TokenKind::Namespace => return st::NAMESPACE,
            _ => {}
        }
    }

    // 2. 命名空间链：完整链在 funcs → 命名空间函数；当前段前缀存在 → 命名空间段
    if let Some(s) = sem {
        let chain = ns_chain(tokens, idx);
        let full = chain.join("::");
        if s.funcs.contains_key(&full) {
            return st::FUNCTION;
        }
        let prefix = format!("{full}::");
        if s.funcs.keys().any(|k| k.starts_with(&prefix)) {
            return st::NAMESPACE;
        }
    }

    // 3. 实例成员访问：`p.dist(`（前 `.`）→ 方法；`p.x`（后非 `(`）→ 字段
    if idx > 0 && matches!(tokens[idx - 1].kind, TokenKind::Dot) {
        let next_is_lparen = matches!(tokens.get(idx + 1), Some(n) if matches!(n.kind, TokenKind::LParen));
        return if next_is_lparen { st::METHOD } else { st::PROPERTY };
    }

    // 4. 形参声明：span 在收集到的形参集合内
    if param_spans.contains(&tokens[idx].span) {
        return st::PARAMETER;
    }

    // 5. 类名（语义表：类定义名已在第 1 步命中，这里是引用）
    if let Some(s) = sem {
        if s.classes.contains_key(name) {
            return st::CLASS;
        }
    }

    // 6. 函数调用：后跟 `(`（非方法调用、非定义名）
    if matches!(tokens.get(idx + 1), Some(n) if matches!(n.kind, TokenKind::LParen)) {
        return st::FUNCTION;
    }

    // 7. 其余标识符（局部变量引用/声明）→ VARIABLE
    st::VARIABLE
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
        let diags = diagnostics_for_source(src, None);
        assert!(diags.is_empty(), "合法源码不应有诊断：{diags:?}");
    }

    /// 词法错误（字符串未闭合）：1 条诊断，位置为开引号处（1-based col 9 → 0-based character 8）。
    #[test]
    fn 词法错误生成一条诊断且位置正确() {
        let src = "var x = \"abc"; // 字符串未闭合，引号在 col 9
        let diags = diagnostics_for_source(src, None);
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
        let diags = diagnostics_for_source(src, None);
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
        let diags = diagnostics_for_source(src, None);
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
        let md = hover_markdown(src, 1, 5, None).expect("命中 add 应返回签名");
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
        let md = hover_markdown(src, 1, 6, None).expect("命中 Animal 应返回类信息");
        assert!(md.contains("**类**：class Animal"), "应为类信息：{md}");
        assert!(!md.contains("extends"), "Animal 无父类：{md}");
        // "class Dog extends Animal" 中 Dog 在 LSP line 8、character 6（有继承）
        let md = hover_markdown(src, 8, 6, None).expect("命中 Dog 应返回类信息");
        assert!(md.contains("**类**：class Dog"), "应为类信息：{md}");
        assert!(md.contains("extends Animal"), "应含父类：{md}");
    }

    /// hover 未命中（关键字 func 上）：返回 None（result 为 null）。
    #[test]
    fn hover未命中返回空() {
        let src = "func main() {\n    println(1)\n}\n";
        // 字符 0 是 `func` 关键字（TokenKind::Func，非 Ident）→ 不命中
        assert!(hover_markdown(src, 0, 0, None).is_none(), "关键字不应命中");
        // 空白位置也不命中
        assert!(hover_markdown(src, 1, 0, None).is_none(), "行首空白不应命中");
    }

    /// hover 命中变量名（既非函数也非类）：返回 None。
    #[test]
    fn hover命中普通变量返回空() {
        let src = "func main() {\n    var count = 1\n    println(count)\n}\n";
        assert!(hover_markdown(src, 1, 8, None).is_none(), "普通变量不应命中");
    }

    /// hover 位置在函数名中间（如 add 的第三个字符）也应命中（长度判断）。
    #[test]
    fn hover命中函数名中间位置() {
        let src = "func add(a: i64, b: i64) -> i64 {\n    return a + b\n}\nfunc main() {\n    println(add(1, 2))\n}\n";
        let md = hover_markdown(src, 0, 7, None).expect("add 中间位置应命中");
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
        let r = definition(定义源码(), 16, 12, None).expect("add 调用应命中函数定义");
        assert_eq!(r.start, Position { line: 10, character: 5 }, "函数名位置");
        assert!(r.end.character > r.start.character, "range 应覆盖整个名字");
    }

    /// 跳转定义：类构造 `Point(0, 0)` → `class Point` 定义处名字位置（line 0、character 6）。
    #[test]
    fn 跳转定义类构造返回类定义() {
        let r = definition(定义源码(), 4, 15, None).expect("Point 构造应命中类定义");
        assert_eq!(r.start, Position { line: 0, character: 6 }, "类名位置");
    }

    /// 跳转定义：实例方法调用 `p.dist()` → `method dist` 定义处名字位置（line 6、character 11）。
    #[test]
    fn 跳转定义方法调用返回方法定义() {
        let r = definition(定义源码(), 17, 14, None).expect("dist 调用应命中方法定义");
        assert_eq!(r.start, Position { line: 6, character: 11 }, "方法名位置");
    }

    /// 跳转定义：静态方法调用 `Point.create()` → 同名方法定义（line 3、character 18）。
    #[test]
    fn 跳转定义静态方法调用返回方法定义() {
        let r = definition(定义源码(), 15, 18, None).expect("create 调用应命中方法定义");
        assert_eq!(r.start, Position { line: 3, character: 18 }, "静态方法名位置");
    }

    /// 跳转定义：变量引用 `count` → `var count` 声明处名字位置（line 14、character 8）。
    #[test]
    fn 跳转定义变量引用返回声明位置() {
        let r = definition(定义源码(), 16, 16, None).expect("count 应命中变量声明");
        assert_eq!(r.start, Position { line: 14, character: 8 }, "变量名位置");
    }

    /// 跳转定义：字段访问 `this.x` → `var x` 字段声明处（line 1、character 8）。
    #[test]
    fn 跳转定义字段访问返回字段声明() {
        let r = definition(定义源码(), 7, 20, None).expect("x 字段应命中声明");
        assert_eq!(r.start, Position { line: 1, character: 8 }, "字段名位置");
    }

    /// 跳转定义：光标在关键字/空白处（无 Ident token）→ None。
    #[test]
    fn 跳转定义未命中返回空() {
        // line 10 的 character 0 是 `func` 关键字（非 Ident）
        assert!(definition(定义源码(), 10, 0, None).is_none(), "关键字不应命中");
        // 行首空白
        assert!(definition(定义源码(), 14, 0, None).is_none(), "行首空白不应命中");
    }

    /// 补全全集：包含关键词 func/var、内置函数 println、类型 i64、顶层函数 add、类名 Point。
    #[test]
    fn 补全全集包含关键词类型与函数() {
        let items = completion(定义源码(), 14, 0, None);
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
        let items = completion(定义源码(), 15, 18, None);
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
        let items = completion(定义源码(), 17, 14, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"func"), "变量 p 不是类名，应回退全集：{labels:?}");
    }

    // ==================== import 展开（base_dir） ====================

    /// 创建唯一临时目录（import 展开测试用），返回目录路径。
    /// 用进程 id + 纳秒时间戳保证并行测试互不冲突。
    fn 临时目录() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX 纪元")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("tie-lsp-test-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir
    }

    /// import 展开后，跨文件命名空间调用（`math.abs`）不再误报「未声明变量」。
    ///
    /// 对比：
    /// - 传 base_dir（import 展开）→ 被导入文件的函数定义内联进程序，0 条诊断；
    /// - 不传 base_dir（单文件分析）→ 语义层看不到 `math` 命名空间，误报未声明变量。
    #[test]
    fn import展开后命名空间调用无误报() {
        // 被导入库：namespace math + abs 函数（与 examples/lib_math.tie 同形态）
        let dir = 临时目录();
        let lib = r#"namespace math {
    func abs(x: i64) -> i64 {
        if x < 0 {
            return -x
        }
        return x
    }
}
"#;
        std::fs::write(dir.join("lib_math.tie"), lib).expect("写被导入文件失败");
        let main = r#"import "./lib_math.tie"
func main() {
    println(math.abs(-5))
}
"#;

        // 有 base_dir：import 展开后语义分析通过
        let diags = diagnostics_for_source(main, Some(&dir));
        assert!(diags.is_empty(), "import 展开后不应误报：{diags:?}");

        // 无 base_dir：单文件分析，报未声明变量
        let diags_single = diagnostics_for_source(main, None);
        assert!(
            diags_single.iter().any(|d| d.message.contains("未声明的变量")),
            "单文件模式应误报未声明变量：{diags_single:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// import 被导入文件不存在 → 1 条诊断，消息含「读取失败」与路径。
    #[test]
    fn import文件不存在生成诊断() {
        let dir = 临时目录();
        let main = r#"import "./no_such_file.tie"
func main() {
    println(1)
}
"#;
        let diags = diagnostics_for_source(main, Some(&dir));
        assert_eq!(diags.len(), 1, "文件缺失应恰好 1 条诊断：{diags:?}");
        assert!(
            diags[0].message.contains("读取失败"),
            "消息应说明读取失败：{}",
            diags[0].message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// import 循环（a 导 b、b 导 a）→ 1 条诊断，消息含「循环导入」。
    #[test]
    fn import循环导入生成诊断() {
        let dir = 临时目录();
        // a.tie 导入 b.tie
        std::fs::write(
            dir.join("a.tie"),
            "import \"./b.tie\"\nnamespace ns {\n    func fa() {\n    }\n}\n",
        )
        .expect("写 a.tie 失败");
        // b.tie 导入 a.tie → 构成循环
        std::fs::write(
            dir.join("b.tie"),
            "import \"./a.tie\"\nnamespace ns {\n    func fb() {\n    }\n}\n",
        )
        .expect("写 b.tie 失败");
        let main = r#"import "./a.tie"
func main() {
    println(1)
}
"#;
        let diags = diagnostics_for_source(main, Some(&dir));
        assert_eq!(diags.len(), 1, "循环导入应恰好 1 条诊断：{diags:?}");
        assert!(
            diags[0].message.contains("循环导入"),
            "消息应说明循环导入：{}",
            diags[0].message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// hover：import 展开后，跨文件命名空间函数（`math.abs`）也能命中签名。
    #[test]
    fn import展开后hover命中跨文件函数() {
        let dir = 临时目录();
        let lib = r#"namespace math {
    func abs(x: i64) -> i64 {
        return x
    }
}
"#;
        std::fs::write(dir.join("lib_math.tie"), lib).expect("写被导入文件失败");
        // 第 3 行 `    println(math.abs(-5))`（LSP line 2）：abs 起始 character 17
        let main = "import \"./lib_math.tie\"\nfunc main() {\n    println(math.abs(-5))\n}\n";
        let md = hover_markdown(main, 2, 17, Some(&dir)).expect("展开后应命中 abs 签名");
        assert!(md.contains("func abs"), "应命中跨文件函数：{md}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 补全：`math.` 点场景 → 补全跨文件命名空间函数（裸名 abs）。
    #[test]
    fn import展开后补全命名空间成员() {
        let dir = 临时目录();
        let lib = r#"namespace math {
    func abs(x: i64) -> i64 {
        return x
    }
}
"#;
        std::fs::write(dir.join("lib_math.tie"), lib).expect("写被导入文件失败");
        // 第 3 行 `    println(math.abs(-5))`（LSP line 2）：光标在 `math.` 之后
        // （`.` 在 character 16，其后的 character 17 是 abs 的 a）——源码保持完整
        // 可解析，模拟用户已在 `.` 后输入首个字符的场景
        let main = "import \"./lib_math.tie\"\nfunc main() {\n    println(math.abs(-5))\n}\n";
        let items = completion(main, 2, 17, Some(&dir));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"abs"), "点场景应补全命名空间函数 abs：{labels:?}");
        // 命名空间点场景不应含无关内容（如关键词 func）
        assert!(!labels.contains(&"func"), "命名空间点场景不应含关键词：{labels:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ==================== 嵌套命名空间（tcmsg.error.no_file） ====================

    /// 嵌套命名空间源码（`tcmsg` 内嵌 `error`，顶层 `main` 调用链式成员）。
    ///
    /// 注意 `r#"` 后直接换行 → 第 1 行为空行；下面注释中的 LSP 行号从 0 计。
    fn 嵌套命名空间源码() -> &'static str {
        r#"
namespace tcmsg {
    func hello() -> string {
        return "Hello from tcmsg"
    }
    namespace error {
        func no_file() -> string {
            return "no file"
        }
    }
}
func main() {
    println(tcmsg.hello())
    println(tcmsg.error.no_file())
}
"#
    }

    /// 嵌套命名空间 hover：`tcmsg.error.no_file` 的 no_file → 命中全名签名。
    ///
    /// 旧实现 `ns_query_name` 只取单层链（`error::no_file`），与语义层注册的
    /// `tcmsg::error::no_file` 对不上而查不到；新实现收集完整链。
    #[test]
    fn 嵌套命名空间hover命中函数签名() {
        let src = 嵌套命名空间源码();
        // 第 14 行 `    println(tcmsg.error.no_file())`（LSP line 13）：
        // no_file 起始 character 24（`    println(` 12 字符 + tcmsg.error. 12 字符）
        let md = hover_markdown(src, 13, 25, None).expect("应命中 no_file 签名");
        assert!(md.contains("func no_file"), "应命中嵌套命名空间函数：{md}");
    }

    /// 嵌套命名空间跳转：`tcmsg.error.no_file` 的 no_file → 定义处（error 命名空间内）。
    ///
    /// 旧实现场景一（`.` 前）只查 methods/fields 裸名，查不到 funcs 里的全名。
    #[test]
    fn 嵌套命名空间跳转命中函数定义() {
        let src = 嵌套命名空间源码();
        // no_file 定义在 `        func no_file() -> string {`（LSP line 6，character 13）
        let range = definition(src, 13, 25, None).expect("应命中 no_file 定义");
        assert_eq!(range.start.line, 6, "定义应在 LSP line 6");
        assert_eq!(range.start.character, 13, "定义应从 character 13 开始");
    }

    /// 嵌套命名空间补全：`tcmsg.error.` 点场景 → 只补该层命名空间函数。
    ///
    /// 旧实现 `member_receiver` 只取 `.` 前最后一个标识符（`error`），
    /// 前缀 `error::` 与注册全名 `tcmsg::error::` 不匹配而查不到。
    #[test]
    fn 嵌套命名空间补全点场景() {
        let src = 嵌套命名空间源码();
        // 第 14 行 `    println(tcmsg.error.no_file())`（LSP line 13）：
        // 光标在 `.` 之后（no_file 的 n 之前，character 24）
        let items = completion(src, 13, 24, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"no_file"), "应补全 error 层函数 no_file：{labels:?}");
        // 不应补全上层（tcmsg.hello 不属于 error 层）
        assert!(!labels.contains(&"hello"), "不应补全上层函数 hello：{labels:?}");
    }

    /// 嵌套命名空间补全：`tcmsg.` 点场景 → 子命名空间 error 作为成员出现。
    #[test]
    fn 嵌套命名空间顶层补全子命名空间() {
        let src = 嵌套命名空间源码();
        // 第 13 行 `    println(tcmsg.hello())`（LSP line 12）：
        // 光标在 `tcmsg.` 之后（character 18 = `.` 的后一位）
        let items = completion(src, 12, 18, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"hello"), "应补全 tcmsg 层函数 hello：{labels:?}");
        assert!(labels.contains(&"error"), "应补全子命名空间 error：{labels:?}");
        assert!(!labels.contains(&"no_file"), "不应补全深层函数 no_file：{labels:?}");
    }

    // ==================== 参数跳转 ====================

    /// 参数跳转：函数体内引用形参名 → 跳到参数声明处。
    ///
    /// 旧实现 `collect_stmt_defs` 只收集 VarDecl，形参不在表中。
    #[test]
    fn 参数引用跳转命中参数声明() {
        let src = r#"
func add(a: i64, b: i64) -> i64 {
    return a + b
}
func main() {
    println(add(1, 2))
}
"#;
        // 第 3 行 `    return a + b`（LSP line 2）：a 起始 character 11
        let range = definition(src, 2, 11, None).expect("参数 a 应可跳转");
        assert_eq!(range.start.line, 1, "参数 a 定义应在 LSP line 1");
        assert_eq!(range.start.character, 9, "参数 a 定义应从 character 9 开始");
    }

    /// 方法参数跳转：方法体内引用形参 → 跳到方法签名参数声明处。
    #[test]
    fn 方法参数引用跳转命中参数声明() {
        let src = r#"
class Point {
    var x: i64
    var y: i64
    method dist(o: Point) -> i64 {
        return o.x
    }
}
func main() {
    var p = Point(1, 2)
    println(p.dist(p))
}
"#;
        // 第 6 行 `        return o.x`（LSP line 5）：参数 o 起始 character 15；
        // 形参 o 声明在 LSP line 4 `    method dist(o: Point)`，character 16
        let range = definition(src, 5, 15, None).expect("参数 o 应可跳转");
        assert_eq!(range.start.line, 4, "参数 o 定义应在 LSP line 4");
        assert_eq!(range.start.character, 16, "参数 o 定义应从 character 16 开始");
    }

    // ==================== 语义高亮（semanticTokens） ====================

    /// semanticTokens：把增量编码解码为 (类型, 位置) 列表（测试用辅助）。
    fn 解码语义token(src: &str, data: &[u32]) -> Vec<(u32, u32, u32, String)> {
        let mut out = Vec::new();
        let mut line = 0u32;
        let mut char = 0u32;
        for chunk in data.chunks(5) {
            let (dl, dc, len, ty, _mod) = (chunk[0], chunk[1], chunk[2], chunk[3], chunk[4]);
            line += dl;
            if dl == 0 {
                char += dc;
            } else {
                char = dc;
            }
            let _ = src;
            out.push((line, char, len, semantic_token_types()[ty as usize].clone()));
        }
        out
    }

    /// 语义高亮：嵌套命名空间调用的链段分类正确。
    /// - 链首段（tcmsg）/中间段（error）→ namespace；
    /// - 末段函数（hello/no_file）→ function；
    /// - 命名空间声明路径 → namespace。
    #[test]
    fn 语义高亮嵌套命名空间链段分类() {
        let src = 嵌套命名空间源码();
        let data = semantic_tokens(src, None);
        assert!(!data.is_empty(), "应产出语义 token");
        let toks = 解码语义token(src, &data);
        // 命名空间声明：`namespace tcmsg`（LSP line 1，col 10）
        let tcmsg_decl = toks.iter().find(|(l, c, _, _)| *l == 1 && *c == 10);
        assert_eq!(tcmsg_decl.map(|t| t.3.as_str()), Some("namespace"), "声明 tcmsg 应为 namespace");
        // `namespace error`（LSP line 5，col 14）
        let error_decl = toks.iter().find(|(l, c, _, _)| *l == 5 && *c == 14);
        assert_eq!(error_decl.map(|t| t.3.as_str()), Some("namespace"), "声明 error 应为 namespace");
        // 调用链 `tcmsg.error.no_file`（LSP line 13）：
        // tcmsg（col 12）→ namespace；error（col 18）→ namespace；no_file（col 24）→ function
        let chain = toks
            .iter()
            .filter(|(l, _, _, _)| *l == 13)
            .map(|(_, c, _, ty)| (*c, ty.clone()))
            .collect::<Vec<_>>();
        assert!(chain.contains(&(12, "namespace".into())), "tcmsg 应为 namespace：{chain:?}");
        assert!(chain.contains(&(18, "namespace".into())), "error 应为 namespace：{chain:?}");
        assert!(chain.contains(&(24, "function".into())), "no_file 应为 function：{chain:?}");
    }

    /// 语义高亮：形参声明 → parameter；变量声明 → variable。
    #[test]
    fn 语义高亮参数与变量分类() {
        let src = r#"
func add(a: i64, b: i64) -> i64 {
    var sum = a + b
    return sum
}
"#;
        let data = semantic_tokens(src, None);
        let toks = 解码语义token(src, &data);
        // 形参 a（LSP line 1，col 9）与 b（col 17）→ parameter
        let a = toks.iter().find(|(l, c, _, _)| *l == 1 && *c == 9);
        assert_eq!(a.map(|t| t.3.as_str()), Some("parameter"), "形参 a 应为 parameter");
        let b = toks.iter().find(|(l, c, _, _)| *l == 1 && *c == 17);
        assert_eq!(b.map(|t| t.3.as_str()), Some("parameter"), "形参 b 应为 parameter");
        // 变量 sum 声明（LSP line 2，col 8）→ variable
        let sum = toks.iter().find(|(l, c, _, _)| *l == 2 && *c == 8);
        assert_eq!(sum.map(|t| t.3.as_str()), Some("variable"), "变量 sum 应为 variable");
    }

    /// 语义高亮：实例方法调用（`p.dist(`）→ method；字段访问（`p.x`）→ property。
    #[test]
    fn 语义高亮方法与字段分类() {
        let src = r#"
class Point {
    var x: i64
    var y: i64
    method dist() -> i64 {
        return this.x
    }
    static method create() -> Point {
        return Point(1, 2)
    }
}
func main() {
    var p = Point(1, 2)
    println(p.dist())
    var q = p.x
}
"#;
        let data = semantic_tokens(src, None);
        let toks = 解码语义token(src, &data);
        // 方法定义名 dist（LSP line 4，col 11）→ method
        let dist_def = toks.iter().find(|(l, c, _, _)| *l == 4 && *c == 11);
        assert_eq!(dist_def.map(|t| t.3.as_str()), Some("method"), "方法定义 dist 应为 method");
        // 类名 Point 引用（LSP line 12，col 12）→ class
        let point_ref = toks.iter().find(|(l, c, _, _)| *l == 12 && *c == 12);
        assert_eq!(point_ref.map(|t| t.3.as_str()), Some("class"), "类引用 Point 应为 class");
        // 方法调用 p.dist（LSP line 13，col 14）→ method
        let dist_call = toks.iter().find(|(l, c, _, _)| *l == 13 && *c == 14);
        assert_eq!(dist_call.map(|t| t.3.as_str()), Some("method"), "方法调用 dist 应为 method");
        // 字段访问 p.x（LSP line 14，col 14）→ property
        let x_field = toks.iter().find(|(l, c, _, _)| *l == 14 && *c == 14);
        assert_eq!(x_field.map(|t| t.3.as_str()), Some("property"), "字段访问 x 应为 property");
    }
}
