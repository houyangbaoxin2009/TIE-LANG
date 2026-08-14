//! 语法分析器（Parser）。
//!
//! 职责：递归下降解析 token 流生成 AST。
//!
//! 说明：文件头部（`// tie:` 指令）已由 tie-prep 预处理阶段提取，
//! 本解析器只处理清理后的正文源码。

use super::ast::{
    AssignStmt, BinaryOp, BreakStmt, ClassField, ContinueStmt, Expr, ExprStmt, ExternDeclStmt,
    FieldAssignStmt, FnDefStmt, ForStmt, IfStmt, ImportStmt, IndexAssignStmt, NamespaceStmt, Param,
    Program, ReturnStmt, Stmt, StructDefStmt, SwitchCase, SwitchStmt, TableCell, TableId,
    TupleField, TypeSpec, UnaryOp, UsingStmt, VarDeclStmt, WhileStmt,
};
use super::lexer::{Span, Token, TokenKind, TyKw};
use std::fmt;

/// 语法错误：携带位置与信息。
#[derive(Debug, Clone)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "语法错误 @{}:{}: {}", self.span.line, self.span.col, self.message)
    }
}

/// 解析入口：token 流 → 程序 AST。
pub fn parse_program(tokens: &[Token]) -> Result<Program, ParseError> {
    Parser::new(tokens).parse_program()
}

/// 递归下降解析器。
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// 解构 desugar 用的临时变量计数器（生成 `_tmpN` 唯一名）
    tmp_counter: usize,
}

impl Parser {
    fn new(tokens: &[Token]) -> Self {
        // to_vec：类型参数闭括号分裂（E1，table<table<T>>）需要原地插入 token
        Self { tokens: tokens.to_vec(), pos: 0, tmp_counter: 0 }
    }

    // ---------- 基础游标 ----------

    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    /// 推进游标并返回被消费的 token（克隆，避免借用冲突）。
    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos.min(self.tokens.len() - 1)].clone();
        if !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        tok
    }

    /// 当前 token 是否匹配某种类（并推进）。
    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek_kind() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    /// 期望当前 token 为某类，否则报错。
    fn expect(&mut self, kind: TokenKind, what: &str) -> Result<(), ParseError> {
        if self.peek_kind() == &kind {
            self.advance();
            Ok(())
        } else {
            Err(self.err(format!("期望 {what}，实际是 {}", self.describe(self.peek_kind()))))
        }
    }

    /// 期望当前 token 为标识符。
    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.peek_kind() {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            _ => Err(self.err(format!("期望标识符，实际是 {}", self.describe(self.peek_kind())))),
        }
    }

    /// 构造当前 token 位置上的错误。
    fn err(&self, message: String) -> ParseError {
        ParseError { span: self.peek().span, message }
    }

    /// token 种类的可读描述。
    fn describe(&self, kind: &TokenKind) -> String {
        match kind {
            TokenKind::Ident(n) => format!("标识符 '{n}'"),
            TokenKind::Int(v) => format!("整数 {v}"),
            TokenKind::Float(v) => format!("浮点数 {v}"),
            TokenKind::Str(_) => "字符串".into(),
            TokenKind::TypeKw(t) => format!("类型 '{}'", t.as_str()),
            TokenKind::Semi => "分号".into(),
            TokenKind::Eof => "文件结束".into(),
            other => format!("{other:?}"),
        }
    }

    // ---------- 程序解析 ----------

    fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut stmts = Vec::new();
        // 顶层只允许函数定义、import、using、struct、命名空间声明、extern 声明
        // 与全局持久变量（var/const，M4：跨函数共享的可变状态；import 由
        // driver 递归展开为函数）。extern 仅顶层（函数体内由 parse_stmt 拦截）。
        // 允许的语句集合由共享的 parse_top_level_stmt 统一维护，保证与
        // 单文件命名空间体完全一致。
        while let Some(decls) = self.parse_top_level_stmt()? {
            stmts.extend(decls);
        }
        Ok(Program { stmts })
    }

    /// 解析一条顶层语句，返回其产生的语句列表（var/const 元组解构会展开为多条）。
    ///
    /// 返回 None 表示已到达文件末尾（EOF）。parse_program 与单文件命名空间体
    /// 共用此函数，保证两处允许的顶层语句集合完全一致。
    ///
    /// 顶层允许：函数定义（func/pub）、import、using、struct、命名空间声明、
    /// extern 声明与全局持久变量（var/const）。**单文件命名空间体复用同一集合**
    /// ——因为单文件模式下文件的全部内容都被包裹进命名空间，其中出现的
    /// import/using/extern/var/const 应保持合法（import/using 由 imports.rs
    /// 在 AST 层面展开，与所在命名空间层级无关；这比块式体内仅允许
    /// 函数/struct/嵌套命名空间更宽松，是刻意为之）。
    fn parse_top_level_stmt(&mut self) -> Result<Option<Vec<Stmt>>, ParseError> {
        if matches!(self.peek_kind(), TokenKind::Eof) {
            return Ok(None);
        }
        match self.peek_kind() {
            TokenKind::Func | TokenKind::Pub => Ok(Some(vec![Stmt::FnDef(self.parse_fn_def()?)])),
            TokenKind::Import => Ok(Some(vec![Stmt::Import(self.parse_import()?)])),
            TokenKind::Using => Ok(Some(vec![Stmt::Using(self.parse_using()?)])),
            TokenKind::Struct => Ok(Some(vec![Stmt::Struct(self.parse_struct()?)])),
            TokenKind::Namespace => Ok(Some(vec![Stmt::Namespace(self.parse_namespace()?)])),
            // T0.7 extern 函数声明：`extern fn name(...) -> ret;`（仅顶层）
            TokenKind::Extern => Ok(Some(vec![Stmt::Extern(self.parse_extern()?)])),
            // 顶层全局持久变量（跨函数共享；限标量类型 + 字面量初始化，语义层校验）
            TokenKind::Var | TokenKind::Const => {
                let is_const = matches!(self.peek_kind(), TokenKind::Const);
                Ok(Some(
                    self.parse_var_decl(is_const)?.into_iter().map(Stmt::VarDecl).collect(),
                ))
            }
            other => Err(self.err(format!(
                "顶层只允许函数定义、import、using、struct、命名空间声明、extern 声明或全局变量，实际是 {}",
                self.describe(other)
            ))),
        }
    }

    /// 命名空间声明，三种形态：
    ///
    /// 1. **块式** `namespace tcmsg { ... }` 或点分 `namespace tcmsg.error { ... }`：
    ///    体内语句限定为函数定义 / struct / 嵌套命名空间，以 `}` 收尾；
    /// 2. **单文件（ASI 分号）** `namespace tcmsg` 独占一行后接文件剩余内容：
    ///    ASI 词法器在行尾补 `Semi`，本函数把**文件剩余的全部顶层语句**归入该
    ///    命名空间，直到文件结束（无需闭合花括号）；
    /// 3. **单文件（显式分号）** `namespace tcmsg;`：与形态 2 完全等价。
    ///
    /// 形态 2/3 的 `;` 只是可选的标记——`namespace foo` 换行（ASI 补分号）与
    /// 手写 `namespace foo;` 产生完全一致的 AST。二者都表示「从声明处起，整份
    /// 文件被包裹进该命名空间」，体内允许与 parse_program 顶层一致的语句集合
    /// （函数/import/using/struct/namespace/extern/var/const，见
    /// parse_top_level_stmt 的说明）。
    ///
    /// 单文件模式下的嵌套 `namespace` 经本函数递归处理：块式嵌套以 `{ }` 界定
    /// 边界；单文件嵌套继续把声明之后的剩余内容归入内层命名空间。EOF 自然结束
    /// 最外层单文件命名空间体。
    fn parse_namespace(&mut self) -> Result<NamespaceStmt, ParseError> {
        let span = self.advance().span; // 消费 `namespace`
        // 路径：至少一段标识符，可点分（`tcmsg.error`）
        let mut path = vec![self.expect_ident()?];
        while self.eat(&TokenKind::Dot) {
            path.push(self.expect_ident()?);
        }
        // 分支：`{` → 块式；其余（含 `;` 与 EOF）→ 单文件模式
        if self.eat(&TokenKind::LBrace) {
            // 块式：体内语句限定为函数 / struct / 嵌套命名空间
            let mut body = Vec::new();
            while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                match self.peek_kind() {
                    TokenKind::Func | TokenKind::Pub => body.push(Stmt::FnDef(self.parse_fn_def()?)),
                    TokenKind::Struct => body.push(Stmt::Struct(self.parse_struct()?)),
                    TokenKind::Namespace => body.push(Stmt::Namespace(self.parse_namespace()?)),
                    other => {
                        return Err(self.err(format!(
                            "命名空间体内只允许函数定义、类定义或嵌套命名空间，实际是 {}",
                            self.describe(other)
                        )))
                    }
                }
            }
            self.expect(TokenKind::RBrace, "命名空间声明必须跟 '}'")?;
            Ok(NamespaceStmt { path, body, span })
        } else {
            // 单文件模式：先吃掉可选的 `;`（ASI 在 `namespace foo` 行尾补出的
            // 分号，或手写 `namespace foo;` 的显式分号，二者等价），随后解析
            // 文件剩余的全部顶层语句直至 EOF 作为命名空间体。
            self.eat(&TokenKind::Semi);
            let mut body = Vec::new();
            while let Some(decls) = self.parse_top_level_stmt()? {
                body.extend(decls);
            }
            Ok(NamespaceStmt { path, body, span })
        }
    }

    /// import 语句：`import "./x.tie"` 或 `import "./x.tie" as 别名`。
    ///
    /// 路径必须是字符串字面量（相对当前文件所在目录）；`as 别名` 可选。
    fn parse_import(&mut self) -> Result<ImportStmt, ParseError> {
        let span = self.advance().span; // 消费 `import`
        // 路径：字符串字面量
        let path = match self.peek_kind() {
            TokenKind::Str(s) => {
                let s = s.clone();
                self.advance();
                s
            }
            other => {
                return Err(self.err(format!(
                    "import 后必须是字符串路径，实际是 {}",
                    self.describe(other)
                )))
            }
        };
        // 可选别名：`as 标识符`
        let alias = if self.eat(&TokenKind::As) {
            match self.peek_kind() {
                TokenKind::Ident(name) => {
                    let name = name.clone();
                    self.advance();
                    Some(name)
                }
                other => {
                    return Err(self.err(format!(
                        "as 后必须是别名标识符，实际是 {}",
                        self.describe(other)
                    )))
                }
            }
        } else {
            None
        };
        self.expect(TokenKind::Semi, "import 语句结束的分号")?;
        // ns_paths 由 imports.rs 展开时填充（parser 阶段未知被导入文件的命名空间）
        Ok(ImportStmt { path, alias, ns_paths: Vec::new(), span })
    }

    /// using 引入语句（M2.1.7）：`using fmt2;` 或 `using fmt.error;`（仅顶层）。
    ///
    /// 至少一段标识符，可点分（`using fmt.error` 表示命名空间路径）；目标必须是
    /// 已 import 引入的命名空间前缀或别名（语义层校验）。
    fn parse_using(&mut self) -> Result<UsingStmt, ParseError> {
        let span = self.advance().span; // 消费 `using`
        let mut path = vec![self.expect_ident()?];
        while self.eat(&TokenKind::Dot) {
            path.push(self.expect_ident()?);
        }
        self.expect(TokenKind::Semi, "using 语句结束的分号")?;
        Ok(UsingStmt { span, path })
    }

    // ---------- 语句解析 ----------

    /// 解析一条或多条语句（var/const 元组解构会展开为多条）。
    ///
    /// 返回 Vec 的原因：`var (q, r) = divmod(10, 3);` 在解析层 desugar 为
    /// 临时变量声明 + 逐字段声明两条语句（见 parse_var_decl）。
    fn parse_stmt(&mut self) -> Result<Vec<Stmt>, ParseError> {
        match self.peek_kind() {
            TokenKind::Var => {
                self.parse_var_decl(false).map(|ds| ds.into_iter().map(Stmt::VarDecl).collect())
            }
            TokenKind::Const => {
                self.parse_var_decl(true).map(|ds| ds.into_iter().map(Stmt::VarDecl).collect())
            }
            TokenKind::If => Ok(vec![Stmt::If(self.parse_if()?)]),
            TokenKind::While => Ok(vec![Stmt::While(self.parse_while(None)?)]),
            TokenKind::For => Ok(vec![Stmt::For(self.parse_for(None)?)]),
            TokenKind::Switch => Ok(vec![Stmt::Switch(self.parse_switch()?)]),
            TokenKind::Return => Ok(vec![Stmt::Return(self.parse_return()?)]),
            TokenKind::Break => Ok(vec![Stmt::Break(self.parse_loop_jump(false)?)]),
            TokenKind::Continue => {
                let b = self.parse_loop_jump(true)?;
                Ok(vec![Stmt::Continue(ContinueStmt { label: b.label, span: b.span })])
            }
            // 循环标签（E5）：`L: while cond { }` / `L: for x in ... { }`——
            // 语句开头的「标识符 + 冒号」只可能是循环标签，转发到带标签解析
            TokenKind::Ident(_) if self.is_loop_label_ahead() => {
                let label = self.expect_ident()?; // 已确认是标识符，直接取名
                let _ = self.advance(); // 冒号
                match self.peek_kind() {
                    TokenKind::While => Ok(vec![Stmt::While(self.parse_while(Some(label))?)]),
                    TokenKind::For => Ok(vec![Stmt::For(self.parse_for(Some(label))?)]),
                    _ => Err(self.err(format!("标签 '{label}' 后必须是 while 或 for"))),
                }
            }
            TokenKind::LBrace => {
                // 裸块（后续版本），此处按语法错误处理
                Err(self.err("函数体内不能有裸代码块".into()))
            }
            // T0.7 extern 声明：仅顶层合法（extern 是链接期符号声明，无函数体）——
            // 函数体内直接报语法错误（与 FnDef 的"函数体内不支持嵌套函数"同层级拦截）
            TokenKind::Extern => {
                Err(self.err("extern 声明只能出现在文件顶层".into()))
            }
            _ => Ok(vec![self.parse_expr_or_assign()?]),
        }
    }

    /// 表达式语句与赋值语句的统一入口：
    /// `Ident op= ...`（变量名后紧跟赋值运算符）→ Assign；`obj.field op= ...` → FieldAssign；
    /// 否则解析为普通表达式语句。
    fn parse_expr_or_assign(&mut self) -> Result<Stmt, ParseError> {
        // 快速路径：变量名后紧跟赋值运算符（`=` 或 `+=` 等复合赋值）→ Assign
        if let TokenKind::Ident(name) = self.peek_kind() {
            let name = name.clone();
            if self.tokens.get(self.pos + 1).map(|t| is_assign_op_kind(&t.kind)).unwrap_or(false) {
                let span = self.advance().span; // 目标变量名
                // 预探测已确认是赋值运算符，flatten 取出内层 Option<BinaryOp>（Eq→None）
                let op = self.eat_assign_op().flatten();
                let value = self.parse_expr()?;
                self.expect(TokenKind::Semi, "语句结束符")?;
                return Ok(Stmt::Assign(AssignStmt { target: name, op, value, span }));
            }
        }
        // 字段赋值：`obj.field op= expr`（base 限定 Var/this 或字段链，见语义层校验）。
        // 先解析完整表达式，若后跟赋值运算符且表达式是纯字段访问链 → FieldAssign。
        let expr = self.parse_expr()?;
        if let Some(op) = self.eat_assign_op() {
            match &expr {
                // 表下标赋值（M4 补齐）：`t[i] op= v` / `t[i][j] op= v`（target 是 Index 链）。
                // 语义层校验 base 是表变量且可寻址；此处语法层放行任意 Index 链。
                Expr::Index { .. } => {
                    let value = self.parse_expr()?;
                    let ispan = expr_span(&expr).unwrap_or(self.peek().span);
                    self.expect(TokenKind::Semi, "语句结束符")?;
                    return Ok(Stmt::IndexAssign(IndexAssignStmt {
                        target: Box::new(expr),
                        op,
                        value,
                        span: ispan,
                    }));
                }
                // base 必须可寻址：变量/this 或 FieldAccess 链（`obj.a.b = v`）
                Expr::FieldAccess { base, field, span } if is_addressable_base(base) => {
                    let value = self.parse_expr()?;
                    let fspan = *span;
                    self.expect(TokenKind::Semi, "语句结束符")?;
                    return Ok(Stmt::FieldAssign(FieldAssignStmt {
                        base: base.clone(),
                        field: field.clone(),
                        op,
                        value,
                        span: fspan,
                    }));
                }
                // 变量赋值（理论上前面的快速路径已拦截，此处防御）
                Expr::Var(name) => {
                    let value = self.parse_expr()?;
                    let aspan = expr_span(&expr).unwrap_or(self.peek().span);
                    self.expect(TokenKind::Semi, "语句结束符")?;
                    return Ok(Stmt::Assign(AssignStmt {
                        target: name.clone(),
                        op,
                        value,
                        span: aspan,
                    }));
                }
                _ => {
                    return Err(self.err("赋值目标必须是变量或对象字段".into()));
                }
            }
        }
        self.parse_expr_stmt_tail(expr).map(Stmt::Expr)
    }

    /// 尝试消费一个赋值运算符（`=` 或复合赋值 `+=` 等）。
    ///
    /// 返回值的三层含义：
    /// - `None`：当前 token 不是赋值运算符（没匹配到）；
    /// - `Some(None)`：普通赋值 `=`；
    /// - `Some(Some(op))`：复合赋值（如 `+=` → BinaryOp::Add）。
    fn eat_assign_op(&mut self) -> Option<Option<BinaryOp>> {
        let op = match self.peek_kind() {
            TokenKind::Eq => None,
            TokenKind::PlusEq => Some(BinaryOp::Add),
            TokenKind::MinusEq => Some(BinaryOp::Sub),
            TokenKind::StarEq => Some(BinaryOp::Mul),
            TokenKind::SlashEq => Some(BinaryOp::Div),
            TokenKind::PercentEq => Some(BinaryOp::Mod),
            TokenKind::AmpEq => Some(BinaryOp::BitAnd),
            TokenKind::PipeEq => Some(BinaryOp::BitOr),
            TokenKind::CaretEq => Some(BinaryOp::BitXor),
            TokenKind::ShlEq => Some(BinaryOp::Shl),
            TokenKind::ShrEq => Some(BinaryOp::Shr),
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    /// 表达式语句尾处理：以分号/ASI 结束。
    fn parse_expr_stmt_tail(&mut self, expr: Expr) -> Result<ExprStmt, ParseError> {
        let span = expr_span(&expr).unwrap_or(self.peek().span);
        self.expect(TokenKind::Semi, "语句结束符")?;
        Ok(ExprStmt { expr, span })
    }

    /// `var name[: Ty] = expr` / `const name[: Ty] = expr`（ASI/分号结束）。
    ///
    /// 元组解构：`var (q, r) = expr;` → 解析层 desugar 为
    /// `var _tmpN = expr; var q = _tmpN.Item1; var r = _tmpN.Item2;`
    /// （临时变量不可变共享源值，字段声明继承用户的 const 标记）。
    fn parse_var_decl(&mut self, is_const: bool) -> Result<Vec<VarDeclStmt>, ParseError> {
        let span = self.advance().span; // var / const
        // 元组解构：变量名位置是左括号
        if matches!(self.peek_kind(), TokenKind::LParen) {
            return self.parse_tuple_destructure(span, is_const);
        }
        let name = self.expect_ident()?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        // T0.4 顶层表全局变量：显式标注表类型（table<T> / 裸 table）时允许省略
        // 初始化器（`var g: table<i64>;`）——解析层合成空表字面量 []（元素类型
        // 由标注决定，语义层登记元数据；运行时在 main 入口创建）。
        // 仅表类型允许省略；其余类型保持既有「期望 '='」错误行为（错误契约不变）。
        if self.eat(&TokenKind::Eq) {
            let init = self.parse_expr()?;
            self.expect(TokenKind::Semi, "语句结束符")?;
            Ok(vec![VarDeclStmt { name, ty, init, span, is_const }])
        } else if matches!(
            &ty,
            Some(TypeSpec::Table(_)) | Some(TypeSpec::Named(TyKw::Table))
        ) {
            self.expect(TokenKind::Semi, "语句结束符")?;
            Ok(vec![VarDeclStmt {
                name,
                ty,
                init: Expr::TableLit { cells: Vec::new(), span },
                span,
                is_const,
            }])
        } else {
            self.expect(TokenKind::Eq, "'='")?;
            unreachable!("expect 失败已返回 Err")
        }
    }

    /// 元组解构声明：`var (q, r) = expr;`（desugar 为临时变量 + 逐字段声明）。
    ///
    /// 仅支持标识符列表（嵌套解构、忽略占位 `_` 留待后续版本）。
    fn parse_tuple_destructure(
        &mut self,
        span: Span,
        is_const: bool,
    ) -> Result<Vec<VarDeclStmt>, ParseError> {
        self.expect(TokenKind::LParen, "'('")?;
        let mut names = Vec::new();
        // 空解构 `var () = ...` 不支持
        if self.eat(&TokenKind::RParen) {
            return Err(ParseError {
                span,
                message: "空解构 () 不支持".into(),
            });
        }
        loop {
            names.push(self.expect_ident()?);
            if self.eat(&TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RParen, "')'")?;
            break;
        }
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        self.expect(TokenKind::Semi, "语句结束符")?;
        // 生成唯一临时变量名 `_tmpN`（下划线开头，避免与用户变量冲突）
        let tmp = format!("_tmp{}", self.tmp_counter);
        self.tmp_counter += 1;
        // 第一条：临时变量持有元组值（const 语义由后续字段声明承载，临时变量可写）
        let mut decls = vec![VarDeclStmt {
            name: tmp.clone(),
            ty: None,
            init,
            span,
            is_const: false,
        }];
        // 逐字段：`var q = _tmpN.Item1;`（按 C# 规则 ItemN 从 1 编号）
        for (i, name) in names.iter().enumerate() {
            let access = format!("Item{}", i + 1);
            let field = Expr::FieldAccess {
                base: Box::new(Expr::Var(tmp.clone())),
                field: access,
                span,
            };
            decls.push(VarDeclStmt {
                name: name.clone(),
                ty: None,
                init: field,
                span,
                is_const,
            });
        }
        Ok(decls)
    }

    /// extern 函数声明（T0.7）：`extern fn name(a: i64, b: string) -> bool;`
    ///
    /// 语法与普通函数定义一致（含 `-> 返回类型`，缺省 void），但**无函数体**
    /// （声明的是链接期外部符号）；以分号结束而非代码块。参数限标量类型、
    /// 返回限标量或 void——parser 只负责解析，类型校验交给语义层。
    fn parse_extern(&mut self) -> Result<ExternDeclStmt, ParseError> {
        let span = self.advance().span; // extern
        // extern 声明语法为 `extern fn name(...)`——`fn` 是 extern 声明的固定
        // 前缀（与普通函数定义的 `func` 区分，贴近 C/Rust 风格）。不把 `fn`
        // 引入全局关键字表（避免破坏现有以 fn 为标识符的代码），此处按固定
        // 标识符文本匹配。
        match self.peek_kind() {
            TokenKind::Ident(s) if s == "fn" => {
                self.advance();
            }
            other => {
                return Err(self.err(format!(
                    "extern 声明必须跟 'fn'，实际是 {}",
                    self.describe(other)
                )))
            }
        }
        let name = self.expect_ident()?;
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        if !self.eat(&TokenKind::RParen) {
            loop {
                let pspan = self.peek().span;
                let pname = self.expect_ident()?;
                self.expect(TokenKind::Colon, "':'")?;
                // extern 形参不支持 ref/默认值/变参（链接期声明无调用语义）；类型限标量
                let pty = self.parse_type()?;
                params.push(Param { name: pname, ty: pty, default: None, by_ref: false, variadic: false, span: pspan });
                if self.eat(&TokenKind::Comma) {
                    continue;
                }
                self.expect(TokenKind::RParen, "')'")?;
                break;
            }
        }
        // 返回类型：`-> Ty` 可省略（默认 void）
        let ret_ty = if self.eat(&TokenKind::Arrow) {
            self.parse_type()?
        } else {
            TypeSpec::Named(TyKw::Void)
        };
        // extern 声明以分号结束（无函数体）
        self.expect(TokenKind::Semi, "extern 声明结束的分号")?;
        Ok(ExternDeclStmt { name, params, ret_ty, span })
    }

    /// `[pub] func name(params) -> Ty { stmts }`（pub 为 M2.1.7 可见性标记）。
    fn parse_fn_def(&mut self) -> Result<FnDefStmt, ParseError> {        let span = self.peek().span; // pub 或 func 所在位置
        // 可选 pub 标记：`pub func name(...)`。命名空间内默认私有，pub 显式导出；
        // 顶层函数恒公有（pub 冗余但合法）。
        let is_pub = self.eat(&TokenKind::Pub);
        self.expect(TokenKind::Func, "'func'")?;
        let name = self.expect_ident()?;
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        if !self.eat(&TokenKind::RParen) {
            loop {
                let pspan = self.peek().span;
                let pname = self.expect_ident()?;
                self.expect(TokenKind::Colon, "':'")?;
                // 按引用传递修饰（T0.3 by_ref）：`name: ref table<T>`——ref 表形参
                // 是真引用（内容修改/重绑定写回调用方）。仅限表参数（语义层校验）。
                let by_ref = self.eat(&TokenKind::Ref);
                // 变参标记（特性④）：`name: ...T`——接收 0..n 个实参，函数体内
                // 打包为动态表（table<T>）。`...` 与 ref/默认值互斥（语法层拦截）。
                let variadic = self.eat(&TokenKind::DotDotDot);
                if variadic && by_ref {
                    return Err(self.err(format!(
                        "参数 '{}' 的变参（...）不能与 ref 修饰同时使用",
                        pname
                    )));
                }
                let pty = self.parse_type()?;
                // 默认值（可选参数）：`name: Ty = 字面量`。限字面量（含空表 []），
                // 与类字段默认值规则一致（语义层校验类型，语法层只负责解析）。
                // ref 参数不允许默认值（语义层校验：引用目标无法由默认值表达）；
                // 变参参数不允许默认值（语法层直接拦截，实参个数由调用点决定）。
                let default = if variadic && *self.peek_kind() == TokenKind::Eq {
                    return Err(self.err(format!(
                        "参数 '{}' 的变参（...）不能有默认值",
                        pname
                    )));
                } else if self.eat(&TokenKind::Eq) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                params.push(Param { name: pname, ty: pty, default, by_ref, variadic, span: pspan });
                if self.eat(&TokenKind::Comma) {
                    // 变参必须是最后一个参数：变参后仍有逗号 → 语法错误
                    if variadic {
                        return Err(self.err("变参（...）必须是函数的最后一个参数".into()));
                    }
                    continue;
                }
                self.expect(TokenKind::RParen, "')'")?;
                break;
            }
        }
        // 返回类型：`-> Ty` 可省略（默认 void）
        let ret_ty = if self.eat(&TokenKind::Arrow) {
            self.parse_type()?
        } else {
            TypeSpec::Named(TyKw::Void)
        };
        let body = self.parse_block()?;
        Ok(FnDefStmt { name, params, ret_ty, is_pub, body, span })
    }

    /// `struct Name [extends Parent] { 字段 }`（纯数据，仅顶层/命名空间体）。
    ///
    /// struct 体**只允许字段声明**（`var name[: Ty] [= 默认值]`）——方法已移出，
    /// 由绑定 struct 名的命名空间函数定义（`namespace Point { pub func dist(p: Point) }`），
    /// `p.dist()` 调用由语义层转发。方法语法出现在 struct 体内 → 报错提示。
    fn parse_struct(&mut self) -> Result<StructDefStmt, ParseError> {
        let span = self.advance().span; // struct
        let name = self.expect_ident()?;
        // 继承：`extends Parent`（字段拍平）
        let parent = if self.eat(&TokenKind::Extends) {
            Some(self.expect_ident()?)
        } else {
            None
        };
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut fields = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            match self.peek_kind() {
                // 字段：`var name[: Ty] [= 默认值]`
                TokenKind::Var => {
                    let fspan = self.advance().span; // var
                    let fname = self.expect_ident()?;
                    let ty = if self.eat(&TokenKind::Colon) {
                        Some(self.parse_type()?)
                    } else {
                        None
                    };
                    let init = if self.eat(&TokenKind::Eq) {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    self.expect(TokenKind::Semi, "字段声明结束符")?;
                    fields.push(ClassField { name: fname, ty, init, span: fspan });
                }
                // 方法语法（M2.1.8 已移出 struct）：报错并提示新写法
                TokenKind::Func | TokenKind::Pub => {
                    return Err(self.err(format!(
                        "struct 体不允许方法定义：逻辑请用绑定 struct 名的命名空间函数定义 \
                         （namespace 数据名 {{ pub func 方法名(首参: 数据名) ... }}），调用仍写 obj.method()"
                    )))
                }
                other => {
                    return Err(self.err(format!(
                        "struct 体内只允许字段(var)，实际是 {}",
                        self.describe(other)
                    )))
                }
            }
        }
        self.expect(TokenKind::RBrace, "'}'")?;
        Ok(StructDefStmt { name, parent, fields, span })
    }

    /// `{ stmts }` 代码块。
    fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut stmts = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            stmts.extend(self.parse_stmt()?);
        }
        self.expect(TokenKind::RBrace, "'}'")?;
        Ok(stmts)
    }

    /// `if cond { } else { }` / `else if` 链。
    fn parse_if(&mut self) -> Result<IfStmt, ParseError> {
        let span = self.advance().span; // if
        let cond = self.parse_expr()?;
        let then_branch = self.parse_block()?;
        let else_branch = if self.eat(&TokenKind::Else) {
            // else if 折叠为嵌套 IfStmt
            if matches!(self.peek_kind(), TokenKind::If) {
                vec![Stmt::If(self.parse_if()?)]
            } else {
                self.parse_block()?
            }
        } else {
            Vec::new()
        };
        Ok(IfStmt { cond, then_branch, else_branch, span })
    }

    /// `while cond { }`（label 为循环标签，E5）。
    fn parse_while(&mut self, label: Option<String>) -> Result<WhileStmt, ParseError> {
        let span = self.advance().span; // while
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(WhileStmt { cond, body, label, span })
    }

    /// `for var in expr { }`（label 为循环标签，E5）。
    fn parse_for(&mut self, label: Option<String>) -> Result<ForStmt, ParseError> {
        let span = self.advance().span; // for
        let var = self.expect_ident()?;
        self.expect(TokenKind::In, "'in'")?;
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(ForStmt { var, iter, body, label, span })
    }

    /// 探测「循环标签前缀」：当前 token 是标识符，且后跟冒号再后跟 while/for（E5）。
    ///
    /// 语句开头的 `标识符 : while/for` 只可能是循环标签（变量声明的冒号前是 var
    /// 关键字，表达式语句的冒号无合法场景），因此可安全判定。
    fn is_loop_label_ahead(&self) -> bool {
        let Some(TokenKind::Colon) = self.tokens.get(self.pos + 1).map(|t| &t.kind) else {
            return false;
        };
        matches!(
            self.tokens.get(self.pos + 2).map(|t| &t.kind),
            Some(TokenKind::While | TokenKind::For)
        )
    }

    /// `break` / `continue` 语句（E1+E5）：可选标签 `break L` / `continue L`。
    /// `is_continue` 区分两个关键字（仅用于构造对应语句类型，解析逻辑一致）。
    fn parse_loop_jump(&mut self, is_continue: bool) -> Result<BreakStmt, ParseError> {
        let kw = if is_continue { "continue" } else { "break" };
        let span = self.advance().span; // break / continue
        // 可选标签：后跟标识符且再后跟分号/行尾 → 带标签跳转
        let label = match self.peek_kind() {
            TokenKind::Ident(name) if matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::Semi | TokenKind::Eof | TokenKind::RBrace)
            ) => Some(name.clone()),
            _ => None,
        };
        if let Some(_) = &label {
            self.advance(); // 消费标签标识符
        }
        self.expect(TokenKind::Semi, "语句结束符")?;
        // break 与 continue 共用同一结构（仅类型语义不同），此处统一返回 BreakStmt，
        // 调用方按 is_continue 包成 ContinueStmt
        let _ = kw;
        Ok(BreakStmt { label, span })
    }

    /// `switch subject { case 值[, 值]... [when 条件]: 语句… default: 语句… }`。
    ///
    /// 模式匹配增强（规划 switch-pattern-matching）：
    /// - 多值：`case 1, 2:`（逗号分隔，任一相等即命中）；
    /// - 区间：`case 3..7:`（Range 表达式，含 3 不含 7）；
    /// - 守卫：`case 8 when flag:`（值命中且守卫为真才进入）；
    /// - 类型匹配：`case string:`（TypeKw token → TypeLit）。
    ///
    /// case 分支体以行（ASI）或分号结束，遇到下一个 case/default/右花括号时终止。
    /// default 分支可选且至多一个（语法层不强制顺序，语义层校验）。
    fn parse_switch(&mut self) -> Result<SwitchStmt, ParseError> {
        let span = self.advance().span; // switch
        let subject = self.parse_expr()?;
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut cases = Vec::new();
        let mut default_body: Option<Vec<Stmt>> = None;
        // 分支循环：case / default / 右花括号
        loop {
            match self.peek_kind() {
                TokenKind::Case => {
                    let cspan = self.advance().span; // case
                    // 模式列表：`值[, 值]...`（至少一个）
                    let mut patterns = vec![self.parse_case_pattern()?];
                    while self.eat(&TokenKind::Comma) {
                        patterns.push(self.parse_case_pattern()?);
                    }
                    // 可选守卫：`when 条件`
                    let when = if self.eat(&TokenKind::When) {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    self.expect(TokenKind::Colon, "':'")?;
                    let body = self.parse_switch_body()?;
                    cases.push(SwitchCase { patterns, when, body, span: cspan });
                }
                TokenKind::Default => {
                    let _ = self.advance().span; // default
                    self.expect(TokenKind::Colon, "':'")?;
                    default_body = Some(self.parse_switch_body()?);
                }
                TokenKind::RBrace => {
                    self.advance();
                    break;
                }
                _ => {
                    return Err(self.err(format!(
                        "switch 体内只允许 case/default/右花括号，实际是 {}",
                        self.describe(self.peek_kind())
                    )))
                }
            }
        }
        Ok(SwitchStmt {
            subject,
            cases,
            default_body: default_body.unwrap_or_default(),
            span,
        })
    }

    /// case 匹配模式：类型匹配（`case string:` / `case i64:`）或值/区间表达式。
    ///
    /// - 类型关键字（TypeKw）→ `Expr::TypeLit`（类型匹配 pattern）；
    /// - 其余 → `parse_expr`（字面量 / 负数 / 区间 Range）。
    fn parse_case_pattern(&mut self) -> Result<Expr, ParseError> {
        // 类型匹配：case string: / case i64: —— TypeKw token 直接生成 TypeLit
        if let TokenKind::TypeKw(ty) = self.peek_kind() {
            let ty = *ty; // TyKw 是 Copy，先取值再消费 token（避免借用冲突）
            let span = self.advance().span;
            return Ok(Expr::TypeLit { ty: TypeSpec::Named(ty), span });
        }
        // 值/区间：parse_expr 支持字面量、负数、区间（3..7）
        self.parse_expr()
    }

    /// case/default 分支体：连续语句直到下一个 case/default/右花括号/文件结束。
    fn parse_switch_body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        while !matches!(
            self.peek_kind(),
            TokenKind::Case | TokenKind::Default | TokenKind::RBrace | TokenKind::Eof
        ) {
            stmts.extend(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    /// `return [expr]`。
    fn parse_return(&mut self) -> Result<ReturnStmt, ParseError> {
        let span = self.advance().span; // return
        let expr = if matches!(self.peek_kind(), TokenKind::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::Semi, "语句结束符")?;
        Ok(ReturnStmt { expr, span })
    }

    // ---------- 类型解析 ----------

    /// 类型参数闭括号 '>'（A1/E1）：支持复合 token 分裂。
    ///
    /// `table<table<i64>>` 的末尾 `>>` 在词法层被合成 Shr（C++/Rust 同款问题），
    /// 类型参数解析到闭括号时必须把 Shr / Ge / ShrEq 拆开，剩余部分插入 token 流：
    /// - `>>`  (Shr)   → '>' + '>'（剩余 '>' 供外层类型继续消费）；
    /// - `>=`  (Ge)    → '>' + '='（剩余 '=' 供声明符消费，如 `var x: table<i64>=..`）；
    /// - `>>=` (ShrEq) → '>' + '>='（剩余 '>=' 再经一轮 Ge 分裂消费）。
    fn expect_type_gt(&mut self, what: &str) -> Result<(), ParseError> {
        let span = self.peek().span;
        match self.peek_kind() {
            TokenKind::Gt => {
                self.advance();
                Ok(())
            }
            // 复合 token：当前位置保留 '>'（分裂），剩余部分插入下一位置
            TokenKind::Shr => {
                self.tokens[self.pos].kind = TokenKind::Gt;
                self.tokens.insert(self.pos + 1, Token { kind: TokenKind::Gt, span });
                self.advance();
                Ok(())
            }
            TokenKind::Ge => {
                self.tokens[self.pos].kind = TokenKind::Gt;
                self.tokens.insert(self.pos + 1, Token { kind: TokenKind::Eq, span });
                self.advance();
                Ok(())
            }
            TokenKind::ShrEq => {
                self.tokens[self.pos].kind = TokenKind::Gt;
                self.tokens.insert(self.pos + 1, Token { kind: TokenKind::Ge, span });
                self.advance();
                Ok(())
            }
            other => Err(self.err(format!("{}，实际是 {}", what, self.describe(other)))),
        }
    }

    fn parse_type(&mut self) -> Result<TypeSpec, ParseError> {
        match self.peek_kind() {
            TokenKind::TypeKw(ty) => {
                let ty = *ty;
                self.advance();
                // table<T>（A1）：表类型后跟 <元素类型> → 带元素类型的表。
                // 闭括号用 expect_type_gt：支持 `table<table<i64>>` 的 `>>` 分裂。
                if ty == TyKw::Table && self.eat(&TokenKind::Lt) {
                    let elem = self.parse_type()?;
                    self.expect_type_gt("表元素类型后需 '>'")?;
                    return Ok(TypeSpec::Table(Box::new(elem)));
                }
                // map<T>（E3）：键值表类型，键恒为字符串，<值类型> 可选。
                // 裸 map = map<i64>（默认值类型）；`map<string>` 显式标注。
                if ty == TyKw::Map {
                    if self.eat(&TokenKind::Lt) {
                        let val = self.parse_type()?;
                        self.expect_type_gt("键值表值类型后需 '>'")?;
                        return Ok(TypeSpec::Map(Box::new(val)));
                    }
                    return Ok(TypeSpec::Map(Box::new(TypeSpec::Named(TyKw::I64))));
                }
                Ok(TypeSpec::Named(ty))
            }
            // 元组类型：`(i64, string)` / `(x: i64, y: i64)`
            TokenKind::LParen => self.parse_tuple_type(),
            // struct 类型：`MyStruct`（用户自定义数据结构，M2.1.8）
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(TypeSpec::Struct(name))
            }
            other => Err(self.err(format!("期望类型，实际是 {}", self.describe(other)))),
        }
    }

    /// 元组类型：`(T1, T2, ...)` 或 `(name: T1, name: T2, ...)`（字段名可选）。
    ///
    /// 空元组 `()` 不支持（C# 风格：无空元组）。
    fn parse_tuple_type(&mut self) -> Result<TypeSpec, ParseError> {
        let span = self.peek().span;
        self.expect(TokenKind::LParen, "'('")?;
        let mut fields = Vec::new();
        // 空元组 `()`：不支持
        if self.eat(&TokenKind::RParen) {
            return Err(ParseError {
                span,
                message: "空元组类型 () 不支持".into(),
            });
        }
        loop {
            // 命名字段：`x: i64`（标识符后紧跟冒号）
            let name = if matches!(self.peek_kind(), TokenKind::Ident(_))
                && self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Colon)
            {
                let n = self.expect_ident()?;
                self.expect(TokenKind::Colon, "':'")?;
                Some(n)
            } else {
                None
            };
            let ty = self.parse_type()?;
            fields.push(TupleField { name, ty });
            if self.eat(&TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RParen, "')'")?;
            break;
        }
        Ok(TypeSpec::Tuple(fields))
    }

    // ---------- 表达式解析（优先级爬升） ----------

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        // 范围运算符 `..` 优先级最低：先解析三目，再检查范围
        let lhs = self.parse_ternary()?;
        if self.eat(&TokenKind::DotDot) {
            let end = self.parse_expr()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            return Ok(Expr::Range { start: Box::new(lhs), end: Box::new(end), span });
        }
        Ok(lhs)
    }

    /// 三目运算符 `cond ? then : else`（M4，优先级仅高于范围、低于 `||`）。
    ///
    /// 右结合：`?` 后的 then 分支与 `:` 后的 else 分支都用 parse_expr 递归，
    /// 因此 `a ? b ? 1 : 2 : 3` 解析为 `a ? (b ? 1 : 2) : 3`（与 C 一致）。
    fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_or()?;
        if self.eat(&TokenKind::Question) {
            let then_expr = self.parse_expr()?;
            self.expect(TokenKind::Colon, "':'")?;
            let else_expr = self.parse_expr()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            return Ok(Expr::Ternary {
                cond: Box::new(lhs),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
                span,
            });
        }
        Ok(lhs)
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while self.eat(&TokenKind::OrOr) {
            let rhs = self.parse_and()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op: BinaryOp::Or, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_bit_or()?;
        while self.eat(&TokenKind::AndAnd) {
            let rhs = self.parse_bit_or()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op: BinaryOp::And, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    /// 按位或 `|`（M4，优先级：低于 `&&`、高于 `^`）。逐级调用 parse_bit_xor。
    fn parse_bit_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_bit_xor()?;
        while self.eat(&TokenKind::Pipe) {
            let rhs = self.parse_bit_xor()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op: BinaryOp::BitOr, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    /// 按位异或 `^`（M4，优先级：低于 `|`、高于 `&`）。逐级调用 parse_bit_and。
    fn parse_bit_xor(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_bit_and()?;
        while self.eat(&TokenKind::Caret) {
            let rhs = self.parse_bit_and()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op: BinaryOp::BitXor, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    /// 按位与 `&`（M4，优先级：低于 `^`、高于 `== !=`）。逐级调用 parse_equality。
    fn parse_bit_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_equality()?;
        while self.eat(&TokenKind::Amp) {
            let rhs = self.parse_equality()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op: BinaryOp::BitAnd, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_comparison()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::EqEq => BinaryOp::Eq,
                TokenKind::NotEq => BinaryOp::NotEq,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_comparison()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_shift()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Lt => BinaryOp::Lt,
                TokenKind::Gt => BinaryOp::Gt,
                TokenKind::Le => BinaryOp::Le,
                TokenKind::Ge => BinaryOp::Ge,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_shift()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    /// 移位 `<<` / `>>`（M4，优先级：低于比较、高于加减）。逐级调用 parse_term。
    fn parse_shift(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_term()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Shl => BinaryOp::Shl,
                TokenKind::Shr => BinaryOp::Shr,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_term()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_factor()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_factor()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn parse_factor(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                TokenKind::Percent => BinaryOp::Mod,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_unary()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let span = self.peek().span;
        let mut expr = match self.peek_kind() {
            TokenKind::Minus => {
                self.advance();
                let operand = self.parse_unary()?;
                Expr::Unary { op: UnaryOp::Neg, operand: Box::new(operand), span }
            }
            TokenKind::Bang => {
                self.advance();
                let operand = self.parse_unary()?;
                Expr::Unary { op: UnaryOp::Not, operand: Box::new(operand), span }
            }
            // M4 前缀自增 `++x`：先增后取新值
            TokenKind::Inc => {
                self.advance();
                let operand = self.parse_unary()?;
                Expr::Unary { op: UnaryOp::PreInc, operand: Box::new(operand), span }
            }
            // M4 前缀自减 `--x`：先减后取新值
            TokenKind::Dec => {
                self.advance();
                let operand = self.parse_unary()?;
                Expr::Unary { op: UnaryOp::PreDec, operand: Box::new(operand), span }
            }
            _ => self.parse_primary()?,
        };
        // 后缀访问：`base[index]` 下标与 `base.field` 字段访问（可链式，如 `a[0][1]`、`p.x.y`）
        loop {
            if self.eat(&TokenKind::LBracket) {
                let index = self.parse_expr()?;
                let ispan = self.peek().span;
                self.expect(TokenKind::RBracket, "']'")?;
                let base_span = expr_span(&expr).unwrap_or(ispan);
                expr = Expr::Index {
                    base: Box::new(expr),
                    index: Box::new(index),
                    span: base_span,
                };
            } else if self.eat(&TokenKind::Dot) {
                // 字段名/方法名：`.x`（字段访问）或 `.m`（方法调用，后紧跟 `(`）
                let field = match self.peek_kind() {
                    TokenKind::Ident(name) => {
                        let name = name.clone();
                        self.advance();
                        name
                    }
                    TokenKind::Int(v) => {
                        let v = *v;
                        self.advance();
                        v.to_string()
                    }
                    other => {
                        return Err(self.err(format!(
                            "字段访问 '.' 后必须是字段名/方法名/数字下标，实际是 {}",
                            self.describe(other)
                        )))
                    }
                };
                let dspan = self.peek().span;
                let base_span = expr_span(&expr).unwrap_or(dspan);
                // 方法调用：字段名后紧跟 `(`（如 `obj.m(1, 2)`）
                if self.eat(&TokenKind::LParen) {
                    let mut args = Vec::new();
                    if !self.eat(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.eat(&TokenKind::Comma) {
                                continue;
                            }
                            self.expect(TokenKind::RParen, "')'")?;
                            break;
                        }
                    }
                    expr = Expr::MethodCall {
                        receiver: Box::new(expr),
                        method: field,
                        args,
                        span: base_span,
                    };
                } else {
                    // 字段访问：`.x` / `.Item1` / `.0`（元组与类共用，语义层按 base 类型分发）
                    expr = Expr::FieldAccess { base: Box::new(expr), field, span: base_span };
                }
            } else if self.eat(&TokenKind::Inc) {
                // M4 后缀自增 `x++`：先取旧值后增（用操作数自身 span 包裹）
                let pspan = expr_span(&expr).unwrap_or(self.peek().span);
                expr = Expr::Unary { op: UnaryOp::PostInc, operand: Box::new(expr), span: pspan };
            } else if self.eat(&TokenKind::Dec) {
                // M4 后缀自减 `x--`：先取旧值后减
                let pspan = expr_span(&expr).unwrap_or(self.peek().span);
                expr = Expr::Unary { op: UnaryOp::PostDec, operand: Box::new(expr), span: pspan };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// 原子表达式：字面量 / 标识符 / 调用 / 括号 / 范围。
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let tok = self.peek().clone();
        let span = tok.span;
        match tok.kind {
            TokenKind::Int(v) => {
                self.advance();
                Ok(Expr::IntLit(v))
            }
            TokenKind::Float(v) => {
                self.advance();
                Ok(Expr::FloatLit(v))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::StrLit(s))
            }
            TokenKind::CharLit(c) => {
                self.advance();
                Ok(Expr::CharLit(c))
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::BoolLit(true))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::BoolLit(false))
            }
            // 平衡三进制 trit 零值（M4 补齐）：zero → TritLit(0)。
            // 正/负值由语义层把 true/false 适配为 TritLit(±1)（按目标类型）。
            TokenKind::Zero => {
                self.advance();
                Ok(Expr::TritLit(0))
            }
            TokenKind::Ident(name) => {
                self.advance();
                // 命名空间路径：`a::b::c`（`::` 分隔，C#/Rust 风格）。
                // 构建 Expr::Path 后返回，后续 `.m(args)` 由 parse_unary 的 Dot 分支
                // 组装为 MethodCall（receiver = Path，语义层按命名空间函数解析）。
                if *self.peek_kind() == TokenKind::DoubleColon {
                    let mut segments = vec![name];
                    while self.eat(&TokenKind::DoubleColon) {
                        let seg = self.expect_ident()?;
                        segments.push(seg);
                    }
                    return Ok(Expr::Path { segments, span });
                }
                // 函数调用
                if self.eat(&TokenKind::LParen) {
                    let mut args = Vec::new();
                    if !self.eat(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.eat(&TokenKind::Comma) {
                                continue;
                            }
                            self.expect(TokenKind::RParen, "')'")?;
                            break;
                        }
                    }
                    Ok(Expr::Call { name, args, span })
                } else {
                    Ok(Expr::Var(name))
                }
            }
            TokenKind::LParen => self.parse_paren_or_tuple(span),
            TokenKind::LBracket => self.parse_table_lit(span),
            other => Err(ParseError {
                span,
                message: format!("无法以 {} 开始表达式", self.describe(&other)),
            }),
        }
    }

    /// `(` 起始：元组字面量 `(1, "a")` / `(x: 1, y: 2)`，或分组表达式 `(expr)`。
    ///
    /// 判定规则：解析第一个字段后遇逗号 → 元组字面量；否则是分组表达式。
    /// 空元组 `()` 不支持（C# 风格：无空元组）。
    fn parse_paren_or_tuple(&mut self, span: Span) -> Result<Expr, ParseError> {
        self.expect(TokenKind::LParen, "'('")?;
        // 空元组 `()`：不支持
        if self.eat(&TokenKind::RParen) {
            return Err(ParseError {
                span,
                message: "空元组 () 不支持".into(),
            });
        }
        // 第一个字段
        let first = self.parse_tuple_field()?;
        // 遇逗号 → 元组字面量（含后续字段）
        if self.eat(&TokenKind::Comma) {
            let mut fields = Vec::new();
            fields.push(first);
            loop {
                fields.push(self.parse_tuple_field()?);
                if self.eat(&TokenKind::Comma) {
                    continue;
                }
                self.expect(TokenKind::RParen, "')'")?;
                break;
            }
            return Ok(Expr::TupleLit { fields, span });
        }
        // 无逗号 → 分组表达式 `(expr)`。命名字段不允许出现在分组里（`(x: 1)` 是元组）。
        self.expect(TokenKind::RParen, "')'")?;
        if first.0.is_some() {
            return Err(ParseError {
                span,
                message: "命名字段必须作为元组元素（如 (x: 1, y: 2)），单个 (x: 1) 不是合法表达式".into(),
            });
        }
        Ok(first.1)
    }

    /// 元组字段：`name: expr`（命名字段）或 `expr`（匿名）。
    fn parse_tuple_field(&mut self) -> Result<(Option<String>, Expr), ParseError> {
        // 命名字段探测：标识符后紧跟冒号
        if let TokenKind::Ident(name) = self.peek_kind()
            && self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Colon)
        {
            let name = name.clone();
            self.advance();
            self.expect(TokenKind::Colon, "':'")?;
            let value = self.parse_expr()?;
            return Ok((Some(name), value));
        }
        let value = self.parse_expr()?;
        Ok((None, value))
    }

    /// 表字面量 `[col, col; row, row]`。
    ///
    /// 语法要点：
    /// - 逗号 `,` 分隔同一行的元素（列）
    /// - 分号 `;` 分隔行
    /// - 元素可为 `value` 或 `id:value`（id 可选）
    /// - id 可为数字下标（`0:1`）或带引号字符串键（`"a":1`）
    ///
    /// 二维表（分号分行）在语法层**降级（desugar）为嵌套表**：
    /// `[1,2;3,4]` → `[[1,2],[3,4]]`（外层表元素 = 每行一个内层子表）。
    /// 这样下游语义/IR/解释层零改动即可复用 E1 嵌套表路径
    /// （`[[0,1],[0,2]]` 已支持 table<table<i64>> 全链路）。
    fn parse_table_lit(&mut self, span: Span) -> Result<Expr, ParseError> {
        self.expect(TokenKind::LBracket, "'['")?;
        let mut cells = Vec::new();
        let mut row = 0usize;
        // 空表 `[]` 合法
        if self.eat(&TokenKind::RBracket) {
            return Ok(Expr::TableLit { cells, span });
        }
        loop {
            // 解析一个元素：`id:value` 或 `value`
            let cell = self.parse_table_cell(row)?;
            cells.push(cell);
            match self.peek_kind() {
                // 逗号：继续同一行的下一列
                TokenKind::Comma => {
                    self.advance();
                }
                // 分号：换行（行号 +1）
                TokenKind::Semi => {
                    self.advance();
                    row += 1;
                }
                // 右括号：表结束
                TokenKind::RBracket => {
                    self.advance();
                    break;
                }
                other => {
                    return Err(self.err(format!(
                        "期望 , 或 ; 或 ]，实际是 {}",
                        self.describe(other)
                    )))
                }
            }
        }
        self.desugar_2d_table(cells, span)
    }

    /// 二维表 desugar（特性①）：若存在 `row > 0` 的元素 → 按行分组为嵌套表。
    ///
    /// 规则：
    /// - 行长度不必一致（与嵌套表规则相同，由语义层检查元素类型一致性）；
    /// - 多行且存在任何 id 元素（`TableId::Num`/`TableId::Str`）→ 语法错误
    ///   （二维表只支持纯位置元素，id 语义在嵌套表中无意义）；
    /// - 单行（全部 row == 0）→ 原样返回（与既有行为完全一致）。
    ///
    /// desugar 后的子表 cell 的 row 全部清零——下游语义层 1025 行的
    /// 「二维表暂不支持」检查据此不再触发，元素类型检查由嵌套表路径覆盖。
    fn desugar_2d_table(&mut self, cells: Vec<TableCell>, span: Span) -> Result<Expr, ParseError> {
        // 单行表（无分号）→ 保持原样（既有行为，零回归）
        if cells.iter().all(|c| c.row == 0) {
            return Ok(Expr::TableLit { cells, span });
        }
        // 多行且存在 id 元素：二维表不支持 id（数字下标/字符串键）
        if cells.iter().any(|c| c.id.is_some()) {
            return Err(self.err("二维表（分号分行）不支持 id 元素（数字下标/字符串键）".into()));
        }
        // 按 row 分组：cells 按出现顺序排列、row 单调不减，
        // 顺序遍历即可保持「行序 = 源顺序」。
        let mut rows: Vec<Vec<TableCell>> = Vec::new();
        for cell in cells {
            // 行号跳变 → 开新行；连续同号 → 追加到当前行
            if rows.len() <= cell.row {
                rows.push(Vec::new());
            }
            rows[cell.row].push(cell);
        }
        // 每行构造一个子表字面量（row 全部清零），作为外层表的元素
        let nested_cells = rows
            .into_iter()
            .map(|row_cells| {
                // 子表 span：取行内首个元素的表达式 span（更贴近源码位置），
                // 找不到则回退用外层表 span
                let sub_span = row_cells
                    .first()
                    .and_then(|c| expr_span(&c.value))
                    .unwrap_or(span);
                // 行内元素 row 全部置 0（已是 0——该行元素在解析时记录的就是本行号，
                // 但 desugar 后作为单行子表必须从 0 起，语义层只认 row==0 的单行表）
                let sub_cells: Vec<TableCell> = row_cells
                    .into_iter()
                    .map(|mut c| {
                        c.row = 0;
                        c
                    })
                    .collect();
                TableCell {
                    id: None,
                    value: Expr::TableLit { cells: sub_cells, span: sub_span },
                    row: 0,
                }
            })
            .collect();
        Ok(Expr::TableLit { cells: nested_cells, span })
    }

    /// 解析表单元格：`id:value`（id 为数字或带引号字符串）或普通 `value`。
    fn parse_table_cell(&mut self, row: usize) -> Result<TableCell, ParseError> {
        // 先探测是否为 `id:` 形式：数字或字符串字面量后紧跟冒号
        let id = match self.peek_kind() {
            // 数字 id：`0:1`（数字后紧跟冒号）
            TokenKind::Int(_) => {
                let save = self.pos;
                if let TokenKind::Int(v) = self.advance().kind
                    && self.eat(&TokenKind::Colon)
                {
                    Some(TableId::Num(v))
                } else {
                    // 不是 id，回退游标，按普通表达式解析
                    self.pos = save;
                    None
                }
            }
            // 字符串 id：`"a":1`（字符串后紧跟冒号）
            TokenKind::Str(_) => {
                let save = self.pos;
                if let TokenKind::Str(s) = self.advance().kind
                    && self.eat(&TokenKind::Colon)
                {
                    Some(TableId::Str(s))
                } else {
                    // 不是 id，回退游标，按普通表达式解析
                    self.pos = save;
                    None
                }
            }
            _ => None,
        };
        let value = self.parse_expr()?;
        Ok(TableCell { id, value, row })
    }
}

/// 从表达式中提取 span（辅助函数）。
fn expr_span(expr: &Expr) -> Option<Span> {
    match expr {
        Expr::Call { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Ternary { span, .. }
        | Expr::Range { span, .. }
        | Expr::TableLit { span, .. }
        | Expr::Index { span, .. }
        | Expr::TupleLit { span, .. }
        | Expr::FieldAccess { span, .. }
        | Expr::MethodCall { span, .. }
        | Expr::TypeLit { span, .. } => Some(*span),
        _ => None,
    }
}

/// 是否为赋值运算符 token（`=` 或 `+=` 等复合赋值）——快速路径的预探测用（不消费）。
fn is_assign_op_kind(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eq
            | TokenKind::PlusEq
            | TokenKind::MinusEq
            | TokenKind::StarEq
            | TokenKind::SlashEq
            | TokenKind::PercentEq
            | TokenKind::AmpEq
            | TokenKind::PipeEq
            | TokenKind::CaretEq
            | TokenKind::ShlEq
            | TokenKind::ShrEq
    )
}

/// 表达式是否可寻址（可作为字段赋值/字段访问的 base）。
///
/// 可寻址：变量（含 this）或 FieldAccess 链（`obj.a.b` 的 GEP 链天然有地址）；
/// 不可寻址：寄存器中的类值（方法调用结果、构造表达式直接连用）——P8 语义层报错。
fn is_addressable_base(expr: &Expr) -> bool {
    match expr {
        Expr::Var(_) => true,
        Expr::FieldAccess { base, .. } => is_addressable_base(base),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    /// 完整前端管道：词法 → 语法，成功时返回程序 AST。
    fn parse(src: &str) -> Program {
        let tokens = tokenize(src).expect("词法分析应成功");
        parse_program(&tokens).expect("语法分析应成功")
    }

    /// 完整前端管道：词法 → 语法，失败时返回语法错误。
    fn parse_err(src: &str) -> ParseError {
        let tokens = tokenize(src).expect("词法分析应成功");
        parse_program(&tokens).expect_err("语法分析应失败")
    }

    // ---------- M4 补齐：trit 字面量解析 ----------

    /// `zero` → TritLit(0)；true/false 仍为 BoolLit；trit 类型标注可解析。
    #[test]
    fn trit字面量zero解析为TritLit() {
        let prog = parse("func main() {\n    var t = zero\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        assert!(matches!(v.init, Expr::TritLit(0)), "zero 应解析为 TritLit(0)");
    }

    /// `var t: trit = true` 语法层面可解析（字面量适配由语义层完成）。
    #[test]
    fn trit类型标注与true赋值解析() {
        let prog = parse("func main() {\n    var t: trit = true\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        // true 仍是 BoolLit（语义层按目标类型 trit 适配为 +1）
        assert!(matches!(v.init, Expr::BoolLit(true)));
        // 显式类型标注保留 trit
        let ty = v.ty.as_ref().expect("应有类型标注");
        assert_eq!(ty, &TypeSpec::Named(TyKw::Trit));
    }

    // ---------- M4 补齐：表下标赋值解析 ----------

    /// `t[0] = v` / `t[i] += v` / `t[i][j] = v` 解析为 IndexAssign。
    #[test]
    fn 表下标赋值解析为IndexAssign() {
        let prog = parse("func main() {\n    t[0] = 9\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::IndexAssign(ia) = &f.body[0] else { panic!("期望 IndexAssign，实际 {:#?}", f.body[0]) };
        assert!(ia.op.is_none(), "普通赋值 op 为 None");
        assert!(matches!(ia.target.as_ref(), Expr::Index { .. }));

        // 复合赋值
        let prog = parse("func main() {\n    t[i] += 1\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::IndexAssign(ia) = &f.body[0] else { panic!("期望 IndexAssign") };
        assert!(ia.op.is_some(), "复合赋值 op 为 Some");

        // 二维下标
        let prog = parse("func main() {\n    m[i][j] = 1\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::IndexAssign(ia) = &f.body[0] else { panic!("期望 IndexAssign") };
        assert!(matches!(ia.target.as_ref(), Expr::Index { .. }));
    }

    #[test]
    fn 命名空间块式声明解析出路径与体内函数() {
        let prog = parse("namespace tcmsg {\n    func no_file(langs: table) -> string {\n        return \"x\"\n    }\n}\n");
        let Stmt::Namespace(ns) = &prog.stmts[0] else { panic!("期望命名空间声明") };
        assert_eq!(ns.path, vec!["tcmsg"]);
        let Stmt::FnDef(f) = &ns.body[0] else { panic!("期望命名空间体内的函数定义") };
        assert_eq!(f.name, "no_file");
    }

    #[test]
    fn 命名空间点分声明解析出完整路径() {
        let prog = parse("namespace tcmsg.error {\n}\n");
        let Stmt::Namespace(ns) = &prog.stmts[0] else { panic!("期望命名空间声明") };
        assert_eq!(ns.path, vec!["tcmsg", "error"]);
        assert!(ns.body.is_empty());
    }

    // ---------- 单文件命名空间（无花括号包裹）----------

    /// `namespace foo` 独占一行（ASI 在行尾补 `Semi`）后接文件剩余内容：
    /// 剩余全部顶层语句应归入命名空间 foo，直到文件结束。
    #[test]
    fn 单文件命名空间ASI分号把后续顶层语句纳入() {
        let prog = parse("namespace foo\nfunc main() {}\n");
        assert_eq!(prog.stmts.len(), 1, "顶层应只剩一个命名空间声明");
        let Stmt::Namespace(ns) = &prog.stmts[0] else { panic!("期望命名空间声明") };
        assert_eq!(ns.path, vec!["foo"]);
        assert_eq!(ns.body.len(), 1, "foo 应包含 main 函数");
        let Stmt::FnDef(f) = &ns.body[0] else { panic!("期望 foo 内的函数定义") };
        assert_eq!(f.name, "main");
    }

    /// 显式分号 `namespace foo;` 与 ASI 分号完全等价：后续顶层语句全部入 foo。
    #[test]
    fn 单文件命名空间显式分号等价于ASI分号() {
        let prog = parse("namespace foo;\nfunc a() {}\n");
        let Stmt::Namespace(ns) = &prog.stmts[0] else { panic!("期望命名空间声明") };
        assert_eq!(ns.path, vec!["foo"]);
        let Stmt::FnDef(f) = &ns.body[0] else { panic!("期望 foo 内的函数定义") };
        assert_eq!(f.name, "a");
    }

    /// 单文件模式下的嵌套：`namespace a`（单文件）后出现块式 `namespace b { }`
    /// 与函数 `func c() {}`——两者都嵌套在 a 体内；b 是块式嵌套（体为空），
    /// c 是 a 的直接子语句。
    #[test]
    fn 单文件命名空间内嵌套块式命名空间与函数() {
        let prog = parse("namespace a\nnamespace b {\n}\nfunc c() {}\n");
        let Stmt::Namespace(ns_a) = &prog.stmts[0] else { panic!("期望命名空间声明") };
        assert_eq!(ns_a.path, vec!["a"]);
        assert_eq!(ns_a.body.len(), 2, "a 应含嵌套的 b 与函数 c");
        // body[0]：块式嵌套命名空间 b
        let Stmt::Namespace(ns_b) = &ns_a.body[0] else { panic!("期望 a 内的嵌套命名空间 b") };
        assert_eq!(ns_b.path, vec!["b"]);
        assert!(ns_b.body.is_empty(), "b 为块式空体");
        // body[1]：函数 c（a 的直接子语句）
        let Stmt::FnDef(f) = &ns_a.body[1] else { panic!("期望 a 内的函数定义") };
        assert_eq!(f.name, "c");
    }

    /// 单文件模式内的嵌套单文件命名空间：`namespace a\nnamespace b\nfunc c() {}`
    /// ——b 继续把剩余内容（c）归入自己，a 体内只剩嵌套的 b。
    #[test]
    fn 单文件命名空间内嵌套单文件命名空间() {
        let prog = parse("namespace a\nnamespace b\nfunc c() {}\n");
        let Stmt::Namespace(ns_a) = &prog.stmts[0] else { panic!("期望命名空间声明") };
        assert_eq!(ns_a.path, vec!["a"]);
        assert_eq!(ns_a.body.len(), 1, "a 内应只有嵌套的 b");
        let Stmt::Namespace(ns_b) = &ns_a.body[0] else { panic!("期望 a 内的嵌套命名空间 b") };
        assert_eq!(ns_b.path, vec!["b"]);
        let Stmt::FnDef(f) = &ns_b.body[0] else { panic!("期望 b 内的函数定义") };
        assert_eq!(f.name, "c");
    }

    /// 块式命名空间仍按原语义解析（体内函数进 body）。
    #[test]
    fn 块式命名空间保持不变() {
        let prog = parse("namespace tcmsg { func f() {} }\n");
        let Stmt::Namespace(ns) = &prog.stmts[0] else { panic!("期望命名空间声明") };
        assert_eq!(ns.path, vec!["tcmsg"]);
        assert_eq!(ns.body.len(), 1);
        let Stmt::FnDef(f) = &ns.body[0] else { panic!("期望 tcmsg 内的函数定义") };
        assert_eq!(f.name, "f");
    }

    /// 空单文件命名空间：文件在 `namespace foo` 后立即结束 → 空体。
    #[test]
    fn 空单文件命名空间文件末尾() {
        let prog = parse("namespace foo\n");
        let Stmt::Namespace(ns) = &prog.stmts[0] else { panic!("期望命名空间声明") };
        assert_eq!(ns.path, vec!["foo"]);
        assert!(ns.body.is_empty(), "文件结束时空体");
        // 显式分号 + 文件结束同样得到空体
        let prog2 = parse("namespace foo;\n");
        let Stmt::Namespace(ns2) = &prog2.stmts[0] else { panic!("期望命名空间声明") };
        assert!(ns2.body.is_empty(), "显式分号 + 文件结束应为空体");
    }

    #[test]
    fn 命名空间路径表达式解析出segments与调用链() {
        // tcmsg::error.no_file(["zh-cn","en-us"]) → MethodCall { receiver: Path([tcmsg,error]), method: no_file }
        let prog = parse("func main() {\n    var m = tcmsg::error.no_file([\"zh-cn\",\"en-us\"])\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        let Expr::MethodCall { receiver, method, args, .. } = &v.init else {
            panic!("期望命名空间方法调用，实际 {:#?}", v.init)
        };
        assert_eq!(method, "no_file");
        let Expr::Path { segments, .. } = receiver.as_ref() else {
            panic!("期望 receiver 是命名空间路径，实际 {:#?}", receiver)
        };
        assert_eq!(segments, &vec!["tcmsg".to_string(), "error".to_string()]);
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn 命名空间路径缺段时报错() {
        // `tcmsg::` 后必须是标识符
        let err = parse_err("func main() {\n    var x = tcmsg::1\n}\n");
        assert!(err.message.contains("期望标识符"), "错误信息：{}", err.message);
    }

    #[test]
    fn 函数定义解析出函数名参数与返回类型() {
        let prog = parse("func f(a: i64, b: i64) -> i64 {\n    return a + b\n}\nfunc g() {}\n");
        assert_eq!(prog.stmts.len(), 2);
        // 带参数与返回类型
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        assert_eq!(f.name, "f");
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "a");
        assert!(matches!(f.params[0].ty, TypeSpec::Named(TyKw::I64)));
        assert_eq!(f.params[1].name, "b");
        assert!(matches!(f.params[1].ty, TypeSpec::Named(TyKw::I64)));
        assert!(matches!(f.ret_ty, TypeSpec::Named(TyKw::I64)));
        // 函数体 `return a + b`
        let Stmt::Return(r) = &f.body[0] else { panic!("期望 return 语句") };
        let Some(expr) = &r.expr else { panic!("期望返回值") };
        assert!(matches!(
            expr,
            Expr::Binary { op: BinaryOp::Add, lhs, rhs, .. }
                if matches!(lhs.as_ref(), Expr::Var(n) if n == "a")
                    && matches!(rhs.as_ref(), Expr::Var(n) if n == "b")
        ));
        // 返回类型省略 → 默认 void
        let Stmt::FnDef(g) = &prog.stmts[1] else { panic!("期望函数定义") };
        assert_eq!(g.name, "g");
        assert!(g.params.is_empty());
        assert!(matches!(g.ret_ty, TypeSpec::Named(TyKw::Void)));
    }

    #[test]
    fn var声明解析出类型标注与初始值() {
        let prog = parse("func main() {\n    var x: i64 = 42\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        assert_eq!(v.name, "x");
        assert!(matches!(v.ty, Some(TypeSpec::Named(TyKw::I64))));
        assert!(matches!(&v.init, Expr::IntLit(42)));
        assert!(!v.is_const, "var 声明应可变");
    }

    #[test]
    fn const声明解析出不可变标志() {
        let prog = parse("func main() {\n    const c = \"hi\"\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        assert_eq!(v.name, "c");
        assert!(v.ty.is_none(), "无类型标注时应自动推导");
        assert!(matches!(&v.init, Expr::StrLit(s) if s == "hi"));
        assert!(v.is_const, "const 声明应不可变");
    }

    #[test]
    fn 元组解构声明拆为临时变量与字段访问() {
        let prog = parse("func main() {\n    var (q, r) = divmod(10, 3)\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        // desugar：`_tmp0` + `q` + `r` 三条声明
        assert_eq!(f.body.len(), 3);
        // 第一条：临时变量持有元组值
        let Stmt::VarDecl(t) = &f.body[0] else { panic!("期望变量声明") };
        assert_eq!(t.name, "_tmp0");
        assert!(!t.is_const);
        assert!(matches!(
            &t.init,
            Expr::Call { name, args, .. } if name == "divmod" && args.len() == 2
        ));
        // 第二条：`q = _tmp0.Item1`
        let Stmt::VarDecl(q) = &f.body[1] else { panic!("期望变量声明") };
        assert_eq!(q.name, "q");
        assert!(matches!(
            &q.init,
            Expr::FieldAccess { base, field, .. }
                if matches!(base.as_ref(), Expr::Var(n) if n == "_tmp0") && field == "Item1"
        ));
        // 第三条：`r = _tmp0.Item2`
        let Stmt::VarDecl(r) = &f.body[2] else { panic!("期望变量声明") };
        assert_eq!(r.name, "r");
        assert!(matches!(
            &r.init,
            Expr::FieldAccess { base, field, .. }
                if matches!(base.as_ref(), Expr::Var(n) if n == "_tmp0") && field == "Item2"
        ));
    }

    #[test]
    fn 变量赋值解析为assign语句() {
        let prog = parse("func main() {\n    x = x + 1\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::Assign(a) = &f.body[0] else { panic!("期望赋值语句") };
        assert_eq!(a.target, "x");
        assert!(matches!(
            &a.value,
            Expr::Binary { op: BinaryOp::Add, lhs, rhs, .. }
                if matches!(lhs.as_ref(), Expr::Var(n) if n == "x")
                    && matches!(rhs.as_ref(), Expr::IntLit(1))
        ));
    }

    #[test]
    fn 对象字段赋值解析为字段赋值语句() {
        let prog = parse("func main() {\n    obj.field = v\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::FieldAssign(fa) = &f.body[0] else { panic!("期望字段赋值语句") };
        assert!(matches!(fa.base.as_ref(), Expr::Var(n) if n == "obj"));
        assert_eq!(fa.field, "field");
        assert!(matches!(&fa.value, Expr::Var(n) if n == "v"));
    }

    #[test]
    fn 字段链赋值保留链式base() {
        let prog = parse("func main() {\n    obj.a.b = v\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::FieldAssign(fa) = &f.body[0] else { panic!("期望字段赋值语句") };
        // base 应为 `obj.a`，field 为 `b`
        assert!(matches!(
            fa.base.as_ref(),
            Expr::FieldAccess { base, field, .. }
                if matches!(base.as_ref(), Expr::Var(n) if n == "obj") && field == "a"
        ));
        assert_eq!(fa.field, "b");
    }

    #[test]
    fn if语句解析出条件与两分支() {
        let prog = parse("func main() {\n    if x > 0 {\n        y = 1\n    } else {\n        y = 2\n    }\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::If(s) = &f.body[0] else { panic!("期望 if 语句") };
        assert!(matches!(&s.cond, Expr::Binary { op: BinaryOp::Gt, lhs, rhs, .. }
            if matches!(lhs.as_ref(), Expr::Var(n) if n == "x")
                && matches!(rhs.as_ref(), Expr::IntLit(0))));
        // then 分支含 `y = 1`
        assert_eq!(s.then_branch.len(), 1);
        assert!(matches!(&s.then_branch[0], Stmt::Assign(a) if a.target == "y"));
        // else 分支含 `y = 2`
        assert_eq!(s.else_branch.len(), 1);
        assert!(matches!(&s.else_branch[0], Stmt::Assign(a) if a.target == "y"));
    }

    #[test]
    fn else_if链折叠为嵌套if() {
        let prog = parse(
            "func main() {\n    if a {\n        y = 1\n    } else if b {\n        y = 2\n    } else {\n        y = 3\n    }\n}",
        );
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::If(s) = &f.body[0] else { panic!("期望 if 语句") };
        assert!(matches!(&s.cond, Expr::Var(n) if n == "a"));
        // else 分支应折叠为嵌套 IfStmt（而非多条语句）
        assert_eq!(s.else_branch.len(), 1);
        let Stmt::If(inner) = &s.else_branch[0] else { panic!("期望嵌套 if") };
        assert!(matches!(&inner.cond, Expr::Var(n) if n == "b"));
        assert!(matches!(&inner.then_branch[0], Stmt::Assign(a) if a.target == "y"));
        // 最内层 else 分支含 `y = 3`
        assert!(matches!(&inner.else_branch[0], Stmt::Assign(a) if a.target == "y"));
    }

    #[test]
    fn while循环解析出条件与循环体() {
        let prog = parse("func main() {\n    while i < 10 {\n        i = i + 1\n    }\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::While(w) = &f.body[0] else { panic!("期望 while 语句") };
        assert!(matches!(&w.cond, Expr::Binary { op: BinaryOp::Lt, .. }));
        assert_eq!(w.body.len(), 1);
        assert!(matches!(&w.body[0], Stmt::Assign(a) if a.target == "i"));
    }

    #[test]
    fn for循环解析出范围与表两种迭代式() {
        let prog = parse(
            "func main() {\n    for i in 0..10 {\n        sum = sum + i\n    }\n    for item in arr {\n        total = total + item\n    }\n}",
        );
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        // 范围迭代：iter 为 Range 表达式
        let Stmt::For(rng) = &f.body[0] else { panic!("期望 for 语句") };
        assert_eq!(rng.var, "i");
        assert!(matches!(
            &rng.iter,
            Expr::Range { start, end, .. }
                if matches!(start.as_ref(), Expr::IntLit(0)) && matches!(end.as_ref(), Expr::IntLit(10))
        ));
        assert_eq!(rng.body.len(), 1);
        // 表遍历：iter 为变量引用
        let Stmt::For(tbl) = &f.body[1] else { panic!("期望 for 语句") };
        assert_eq!(tbl.var, "item");
        assert!(matches!(&tbl.iter, Expr::Var(n) if n == "arr"));
    }

    #[test]
    fn switch解析出多分支与default() {
        let prog = parse(
            "func main() {\n    switch x {\n        case 1:\n            y = 10\n        case -1:\n            y = -10\n        default:\n            y = 0\n    }\n}",
        );
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::Switch(s) = &f.body[0] else { panic!("期望 switch 语句") };
        assert!(matches!(&s.subject, Expr::Var(n) if n == "x"));
        assert_eq!(s.cases.len(), 2);
        // case 1 分支
        assert_eq!(s.cases[0].patterns.len(), 1);
        assert!(matches!(&s.cases[0].patterns[0], Expr::IntLit(1)));
        assert!(matches!(&s.cases[0].body[0], Stmt::Assign(a) if a.target == "y"));
        // case -1 分支（负数由一元负号包裹）
        assert!(matches!(
            &s.cases[1].patterns[0],
            Expr::Unary { op: UnaryOp::Neg, operand, .. }
                if matches!(operand.as_ref(), Expr::IntLit(1))
        ));
        // default 分支
        assert_eq!(s.default_body.len(), 1);
        assert!(matches!(&s.default_body[0], Stmt::Assign(a) if a.target == "y"));
    }

    #[test]
    fn switch多值区间守卫类型匹配解析() {
        // 模式匹配增强：多值 `case 1, 2:` / 区间 `case 3..7:` / 守卫 `case 8 when flag:`
        // / 类型匹配 `case string:`——逐一验证 AST 形态
        let prog = parse(
            "func main() {\n    switch x {\n        case 1, 2:\n            y = 10\n        case 3..7:\n            y = 20\n        case 8 when flag:\n            y = 30\n        case string:\n            y = 40\n    }\n}",
        );
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::Switch(s) = &f.body[0] else { panic!("期望 switch 语句") };
        assert_eq!(s.cases.len(), 4);
        // case 1, 2：多值（2 个 pattern）
        assert_eq!(s.cases[0].patterns.len(), 2);
        assert!(matches!(&s.cases[0].patterns[0], Expr::IntLit(1)));
        assert!(matches!(&s.cases[0].patterns[1], Expr::IntLit(2)));
        assert!(s.cases[0].when.is_none());
        // case 3..7：区间（Range 表达式）
        assert_eq!(s.cases[1].patterns.len(), 1);
        assert!(matches!(
            &s.cases[1].patterns[0],
            Expr::Range { start, end, .. }
                if matches!(start.as_ref(), Expr::IntLit(3))
                    && matches!(end.as_ref(), Expr::IntLit(7))
        ));
        assert!(s.cases[1].when.is_none());
        // case 8 when flag：守卫
        assert_eq!(s.cases[2].patterns.len(), 1);
        assert!(matches!(&s.cases[2].patterns[0], Expr::IntLit(8)));
        assert!(matches!(&s.cases[2].when, Some(Expr::Var(n)) if n == "flag"));
        // case string：类型匹配（TypeLit）
        assert_eq!(s.cases[3].patterns.len(), 1);
        assert!(matches!(
            &s.cases[3].patterns[0],
            Expr::TypeLit { ty: TypeSpec::Named(TyKw::Str), .. }
        ));
        assert!(s.cases[3].when.is_none());
    }

    #[test]
    fn import无别名解析() {
        let prog = parse("import \"./lib.tie\"\nfunc main() {}\n");
        let Stmt::Import(imp) = &prog.stmts[0] else { panic!("期望 import 语句") };
        assert_eq!(imp.path, "./lib.tie");
        assert!(imp.alias.is_none());
    }

    #[test]
    fn import带别名解析() {
        let prog = parse("import \"./lib.tie\" as lib\nfunc main() {}\n");
        let Stmt::Import(imp) = &prog.stmts[0] else { panic!("期望 import 语句") };
        assert_eq!(imp.path, "./lib.tie");
        assert_eq!(imp.alias.as_deref(), Some("lib"));
    }

    #[test]
    fn struct定义解析出字段与继承() {
        let prog = parse(
            "struct Point extends Base {\n    var x: i64 = 0\n    var y: i64\n}",
        );
        let Stmt::Struct(c) = &prog.stmts[0] else { panic!("期望 struct 定义") };
        assert_eq!(c.name, "Point");
        assert_eq!(c.parent.as_deref(), Some("Base"));
        // 字段：`x`（带默认值）与 `y`（无默认值）
        assert_eq!(c.fields.len(), 2);
        assert_eq!(c.fields[0].name, "x");
        assert!(matches!(c.fields[0].ty, Some(TypeSpec::Named(TyKw::I64))));
        assert!(matches!(&c.fields[0].init, Some(Expr::IntLit(0))));
        assert_eq!(c.fields[1].name, "y");
        assert!(c.fields[1].init.is_none());
    }

    #[test]
    fn struct体内方法语法报错() {
        // M2.1.8：struct 纯数据，方法请用命名空间函数定义 → parser 报错提示
        let err = parse_err(
            "struct Point {\n    var x: i64\n    func move(dx: i64) {\n        x = dx\n    }\n}",
        );
        assert!(err.message.contains("命名空间函数"), "错误应提示新写法：{err}");
    }

    #[test]
    fn 表字面量位置元素解析() {
        let prog = parse("func main() {\n    var t = [1, 2, 3]\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        let Expr::TableLit { cells, .. } = &v.init else { panic!("期望表字面量") };
        assert_eq!(cells.len(), 3);
        for cell in cells {
            assert!(cell.id.is_none(), "位置元素应无显式 id");
            assert_eq!(cell.row, 0, "单行表元素行号应为 0");
        }
        assert!(matches!(&cells[0].value, Expr::IntLit(1)));
        assert!(matches!(&cells[2].value, Expr::IntLit(3)));
    }

    #[test]
    fn 表字面量字符串键值解析() {
        let prog = parse("func main() {\n    var t = [\"a\": 1, 0: 2]\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        let Expr::TableLit { cells, .. } = &v.init else { panic!("期望表字面量") };
        assert_eq!(cells.len(), 2);
        // 字符串键 `"a": 1`
        assert!(matches!(&cells[0].id, Some(TableId::Str(s)) if s == "a"));
        assert!(matches!(&cells[0].value, Expr::IntLit(1)));
        // 数字下标 `0: 2`
        assert!(matches!(&cells[1].id, Some(TableId::Num(0))));
        assert!(matches!(&cells[1].value, Expr::IntLit(2)));
    }

    #[test]
    fn 二元运算优先级乘法高于加法且括号可改序() {
        let prog = parse(
            "func f() -> i64 {\n    return 1 + 2 * 3\n}\nfunc g() -> i64 {\n    return (1 + 2) * 3\n}",
        );
        // `1 + 2 * 3`：乘法先结合，嵌套在加法右侧
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::Return(r) = &f.body[0] else { panic!("期望 return 语句") };
        let Some(expr) = &r.expr else { panic!("期望返回值") };
        assert!(matches!(
            expr,
            Expr::Binary { op: BinaryOp::Add, lhs, rhs, .. }
                if matches!(lhs.as_ref(), Expr::IntLit(1))
                    && matches!(
                        rhs.as_ref(),
                        Expr::Binary { op: BinaryOp::Mul, .. }
                    )
        ));
        // `(1 + 2) * 3`：括号提升加法，乘法在最外层
        let Stmt::FnDef(g) = &prog.stmts[1] else { panic!("期望函数定义") };
        let Stmt::Return(r) = &g.body[0] else { panic!("期望 return 语句") };
        let Some(expr) = &r.expr else { panic!("期望返回值") };
        assert!(matches!(
            expr,
            Expr::Binary { op: BinaryOp::Mul, lhs, rhs, .. }
                if matches!(
                    lhs.as_ref(),
                    Expr::Binary { op: BinaryOp::Add, .. }
                ) && matches!(rhs.as_ref(), Expr::IntLit(3))
        ));
    }

    #[test]
    fn 一元负号与逻辑非解析() {
        let prog = parse(
            "func f() -> i64 {\n    return -x\n}\nfunc g() -> bool {\n    return !flag\n}",
        );
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::Return(r) = &f.body[0] else { panic!("期望 return 语句") };
        assert!(matches!(
            &r.expr,
            Some(Expr::Unary { op: UnaryOp::Neg, operand, .. })
                if matches!(operand.as_ref(), Expr::Var(n) if n == "x")
        ));
        let Stmt::FnDef(g) = &prog.stmts[1] else { panic!("期望函数定义") };
        let Stmt::Return(r) = &g.body[0] else { panic!("期望 return 语句") };
        assert!(matches!(
            &r.expr,
            Some(Expr::Unary { op: UnaryOp::Not, operand, .. })
                if matches!(operand.as_ref(), Expr::Var(n) if n == "flag")
        ));
    }

    #[test]
    fn 调用成员访问与下标链式解析() {
        let prog = parse(
            "func main() {\n    println(\"hi\")\n    var t = arr[0]\n    var u = p.x\n    var v = obj.m(1, 2)\n    var w = a[0].b\n}",
        );
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        // 函数调用语句
        let Stmt::Expr(e) = &f.body[0] else { panic!("期望表达式语句") };
        assert!(matches!(&e.expr, Expr::Call { name, args, .. }
            if name == "println" && args.len() == 1));
        // 下标访问 `arr[0]`
        let Stmt::VarDecl(t) = &f.body[1] else { panic!("期望变量声明") };
        assert!(matches!(&t.init, Expr::Index { base, index, .. }
            if matches!(base.as_ref(), Expr::Var(n) if n == "arr")
                && matches!(index.as_ref(), Expr::IntLit(0))));
        // 字段访问 `p.x`
        let Stmt::VarDecl(u) = &f.body[2] else { panic!("期望变量声明") };
        assert!(matches!(&u.init, Expr::FieldAccess { base, field, .. }
            if matches!(base.as_ref(), Expr::Var(n) if n == "p") && field == "x"));
        // 方法调用 `obj.m(1, 2)`
        let Stmt::VarDecl(v) = &f.body[3] else { panic!("期望变量声明") };
        assert!(matches!(&v.init, Expr::MethodCall { receiver, method, args, .. }
            if matches!(receiver.as_ref(), Expr::Var(n) if n == "obj")
                && method == "m" && args.len() == 2));
        // 链式 `a[0].b`：字段访问套下标
        let Stmt::VarDecl(w) = &f.body[4] else { panic!("期望变量声明") };
        assert!(matches!(&w.init, Expr::FieldAccess { base, field, .. }
            if matches!(base.as_ref(), Expr::Index { .. }) && field == "b"));
    }

    #[test]
    fn 元组字面量与分组表达式解析() {
        let prog = parse(
            "func main() {\n    var t = (1, \"a\")\n    var u = (x: 1, y: 2)\n    var v = (1 + 2) * 3\n}",
        );
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        // 匿名元组 `(1, "a")`
        let Stmt::VarDecl(t) = &f.body[0] else { panic!("期望变量声明") };
        assert!(matches!(&t.init, Expr::TupleLit { fields, .. } if fields.len() == 2));
        // 命名元组 `(x: 1, y: 2)`
        let Stmt::VarDecl(u) = &f.body[1] else { panic!("期望变量声明") };
        let Expr::TupleLit { fields, .. } = &u.init else { panic!("期望元组字面量") };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].0.as_deref(), Some("x"));
        assert!(matches!(&fields[0].1, Expr::IntLit(1)));
        // 分组表达式 `(1 + 2) * 3` 不是元组
        let Stmt::VarDecl(v) = &f.body[2] else { panic!("期望变量声明") };
        assert!(matches!(
            &v.init,
            Expr::Binary { op: BinaryOp::Mul, lhs, .. }
                if matches!(lhs.as_ref(), Expr::Binary { op: BinaryOp::Add, .. })
        ));
    }

    #[test]
    fn 顶层裸语句报错() {
        let err = parse_err("x = 1\n");
        assert!(
            err.message.contains("顶层只允许"),
            "错误信息应提示顶层限制，实际：{}",
            err.message
        );
    }

    #[test]
    fn import缺路径或缺别名报错() {
        // import 后缺字符串路径
        let err1 = parse_err("import 123\nfunc main() {}\n");
        assert!(
            err1.message.contains("import 后必须是字符串路径"),
            "实际：{}",
            err1.message
        );
        // `as` 后缺别名
        let err2 = parse_err("import \"./lib.tie\" as\n");
        assert!(
            err2.message.contains("as 后必须是别名标识符"),
            "实际：{}",
            err2.message
        );
    }

    #[test]
    fn 函数体内裸代码块报错() {
        let err = parse_err("func main() {\n    {\n        x = 1\n    }\n}\n");
        assert!(
            err.message.contains("函数体内不能有裸代码块"),
            "实际：{}",
            err.message
        );
    }

    // ---------- M4 运算符扩展 ----------

    #[test]
    fn 位运算优先级按c标准排列() {
        // `1 | 2 ^ 3 & 4`：优先级从低到高为 `|` < `^` < `&`
        // → 顶层 `|`，右操作数是 `^`，`^` 的右操作数是 `&`
        let prog = parse("func f() -> i64 {\n    return 1 | 2 ^ 3 & 4\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::Return(r) = &f.body[0] else { panic!("期望 return 语句") };
        let Some(expr) = &r.expr else { panic!("期望返回值") };
        assert!(matches!(
            expr,
            Expr::Binary { op: BinaryOp::BitOr, lhs, rhs, .. }
                if matches!(lhs.as_ref(), Expr::IntLit(1))
                    && matches!(
                        rhs.as_ref(),
                        Expr::Binary { op: BinaryOp::BitXor, lhs, rhs, .. }
                            if matches!(lhs.as_ref(), Expr::IntLit(2))
                                && matches!(
                                    rhs.as_ref(),
                                    Expr::Binary { op: BinaryOp::BitAnd, lhs, rhs, .. }
                                        if matches!(lhs.as_ref(), Expr::IntLit(3))
                                            && matches!(rhs.as_ref(), Expr::IntLit(4))
                                )
                    )
        ));
    }

    #[test]
    fn 移位运算解析为shl与shr() {
        let prog = parse("func f() -> i64 {\n    return 8 >> 2\n}\nfunc g() -> i64 {\n    return 1 << 3\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::Return(r) = &f.body[0] else { panic!("期望 return 语句") };
        assert!(matches!(
            &r.expr,
            Some(Expr::Binary { op: BinaryOp::Shr, lhs, rhs, .. })
                if matches!(lhs.as_ref(), Expr::IntLit(8)) && matches!(rhs.as_ref(), Expr::IntLit(2))
        ));
        let Stmt::FnDef(g) = &prog.stmts[1] else { panic!("期望函数定义") };
        let Stmt::Return(r) = &g.body[0] else { panic!("期望 return 语句") };
        assert!(matches!(
            &r.expr,
            Some(Expr::Binary { op: BinaryOp::Shl, lhs, rhs, .. })
                if matches!(lhs.as_ref(), Expr::IntLit(1)) && matches!(rhs.as_ref(), Expr::IntLit(3))
        ));
    }

    #[test]
    fn 三目运算符解析出条件与两分支() {
        // `a > 0 ? 1 : -1`：cond 是 `a > 0`，then 是 `1`，else 是 `-1`（一元负号包裹）
        let prog = parse("func f() -> i64 {\n    return a > 0 ? 1 : -1\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::Return(r) = &f.body[0] else { panic!("期望 return 语句") };
        let Some(Expr::Ternary { cond, then_expr, else_expr, .. }) = &r.expr else {
            panic!("期望三目表达式")
        };
        assert!(matches!(
            cond.as_ref(),
            Expr::Binary { op: BinaryOp::Gt, lhs, rhs, .. }
                if matches!(lhs.as_ref(), Expr::Var(n) if n == "a")
                    && matches!(rhs.as_ref(), Expr::IntLit(0))
        ));
        assert!(matches!(then_expr.as_ref(), Expr::IntLit(1)));
        assert!(matches!(
            else_expr.as_ref(),
            Expr::Unary { op: UnaryOp::Neg, operand, .. }
                if matches!(operand.as_ref(), Expr::IntLit(1))
        ));
    }

    #[test]
    fn 嵌套三目右结合解析() {
        // `a ? b ? 1 : 2 : 3`：外层 then 分支是内层三目（右结合）
        let prog = parse("func f() -> i64 {\n    return a ? b ? 1 : 2 : 3\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::Return(r) = &f.body[0] else { panic!("期望 return 语句") };
        let Some(Expr::Ternary { cond, then_expr, else_expr, .. }) = &r.expr else {
            panic!("期望三目表达式")
        };
        assert!(matches!(cond.as_ref(), Expr::Var(n) if n == "a"));
        // then 分支是嵌套三目 `b ? 1 : 2`
        assert!(matches!(
            then_expr.as_ref(),
            Expr::Ternary { cond, then_expr, else_expr, .. }
                if matches!(cond.as_ref(), Expr::Var(n) if n == "b")
                    && matches!(then_expr.as_ref(), Expr::IntLit(1))
                    && matches!(else_expr.as_ref(), Expr::IntLit(2))
        ));
        // else 分支 `3`
        assert!(matches!(else_expr.as_ref(), Expr::IntLit(3)));
    }

    #[test]
    fn 复合赋值解析出运算符与普通赋值op为none() {
        let prog = parse("func main() {\n    x += 1\n    x = 1\n    obj.f -= 2\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        // `x += 1`：复合加等 op=Some(Add)
        let Stmt::Assign(a) = &f.body[0] else { panic!("期望赋值语句") };
        assert_eq!(a.target, "x");
        assert_eq!(a.op, Some(BinaryOp::Add));
        // `x = 1`：普通赋值 op=None
        let Stmt::Assign(a) = &f.body[1] else { panic!("期望赋值语句") };
        assert_eq!(a.target, "x");
        assert_eq!(a.op, None);
        // `obj.f -= 2`：字段复合减等 op=Some(Sub)
        let Stmt::FieldAssign(fa) = &f.body[2] else { panic!("期望字段赋值语句") };
        assert!(matches!(fa.base.as_ref(), Expr::Var(n) if n == "obj"));
        assert_eq!(fa.field, "f");
        assert_eq!(fa.op, Some(BinaryOp::Sub));
    }

    #[test]
    fn 自增自减前缀与后缀解析() {
        let prog = parse("func main() {\n    ++x\n    x++\n    --y\n    y--\n}");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        // `++x` → PreInc
        let Stmt::Expr(e) = &f.body[0] else { panic!("期望表达式语句") };
        assert!(matches!(
            &e.expr,
            Expr::Unary { op: UnaryOp::PreInc, operand, .. }
                if matches!(operand.as_ref(), Expr::Var(n) if n == "x")
        ));
        // `x++` → PostInc
        let Stmt::Expr(e) = &f.body[1] else { panic!("期望表达式语句") };
        assert!(matches!(
            &e.expr,
            Expr::Unary { op: UnaryOp::PostInc, operand, .. }
                if matches!(operand.as_ref(), Expr::Var(n) if n == "x")
        ));
        // `--y` → PreDec
        let Stmt::Expr(e) = &f.body[2] else { panic!("期望表达式语句") };
        assert!(matches!(
            &e.expr,
            Expr::Unary { op: UnaryOp::PreDec, operand, .. }
                if matches!(operand.as_ref(), Expr::Var(n) if n == "y")
        ));
        // `y--` → PostDec
        let Stmt::Expr(e) = &f.body[3] else { panic!("期望表达式语句") };
        assert!(matches!(
            &e.expr,
            Expr::Unary { op: UnaryOp::PostDec, operand, .. }
                if matches!(operand.as_ref(), Expr::Var(n) if n == "y")
        ));
    }

    // ---------- E1+E5：break/continue + 标签跳转解析 ----------

    #[test]
    fn break_continue解析为循环跳转语句() {
        let prog = parse("func main() {\n    while true {\n        break\n        continue\n    }\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::While(w) = &f.body[0] else { panic!("期望 while 语句") };
        assert!(matches!(w.body[0], Stmt::Break(_)), "break 应解析为 Stmt::Break");
        assert!(matches!(w.body[1], Stmt::Continue(_)), "continue 应解析为 Stmt::Continue");
    }

    #[test]
    fn break_continue带标签解析() {
        let prog = parse("func main() {\n    outer: while true {\n        break outer\n        continue outer\n    }\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::While(w) = &f.body[0] else { panic!("期望 while 语句") };
        assert_eq!(w.label.as_deref(), Some("outer"), "循环标签应解析为 outer");
        let Stmt::Break(b) = &w.body[0] else { panic!("期望 break 语句") };
        assert_eq!(b.label.as_deref(), Some("outer"), "break 标签应解析为 outer");
        let Stmt::Continue(c) = &w.body[1] else { panic!("期望 continue 语句") };
        assert_eq!(c.label.as_deref(), Some("outer"), "continue 标签应解析为 outer");
    }

    #[test]
    fn 标签后非循环报错() {
        // 只有「标识符 : while/for」才被识别为循环标签；其他场景走表达式解析并报错
        let err = parse_err("func main() {\n    foo: bar = 1\n}\n");
        assert!(
            err.message.contains("无法") || err.message.contains("期望"),
            "错误：{}",
            err.message
        );
    }

    // ---------- T0.7：extern 函数声明 ----------

    /// extern 声明（含返回类型、多参数、无参数、缺省 void）应解析为 Stmt::Extern。
    #[test]
    fn extern顶层声明解析() {
        let prog = parse("extern fn foo(a: i64, b: string) -> bool;\nfunc main() {}\n");
        let Stmt::Extern(e) = &prog.stmts[0] else { panic!("期望 extern 声明") };
        assert_eq!(e.name, "foo");
        assert_eq!(e.params.len(), 2, "应解析出 2 个参数");
        assert_eq!(e.params[0].ty, TypeSpec::Named(TyKw::I64));
        assert_eq!(e.params[1].ty, TypeSpec::Named(TyKw::Str));
        assert_eq!(e.ret_ty, TypeSpec::Named(TyKw::Bool));
        // 无参数 + 缺省返回类型（void）
        let prog2 = parse("extern fn bar();\nfunc main() {}\n");
        let Stmt::Extern(e2) = &prog2.stmts[0] else { panic!("期望 extern 声明") };
        assert!(e2.params.is_empty(), "无参数 extern");
        assert_eq!(e2.ret_ty, TypeSpec::Named(TyKw::Void), "缺省返回类型应为 void");
    }

    /// extern 出现在函数体内 → 语法错误（extern 仅顶层合法）。
    #[test]
    fn extern函数体内报错() {
        let err = parse_err("func main() {\n    extern fn foo() -> i64;\n}\n");
        assert!(
            err.message.contains("只能出现在文件顶层"),
            "错误消息：{}",
            err.message
        );
    }

    /// extern 声明遵循 ASI 规则：换行后自动补分号（与普通语句一致）。
    #[test]
    fn extern声明ASI分号插入() {
        // 换行分隔 → ASI 补分号，extern 声明解析成功
        let prog = parse("extern fn foo() -> i64\nfunc main() {}\n");
        assert!(matches!(&prog.stmts[0], Stmt::Extern(_)), "ASI 后 extern 应成功");
        // 同一行紧接下一语句且无分号 → 语法错误
        let err = parse_err("extern fn foo() -> i64 func main() {}\n");
        assert!(
            err.message.contains("分号"),
            "错误消息：{}",
            err.message
        );
    }

    // ---------- 特性①：二维表 desugar ----------

    /// `[1,2;3,4]` 解析后 desugar 为嵌套表：外层 cells = 两个内层 TableLit。
    #[test]
    fn 二维表分号分行desugar为嵌套表() {
        let prog = parse("func main() {\n    var t = [1, 2; 3, 4]\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        // 外层必须是表字面量，其每个 cell 的 value 都是内层表字面量
        let Expr::TableLit { cells, .. } = &v.init else {
            panic!("二维表应 desugar 为外层表字面量，实际 {:#?}", v.init)
        };
        assert_eq!(cells.len(), 2, "外层应有 2 行");
        for c in cells {
            assert!(c.id.is_none(), "desugar 后外层元素不应带 id");
            assert_eq!(c.row, 0, "desugar 后外层元素 row 应为 0");
            let Expr::TableLit { cells: sub, .. } = &c.value else {
                panic!("外层元素应为内层表字面量，实际 {:#?}", c.value)
            };
            assert_eq!(sub.len(), 2, "每行应有 2 列");
            assert!(sub.iter().all(|sc| sc.row == 0), "内层元素 row 应为 0");
        }
    }

    /// 二维表含 id 元素（`[1,2; 3:4]`）→ 语法错误。
    #[test]
    fn 二维表含id元素报错() {
        let err = parse_err("func main() {\n    var t = [1, 2; 3: 4]\n}\n");
        assert!(
            err.message.contains("二维表"),
            "错误消息：{}",
            err.message
        );
    }

    /// 行长度不一致合法：`[1; 2,3]` → 行 0 一列、行 1 两列。
    #[test]
    fn 二维表行长度不一致合法() {
        let prog = parse("func main() {\n    var t = [1; 2, 3]\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        let Expr::TableLit { cells, .. } = &v.init else {
            panic!("二维表应 desugar 为外层表字面量，实际 {:#?}", v.init)
        };
        assert_eq!(cells.len(), 2, "外层应有 2 行");
        // 行 0：1 列；行 1：2 列（desugar 前 row 已不可见，按外层 cells 顺序验证）
        let Expr::TableLit { cells: r0, .. } = &cells[0].value else { panic!("行 0 应为表") };
        let Expr::TableLit { cells: r1, .. } = &cells[1].value else { panic!("行 1 应为表") };
        assert_eq!(r0.len(), 1, "行 0 应为 1 列");
        assert_eq!(r1.len(), 2, "行 1 应为 2 列");
    }

    /// 单行表（无分号）保持原样：非嵌套表、元素 row 全 0。
    #[test]
    fn 单行表不desugar() {
        let prog = parse("func main() {\n    var t = [1, 2, 3]\n}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        let Stmt::VarDecl(v) = &f.body[0] else { panic!("期望变量声明") };
        let Expr::TableLit { cells, .. } = &v.init else { panic!("期望表字面量") };
        assert_eq!(cells.len(), 3);
        // 单行表元素保持原始值表达式（数字字面量，而非内层表）
        assert!(cells.iter().all(|c| matches!(c.value, Expr::IntLit(_))));
        assert!(cells.iter().all(|c| c.row == 0));
    }

    // ---------- 特性④：变参函数参数解析 ----------

    /// `func f(rest: ...i64)` 解析出 variadic 标记；`...` 与默认值互斥。
    #[test]
    fn 变参参数解析与默认值互斥() {
        let prog = parse("func f(rest: ...i64) {}\nfunc main() {}\n");
        let Stmt::FnDef(f) = &prog.stmts[0] else { panic!("期望函数定义") };
        assert_eq!(f.params.len(), 1);
        assert!(f.params[0].variadic, "变参标记应解析为 variadic=true");

        // 变参 + 默认值 → 语法错误
        let err = parse_err("func f(rest: ...i64 = 1) {}\nfunc main() {}\n");
        assert!(err.message.contains("默认值"), "错误消息：{}", err.message);

        // 变参后仍有参数 → 语法错误（变参必须最后一个）
        let err = parse_err("func f(a: ...i64, b: i64) {}\nfunc main() {}\n");
        assert!(err.message.contains("最后一个参数"), "错误消息：{}", err.message);

        // 变参 + ref → 语法错误（`ref` 修饰与 `...` 互斥）
        let err = parse_err("func f(a: ref ...table<i64>) {}\nfunc main() {}\n");
        assert!(err.message.contains("ref"), "错误消息：{}", err.message);
    }
}
