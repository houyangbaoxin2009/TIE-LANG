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
    BinaryOp, ClassDefStmt, ClassField, Expr, FnDefStmt, MethodDefStmt, Program, Stmt, TableId,
    TupleField, TypeSpec, UnaryOp,
};
use super::lexer::{Span, TyKw};
use std::collections::{HashMap, HashSet};
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

/// 语义分析结果：函数签名表 + 函数内表达式类型推断表 + 类信息表。
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
    /// 类信息：类名 → 拍平后的字段/方法表（P8，IR 布局与 mangle 用）
    pub classes: HashMap<String, ClassInfo>,
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

/// 类的方法签名（P8）。
#[derive(Debug, Clone)]
pub struct MethodSig {
    /// 静态方法：不绑定 this，通过 `类名.方法名()` 调用
    pub is_static: bool,
    pub param_tys: Vec<TypeSpec>,
    pub ret_ty: TypeSpec,
}

/// 类的完整信息（P8）：字段与方法均为**继承拍平**后的结果。
///
/// 字段顺序即 LLVM 结构体字段序（父类字段在前，子类字段在后）；
/// `field_index` 是字段名 → GEP 偏移的唯一权威来源（语义校验与 IR 生成共用，
/// 避免两处各自遍历拍平造成错位）。
#[derive(Debug, Clone)]
pub struct ClassInfo {
    /// 直接父类名（`extends Parent`）
    pub parent: Option<String>,
    /// 拍平字段（含继承），顺序即 LLVM 结构体字段序
    pub fields: Vec<ClassField>,
    /// 字段名 → 字段下标（拍平顺序，IR 的 GEP 偏移）
    pub field_index: HashMap<String, usize>,
    /// 拍平方法（子类同名方法遮蔽父类）
    pub methods: HashMap<String, MethodSig>,
    /// 方法名 → 实际定义它的类（mangle 用 `@<定义类>$<方法名>`）
    pub method_owner: HashMap<String, String>,
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

    // 类收集：继承链解析（环检测）+ 字段/方法拍平 + 冲突检查（类名 vs 函数名）
    ctx.collect_classes(program)?;

    // 第二遍：检查函数体
    for stmt in &program.stmts {
        if let Stmt::FnDef(f) = stmt {
            ctx.check_fn(f)?;
        }
    }

    // 第三遍：检查方法体（this 绑定、成员访问、类型检查）
    for stmt in &program.stmts {
        if let Stmt::Class(c) = stmt {
            for m in &c.methods {
                ctx.check_method(m, &c.name)?;
            }
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

    /// 类收集（第一遍的延续）：继承链解析 + 字段/方法拍平 + 冲突检查。
    ///
    /// 顺序保证：父类先于子类拍平（递归），拍平结果存 `result.classes`。
    fn collect_classes(&mut self, program: &Program) -> Result<(), SemanticError> {
        // 第一步：类名登记与冲突检查（类名 vs 函数名、类名 vs 类名）
        for stmt in &program.stmts {
            if let Stmt::Class(c) = stmt {
                if self.result.funcs.contains_key(&c.name) {
                    return Err(SemanticError {
                        span: c.span,
                        message: format!("类名 '{}' 与函数名冲突", c.name),
                    });
                }
                if self.result.classes.contains_key(&c.name) {
                    return Err(SemanticError {
                        span: c.span,
                        message: format!("类 '{}' 重复定义", c.name),
                    });
                }
                self.result.classes.insert(c.name.clone(), ClassInfo {
                    parent: c.parent.clone(),
                    fields: Vec::new(),
                    field_index: HashMap::new(),
                    methods: HashMap::new(),
                    method_owner: HashMap::new(),
                });
            }
        }
        // 第二步：逐个类做继承链拍平（递归解析父类字段/方法）
        // 先构造「类名 → 定义」映射以便查找父类
        let defs: HashMap<String, &ClassDefStmt> = program
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::Class(c) => Some((c.name.clone(), c)),
                _ => None,
            })
            .collect();
        let names: Vec<String> = self.result.classes.keys().cloned().collect();
        for name in names {
            let info = self.flatten_class(&name, &defs, &mut HashSet::new())?;
            // 拍平后的结果替换占位
            self.result.classes.insert(name, info);
        }
        Ok(())
    }

    /// 拍平单个类：递归合并父类字段/方法，自身字段/方法叠加，环检测。
    ///
    /// `chain` 是当前继承链上的类名集合（路径环检测用，非全局访问集合）。
    fn flatten_class(
        &self,
        name: &str,
        defs: &HashMap<String, &ClassDefStmt>,
        chain: &mut HashSet<String>,
    ) -> Result<ClassInfo, SemanticError> {
        // 环检测：`class A extends B` 且 B 又依赖 A → 死循环
        if !chain.insert(name.to_string()) {
            return Err(SemanticError {
                span: defs.get(name).map(|c| c.span).unwrap_or(Span { line: 0, col: 0 }),
                message: format!("类继承形成环（含 '{name}'）"),
            });
        }
        let def = defs.get(name).ok_or_else(|| SemanticError {
            span: Span { line: 0, col: 0 },
            message: format!("内部错误：类 '{name}' 无定义"),
        })?;
        let mut info = ClassInfo {
            parent: def.parent.clone(),
            fields: Vec::new(),
            field_index: HashMap::new(),
            methods: HashMap::new(),
            method_owner: HashMap::new(),
        };
        // 父类字段/方法拍平（递归；父类未定义 → 报错）
        if let Some(p) = &def.parent {
            if !defs.contains_key(p) {
                return Err(SemanticError {
                    span: def.span,
                    message: format!("父类 '{p}' 未定义"),
                });
            }
            let pinfo = self.flatten_class(p, defs, chain)?;
            info.fields = pinfo.fields;
            info.field_index = pinfo.field_index;
            info.methods = pinfo.methods;
            info.method_owner = pinfo.method_owner;
        }
        // 自身字段：字段名跨继承链唯一（布局平铺后重名即歧义）
        for f in &def.fields {
            if info.field_index.contains_key(&f.name) {
                return Err(SemanticError {
                    span: f.span,
                    message: format!(
                        "字段 '{name}.{}' 与继承链中的字段重名（字段名必须跨继承链唯一）",
                        f.name
                    ),
                });
            }
            // 解析字段类型（显式标注优先；否则从默认值字面量推导；都无 → 报错）
            let fty = self.resolve_class_field_ty(f)?;
            let mut cf = f.clone();
            cf.ty = Some(fty);
            let idx = info.fields.len();
            info.fields.push(cf);
            info.field_index.insert(f.name.clone(), idx);
        }
        // 自身方法：同名遮蔽父类（method_owner 记录实际定义类），同类内重名报错
        for m in &def.methods {
            if let Some(owner) = info.method_owner.get(&m.name)
                && owner == name
            {
                return Err(SemanticError {
                    span: m.span,
                    message: format!("方法 '{name}.{}' 重复定义", m.name),
                });
            }
            let sig = MethodSig {
                is_static: m.is_static,
                param_tys: m.params.iter().map(|p| p.ty.clone()).collect(),
                ret_ty: m.ret_ty.clone(),
            };
            info.methods.insert(m.name.clone(), sig);
            info.method_owner.insert(m.name.clone(), name.to_string());
        }
        chain.remove(name);
        Ok(info)
    }

    /// 检查方法体：实例方法先绑定 `this`（当前类类型），静态方法不绑定。
    fn check_method(&mut self, m: &MethodDefStmt, class_name: &str) -> Result<(), SemanticError> {
        self.cur_fn = format!("{class_name}.{}", m.name);
        // 方法体内作用域：this（实例方法）+ 参数
        let mut scope: HashMap<String, TypeSpec> = HashMap::new();
        if !m.is_static {
            scope.insert("this".to_string(), TypeSpec::Class(class_name.to_string()));
        }
        for p in &m.params {
            if scope.insert(p.name.clone(), p.ty.clone()).is_some() {
                return Err(SemanticError {
                    span: p.span,
                    message: format!("参数 '{}' 重复", p.name),
                });
            }
        }
        for stmt in &m.body {
            self.check_stmt(stmt, &mut scope, &m.ret_ty)?;
        }
        Ok(())
    }

    /// 解析类字段的具体类型（P8）。
    ///
    /// 规则：显式标注优先；无标注但有默认值 → 从默认值字面量推导；
    /// 两者皆无 → 报错（IR 无法确定字段类型）。
    fn resolve_class_field_ty(&self, f: &ClassField) -> Result<TypeSpec, SemanticError> {
        // 有显式标注：直接用（默认值类型由后续构造/赋值校验把关）
        if let Some(t) = &f.ty {
            return Ok(t.clone());
        }
        // 无标注：从默认值字面量推导（P8 限字面量，保证类型可静态确定）
        let ty = match &f.init {
            Some(Expr::IntLit(_)) => TypeSpec::Named(TyKw::I64),
            Some(Expr::FloatLit(_)) => TypeSpec::Named(TyKw::F64),
            Some(Expr::BoolLit(_)) => TypeSpec::Named(TyKw::Bool),
            Some(Expr::StrLit(_)) => TypeSpec::Named(TyKw::Str),
            Some(Expr::CharLit(_)) => TypeSpec::Named(TyKw::Char),
            Some(_) => {
                return Err(SemanticError {
                    span: f.span,
                    message: format!(
                        "字段 '{}' 无类型标注，默认值必须是字面量（当前是表达式）",
                        f.name
                    ),
                })
            }
            None => {
                return Err(SemanticError {
                    span: f.span,
                    message: format!("字段 '{}' 必须标注类型或有默认值", f.name),
                })
            }
        };
        Ok(ty)
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
                // 赋值：目标必须已声明；const 不可变；普通赋值类型匹配 / 复合赋值按运算符校验
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
                // const 变量不允许重新赋值（普通赋值与复合赋值一律禁止）
                if self.result.const_vars.contains(&a.target) {
                    return Err(SemanticError {
                        span: a.span,
                        message: format!("不能给 const 变量 '{}' 赋值", a.target),
                    });
                }
                // 复合赋值：按运算符类型规则校验（字符串 += 先放行，其余按运算符要求）
                if let Some(op) = a.op {
                    self.check_compound_assign(&target_ty, op, &value_ty, &a.value, a.span)?;
                } else if !types_match(&target_ty, &value_ty, Some(&a.value)) {
                    // 普通赋值：类型必须兼容（字面量可适配目标类型）
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
            Stmt::Class(_) => {
                // 类定义只允许出现在文件顶层（analyze 第三遍统一检查方法体）
                Err(SemanticError {
                    span: stmt_span(stmt),
                    message: "类定义只能出现在文件顶层".into(),
                })
            }
            Stmt::FieldAssign(fa) => {
                // 字段赋值：base 必须是类实例（变量/this/字段链，可寻址），字段存在，类型匹配
                let base_ty = self.infer_expr(&fa.base, scope)?;
                self.result.expr_types.insert(addr_of(&fa.base), base_ty.clone());
                let TypeSpec::Class(class_name) = &base_ty else {
                    return Err(SemanticError {
                        span: fa.span,
                        message: format!(
                            "字段赋值的对象必须是类实例，实际是 {}",
                            type_name(&base_ty)
                        ),
                    });
                };
                let info = self
                    .result
                    .classes
                    .get(class_name)
                    .cloned()
                    .ok_or_else(|| SemanticError {
                        span: fa.span,
                        message: format!("内部错误：类 '{class_name}' 无信息"),
                    })?;
                let field_ty = info
                    .field_index
                    .get(&fa.field)
                    .map(|&i| info.fields[i].ty.clone().expect("字段类型已在类收集时解析"))
                    .ok_or_else(|| SemanticError {
                        span: fa.span,
                        message: format!("类 '{class_name}' 没有字段 '{}'", fa.field),
                    })?;
                let value_ty = self.infer_expr(&fa.value, scope)?;
                self.result.expr_types.insert(addr_of(&fa.value), value_ty.clone());
                // 复合字段赋值：按运算符类型规则校验（与 Assign 共用辅助函数）
                if let Some(op) = fa.op {
                    self.check_compound_assign(&field_ty, op, &value_ty, &fa.value, fa.span)?;
                } else if !types_match(&field_ty, &value_ty, Some(&fa.value)) {
                    return Err(SemanticError {
                        span: fa.span,
                        message: format!(
                            "字段赋值类型不匹配：'{class_name}.{}' 类型为 {}，表达式为 {}",
                            fa.field,
                            type_name(&field_ty),
                            type_name(&value_ty)
                        ),
                    });
                }
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
                // 构造调用：`Counter(1, 2)` 命中类名 → 按字段逐位置初始化（P8）。
                // 参数个数 ≤ 字段数（缺省用字段默认值/零值）；类型逐个匹配。
                if let Some(info) = self.result.classes.get(name).cloned() {
                    if args.len() > info.fields.len() {
                        return Err(SemanticError {
                            span: *span,
                            message: format!(
                                "构造 '{name}' 最多 {} 个参数（字段数），实际 {} 个",
                                info.fields.len(),
                                args.len()
                            ),
                        });
                    }
                    for (a, f) in args.iter().zip(info.fields.iter()) {
                        let at = self.infer_expr(a, scope)?;
                        let fty = f.ty.clone().expect("字段类型已在类收集时解析");
                        if !types_match(&fty, &at, Some(a)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!(
                                    "构造 '{name}' 参数类型不匹配：字段 '{}' 期望 {}，实际 {}",
                                    f.name,
                                    type_name(&fty),
                                    type_name(&at)
                                ),
                            });
                        }
                        self.result.expr_types.insert(addr_of(a), at);
                    }
                    return Ok(TypeSpec::Class(name.clone()));
                }
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
                // 内置函数 len：单参数，要求字符串或表，返回 i64（字符串长度 / 表元素个数）。
                // M2 扩展：len(表) 返回元素个数（编译期定长，IR 直接输出常量；解释器取运行时长度）。
                if name == "len" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("len() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    // 表字面量参数：直接接受（表，编译期已知长度）
                    if matches!(&args[0], Expr::TableLit { .. }) {
                        return Ok(TypeSpec::Named(TyKw::I64));
                    }
                    // 表变量（scope 类型为 Table）或字符串
                    if matches!(&at, TypeSpec::Named(TyKw::Table))
                        || matches!(&at, TypeSpec::Named(TyKw::Str))
                    {
                        return Ok(TypeSpec::Named(TyKw::I64));
                    }
                    return Err(SemanticError {
                        span: expr_span_of(&args[0]),
                        message: format!("len() 参数必须是字符串或表，实际是 {}", type_name(&at)),
                    });
                }
                // 内置函数 print：同 println（不换行），任意参数，void
                if name == "print" {
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        if matches!(&at, TypeSpec::Tuple(_)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("print 不支持元组参数（类型 {}）", type_name(&at)),
                            });
                        }
                        self.result.expr_types.insert(addr_of(a), at);
                    }
                    return Ok(TypeSpec::Named(TyKw::Void));
                }
                // 内置函数 read_line：零参数，返回 string（REPL 自举：读 stdin 一行）
                if name == "read_line" {
                    if !args.is_empty() {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("read_line() 期望 0 个参数，实际 {} 个", args.len()),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 内置函数 eval：单参数 string，返回 string（REPL 自举：动态求值代码）
                if name == "eval" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("eval() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("eval() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 内置函数 file_read：单字符串参数，返回 string（读取文件内容；失败运行时报错）。
                // M2 标准库 floor：文件读取是 Rust 层唯一实现，其余 std 库用 tie 语言自写。
                if name == "file_read" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("file_read() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("file_read() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 内置函数 file_write / file_append：两个字符串参数，返回 bool（成功与否）。
                // file_write 覆盖写，file_append 追加写。
                if name == "file_write" || name == "file_append" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("{name}() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at.clone());
                        if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("{name}() 参数必须是字符串，实际是 {}", type_name(&at)),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::Bool));
                }
                // 内置函数 file_exists：单字符串参数，返回 bool（文件是否存在）。
                if name == "file_exists" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("file_exists() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("file_exists() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Bool));
                }
                // 内置函数 str_char：字符串 + 整数下标，返回 string（第 i 个 Unicode 码点；越界返回空串）。
                // i 按字符（码点）计数，非字节。
                if name == "str_char" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("str_char() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let st = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), st.clone());
                    if !matches!(&st, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("str_char() 第 1 个参数必须是字符串，实际是 {}", type_name(&st)),
                        });
                    }
                    let it = self.infer_expr(&args[1], scope)?;
                    self.result.expr_types.insert(addr_of(&args[1]), it.clone());
                    if !it.is_int() {
                        return Err(SemanticError {
                            span: expr_span_of(&args[1]),
                            message: format!("str_char() 第 2 个参数必须是整数，实际是 {}", type_name(&it)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 内置函数 to_string：单数字参数（i64/f64），返回 string（数字格式化）。
                // 数字重载：语义层允许任意数字类型（num 类别框），IR 层按实参类型分派 i64/f64。
                if name == "to_string" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("to_string() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !is_number(&at) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("to_string() 参数必须是数字（i64/f64），实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 内置函数 parse_int：字符串参数，返回 i64（非法输入运行时报错）。
                if name == "parse_int" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("parse_int() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("parse_int() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::I64));
                }
                // 内置函数 parse_float：字符串参数，返回 f64（非法输入运行时报错）。
                if name == "parse_float" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("parse_float() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("parse_float() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::F64));
                }
                // 内置函数 exit：整数参数，void（刷新 stdout 后终止进程）。
                if name == "exit" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("exit() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !at.is_int() {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("exit() 参数必须是整数，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Void));
                }
                // 内置函数 time_now：零参数，返回 i64（Unix 纪元秒数）。
                if name == "time_now" {
                    if !args.is_empty() {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("time_now() 期望 0 个参数，实际 {} 个", args.len()),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::I64));
                }
                // 内置函数 rand_range：两个整数参数，返回 i64（[min, max) 内随机整数）。
                if name == "rand_range" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("rand_range() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at.clone());
                        if !at.is_int() {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("rand_range() 参数必须是整数，实际是 {}", type_name(&at)),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::I64));
                }
                // 内置函数 sqrt/sin/cos/tan/exp/log/floor/ceil/round：单数字参数，返回 f64。
                // 数字重载：语义层允许任意数字类型（num 类别框），IR 层按实参类型提升为 double。
                if matches!(
                    name.as_str(),
                    "sqrt" | "sin" | "cos" | "tan" | "exp" | "log" | "floor" | "ceil" | "round"
                ) {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("{name}() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !is_number(&at) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("{name}() 参数必须是数字（i64/f64），实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::F64));
                }
                // 内置函数 pow：两个数字参数，返回 f64（x^y）。
                if name == "pow" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("pow() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at.clone());
                        if !is_number(&at) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("pow() 参数必须是数字（i64/f64），实际是 {}", type_name(&at)),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::F64));
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
                    // M4 自增自减（前缀 ++/-- 与后缀 ++/--）：操作数必须是可写的数字变量/字段
                    UnaryOp::PreInc | UnaryOp::PreDec | UnaryOp::PostInc | UnaryOp::PostDec => {
                        // const 变量：专门报错（与普通赋值的报错风格一致）
                        if let Expr::Var(name) = operand.as_ref() {
                            if self.result.const_vars.contains(name) {
                                return Err(SemanticError {
                                    span: *span,
                                    message: format!("不能对 const 变量 '{name}' 自增/自减"),
                                });
                            }
                        } else if !matches!(operand.as_ref(), Expr::FieldAccess { .. }) {
                            // 其余不可写操作数（字面量/调用结果等）
                            return Err(SemanticError {
                                span: *span,
                                message: "自增/自减的操作数必须是可写数字变量".into(),
                            });
                        }
                        // 类型必须是数字（整数或浮点）
                        if !is_number(&ot) {
                            return Err(SemanticError {
                                span: *span,
                                message: "自增/自减的操作数必须是可写数字变量".into(),
                            });
                        }
                        ot
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
                    // M4 位运算与移位：仅支持整数（浮点不行，is_number 不够精确）
                    BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr => {
                        if !lt.is_int() {
                            return Err(SemanticError {
                                span: *span,
                                message: format!("位运算只支持整数，不能用于 {}", type_name(&lt)),
                            });
                        }
                        lt
                    }
                }
            }
            Expr::Ternary { cond, then_expr, else_expr, span } => {
                // M4 三目：条件必须是 bool，两分支类型必须一致，返回 then 分支的类型
                let ct = self.infer_expr(cond, scope)?;
                self.result.expr_types.insert(addr_of(cond), ct.clone());
                if !is_bool_like(&ct) {
                    return Err(SemanticError {
                        span: *span,
                        message: "三目条件必须是 bool".into(),
                    });
                }
                let then_ty = self.infer_expr(then_expr, scope)?;
                self.result.expr_types.insert(addr_of(then_expr), then_ty.clone());
                let else_ty = self.infer_expr(else_expr, scope)?;
                self.result.expr_types.insert(addr_of(else_expr), else_ty.clone());
                if !types_compatible(&then_ty, &else_ty) {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "三目两分支类型不一致：{} 与 {}",
                            type_name(&then_ty),
                            type_name(&else_ty)
                        ),
                    });
                }
                then_ty
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
                // 记录布局元数据：元素类型 + 长度（len(表) 直接查；表变量声明分支同样插入，幂等）
                let info = TableInfo { elem_ty: first_ty.clone(), len: cells.len() };
                self.result.tables.insert(addr_of(expr), info);
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
            Expr::FieldAccess { base, field, span } => {
                // 字段访问（读）：base 是元组 → 元组字段（命名/ItemN/数字）；
                // base 是类实例 → 类字段（命名）。按 base 的推导类型分发。
                let base_ty = self.infer_expr(base, scope)?;
                self.result.expr_types.insert(addr_of(base), base_ty.clone());
                match &base_ty {
                    TypeSpec::Tuple(fields) => {
                        let idx = tuple_field_index(&base_ty, field).ok_or_else(|| SemanticError {
                            span: *span,
                            message: format!(
                                "元组没有字段 '{field}'（元组字段为 {}）",
                                type_name(&base_ty)
                            ),
                        })?;
                        fields[idx].ty.clone()
                    }
                    TypeSpec::Class(class_name) => {
                        // 寄存器中的类值不可寻址：构造表达式/方法调用结果直接连用字段
                        // 会在 IR 阶段无法取地址，语义层提前报错（Oracle 方案）。
                        if !is_addressable_expr(base) {
                            return Err(SemanticError {
                                span: *span,
                                message: format!(
                                    "类实例 '{class_name}' 的字段访问需要可寻址对象（变量/this/字段链），"
                                ),
                            });
                        }
                        let info = self
                            .result
                            .classes
                            .get(class_name)
                            .cloned()
                            .ok_or_else(|| SemanticError {
                                span: *span,
                                message: format!("内部错误：类 '{class_name}' 无信息"),
                            })?;
                        let idx = info
                            .field_index
                            .get(field)
                            .copied()
                            .ok_or_else(|| SemanticError {
                                span: *span,
                                message: format!("类 '{class_name}' 没有字段 '{field}'"),
                            })?;
                        info.fields[idx].ty.clone().expect("字段类型已在类收集时解析")
                    }
                    _ => {
                        return Err(SemanticError {
                            span: *span,
                            message: format!(
                                "字段访问 '.' 的对象必须是元组或类实例，实际是 {}",
                                type_name(&base_ty)
                            ),
                        })
                    }
                }
            }
            Expr::MethodCall { receiver, method, args, span } => {
                // 方法调用：receiver 是变量/this → 实例方法（receiver 类型必须是类）；
                // receiver 是类名 → 静态方法（无 this）。同一变体两种语义，此处分发。
                // 静态方法：receiver 是 Var 且名字在 classes 表中（未绑定变量）
                if let Expr::Var(rname) = receiver.as_ref()
                    && !scope.contains_key(rname)
                    && self.result.classes.contains_key(rname)
                {
                    let class_name = rname.clone();
                    let info = self.result.classes[&class_name].clone();
                    let sig = info.methods.get(method).cloned().ok_or_else(|| SemanticError {
                        span: *span,
                        message: format!("类 '{class_name}' 没有方法 '{method}'"),
                    })?;
                    if !sig.is_static {
                        return Err(SemanticError {
                            span: *span,
                            message: format!(
                                "实例方法 '{method}' 必须通过实例调用（如 obj.{method}(...)）"
                            ),
                        });
                    }
                    self.check_call_args(method, &sig.param_tys, args, scope, span)?;
                    return Ok(sig.ret_ty);
                }
                // 实例方法：receiver 推断类型必须是类
                let recv_ty = self.infer_expr(receiver, scope)?;
                self.result.expr_types.insert(addr_of(receiver), recv_ty.clone());
                let TypeSpec::Class(class_name) = &recv_ty else {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "方法调用的对象必须是类实例，实际是 {}",
                            type_name(&recv_ty)
                        ),
                    });
                };
                // 寄存器中的类值不可寻址：构造表达式/方法调用结果直接调用方法
                // 会在 IR 阶段无法取 this 地址，语义层提前报错（Oracle 方案）。
                if !is_addressable_expr(receiver) {
                    return Err(SemanticError {
                        span: *span,
                        // 寄存器中的类值不可寻址：无法取 this 地址
                        message: "方法调用的对象需要可寻址的类实例（变量/this/字段链）".to_string(),
                    });
                }
                let info = self
                    .result
                    .classes
                    .get(class_name)
                    .cloned()
                    .ok_or_else(|| SemanticError {
                        span: *span,
                        message: format!("内部错误：类 '{class_name}' 无信息"),
                    })?;
                let sig = info.methods.get(method).cloned().ok_or_else(|| SemanticError {
                    span: *span,
                    message: format!("类 '{class_name}' 没有方法 '{method}'"),
                })?;
                if sig.is_static {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "静态方法 '{method}' 必须通过类名调用（如 {class_name}.{method}(...)）"
                        ),
                    });
                }
                self.check_call_args(method, &sig.param_tys, args, scope, span)?;
                sig.ret_ty
            }
        };
        Ok(ty)
    }

    /// 校验方法调用的实参（个数 + 逐个类型匹配）。
    fn check_call_args(
        &mut self,
        method: &str,
        param_tys: &[TypeSpec],
        args: &[Expr],
        scope: &HashMap<String, TypeSpec>,
        span: &Span,
    ) -> Result<(), SemanticError> {
        if param_tys.len() != args.len() {
            return Err(SemanticError {
                span: *span,
                message: format!(
                    "方法 '{method}' 期望 {} 个参数，实际 {} 个",
                    param_tys.len(),
                    args.len()
                ),
            });
        }
        for (a, want) in args.iter().zip(param_tys.iter()) {
            let at = self.infer_expr(a, scope)?;
            if !types_match(want, &at, Some(a)) {
                return Err(SemanticError {
                    span: expr_span_of(a),
                    message: format!(
                        "调用 '{method}' 参数类型不匹配：期望 {}，实际 {}",
                        type_name(want),
                        type_name(&at)
                    ),
                });
            }
            self.result.expr_types.insert(addr_of(a), at);
        }
        Ok(())
    }

    /// 复合赋值类型校验（M4）：`x op= v` 中 op 对应的二元运算对 target/value 的类型要求。
    ///
    /// 规则（与 infer_expr 的 Binary 分支类型规则对齐）：
    /// - `+=`：数字相加，或字符串拼接复合（target 是 string 且 value 也是 string，先放行）；
    /// - `-=` / `*=` / `/=`：两侧数字且兼容；
    /// - `%=`：仅整数（取模只支持整数）；
    /// - 位运算复合（`&=` `|=` `^=` `<<=` `>>=`）：仅整数；
    /// - 比较/逻辑运算符不能用于复合赋值。
    ///
    /// 值表达式用 types_match（含整数字面量适配目标类型，与 `x += 1` 中 1 适配 x 的类型一致）。
    fn check_compound_assign(
        &self,
        target_ty: &TypeSpec,
        op: BinaryOp,
        value_ty: &TypeSpec,
        value: &Expr,
        span: Span,
    ) -> Result<(), SemanticError> {
        // 值必须与目标类型匹配（含字面量适配）；字符串拼接的 Add 由上面的分支放行后同样要校验
        let ty_ok = || types_match(target_ty, value_ty, Some(value));
        match op {
            // 字符串拼接复合：`s += "a"`（target 是 string 且 value 也是 string）
            BinaryOp::Add if matches!(target_ty, TypeSpec::Named(TyKw::Str)) => {
                if !ty_ok() {
                    return Err(SemanticError {
                        span,
                        message: format!(
                            "复合赋值类型不匹配：目标类型 {} 与表达式 {}",
                            type_name(target_ty),
                            type_name(value_ty)
                        ),
                    });
                }
                Ok(())
            }
            // 算术复合：`+=`（数字）`-=` `*=` `/=`（数字且兼容）
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                if !target_ty.is_number() {
                    return Err(SemanticError {
                        span,
                        message: format!("复合赋值运算符不能用于 {}", type_name(target_ty)),
                    });
                }
                if !ty_ok() {
                    return Err(SemanticError {
                        span,
                        message: format!(
                            "复合赋值类型不匹配：目标类型 {} 与表达式 {}",
                            type_name(target_ty),
                            type_name(value_ty)
                        ),
                    });
                }
                Ok(())
            }
            // 取模复合：`%=` 仅整数
            BinaryOp::Mod => {
                if !target_ty.is_int() {
                    return Err(SemanticError {
                        span,
                        message: format!("复合赋值取模只支持整数，目标类型是 {}", type_name(target_ty)),
                    });
                }
                if !ty_ok() {
                    return Err(SemanticError {
                        span,
                        message: format!(
                            "复合赋值类型不匹配：目标类型 {} 与表达式 {}",
                            type_name(target_ty),
                            type_name(value_ty)
                        ),
                    });
                }
                Ok(())
            }
            // 位运算复合：`&=` `|=` `^=` `<<=` `>>=` 仅整数
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr => {
                if !target_ty.is_int() {
                    return Err(SemanticError {
                        span,
                        message: format!("复合赋值位运算只支持整数，目标类型是 {}", type_name(target_ty)),
                    });
                }
                if !ty_ok() {
                    return Err(SemanticError {
                        span,
                        message: format!(
                            "复合赋值类型不匹配：目标类型 {} 与表达式 {}",
                            type_name(target_ty),
                            type_name(value_ty)
                        ),
                    });
                }
                Ok(())
            }
            // 比较/逻辑运算符不能用于复合赋值
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge
            | BinaryOp::And
            | BinaryOp::Or => Err(SemanticError {
                span,
                message: "无效的复合赋值运算符".into(),
            }),
        }
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
        Stmt::Class(c) => c.span,
        Stmt::FieldAssign(f) => f.span,
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
        | Expr::Ternary { span, .. }
        | Expr::Range { span, .. }
        | Expr::TableLit { span, .. }
        | Expr::Index { span, .. }
        | Expr::TupleLit { span, .. }
        | Expr::FieldAccess { span, .. }
        | Expr::MethodCall { span, .. } => *span,
    }
}

/// 类实例表达式是否可寻址（P8）。
///
/// 可寻址：变量（含 this）或 FieldAccess 链（`obj.a.b` 的 GEP 链天然有内存地址）；
/// 不可寻址：寄存器中的类值（构造表达式/方法调用结果直接连用）——IR 层无法取地址，
/// 必须先在语义层报错（Oracle 方案：寄存器中的类值不可寻址）。
fn is_addressable_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Var(_) => true,
        Expr::FieldAccess { base, .. } => is_addressable_expr(base),
        _ => false,
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
        TypeSpec::Class(name) => Box::leak(name.clone().into_boxed_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse_program;

    /// 完整前端管道：词法 → 语法 → 语义。
    fn analyze_src(src: &str) -> Result<SemanticResult, SemanticError> {
        let tokens = tokenize(src).expect("词法分析失败");
        let program = parse_program(&tokens).expect("语法分析失败");
        analyze(&program)
    }

    /// 断言语义分析报错，且错误消息包含关键字。
    fn expect_err(src: &str, keyword: &str) {
        let err = analyze_src(src).expect_err("应当报语义错误");
        assert!(
            err.message.contains(keyword),
            "错误消息 '{}' 应包含关键字 '{}'",
            err.message,
            keyword
        );
    }

    #[test]
    fn repl内置函数通过() {
        // read_line / eval / print：REPL 自举新增的内置函数，语义检查应通过
        let sem = analyze_src(
            r#"
            func main() {
                var name = read_line()
                println(name)
                var r = eval("1 + 2")
                print(r)
            }
            "#,
        )
        .expect("应当通过语义检查");
        // read_line 返回 string、eval 返回 string（记录在调用表达式上）
        assert!(sem
            .expr_types
            .values()
            .any(|t| matches!(t, TypeSpec::Named(TyKw::Str))));
    }

    #[test]
    fn repl内置函数参数错误报错() {
        // read_line 不接受参数
        expect_err(
            r#"
            func main() {
                var x = read_line("hi")
            }
            "#,
            "read_line() 期望 0 个参数",
        );
        // eval 参数必须是字符串
        expect_err(
            r#"
            func main() {
                var x = eval(42)
            }
            "#,
            "eval() 参数必须是字符串",
        );
        // eval 必须恰好 1 个参数
        expect_err(
            r#"
            func main() {
                var x = eval()
            }
            "#,
            "eval() 期望 1 个参数",
        );
    }

    #[test]
    fn 函数签名收集() {
        let sem = analyze_src(
            r#"
            func add(a: i64, b: i64) -> i64 {
                return a + b
            }
            func greet(name: string) {
                println(name)
            }
            func main() {
                const MAX = 10
                println(MAX)
            }
            "#,
        )
        .expect("应当通过语义检查");
        // funcs 表：函数名齐全
        assert!(sem.funcs.contains_key("add"));
        assert!(sem.funcs.contains_key("greet"));
        assert!(sem.funcs.contains_key("main"));
        // 参数类型与返回类型逐项核对
        assert_eq!(
            sem.funcs["add"].param_tys,
            vec![TypeSpec::Named(TyKw::I64), TypeSpec::Named(TyKw::I64)]
        );
        assert_eq!(sem.funcs["add"].ret_ty, TypeSpec::Named(TyKw::I64));
        assert_eq!(sem.funcs["greet"].param_tys, vec![TypeSpec::Named(TyKw::Str)]);
        // 省略 `-> Ty` 的返回类型默认为 void
        assert_eq!(sem.funcs["greet"].ret_ty, TypeSpec::Named(TyKw::Void));
        // const 变量登记进 const_vars
        assert!(sem.const_vars.contains("MAX"));
    }

    #[test]
    fn 函数重复定义报错() {
        expect_err(
            r#"
            func f() {}
            func f() {}
            func main() {
                println(1)
            }
            "#,
            "重复定义",
        );
    }

    #[test]
    fn 使用未声明变量报错() {
        expect_err(
            r#"
            func main() {
                println(x)
            }
            "#,
            "未声明的变量",
        );
    }

    #[test]
    fn const变量赋值报错() {
        expect_err(
            r#"
            func main() {
                const x = 1
                x = 2
            }
            "#,
            "不能给 const 变量",
        );
    }

    #[test]
    fn 变量类型不匹配报错() {
        // i64 变量赋 string 值：类型不匹配
        expect_err(
            r#"
            func main() {
                var x: i64 = "hello"
            }
            "#,
            "类型不匹配",
        );
    }

    #[test]
    fn return类型与函数签名不一致报错() {
        expect_err(
            r#"
            func f() -> i64 {
                return "hello"
            }
            func main() {
                println(1)
            }
            "#,
            "return 类型不匹配",
        );
    }

    #[test]
    fn return类型与签名一致通过() {
        let sem = analyze_src(
            r#"
            func add(a: i64, b: i64) -> i64 {
                return a + b
            }
            func main() {
                println(add(1, 2))
            }
            "#,
        )
        .expect("应当通过语义检查");
        assert_eq!(sem.funcs["add"].ret_ty, TypeSpec::Named(TyKw::I64));
    }

    #[test]
    fn 类名与函数名冲突报错() {
        expect_err(
            r#"
            func Point() -> i64 {
                return 1
            }
            class Point {
                var x: i64
            }
            func main() {
                println(1)
            }
            "#,
            "与函数名冲突",
        );
    }

    #[test]
    fn 继承环检测报错() {
        // A extends B，B extends A → 死循环，必须报错
        expect_err(
            r#"
            class A extends B {
                var a: i64
            }
            class B extends A {
                var b: i64
            }
            func main() {
                println(1)
            }
            "#,
            "类继承形成环",
        );
    }

    #[test]
    fn 子类遮蔽父类方法与字段拍平() {
        let sem = analyze_src(
            r#"
            class Animal {
                var name: string
                var age: i64
                method speak() -> string {
                    return this.name + " makes a sound"
                }
            }
            class Dog extends Animal {
                var breed: string
                method speak() -> string {
                    return this.name + " barks"
                }
                method info() -> string {
                    return "I am a " + this.breed
                }
            }
            func main() {
                var d = Dog("Rex", 3, "Golden")
                println(d.speak())
            }
            "#,
        )
        .expect("应当通过语义检查");
        // 字段拍平：父类字段在前，子类字段在后（顺序即 LLVM 结构体字段序）
        let dog = &sem.classes["Dog"];
        assert_eq!(dog.fields.len(), 3);
        assert_eq!(dog.field_index["name"], 0);
        assert_eq!(dog.field_index["age"], 1);
        assert_eq!(dog.field_index["breed"], 2);
        // 方法拍平：子类遮蔽父类同名方法，method_owner 记录实际定义类
        assert!(dog.methods.contains_key("speak"));
        assert!(dog.methods.contains_key("info"));
        assert_eq!(dog.method_owner["speak"], "Dog");
        assert_eq!(dog.method_owner["info"], "Dog");
        // 父类自身的方法归属
        assert_eq!(sem.classes["Animal"].method_owner["speak"], "Animal");
    }

    #[test]
    fn 方法重复定义报错() {
        expect_err(
            r#"
            class A {
                method f() {}
                method f() {}
            }
            func main() {
                println(1)
            }
            "#,
            "重复定义",
        );
    }

    #[test]
    fn 方法内this使用正确() {
        let sem = analyze_src(
            r#"
            class Counter {
                var count: i64
                method inc() {
                    this.count = this.count + 1
                }
                method get() -> i64 {
                    return this.count
                }
            }
            func main() {
                var c = Counter(0)
                c.inc()
                println(c.get())
            }
            "#,
        )
        .expect("应当通过语义检查");
        // 方法签名收集进 classes 表
        let counter = &sem.classes["Counter"];
        assert!(counter.methods.contains_key("inc"));
        assert!(counter.methods.contains_key("get"));
        assert_eq!(counter.methods["get"].ret_ty, TypeSpec::Named(TyKw::I64));
        // 实例方法不标记静态
        assert!(!counter.methods["inc"].is_static);
    }

    #[test]
    fn 静态方法内使用this报错() {
        // 静态方法不绑定 this：体内引用 this 视为未声明变量
        expect_err(
            r#"
            class Counter {
                var count: i64
                static method bad() {
                    println(this.count)
                }
            }
            func main() {
                println(1)
            }
            "#,
            "未声明的变量",
        );
    }

    #[test]
    fn 表字面量元数据记录元素类型与长度() {
        let sem = analyze_src(
            r#"
            func main() {
                var arr: table = [10, 20, 30]
                println(arr[1])
            }
            "#,
        )
        .expect("应当通过语义检查");
        // tables 元数据：元素类型 i64、长度 3（供 IR 布局）
        assert_eq!(sem.tables.len(), 1);
        assert!(sem
            .tables
            .values()
            .any(|t| t.elem_ty == TypeSpec::Named(TyKw::I64) && t.len == 3));
    }

    #[test]
    fn 表元素类型不一致报错() {
        // 表是同构容器：元素类型必须全部一致
        expect_err(
            r#"
            func main() {
                var arr: table = [1, "a"]
            }
            "#,
            "表元素类型不一致",
        );
    }

    #[test]
    fn 宽类型num推导出具体类型写入expr_types() {
        let sem = analyze_src(
            r#"
            func main() {
                var a: num = 42
                var b: i64 = a
            }
            "#,
        )
        .expect("应当通过语义检查");
        // 宽类型通过后 scope 存具体推导类型（i64），
        // 后续 `var b: i64 = a` 类型匹配即证明推导生效
        // 且 expr_types 记录了字面量 42 的具体类型
        assert!(sem.expr_types.values().any(|t| *t == TypeSpec::Named(TyKw::I64)));
    }

    #[test]
    fn 宽类型num拒绝字符串初始化() {
        expect_err(
            r#"
            func main() {
                var a: num = "hello"
            }
            "#,
            "不匹配",
        );
    }

    #[test]
    fn 宽类型text推导出字符串类型() {
        let sem = analyze_src(
            r#"
            func main() {
                var s: text = "hello"
                var t: string = s
            }
            "#,
        )
        .expect("应当通过语义检查");
        // text 类别框接受字符串，scope 存具体类型 string
        assert!(sem.expr_types.values().any(|t| *t == TypeSpec::Named(TyKw::Str)));
    }

    #[test]
    fn 元组解构与命名字段访问合法() {
        let sem = analyze_src(
            r#"
            func divmod(a: i64, b: i64) -> (q: i64, r: i64) {
                return (a / b, a % b)
            }
            func main() {
                var (q, r) = divmod(17, 5)
                println(q)
                println(r)
                var p = (x: 3, y: 4)
                println(p.x)
            }
            "#,
        )
        .expect("应当通过语义检查");
        // 函数签名含元组返回类型（命名元组，两字段）
        assert!(matches!(
            &sem.funcs["divmod"].ret_ty,
            TypeSpec::Tuple(fields) if fields.len() == 2
        ));
        // 解构逐字段声明 + 命名访问均通过类型检查
        assert!(sem
            .expr_types
            .values()
            .any(|t| matches!(t, TypeSpec::Tuple(_))));
    }

    #[test]
    fn 元组访问不存在的字段报错() {
        expect_err(
            r#"
            func main() {
                var p = (x: 3, y: 4)
                println(p.z)
            }
            "#,
            "元组没有字段",
        );
    }

    #[test]
    fn 函数调用参数个数不匹配报错() {
        expect_err(
            r#"
            func f(a: i64) {
                println(a)
            }
            func main() {
                f(1, 2)
            }
            "#,
            "期望 1 个参数",
        );
    }

    #[test]
    fn 调用未定义函数报错() {
        expect_err(
            r#"
            func main() {
                foo()
            }
            "#,
            "未定义的函数",
        );
    }

    #[test]
    fn 赋值目标未声明报错() {
        expect_err(
            r#"
            func main() {
                x = 1
            }
            "#,
            "赋值目标",
        );
    }

    #[test]
    fn if条件必须是bool报错() {
        expect_err(
            r#"
            func main() {
                if 1 {
                    println(1)
                }
            }
            "#,
            "if 条件必须是 bool",
        );
    }

    #[test]
    fn 静态方法通过实例调用报错() {
        expect_err(
            r#"
            class Counter {
                var count: i64
                static method make() -> i64 {
                    return 1
                }
            }
            func main() {
                var c = Counter(0)
                println(c.make())
            }
            "#,
            "必须通过类名调用",
        );
    }

    // ---------- M4 运算符扩展 ----------

    #[test]
    fn 位运算类型检查() {
        // i64 位运算/移位合法（& | ^ << >>）
        let sem = analyze_src(
            r#"
            func main() {
                var x: i64 = 1
                var y: i64 = x & 3
                var z: i64 = x | 2 ^ 1
                var w: i64 = x << 2 >> 1
            }
            "#,
        )
        .expect("整数位运算应通过语义检查");
        assert!(sem.expr_types.values().any(|t| *t == TypeSpec::Named(TyKw::I64)));
        // 浮点位运算报错（位运算只支持整数）——两侧同为 f64 才能命中位运算整数校验
        expect_err(
            r#"
            func main() {
                var f: f64 = 1.0
                var g: f64 = 2.0
                var y = f & g
            }
            "#,
            "位运算只支持整数",
        );
    }

    #[test]
    fn 复合赋值类型检查() {
        // i64 全部复合赋值运算符通过
        analyze_src(
            r#"
            func main() {
                var x: i64 = 5
                x += 1
                x -= 2
                x *= 3
                x /= 4
                x %= 2
                x &= 1
                x |= 2
                x ^= 3
                x <<= 1
                x >>= 2
            }
            "#,
        )
        .expect("i64 复合赋值应通过");
        // 字符串拼接复合 `s += "a"` 通过
        analyze_src(
            r#"
            func main() {
                var s: string = "hello"
                s += " world"
            }
            "#,
        )
        .expect("字符串 += 应通过");
        // 取模复合值类型不匹配（`x %= 1.5`：x 是 i64，1.5 是 f64）报错
        expect_err(
            r#"
            func main() {
                var x: i64 = 5
                x %= 1.5
            }
            "#,
            "复合赋值类型不匹配",
        );
        // 字符串做减法复合报错（复合赋值运算符不能用于 string）
        expect_err(
            r#"
            func main() {
                var s: string = "a"
                s -= "b"
            }
            "#,
            "复合赋值运算符不能用于",
        );
    }

    #[test]
    fn 三目类型检查() {
        // 两分支同类型（i64）通过
        analyze_src(
            r#"
            func main() {
                var a: i64 = 1
                var b: i64 = a > 0 ? 1 : -1
            }
            "#,
        )
        .expect("三目两分支同类型应通过");
        // 两分支类型不一致报错
        expect_err(
            r#"
            func main() {
                var a: i64 = 1
                var b = a > 0 ? 1 : "x"
            }
            "#,
            "三目两分支类型不一致",
        );
        // 条件非 bool 报错
        expect_err(
            r#"
            func main() {
                var b = 1 ? 2 : 3
            }
            "#,
            "三目条件必须是 bool",
        );
    }

    #[test]
    fn 自增自减类型检查() {
        // 数字变量自增自减通过（前缀与后缀、整数与浮点）
        analyze_src(
            r#"
            func main() {
                var x: i64 = 1
                x++
                ++x
                x--
                --x
                var f: f64 = 1.0
                f++
            }
            "#,
        )
        .expect("数字变量自增自减应通过");
        // 对 const 变量自增报错
        expect_err(
            r#"
            func main() {
                const x = 1
                x++
            }
            "#,
            "不能对 const 变量",
        );
        // 对未声明变量自增报错
        expect_err(
            r#"
            func main() {
                y++
            }
            "#,
            "未声明的变量",
        );
        // 对字面量（不可写）自增报错
        expect_err(
            r#"
            func main() {
                1++
            }
            "#,
            "自增/自减的操作数必须是可写数字变量",
        );
    }
}
