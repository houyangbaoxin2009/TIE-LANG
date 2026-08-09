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
    BinaryOp, ClassField, Expr, FnDefStmt, Program, Stmt, StructDefStmt, TableId,
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
    /// 表变量元数据：函数名 + 变量名 → 元素类型与动态标志（IR 生成动态表操作用）。
    /// 与 Analyzer 内部的 table_vars 同步（IR 层只读 SemanticResult，不持有 Analyzer）。
    pub table_vars: HashMap<(String, String), TableInfo>,
    /// 函数返回的动态表元素类型：函数名 → 元素类型（None = 未知/冲突，调用方不可
    /// 对返回表做 for/下标/table_at）。无泛型，靠分析函数体内 return 语句推断。
    pub table_ret_elems: HashMap<String, Option<TypeSpec>>,
    /// 类信息：类名 → 拍平后的字段/方法表（P8，IR 布局与 mangle 用）
    pub classes: HashMap<String, ClassInfo>,
    /// 命名空间函数全名：FnDefStmt 地址 → 全名（如 "tcmsg::error::no_file"）。
    /// 仅命名空间内的函数需要（顶层函数全名即裸名）。IR 层生成 LLVM 符号用。
    pub fn_full_names: HashMap<usize, String>,
    /// 命名空间调用解析：调用表达式地址 → 解析后的全名（如 MethodCall 的 receiver
    /// 是 Path 时拼接 "tcmsg::error::no_file"）。IR 层与解释层据此生成调用目标。
    /// 键与 expr_types 一致（AST 节点地址），调用方用 addr_of(expr) 查询。
    pub resolved_calls: HashMap<usize, String>,
    /// 顶层全局持久变量（M4）：变量名 → 类型（跨函数共享；限标量类型 + 字面量初始化）。
    /// 函数体内 Var 解析/Assign 目标优先查函数作用域，未命中再查全局表。
    pub globals: HashMap<String, TypeSpec>,
    /// 顶层全局不可变变量名（const 声明；赋值时校验，与函数内 const_vars 并列）。
    pub const_globals: std::collections::HashSet<String>,
}

/// 表（table）的布局信息：元素类型、元素个数与是否动态。
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// 元素类型（同构容器）
    pub elem_ty: TypeSpec,
    /// 元素个数（编译期已知，定长；动态表为 0，运行时以 len 为准）
    pub len: usize,
    /// 是否动态表（table_new_* 创建，运行时 {ptr,len,cap} 结构；false = 定长字面量）
    pub dynamic: bool,
}

/// 函数签名。
#[derive(Debug, Clone)]
pub struct FuncSig {
    pub param_tys: Vec<TypeSpec>,
    /// 参数默认值（可选参数）：与 param_tys 等长对齐，None = 必选参数。
    /// 调用点省略实参时按此补齐（LLVM 函数签名不变，缺省实参在调用点生成）。
    pub param_defaults: Vec<Option<Expr>>,
    pub ret_ty: TypeSpec,
    /// 是否公有（M2.1.7 单文件命名空间）：命名空间内函数默认私有（仅同命名
    /// 空间可见，`pub func` 显式导出）；顶层函数恒为 true。
    pub is_pub: bool,
}

/// struct 的完整信息（M2.1.8）：字段为**继承拍平**后的结果。
///
/// 字段顺序即 LLVM 结构体字段序（父 struct 字段在前，子 struct 字段在后）；
/// `field_index` 是字段名 → GEP 偏移的唯一权威来源（语义校验与 IR 生成共用，
/// 避免两处各自遍历拍平造成错位）。逻辑（方法）不在 struct 内——由绑定
/// struct 名的命名空间函数定义（`namespace Point { pub func dist(p: Point) }`），
/// `p.dist()` 调用由语义层转发到 `Point::dist(p)`。
#[derive(Debug, Clone)]
pub struct ClassInfo {
    /// 直接父 struct 名（`extends Parent`）
    pub parent: Option<String>,
    /// 拍平字段（含继承），顺序即 LLVM 结构体字段序
    pub fields: Vec<ClassField>,
    /// 字段名 → 字段下标（拍平顺序，IR 的 GEP 偏移）
    pub field_index: HashMap<String, usize>,
}

/// 语义分析入口。
pub fn analyze(program: &Program) -> Result<SemanticResult, SemanticError> {
    let mut ctx = Analyzer {
        result: SemanticResult::default(),
        table_vars: HashMap::new(),
        cur_fn: String::new(),
        ns_stack: Vec::new(),
        import_views: Vec::new(),
        using_prefixes: Vec::new(),
        loop_labels: Vec::new(),
    };

    // 第零遍：收集顶层 import / using 语句，构建导入视图（M2.1.7）。
    // import 语句由 imports.rs 展开时保留（携带被导入文件的命名空间路径）；
    // 语义层据此做：别名唯一入口映射、using 目标校验、裸调用补全。
    ctx.collect_imports_using(program)?;

    // 第一遍：收集所有函数签名（允许前向引用）。
    // 顶层函数以裸名注册；命名空间内函数以全名（"tcmsg::error::no_file"）注册，
    // 并记录 FnDefStmt 地址 → 全名的映射（IR 层生成 LLVM 符号用）。
    for stmt in &program.stmts {
        match stmt {
            Stmt::FnDef(f) => {
                let sig = FuncSig {
                    param_tys: f.params.iter().map(|p| p.ty.clone()).collect(),
                    param_defaults: f.params.iter().map(|p| p.default.clone()).collect(),
                    ret_ty: f.ret_ty.clone(),
                    // 顶层函数恒公有（与现状兼容）
                    is_pub: true,
                };
                if ctx.result.funcs.insert(f.name.clone(), sig).is_some() {
                    return Err(SemanticError {
                        span: f.span,
                        message: format!("函数 '{}' 重复定义", f.name),
                    });
                }
            }
            Stmt::Namespace(ns) => {
                // 命名空间体内函数：递归注册全名（当前命名空间路径 + 函数名）
                ctx.collect_ns_funcs(&ns.body, &ns.path)?;
            }
            // 顶层全局持久变量（M4）：收集类型并校验（显式标量类型 + 字面量初始化 +
            // 命名不冲突）。函数体内 Var 解析/Assign 未命中作用域时查全局表。
            // 无显式类型标注（var x = 1）→ 报错（IR 全局需要静态类型布局）。
            Stmt::VarDecl(v) => {
                let Some(ty) = v.ty.clone() else {
                    return Err(SemanticError {
                        span: v.span,
                        message: format!("全局变量 '{}' 必须显式标注类型（如 var x: i64）", v.name),
                    });
                };
                // 全局变量限标量类型（i64/f64/bool/char/string）——IR 需要静态初始化布局
                let is_scalar = matches!(
                    &ty,
                    TypeSpec::Named(
                        TyKw::I8 | TyKw::I16 | TyKw::I32 | TyKw::I64
                            | TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64
                            | TyKw::F32 | TyKw::F64 | TyKw::Bool | TyKw::Char | TyKw::Str
                    )
                );
                if !is_scalar {
                    return Err(SemanticError {
                        span: v.span,
                        message: format!(
                            "全局变量 '{}' 必须是标量类型（i8..u64/f32/f64/bool/char/string），实际是 {}",
                            v.name,
                            type_name(&ty)
                        ),
                    });
                }
                // 初始化必须是编译期字面量（与字段默认值同规则，IR 静态初始化）
                if !is_const_literal(&v.init) {
                    return Err(SemanticError {
                        span: v.span,
                        message: format!(
                            "全局变量 '{}' 的初始化必须是字面量（数/布尔/字符/字符串）",
                            v.name
                        ),
                    });
                }
                // 命名冲突：函数名 / 全局变量重名
                if ctx.result.funcs.contains_key(&v.name) {
                    return Err(SemanticError {
                        span: v.span,
                        message: format!("全局变量 '{}' 与函数名冲突", v.name),
                    });
                }
                if ctx.result.globals.contains_key(&v.name) {
                    return Err(SemanticError {
                        span: v.span,
                        message: format!("全局变量 '{}' 重复定义", v.name),
                    });
                }
                // 初始化类型与标注类型匹配（字面量适配）
                let init_ty = ctx.infer_expr(&v.init, &HashMap::new())?;
                if !types_match(&ty, &init_ty, Some(&v.init)) {
                    return Err(SemanticError {
                        span: v.span,
                        message: format!(
                            "全局变量 '{}' 初始化类型不匹配：期望 {}，实际 {}",
                            v.name,
                            type_name(&ty),
                            type_name(&init_ty)
                        ),
                    });
                }
                ctx.result.globals.insert(v.name.clone(), ty);
                if v.is_const {
                    ctx.result.const_globals.insert(v.name.clone());
                }
            }
            _ => {}
        }
    }

    // 类收集：继承链解析（环检测）+ 字段/方法拍平 + 冲突检查（类名 vs 函数名）
    ctx.collect_structs(program)?;

    // 内置 list_dir：返回「字符串动态表」（文件名集合）。预登记元素类型，使
    // `var t = list_dir(p)` / `for x in list_dir(p)` / `table_at(t, i)` 的
    // 元素类型静态可知（与 table_new_string 同布局；IR 层按 string 桥访问）。
    ctx.result
        .table_ret_elems
        .insert("list_dir".to_string(), Some(TypeSpec::Named(TyKw::Str)));
    // 内置 walk_dir（M4 补齐）：返回「字符串动态表」（目录下全部文件相对路径）。
    // 与 list_dir 同布局（string 元素），使 `for x in walk_dir(p)` 元素类型静态可知。
    ctx.result
        .table_ret_elems
        .insert("walk_dir".to_string(), Some(TypeSpec::Named(TyKw::Str)));
    // 内置 byte_read / byte_concat（D7）：返回「i64 字节表」（元素 0..255）。
    // 与 list_dir 同布局但元素类型 i64，使 `var b = byte_read(p)` 下标读取得 i64。
    ctx.result
        .table_ret_elems
        .insert("byte_read".to_string(), Some(TypeSpec::Named(TyKw::I64)));
    ctx.result
        .table_ret_elems
        .insert("byte_concat".to_string(), Some(TypeSpec::Named(TyKw::I64)));
    // 内置 regex_find_all（P1）：返回「字符串动态表」（全部匹配片段）。与 list_dir
    // 同布局（string 元素），使 `for x in regex_find_all(s, p)` 元素类型静态可知。
    ctx.result
        .table_ret_elems
        .insert("regex_find_all".to_string(), Some(TypeSpec::Named(TyKw::Str)));

    // 表返回预扫描（fixpoint）：收集「返回动态表」的函数及其元素类型，支持前向引用。
    // 一个函数返回动态表，当且仅当其 return 表达式是 table_new_* 调用、调用另一个
    // 已知返回动态表的函数，或返回本函数内声明的动态表变量。反复扫描直到不再新增。
    loop {
        let mut changed = false;
        // 递归扫描顶层与命名空间体内的函数（返回表 fixpoint）。
        // 命名空间函数以全名登记（ns::name），与调用解析一致。
        fn scan_tables(
            ctx: &mut Analyzer,
            stmts: &[Stmt],
            ns_prefix: &[String],
        ) -> bool {
            let mut changed = false;
            for stmt in stmts {
                match stmt {
                    Stmt::FnDef(f) => {
                        let full = if ns_prefix.is_empty() {
                            f.name.clone()
                        } else {
                            let mut p = ns_prefix.to_vec();
                            p.push(f.name.clone());
                            p.join("::")
                        };
                        if ctx.result.table_ret_elems.contains_key(&full) {
                            continue;
                        }
                        let mut local: HashMap<String, TypeSpec> = HashMap::new();
                        ctx.collect_local_dyn_tables(&f.body, &mut local);
                        if let Some(te) = ctx.scan_return_table_elem(&f.body, &local) {
                            ctx.result.table_ret_elems.insert(full, Some(te));
                            changed = true;
                        }
                    }
                    Stmt::Namespace(ns) => {
                        // 递归：路径 = ns_prefix + ns.path
                        let mut p = ns_prefix.to_vec();
                        p.extend(ns.path.clone());
                        if scan_tables(ctx, &ns.body, &p) {
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
            changed
        }
        if scan_tables(&mut ctx, &program.stmts, &[]) {
            changed = true;
        }
        if !changed {
            break;
        }
    }

    // 第二遍：检查函数体（含命名空间内函数；裸调用按命名空间前缀补全解析）
    for stmt in &program.stmts {
        match stmt {
            Stmt::FnDef(f) => ctx.check_fn(f)?,
            Stmt::Namespace(ns) => ctx.check_ns_stmts(&ns.body, &ns.path)?,
            _ => {}
        }
    }

    // 第三遍：方法体检查已随方法体系移出 struct（M2.1.8）——
    // 方法即绑定 struct 名的命名空间函数，其函数体已由第一遍 check_fn/check_ns_stmts 覆盖。

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
    /// 当前命名空间前缀栈（检查命名空间体内的函数时用；空 = 顶层）。
    /// 元素按从外到内排列，如 `namespace tcmsg { namespace error { } }` → ["tcmsg","error"]。
    ns_stack: Vec<String>,
    /// 导入视图（M2.1.7）：当前文件顶层的 import 语句（展开时保留）。
    /// 每项含别名与目标文件声明的命名空间路径——调用解析据此做前缀映射
    /// （别名唯一入口）与裸调用补全。
    import_views: Vec<ImportView>,
    /// using 引入的命名空间前缀（M2.1.7）：`using fmt2;` 之后，该命名空间的
    /// 公有函数可裸名调用（裸调用解析的第三候选）。
    using_prefixes: Vec<Vec<String>>,
    /// 循环标签栈（E5）：当前嵌套循环的标签（从外到内）。break/continue 的
    /// 标签跳转校验据此查找目标；无标签 break/continue 作用于栈顶循环。
    loop_labels: Vec<Option<String>>,
}

/// 一条 import 引入的视图信息（M2.1.7 单文件命名空间）。
struct ImportView {
    /// `as 别名`；Some 时原命名空间前缀在导入方**唯一入口**被别名取代
    alias: Option<String>,
    /// 被导入文件声明的命名空间路径（如 tools.tie 的 `namespace fmt` → [["fmt"]]）
    ns_paths: Vec<Vec<String>>,
}

impl Analyzer {
    /// 第零遍：收集顶层 import / using 语句（M2.1.7）。
    ///
    /// - import：构建导入视图（别名 → 原命名空间路径映射、using 目标校验依据）；
    /// - using：目标必须是已导入的命名空间前缀或别名（否则报错），收集其
    ///   原命名空间路径到 using_prefixes（裸调用补全候选）。
    fn collect_imports_using(&mut self, program: &Program) -> Result<(), SemanticError> {
        for stmt in &program.stmts {
            match stmt {
                Stmt::Import(imp) => {
                    self.import_views.push(ImportView {
                        alias: imp.alias.clone(),
                        ns_paths: imp.ns_paths.clone(),
                    });
                }
                Stmt::Using(u) => {
                    // using 目标解析：把「导入侧可见前缀」归一为原命名空间路径。
                    // 无别名：using fmt / using fmt.inner → 原路径直接匹配；
                    // 有别名：原前缀被屏蔽（唯一入口），只能用别名——using f2
                    // 或 using f2.inner（别名 + 子路径，与 map_import_prefix 一致）。
                    // 命中后存原命名空间全路径到 using_prefixes（裸调用补全候选）。
                    let mut target: Option<Vec<String>> = None;
                    for v in &self.import_views {
                        for ns in &v.ns_paths {
                            let hit = match &v.alias {
                                Some(alias) => {
                                    u.path.first() == Some(alias) && &u.path[1..] == &ns[1..]
                                }
                                None => &u.path == ns,
                            };
                            if hit {
                                target = Some(ns.clone());
                            }
                        }
                    }
                    let resolved = target.ok_or_else(|| SemanticError {
                        span: u.span,
                        message: format!(
                            "using 目标 '{}' 未导入：using 只能引用 import 引入的命名空间前缀或别名",
                            u.path.join(".")
                        ),
                    })?;
                    if self.using_prefixes.contains(&resolved) {
                        return Err(SemanticError {
                            span: u.span,
                            message: format!("命名空间 '{}' 重复 using", resolved.join("::")),
                        });
                    }
                    self.using_prefixes.push(resolved);
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// 裸调用名解析（M2.1.7）：返回实际调用的注册键（顶层裸名或命名空间全名）。
    ///
    /// 候选顺序（上一候选命中即返回，与旧版前缀补全顺序兼容）：
    /// 1. 裸名（顶层函数）：funcs 中存在即命中；
    /// 2. 当前命名空间前缀补全：`ns_stack + name` 全名存在（命名空间内函数互调）；
    /// 3. using 引入的命名空间：`using_prefixes[i] + name` 全名存在——多候选报歧义，
    ///    单候选命中（using 裸调用补全）；
    /// 4. 都不命中：返回裸名（保持原样，由调用方按「未定义函数」报错）。
    fn resolve_bare_call(&mut self, name: &str, span: &Span) -> Result<String, SemanticError> {
        // 候选 1：顶层裸名
        if self.result.funcs.contains_key(name) {
            return Ok(name.to_string());
        }
        // 候选 2：当前命名空间前缀补全（逐级外层：tcmsg::error::x → tcmsg::x → x，
        // 子命名空间可裸调父命名空间函数，如 tcmsg::error 内裸调 tcmsg 的 lookup）
        if !self.ns_stack.is_empty() {
            for depth in (0..=self.ns_stack.len()).rev() {
                let mut segs = self.ns_stack[..depth].to_vec();
                segs.push(name.to_string());
                let full = segs.join("::");
                if self.result.funcs.contains_key(&full) {
                    return Ok(full);
                }
            }
        }
        // 候选 3：using 引入的命名空间（裸调用补全；多候选报歧义）
        let mut hit: Option<String> = None;
        for prefix in &self.using_prefixes {
            let mut segs = prefix.clone();
            segs.push(name.to_string());
            let full = segs.join("::");
            if self.result.funcs.contains_key(&full) {
                if hit.is_some() {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "裸调用 '{name}' 有歧义：多个 using 引入的命名空间都包含该函数，请改用命名空间前缀调用"
                        ),
                    });
                }
                hit = Some(full);
            }
        }
        if let Some(full) = hit {
            return Ok(full);
        }
        // 都不命中：保持裸名（下方按未定义报错）
        Ok(name.to_string())
    }
    /// 导入前缀映射（M2.1.7）：把 receiver 路径段映射到被导入文件的命名空间全路径。
    ///
    /// 返回：
    /// - `Ok(Some(ns))`：命中导入视图（别名或原名），ns 是被导入文件的原命名空间路径；
    /// - `Ok(None)`：不涉及导入视图（调用方按 funcs 前缀原判定）；
    /// - `Err`：**唯一入口违规**——import 声明了别名，调用方却仍用原前缀访问。
    fn map_import_prefix(
        &mut self,
        segs: &[String],
        span: &Span,
    ) -> Result<Option<Vec<String>>, SemanticError> {
        for view in &self.import_views {
            for ns in &view.ns_paths {
                match &view.alias {
                    // 有别名：原前缀被屏蔽（唯一入口），别名是唯一可用前缀
                    Some(alias) => {
                        // 违规：调用方用原前缀（ns 或其子路径）
                        if segs.len() >= ns.len() && &segs[..ns.len()] == &ns[..] {
                            return Err(SemanticError {
                                span: *span,
                                message: format!(
                                    "命名空间前缀 '{}' 已被别名 '{}' 取代（import as 唯一入口），请改用别名访问",
                                    ns.join("::"),
                                    alias
                                ),
                            });
                        }
                        // 别名命中：segs[0] == alias 且其余段 == ns 的后续段
                        if segs[0] == *alias && &segs[1..] == &ns[1..] {
                            return Ok(Some(ns.clone()));
                        }
                    }
                    // 无别名：原前缀可用（确认是导入视图的命名空间）
                    None => {
                        if segs == ns {
                            return Ok(Some(ns.clone()));
                        }
                    }
                }
            }
        }
        Ok(None)
    }
    /// 可见性校验（M2.1.7）：命名空间内函数默认私有（仅同命名空间可调），
    /// `pub func` 显式导出后跨命名空间/跨文件可调。
    ///
    /// - 顶层函数（call_name 不含 `::`）恒公有（FuncSig.is_pub 恒 true）；
    /// - 命名空间函数：is_pub 为 true，或调用者所在命名空间（ns_stack）与
    ///   函数前缀一致（同命名空间互调）→ 放行；否则报私有调用错误。
    fn check_visibility(
        &mut self,
        call_name: &str,
        sig: &FuncSig,
        span: &Span,
    ) -> Result<(), SemanticError> {
        // 显式 pub：跨命名空间/跨文件可调
        if sig.is_pub {
            return Ok(());
        }
        // 顶层函数恒公有；只有命名空间函数（含 ::）才需要私有校验
        let Some((prefix, _)) = call_name.rsplit_once("::") else {
            return Ok(());
        };
        let own: Vec<&str> = prefix.split("::").collect();
        // 同命名空间或**子命名空间**放行：tcmsg::error 内可访问 tcmsg 的私有函数
        //（子模块可见父模块私有项，与裸调用逐级外层补全配套）
        let same_ns = self.ns_stack.len() >= own.len()
            && self.ns_stack[..own.len()]
                .iter()
                .zip(own.iter())
                .all(|(a, b)| a == b);
        if same_ns {
            return Ok(());
        }
        Err(SemanticError {
            span: *span,
            message: format!(
                "函数 '{call_name}' 是命名空间 '{prefix}' 的私有函数（默认私有，`pub func` 显式导出），不可在命名空间之外调用"
            ),
        })
    }
    /// 命名空间内函数收集（第一遍）：递归注册全名（路径段::函数名），支持嵌套命名空间。
    ///
    /// - 函数以全名进 funcs 表（如 "tcmsg::error::no_file"），供命名空间路径调用解析；
    /// - 同时记录 FnDefStmt 地址 → 全名到 fn_full_names（IR 层生成 LLVM 符号用）。
    /// - 体内类定义：当前不支持（命名空间内只允许函数与嵌套命名空间，parser 已限制）。
    fn collect_ns_funcs(&mut self, stmts: &[Stmt], prefix: &[String]) -> Result<(), SemanticError> {
        for stmt in stmts {
            match stmt {
                Stmt::FnDef(f) => {
                    // 全名 = 命名空间路径（已含外层）:: 函数名
                    let mut segs = prefix.to_vec();
                    segs.push(f.name.clone());
                    let full = segs.join("::");
                    let sig = FuncSig {
                        param_tys: f.params.iter().map(|p| p.ty.clone()).collect(),
                        param_defaults: f.params.iter().map(|p| p.default.clone()).collect(),
                        ret_ty: f.ret_ty.clone(),
                        // 命名空间内函数默认私有；`pub func` 显式导出（M2.1.7）
                        is_pub: f.is_pub,
                    };
                    if self.result.funcs.insert(full.clone(), sig).is_some() {
                        return Err(SemanticError {
                            span: f.span,
                            message: format!("函数 '{}' 在命名空间 '{}' 中重复定义", f.name, prefix.join("::")),
                        });
                    }
                    // FnDefStmt 地址 → 全名（IR 层生成符号用；与 AST 同生命周期）
                    self.result
                        .fn_full_names
                        .insert(f as *const FnDefStmt as usize, full);
                }
                Stmt::Namespace(inner) => {
                    // 嵌套命名空间：路径拼接后递归
                    let mut segs = prefix.to_vec();
                    segs.extend(inner.path.iter().cloned());
                    self.collect_ns_funcs(&inner.body, &segs)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// 命名空间体内语句检查（第二遍）：维护 ns_stack（裸调用前缀补全依据），
    /// 递归检查体内函数与嵌套命名空间。
    fn check_ns_stmts(&mut self, stmts: &[Stmt], prefix: &[String]) -> Result<(), SemanticError> {
        // 推入当前命名空间路径（check_fn 内部据此补全裸调用）
        self.ns_stack.extend(prefix.iter().cloned());
        for stmt in stmts {
            match stmt {
                Stmt::FnDef(f) => self.check_fn(f)?,
                Stmt::Namespace(inner) => {
                    // 嵌套命名空间：外层前缀已在栈上，递归只推内层路径
                    self.check_ns_stmts(&inner.body, &inner.path)?;
                }
                _ => {}
            }
        }
        // 弹出本层前缀（恢复调用方栈状态）
        let cur = self.ns_stack.len();
        self.ns_stack.truncate(cur.saturating_sub(prefix.len()));
        Ok(())
    }

    fn check_fn(&mut self, f: &FnDefStmt) -> Result<(), SemanticError> {
        // cur_fn 用全名（命名空间函数 = 路径::函数名），与 IR 层 gen_fn 的 cur_fn
        // 一致——table_vars/table_ret_elems 的键都依赖它（IR 层按同名查表）。
        self.cur_fn = if self.ns_stack.is_empty() {
            f.name.clone()
        } else {
            let mut segs = self.ns_stack.clone();
            segs.push(f.name.clone());
            segs.join("::")
        };
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
        // 可选参数（默认值）校验（M2.1 默认值参数）：
        // - 默认值必须跟在必选参数之后（一旦出现默认值，后续参数必须都有默认值）；
        // - 默认值限字面量：标量字面量（数/布尔/字符/字符串）或空表 `[]`（非空表
        //   字面量暂不支持——表默认值的布局元数据按调用点展开，非空表会因语义
        //   元数据键（表达式地址）与调用点克隆体不一致而失效，故语义层直接拦截）；
        // - 默认值类型必须与参数类型匹配。
        let mut seen_default = false;
        for p in &f.params {
            if p.default.is_some() {
                seen_default = true;
            } else if seen_default {
                return Err(SemanticError {
                    span: p.span,
                    message: format!(
                        "参数 '{}' 缺少默认值：可选参数（带默认值）必须连续排在必选参数之后",
                        p.name
                    ),
                });
            }
        }
        for p in &f.params {
            let Some(d) = &p.default else { continue };
            // 限字面量：标量字面量或空表 `[]`
            let is_ok_literal = match d {
                Expr::IntLit(_)
                | Expr::FloatLit(_)
                | Expr::BoolLit(_)
                | Expr::CharLit(_)
                | Expr::StrLit(_) => true,
                Expr::TableLit { cells, .. } => cells.is_empty(),
                _ => false,
            };
            if !is_ok_literal {
                return Err(SemanticError {
                    span: expr_span_of(d),
                    message: format!(
                        "参数 '{}' 的默认值必须是字面量（数/布尔/字符/字符串或空表 []）",
                        p.name
                    ),
                });
            }
            // 类型匹配：默认值推导类型必须与参数类型兼容（空表 → table）
            let dt = self.infer_expr(d, &scope)?;
            if !types_match(&p.ty, &dt, Some(d)) {
                return Err(SemanticError {
                    span: expr_span_of(d),
                    message: format!(
                        "参数 '{}' 默认值类型不匹配：期望 {}，实际 {}",
                        p.name,
                        type_name(&p.ty),
                        type_name(&dt)
                    ),
                });
            }
        }
        // 表参数：按「动态字符串表」登记布局元数据（M2 无泛型，约定 table<string>）。
        // 使函数体内可对表参数做 for 遍历 / len / table_at / 下标访问（如 std/json.tie
        // 的 json_array/json_object 消费调用方传入的序列化片段表）。元素类型固定为 string，
        // 与动态表（table_new_*）的 table_vars 布局结构一致。
        for p in &f.params {
            if p.ty == TypeSpec::Named(TyKw::Table) {
                let info = TableInfo {
                    elem_ty: TypeSpec::Named(TyKw::Str),
                    len: 0,
                    dynamic: true,
                };
                // 键用 cur_fn 全名（命名空间函数 = 路径::函数名），与 IR 层查询一致；
                // 用裸名 f.name 会导致命名空间函数内下标访问查不到布局元数据。
                self.table_vars
                    .insert((self.cur_fn.clone(), p.name.clone()), info.clone());
                self.result
                    .table_vars
                    .insert((self.cur_fn.clone(), p.name.clone()), info);
            }
        }
        for stmt in &f.body {
            self.check_stmt(stmt, &mut scope, &f.ret_ty)?;
        }
        Ok(())
    }

    /// struct 收集（M2.1.8）：继承链解析 + 字段拍平 + 冲突检查。
    ///
    /// 顺序保证：父 struct 先于子 struct 拍平（递归），拍平结果存 `result.classes`。
    /// 方法已移出 struct（逻辑 = 绑定 struct 名的命名空间函数），此处只处理字段。
    fn collect_structs(&mut self, program: &Program) -> Result<(), SemanticError> {
        // 第一步：struct 名登记与冲突检查（struct 名 vs 函数名、struct 名 vs struct 名）
        for stmt in &program.stmts {
            if let Stmt::Struct(c) = stmt {
                if self.result.funcs.contains_key(&c.name) {
                    return Err(SemanticError {
                        span: c.span,
                        message: format!("struct 名 '{}' 与函数名冲突", c.name),
                    });
                }
                if self.result.classes.contains_key(&c.name) {
                    return Err(SemanticError {
                        span: c.span,
                        message: format!("struct '{}' 重复定义", c.name),
                    });
                }
                self.result.classes.insert(c.name.clone(), ClassInfo {
                    parent: c.parent.clone(),
                    fields: Vec::new(),
                    field_index: HashMap::new(),
                });
            }
        }
        // 第二步：逐个 struct 做继承链拍平（递归解析父 struct 字段）
        // 先构造「struct 名 → 定义」映射以便查找父 struct
        let defs: HashMap<String, &StructDefStmt> = program
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::Struct(c) => Some((c.name.clone(), c)),
                _ => None,
            })
            .collect();
        let names: Vec<String> = self.result.classes.keys().cloned().collect();
        for name in names {
            let info = self.flatten_struct(&name, &defs, &mut HashSet::new())?;
            // 拍平后的结果替换占位
            self.result.classes.insert(name, info);
        }
        Ok(())
    }

    /// 拍平单个 struct：递归合并父 struct 字段，自身字段叠加，环检测。
    ///
    /// `chain` 是当前继承链上的 struct 名集合（路径环检测用，非全局访问集合）。
    fn flatten_struct(
        &self,
        name: &str,
        defs: &HashMap<String, &StructDefStmt>,
        chain: &mut HashSet<String>,
    ) -> Result<ClassInfo, SemanticError> {
        // 环检测：`struct A extends B` 且 B 又依赖 A → 死循环
        if !chain.insert(name.to_string()) {
            return Err(SemanticError {
                span: defs.get(name).map(|c| c.span).unwrap_or(Span { line: 0, col: 0 }),
                message: format!("struct 继承形成环（含 '{name}'）"),
            });
        }
        let def = defs.get(name).ok_or_else(|| SemanticError {
            span: Span { line: 0, col: 0 },
            message: format!("内部错误：struct '{name}' 无定义"),
        })?;
        let mut info = ClassInfo {
            parent: def.parent.clone(),
            fields: Vec::new(),
            field_index: HashMap::new(),
        };
        // 父 struct 字段拍平（递归；父 struct 未定义 → 报错）
        if let Some(p) = &def.parent {
            if !defs.contains_key(p) {
                return Err(SemanticError {
                    span: def.span,
                    message: format!("父 struct '{p}' 未定义"),
                });
            }
            let pinfo = self.flatten_struct(p, defs, chain)?;
            info.fields = pinfo.fields;
            info.field_index = pinfo.field_index;
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
        chain.remove(name);
        Ok(info)
    }

    /// 解析 struct 字段的具体类型。
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
                            // table：初始化必须是表字面量（定长）或 table_new_*/返回表的
                            // 函数调用（动态，运行时 {ptr,len,cap} 结构）。
                            if let Expr::TableLit { cells, .. } = &v.init {
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
                                    dynamic: false,
                                };
                                self.result.tables.insert(addr_of(&v.init), info.clone());
                                // 变量名 → 布局（下标访问/遍历时按变量名查询元素类型）
                                self.table_vars
                                    .insert((self.cur_fn.clone(), v.name.clone()), info.clone());
                                self.result
                                    .table_vars
                                    .insert((self.cur_fn.clone(), v.name.clone()), info);
                            } else {
                                // 动态表：table_new_* 或返回表的函数调用（元素类型静态已知）
                                let elem_ty = self.dynamic_table_elem_ty(&v.init, &v.name, scope)?;
                                let info = TableInfo { elem_ty, len: 0, dynamic: true };
                                self.table_vars
                                    .insert((self.cur_fn.clone(), v.name.clone()), info.clone());
                                self.result
                                    .table_vars
                                    .insert((self.cur_fn.clone(), v.name.clone()), info);
                            }
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
                        // 平衡三进制字面量适配（M4 补齐）：标注 trit 且初始化是
                        // BoolLit/TritLit 时，把初始化表达式的类型改写为 Trit——
                        // IR 层据此生成 i8 常量（true→1 / false→-1 / zero→0）。
                        if matches!(d, TypeSpec::Named(TyKw::Trit))
                            && matches!(v.init, Expr::BoolLit(_) | Expr::TritLit(_))
                        {
                            self.result.expr_types.insert(
                                addr_of(&v.init),
                                TypeSpec::Named(TyKw::Trit),
                            );
                        }
                    }
                    None => {
                        if init_ty.is_void() {
                            return Err(SemanticError {
                                span: v.span,
                                message: format!("变量 '{}' 不能用 void 表达式初始化", v.name),
                            });
                        }
                        // 未标注的动态表：`var x = table_new_i64()` / `var x = make_list(5)`
                        // （init_ty 为 Table 标记，元素类型需从调用名/返回表推断）
                        if init_ty == TypeSpec::Named(TyKw::Table) {
                            let elem_ty = self.dynamic_table_elem_ty(&v.init, &v.name, scope)?;
                            let info = TableInfo { elem_ty, len: 0, dynamic: true };
                            self.table_vars
                                .insert((self.cur_fn.clone(), v.name.clone()), info.clone());
                            self.result
                                .table_vars
                                .insert((self.cur_fn.clone(), v.name.clone()), info);
                        } else if let Expr::TableLit { cells, .. } = &v.init {
                            // 未标注表字面量：`var arr = [1,2,3]`（init_ty 是元素类型）。
                            // 登记 table_vars 元数据（定长、元素类型 = 首个元素类型），
                            // 使下标访问/下标赋值（t[i]=v）能定位表身份与元素类型。
                            // scope 存元素类型（既有行为，保持 IR 定长表布局一致）。
                            let elem_ty = if cells.is_empty() {
                                TypeSpec::Named(TyKw::I64)
                            } else {
                                init_ty.clone()
                            };
                            let info = TableInfo { elem_ty, len: cells.len(), dynamic: false };
                            self.table_vars
                                .insert((self.cur_fn.clone(), v.name.clone()), info.clone());
                            self.result
                                .table_vars
                                .insert((self.cur_fn.clone(), v.name.clone()), info);
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
            Stmt::Using(_) => {
                // using 只允许出现在文件顶层（第零遍收集；函数体内无意义）
                Err(SemanticError {
                    span: stmt_span(stmt),
                    message: "using 语句只能出现在文件顶层".into(),
                })
            }
            Stmt::Namespace(ns) => {
                // 命名空间块（函数体内出现）：命名空间只允许在顶层（parser 已限制），
                // 但防御性处理——递归检查体内语句。体内 FnDef/嵌套 Namespace 已由
                // analyze 顶层收集阶段以全名注册并检查，此处跳过避免重复检查与
                // "嵌套函数定义"误报；其余语句（如表达式）递归检查。
                for s in &ns.body {
                    match s {
                        Stmt::FnDef(_) | Stmt::Namespace(_) => {} // 已由 analyze 处理
                        _ => self.check_stmt(s, scope, ret_ty)?,
                    }
                }
                Ok(())
            }
            Stmt::Expr(e) => {
                let ty = self.infer_expr(&e.expr, scope)?;
                self.result.expr_types.insert(addr_of(&e.expr), ty);
                Ok(())
            }
            Stmt::Assign(a) => {
                // 赋值：目标必须已声明（函数作用域或全局持久变量）；const 不可变；
                // 普通赋值类型匹配 / 复合赋值按运算符校验
                let target_ty = match scope.get(&a.target) {
                    Some(t) => t.clone(),
                    None => match self.result.globals.get(&a.target) {
                        Some(t) => t.clone(),
                        None => {
                            return Err(SemanticError {
                                span: a.span,
                                message: format!("赋值目标 '{}' 未声明", a.target),
                            })
                        }
                    },
                };
                let value_ty = self.infer_expr(&a.value, scope)?;
                self.result.expr_types.insert(addr_of(&a.value), value_ty.clone());
                // const 变量不允许重新赋值（函数内 const 与全局 const 并列校验）
                if self.result.const_vars.contains(&a.target)
                    || self.result.const_globals.contains(&a.target)
                {
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
                // 动态表返回：记录元素类型（供调用方推断返回表的元素类型）。
                // 支持返回 table_new_*、返回表的函数调用，或本函数内声明的动态表变量。
                if let Some(e) = &r.expr
                    && matches!(ret_ty, TypeSpec::Named(TyKw::Table))
                {
                    let elem_ty = self.table_arg_elem_ty(e, scope)?;
                    self.result.table_ret_elems.insert(self.cur_fn.clone(), Some(elem_ty));
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
                // 循环标签入栈（E5）：body 内 break L/continue L 可跳转到此循环
                self.loop_labels.push(w.label.clone());
                self.check_block(&w.body, scope, ret_ty)?;
                self.loop_labels.pop();
                Ok(())
            }
            // break/continue（E1+E5）：必须在循环内；带标签的必须能找到匹配循环
            Stmt::Break(b) => self.check_loop_jump(b.label.as_deref(), b.span, "break"),
            Stmt::Continue(c) => self.check_loop_jump(c.label.as_deref(), c.span, "continue"),            Stmt::For(f) => {
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
                // 循环标签入栈（E5）：body 内 break L/continue L 可跳转到此循环
                self.loop_labels.push(f.label.clone());
                self.check_block(&f.body, scope, ret_ty)?;
                self.loop_labels.pop();
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
                // 每个 case 的每个 pattern 逐一校验（模式匹配增强）：
                // - 字面量：类型一致 + 编译期字面量 + 不重复（沿用现状）；
                // - 区间 Range：两端为整数或字符字面量且 start < end，类型与 subject 一致；
                // - 类型 TypeLit：仅宽类型/动态容器对象上允许（当前 subject 均为静态类型 → 报错）；
                // - when 守卫：必须为布尔表达式。
                let mut seen: Vec<String> = Vec::new();
                for c in &s.cases {
                    for pat in &c.patterns {
                        match pat {
                            // 区间 pattern：`case 3..7:`（整数/字符，start < end）
                            Expr::Range { start, end, span } => {
                                // 两端必须是编译期字面量（整数或字符，浮点区间明确不支持）
                                if !is_int_or_char_literal(start) || !is_int_or_char_literal(end) {
                                    return Err(SemanticError {
                                        span: *span,
                                        message: "case 区间两端必须是整数或字符字面量（浮点区间不支持）".into(),
                                    });
                                }
                                // start < end（字符按 UTF-32 值比较）
                                if literal_cmp(start, end) >= 0 {
                                    return Err(SemanticError {
                                        span: *span,
                                        message: "case 区间必须 start < end（左闭右开）".into(),
                                    });
                                }
                                // 区间类型与 subject 一致：整数区间对整数 subject，字符区间对字符 subject
                                let range_is_char = matches!(start.as_ref(), Expr::CharLit(_));
                                let ok = if range_is_char {
                                    matches!(&subject_ty, TypeSpec::Named(TyKw::Char))
                                } else {
                                    subject_ty.is_int()
                                };
                                if !ok {
                                    return Err(SemanticError {
                                        span: *span,
                                        message: format!(
                                            "case 区间类型与 switch 对象类型 {} 不匹配",
                                            type_name(&subject_ty)
                                        ),
                                    });
                                }
                                // 区间去重键（避免重复区间）
                                let key = literal_key(start) + ".." + &literal_key(end);
                                if seen.contains(&key) {
                                    return Err(SemanticError {
                                        span: *span,
                                        message: format!("重复的 case 区间 {key}"),
                                    });
                                }
                                seen.push(key);
                            }
                            // 类型匹配 pattern：`case string:` —— 仅宽类型/动态容器对象上有意义；
                            // 当前 switch subject 限定静态类型（上方检查），故一律报错
                            Expr::TypeLit { ty, span } => {
                                return Err(SemanticError {
                                    span: *span,
                                    message: format!(
                                        "case 类型匹配（{}）仅支持宽类型或动态容器对象，当前 switch 对象是静态类型 {}",
                                        type_name(ty),
                                        type_name(&subject_ty)
                                    ),
                                });
                            }
                            // 字面量 pattern：类型一致 + 编译期字面量 + 不重复（沿用现状）
                            _ => {
                                let value_ty = self.infer_expr(pat, scope)?;
                                self.result.expr_types.insert(addr_of(pat), value_ty.clone());
                                // case 值必须是编译期字面量（不允许变量/表达式）
                                if !is_const_literal(pat) {
                                    return Err(SemanticError {
                                        span: c.span,
                                        message: "case 值必须是字面量（整数/浮点/字符/布尔/字符串）".into(),
                                    });
                                }
                                // case 值类型必须与 subject 类型匹配
                                if !types_match(&subject_ty, &value_ty, Some(pat)) {
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
                                let key = literal_key(pat);
                                if seen.contains(&key) {
                                    return Err(SemanticError {
                                        span: c.span,
                                        message: format!("重复的 case 值 {}", key),
                                    });
                                }
                                seen.push(key);
                            }
                        }
                    }
                    // when 守卫：必须为布尔表达式（与 if 条件同规则）
                    if let Some(w) = &c.when {
                        let wty = self.infer_expr(w, scope)?;
                        self.result.expr_types.insert(addr_of(w), wty.clone());
                        if !is_bool_like(&wty) {
                            return Err(SemanticError {
                                span: expr_span_of(w),
                                message: format!(
                                    "when 守卫必须是布尔表达式，实际是 {}",
                                    type_name(&wty)
                                ),
                            });
                        }
                    }
                    self.check_block(&c.body, scope, ret_ty)?;
                }
                self.check_block(&s.default_body, scope, ret_ty)?;
                Ok(())
            }
            Stmt::Struct(_) => {
                // struct 定义只允许出现在文件顶层（字段类型解析在 collect_structs）
                Err(SemanticError {
                    span: stmt_span(stmt),
                    message: "struct 定义只能出现在文件顶层".into(),
                })
            }
            Stmt::FieldAssign(fa) => {
                // 字段赋值：base 必须是 struct 实例（变量/字段链，可寻址），字段存在，类型匹配
                let base_ty = self.infer_expr(&fa.base, scope)?;
                self.result.expr_types.insert(addr_of(&fa.base), base_ty.clone());
                let TypeSpec::Struct(class_name) = &base_ty else {
                    return Err(SemanticError {
                        span: fa.span,
                        message: format!(
                            "字段赋值的对象必须是 struct 实例，实际是 {}",
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
            Stmt::IndexAssign(ia) => {
                // 表下标赋值（M4 补齐）：`t[i] = v`——target 是 Index 链
                //（`t[i]` / `t[i][j]`）。校验：base 是表、下标是整数、
                // 值类型与表元素类型匹配、复合赋值按运算符规则。
                let Expr::Index { base, index, .. } = ia.target.as_ref() else {
                    return Err(SemanticError {
                        span: ia.span,
                        message: "下标赋值的目标必须是表元素访问（t[i]）".into(),
                    });
                };
                // 1) base 必须是表（查 table_vars 元数据；非表 → 报错）。
                // 注：未标注表字面量变量（var arr = [1,2,3]）在 scope 推导为元素类型
                //（既有行为），表身份由 table_vars 元数据决定——以此为准。
                let base_ty = self.infer_expr(base, scope)?;
                self.result.expr_types.insert(addr_of(base), base_ty.clone());
                let is_table = if let Expr::Var(vn) = base.as_ref() {
                    self.table_vars.contains_key(&(self.cur_fn.clone(), vn.clone()))
                        || matches!(&base_ty, TypeSpec::Named(TyKw::Table))
                } else {
                    matches!(&base_ty, TypeSpec::Named(TyKw::Table))
                };
                let elem_ty = if is_table {
                    match self.table_arg_elem_ty(base, scope) {
                        Ok(et) => et,
                        Err(e) => return Err(e),
                    }
                } else {
                    return Err(SemanticError {
                        span: ia.span,
                        message: format!(
                            "下标赋值的对象必须是表，实际是 {}",
                            type_name(&base_ty)
                        ),
                    });
                };
                // 2) 下标必须是整数
                let idx_ty = self.infer_expr(index, scope)?;
                self.result.expr_types.insert(addr_of(index), idx_ty.clone());
                if !idx_ty.is_int() {
                    return Err(SemanticError {
                        span: expr_span_of(index),
                        message: format!("下标必须是整数，实际是 {}", type_name(&idx_ty)),
                    });
                }
                // 3) 值类型与元素类型匹配（复合赋值走 check_compound_assign）
                let value_ty = self.infer_expr(&ia.value, scope)?;
                self.result.expr_types.insert(addr_of(&ia.value), value_ty.clone());
                if let Some(op) = ia.op {
                    self.check_compound_assign(&elem_ty, op, &value_ty, &ia.value, ia.span)?;
                } else if !types_match(&elem_ty, &value_ty, Some(&ia.value)) {
                    return Err(SemanticError {
                        span: ia.span,
                        message: format!(
                            "下标赋值类型不匹配：表元素类型为 {}，表达式为 {}",
                            type_name(&elem_ty),
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

    /// break/continue 的循环上下文校验（E1+E5）：
    /// - 无标签：必须在任意循环内（loop_labels 非空）；
    /// - 带标签：标签必须匹配当前某个外层/内层循环的标签（从栈顶向内查找）。
    fn check_loop_jump(
        &self,
        label: Option<&str>,
        span: Span,
        kw: &str,
    ) -> Result<(), SemanticError> {
        match label {
            // 无标签：作用于最近循环，要求处于循环体内
            None => {
                if self.loop_labels.is_empty() {
                    return Err(SemanticError {
                        span,
                        message: format!("{kw} 只能出现在循环体内"),
                    });
                }
                Ok(())
            }
            // 带标签：从栈顶向内查找匹配标签（最近优先）
            Some(l) => {
                if !self.loop_labels.iter().any(|x| x.as_deref() == Some(l)) {
                    return Err(SemanticError {
                        span,
                        message: format!("{kw} 的标签 '{l}' 未匹配任何外层循环"),
                    });
                }
                Ok(())
            }
        }
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
            // 平衡三进制 trit 字面量（M4 补齐）：zero → trit 类型
            Expr::TritLit(_) => TypeSpec::Named(TyKw::Trit),
            Expr::StrLit(_) => TypeSpec::Named(TyKw::Str),
            Expr::CharLit(_) => TypeSpec::Named(TyKw::Char),
            // 类型字面量（case 类型匹配 pattern）：只能出现在 switch 的 case pattern
            // 位置（语义层 switch 校验已单独处理），普通表达式上下文不应出现
            Expr::TypeLit { ty, span } => {
                return Err(SemanticError {
                    span: *span,
                    message: format!("类型字面量 {} 只能用作 switch 的 case 类型匹配", type_name(ty)),
                })
            }
            Expr::Var(name) => match scope.get(name) {
                Some(t) => t.clone(),
                // 未命中函数作用域：查顶层全局持久变量（M4）
                None => match self.result.globals.get(name) {
                    Some(t) => t.clone(),
                    None => {
                        return Err(SemanticError {
                            span: expr_span_of(expr),
                            message: format!("未声明的变量 '{name}'"),
                        })
                    }
                },
            },
            // 命名空间路径（a::b::c）本身不是值表达式：只能作为 MethodCall 的
            // receiver（tcmsg::error.no_file()），独立出现即语义错误。
            Expr::Path { segments, span } => {
                return Err(SemanticError {
                    span: *span,
                    message: format!(
                        "命名空间路径 '{}' 不能作为值使用（只能用于调用，如 '{}::xxx()'）",
                        segments.join("::"),
                        segments.join("::")
                    ),
                })
            }
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
                    return Ok(TypeSpec::Struct(name.clone()));
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
                // 内置函数 str_len：单字符串参数，返回 i64（字符串的 Unicode 码点数）。
                // 与 len（UTF-8 字节数）区分：str_char 按码点索引，码点级遍历必须用
                // str_len 做边界，否则中文等多字节字符会错位。表用 len（元素数）。
                if name == "str_len" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("str_len() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("str_len() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::I64));
                }
                // 内置函数 table_new_*：零参数，创建空动态表，返回 table。
                // 元素类型由函数名决定（i64/f64/string/bool），运行时 {ptr,len,cap} 结构。
                if let Some(elem_ty) = table_new_elem_ty(name) {
                    if !args.is_empty() {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("{name}() 期望 0 个参数，实际 {} 个", args.len()),
                        });
                    }
                    // 记下返回元素类型（调用点处用：函数返回表时由 table_ret_elems 承担，
                    // 直接赋给变量的动态表由 VarDecl 分支的 dynamic_table_elem_ty 解析）。
                    self.result.expr_types.insert(addr_of(expr), TypeSpec::Named(TyKw::Table));
                    let _ = elem_ty;
                    return Ok(TypeSpec::Named(TyKw::Table));
                }
                // 内置函数 table_push：双参数（表 + 元素），void。
                // 元素类型须与表的元素类型一致（定长/动态表均查 table_vars）。
                if name == "table_push" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("table_push() 期望 2 个参数（表, 元素），实际 {} 个", args.len()),
                        });
                    }
                    let t_ty = self.infer_expr(&args[0], scope)?;
                    let x_ty = self.infer_expr(&args[1], scope)?;
                    self.result.expr_types.insert(addr_of(&args[1]), x_ty.clone());
                    if !matches!(&t_ty, TypeSpec::Named(TyKw::Table)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("table_push() 第 1 个参数必须是表，实际是 {}", type_name(&t_ty)),
                        });
                    }
                    // 第 1 个参数必须是表变量（table_vars 才有元素类型；IR 需地址写回）
                    let Expr::Var(name0) = &args[0] else {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: "table_push() 第 1 个参数必须是表变量（不能是字面量/下标）".into(),
                        });
                    };
                    let info = self
                        .table_vars
                        .get(&(self.cur_fn.clone(), name0.clone()))
                        .ok_or_else(|| SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("table_push() 找不到表变量 '{}' 的元素类型", name0),
                        })?;
                    // 只有动态表（table_new_* 创建）可 push；定长表用下标赋值/字面量
                    if !info.dynamic {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("table_push() 只能用于动态表（table_new_* 创建），'{}' 是定长表", name0),
                        });
                    }
                    if !types_match(&info.elem_ty, &x_ty, Some(&args[1])) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[1]),
                            message: format!(
                                "table_push() 元素类型不匹配：表 '{}' 的元素是 {}，推入的是 {}",
                                name0,
                                type_name(&info.elem_ty),
                                type_name(&x_ty)
                            ),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Void));
                }
                // 内置函数 table_at：双参数（表, 下标），返回表元素类型。
                // 下标必须整数；越界是运行时错误（编译路径与解释路径报一致中文错误）。
                if name == "table_at" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("table_at() 期望 2 个参数（表, 下标），实际 {} 个", args.len()),
                        });
                    }
                    let t_ty = self.infer_expr(&args[0], scope)?;
                    let i_ty = self.infer_expr(&args[1], scope)?;
                    self.result.expr_types.insert(addr_of(&args[1]), i_ty.clone());
                    if !matches!(&t_ty, TypeSpec::Named(TyKw::Table)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("table_at() 第 1 个参数必须是表，实际是 {}", type_name(&t_ty)),
                        });
                    }
                    if !i_ty.is_int() {
                        return Err(SemanticError {
                            span: expr_span_of(&args[1]),
                            message: format!("table_at() 下标必须是整数，实际是 {}", type_name(&i_ty)),
                        });
                    }
                    // 元素类型：表变量（table_vars）/ 返回表的函数（table_ret_elems）。
                    // 定长表用下标 t[i] 访问，table_at 仅用于动态表（运行时 {ptr,len,cap}）。
                    let elem_ty = self.table_arg_elem_ty(&args[0], scope)?;
                    return Ok(elem_ty);
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
                // 内置函数 eval_call：两个字符串参数（函数名, 参数），返回 string
                //（调用已注册用户函数——tie:script 模块协议执行基础）。
                if name == "eval_call" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("eval_call() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at.clone());
                        if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("eval_call() 参数必须是字符串，实际是 {}", type_name(&at)),
                            });
                        }
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
                // 内置函数 file_delete：单字符串参数，返回 bool（文件删除成功与否；
                // 不存在/不可删 → false）。
                if name == "file_delete" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("file_delete() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("file_delete() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Bool));
                }
                // ---------- M4 补齐：系统能力内置函数（M6 包管理器前置） ----------
                //
                // 签名校验（参数个数 + 字符串类型），返回类型按原语映射。
                // 分组：单字符串参 → string（http_get/exec_output/path_* 单参/get_env）；
                // 单字符串参 → bool/i64/table（mkdir_all/remove_dir_all、exec_code、walk_dir）；
                // 双字符串参 → bool/string/void（http_get_file/untar_gz/unzip/copy_dir/
                // file_copy/file_move、path_join、set_env）；零参 → string（cwd）。
                if matches!(
                    name.as_str(),
                    "http_get" | "exec_output" | "path_basename" | "path_dirname" | "path_abs"
                        | "path_normalize" | "get_env"
                ) {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("{name}() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("{name}() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 单字符串参数 → bool（mkdir_all 建多级目录 / remove_dir_all 递归删目录）
                if name == "mkdir_all" || name == "remove_dir_all" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("{name}() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("{name}() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Bool));
                }
                // 单字符串参数 → i64（exec_code 执行命令返回退出码）
                if name == "exec_code" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("exec_code() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("exec_code() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::I64));
                }
                // 单字符串参数 → table（walk_dir 递归列出全部文件相对路径，字符串动态表）
                if name == "walk_dir" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("walk_dir() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("walk_dir() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Table));
                }
                // ---------- D7：字节流 / 位操作原语（编解码器底座） ----------
                // byte_read(path) -> table（i64 字节表）；byte_write(path, bytes: table) -> bool；
                // bit_read(bytes: table, pos: i64) -> i64；bit_write(bytes: table, pos: i64, bit: i64) -> bool；
                // byte_concat(a: table, b: table) -> table。
                if name == "byte_read" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("byte_read() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("byte_read() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Table));
                }
                // byte_write(path: string, bytes: table) -> bool
                if name == "byte_write" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("byte_write() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let p = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), p.clone());
                    if !matches!(&p, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("byte_write() 第 1 个参数必须是字符串，实际是 {}", type_name(&p)),
                        });
                    }
                    let b = self.infer_expr(&args[1], scope)?;
                    self.result.expr_types.insert(addr_of(&args[1]), b.clone());
                    if !matches!(&b, TypeSpec::Named(TyKw::Table)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[1]),
                            message: format!("byte_write() 第 2 个参数必须是字节表，实际是 {}", type_name(&b)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Bool));
                }
                // bit_read(bytes: table, pos: i64) -> i64
                if name == "bit_read" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("bit_read() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let b = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), b.clone());
                    if !matches!(&b, TypeSpec::Named(TyKw::Table)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("bit_read() 第 1 个参数必须是字节表，实际是 {}", type_name(&b)),
                        });
                    }
                    let q = self.infer_expr(&args[1], scope)?;
                    self.result.expr_types.insert(addr_of(&args[1]), q.clone());
                    if !q.is_int() {
                        return Err(SemanticError {
                            span: expr_span_of(&args[1]),
                            message: format!("bit_read() 第 2 个参数必须是整数，实际是 {}", type_name(&q)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::I64));
                }
                // bit_write(bytes: table, pos: i64, bit: i64) -> bool
                if name == "bit_write" {
                    if args.len() != 3 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("bit_write() 期望 3 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let b = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), b.clone());
                    if !matches!(&b, TypeSpec::Named(TyKw::Table)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("bit_write() 第 1 个参数必须是字节表，实际是 {}", type_name(&b)),
                        });
                    }
                    for a in &args[1..] {
                        let t = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), t.clone());
                        if !t.is_int() {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("bit_write() 位置/位值必须是整数，实际是 {}", type_name(&t)),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::Bool));
                }
                // byte_concat(a: table, b: table) -> table
                if name == "byte_concat" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("byte_concat() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let t = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), t.clone());
                        if !matches!(&t, TypeSpec::Named(TyKw::Table)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("byte_concat() 参数必须是字节表，实际是 {}", type_name(&t)),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::Table));
                }
                // 双字符串参数 → bool（http_get_file/untar_gz/unzip/copy_dir/file_copy/file_move）
                if matches!(
                    name.as_str(),
                    "http_get_file" | "untar_gz" | "unzip" | "copy_dir" | "file_copy"
                        | "file_move"
                ) {
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
                // 双字符串参数 → string（path_join 拼接路径）
                if name == "path_join" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("path_join() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at.clone());
                        if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("path_join() 参数必须是字符串，实际是 {}", type_name(&at)),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 双字符串参数 → void（set_env 设置环境变量）
                if name == "set_env" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("set_env() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at.clone());
                        if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("set_env() 参数必须是字符串，实际是 {}", type_name(&at)),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::Void));
                }
                // 零参数 → string（cwd 当前工作目录）
                if name == "cwd" {
                    if !args.is_empty() {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("cwd() 期望 0 个参数，实际 {} 个", args.len()),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
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
                // ---------- P1 正则表达式内置函数 ----------
                //
                // 语义：regex_match 部分匹配即真（RE2 无回溯）；regex_find 返回首个匹配片段；
                // regex_find_all 返回全部匹配片段（字符串动态表）；regex_replace 全部替换
                // （to 支持 $1 捕获引用）；regex_group 返回首个匹配的第 i 个捕获组
                // （i=0 为整个匹配）。模式非法 → 运行时报错（编译/解释两路径一致）。
                if name == "regex_match" || name == "regex_find" || name == "regex_find_all" {
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
                    return Ok(match name.as_str() {
                        "regex_match" => TypeSpec::Named(TyKw::Bool),
                        "regex_find" => TypeSpec::Named(TyKw::Str),
                        _ => TypeSpec::Named(TyKw::Table),
                    });
                }
                if name == "regex_replace" {
                    if args.len() != 3 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("regex_replace() 期望 3 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at.clone());
                        if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("regex_replace() 参数必须是字符串，实际是 {}", type_name(&at)),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                if name == "regex_group" {
                    if args.len() != 3 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("regex_group() 期望 3 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let st = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), st.clone());
                    if !matches!(&st, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("regex_group() 第 1 个参数必须是字符串，实际是 {}", type_name(&st)),
                        });
                    }
                    let pt = self.infer_expr(&args[1], scope)?;
                    self.result.expr_types.insert(addr_of(&args[1]), pt.clone());
                    if !matches!(&pt, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[1]),
                            message: format!("regex_group() 第 2 个参数必须是字符串，实际是 {}", type_name(&pt)),
                        });
                    }
                    let it = self.infer_expr(&args[2], scope)?;
                    self.result.expr_types.insert(addr_of(&args[2]), it.clone());
                    if !it.is_int() {
                        return Err(SemanticError {
                            span: expr_span_of(&args[2]),
                            message: format!("regex_group() 第 3 个参数必须是整数，实际是 {}", type_name(&it)),
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
                    // 平衡三进制（M4 补齐）：to_string(trit) → "-1"/"0"/"1"
                    if matches!(&at, TypeSpec::Named(TyKw::Trit)) {
                        return Ok(TypeSpec::Named(TyKw::Str));
                    }
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
                // 内置函数 parse_trit（M4 补齐）：字符串参数，返回 trit。
                // 接受 "-1"/"0"/"1"（非法输入 → 运行时错误，两路径一致）。
                if name == "parse_trit" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("parse_trit() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("parse_trit() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Trit));
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
                // 内置函数 arg_count：零参数，返回 i64（命令行用户参数个数，不含程序名）。
                // 进程/环境 floor：编译与解释两路径共用 std::env::args（见 tie-interp 桥）。
                if name == "arg_count" {
                    if !args.is_empty() {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("arg_count() 期望 0 个参数，实际 {} 个", args.len()),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::I64));
                }
                // 内置函数 arg_string：整数参数，返回 string（第 i 个用户命令行参数；
                // 越界返回空串）。
                if name == "arg_string" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("arg_string() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !at.is_int() {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("arg_string() 参数必须是整数，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 内置函数 list_dir：单字符串参数（目录路径），返回 table<string>（文件名集合）。
                // 目录不存在/读取失败 → 运行时错误（文本与编译路径一致）。
                // M2 文件系统 floor：目录枚举是 Rust 层唯一实现（tie 无法表达目录遍历）。
                if name == "list_dir" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("list_dir() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("list_dir() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Table));
                }
                // 内置函数 msg_set_lang：单字符串参数（语言名），void（切换消息语言）。
                // 消息系统 floor（#25）：语言与字典是进程内可变状态，tie 无全局可变变量，
                // 由 Rust 层 thread_local 持有（实在不行才 Rust 的典型场景）。
                if name == "msg_set_lang" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("msg_set_lang() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("msg_set_lang() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Void));
                }
                // 内置函数 msg_get_lang：零参数，返回 string（当前消息语言）。
                // 与 msg_set_lang 配套：供标准库按当前语言匹配文本（tcmsg 综合方案）。
                if name == "msg_get_lang" {
                    if !args.is_empty() {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("msg_get_lang() 期望 0 个参数，实际 {} 个", args.len()),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 内置函数 msg_register：三个字符串参数（键, 语言, 文本），void（登记消息）。
                if name == "msg_register" {
                    if args.len() != 3 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("msg_register() 期望 3 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at.clone());
                        if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!("msg_register() 参数必须是字符串，实际是 {}", type_name(&at)),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::Void));
                }
                // 内置函数 msg_t：单字符串参数（键），返回 string（当前语言翻译，回退 zh，再回退键本身）。
                if name == "msg_t" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("msg_t() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("msg_t() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
                }
                // 内置函数 print_err（M4）：单字符串参数，void——向 stderr 输出一行
                //（消息系统的 error/warn/debug 通道；info 走 stdout 的 println）。
                if name == "print_err" {
                    if args.len() != 1 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("print_err() 期望 1 个参数，实际 {} 个", args.len()),
                        });
                    }
                    let at = self.infer_expr(&args[0], scope)?;
                    self.result.expr_types.insert(addr_of(&args[0]), at.clone());
                    if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                        return Err(SemanticError {
                            span: expr_span_of(&args[0]),
                            message: format!("print_err() 参数必须是字符串，实际是 {}", type_name(&at)),
                        });
                    }
                    return Ok(TypeSpec::Named(TyKw::Void));
                }
                // 内置函数 msg_t_lang（M4）：两个字符串参数（键, 语言），返回 string——
                // 按指定语言查询字典（不做回退，未命中返回空串）；供 tcmsg 用顶层
                // 持久变量表达回退链后逐语言遍历（替代固定 zh 回退的 msg_t）。
                if name == "msg_t_lang" {
                    if args.len() != 2 {
                        return Err(SemanticError {
                            span: *span,
                            message: format!("msg_t_lang() 期望 2 个参数，实际 {} 个", args.len()),
                        });
                    }
                    for a in args {
                        let at = self.infer_expr(a, scope)?;
                        self.result.expr_types.insert(addr_of(a), at.clone());
                        if !matches!(&at, TypeSpec::Named(TyKw::Str)) {
                            return Err(SemanticError {
                                span: expr_span_of(a),
                                message: format!(
                                    "msg_t_lang() 参数必须是字符串，实际是 {}",
                                    type_name(&at)
                                ),
                            });
                        }
                    }
                    return Ok(TypeSpec::Named(TyKw::Str));
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
                // 用户函数：校验参数个数与类型。
                // 裸名解析顺序（M2.1.7 升级）：先查裸名（顶层函数），再按当前
                // 命名空间前缀补全（命名空间内函数互调），再查 using 引入的命名
                // 空间（唯一候选）；命中命名空间函数则记录调用表达式 → 全名映射。
                let call_name = self.resolve_bare_call(name, span)?;
                let sig = self.result.funcs.get(&call_name).cloned().ok_or_else(|| {
                    SemanticError {
                        span: *span,
                        message: format!("未定义的函数 '{name}'"),
                    }
                })?;
                // 可见性校验（M2.1.7）：私有函数仅同命名空间内可调
                self.check_visibility(&call_name, &sig, span)?;
                // 裸调用命中命名空间函数 → 记录全名（IR/解释层据此生成调用）
                if call_name != *name {
                    self.result.resolved_calls.insert(addr_of(expr), call_name.clone());
                }
                // 参数个数区间检查（默认值参数）：实参数必须在 [必选数, 总形参数] 内。
                // 必选数 = 无默认值的形参数（可选参数连续排在尾部，已由 check_fn 保证）。
                let required = sig
                    .param_defaults
                    .iter()
                    .take_while(|d| d.is_none())
                    .count();
                if args.len() < required || args.len() > sig.param_tys.len() {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "函数 '{call_name}' 期望 {} 个参数，实际 {} 个",
                            param_count_desc(required, sig.param_tys.len()),
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
                                "调用 '{call_name}' 参数类型不匹配：期望 {}，实际 {}",
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
                        // 平衡三值逻辑非（M4 补齐）：!trit → trit（-1↔1，0 保持）
                        if matches!(&ot, TypeSpec::Named(TyKw::Trit)) {
                            return Ok(TypeSpec::Named(TyKw::Trit));
                        }
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
                // 左右类型必须一致（int 与 float 不隐式转换）。
                // 例外（M4 补齐）：trit 与 i64 的混合运算由下方分支放行
                //（算术提升 i64、比较允许），此处兼容检查跳过 trit×i64 组合。
                let is_trit = |t: &TypeSpec| matches!(t, TypeSpec::Named(TyKw::Trit));
                if !types_compatible(&lt, &rt)
                    && !(is_trit(&lt) && matches!(&rt, TypeSpec::Named(TyKw::I64)))
                    && !(is_trit(&rt) && matches!(&lt, TypeSpec::Named(TyKw::I64)))
                {
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
                        // 平衡三进制算术（M4 补齐）：trit ± * trit → trit（值域 clamp 到
                        // [-1,1]，Kleene 风格饱和算术）；trit 与 i64 混合 → i64（sext 提升）。
                        // div/mod 不允许 trit（三值除法无意义）。
                        let is_trit = |t: &TypeSpec| matches!(t, TypeSpec::Named(TyKw::Trit));
                        if is_trit(&lt) || is_trit(&rt) {
                            if matches!(op, BinaryOp::Div | BinaryOp::Mod) {
                                return Err(SemanticError {
                                    span: *span,
                                    message: "trit 不支持除/取模运算（三值无除法）".into(),
                                });
                            }
                            if is_trit(&lt) && is_trit(&rt) {
                                return Ok(TypeSpec::Named(TyKw::Trit));
                            }
                            // 一侧 trit 一侧 i64（或反之）→ 提升为 i64
                            if (is_trit(&lt) && matches!(&rt, TypeSpec::Named(TyKw::I64)))
                                || (is_trit(&rt) && matches!(&lt, TypeSpec::Named(TyKw::I64)))
                            {
                                return Ok(TypeSpec::Named(TyKw::I64));
                            }
                            return Err(SemanticError {
                                span: *span,
                                message: format!(
                                    "trit 只能与 trit 或 i64 做算术，不能与 {}",
                                    type_name(if is_trit(&lt) { &rt } else { &lt })
                                ),
                            });
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
                        // 平衡三进制比较（M4 补齐）：trit 可比较（==/!=/</>/<=/>=），
                        // 与 trit 或与 i64（数值序 -1 < 0 < 1）→ bool
                        let is_trit = |t: &TypeSpec| matches!(t, TypeSpec::Named(TyKw::Trit));
                        if is_trit(&lt) && (is_trit(&rt) || matches!(&rt, TypeSpec::Named(TyKw::I64))) {
                            return Ok(TypeSpec::Named(TyKw::Bool));
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
                        // 平衡三值逻辑（Kleene 语义，M4 补齐）：trit && trit → trit
                        //（min/max 规则：&& 取较小者，|| 取较大者）；bool 保持原逻辑。
                        if matches!(&lt, TypeSpec::Named(TyKw::Trit))
                            && matches!(&rt, TypeSpec::Named(TyKw::Trit))
                        {
                            return Ok(TypeSpec::Named(TyKw::Trit));
                        }
                        if !is_bool_like(&lt) {
                            return Err(SemanticError {
                                span: *span,
                                message: "逻辑运算符两侧必须是 bool（或两侧同为 trit）".into(),
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
                let info = TableInfo { elem_ty: first_ty.clone(), len: cells.len(), dynamic: false };
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
                // 表身份判定：base_ty 是 Table，或 base 是登记过 table_vars 的变量
                //（未标注表字面量变量 var arr = [1,2,3] 的 base_ty 是元素类型——既有行为，
                // 表身份由 table_vars 元数据决定）。
                let is_table = if let Expr::Var(name) = base.as_ref() {
                    self.table_vars.contains_key(&(self.cur_fn.clone(), name.clone()))
                        || base_ty == TypeSpec::Named(TyKw::Table)
                } else {
                    base_ty == TypeSpec::Named(TyKw::Table)
                };
                if !is_table {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "下标访问的对象必须是表或字符串，实际是 {}",
                            type_name(&base_ty)
                        ),
                    });
                }
                // 元素类型：base 是表变量 → 查其布局元数据；是内联表字面量 → 元素同构类型；
                // 是返回表的函数调用（如 csv.csv_cells(...)[0]）→ 查 table_ret_elems
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
                    Expr::Call { .. } | Expr::MethodCall { .. } => {
                        let full = ns_call_full_name(
                            &self.result.funcs,
                            base,
                            scope,
                            &self.ns_stack,
                            &self.using_prefixes,
                            &self.result.globals,
                        )
                        .ok_or_else(|| SemanticError {
                            span: *span,
                            message: format!(
                                "下标访问的调用 '{}' 未解析（内部错误）",
                                type_name(&base_ty)
                            ),
                        })?;
                        match self.result.table_ret_elems.get(&full) {
                            Some(Some(elem)) => elem.clone(),
                            _ => {
                                return Err(SemanticError {
                                    span: *span,
                                    message: format!("下标访问的调用 '{full}' 不是返回表的函数"),
                                })
                            }
                        }
                    }
                    _ => {
                        return Err(SemanticError {
                            span: *span,
                            message: "下标访问仅支持表变量、表字面量或返回表的函数调用".into(),
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
                    TypeSpec::Struct(class_name) => {
                        // 寄存器中的 struct 值不可寻址：构造表达式/方法调用结果直接连用字段
                        // 会在 IR 阶段无法取地址，语义层提前报错（Oracle 方案）。
                        if !is_addressable_expr(base) {
                            return Err(SemanticError {
                                span: *span,
                                message: format!(
                                    "struct 实例 '{class_name}' 的字段访问需要可寻址对象（变量/字段链），"
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
                                message: format!("内部错误：struct '{class_name}' 无信息"),
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
                // 方法调用（M2.1.8）：receiver 是 struct 实例 → 方法转发到
                // `struct名::方法名`（namespace 绑定函数，首参 = 接收者）；
                // receiver 是命名空间路径（a::b / a.b / 导入别名）→ 命名空间函数调用；
                // receiver 是 struct 名（静态调用，如 Point.origin()）→ 命名空间分支覆盖。
                //
                // 命名空间调用判定（M2.1.7 升级）：除了 funcs 前缀存在（原逻辑），
                // 命中「导入视图」（别名可用前缀 / 原前缀）也算命名空间形态——
                // 别名对应的前缀不在 funcs 表中（funcs 键是原路径全名），必须由
                // 视图映射后才能查到。
                let ns_segs: Option<Vec<String>> = match receiver.as_ref() {
                    Expr::Path { segments, .. } => Some(segments.clone()),
                    // Var/FieldAccess 链未绑定（非类实例/非变量）→ 命名空间形态
                    _ => ns_path_segments(receiver, scope, &self.result.globals),
                };
                let import_mapped: Result<Option<Vec<String>>, SemanticError> =
                    match &ns_segs {
                        Some(segs) => self.map_import_prefix(segs, span),
                        None => Ok(None),
                    };
                // `a::b` 路径语法本身就是命名空间形态（无歧义，无需 funcs 前缀佐证）；
                // 点分/单段形态需要 funcs 前缀存在或命中导入视图，避免误判实例方法。
                let is_path_form = matches!(receiver.as_ref(), Expr::Path { .. });
                let is_ns_call = match &import_mapped {
                    Ok(Some(_)) => true,
                    Ok(None) => match &ns_segs {
                        Some(segs) => is_path_form || ns_prefix_exists(&self.result.funcs, segs),
                        None => false,
                    },
                    // 唯一入口违规（原前缀被别名取代）：视为命名空间调用 → 报错
                    Err(_) => true,
                };
                if is_ns_call {
                    let segs = ns_segs.expect("is_ns_call 为真时 ns_segs 必为 Some");
                    // 导入前缀映射（别名 → 原路径；唯一入口违规在此传播报错）
                    let mapped = match import_mapped {
                        Ok(Some(p)) => p,
                        Ok(None) => segs.clone(),
                        Err(e) => return Err(e),
                    };
                    // 全名 = 映射后路径::方法名（如 fmt2.public_api → fmt::public_api）
                    let mut full = mapped;
                    full.push(method.clone());
                    let full = full.join("::");
                    let sig = self
                        .result
                        .funcs
                        .get(&full)
                        .cloned()
                        .ok_or_else(|| SemanticError {
                            span: *span,
                            message: format!("命名空间函数 '{full}' 未定义"),
                        })?;
                    // 可见性校验（M2.1.7）：私有函数仅同命名空间内可调
                    self.check_visibility(&full, &sig, span)?;
                    // 参数个数区间检查（默认值参数）：实参数必须在 [必选数, 总形参数] 内。
                    let required = sig
                        .param_defaults
                        .iter()
                        .take_while(|d| d.is_none())
                        .count();
                    if args.len() < required || args.len() > sig.param_tys.len() {
                        return Err(SemanticError {
                            span: *span,
                            message: format!(
                                "命名空间函数 '{full}' 期望 {} 个参数，实际 {} 个",
                                param_count_desc(required, sig.param_tys.len()),
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
                                    "调用 '{full}' 参数类型不匹配：期望 {}，实际 {}",
                                    type_name(want),
                                    type_name(&at)
                                ),
                            });
                        }
                        self.result.expr_types.insert(addr_of(a), at);
                    }
                    self.result.resolved_calls.insert(addr_of(expr), full);
                    return Ok(sig.ret_ty);
                }
                // ===== M2.1.8：struct 方法转发 =====
                // receiver 必须是 struct 实例（变量/字段链/构造表达式）。方法函数定义
                // 在绑定 struct 名的命名空间里（namespace Point { pub func dist(p: Point) }），
                // `p.dist(args)` 转发为 `Point::dist(p, args)`——沿继承链查找方法函数
                // （子 → 父），方法函数必须 pub（否则私有拦截，与普通命名空间函数一致）。
                // `Point.origin()` 静态调用已由上方命名空间分支覆盖（funcs 前缀
                // "Point::" 命中 → 全名 Point::origin，无首参插入）。
                let recv_ty = self.infer_expr(receiver, scope)?;
                self.result.expr_types.insert(addr_of(receiver), recv_ty.clone());
                let TypeSpec::Struct(struct_name) = &recv_ty else {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "方法调用的对象必须是 struct 实例或 struct 名，实际是 {}",
                            type_name(&recv_ty)
                        ),
                    });
                };
                // 实例方法首参按**引用**传递（by_ptr：函数内字段修改反映到调用方，
                // 与 class 时代的 this 指针一致）→ receiver 必须可寻址（变量/字段链）。
                // 寄存器中的 struct 值（构造表达式/方法链返回值）不可取地址，语义层报错。
                if !is_addressable_expr(receiver) {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "方法调用的对象需要可寻址的 struct 实例（变量/字段链），{} 不可取地址",
                            type_name(&recv_ty)
                        ),
                    });
                }
                // 沿继承链查找方法函数（子 → 父）：funcs 键 `T::method`
                let mut cur: Option<&str> = Some(struct_name.as_str());
                let full: String = loop {
                    let Some(cn) = cur else { break String::new() };
                    let candidate = format!("{cn}::{method}");
                    if self.result.funcs.contains_key(&candidate) {
                        break candidate;
                    }
                    cur = self.result.classes.get(cn).and_then(|i| i.parent.as_deref());
                };
                if full.is_empty() {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "struct '{struct_name}'（含继承链）没有方法 '{method}'：请在 \
                             namespace {struct_name} 中定义 pub func {method}(首参: {struct_name}, ...)"
                        ),
                    });
                }
                let sig = self.result.funcs.get(&full).cloned().expect("上方已确认存在");
                // 方法函数必须 pub（私有 → 拦截）
                self.check_visibility(&full, &sig, span)?;
                // 参数个数：总实参 = [receiver] + args（首参是隐含的接收者）
                let required = sig.param_defaults.iter().take_while(|d| d.is_none()).count();
                let total = sig.param_tys.len();
                let n_args = args.len() + 1;
                if n_args < required || n_args > total {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "方法 '{full}' 期望 {required}-{total} 个参数（含接收者对象），实际 {n_args} 个"
                        ),
                    });
                }
                // 首参类型：receiver 必须是首参类型或其子类（子类实例可调父类方法）
                let first = &sig.param_tys[0];
                if !struct_assignable(&self.result.classes, first, &recv_ty) {
                    return Err(SemanticError {
                        span: *span,
                        message: format!(
                            "方法 '{full}' 首参类型不匹配：期望 {}，实际 {}",
                            type_name(first),
                            type_name(&recv_ty)
                        ),
                    });
                }
                // 其余实参逐个校验
                for (a, want) in args.iter().zip(sig.param_tys.iter().skip(1)) {
                    let at = self.infer_expr(a, scope)?;
                    if !types_match(want, &at, Some(a)) {
                        return Err(SemanticError {
                            span: expr_span_of(a),
                            message: format!(
                                "调用 '{full}' 参数类型不匹配：期望 {}，实际 {}",
                                type_name(want),
                                type_name(&at)
                            ),
                        });
                    }
                    self.result.expr_types.insert(addr_of(a), at);
                }
                self.result.resolved_calls.insert(addr_of(expr), full);
                sig.ret_ty
            }
        };
        Ok(ty)
    }

    /// 递归扫描语句树，寻找「返回动态表」的 return 表达式元素类型。
    ///
    /// 用于表返回预扫描（fixpoint）：遍历语句块（含 if/while/for 嵌套）的 return 语句，
    /// 若其表达式是 table_new_* 调用、调用已知返回动态表的函数，或返回本函数内声明的
    /// 动态表变量（local 表），则返回元素类型。
    fn scan_return_table_elem(&self, stmts: &[Stmt], local: &HashMap<String, TypeSpec>) -> Option<TypeSpec> {
        for s in stmts {
            match s {
                Stmt::Return(r) => {
                    let e = r.expr.as_ref()?;
                    match e {
                        Expr::Call { name, .. } => {
                            // table_new_*：元素类型由名字决定
                            if let Some(elem) = table_new_elem_ty(name) {
                                return Some(elem);
                            }
                            // 调用已知返回动态表的函数
                            match self.result.table_ret_elems.get(name) {
                                Some(Some(elem)) => return Some(elem.clone()),
                                _ => continue,
                            }
                        }
                        // 返回本函数内声明的动态表变量
                        Expr::Var(name) => {
                            if let Some(elem) = local.get(name) {
                                return Some(elem.clone());
                            }
                            continue;
                        }
                        _ => continue,
                    }
                }
                Stmt::If(i) => {
                    if let Some(e) = self.scan_return_table_elem(&i.then_branch, local) {
                        return Some(e);
                    }
                    if let Some(e) = self.scan_return_table_elem(&i.else_branch, local) {
                        return Some(e);
                    }
                }
                Stmt::While(w) => {
                    if let Some(e) = self.scan_return_table_elem(&w.body, local) {
                        return Some(e);
                    }
                }
                Stmt::For(f) => {
                    if let Some(e) = self.scan_return_table_elem(&f.body, local) {
                        return Some(e);
                    }
                }
                Stmt::Switch(s) => {
                    for c in &s.cases {
                        if let Some(e) = self.scan_return_table_elem(&c.body, local) {
                            return Some(e);
                        }
                    }
                    if let Some(e) = self.scan_return_table_elem(&s.default_body, local) {
                        return Some(e);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// 收集语句块内声明的动态表变量 → 元素类型（供表返回预扫描解析 `return 变量`）。
    ///
    /// 只认 `var x = table_new_*()` 或 `var x = <已知返回表的函数>()`；嵌套块一并收集。
    fn collect_local_dyn_tables(&self, stmts: &[Stmt], out: &mut HashMap<String, TypeSpec>) {
        for s in stmts {
            match s {
                Stmt::VarDecl(v) => {
                    if let Expr::Call { name, .. } = &v.init {
                        if let Some(elem) = table_new_elem_ty(name) {
                            out.insert(v.name.clone(), elem);
                        } else if let Some(Some(elem)) = self.result.table_ret_elems.get(name) {
                            out.insert(v.name.clone(), elem.clone());
                        }
                    }
                }
                Stmt::If(i) => {
                    self.collect_local_dyn_tables(&i.then_branch, out);
                    self.collect_local_dyn_tables(&i.else_branch, out);
                }
                Stmt::While(w) => self.collect_local_dyn_tables(&w.body, out),
                Stmt::For(f) => self.collect_local_dyn_tables(&f.body, out),
                Stmt::Switch(s) => {
                    for c in &s.cases {
                        self.collect_local_dyn_tables(&c.body, out);
                    }
                    self.collect_local_dyn_tables(&s.default_body, out);
                }
                _ => {}
            }
        }
    }

    /// 解析「动态表构造/返回」调用的元素类型。
    ///
    /// 支持：table_new_*（元素类型由名字决定）与返回动态表的用户函数
    /// （元素类型由 table_ret_elems 推断；命名空间函数如 str.split 也支持）。
    /// 返回 Err 表示该表达式不是合法的动态表来源。
    fn dynamic_table_elem_ty(
        &self,
        expr: &Expr,
        var_name: &str,
        scope: &HashMap<String, TypeSpec>,
    ) -> Result<TypeSpec, SemanticError> {
        // 裸调用：函数名在表达式上；命名空间调用（MethodCall）：用统一解析拿全名
        let Some(name) = ns_call_full_name(
            &self.result.funcs,
            expr,
            scope,
            &self.ns_stack,
            &self.using_prefixes,
            &self.result.globals,
        ) else {
            return Err(SemanticError {
                span: expr_span_of(expr),
                message: format!(
                    "变量 '{}' 标注 table，初始化必须是表字面量 [...] 或 table_new_* / 返回表的函数调用",
                    var_name
                ),
            });
        };
        // table_new_*：元素类型由名字决定
        if let Some(elem) = table_new_elem_ty(&name) {
            return Ok(elem);
        }
        // 用户函数返回表：查 table_ret_elems
        match self.result.table_ret_elems.get(&name) {
            Some(Some(elem)) => Ok(elem.clone()),
            Some(None) => Err(SemanticError {
                span: expr_span_of(expr),
                message: format!(
                    "函数 '{name}' 返回的表元素类型未知，无法确定 '{}' 的元素类型",
                    var_name
                ),
            }),
            None => Err(SemanticError {
                span: expr_span_of(expr),
                message: format!("函数 '{name}' 未定义或不是返回表的函数"),
            }),
        }
    }

    /// 解析 table_at / 下标访问的表参数的元素类型。
    ///
    /// 表字面量查 tables 元数据；表变量查 table_vars；返回表的函数查 table_ret_elems。
    fn table_arg_elem_ty(
        &self,
        expr: &Expr,
        scope: &HashMap<String, TypeSpec>,
    ) -> Result<TypeSpec, SemanticError> {
        match expr {
            Expr::TableLit { .. } => {
                let info = self.result.tables.get(&addr_of(expr)).ok_or_else(|| SemanticError {
                    span: expr_span_of(expr),
                    message: "找不到表字面量的元素类型元数据".into(),
                })?;
                Ok(info.elem_ty.clone())
            }
            Expr::Var(name) => {
                // 表变量：查 table_vars（定长/动态统一）
                if let Some(info) = self.table_vars.get(&(self.cur_fn.clone(), name.clone())) {
                    return Ok(info.elem_ty.clone());
                }
                // 函数参数（scope 类型为 Table，但无 table_vars 元数据）→ 元素类型未知
                if matches!(scope.get(name), Some(TypeSpec::Named(TyKw::Table))) {
                    return Err(SemanticError {
                        span: expr_span_of(expr),
                        message: format!("表参数 '{}' 的元素类型未知，无法确定 table_at 返回类型", name),
                    });
                }
                Err(SemanticError {
                    span: expr_span_of(expr),
                    message: format!("'{}' 不是表变量", name),
                })
            }
            Expr::Call { name, .. } => {
                // 裸调用/命名空间调用（如 str.split）：统一解析全名后查 table_ret_elems。
                // 内置 table_new_* 也在此识别（与 scan_return_table_elem 的预扫描对齐）：
                // `return table_new_string()` / `table_at(table_new_i64(), 0)` 等
                // 内联调用可直接确定元素类型，无需走用户函数表。
                if let Some(elem) = table_new_elem_ty(name) {
                    return Ok(elem);
                }
                let full = ns_call_full_name(
                    &self.result.funcs,
                    expr,
                    scope,
                    &self.ns_stack,
                    &self.using_prefixes,
                    &self.result.globals,
                )
                .unwrap_or_default();
                match self.result.table_ret_elems.get(&full) {
                    Some(Some(elem)) => Ok(elem.clone()),
                    Some(None) => Err(SemanticError {
                        span: expr_span_of(expr),
                        message: format!("函数 '{full}' 返回的表元素类型未知"),
                    }),
                    None => Err(SemanticError {
                        span: expr_span_of(expr),
                        message: format!("函数 '{full}' 未定义或不是返回表的函数"),
                    }),
                }
            }
            Expr::MethodCall { .. } => {
                // 命名空间方法调用（如 str.split 经 obj.split 形态）：查 table_ret_elems
                let full = ns_call_full_name(
                    &self.result.funcs,
                    expr,
                    scope,
                    &self.ns_stack,
                    &self.using_prefixes,
                    &self.result.globals,
                )
                .unwrap_or_default();
                match self.result.table_ret_elems.get(&full) {
                    Some(Some(elem)) => Ok(elem.clone()),
                    Some(None) => Err(SemanticError {
                        span: expr_span_of(expr),
                        message: format!("函数 '{full}' 返回的表元素类型未知"),
                    }),
                    None => Err(SemanticError {
                        span: expr_span_of(expr),
                        message: format!("函数 '{full}' 未定义或不是返回表的函数"),
                    }),
                }
            }
            _ => Err(SemanticError {
                span: expr_span_of(expr),
                message: "table_at 第 1 个参数必须是表字面量、表变量或返回表的函数".into(),
            }),
        }
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

/// 命名空间路径段提取：把 `tcmsg.error.hello` 的 receiver（FieldAccess 链/Var）
/// 递归拍平为路径段 ["tcmsg","error"]。
///
/// 条件：链上每个标识符都必须是**未绑定**的（非变量），否则它可能是实例字段链
/// （obj.field.method 之类），不属于命名空间。返回 None 表示不是命名空间形态
/// （调用方按 struct 方法转发处理）。
/// 参数个数区间的中文措辞（不含「个」）：必选数 == 总数 → "N"；否则 "N-M"。
/// 与调用点模板「期望 {desc} 个参数」拼接。
fn param_count_desc(required: usize, total: usize) -> String {
    if required == total {
        format!("{total}")
    } else {
        format!("{required}-{total}")
    }
}

fn ns_path_segments(
    expr: &Expr,
    scope: &HashMap<String, TypeSpec>,
    globals: &HashMap<String, TypeSpec>,
) -> Option<Vec<String>> {
    match expr {
        Expr::Var(name) => {
            // 未绑定才是命名空间首段；绑定变量/全局持久变量不是
            if scope.contains_key(name) || globals.contains_key(name) {
                None
            } else {
                Some(vec![name.clone()])
            }
        }
        Expr::FieldAccess { base, field, .. } => {
            let mut segs = ns_path_segments(base, scope, globals)?;
            // 字段名也必须未绑定（类实例字段链的中间段通常绑定在对象上，
            // 但语义上 FieldAccess 的 field 无独立绑定——保守起见：基座必须是
            // 命名空间形态（Var/FieldAccess 链），field 直接追加）
            segs.push(field.clone());
            Some(segs)
        }
        _ => None,
    }
}

/// 命名空间前缀存在判定：funcs 中是否存在以 `路径段::` 开头的函数键。
/// 用于把「未绑定 Var/FieldAccess 链」识别为命名空间（如 tcmsg.hello() 的
/// tcmsg 是命名空间而非变量）。存在即视为命名空间，即使目标全名未注册
/// 也走命名空间调用路径（由下方 get().ok_or_else 报"命名空间函数未定义"）。
fn ns_prefix_exists(funcs: &HashMap<String, FuncSig>, segs: &[String]) -> bool {
    let prefix = format!("{}::", segs.join("::"));
    funcs.keys().any(|k| k.starts_with(&prefix))
}

/// 命名空间调用 → 函数全名解析（表元数据查询用）。
///
/// 输入是调用表达式（`Expr::Call` 的裸名或 `Expr::MethodCall` 的 receiver+method），
/// 输出是注册用的全名（如 "str::split"）。规则与 infer_expr 的命名空间调用
/// 判定一致：receiver 是未绑定 Var（单段）/ FieldAccess 链（点分）/ Path（a::b），
/// 且 funcs 中存在该前缀 → 全名 = 路径段::方法名。
/// 裸调用按当前命名空间前缀补全（命名空间内函数互调返回表时，如 prep::split_lines——
/// table_ret_elems 注册键是全名，见 dynamic_table_elem_ty 的调用约定）。
/// M2.1.7 using：裸调用第三候选查 using 引入的命名空间（唯一候选；多候选歧义返回 None，
/// 由调用方按未定义报错），与 resolve_bare_call 的解析顺序一致。
fn ns_call_full_name(
    funcs: &HashMap<String, FuncSig>,
    expr: &Expr,
    scope: &HashMap<String, TypeSpec>,
    ns_stack: &[String],
    using_prefixes: &[Vec<String>],
    globals: &HashMap<String, TypeSpec>,
) -> Option<String> {
    let (segments, method) = match expr {
        // 裸调用：先查裸名（顶层函数），未命中再按当前命名空间前缀补全，
        // 再查 using 引入的命名空间（M2.1.7 第三候选）。
        // 不做 funcs 校验——内建函数（table_new_i64 等）不注册进 funcs，校验会误杀；
        // 后续查表 table_ret_elems 查不到会由调用方给出正确错误。
        Expr::Call { name, .. } => {
            if funcs.contains_key(name) {
                return Some(name.clone());
            }
            // 当前命名空间前缀补全（逐级外层：tcmsg::error::x → tcmsg::x → x，
            // 与 resolve_bare_call 一致——子命名空间裸调父命名空间函数）
            if !ns_stack.is_empty() {
                for depth in (0..=ns_stack.len()).rev() {
                    let mut segs = ns_stack[..depth].to_vec();
                    segs.push(name.clone());
                    let full = segs.join("::");
                    if funcs.contains_key(&full) {
                        return Some(full);
                    }
                }
            }
            // using 引入的命名空间（唯一候选；多候选歧义 → None）
            let mut hit: Option<String> = None;
            for prefix in using_prefixes {
                let mut segs = prefix.clone();
                segs.push(name.clone());
                let full = segs.join("::");
                if funcs.contains_key(&full) {
                    if hit.is_some() {
                        return None;
                    }
                    hit = Some(full);
                }
            }
            return hit.or_else(|| Some(name.clone()));
        }
        Expr::MethodCall { receiver, method, .. } => {
            // Path（a::b）→ 段；Var/FieldAccess 链 → 未绑定则视为命名空间形态
            let segs = match receiver.as_ref() {
                Expr::Path { segments, .. } => segments.clone(),
                _ => ns_path_segments(receiver, scope, globals)?,
            };
            (segs, method)
        }
        _ => return None,
    };
    if !ns_prefix_exists(funcs, &segments) {
        return None;
    }
    let mut full = segments;
    full.push(method.clone());
    Some(full.join("::"))
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
        Stmt::Break(b) => b.span,
        Stmt::Continue(c) => c.span,
        Stmt::Switch(s) => s.span,
        Stmt::Import(i) => i.span,
        Stmt::Namespace(n) => n.span,
        Stmt::Using(u) => u.span,
        Stmt::Struct(c) => c.span,
        Stmt::FieldAssign(f) => f.span,
        Stmt::IndexAssign(i) => i.span,
    }
}

/// 从表达式中取 span（含字面量：用占位位置）。
fn expr_span_of(expr: &Expr) -> Span {
    match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::StrLit(_) | Expr::CharLit(_) | Expr::BoolLit(_)
        | Expr::TritLit(_) | Expr::Var(_) => {
            // 字面量无 span，用 (0,0) 占位（语义错误主要针对变量/调用，已有 span）
            Span { line: 0, col: 0 }
        }
        Expr::TypeLit { span, .. }
        | Expr::Call { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Ternary { span, .. }
        | Expr::Range { span, .. }
        | Expr::TableLit { span, .. }
        | Expr::Index { span, .. }
        | Expr::TupleLit { span, .. }
        | Expr::FieldAccess { span, .. }
        | Expr::Path { span, .. }
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

/// struct 继承兼容（M2.1.8）：`got` 能否赋给 `want`——
/// got == want，或 got 是 want 的后代（沿 extends 链上升可找到 want）。
///
/// 用于方法转发首参校验：子类实例可调用父 struct 的方法函数
/// （`namespace Parent { pub func dist(p: Parent) }`，`child.dist()` 时首参是 Child）。
fn struct_assignable(
    classes: &HashMap<String, ClassInfo>,
    want: &TypeSpec,
    got: &TypeSpec,
) -> bool {
    let (TypeSpec::Struct(w), TypeSpec::Struct(g)) = (want, got) else {
        return want == got;
    };
    let mut cur = Some(g.as_str());
    while let Some(c) = cur {
        if c == w {
            return true;
        }
        cur = classes.get(c).and_then(|i| i.parent.as_deref());
    }
    false
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
        // 平衡三进制 trit 字面量适配（M4 补齐）：
        // - `var t: trit = zero` → TritLit(0) 直接匹配；
        // - `var t: trit = true` / `= false` → BoolLit 按目标类型适配为 TritLit(+1)/(-1)；
        // - 裸 `true`（无 trit 标注）保持 bool。
        Some(Expr::TritLit(_)) => matches!(want, TypeSpec::Named(TyKw::Trit)),
        Some(Expr::BoolLit(_)) => matches!(want, TypeSpec::Named(TyKw::Trit)),
        // 表字面量可传给 table 参数（元素类型与长度由字面量布局元数据记录，
        // IR/解释路径按布局访问；如 tcmsg::error.no_file(["zh-cn","en-us"])）
        Some(Expr::TableLit { .. }) => matches!(want, TypeSpec::Named(TyKw::Table)),
        _ => false,
    }
}

/// 是否为数字类型（整数或浮点）。
fn is_number(t: &TypeSpec) -> bool {
    t.is_number()
}

/// table_new_* 内置函数名 → 元素类型（动态表构造）。
fn table_new_elem_ty(name: &str) -> Option<TypeSpec> {
    match name {
        "table_new_i64" => Some(TypeSpec::Named(TyKw::I64)),
        "table_new_f64" => Some(TypeSpec::Named(TyKw::F64)),
        "table_new_string" => Some(TypeSpec::Named(TyKw::Str)),
        "table_new_bool" => Some(TypeSpec::Named(TyKw::Bool)),
        _ => None,
    }
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

/// 是否为整数或字符字面量（switch 区间 pattern 的两端只允许这两种）。
///
/// 支持负数（`-3` 由一元负号包裹）；浮点明确排除（区间语义含入模糊）。
fn is_int_or_char_literal(expr: &Expr) -> bool {
    match expr {
        Expr::IntLit(_) | Expr::CharLit(_) => true,
        Expr::Unary { op: UnaryOp::Neg, operand, .. } => matches!(operand.as_ref(), Expr::IntLit(_)),
        _ => false,
    }
}

/// 比较两个整数/字符字面量的大小（switch 区间 `start < end` 校验）。
///
/// 返回负数/零/正数（Rust `cmp` 语义）；负数区间两端均取负后比较。
fn literal_cmp(a: &Expr, b: &Expr) -> i64 {
    let av = literal_value(a);
    let bv = literal_value(b);
    av - bv
}

/// 取整数/字符字面量的数值（字符按 UTF-32 值；负数取负）。
fn literal_value(expr: &Expr) -> i64 {
    match expr {
        Expr::IntLit(v) => *v,
        Expr::CharLit(c) => *c as i64,
        Expr::Unary { op: UnaryOp::Neg, operand, .. } => match operand.as_ref() {
            Expr::IntLit(v) => -v,
            _ => 0,
        },
        _ => 0,
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
        TypeSpec::Struct(name) => Box::leak(name.clone().into_boxed_str()),
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
    fn 命名空间函数以全名注册且路径调用解析() {
        // tcmsg::error.no_file(...) → 全名 "tcmsg::error::no_file" 注册并解析
        let sem = analyze_src(
            "namespace tcmsg {\n\
             \x20   namespace error {\n\
             \x20       pub func no_file(langs: table) -> string {\n\
             \x20           return \"file not found\"\n\
             \x20       }\n\
             \x20   }\n\
             }\n\
             func main() {\n\
             \x20   var m = tcmsg::error.no_file([\"zh-cn\",\"en-us\"])\n\
             }\n",
        )
        .expect("命名空间语义分析应通过");
        // 全名注册
        assert!(sem.funcs.contains_key("tcmsg::error::no_file"), "命名空间函数应以全名注册");
        // 调用点解析记录（MethodCall 表达式地址 → 全名）
        assert!(
            sem.resolved_calls.values().any(|v| v == "tcmsg::error::no_file"),
            "命名空间路径调用应记录全名，实际 {:#?}",
            sem.resolved_calls
        );
    }

    #[test]
    fn 命名空间内函数裸调用按前缀补全() {
        // tcmsg::error 内两个函数互调：helper() 裸调用 → 解析为 tcmsg::error::helper
        let sem = analyze_src(
            "namespace tcmsg.error {\n\
             \x20   pub func no_file(langs: table) -> string {\n\
             \x20       return helper()\n\
             \x20   }\n\
             \x20   func helper() -> string {\n\
             \x20       return \"x\"\n\
             \x20   }\n\
             }\n\
             func main() {\n\
             \x20   var m = tcmsg::error.no_file([\"zh-cn\"])\n\
             }\n",
        )
        .expect("命名空间内裸调用应通过前缀补全");
        assert!(sem.funcs.contains_key("tcmsg::error::no_file"));
        assert!(sem.funcs.contains_key("tcmsg::error::helper"));
    }

    #[test]
    fn 默认值参数省略实参合法且类型校验() {
        // 默认值参数：省略实参的调用通过区间检查；签名登记默认值表达式。
        let sem = analyze_src(
            "func greet(name: string, prefix: string = \"Hello\") -> string {\n\
             \x20   return prefix + \", \" + name\n\
             }\n\
             func main() {\n\
             \x20   var a = greet(\"World\")            // 省略默认参数\n\
             \x20   var b = greet(\"World\", \"Hi\")     // 显式传参\n\
             \x20   var c = greet(\"World\", \"Hi\", \"x\") // 超参 → 报错\n\
             }\n",
        );
        // 超参报错（c 行）：但 analyze 一次成功则说明 a/b 合法；这里单独断言 c 失败
        assert!(sem.is_err(), "超过总形参数应报错，实际通过：{sem:?}");
        // 合法路径：只省略默认参数的调用
        let sem = analyze_src(
            "func greet(name: string, prefix: string = \"Hello\") -> string {\n\
             \x20   return prefix + \", \" + name\n\
             }\n\
             func main() {\n\
             \x20   var a = greet(\"World\")\n\
             \x20   var b = greet(\"World\", \"Hi\")\n\
             }\n",
        )
        .expect("省略默认参数应通过区间检查");
        // 签名登记默认值：第 2 个参数带默认值
        let sig = sem.funcs.get("greet").expect("greet 应注册");
        assert_eq!(sig.param_defaults.len(), 2);
        assert!(sig.param_defaults[0].is_none(), "必选参数无默认值");
        assert!(sig.param_defaults[1].is_some(), "可选参数有默认值");
    }

    #[test]
    fn 默认值参数限制规则() {
        // 可选参数必须连续排在必选参数之后
        expect_err(
            "func f(a: i64 = 1, b: i64) {\n}\nfunc main() {\n}\n",
            "必须连续排在必选参数之后",
        );
        // 默认值必须是字面量（变量引用 → 报错）
        expect_err(
            "func f(a: i64 = x) {\n}\nfunc main() {\n  var x = 1\n}\n",
            "默认值必须是字面量",
        );
        // 非空表字面量默认值 → 报错
        expect_err(
            "func f(t: table = [1, 2]) {\n}\nfunc main() {\n}\n",
            "默认值必须是字面量",
        );
        // 默认值类型不匹配（字符串默认值给 i64 参数）→ 报错
        expect_err(
            "func f(a: i64 = \"x\") {\n}\nfunc main() {\n}\n",
            "默认值类型不匹配",
        );
        // 空表 [] 默认值 → 合法（table 参数）
        analyze_src(
            "func f(t: table = []) {\n}\nfunc main() {\n}\n",
        )
        .expect("空表默认值应合法");
    }

    #[test]
    fn 命名空间函数未定义报错() {
        expect_err(
            "namespace tcmsg {\n\
             \x20   func main() {\n\
             \x20       var m = tcmsg::error.no_file()\n\
             \x20   }\n\
             }\n",
            "命名空间函数 'tcmsg::error::no_file' 未定义",
        );
    }

    #[test]
    fn 命名空间路径不能作为值使用() {
        expect_err(
            "func main() {\n\
             \x20   var x = tcmsg::error\n\
             }\n",
            "不能作为值使用",
        );
    }

    #[test]
    fn 命名空间内函数重复定义报错() {
        expect_err(
            "namespace tcmsg {\n\
             \x20   func dup() -> string {\n\
             \x20       return \"a\"\n\
             \x20   }\n\
             \x20   func dup() -> string {\n\
             \x20       return \"b\"\n\
             \x20   }\n\
             }\n",
            "重复定义",
        );
    }

    #[test]
    fn 单段命名空间调用解析为全名() {
        // tcmsg.hello()：语法层是 MethodCall { receiver: Var("tcmsg") }，
        // 语义层应把未绑定 Var + funcs 前缀命中识别为命名空间调用 → tcmsg::hello。
        let sem = analyze_src(
            "namespace tcmsg {\n\
             \x20   pub func hello() -> string {\n\
             \x20       return \"x\"\n\
             \x20   }\n\
             }\n\
             func main() {\n\
             \x20   var m = tcmsg.hello()\n\
             }\n",
        )
        .expect("单段命名空间调用应通过语义分析");
        assert!(sem.funcs.contains_key("tcmsg::hello"));
        assert!(
            sem.resolved_calls.values().any(|v| v == "tcmsg::hello"),
            "单段命名空间调用应记录全名 tcmsg::hello，实际 {:#?}",
            sem.resolved_calls
        );
    }

    #[test]
    fn 点分命名空间调用解析为全名() {
        // tcmsg.error.no_file()：语法层是 MethodCall { receiver: FieldAccess{Var(tcmsg).error} }，
        // 语义层应把未绑定 FieldAccess 链 + funcs 前缀命中识别为命名空间调用。
        let sem = analyze_src(
            "namespace tcmsg {\n\
             \x20   namespace error {\n\
             \x20       pub func no_file() -> string {\n\
             \x20           return \"x\"\n\
             \x20       }\n\
             \x20   }\n\
             }\n\
             func main() {\n\
             \x20   var m = tcmsg.error.no_file()\n\
             }\n",
        )
        .expect("点分命名空间调用应通过语义分析");
        assert!(sem.funcs.contains_key("tcmsg::error::no_file"));
        assert!(
            sem.resolved_calls.values().any(|v| v == "tcmsg::error::no_file"),
            "点分命名空间调用应记录全名 tcmsg::error::no_file，实际 {:#?}",
            sem.resolved_calls
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
    fn struct名与函数名冲突报错() {
        expect_err(
            r#"
            func Point() -> i64 {
                return 1
            }
            struct Point {
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
            struct A extends B {
                var a: i64
            }
            struct B extends A {
                var b: i64
            }
            func main() {
                println(1)
            }
            "#,
            "struct 继承形成环",
        );
    }

    #[test]
    fn 子类字段拍平与方法函数转发() {
        // M2.1.8：struct 纯数据（只字段），逻辑 = 绑定 struct 名的命名空间函数；
        // d.speak() 转发到 Dog::speak(d)（沿继承链）。
        let sem = analyze_src(
            r#"
            struct Animal {
                var name: string
                var age: i64
            }
            struct Dog extends Animal {
                var breed: string
            }
            namespace Animal {
                pub func speak(a: Animal) -> string {
                    return a.name + " makes a sound"
                }
            }
            namespace Dog {
                pub func speak(d: Dog) -> string {
                    return d.name + " barks"
                }
                pub func info(d: Dog) -> string {
                    return "I am a " + d.breed
                }
            }
            func main() {
                var d = Dog("Rex", 3, "Golden")
                println(d.speak())
                println(d.info())
            }
            "#,
        )
        .expect("应当通过语义检查");
        // 字段拍平：父 struct 字段在前，子 struct 字段在后（顺序即 LLVM 结构体字段序）
        let dog = &sem.classes["Dog"];
        assert_eq!(dog.fields.len(), 3);
        assert_eq!(dog.field_index["name"], 0);
        assert_eq!(dog.field_index["age"], 1);
        assert_eq!(dog.field_index["breed"], 2);
        // 方法 = 绑定 struct 名的命名空间函数（全名注册，供 d.speak() 转发）
        assert!(sem.funcs.contains_key("Animal::speak"));
        assert!(sem.funcs.contains_key("Dog::speak"));
        assert!(sem.funcs.contains_key("Dog::info"));
        // 调用点解析：d.speak() → Dog::speak、d.info() → Dog::info（resolved_calls 全名）
        assert!(
            sem.resolved_calls.values().any(|v| v == "Dog::speak"),
            "子类实例方法应转发到 Dog::speak"
        );
        assert!(
            sem.resolved_calls.values().any(|v| v == "Dog::info"),
            "子类实例方法应转发到 Dog::info"
        );
    }

    #[test]
    fn 方法函数重复定义报错() {
        // 方法 = 命名空间函数：namespace A 内同名 pub func 重复 → 报错
        expect_err(
            r#"
            struct A {
                var x: i64
            }
            namespace A {
                pub func f(a: A) {}
                pub func f(a: A) {}
            }
            func main() {
                println(1)
            }
            "#,
            "重复定义",
        );
    }

    #[test]
    fn 方法函数this废弃() {
        // M2.1.8：this 不再是关键字/隐式对象——方法函数用显式首参（接收者），
        // this 是普通标识符，未声明即报「未声明的变量」。
        let sem = analyze_src(
            r#"
            struct Counter {
                var count: i64
            }
            namespace Counter {
                pub func inc(c: Counter) {
                    c.count = c.count + 1
                }
                pub func get(c: Counter) -> i64 {
                    return c.count
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
        // 方法函数以全名注册进 funcs 表
        assert!(sem.funcs.contains_key("Counter::inc"));
        assert!(sem.funcs.contains_key("Counter::get"));
        assert_eq!(
            sem.funcs["Counter::get"].ret_ty,
            TypeSpec::Named(TyKw::I64)
        );
        // 调用转发记录全名
        assert!(sem.resolved_calls.values().any(|v| v == "Counter::inc"));
        assert!(sem.resolved_calls.values().any(|v| v == "Counter::get"));
    }

    #[test]
    fn this作为普通标识符报未声明() {
        // this 已废弃（M2.1.8）：方法函数体内引用 this 视为未声明变量
        expect_err(
            r#"
            struct Counter {
                var count: i64
            }
            namespace Counter {
                pub func bad(c: Counter) {
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
    fn 无接收者方法函数经实例调用报错() {
        // M2.1.8：方法 = 命名空间函数，实例转发要求首参接收 receiver。
        // `make` 无首参（静态风格）→ `c.make()` 转发时实参多出 receiver → 报错。
        expect_err(
            r#"
            struct Counter {
                var count: i64
            }
            namespace Counter {
                pub func make() -> i64 {
                    return 1
                }
            }
            func main() {
                var c = Counter(0)
                println(c.make())
            }
            "#,
            "含接收者对象",
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

    // ---------- M4 补齐：trit 平衡三进制类型规则 ----------

    #[test]
    fn trit字面量与类型标注() {
        // zero → trit；true/false 适配 trit（标注场景）；裸 true 保持 bool
        analyze_src(
            r#"
            func main() {
                var t: trit = zero
                var p: trit = true
                var n: trit = false
                var b = true
            }
            "#,
        )
        .expect("trit 字面量与标注应通过");
    }

    #[test]
    fn trit与bool互不混淆() {
        // 裸 true 仍是 bool：bool 变量不能赋 trit 值
        analyze_src(
            r#"
            func main() {
                var t: trit = zero
                var b: bool = t
            }
            "#,
        )
        .expect_err("bool 变量不能赋 trit 值（类型不匹配）");
        // trit 变量不能赋 bool 值
        analyze_src(
            r#"
            func main() {
                var b: bool = true
                var t: trit = b
            }
            "#,
        )
        .expect_err("trit 变量不能赋 bool 值（类型不匹配）");
    }

    #[test]
    fn trit三值逻辑运算() {
        // Kleene 三值逻辑：trit && trit → trit；trit || trit → trit；!trit → trit
        analyze_src(
            r#"
            func main() {
                var a: trit = true
                var b: trit = zero
                var c = a && b
                var d = a || b
                var e = !a
                var eq = (a == b)
                var lt = (a < b)
            }
            "#,
        )
        .expect("trit 三值逻辑/比较应通过");
        // trit && bool 不允许（两侧必须同为 trit 或同为 bool）
        analyze_src(
            r#"
            func main() {
                var a: trit = true
                var b = true
                var c = a && b
            }
            "#,
        )
        .expect_err("trit 与 bool 混合逻辑运算应报错");
    }

    #[test]
    fn trit算术与i64互转() {
        // trit ± * trit → trit；trit + i64 → i64；比较 trit vs i64 → bool
        analyze_src(
            r#"
            func main() {
                var a: trit = true
                var b: trit = zero
                var c = a + b
                var d: trit = a * b
                var e = a + 5
                var f = (a == 1)
                var s = to_string(a)
                var p: trit = parse_trit("-1")
            }
            "#,
        )
        .expect("trit 算术/混合/转换应通过");
        // trit 除法不允许
        analyze_src(
            r#"
            func main() {
                var a: trit = true
                var b: trit = zero
                var c = a / b
            }
            "#,
        )
        .expect_err("trit 除法应报错");
        // to_string(bool) 仍报错（不因 trit 放宽）
        analyze_src(
            r#"
            func main() {
                var b = true
                var s = to_string(b)
            }
            "#,
        )
        .expect_err("to_string(bool) 应报错（trit 放宽不影响 bool）");
    }

    // ---------- M4 补齐：表下标赋值类型校验 ----------

    #[test]
    fn 表下标赋值类型匹配() {
        // 定长表字面量 + 下标赋值（元素类型 i64）
        analyze_src(
            r#"
            func main() {
                var arr = [1, 2, 3]
                arr[0] = 9
                arr[1] += 1
            }
            "#,
        )
        .expect("定长表下标赋值应通过");
        // 动态表 + 下标赋值
        analyze_src(
            r#"
            func main() {
                var t = table_new_i64()
                table_push(t, 1)
                t[0] = 5
            }
            "#,
        )
        .expect("动态表下标赋值应通过");
        // 字符串表 + 字符串元素
        analyze_src(
            r#"
            func main() {
                var t = table_new_string()
                table_push(t, "a")
                t[0] = "b"
            }
            "#,
        )
        .expect("字符串表下标赋值应通过");
    }

    #[test]
    fn 表下标赋值类型不匹配报错() {
        // 元素类型不匹配：i64 表赋 string
        analyze_src(
            r#"
            func main() {
                var t = table_new_i64()
                table_push(t, 1)
                t[0] = "x"
            }
            "#,
        )
        .expect_err("i64 表赋 string 应报错");
        // 下标非整数
        analyze_src(
            r#"
            func main() {
                var t = table_new_i64()
                table_push(t, 1)
                t["x"] = 1
            }
            "#,
        )
        .expect_err("非整数下标应报错");
        // 非表对象下标赋值
        analyze_src(
            r#"
            func main() {
                var x = 5
                x[0] = 1
            }
            "#,
        )
        .expect_err("非表对象下标赋值应报错");
    }

    #[test]
    fn 动态表table_new零参数返回table() {
        // table_new_*：零参数创建空动态表，返回 table 类型
        let sem = analyze_src(
            r#"
            func main() {
                var t = table_new_i64()
            }
            "#,
        )
        .expect("table_new_i64 应通过");
        // 动态表变量登记到 table_vars，元素类型 i64、动态标志 true
        let info = sem.table_vars.get(&("main".to_string(), "t".to_string())).expect("应有表元数据");
        assert_eq!(info.elem_ty, TypeSpec::Named(TyKw::I64));
        assert!(info.dynamic, "table_new 创建的是动态表");
    }

    #[test]
    fn return内联table_new识别元素类型() {
        // M4 补齐回归：`return table_new_string()` 内联调用应被 table_arg_elem_ty 识别
        //（此前只覆盖预扫描路径，第二遍检查缺 table_new_* 分支报"未定义或不是返回表的函数"）
        analyze_src(
            r#"
            func make() -> table {
                return table_new_string()
            }
            func main() {
                var t = make()
                var n = len(t)
            }
            "#,
        )
        .expect("return table_new_string() 应通过");
    }

    #[test]
    fn 动态表标注table接受table_new调用() {
        // 标注 table 的变量可用 table_new_* 初始化（此前只允许字面量）
        analyze_src(
            r#"
            func main() {
                var t: table = table_new_string()
                table_push(t, "hi")
            }
            "#,
        )
        .expect("标注 table + table_new_string 初始化应通过");
    }

    #[test]
    fn 动态表table_new错误参数个数报错() {
        expect_err(
            r#"
            func main() {
                var t = table_new_i64(5)
            }
            "#,
            "期望 0 个参数",
        );
    }

    #[test]
    fn 动态表table_push类型匹配通过() {
        analyze_src(
            r#"
            func main() {
                var t = table_new_f64()
                table_push(t, 1.5)
                table_push(t, 2.0)
            }
            "#,
        )
        .expect("f64 表推入浮点字面量应通过");
    }

    #[test]
    fn 动态表table_push元素类型不匹配报错() {
        // 元素类型与表不一致：i64 表推入字符串
        expect_err(
            r#"
            func main() {
                var t = table_new_i64()
                table_push(t, "oops")
            }
            "#,
            "元素类型不匹配",
        );
    }

    #[test]
    fn 动态表table_push非表变量报错() {
        expect_err(
            r#"
            func main() {
                var t = table_new_i64()
                table_push(table_new_i64(), 5)
            }
            "#,
            "必须是表变量",
        );
    }

    #[test]
    fn 动态表table_at返回元素类型() {
        let sem = analyze_src(
            r#"
            func main() {
                var t = table_new_bool()
                var b: bool = table_at(t, 0)
            }
            "#,
        )
        .expect("bool 表 table_at 返回 bool 应通过");
        assert!(sem.table_vars.contains_key(&("main".to_string(), "t".to_string())));
    }

    #[test]
    fn 动态表table_at下标非整数报错() {
        expect_err(
            r#"
            func main() {
                var t = table_new_i64()
                var x = table_at(t, "a")
            }
            "#,
            "下标必须是整数",
        );
    }

    #[test]
    fn 动态表下标与遍历用元素类型() {
        // 动态表与定长表统一支持 t[i] 与 for-in（元素类型查 table_vars）
        analyze_src(
            r#"
            func main() {
                var t = table_new_i64()
                table_push(t, 1)
                table_push(t, 2)
                var a = t[0]
                for e in t {
                    var b = e
                }
            }
            "#,
        )
        .expect("动态表 t[i] 与 for-in 应通过");
    }

    #[test]
    fn 函数返回动态表元素类型传播() {
        // make_list 返回 table_new_i64，调用方 var t = make_list(3) 继承 i64 元素类型
        let sem = analyze_src(
            r#"
            func make_list(n: i64) -> table {
                var t = table_new_i64()
                var i = 0
                while i < n {
                    table_push(t, i)
                    i++
                }
                return t
            }
            func main() {
                var t = make_list(3)
                table_push(t, 99)
            }
            "#,
        )
        .expect("返回表的函数 + 调用方继承元素类型应通过");
        let info = sem.table_vars.get(&("main".to_string(), "t".to_string())).expect("应有表元数据");
        assert_eq!(info.elem_ty, TypeSpec::Named(TyKw::I64));
        assert!(info.dynamic);
        assert_eq!(sem.table_ret_elems.get("make_list"), Some(&Some(TypeSpec::Named(TyKw::I64))));
    }

    #[test]
    fn 函数返回动态表类型不匹配报错() {
        // 函数声明返回 table 但 return 非动态表来源 → 报错
        expect_err(
            r#"
            func bad() -> table {
                var x = 1
                return x
            }
            func main() {}
            "#,
            "return 类型不匹配",
        );
    }

    // ---------- switch 模式匹配增强（规划 switch-pattern-matching） ----------

    #[test]
    fn switch多值case通过() {
        // 多值 `case 1, 2:` —— 任一相等即命中，语义层逐个校验
        analyze_src(
            r#"
            func main() {
                var x: i64 = 2
                switch x {
                    case 1, 2:
                        println("一二")
                    default:
                        println("其他")
                }
            }
            "#,
        )
        .expect("多值 case 应通过");
    }

    #[test]
    fn switch区间case通过() {
        // 区间 `case 3..7:` —— 整数区间，start < end
        analyze_src(
            r#"
            func main() {
                var x: i64 = 5
                switch x {
                    case 3..7:
                        println("三四五六")
                    default:
                        println("其他")
                }
            }
            "#,
        )
        .expect("整数区间 case 应通过");
    }

    #[test]
    fn switch字符区间case通过() {
        // 字符区间 `case 'a'..'e':` —— 字符 subject + 字符区间
        analyze_src(
            r#"
            func main() {
                var c: char = 'b'
                switch c {
                    case 'a'..'e':
                        println("元音前")
                    default:
                        println("其他")
                }
            }
            "#,
        )
        .expect("字符区间 case 应通过");
    }

    #[test]
    fn switch守卫when通过() {
        // 守卫 `case 8 when flag:` —— 值匹配且守卫为真才进入
        analyze_src(
            r#"
            func main() {
                var x: i64 = 8
                var flag: bool = true
                switch x {
                    case 8 when flag:
                        println("八且 flag")
                    default:
                        println("其他")
                }
            }
            "#,
        )
        .expect("when 守卫 case 应通过");
    }

    #[test]
    fn switch多值与区间与守卫组合通过() {
        // 多值 + 区间 + 守卫自由组合：`case 1, 3..5 when flag:`
        analyze_src(
            r#"
            func main() {
                var x: i64 = 4
                var flag: bool = true
                switch x {
                    case 1, 3..5 when flag:
                        println("命中")
                    default:
                        println("其他")
                }
            }
            "#,
        )
        .expect("多值+区间+守卫组合应通过");
    }

    #[test]
    fn switch浮点区间报错() {
        // 浮点区间：边界含入语义模糊，明确不支持
        expect_err(
            r#"
            func main() {
                var x: f64 = 1.5
                switch x {
                    case 1.0..2.5:
                        println("x")
                }
            }
            "#,
            "区间两端必须是整数或字符字面量",
        );
    }

    #[test]
    fn switch区间start大于end报错() {
        // start > end：空区间无意义
        expect_err(
            r#"
            func main() {
                var x: i64 = 5
                switch x {
                    case 7..3:
                        println("x")
                }
            }
            "#,
            "start < end",
        );
    }

    #[test]
    fn switch区间与subject类型不匹配报错() {
        // 整数区间用在字符串 subject 上 → 类型不匹配
        expect_err(
            r#"
            func main() {
                var s: string = "hi"
                switch s {
                    case 1..3:
                        println("x")
                }
            }
            "#,
            "区间类型与 switch 对象类型",
        );
    }

    #[test]
    fn switch静态类型上类型匹配报错() {
        // 类型匹配 `case string:` 仅宽类型/动态容器对象上有意义；
        // 当前 switch 对象是静态类型（i64）→ 报错
        expect_err(
            r#"
            func main() {
                var x: i64 = 1
                switch x {
                    case string:
                        println("字符串")
                }
            }
            "#,
            "类型匹配",
        );
    }

    #[test]
    fn switch守卫非布尔报错() {
        // when 守卫必须是布尔表达式
        expect_err(
            r#"
            func main() {
                var x: i64 = 8
                switch x {
                    case 8 when 42:
                        println("x")
                }
            }
            "#,
            "when 守卫必须是布尔表达式",
        );
    }

    #[test]
    fn switch多值中非字面量报错() {
        // 多值中的每个值都必须是字面量（变量不允许）
        expect_err(
            r#"
            func main() {
                var x: i64 = 2
                var y: i64 = 3
                switch x {
                    case 1, y:
                        println("x")
                }
            }
            "#,
            "case 值必须是字面量",
        );
    }

    #[test]
    fn switch多值重复报错() {
        // 多值内部重复：`case 1, 1:` → 重复 case 检测
        expect_err(
            r#"
            func main() {
                var x: i64 = 2
                switch x {
                    case 1, 1:
                        println("x")
                }
            }
            "#,
            "重复的 case 值",
        );
    }

    // ==================== M2.1.7 单文件命名空间（pub/using/别名） ====================

    /// M2.1.7 辅助：写临时文件 + import 展开 + 语义分析。
    /// `files` 是被导入文件（名 → 内容），主源码作为 `main` 传入。
    /// 目录用 pid + 纳秒时间戳，保证并行测试互不冲突。
    fn expand_analyze(main: &str, files: &[(&str, &str)]) -> Result<SemanticResult, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX 纪元")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("tie-ns-test-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        for (name, src) in files {
            std::fs::write(dir.join(name), src).expect("写被导入文件失败");
        }
        let tokens = tokenize(main).expect("词法分析失败");
        let program = parse_program(&tokens).expect("语法分析失败");
        let expanded = crate::imports::expand_imports(program, &dir).map_err(|e| e.message)?;
        let result = analyze(&expanded).map_err(|e| e.message)?;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(result)
    }

    #[test]
    fn pub函数跨命名空间调用放行() {
        // pub func：顶层 main 跨命名空间调用通过（M2.1.7 显式导出）
        let sem = expand_analyze(
            "import \"./tools.tie\"\n\
             func main() {\n\
             \x20   var s = fmt.public_api()\n\
             \x20   println(s)\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   pub func public_api() -> string {\n\
                 \x20       return \"ok\"\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect("pub 函数跨命名空间调用应通过");
        assert!(sem.funcs.contains_key("fmt::public_api"));
        assert!(
            sem.resolved_calls.values().any(|v| v == "fmt::public_api"),
            "调用应记录全名 fmt::public_api"
        );
    }

    #[test]
    fn 私有函数跨命名空间调用拦截() {
        // 默认私有：顶层 main 跨命名空间调用命名空间内私有函数 → 报错
        let err = expand_analyze(
            "import \"./tools.tie\"\n\
             func main() {\n\
             \x20   var s = fmt.secret()\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   func secret() -> string {\n\
                 \x20       return \"x\"\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect_err("私有函数跨命名空间调用应报错");
        assert!(
            err.contains("私有函数") && err.contains("fmt::secret"),
            "错误应说明私有函数：{err}"
        );
    }

    #[test]
    fn 同命名空间私有互调放行() {
        // 命名空间内私有函数互调（裸调用）：同一命名空间内不拦截
        expand_analyze(
            "import \"./tools.tie\"\n\
             func main() {\n\
             \x20   var s = fmt.entry()\n\
             \x20   println(s)\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   pub func entry() -> string {\n\
                 \x20       return helper()\n\
                 \x20   }\n\
                 \x20   func helper() -> string {\n\
                 \x20       return \"h\"\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect("同命名空间私有互调应放行");
    }

    #[test]
    fn using引入后裸调用补全() {
        // using fmt; 之后 fmt 的公有函数可裸名调用（第三候选）
        let sem = expand_analyze(
            "import \"./tools.tie\"\n\
             using fmt;\n\
             func main() {\n\
             \x20   var s = public_api()\n\
             \x20   println(s)\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   pub func public_api() -> string {\n\
                 \x20       return \"ok\"\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect("using 引入后裸调用应补全");
        assert!(
            sem.resolved_calls.values().any(|v| v == "fmt::public_api"),
            "裸调用应记录全名 fmt::public_api"
        );
    }

    #[test]
    fn using目标未导入报错() {
        // using 目标不是任何 import 引入的命名空间/别名 → 报错
        let err = expand_analyze(
            "import \"./tools.tie\"\n\
             using unknown;\n\
             func main() {\n\
             \x20   println(1)\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   pub func public_api() -> string {\n\
                 \x20       return \"ok\"\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect_err("using 目标未导入应报错");
        assert!(err.contains("using 目标 'unknown' 未导入"), "错误消息：{err}");
    }

    #[test]
    fn using重复引入报错() {
        // 同一命名空间重复 using → 报错
        let err = expand_analyze(
            "import \"./tools.tie\"\n\
             using fmt;\n\
             using fmt;\n\
             func main() {\n\
             \x20   println(1)\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   pub func public_api() -> string {\n\
                 \x20       return \"ok\"\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect_err("重复 using 应报错");
        assert!(err.contains("重复 using"), "错误消息：{err}");
    }

    #[test]
    fn using多候选歧义报错() {
        // 两个 using 命名空间都含同名函数 → 裸调用歧义报错
        let err = expand_analyze(
            "import \"./a.tie\"\n\
             import \"./b.tie\"\n\
             using fa;\n\
             using fb;\n\
             func main() {\n\
             \x20   var s = greet()\n\
             }\n",
            &[
                (
                    "a.tie",
                    "namespace fa {\n\
                     \x20   pub func greet() -> string {\n\
                     \x20       return \"a\"\n\
                     \x20   }\n\
                     }\n",
                ),
                (
                    "b.tie",
                    "namespace fb {\n\
                     \x20   pub func greet() -> string {\n\
                     \x20       return \"b\"\n\
                     \x20   }\n\
                     }\n",
                ),
            ],
        )
        .expect_err("裸调用歧义应报错");
        assert!(err.contains("有歧义"), "错误消息：{err}");
    }

    #[test]
    fn import别名唯一入口() {
        // import as 别名：原前缀被屏蔽（唯一入口），必须用别名访问
        let sem = expand_analyze(
            "import \"./tools.tie\" as f2;\n\
             func main() {\n\
             \x20   var s = f2.public_api()\n\
             \x20   println(s)\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   pub func public_api() -> string {\n\
                 \x20       return \"ok\"\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect("别名访问应通过");
        assert!(
            sem.resolved_calls.values().any(|v| v == "fmt::public_api"),
            "别名调用应映射回全名 fmt::public_api"
        );
    }

    #[test]
    fn import别名原前缀调用报错() {
        // 声明别名后仍用原前缀 → 唯一入口违规报错
        let err = expand_analyze(
            "import \"./tools.tie\" as f2;\n\
             func main() {\n\
             \x20   var s = fmt.public_api()\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   pub func public_api() -> string {\n\
                 \x20       return \"ok\"\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect_err("原前缀调用应报唯一入口违规");
        assert!(
            err.contains("已被别名") && err.contains("f2"),
            "错误应说明唯一入口与别名：{err}"
        );
    }

    #[test]
    fn import别名嵌套命名空间访问() {
        // 别名 + 嵌套命名空间：f2.inner.deep() → fmt::inner::deep
        let sem = expand_analyze(
            "import \"./tools.tie\" as f2;\n\
             func main() {\n\
             \x20   var s = f2.inner.deep()\n\
             \x20   println(s)\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   namespace inner {\n\
                 \x20       pub func deep() -> string {\n\
                 \x20           return \"d\"\n\
                 \x20       }\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect("别名 + 嵌套命名空间调用应通过");
        assert!(
            sem.resolved_calls.values().any(|v| v == "fmt::inner::deep"),
            "应映射回全名 fmt::inner::deep"
        );
    }

    #[test]
    fn using嵌套命名空间路径() {
        // using fmt.inner; → inner 命名空间公有函数可裸调用
        let sem = expand_analyze(
            "import \"./tools.tie\"\n\
             using fmt.inner;\n\
             func main() {\n\
             \x20   var s = deep()\n\
             \x20   println(s)\n\
             }\n",
            &[(
                "tools.tie",
                "namespace fmt {\n\
                 \x20   namespace inner {\n\
                 \x20       pub func deep() -> string {\n\
                 \x20           return \"d\"\n\
                 \x20       }\n\
                 \x20   }\n\
                 }\n",
            )],
        )
        .expect("using 嵌套命名空间裸调用应通过");
        assert!(
            sem.resolved_calls.values().any(|v| v == "fmt::inner::deep"),
            "裸调用应记录全名 fmt::inner::deep"
        );
    }
}
