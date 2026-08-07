//! 语义分析器（Semantic Analyzer）。
//!
//! 职责：在 AST 上进行静态检查——
//! - 构建符号表：函数签名、局部变量及其类型
//! - 类型检查：变量使用前已声明、类型匹配（let 推导）、运算数类型正确
//! - 结构检查：入口函数 `main` 存在、return 类型一致
//!
//! 输出：每个函数体内各表达式的推断类型表，供 IR 生成阶段使用
//! （IR 生成时无需重复推导类型）。

use super::ast::{
    BinaryOp, Expr, FnDefStmt, Program, Stmt, TableId, TupleField, TypeSpec, UnaryOp,
};
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
    /// 不可变变量集合（const 声明，赋值时校验）
    pub const_vars: std::collections::HashSet<String>,
    /// 表元数据：表字面量表达式地址 → 元素类型与长度（IR 生成布局用）
    pub tables: HashMap<usize, TableInfo>,
}

/// 表（table）的布局信息：元素类型与元素个数。
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// 元素类型（同构容器）
    pub elem_ty: TypeSpec,
    /// 元素个数（编译期已知，定长）
    pub len: usize,
}

/// 函数签名。
#[derive(Debug, Clone)]
pub struct FuncSig {
    pub param_tys: Vec<TypeSpec>,
    pub ret_ty: TypeSpec,
}

/// 语义分析入口。
pub fn analyze(program: &Program) -> Result<SemanticResult, SemanticError> {
    let mut ctx = Analyzer {
        result: SemanticResult::default(),
        table_vars: HashMap::new(),
        cur_fn: String::new(),
    };

    // 第一遍：收集所有函数签名（允许前向引用）
    for stmt in &program.stmts {
        if let Stmt::FnDef(f) = stmt {
            let sig = FuncSig {
                param_tys: f.params.iter().map(|p| p.ty.clone()).collect(),
                ret_ty: f.ret_ty.clone(),
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
    /// 表变量布局：函数内变量名 → 表布局信息（下标访问 / 遍历时查询元素类型）
    table_vars: HashMap<(String, String), TableInfo>,
    /// 当前检查的函数名（table_vars 键用）
    cur_fn: String,
}

impl Analyzer {
    fn check_fn(&mut self, f: &FnDefStmt) -> Result<(), SemanticError> {
        self.cur_fn = f.name.clone();
        // 函数体内作用域：参数先入表
        let mut scope: HashMap<String, TypeSpec> = HashMap::new();
        for p in &f.params {
            if scope.insert(p.name.clone(), p.ty.clone()).is_some() {
                return Err(SemanticError {
                    span: p.span,
                    message: format!("参数 '{}' 重复", p.name),
                });
            }
        }
        for stmt in &f.body {
            self.check_stmt(stmt, &mut scope, &f.ret_ty)?;
        }
        Ok(())
    }

    fn check_stmt(
        &mut self,
        stmt: &Stmt,
        scope: &mut HashMap<String, TypeSpec>,
        ret_ty: &TypeSpec,
    ) -> Result<(), SemanticError> {
        match stmt {
            Stmt::VarDecl(v) => {
                // 先检查初始化表达式（可能引用自身？不允许）
                let init_ty = self.infer_expr(&v.init, scope)?;
                let declared = v.ty.clone();
                // 类型匹配：显式类型必须与推导类型兼容（int 与 float 不隐式转换；
                // 但整数字面量可适配任意整数标注、浮点字面量可适配任意浮点标注；
                // 宽类型 num/text/misc 是「类别框」，编译器归类即可，不精确推导）
                match &declared {
                    Some(d) => {
                        if d.is_wide() {
                            // 宽类型：只校验初始化表达式是否属于该类别框。
                            // 通过后 scope 存**具体推导类型**（IR 层无感知宽类型），
                            // 编译器省去精确匹配校验 → 加快编译速度。
                            if !d.wide_accepts(&init_ty) {
                                return Err(SemanticError {
                                    span: v.span,
                                    message: format!(
                                        "变量 '{}' 标注 {} 不匹配初始化表达式的类型 {}",
                                        v.name,
                                        type_name(d),
                                        type_name(&init_ty)
                                    ),
                                });
                            }
                            scope.insert(v.name.clone(), init_ty.clone());
                        } else if d.is_table() {
                            // table：初始化必须是表字面量（数组/高级数组）
                            let Expr::TableLit { cells, .. } = &v.init else {
                                return Err(SemanticError {
                                    span: v.span,
                                    message: format!(
                                        "变量 '{}' 标注 table，初始化必须是表字面量 [...]",
                                        v.name
                                    ),
                                });
                            };
                            // M2 范围：仅支持单行纯位置表（无字符串 id、无分号分行）
                            let rows: std::collections::HashSet<usize> =
                                cells.iter().map(|c| c.row).collect();
                            if rows.len() > 1 {
                                return Err(SemanticError {
                                    span: v.span,
                                    message: "二维表（分号分行）的运行时留待 M3，当前仅支持单行表".into(),
                                });
                            }
                            if cells.iter().any(|c| matches!(c.id, Some(TableId::Str(_)))) {
                                return Err(SemanticError {
                                    span: v.span,
                                    message: "字符串 id 表（[\"a\":1]）的运行时留待 M3，当前仅支持数字下标".into(),
                                });
                            }
                            // 记录布局元数据：元素类型 = init 推导类型，长度 = 元素个数
                            let info = TableInfo {
                                elem_ty: init_ty.clone(),
                                len: cells.len(),
                            };
                            self.result.tables.insert(addr_of(&v.init), info.clone());
                            // 变量名 → 布局（下标访问/遍历时按变量名查询元素类型）
                            self.table_vars.insert((self.cur_fn.clone(), v.name.clone()), info);
                            // scope 存 Table 标记（表是容器，不是普通值）
                            scope.insert(v.name.clone(), TypeSpec::Named(TyKw::Table));
                        } else if !types_match(d, &init_ty, Some(&v.init)) {
                            return Err(SemanticError {
                                span: v.span,
                                message: format!(
                                    "变量 '{}' 类型不匹配：标注 {}，表达式推导为 {}",
                                    v.name,
                                    type_name(d),
                                    type_name(&init_ty)
                                ),
                            });
                        } else {
                            scope.insert(v.name.clone(), d.clone());
                        }
                    }
                    None => {
                        if init_ty.is_void() {
                            return Err(SemanticError {
                                span: v.span,
                                message: format!("变量 '{}' 不能用 void 表达式初始化", v.name),
                            });
                        }
                        scope.insert(v.name.clone(), init_ty.clone());
                    }
                }
                // 推导类型写入结果表（记录到 init 表达式上，IR 用）。
                // 特例：元组标注 + 元组字面量初始化时，覆盖写为**标注类型**——
                // 文本 IR 的聚合类型必须逐字段精确（`ret {i32,i32} <i64 建的值>` 非法），
                // 语义层校验通过后由 IR 按标注类型建结构体（宽类型分支的先例）。
                let recorded = if matches!(declared, Some(TypeSpec::Tuple(_)))
                    && matches!(&v.init, Expr::TupleLit { .. })
                {
                    declared.clone().expect("declared 已确认是 Some")
                } else {
                    init_ty
                };
                self.result.expr_types.insert(addr_of(&v.init), recorded);
                // 记录 const 变量（赋值语句会拒绝重赋值）
                if v.is_const {
                    self.result.const_vars.insert(v.name.clone());
                }
                Ok(())
            }
            Stmt::FnDef(_) => {
                // 函数体内的嵌套函数暂不支持
                Err(SemanticError {
                    span: stmt_span(stmt),
                    message: "函数体内不支持嵌套函数定义".into(),
                })
            }
            Stmt::Import(_) => {
                // import 只允许出现在文件顶层（driver 在语义分析前已展开为函数）
                Err(SemanticError {
                    span: stmt_span(stmt),
                    message: "import 语句只能出现在文件顶层".into(),
                })
            }
            Stmt::Expr(e) => {
                let ty = self.infer_expr(&e.expr, scope)?;
                self.result.expr_types.insert(addr_of(&e.expr), ty);
                Ok(())
            }
            Stmt::Assign(a) => {
                // 赋值：目标必须已声明；const 不可变；类型必须匹配
                let target_ty = match scope.get(&a.target) {
                    Some(t) => t.clone(),
                    None => {
                        return Err(SemanticError {
                            span: a.span,
                            message: format!("赋值目标 '{}' 未声明", a.target),
                        })
                    }
                };
                let value_ty = self.infer_expr(&a.value, scope)?;
                self.result.expr_types.insert(addr_of(&a.value), value_ty.clone());
                // const 变量不允许重新赋值
                if self.result.const_vars.contains(&a.target) {
                    return Err(SemanticError {
                        span: a.span,
                        message: format!("不能给 const 变量 '{}' 赋值", a.target),
                    });
                }
                // 类型必须兼容（无字面量适配：赋值用变量原本的具体类型）
                if !types_match(&target_ty, &value_ty, Some(&a.value)) {
                    return Err(SemanticError {
                        span: a.span,
                        message: format!(
                            "赋值类型不匹配：变量 '{}' 类型为 {}，表达式为 {}",
                            a.target,
                            type_name(&target_ty),
                            type_name(&value_ty)
                        ),
                    });
                }
                Ok(())
            }
            Stmt::Return(r) => {
                let expr_ty = match &r.expr {
                    Some(e) => {
                        let ty = self.infer_expr(e, scope)?;
                        self.result.expr_types.insert(addr_of(e), ty.clone());
                        ty
                    }
                    None => TypeSpec::Named(TyKw::Void),
                };
                // return 类型与函数返回类型匹配（含字面量适配）
                if !types_match(ret_ty, &expr_ty, r.expr.as_ref()) {
                    return Err(SemanticError {
                        span: r.span,
                        message: format!(
                            "return 类型不匹配：函数返回 {}，实际返回 {}",
                            type_name(ret_ty),
                            type_name(&expr_ty)
                        ),
                    });
                }
                // 元组返回字面量：把**返回类型**覆盖写回 expr_types（键 = return 表达式地址），
                // IR 按返回类型建结构体（文本 IR 聚合类型必须逐字段精确，与 VarDecl 分支同理）。
                if let Some(e) = &r.expr
                    && matches!(ret_ty, TypeSpec::Tuple(_))
                    && matches!(e, Expr::TupleLit { .. })
                {
                    self.result.expr_types.insert(addr_of(e), ret_ty.clone());
                }
                Ok(())
            }
            Stmt::If(i) => {
                let c = self.infer_expr(&i.cond, scope)?;
                self.result.expr_types.insert(addr_of(&i.cond), c.clone());
                if !is_bool_like(&c) {
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
                self.result.expr_types.insert(addr_of(&w.cond), c.clone());
                if !is_bool_like(&c) {
                    return Err(SemanticError {
                        span: expr_span_of(&w.cond),
                        message: "while 条件必须是 bool".into(),
                    });
                }
                self.check_block(&w.body, scope, ret_ty)?;
                Ok(())
            }
            Stmt::For(f) => {
                // for var in iter：iter 应为范围（默认 i64 元素）或表（元素类型）
                let iter_ty = self.infer_expr(&f.iter, scope)?;
                self.result.expr_types.insert(addr_of(&f.iter), iter_ty.clone());
                let elem_ty = if iter_ty == TypeSpec::Named(TyKw::Table) {
                    // 表遍历：循环变量类型 = 表的元素类型
                    match &f.iter {
                        Expr::Var(name) => {
                            let key = (self.cur_fn.clone(), name.clone());
                            match self.table_vars.get(&key) {
                                Some(info) => info.elem_ty.clone(),
                                None => {
                                    return Err(SemanticError {
                                        span: f.span,
                                        message: format!("遍历的表 '{name}' 缺少布局元数据（内部错误）"),
                                    })
                                }
                            }
                        }
                        _ => {
                            return Err(SemanticError {
                                span: f.span,
                                message: "表遍历仅支持表变量（内联表字面量遍历留待后续）".into(),
                            })
                        }
                    }
                } else if matches!(f.iter, Expr::Range { .. }) {
                    TypeSpec::Named(TyKw::I64)
                } else {
                    return Err(SemanticError {
                        span: f.span,
                        message: format!(
                            "for 迭代对象仅支持范围（0..10）或表变量，实际是 {}",
                            type_name(&iter_ty)
                        ),
                    });
                };
                scope.insert(f.var.clone(), elem_ty);
                self.check_block(&f.body, scope, ret_ty)?;
                Ok(())
            }
            Stmt::Switch(s) => {
                // subject 表达式类型推断（与 case 值类型必须一致）
                let subject_ty = self.infer_expr(&s.subject, scope)?;
                self.result.expr_types.insert(addr_of(&s.subject), subject_ty.clone());
                // subject 必须是数字、布尔、字符或字符串（字符串用 strcmp 比较）
                if !is_number(&subject_ty)
                    && !is_bool_like(&subject_ty)
                    && !matches!(&subject_ty, TypeSpec::Named(TyKw::Char))
                    && !matches!(&subject_ty, TypeSpec::Named(TyKw::Str))
                {
                    return Err(SemanticError {
                        span: s.span,
                        message: format!(
                            "switch 对象仅支持数字、布尔、字符或字符串类型，实际是 {}",
                            type_name(&subject_ty)
                        ),
                    });
                }
                // case 值必须与 subject 类型一致（字面量类型精确匹配；
                // 整数字面量可适配任意整数，浮点字面量可适配任意浮点）
                let mut seen: Vec<String> = Vec::new();
                for c in &s.cases {
                    let value_ty = self.infer_expr(&c.value, scope)?;
                    self.result.expr_types.insert(addr_of(&c.value), value_ty.clone());
                    // case 值必须是编译期字面量（不允许变量/表达式）
                    if !is_const_literal(&c.value) {
                        return Err(SemanticError {
                            span: c.span,
                            message: "case 值必须是字面量（整数/浮点/字符/布尔/字符串）".into(),
                        });
                    }
                    // case 值类型必须与 subject 类型匹配
                    if !types_match(&subject_ty, &value_ty, Some(&c.value)) {
                        return Err(SemanticError {
                            span: c.span,
                            message: format!(
                                "case 值类型 {} 与 switch 对象类型 {} 不匹配",
                                type_name(&value_ty),
                                type_name(&subject_ty)
                            ),
                        });
                    }
                    // 重复 case 检测
                    let key = literal_key(&c.value);
                    if seen.contains(&key) {
                        return Err(SemanticError {
                            span: c.span,
                            message: format!("重复的 case 值 {}", key),
                        });
                    }
                    seen.push(key);
                    self.check_block(&c.body, scope, ret_ty)?;
                }
                self.check_block(&s.default_body, scope, ret_ty)?;
                Ok(())
            }
        }
    }

    fn check_block(
        &mut self,
        stmts: &[Stmt],
        scope: &mut HashMap<String, TypeSpec>,
        ret_ty: &TypeSpec,
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
            Expr::CharLit(_) => TypeSpec::Named(TyKw::Char),
            Expr::Var(name) => match scope.get(name) {
                Some(t) => t.clone(),
                None => {
                    return Err(SemanticError {
                        span: expr_span_of(expr),
                        message: format!("未声明的变量 '{name}'"),
                    })
                }
            },
            Expr::Call { name, args, span } => {
                // 内置函数 println：任意参数，void（元组除外——IR 层 printf 变参无法传结构体）
                if name == "println" {
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        if matches!(&at, TypeSpec::Tuple(_)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("println 不支持元组参数（类型 {}）", type_name(&at)),
                            });
                        }
                        self.result.expr_types.insert(addr_of(a), at);
                    }
                    return Ok(TypeSpec::Named(TyKw::Void));
                }
                // 内置函数 len：单参数，要求字符串，返回 i64（字符串长度）
                if name == "len" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("len() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("len() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::I64));
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
                    if !types_match(want, &at, Some(a)) {
                        return Err(SemanticError {
                            span: expr_span_of(a),
                            message: format!(
                                "调用 '{name}' 参数类型不匹配：期望 {}，实际 {}",
                                type_name(want),
                                type_name(&at)
                            ),
                        });
                    }
                    self.result.expr_types.insert(addr_of(a), at);
                }
                sig.ret_ty
            }
            Expr::Unary { op, operand, span } => {
                let ot = self.infer_expr(operand, scope)?;
                self.result.expr_types.insert(addr_of(operand), ot.clone());
                match op {
                    UnaryOp::Neg => {
                        if !is_number(&ot) {
                            return Err(SemanticError {
                                span: *span,
                                message: "取负运算的操作数必须是数字".into(),
                            });
                        }
                        ot
                    }
                    UnaryOp::Not => {
                        if !is_bool_like(&ot) {
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
                self.result.expr_types.insert(addr_of(lhs), lt.clone());
                self.result.expr_types.insert(addr_of(rhs), rt.clone());
                // 左右类型必须一致（int 与 float 不隐式转换）
                if !types_compatible(&lt, &rt) {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "二元运算两侧类型不一致：{} 与 {}",
                            type_name(&lt),
                            type_name(&rt)
                        ),
                    });
                }
                match op {
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod => {
                        // 字符串拼接：`+` 且两侧都是 string
                        if *op == BinaryOp::Add
                            && matches!(&lt, TypeSpec::Named(TyKw::Str))
                        {
                            return Ok(TypeSpec::Named(TyKw::Str));
                        }
                        if !is_number(&lt) {
                            return Err(SemanticError {
                                span: *span,
                                message: format!("算术运算符不能用于 {}", type_name(&lt)),
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
                        // 字符串比较：任意比较运算符且两侧都是 string（用 strcmp）
                        if matches!(&lt, TypeSpec::Named(TyKw::Str)) {
                            return Ok(TypeSpec::Named(TyKw::Bool));
                        }
                        // 元组比较本期不支持（逐字段 == 留待后续版本）
                        if matches!(&lt, TypeSpec::Tuple(_)) {
                            return Err(SemanticError {
                                span: *span,
                                message: "元组暂不支持比较运算（逐字段比较留待后续版本）".into(),
                            });
                        }
                        if !is_number(&lt) && !matches!(&lt, TypeSpec::Named(TyKw::Bool)) {
                            return Err(SemanticError {
                                span: *span,
                                message: format!("比较运算符不能用于 {}", type_name(&lt)),
                            });
                        }
                        TypeSpec::Named(TyKw::Bool)
                    }
                    BinaryOp::And | BinaryOp::Or => {
                        if !is_bool_like(&lt) {
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
                self.result.expr_types.insert(addr_of(start), st.clone());
                self.result.expr_types.insert(addr_of(end), et.clone());
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
                self.result.expr_types.insert(addr_of(&cells[0].value), first_ty.clone());
                for cell in &cells[1..] {
                    let ct = self.infer_expr(&cell.value, scope)?;
                    self.result.expr_types.insert(addr_of(&cell.value), ct.clone());
                    if !types_compatible(&first_ty, &ct) {
                        return Err(SemanticError {
                            span: *span,
                            message: format!(
                                "表元素类型不一致：{} 与 {}",
                                type_name(&first_ty),
                                type_name(&ct)
                            ),
                        });
                    }
                }
                // 表类型：元素类型（当前 IR 阶段仅支持数/字符串元素的同构表）
                first_ty
            }
            Expr::Index { base, index, span } => {
                // 下标访问：base 必须是表（元素读取）或字符串（取字符），index 必须是整数
                let base_ty = self.infer_expr(base, scope)?;
                self.result.expr_types.insert(addr_of(base), base_ty.clone());
                let index_ty = self.infer_expr(index, scope)?;
                self.result.expr_types.insert(addr_of(index), index_ty.clone());
                if !index_ty.is_int() {
                    return Err(SemanticError {
                        span: *span,
                        message: format!("下标必须是整数，实际是 {}", type_name(&index_ty)),
                    });
                }
                // 字符串下标：s[i] → 取第 i 个字符（char）
                if matches!(&base_ty, TypeSpec::Named(TyKw::Str)) {
                    return Ok(TypeSpec::Named(TyKw::Char));
                }
                if base_ty != TypeSpec::Named(TyKw::Table) {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "下标访问的对象必须是表或字符串，实际是 {}",
                            type_name(&base_ty)
                        ),
                    });
                }
                // 元素类型：base 是表变量 → 查其布局元数据；是内联表字面量 → 元素同构类型
                match base.as_ref() {
                    Expr::TableLit { .. } => base_ty,
                    Expr::Var(name) => {
                        let key = (self.cur_fn.clone(), name.clone());
                        match self.table_vars.get(&key) {
                            Some(info) => info.elem_ty.clone(),
                            None => {
                                return Err(SemanticError {
                                    span: *span,
                                    message: format!("下标访问的表 '{name}' 缺少布局元数据（内部错误）"),
                                })
                            }
                        }
                    }
                    _ => {
                        return Err(SemanticError {
                            span: *span,
                            message: "下标访问仅支持表变量或表字面量".into(),
                        })
                    }
                }
            }
            Expr::TupleLit { fields, span } => {
                // 元组字面量：逐字段推断类型（C# 风格，元素 ≥1，命名可选）。
                // 空元组 () 在解析层已拒绝，这里仅防御。
                if fields.is_empty() {
                    return Err(SemanticError {
                        span: *span,
                        message: "空元组 () 不支持".into(),
                    });
                }
                let mut tys = Vec::with_capacity(fields.len());
                // 字段名查重：`(x: 1, x: 2)` 报错
                let mut seen: Vec<&str> = Vec::new();
                for (name, e) in fields {
                    let ft = self.infer_expr(e, scope)?;
                    self.result.expr_types.insert(addr_of(e), ft.clone());
                    if let Some(n) = name {
                        if seen.contains(&n.as_str()) {
                            return Err(SemanticError {
                                span: *span,
                                message: format!("元组字段名 '{n}' 重复"),
                            });
                        }
                        seen.push(n);
                    }
                    tys.push(TupleField { name: name.clone(), ty: ft });
                }
                TypeSpec::Tuple(tys)
            }
            Expr::TupleField { base, access, span } => {
                // 字段访问：base 必须是元组；字段名/ItemN/数字 → 下标（越界或名字不存在报错）
                let base_ty = self.infer_expr(base, scope)?;
                self.result.expr_types.insert(addr_of(base), base_ty.clone());
                let TypeSpec::Tuple(fields) = &base_ty else {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "字段访问 '.' 的对象必须是元组，实际是 {}",
                            type_name(&base_ty)
                        ),
                    });
                };
                let idx = tuple_field_index(&base_ty, access).ok_or_else(|| SemanticError {
                    span: *span,
                    message: format!(
                        "元组没有字段 '{access}'（元组字段为 {}）",
                        type_name(&base_ty)
                    ),
                })?;
                fields[idx].ty.clone()
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
        Stmt::Assign(a) => a.span,
        Stmt::Return(r) => r.span,
        Stmt::If(i) => i.span,
        Stmt::While(w) => w.span,
        Stmt::For(f) => f.span,
        Stmt::Switch(s) => s.span,
        Stmt::Import(i) => i.span,
    }
}

/// 从表达式中取 span（含字面量：用占位位置）。
fn expr_span_of(expr: &Expr) -> Span {
    match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::StrLit(_) | Expr::CharLit(_) | Expr::BoolLit(_)
        | Expr::Var(_) => {
            // 字面量无 span，用 (0,0) 占位（语义错误主要针对变量/调用，已有 span）
            Span { line: 0, col: 0 }
        }
        Expr::Call { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Range { span, .. }
        | Expr::TableLit { span, .. }
        | Expr::Index { span, .. }
        | Expr::TupleLit { span, .. }
        | Expr::TupleField { span, .. } => *span,
    }
}

/// 类型是否兼容（当前：必须完全相同；int 与 float 不隐式转换）。
///
/// 元组递归：两个元组长度相同且逐字段类型兼容即兼容（C# 语义：
/// 字段名是编译期标签，`(x: i64, y: i64)` 与 `(i64, i64)` 可互换）。
fn types_compatible(a: &TypeSpec, b: &TypeSpec) -> bool {
    match (a, b) {
        (TypeSpec::Tuple(x), TypeSpec::Tuple(y)) => {
            x.len() == y.len()
                && x.iter().zip(y).all(|(xf, yf)| types_compatible(&xf.ty, &yf.ty))
        }
        _ => a == b,
    }
}

/// 类型是否匹配（含字面量适配）。
///
/// 规则：
/// - 显式类型与推导类型完全相同 → 匹配；
/// - 整数字面量可适配任意整数标注（如 `let x: i32 = 42`）；
/// - 浮点字面量可适配任意浮点标注（如 `let x: f64 = 1.5`）；
/// - 其余情况（变量、运算结果等）不隐式转换。
fn types_match(want: &TypeSpec, got: &TypeSpec, init: Option<&Expr>) -> bool {
    if types_compatible(want, got) {
        return true;
    }
    // 元组：逐字段适配（字段级字面量适配，如 `(i32, i64) = (1, 2)`）
    if let (TypeSpec::Tuple(wt), TypeSpec::Tuple(gt), Some(Expr::TupleLit { fields, .. })) =
        (want, got, init)
        && wt.len() == gt.len()
        && wt.len() == fields.len()
    {
        return wt.iter().zip(gt).zip(fields).all(|((wf, gf), (_, fe))| {
            types_match(&wf.ty, &gf.ty, Some(fe))
        });
    }
    match init {
        Some(Expr::IntLit(_)) => want.is_int() && got.is_int(),
        Some(Expr::FloatLit(_)) => want.is_float() && got.is_float(),
        _ => false,
    }
}

/// 是否为数字类型（整数或浮点）。
fn is_number(t: &TypeSpec) -> bool {
    t.is_number()
}

/// 是否为编译期字面量（switch 的 case 值只允许字面量）。
fn is_const_literal(expr: &Expr) -> bool {
    match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::CharLit(_) | Expr::BoolLit(_) | Expr::StrLit(_) => {
            true
        }
        // 负数字面量：`-1` / `-1.5`
        Expr::Unary { op: UnaryOp::Neg, operand, .. } => {
            matches!(operand.as_ref(), Expr::IntLit(_) | Expr::FloatLit(_))
        }
        _ => false,
    }
}

/// 字面量的去重键（用于检测重复 case）。
fn literal_key(expr: &Expr) -> String {
    match expr {
        Expr::IntLit(v) => format!("i:{v}"),
        Expr::FloatLit(v) => format!("f:{v}"),
        Expr::CharLit(c) => format!("c:{c:?}"),
        Expr::BoolLit(b) => format!("b:{b}"),
        Expr::StrLit(s) => format!("s:{s}"),
        Expr::Unary { op: UnaryOp::Neg, operand, .. } => match operand.as_ref() {
            Expr::IntLit(v) => format!("i:-{v}"),
            Expr::FloatLit(v) => format!("f:-{v}"),
            _ => "?".into(),
        },
        _ => "?".into(),
    }
}

/// 是否为布尔类（if/while 条件）。
fn is_bool_like(t: &TypeSpec) -> bool {
    matches!(t, TypeSpec::Named(TyKw::Bool))
}

/// 解析元组字段访问 `access`，返回字段下标。
///
/// 支持三种形式：
/// - 命名：`.x` / `.q`（按字段名查找）；
/// - 位置：`.Item1`、`.Item2` …（1 起编号）；
/// - 数字：`.0`、`.1` …（0 起编号）。
///
/// 找不到返回 None（由调用方报错）。
fn tuple_field_index(tuple_ty: &TypeSpec, access: &str) -> Option<usize> {
    let TypeSpec::Tuple(fields) = tuple_ty else {
        return None;
    };
    // 数字下标：`.0` 起
    if let Ok(i) = access.parse::<usize>() {
        return (i < fields.len()).then_some(i);
    }
    // ItemN：1 起编号
    if let Some(n) = access.strip_prefix("Item")
        && let Ok(i) = n.parse::<usize>()
    {
        let zero = i.checked_sub(1)?;
        return (zero < fields.len()).then_some(zero);
    }
    // 字段名
    fields.iter().position(|f| f.name.as_deref() == Some(access))
}

/// 类型的可读名称。
fn type_name(t: &TypeSpec) -> &'static str {
    match t {
        TypeSpec::Named(k) => k.as_str(),
        TypeSpec::Tuple(_) => "tuple",
    }
}
