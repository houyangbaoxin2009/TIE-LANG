//! 语法分析器（Parser）。
//!
//! 职责：递归下降解析 token 流生成 AST。
//!
//! 说明：文件头部（`// tie:` 指令）已由 tie-prep 预处理阶段提取，
//! 本解析器只处理清理后的正文源码。

use super::ast::{
    AssignStmt, BinaryOp, Expr, ExprStmt, FnDefStmt, ForStmt, IfStmt, ImportStmt, Param, Program,
    ReturnStmt, Stmt, SwitchCase, SwitchStmt, TableCell, TableId, TypeSpec, UnaryOp, VarDeclStmt,
    WhileStmt,
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
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
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
        // 顶层只允许函数定义与 import（import 由 driver 递归展开为函数）
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            match self.peek_kind() {
                TokenKind::Func => stmts.push(Stmt::FnDef(self.parse_fn_def()?)),
                TokenKind::Import => stmts.push(Stmt::Import(self.parse_import()?)),
                other => {
                    return Err(self.err(format!(
                        "顶层只允许函数定义或 import，实际是 {}",
                        self.describe(other)
                    )))
                }
            }
        }
        Ok(Program { stmts })
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
        Ok(ImportStmt { path, alias, span })
    }

    // ---------- 语句解析 ----------

    /// 解析一个语句（函数体内）。
    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        match self.peek_kind() {
            TokenKind::Var => self.parse_var_decl(false).map(Stmt::VarDecl),
            TokenKind::Const => self.parse_var_decl(true).map(Stmt::VarDecl),
            TokenKind::If => self.parse_if().map(Stmt::If),
            TokenKind::While => self.parse_while().map(Stmt::While),
            TokenKind::For => self.parse_for().map(Stmt::For),
            TokenKind::Switch => self.parse_switch().map(Stmt::Switch),
            TokenKind::Return => self.parse_return().map(Stmt::Return),
            TokenKind::LBrace => {
                // 裸块（后续版本），此处按语法错误处理
                Err(self.err("函数体内不能有裸代码块".into()))
            }
            _ => self.parse_expr_or_assign(),
        }
    }

    /// 表达式语句与赋值语句的统一入口：
    /// `Ident = ...`（变量名后紧跟等号）→ Assign；否则解析为普通表达式语句。
    fn parse_expr_or_assign(&mut self) -> Result<Stmt, ParseError> {
        if let TokenKind::Ident(name) = self.peek_kind() {
            let name = name.clone();
            if self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Eq) {
                let span = self.advance().span; // 目标变量名
                self.expect(TokenKind::Eq, "'='")?;
                let value = self.parse_expr()?;
                self.expect(TokenKind::Semi, "语句结束符")?;
                return Ok(Stmt::Assign(AssignStmt { target: name, value, span }));
            }
        }
        self.parse_expr_stmt().map(Stmt::Expr)
    }

    /// `var name[: Ty] = expr` / `const name[: Ty] = expr`（ASI/分号结束）。
    fn parse_var_decl(&mut self, is_const: bool) -> Result<VarDeclStmt, ParseError> {
        let span = self.advance().span; // var / const
        let name = self.expect_ident()?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        self.expect(TokenKind::Semi, "语句结束符")?;
        Ok(VarDeclStmt { name, ty, init, span, is_const })
    }

    /// `func name(params) -> Ty { stmts }`。
    fn parse_fn_def(&mut self) -> Result<FnDefStmt, ParseError> {
        let span = self.advance().span; // func
        let name = self.expect_ident()?;
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        if !self.eat(&TokenKind::RParen) {
            loop {
                let pspan = self.peek().span;
                let pname = self.expect_ident()?;
                self.expect(TokenKind::Colon, "':'")?;
                let pty = self.parse_type()?;
                params.push(Param { name: pname, ty: pty, span: pspan });
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
        Ok(FnDefStmt { name, params, ret_ty, body, span })
    }

    /// `{ stmts }` 代码块。
    fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(TokenKind::LBrace, "'{'")?;
        let mut stmts = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            stmts.push(self.parse_stmt()?);
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

    /// `switch subject { case 值: 语句… default: 语句… }`。
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
                    let value = self.parse_case_value()?;
                    self.expect(TokenKind::Colon, "':'")?;
                    let body = self.parse_switch_body()?;
                    cases.push(SwitchCase { value, body, span: cspan });
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

    /// case 匹配值：编译期字面量（整数/浮点/字符/布尔/字符串/负数字面量）。
    /// 不支持变量或任意表达式（语义层同样校验）。
    fn parse_case_value(&mut self) -> Result<Expr, ParseError> {
        // 负数 case：`case -1:` 由一元负号包裹
        let expr = self.parse_unary()?;
        Ok(expr)
    }

    /// case/default 分支体：连续语句直到下一个 case/default/右花括号/文件结束。
    fn parse_switch_body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        while !matches!(
            self.peek_kind(),
            TokenKind::Case | TokenKind::Default | TokenKind::RBrace | TokenKind::Eof
        ) {
            stmts.push(self.parse_stmt()?);
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

    /// 表达式语句（以分号/ASI 结束）。
    fn parse_expr_stmt(&mut self) -> Result<ExprStmt, ParseError> {
        let expr = self.parse_expr()?;
        let span = expr_span(&expr).unwrap_or(self.peek().span);
        self.expect(TokenKind::Semi, "语句结束符")?;
        Ok(ExprStmt { expr, span })
    }

    // ---------- 类型解析 ----------

    fn parse_type(&mut self) -> Result<TypeSpec, ParseError> {
        match self.peek_kind() {
            TokenKind::TypeKw(ty) => {
                let ty = *ty;
                self.advance();
                Ok(TypeSpec::Named(ty))
            }
            other => Err(self.err(format!("期望类型，实际是 {}", self.describe(other)))),
        }
    }

    // ---------- 表达式解析（优先级爬升） ----------

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        // 范围运算符 `..` 优先级最低：`a..b` 先解析 lhs，再解析 rhs
        let lhs = self.parse_or()?;
        if self.eat(&TokenKind::DotDot) {
            let end = self.parse_expr()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            return Ok(Expr::Range { start: Box::new(lhs), end: Box::new(end), span });
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
        let mut lhs = self.parse_equality()?;
        while self.eat(&TokenKind::AndAnd) {
            let rhs = self.parse_equality()?;
            let span = expr_span(&lhs).unwrap_or(self.peek().span);
            lhs = Expr::Binary { op: BinaryOp::And, lhs: Box::new(lhs), rhs: Box::new(rhs), span };
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
        let mut lhs = self.parse_term()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Lt => BinaryOp::Lt,
                TokenKind::Gt => BinaryOp::Gt,
                TokenKind::Le => BinaryOp::Le,
                TokenKind::Ge => BinaryOp::Ge,
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
            _ => self.parse_primary()?,
        };
        // 后缀下标访问：`base[index]`（可链式 `a[0][1]`）
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
            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen, "')'")?;
                Ok(inner)
            }
            TokenKind::LBracket => self.parse_table_lit(span),
            other => Err(ParseError {
                span,
                message: format!("无法以 {} 开始表达式", self.describe(&other)),
            }),
        }
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
        | Expr::Range { span, .. }
        | Expr::TableLit { span, .. } => Some(*span),
        _ => None,
    }
}
