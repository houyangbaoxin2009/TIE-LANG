//! 语法分析器（Parser）。
//!
//! 职责：递归下降解析 token 流生成 AST。
//!
//! 说明：文件头部（`// tie:` 指令）已由 tie-prep 预处理阶段提取，
//! 本解析器只处理清理后的正文源码。

use super::ast::{
    AssignStmt, BinaryOp, ClassDefStmt, ClassField, Expr, ExprStmt, FieldAssignStmt, FnDefStmt,
    ForStmt, IfStmt, ImportStmt, MethodDefStmt, NamespaceStmt, Param, Program, ReturnStmt, Stmt,
    SwitchCase, SwitchStmt, TableCell, TableId, TupleField, TypeSpec, UnaryOp, UsingStmt,
    VarDeclStmt, WhileStmt,
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
struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// 解构 desugar 用的临时变量计数器（生成 `_tmpN` 唯一名）
    tmp_counter: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0, tmp_counter: 0 }
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
        // 顶层只允许函数定义、import、using、类定义与命名空间声明（import 由 driver 递归展开为函数）
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            match self.peek_kind() {
                TokenKind::Func | TokenKind::Pub => stmts.push(Stmt::FnDef(self.parse_fn_def()?)),
                TokenKind::Import => stmts.push(Stmt::Import(self.parse_import()?)),
                TokenKind::Using => stmts.push(Stmt::Using(self.parse_using()?)),
                TokenKind::Class => stmts.push(Stmt::Class(self.parse_class()?)),
                TokenKind::Namespace => stmts.push(Stmt::Namespace(self.parse_namespace()?)),
                other => {
                    return Err(self.err(format!(
                        "顶层只允许函数定义、import、using、类定义或命名空间声明，实际是 {}",
                        self.describe(other)
                    )))
                }
            }
        }
        Ok(Program { stmts })
    }

    /// 命名空间声明（C# 风格块式）：`namespace tcmsg { ... }` 或点分 `namespace tcmsg.error { ... }`。
    ///
    /// 路径段用 `.` 连接（点分声明）；体内允许函数定义、类定义与嵌套命名空间。
    /// 前缀命名（`namespace tcmsg;`）由 tie-prep 预处理转化为块式后进入本函数。
    fn parse_namespace(&mut self) -> Result<NamespaceStmt, ParseError> {
        let span = self.advance().span; // 消费 `namespace`
        // 路径：至少一段标识符，可点分（`tcmsg.error`）
        let mut path = vec![self.expect_ident()?];
        while self.eat(&TokenKind::Dot) {
            path.push(self.expect_ident()?);
        }
        self.expect(TokenKind::LBrace, "命名空间声明必须跟 '{'")?;
        // 体内语句：函数 / 类 / 嵌套命名空间
        let mut body = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            match self.peek_kind() {
                TokenKind::Func | TokenKind::Pub => body.push(Stmt::FnDef(self.parse_fn_def()?)),
                TokenKind::Class => body.push(Stmt::Class(self.parse_class()?)),
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
            TokenKind::While => Ok(vec![Stmt::While(self.parse_while()?)]),
            TokenKind::For => Ok(vec![Stmt::For(self.parse_for()?)]),
            TokenKind::Switch => Ok(vec![Stmt::Switch(self.parse_switch()?)]),
            TokenKind::Return => Ok(vec![Stmt::Return(self.parse_return()?)]),
            TokenKind::LBrace => {
                // 裸块（后续版本），此处按语法错误处理
                Err(self.err("函数体内不能有裸代码块".into()))
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
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        self.expect(TokenKind::Semi, "语句结束符")?;
        Ok(vec![VarDeclStmt { name, ty, init, span, is_const }])
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

    /// `[pub] func name(params) -> Ty { stmts }`（pub 为 M2.1.7 可见性标记）。
    fn parse_fn_def(&mut self) -> Result<FnDefStmt, ParseError> {
        let span = self.peek().span; // pub 或 func 所在位置
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
                let pty = self.parse_type()?;
                // 默认值（可选参数）：`name: Ty = 字面量`。限字面量（含空表 []），
                // 与类字段默认值规则一致（语义层校验类型，语法层只负责解析）。
                let default = if self.eat(&TokenKind::Eq) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                params.push(Param { name: pname, ty: pty, default, span: pspan });
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
        let body = self.parse_block()?;
        Ok(FnDefStmt { name, params, ret_ty, is_pub, body, span })
    }

    /// `class Name [extends Parent] { 字段… 方法… }`（仅顶层，P8）。
    ///
    /// 类体由字段声明（`var name[: Ty] [= 默认值]`）与方法定义
    /// （`[static] func name(params) -> Ty { body }`）交错组成。
    fn parse_class(&mut self) -> Result<ClassDefStmt, ParseError> {
        let span = self.advance().span; // class
        let name = self.expect_ident()?;
        // 继承：`extends Parent`
        let parent = if self.eat(&TokenKind::Extends) {
            Some(self.expect_ident()?)
        } else {
            None
        };
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
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
                // 方法：`[static] func ...`（类内函数定义即方法）
                TokenKind::Func | TokenKind::Static => {
                    methods.push(self.parse_method()?);
                }
                other => {
                    return Err(self.err(format!(
                        "类体内只允许字段(var)或方法(func/static func)，实际是 {}",
                        self.describe(other)
                    )))
                }
            }
        }
        self.expect(TokenKind::RBrace, "'}'")?;
        Ok(ClassDefStmt { name, parent, fields, methods, span })
    }

    /// `[static] func name(params) -> Ty { body }`（P8，类内函数定义即方法）。
    ///
    /// 与 parse_fn_def 结构相同，多一个可选 `static` 前缀；`func` 在类体内即方法。
    fn parse_method(&mut self) -> Result<MethodDefStmt, ParseError> {
        let span = self.peek().span;
        let is_static = self.eat(&TokenKind::Static);
        self.expect(TokenKind::Func, "'func'")?;
        let name = self.expect_ident()?;
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        if !self.eat(&TokenKind::RParen) {
            loop {
                let pspan = self.peek().span;
                let pname = self.expect_ident()?;
                self.expect(TokenKind::Colon, "':'")?;
                let pty = self.parse_type()?;
                // 默认值（可选参数）：`name: Ty = 字面量`（与函数定义同一语法）。
                let default = if self.eat(&TokenKind::Eq) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                params.push(Param { name: pname, ty: pty, default, span: pspan });
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
        let body = self.parse_block()?;
        Ok(MethodDefStmt { name, is_static, params, ret_ty, body, span })
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

    /// `while cond { }`。
    fn parse_while(&mut self) -> Result<WhileStmt, ParseError> {
        let span = self.advance().span; // while
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(WhileStmt { cond, body, span })
    }

    /// `for var in expr { }`。
    fn parse_for(&mut self) -> Result<ForStmt, ParseError> {
        let span = self.advance().span; // for
        let var = self.expect_ident()?;
        self.expect(TokenKind::In, "'in'")?;
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(ForStmt { var, iter, body, span })
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

    fn parse_type(&mut self) -> Result<TypeSpec, ParseError> {
        match self.peek_kind() {
            TokenKind::TypeKw(ty) => {
                let ty = *ty;
                self.advance();
                Ok(TypeSpec::Named(ty))
            }
            // 元组类型：`(i64, string)` / `(x: i64, y: i64)`
            TokenKind::LParen => self.parse_tuple_type(),
            // 类类型：`MyClass`（用户自定义类型，P8）
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(TypeSpec::Class(name))
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
            // 当前实例 `this`：以特殊变量名形式进入表达式
            // （lexer 已把 this 作为关键字，用户无法声明同名变量，语义层识别该名）
            TokenKind::This => {
                self.advance();
                Ok(Expr::Var("this".to_string()))
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
        Ok(Expr::TableLit { cells, span })
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
    fn 类定义解析出字段方法与继承() {
        let prog = parse(
            "class Point extends Base {\n    var x: i64 = 0\n    var y: i64\n    static func origin() -> Point {\n        return 0\n    }\n    func move(dx: i64) -> void {\n        this.x = dx\n    }\n}",
        );
        let Stmt::Class(c) = &prog.stmts[0] else { panic!("期望类定义") };
        assert_eq!(c.name, "Point");
        assert_eq!(c.parent.as_deref(), Some("Base"));
        // 字段：`x`（带默认值）与 `y`（无默认值）
        assert_eq!(c.fields.len(), 2);
        assert_eq!(c.fields[0].name, "x");
        assert!(matches!(c.fields[0].ty, Some(TypeSpec::Named(TyKw::I64))));
        assert!(matches!(&c.fields[0].init, Some(Expr::IntLit(0))));
        assert_eq!(c.fields[1].name, "y");
        assert!(c.fields[1].init.is_none());
        // 方法：静态 `origin` 与实例 `move`
        assert_eq!(c.methods.len(), 2);
        let m0 = &c.methods[0];
        assert!(m0.is_static);
        assert_eq!(m0.name, "origin");
        assert!(matches!(&m0.ret_ty, TypeSpec::Class(n) if n == "Point"));
        let m1 = &c.methods[1];
        assert!(!m1.is_static);
        assert_eq!(m1.name, "move");
        assert_eq!(m1.params.len(), 1);
        assert_eq!(m1.params[0].name, "dx");
        assert!(matches!(m1.params[0].ty, TypeSpec::Named(TyKw::I64)));
        // 实例方法体内 `this.x = dx` 解析为 FieldAssign，base 为 this
        let Stmt::FieldAssign(fa) = &m1.body[0] else { panic!("期望字段赋值语句") };
        assert!(matches!(fa.base.as_ref(), Expr::Var(n) if n == "this"));
        assert_eq!(fa.field, "x");
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
}
