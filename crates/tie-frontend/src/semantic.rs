//! 语义分析器（Semantic Analyzer）。
//!
//! 职责：在 AST 上进行静态检查——
//! - 构建符号表：函数签名、局部变量及其类型
//! - 类型检查：变量使用前已声明、类型匹配（let 推导）、运算数类型正确
//! - 结构检查：入口函数 `main` 存在、return 类型一致
//!
//! 输出：每个函数体内各表达式的推断类型表，供 IR 生成阶段使用
//! （IR 生成时无需重复推导类型）。

use super::ast::{BinaryOp, Expr, FnDefStmt, Program, Stmt, TypeSpec, UnaryOp};
use super::lexer::{Span, TyKw};
use std::collections::HashMap;
use std::fmt;
/// 语义错误：携带位置与信息。
#[derive(Debug, Clone)]
pub struct SemanticError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "语义错误 @{}:{}: {}", self.span.line, self.span.col, self.message)
    }
}

/// 语义分析结果：函数签名表 + 函数内表达式类型推断表。
#[derive(Debug, Default)]
pub struct SemanticResult {
    /// 函数签名：函数名 → 签名
    pub funcs: HashMap<String, FuncSig>,
    /// 各函数体内表达式的类型推断（AST 节点地址 → 类型）
    pub expr_types: HashMap<usize, TypeSpec>,
}

/// 函数签名。
#[derive(Debug, Clone)]
pub struct FuncSig {
    pub param_tys: Vec<TypeSpec>,
    pub ret_ty: TypeSpec,
}

/// 语义分析入口。
pub fn analyze(program: &Program) -> Result<SemanticResult, SemanticError> {
    let mut ctx = Analyzer { result: SemanticResult::default() };

    // 第一遍：收集所有函数签名（允许前向引用）
    for stmt in &program.stmts {
        if let Stmt::FnDef(f) = stmt {
            let sig = FuncSig {
                param_tys: f.params.iter().map(|p| p.ty).collect(),
                ret_ty: f.ret_ty,
            };
            if ctx.result.funcs.insert(f.name.clone(), sig).is_some() {
                return Err(SemanticError {
                    span: f.span,
                    message: format!("函数 '{}' 重复定义", f.name),
                });
            }
        }
    }

    // 第二遍：检查函数体
    for stmt in &program.stmts {
        if let Stmt::FnDef(f) = stmt {
            ctx.check_fn(f)?;
        }
    }

    // 入口检查：logic 类文件必须有 main（main 检查在 driver 按头类型分派）
    Ok(ctx.result)
}

/// 语义分析器上下文。
struct Analyzer {
    result: SemanticResult,
}

impl Analyzer {
    fn check_fn(&mut self, f: &FnDefStmt) -> Result<(), SemanticError> {
        // 函数体内作用域：参数先入表
        let mut scope: HashMap<String, TypeSpec> = HashMap::new();
        for p in &f.params {
            if scope.insert(p.name.clone(), p.ty).is_some() {
                return Err(SemanticError {
                    span: p.span,
                    message: format!("参数 '{}' 重复", p.name),
                });
            }
        }
        for stmt in &f.body {
            self.check_stmt(stmt, &mut scope, f.ret_ty)?;
        }
        Ok(())
    }

    fn check_stmt(
        &mut self,
        stmt: &Stmt,
        scope: &mut HashMap<String, TypeSpec>,
        ret_ty: TypeSpec,
    ) -> Result<(), SemanticError> {
        match stmt {
            Stmt::VarDecl(v) => {
                // 先检查初始化表达式（可能引用自身？不允许）
                let init_ty = self.infer_expr(&v.init, scope)?;
                let declared = v.ty;
                // 类型匹配：显式类型必须与推导类型兼容（int 与 float 不隐式转换；
                // 但整数字面量可适配任意整数标注、浮点字面量可适配任意浮点标注；
                // 宽类型 num/text/misc 是「类别框」，编译器归类即可，不精确推导）
                match declared {
                    Some(d) => {
                        if d.is_wide() {
                            // 宽类型：只校验初始化表达式是否属于该类别框。
                            // 通过后 scope 存**具体推导类型**（IR 层无感知宽类型），
                            // 编译器省去精确匹配校验 → 加快编译速度。
                            if !d.wide_accepts(init_ty) {
                                return Err(SemanticError {
                                    span: v.span,
                                    message: format!(
                                        "变量 '{}' 标注 {} 不匹配初始化表达式的类型 {}",
                                        v.name,
                                        type_name(d),
                                        type_name(init_ty)
                                    ),
                                });
                            }
                            scope.insert(v.name.clone(), init_ty);
                        } else if d.is_table() {
                            // table：初始化必须是表字面量（数组/高级数组）
                            if !matches!(v.init, Expr::TableLit { .. }) {
                                return Err(SemanticError {
                                    span: v.span,
                                    message: format!(
                                        "变量 '{}' 标注 table，初始化必须是表字面量 [...]",
                                        v.name
                                    ),
                                });
                            }
                            scope.insert(v.name.clone(), init_ty);
                        } else if !types_match(d, init_ty, Some(&v.init)) {
                            return Err(SemanticError {
                                span: v.span,
                                message: format!(
                                    "变量 '{}' 类型不匹配：标注 {}，表达式推导为 {}",
                                    v.name,
                                    type_name(d),
                                    type_name(init_ty)
                                ),
                            });
                        } else {
                            scope.insert(v.name.clone(), d);
                        }
                    }
                    None => {
                        if init_ty.is_void() {
                            return Err(SemanticError {
                                span: v.span,
                                message: format!("变量 '{}' 不能用 void 表达式初始化", v.name),
                            });
                        }
                        scope.insert(v.name.clone(), init_ty);
                    }
                }
                // 推导类型写入结果表（记录到 init 表达式上，IR 用）
                self.result.expr_types.insert(addr_of(&v.init), init_ty);
                Ok(())
            }
            Stmt::FnDef(_) => {
                // 函数体内的嵌套函数暂不支持
                Err(SemanticError {
                    span: stmt_span(stmt),
                    message: "函数体内不支持嵌套函数定义".into(),
                })
            }
            Stmt::Expr(e) => {
                let ty = self.infer_expr(&e.expr, scope)?;
                self.result.expr_types.insert(addr_of(&e.expr), ty);
                Ok(())
            }
            Stmt::Return(r) => {
                let expr_ty = match &r.expr {
                    Some(e) => {
                        let ty = self.infer_expr(e, scope)?;
                        self.result.expr_types.insert(addr_of(e), ty);
                        ty
                    }
                    None => TypeSpec::Named(TyKw::Void),
                };
                // return 类型与函数返回类型匹配（含字面量适配）
                if !types_match(ret_ty, expr_ty, r.expr.as_ref()) {
                    return Err(SemanticError {
                        span: r.span,
                        message: format!(
                            "return 类型不匹配：函数返回 {}，实际返回 {}",
                            type_name(ret_ty),
                            type_name(expr_ty)
                        ),
                    });
                }
                Ok(())
            }
            Stmt::If(i) => {
                let c = self.infer_expr(&i.cond, scope)?;
                self.result.expr_types.insert(addr_of(&i.cond), c);
                if !is_bool_like(c) {
                    return Err(SemanticError {
                        span: expr_span_of(&i.cond),
                        message: "if 条件必须是 bool".into(),
                    });
                }
                self.check_block(&i.then_branch, scope, ret_ty)?;
                self.check_block(&i.else_branch, scope, ret_ty)?;
                Ok(())
            }
            Stmt::While(w) => {
                let c = self.infer_expr(&w.cond, scope)?;
                self.result.expr_types.insert(addr_of(&w.cond), c);
                if !is_bool_like(c) {
                    return Err(SemanticError {
                        span: expr_span_of(&w.cond),
                        message: "while 条件必须是 bool".into(),
                    });
                }
                self.check_block(&w.body, scope, ret_ty)?;
                Ok(())
            }
            Stmt::For(f) => {
                // for var in iter：iter 应为范围或数组（范围先支持）
                let iter_ty = self.infer_expr(&f.iter, scope)?;
                self.result.expr_types.insert(addr_of(&f.iter), iter_ty);
                // 循环变量类型：范围 → i64（默认整数）
                scope.insert(f.var.clone(), TypeSpec::Named(TyKw::I64));
                self.check_block(&f.body, scope, ret_ty)?;
                Ok(())
            }
        }
    }

    fn check_block(
        &mut self,
        stmts: &[Stmt],
        scope: &mut HashMap<String, TypeSpec>,
        ret_ty: TypeSpec,
    ) -> Result<(), SemanticError> {
        for s in stmts {
            self.check_stmt(s, scope, ret_ty)?;
        }
        Ok(())
    }

    /// 表达式类型推断（同时检查）。
    fn infer_expr(
        &mut self,
        expr: &Expr,
        scope: &HashMap<String, TypeSpec>,
    ) -> Result<TypeSpec, SemanticError> {
        let ty = match expr {
            Expr::IntLit(_) => TypeSpec::Named(TyKw::I64),
            Expr::FloatLit(_) => TypeSpec::Named(TyKw::F64),
            Expr::BoolLit(_) => TypeSpec::Named(TyKw::Bool),
            Expr::StrLit(_) => TypeSpec::Named(TyKw::Str),
            Expr::Var(name) => match scope.get(name) {
                Some(t) => *t,
                None => {
                    return Err(SemanticError {
                        span: expr_span_of(expr),
                        message: format!("未声明的变量 '{name}'"),
                    })
                }
            },
            Expr::Call { name, args, span } => {
                // 内置函数 println：任意参数，void
                if name == "println" {
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at);
                    }
                    return Ok(TypeSpec::Named(TyKw::Void));
                }
                // 用户函数：校验参数个数与类型
                let sig = self.result.funcs.get(name).cloned().ok_or_else(|| SemanticError {
                    span: *span,
                    message: format!("未定义的函数 '{name}'"),
                })?;
                if sig.param_tys.len() != args.len() {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "函数 '{name}' 期望 {} 个参数，实际 {} 个",
                            sig.param_tys.len(),
                            args.len()
                        ),
                    });
                }
                for (a, want) in args.iter().zip(sig.param_tys.iter()) {
                    let at = self.infer_expr(a, scope)?;
                    if !types_match(*want, at, Some(a)) {
                        return Err(SemanticError {
                            span: expr_span_of(a),
                            message: format!(
                                "调用 '{name}' 参数类型不匹配：期望 {}，实际 {}",
                                type_name(*want),
                                type_name(at)
                            ),
                        });
                    }
                    self.result.expr_types.insert(addr_of(a), at);
                }
                sig.ret_ty
            }
            Expr::Unary { op, operand, span } => {
                let ot = self.infer_expr(operand, scope)?;
                self.result.expr_types.insert(addr_of(operand), ot);
                match op {
                    UnaryOp::Neg => {
                        if !is_number(ot) {
                            return Err(SemanticError {
                                span: *span,
                                message: "取负运算的操作数必须是数字".into(),
                            });
                        }
                        ot
                    }
                    UnaryOp::Not => {
                        if !is_bool_like(ot) {
                            return Err(SemanticError {
                                span: *span,
                                message: "逻辑非的操作数必须是 bool".into(),
                            });
                        }
                        TypeSpec::Named(TyKw::Bool)
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, span } => {
                let lt = self.infer_expr(lhs, scope)?;
                let rt = self.infer_expr(rhs, scope)?;
                self.result.expr_types.insert(addr_of(lhs), lt);
                self.result.expr_types.insert(addr_of(rhs), rt);
                // 左右类型必须一致（int 与 float 不隐式转换）
                if !types_compatible(lt, rt) {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "二元运算两侧类型不一致：{} 与 {}",
                            type_name(lt),
                            type_name(rt)
                        ),
                    });
                }
                match op {
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod => {
                        if !is_number(lt) {
                            return Err(SemanticError {
                                span: *span,
                                message: format!("算术运算符不能用于 {}", type_name(lt)),
                            });
                        }
                        // 取模只支持整数
                        if *op == BinaryOp::Mod && !lt.is_int() {
                            return Err(SemanticError {
                                span: *span,
                                message: "取模运算只支持整数".into(),
                            });
                        }
                        lt
                    }
                    BinaryOp::Eq
                    | BinaryOp::NotEq
                    | BinaryOp::Lt
                    | BinaryOp::Gt
                    | BinaryOp::Le
                    | BinaryOp::Ge => {
                        if !is_number(lt) && !matches!(lt, TypeSpec::Named(TyKw::Bool)) {
                            return Err(SemanticError {
                                span: *span,
                                message: format!("比较运算符不能用于 {}", type_name(lt)),
                            });
                        }
                        TypeSpec::Named(TyKw::Bool)
                    }
                    BinaryOp::And | BinaryOp::Or => {
                        if !is_bool_like(lt) {
                            return Err(SemanticError {
                                span: *span,
                                message: "逻辑运算符两侧必须是 bool".into(),
                            });
                        }
                        TypeSpec::Named(TyKw::Bool)
                    }
                }
            }
            Expr::Range { start, end, span } => {
                let st = self.infer_expr(start, scope)?;
                let et = self.infer_expr(end, scope)?;
                self.result.expr_types.insert(addr_of(start), st);
                self.result.expr_types.insert(addr_of(end), et);
                if !st.is_int() || !et.is_int() {
                    return Err(SemanticError {
                        span: *span,
                        message: "范围两端必须是整数".into(),
                    });
                }
                // 范围类型：视作 i64 容器（当前仅 for 使用）
                TypeSpec::Named(TyKw::I64)
            }
            Expr::TableLit { cells, span } => {
                // 表字面量：所有元素类型必须一致（表是元素同构的容器）。
                // 空表视作 i64 元素（类型后续由上下文确定）。
                if cells.is_empty() {
                    return Ok(TypeSpec::Named(TyKw::I64));
                }
                // 推导第一个元素的类型，其余元素必须与其一致
                let first_ty = self.infer_expr(&cells[0].value, scope)?;
                self.result.expr_types.insert(addr_of(&cells[0].value), first_ty);
                for cell in &cells[1..] {
                    let ct = self.infer_expr(&cell.value, scope)?;
                    self.result.expr_types.insert(addr_of(&cell.value), ct);
                    if !types_compatible(first_ty, ct) {
                        return Err(SemanticError {
                            span: *span,
                            message: format!(
                                "表元素类型不一致：{} 与 {}",
                                type_name(first_ty),
                                type_name(ct)
                            ),
                        });
                    }
                }
                // 表类型：元素类型（当前 IR 阶段仅支持数/字符串元素的同构表）
                first_ty
            }
        };
        Ok(ty)
    }
}

// ---------- 辅助 ----------

/// 表达式在内存中的地址（作为 expr_types 表的键）。
fn addr_of(expr: &Expr) -> usize {
    expr as *const Expr as usize
}

/// 从语句中取 span。
fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::VarDecl(v) => v.span,
        Stmt::FnDef(f) => f.span,
        Stmt::Expr(e) => e.span,
        Stmt::Return(r) => r.span,
        Stmt::If(i) => i.span,
        Stmt::While(w) => w.span,
        Stmt::For(f) => f.span,
    }
}

/// 从表达式中取 span（含字面量：用占位位置）。
fn expr_span_of(expr: &Expr) -> Span {
    match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::StrLit(_) | Expr::BoolLit(_) | Expr::Var(_) => {
            // 字面量无 span，用 (0,0) 占位（语义错误主要针对变量/调用，已有 span）
            Span { line: 0, col: 0 }
        }
        Expr::Call { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Range { span, .. }
        | Expr::TableLit { span, .. } => *span,
    }
}

/// 类型是否兼容（当前：必须完全相同；int 与 float 不隐式转换）。
fn types_compatible(a: TypeSpec, b: TypeSpec) -> bool {
    a == b
}

/// 类型是否匹配（含字面量适配）。
///
/// 规则：
/// - 显式类型与推导类型完全相同 → 匹配；
/// - 整数字面量可适配任意整数标注（如 `let x: i32 = 42`）；
/// - 浮点字面量可适配任意浮点标注（如 `let x: f64 = 1.5`）；
/// - 其余情况（变量、运算结果等）不隐式转换。
fn types_match(want: TypeSpec, got: TypeSpec, init: Option<&Expr>) -> bool {
    if types_compatible(want, got) {
        return true;
    }
    match init {
        Some(Expr::IntLit(_)) => want.is_int() && got.is_int(),
        Some(Expr::FloatLit(_)) => want.is_float() && got.is_float(),
        _ => false,
    }
}

/// 是否为数字类型（整数或浮点）。
fn is_number(t: TypeSpec) -> bool {
    t.is_number()
}

/// 是否为布尔类（if/while 条件）。
fn is_bool_like(t: TypeSpec) -> bool {
    matches!(t, TypeSpec::Named(TyKw::Bool))
}

/// 类型的可读名称。
fn type_name(t: TypeSpec) -> &'static str {
    match t {
        TypeSpec::Named(k) => k.as_str(),
    }
}
