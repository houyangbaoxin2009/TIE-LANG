//! 中端：AST → LLVM IR 文本生成。
//!
//! 职责：把语义分析通过的 AST 翻译为 LLVM IR（文本形式 .ll）。
//! 后续的中间优化交给 LLVM `opt` 完成，本模块只负责生成合法的 IR。
//!
//! # 简化约定（v0.1）
//! - 变量使用 alloca/store/load 模式（依赖 opt 的 mem2reg 提升）
//! - println 通过声明 `printf` 实现，按参数类型选择格式串
//! - 函数入口块命名为 `entry`，控制流块命名为 `if.then`/`if.else`/`loop.cond` 等

use tie_frontend::ast::{
    BinaryOp, Expr, FieldAssignStmt, FnDefStmt, MethodDefStmt, Program, Stmt, TypeSpec, UnaryOp,
};
use tie_frontend::lexer::TyKw;
use tie_frontend::semantic::{ClassInfo, FuncSig, MethodSig, SemanticResult};
use std::collections::HashMap;

/// IR 生成结果。
pub struct IrOutput {
    /// LLVM IR 文本
    pub ir: String,
}

/// IR 生成错误（一般来自语义结果的缺失，正常情况不应触发）。
#[derive(Debug)]
pub struct IrError {
    pub message: String,
}

impl std::fmt::Display for IrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IR 生成错误: {}", self.message)
    }
}

/// 变量绑定：alloca 目标名 + 类型。
#[derive(Debug, Clone)]
struct VarBind {
    /// 值名（alloca 的结果，如 `%x`）
    value: String,
    /// LLVM 类型名
    ty: &'static str,
    /// 是否按指针绑定（by_ptr）：为 true 时 value 本身即对象地址
    /// （this 参数不 alloca，直接绑定参数寄存器），false 为常规 alloca。
    by_ptr: bool,
}

/// IR 生成器。
struct IrGenerator<'p> {
    program: &'p Program,
    sem: &'p SemanticResult,
    /// 输出缓冲
    out: String,
    /// 全局字符串/格式串常量声明缓冲（函数体内延迟收集，结束时统一输出）
    globals: String,
    /// 临时寄存器计数器（每个函数内从 1 开始）
    reg: u32,
    /// 作用域栈：变量名 → 绑定
    scopes: Vec<HashMap<String, VarBind>>,
    /// 字符串常量计数
    str_count: u32,
    /// 格式串缓存：格式串 → 全局常量名
    fmt_cache: HashMap<String, String>,
    /// 元组聚合类型缓存：类型文本 → 泄漏的 'static 类型名（去重，避免重复泄漏）
    ty_cache: HashMap<String, &'static str>,
    /// 当前所在函数
    cur_fn: String,
}

/// IR 生成入口：程序 AST + 语义结果 → LLVM IR 文本。
pub fn gen_ir(program: &Program, sem: &SemanticResult) -> Result<IrOutput, IrError> {
    let mut generator = IrGenerator {
        program,
        sem,
        out: String::new(),
        globals: String::new(),
        reg: 0,
        scopes: Vec::new(),
        str_count: 0,
        fmt_cache: HashMap::new(),
        ty_cache: HashMap::new(),
        cur_fn: String::new(),
    };
    generator.run()?;
    Ok(IrOutput { ir: generator.out })
}

impl<'p> IrGenerator<'p> {
    // ---------- 模块级 ----------

    fn run(&mut self) -> Result<(), IrError> {
        // 模块头
        self.out.push_str("; ModuleID = 'tie'\n");
        self.out.push_str("source_filename = \"input.tie\"\n\n");
        // printf 声明（println 依赖）
        self.out.push_str("declare i32 @printf(ptr, ...)\n\n");
        // 字符串运行时依赖（拼接/比较/长度）：
        // - strlen：字符串长度（len() 内置函数）
        // - strcmp：字符串比较（== != < > <= >= 与 switch 字符串 case）
        // - malloc：拼接结果动态分配（string + string → 新串）
        // - llvm.memcpy：拼接时把两段拷贝进新缓冲区
        self.out.push_str("declare i64 @strlen(ptr)\n");
        self.out.push_str("declare i32 @strcmp(ptr, ptr)\n");
        self.out.push_str("declare ptr @malloc(i64)\n");
        self.out
            .push_str("declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)\n\n");

        // 收集函数签名（与语义一致）
        let sigs: HashMap<String, FuncSig> = self
            .program
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::FnDef(f) => Some((
                    f.name.clone(),
                    FuncSig {
                        param_tys: f.params.iter().map(|p| p.ty.clone()).collect(),
                        ret_ty: f.ret_ty.clone(),
                    },
                )),
                _ => None,
            })
            .collect();

        // 生成各函数
        for stmt in &self.program.stmts {
            if let Stmt::FnDef(f) = stmt {
                self.gen_fn(f, &sigs)?;
            }
        }

        // 生成各类的方法（P8）：按类定义顺序，逐个方法生成。
        // 方法名 mangling：`@<定义类>$<方法名>`（继承中同名方法各自独立生成）。
        for stmt in &self.program.stmts {
            if let Stmt::Class(c) = stmt {
                for m in &c.methods {
                    self.gen_method(m, &c.name)?;
                }
            }
        }

        // 函数体生成过程中延迟收集的全局常量，统一输出到模块级
        self.out.push('\n');
        self.out.push_str(&self.globals);
        Ok(())
    }

    // ---------- 函数生成 ----------

    fn gen_fn(&mut self, f: &FnDefStmt, sigs: &HashMap<String, FuncSig>) -> Result<(), IrError> {
        self.cur_fn = f.name.clone();
        self.reg = 0;
        self.scopes.clear();

        // 签名行
        let ret_llvm = self.llvm_ty(&f.ret_ty);
        let mut params = Vec::new();
        for p in &f.params {
            params.push(format!("{} {}", self.llvm_ty(&p.ty), mangle(&p.name)));
        }
        self.out.push_str(&format!(
            "define {} @{}({}) {{\n",
            ret_llvm,
            f.name,
            params.join(", ")
        ));
        // 入口块
        self.out.push_str("entry:\n");
        self.indent();

        // 参数入作用域：alloca + store
        let mut scope = HashMap::new();
        for p in &f.params {
            let ty = self.llvm_ty(&p.ty);
            let pname = mangle(&p.name);
            let alloca = self.new_reg();
            self.line(&format!("{alloca} = alloca {ty}"));
            self.line(&format!("store {ty} {pname}, ptr {alloca}"));
            scope.insert(p.name.clone(), VarBind { value: alloca, ty, by_ptr: false });
        }
        self.scopes.push(scope);

        // 函数体
        for stmt in &f.body {
            self.gen_stmt(stmt)?;
        }

        // 结尾：无 return 时补默认返回。
        // 判断依据：函数体最后一条非空指令是否以 `ret ` 开头（含 `ret void`/`ret i64 ...`）。
        let last_line = self
            .out
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        let needs_ret = !last_line.starts_with("ret ");
        if needs_ret {
            if f.ret_ty.is_void() {
                self.line("ret void");
            } else {
                // 非 void 且缺 return：语义已拦截，这里兜底返回。
                // 注意 ptr（string）类型不能用整数 0，必须用 null。
                let ty = self.llvm_ty(&f.ret_ty);
                let zero = if ty == "ptr" { "null" } else { "0" };
                self.line(&format!("ret {ty} {zero}"));
            }
        }

        self.dedent();
        self.out.push_str("}\n\n");
        self.scopes.pop();
        let _ = sigs;
        Ok(())
    }

    // ---------- 方法生成（P8） ----------

    /// 方法生成：`define ret @<类>$<方法>(ptr %this, 参数...)`。
    ///
    /// - 实例方法：第一个参数是隐藏的 this（`ptr`），绑定为 by_ptr VarBind（不 alloca，
    ///   直接引用参数寄存器作为对象地址，字段访问 GEP 时用该地址）。
    /// - 静态方法：无 this 参数，签名与普通函数一致。
    fn gen_method(&mut self, m: &MethodDefStmt, class_name: &str) -> Result<(), IrError> {
        self.cur_fn = format!("{class_name}${}", m.name);
        self.reg = 0;
        self.scopes.clear();

        // 签名行：实例方法首参为 this（ptr），静态方法无
        let ret_llvm = self.llvm_ty(&m.ret_ty);
        let mut params = Vec::new();
        if !m.is_static {
            params.push("ptr %this".to_string());
        }
        for p in &m.params {
            params.push(format!("{} {}", self.llvm_ty(&p.ty), mangle(&p.name)));
        }
        self.out
            .push_str(&format!("define {} @{}({}) {{\n", ret_llvm, self.cur_fn, params.join(", ")));
        // 入口块
        self.out.push_str("entry:\n");
        self.indent();

        // 参数入作用域：this 直接绑定参数寄存器（by_ptr，不 alloca）；
        // 普通参数 alloca + store（与函数一致）
        let mut scope = HashMap::new();
        if !m.is_static {
            let this_ty = self.llvm_ty(&TypeSpec::Class(class_name.to_string()));
            scope.insert(
                "this".to_string(),
                VarBind { value: "%this".to_string(), ty: this_ty, by_ptr: true },
            );
        }
        for p in &m.params {
            let ty = self.llvm_ty(&p.ty);
            let pname = mangle(&p.name);
            let alloca = self.new_reg();
            self.line(&format!("{alloca} = alloca {ty}"));
            self.line(&format!("store {ty} {pname}, ptr {alloca}"));
            scope.insert(p.name.clone(), VarBind { value: alloca, ty, by_ptr: false });
        }
        self.scopes.push(scope);

        // 方法体
        for stmt in &m.body {
            self.gen_stmt(stmt)?;
        }

        // 结尾：无 return 时补默认返回（与 gen_fn 同一逻辑）
        let last_line = self
            .out
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        let needs_ret = !last_line.starts_with("ret ");
        if needs_ret {
            if m.ret_ty.is_void() {
                self.line("ret void");
            } else {
                let ty = self.llvm_ty(&m.ret_ty);
                let zero = if ty == "ptr" { "null" } else { "0" };
                self.line(&format!("ret {ty} {zero}"));
            }
        }

        self.dedent();
        self.out.push_str("}\n\n");
        self.scopes.pop();
        Ok(())
    }

    // ---------- 语句生成 ----------

    fn gen_stmt(&mut self, stmt: &Stmt) -> Result<(), IrError> {
        match stmt {
            Stmt::VarDecl(v) => {
                // 表变量：直接生成定长数组布局（alloca [N x T] + 逐元素 store），
                // 长度与元素类型来自语义层 tables 元数据（键 = init 表达式地址）。
                if v.ty.as_ref().map(|t| t.is_table()).unwrap_or(false) {
                    return self.gen_table_var(v);
                }
                let (val, ty) = self.gen_expr(&v.init)?;
                // 声明类型以语义为准
                let ty_name = match &v.ty {
                    // 宽类型（num/text/misc）是编译期概念，语义分析阶段
                    // 已把具体推导类型记录在 expr_types 表中（键为 init 表达式地址），
                    // IR 阶段按地址取回具体类型，避免直接对宽类型调用 llvm_ty。
                    Some(t) if t.is_wide() => {
                        let key = &v.init as *const Expr as usize;
                        let concrete = self
                            .sem
                            .expr_types
                            .get(&key)
                            .cloned()
                            .unwrap_or(TypeSpec::Named(TyKw::I64));
                        self.llvm_ty(&concrete)
                    }
                    Some(t) => self.llvm_ty(t),
                    None => ty,
                };
                let alloca = self.new_reg();
                self.line(&format!("{alloca} = alloca {ty_name}"));
                self.line(&format!("store {ty_name} {val}, ptr {alloca}"));
                // 变量类型：int/float/bool 等；string 特殊（ptr）
                self.cur_scope_mut()
                    .insert(v.name.clone(), VarBind { value: alloca, ty: ty_name, by_ptr: false });
                Ok(())
            }
            Stmt::FnDef(_) => Ok(()), // 顶层函数，不在此生成
            Stmt::Expr(e) => {
                self.gen_expr(&e.expr)?;
                Ok(())
            }
            Stmt::Assign(a) => {
                // 赋值：查找目标变量绑定（语义已保证存在且非 const）
                let bind = self.lookup_var(&a.target).cloned().ok_or_else(|| IrError {
                    message: format!("内部错误：赋值目标 '{}' 未入作用域（函数 {}）", a.target, self.cur_fn),
                })?;
                let (val, _ty) = self.gen_expr(&a.value)?;
                // 按变量的声明类型 store（语义已保证类型匹配）
                self.line(&format!("store {} {}, ptr {}", bind.ty, val, bind.value));
                Ok(())
            }
            Stmt::Return(r) => match &r.expr {
                Some(e) => {
                    let (val, _ty) = self.gen_expr(e)?;
                    // 返回类型以当前函数/方法签名为准：字面量可能被语义适配
                    // （如返回 i32 的函数 `return 42`，字面量推导为 i64）。
                    // 方法名形如 `类$方法`，从 classes 表查签名（不在 funcs 表）。
                    let ret_ty = self.current_ret_ty();
                    let ret_llvm = self.llvm_ty(&ret_ty);
                    // 非字面量场景语义已保证类型一致；字面量直接按签名类型写出常量
                    self.line(&format!("ret {ret_llvm} {val}"));
                    Ok(())
                }
                None => {
                    self.line("ret void");
                    Ok(())
                }
            },
            Stmt::If(i) => self.gen_if(i),
            Stmt::While(w) => self.gen_while(w),
            Stmt::For(f) => self.gen_for(f),
            Stmt::Switch(s) => self.gen_switch(s),
            Stmt::Class(_) => {
                // 类定义只在顶层生成方法（run 中遍历），函数体内不应出现
                Ok(())
            }
            Stmt::FieldAssign(fa) => self.gen_field_assign(fa),
            Stmt::Import(_) => {
                // import 已在 driver 层展开为函数（语义分析前），IR 阶段不应出现
                Ok(())
            }
        }
    }

    /// 字段赋值：`obj.field = value` → GEP 到字段偏移 + store（P8）。
    ///
    /// base 必须是可寻址的类实例（变量/this/字段链，语义已保证），
    /// 字段偏移取自语义层 field_index（继承拍平后的权威 GEP 下标）。
    fn gen_field_assign(&mut self, fa: &FieldAssignStmt) -> Result<(), IrError> {
        // base 地址：变量/this → 绑定指针；字段链 → 逐级 GEP（gen_class_addr 内部处理）
        let (base_ptr, base_llvm) = self.gen_class_addr(&fa.base)?;
        // base 语义类型必须是类：取拍平 field_index 的字段下标
        let base_ty = self.sem_ty_of(&fa.base).ok_or_else(|| IrError {
            message: format!("内部错误：字段赋值缺少基类型（函数 {}）", self.cur_fn),
        })?;
        let TypeSpec::Class(class_name) = &base_ty else {
            return Err(IrError {
                message: format!(
                    "内部错误：字段赋值的对象不是类（{}，函数 {}）",
                    type_name_of(&base_ty),
                    self.cur_fn
                ),
            });
        };
        let info = self
            .sem
            .classes
            .get(class_name)
            .cloned()
            .ok_or_else(|| IrError {
                message: format!("内部错误：类 '{class_name}' 无信息（函数 {}）", self.cur_fn),
            })?;
        let idx = info.field_index.get(&fa.field).copied().ok_or_else(|| IrError {
            message: format!(
                "内部错误：类 '{class_name}' 无字段 '{}'（函数 {}）",
                fa.field, self.cur_fn
            ),
        })?;
        // 字段类型（语义已保证与 value 匹配）
        let fty = info.fields[idx].ty.clone().expect("字段类型已在类收集时解析");
        let f_llvm = self.llvm_ty(&fty);
        // 值
        let (val, _t) = self.gen_expr(&fa.value)?;
        // GEP 到字段偏移，store 直写（Oracle 方案：不读改写）
        let ptr = self.new_reg();
        self.line(&format!("{ptr} = getelementptr {base_llvm}, ptr {base_ptr}, i32 0, i32 {idx}"));
        self.line(&format!("store {f_llvm} {val}, ptr {ptr}"));
        Ok(())
    }

    /// 表变量声明：生成定长数组布局。
    ///
    /// 布局：`alloca [N x T]`，随后对每个元素 store 到数组内偏移（GEP）。
    /// 长度与元素类型取自语义层 tables 元数据（键 = init 表达式地址）。
    fn gen_table_var(&mut self, v: &tie_frontend::ast::VarDeclStmt) -> Result<(), IrError> {
        let key = &v.init as *const Expr as usize;
        let info = self
            .sem
            .tables
            .get(&key)
            .cloned()
            .ok_or_else(|| IrError {
                message: format!("内部错误：表变量 '{}' 缺少布局元数据", v.name),
            })?;
        let elem_llvm = self.llvm_ty(&info.elem_ty);
        let arr_ty = format!("[{} x {}]", info.len, elem_llvm);
        let alloca = self.new_reg();
        self.line(&format!("{alloca} = alloca {arr_ty}"));
        // 逐元素 store 到数组内偏移（GEP 第 0 行，第 i 列）
        if let Expr::TableLit { cells, .. } = &v.init {
            for (i, cell) in cells.iter().enumerate() {
                let (val, _t) = self.gen_expr(&cell.value)?;
                let ptr = self.new_reg();
                self.line(&format!("{ptr} = getelementptr {arr_ty}, ptr {alloca}, i64 0, i64 {i}"));
                self.line(&format!("store {elem_llvm} {val}, ptr {ptr}"));
            }
        }
        // 绑定：变量类型 = 数组类型（下标访问用 GEP，不整体 load）。
        // 数组类型是动态拼接的字符串，Box::leak 获得 'static 生命周期
        //（编译器进程短期运行，泄漏少量字符串可接受）。
        let arr_ty_static: &'static str = Box::leak(arr_ty.into_boxed_str());
        self.cur_scope_mut()
            .insert(v.name.clone(), VarBind { value: alloca, ty: arr_ty_static, by_ptr: false });
        Ok(())
    }

    fn gen_if(&mut self, i: &tie_frontend::ast::IfStmt) -> Result<(), IrError> {
        let (cond, _) = self.gen_expr(&i.cond)?;
        let then_label = self.new_label("if.then");
        let else_label = self.new_label("if.else");
        let merge_label = self.new_label("if.merge");
        self.line(&format!("br i1 {cond}, label %{then_label}, label %{else_label}"));

        // then 分支：若块已以 return 终止（block_terminated），不再追加 br，
        // 否则 `ret` 后跟 `br` 会产生死代码指令，LLVM 报「指令编号不连续」错误
        self.block_start(&then_label);
        self.scopes.push(HashMap::new());
        for s in &i.then_branch {
            self.gen_stmt(s)?;
        }
        self.scopes.pop();
        if !self.block_terminated() {
            self.line(&format!("br label %{merge_label}"));
        }
        self.block_end();

        // else 分支
        self.block_start(&else_label);
        self.scopes.push(HashMap::new());
        for s in &i.else_branch {
            self.gen_stmt(s)?;
        }
        self.scopes.pop();
        if !self.block_terminated() {
            self.line(&format!("br label %{merge_label}"));
        }
        self.block_end();

        self.block_start(&merge_label);
        Ok(())
    }

    fn gen_while(&mut self, w: &tie_frontend::ast::WhileStmt) -> Result<(), IrError> {
        let cond_label = self.new_label("loop.cond");
        let body_label = self.new_label("loop.body");
        let exit_label = self.new_label("loop.exit");
        self.line(&format!("br label %{cond_label}"));

        self.block_start(&cond_label);
        let (cond, _) = self.gen_expr(&w.cond)?;
        self.line(&format!("br i1 {cond}, label %{body_label}, label %{exit_label}"));
        self.block_end();

        self.block_start(&body_label);
        self.scopes.push(HashMap::new());
        for s in &w.body {
            self.gen_stmt(s)?;
        }
        self.scopes.pop();
        // 循环体若已以 return 终止，则无需跳回条件块（否则 ret 后产生死代码 br）
        if !self.block_terminated() {
            self.line(&format!("br label %{cond_label}"));
        }
        self.block_end();

        self.block_start(&exit_label);
        Ok(())
    }

    fn gen_for(&mut self, f: &tie_frontend::ast::ForStmt) -> Result<(), IrError> {
        // 表遍历：`for item in arr`（arr 为表变量，聚合类型 [N x T]）
        if let Expr::Var(name) = &f.iter
            && let Some(bind) = self.lookup_var(name).cloned()
            && let Some((len, elem_ty)) = parse_array_shape(bind.ty)
        {
            return self.gen_for_table(f, &bind, len, elem_ty);
        }
        // 范围遍历：`for x in start..end`
        let Expr::Range { start, end, .. } = &f.iter else {
            return Err(IrError {
                message: format!("for 迭代对象仅支持范围（start..end）或表变量（函数 {}）", self.cur_fn),
            });
        };
        let (start_val, start_ty) = self.gen_expr(start)?;
        let (end_val, end_ty) = self.gen_expr(end)?;
        // 循环变量固定为 i64：start/end 若是窄整数，需先扩展到 i64（按符号性 sext/zext）
        let start_val = self.extend_int_to_i64(&start_val, start_ty, start)?;
        let end_val = self.extend_int_to_i64(&end_val, end_ty, end)?;

        // 循环变量 alloca
        let var_alloca = self.new_reg();
        self.line(&format!("{var_alloca} = alloca i64"));
        self.line(&format!("store i64 {start_val}, ptr {var_alloca}"));

        let cond_label = self.new_label("for.cond");
        let body_label = self.new_label("for.body");
        let exit_label = self.new_label("for.exit");
        self.line(&format!("br label %{cond_label}"));

        self.block_start(&cond_label);
        let cur = self.new_reg();
        let done = self.new_reg();
        self.line(&format!("{cur} = load i64, ptr {var_alloca}"));
        self.line(&format!("{done} = icmp sge i64 {cur}, {end_val}"));
        self.line(&format!("br i1 {done}, label %{exit_label}, label %{body_label}"));
        self.block_end();

        self.block_start(&body_label);
        // 循环变量可见
        self.scopes.push(HashMap::from([(
            f.var.clone(),
            VarBind { value: var_alloca.clone(), ty: "i64", by_ptr: false },
        )]));
        for s in &f.body {
            self.gen_stmt(s)?;
        }
        self.scopes.pop();
        // 循环体若已以 return 终止，跳过自增与回跳（否则 ret 后产生死代码指令）
        if !self.block_terminated() {
            // 自增
            let next = self.new_reg();
            self.line(&format!("{next} = add i64 {cur}, 1"));
            self.line(&format!("store i64 {next}, ptr {var_alloca}"));
            self.line(&format!("br label %{cond_label}"));
        }
        self.block_end();

        self.block_start(&exit_label);
        Ok(())
    }

    /// 表遍历：`for item in arr`，生成 0..N 计数器循环。
    ///
    /// 布局：计数器 alloca（i64，0..N）+ 循环变量 alloca（元素类型 T）。
    /// 每次迭代：GEP 取 arr[i] → load 元素 → store 到循环变量。
    fn gen_for_table(
        &mut self,
        f: &tie_frontend::ast::ForStmt,
        arr_bind: &VarBind,
        len: usize,
        elem_ty: &'static str,
    ) -> Result<(), IrError> {
        // 计数器 alloca（i64）
        let idx_alloca = self.new_reg();
        self.line(&format!("{idx_alloca} = alloca i64"));
        self.line(&format!("store i64 0, ptr {idx_alloca}"));
        // 循环变量 alloca（元素类型 T，每次迭代覆盖）
        let item_alloca = self.new_reg();
        self.line(&format!("{item_alloca} = alloca {elem_ty}"));

        let cond_label = self.new_label("for.cond");
        let body_label = self.new_label("for.body");
        let exit_label = self.new_label("for.exit");
        self.line(&format!("br label %{cond_label}"));

        self.block_start(&cond_label);
        let cur = self.new_reg();
        let done = self.new_reg();
        self.line(&format!("{cur} = load i64, ptr {idx_alloca}"));
        self.line(&format!("{done} = icmp sge i64 {cur}, {len}"));
        self.line(&format!("br i1 {done}, label %{exit_label}, label %{body_label}"));
        self.block_end();

        self.block_start(&body_label);
        // item = arr[cur]（GEP + load）
        let ptr = self.new_reg();
        self.line(&format!(
            "{ptr} = getelementptr {}, ptr {}, i64 0, i64 {cur}",
            arr_bind.ty, arr_bind.value
        ));
        let val = self.new_reg();
        self.line(&format!("{val} = load {elem_ty}, ptr {ptr}"));
        self.line(&format!("store {elem_ty} {val}, ptr {item_alloca}"));
        // 循环变量可见
        self.scopes.push(HashMap::from([(
            f.var.clone(),
            VarBind { value: item_alloca.clone(), ty: elem_ty, by_ptr: false },
        )]));
        for s in &f.body {
            self.gen_stmt(s)?;
        }
        self.scopes.pop();
        // 循环体若已以 return 终止，跳过自增与回跳（否则 ret 后产生死代码指令）
        if !self.block_terminated() {
            // 自增
            let next = self.new_reg();
            self.line(&format!("{next} = add i64 {cur}, 1"));
            self.line(&format!("store i64 {next}, ptr {idx_alloca}"));
            self.line(&format!("br label %{cond_label}"));
        }
        self.block_end();

        self.block_start(&exit_label);
        Ok(())
    }

    /// switch 多分支选择：生成比较链 + 各 case 体块。
    ///
    /// 结构（每个 case 一个比较块 + 一个体块）：
    /// ```text
    ///   br label %sw.cmp.0
    /// sw.cmp.0:
    ///   %c = icmp eq T %subj, <case0 值>
    ///   br i1 %c, label %sw.body.0, label %sw.cmp.1
    /// sw.body.0: …; br label %sw.exit
    /// sw.cmp.1: …（不匹配 → default 或 exit）
    /// sw.default: …; br label %sw.exit
    /// sw.exit:
    /// ```
    /// 比较类型统一：整数扩展为 i64（icmp eq）、float 扩展为 double（fcmp oeq）、
    /// 布尔用 i1（icmp eq）、字符用 i32（icmp eq，case 字符字面量同为 i32），
    /// 保证 case 字面量与 subject 同类型可比较。
    fn gen_switch(&mut self, s: &tie_frontend::ast::SwitchStmt) -> Result<(), IrError> {
        // subject 求值 + 统一比较类型（返回比较指令的「运算类型」）
        let (raw_subj, subj_ty) = self.gen_expr(&s.subject)?;
        // 字符串 subject：比较走 strcmp（case 值同为 ptr 字面量），cmp_op 仅作标记
        let is_str_subj = matches!(
            self.sem_ty_of(&s.subject),
            Some(TypeSpec::Named(TyKw::Str))
        );
        // 字符（LLVM i32）需按语义类型区分：直接 i32 比较，不扩展
        let is_char = matches!(
            self.sem_ty_of(&s.subject),
            Some(TypeSpec::Named(TyKw::Char))
        );
        let (subj, cmp_op): (String, &str) = if is_str_subj {
            (raw_subj, "strcmp")
        } else {
            match subj_ty {
                "i1" => (raw_subj, "icmp eq i1"),
                "double" => (raw_subj, "fcmp oeq double"),
                "float" => {
                    // float 扩展为 double，与 case 浮点字面量（double）同类型
                    let ext = self.new_reg();
                    self.line(&format!("{ext} = fpext float {raw_subj} to double"));
                    (ext, "fcmp oeq double")
                }
                _ if is_char => {
                    // 字符：LLVM i32，case 字符字面量也是 i32，直接 icmp eq i32
                    (raw_subj, "icmp eq i32")
                }
                _ => {
                    // 整数（i8/i16/i32/i64/u*）：扩展为 i64（按符号性 sext/zext）
                    let ext = self.extend_int_to_i64(&raw_subj, subj_ty, &s.subject)?;
                    (ext, "icmp eq i64")
                }
            }
        };

        let exit_label = self.new_label("sw.exit");
        let has_default = !s.default_body.is_empty();
        // default 标签在循环前统一创建，供最后一个 case 的 else 目标引用，
        // 也用于 default 体块生成（保证同一标签只定义一次）
        let def_label = if has_default { Some(self.new_label("sw.default")) } else { None };

        // 无 case 分支：直接跳 default 或 exit
        if s.cases.is_empty() {
            match &def_label {
                Some(def) => {
                    self.line(&format!("br label %{def}"));
                    self.gen_switch_body(&s.default_body, def, &exit_label)?;
                }
                None => {
                    self.line(&format!("br label %{exit_label}"));
                }
            }
            self.block_start(&exit_label);
            return Ok(());
        }

        // 第一个比较块入口
        let first_cmp = self.new_label("sw.cmp");
        self.line(&format!("br label %{first_cmp}"));
        let mut cur_cmp = first_cmp;

        for (i, case) in s.cases.iter().enumerate() {
            let is_last = i == s.cases.len() - 1;
            let body_label = self.new_label("sw.body");
            // 下一个比较目标：还有 case → 新比较块；否则 → default（有）或 exit
            let next_cmp = if is_last {
                None
            } else {
                Some(self.new_label("sw.cmp"))
            };

            // 比较块：subject == case 值
            self.block_start(&cur_cmp);
            let (case_val, _case_ty) = self.gen_expr(&case.value)?;
            let cond = if is_str_subj {
                // 字符串：strcmp(subj, case) == 0 才算匹配
                let cmp_res = self.new_reg();
                self.line(&format!("{cmp_res} = call i32 @strcmp(ptr {subj}, ptr {case_val})"));
                let c = self.new_reg();
                self.line(&format!("{c} = icmp eq i32 {cmp_res}, 0"));
                c
            } else {
                let c = self.new_reg();
                self.line(&format!("{c} = {cmp_op} {subj}, {case_val}"));
                c
            };
            let else_target = match &next_cmp {
                Some(l) => l.clone(),
                None => def_label.clone().unwrap_or_else(|| exit_label.clone()),
            };
            self.line(&format!("br i1 {cond}, label %{body_label}, label %{else_target}"));
            self.block_end();

            // 体块：case 语句列表
            self.gen_switch_body(&case.body, &body_label, &exit_label)?;

            if let Some(l) = next_cmp {
                cur_cmp = l;
            }
        }

        // default 体块（可选）
        if let Some(def) = &def_label {
            self.gen_switch_body(&s.default_body, def, &exit_label)?;
        }

        self.block_start(&exit_label);
        Ok(())
    }

    /// 生成 switch 的一个分支体（case 或 default）：语句列表 + 跳回 exit。
    ///
    /// 分支体有自己的作用域（内部变量不外泄），结束后无条件跳转到 exit。
    fn gen_switch_body(
        &mut self,
        body: &[Stmt],
        body_label: &str,
        exit_label: &str,
    ) -> Result<(), IrError> {
        self.block_start(body_label);
        self.scopes.push(HashMap::new());
        for st in body {
            self.gen_stmt(st)?;
        }
        self.scopes.pop();
        // 分支体若已以 return 终止，无需跳回 exit（否则 ret 后产生死代码 br）
        if !self.block_terminated() {
            self.line(&format!("br label %{exit_label}"));
        }
        self.block_end();
        Ok(())
    }

    // ---------- 表达式生成 ----------

    /// 生成表达式，返回 (值名, LLVM 类型名)。
    fn gen_expr(&mut self, expr: &Expr) -> Result<(String, &'static str), IrError> {
        match expr {
            Expr::IntLit(v) => Ok((v.to_string(), "i64")),
            Expr::FloatLit(v) => Ok((format_float(*v), "double")),
            Expr::BoolLit(b) => Ok((if *b { "true".into() } else { "false".into() }, "i1")),
            Expr::CharLit(c) => Ok(((*c as i32).to_string(), "i32")),
            Expr::StrLit(s) => {
                let g = self.string_global(s);
                // 字符串：返回全局常量指针（ptr 类型，供 %s / 传参直接使用）
                Ok((format!("@{g}"), "ptr"))
            }
            Expr::Var(name) => {
                // 克隆绑定以结束对 scopes 的借用，随后可安全调用 &mut 方法
                let bind = self.lookup_var(name).cloned().ok_or_else(|| IrError {
                    message: format!("内部错误：变量 '{name}' 未入作用域（函数 {}）", self.cur_fn),
                })?;
                let ty = bind.ty;
                // i1 类型需要扩展（load i1 无法直接使用），这里统一 load
                let tmp = self.new_reg();
                self.line(&format!("{tmp} = load {ty}, ptr {}", bind.value));
                Ok((tmp, ty))
            }
            Expr::Call { name, args, .. } => {
                // 构造调用：类名(...) → insertvalue 链构建结构体值（P8）
                if let Some(info) = self.sem.classes.get(name).cloned() {
                    return self.gen_construct(name, &info, args);
                }
                self.gen_call(name, args)
            }
            Expr::Unary { op, operand, .. } => {
                let (val, ty) = self.gen_expr(operand)?;
                match op {
                    UnaryOp::Neg => {
                        let tmp = self.new_reg();
                        // 浮点用 fneg，整数用 sub {ty} 0
                        if ty == "double" || ty == "float" {
                            self.line(&format!("{tmp} = fneg {ty} {val}"));
                        } else {
                            self.line(&format!("{tmp} = sub {ty} 0, {val}"));
                        }
                        Ok((tmp, ty))
                    }
                    UnaryOp::Not => {
                        let tmp = self.new_reg();
                        self.line(&format!("{tmp} = xor i1 {val}, true"));
                        Ok((tmp, "i1"))
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => self.gen_binary(*op, lhs, rhs),
            Expr::Range { .. } => Err(IrError {
                message: "范围表达式只能在 for 中使用（不能单独求值）".into(),
            }),
            Expr::TableLit { .. } => Err(IrError {
                message: "表字面量只能用于表变量声明（var x: table = [...]）".into(),
            }),
            Expr::Index { base, index, .. } => self.gen_index(base, index),
            Expr::TupleLit { fields, .. } => {
                // 元组字面量 → 逐字段 insertvalue 构造聚合值。
                // 聚合类型与字段类型以语义结果为准（含标注适配后的精确类型）。
                let tuple_ty = self.sem_ty_of(expr).ok_or_else(|| IrError {
                    message: format!("内部错误：元组字面量缺少语义类型（函数 {}）", self.cur_fn),
                })?;
                let TypeSpec::Tuple(tfs) = &tuple_ty else {
                    return Err(IrError {
                        message: format!("内部错误：元组字面量语义类型不是元组（函数 {}）", self.cur_fn),
                    });
                };
                let agg_ty = self.llvm_ty(&tuple_ty);
                // insertvalue 链：undef 起始，逐字段插入
                let mut cur = "undef".to_string();
                for (i, ((_name, e), tf)) in fields.iter().zip(tfs.iter()).enumerate() {
                    let (v, _t) = self.gen_expr(e)?;
                    let ft = self.llvm_ty(&tf.ty);
                    let tmp = self.new_reg();
                    self.line(&format!("{tmp} = insertvalue {agg_ty} {cur}, {ft} {v}, {i}"));
                    cur = tmp;
                }
                Ok((cur, agg_ty))
            }
            Expr::FieldAccess { base, field, .. } => {
                // 字段访问（读）：按 base 的语义类型分发——
                // 元组 → extractvalue（寄存器中的聚合值直接取字段）；
                // 类实例 → GEP + load（对象在 alloca/参数中，按字段偏移读）。
                let base_ty = self.sem_ty_of(base).ok_or_else(|| IrError {
                    message: format!("内部错误：字段访问缺少基类型（函数 {}）", self.cur_fn),
                })?;
                match &base_ty {
                    TypeSpec::Tuple(tfs) => {
                        let (bv, _bt) = self.gen_expr(base)?;
                        let idx = tuple_field_index(&base_ty, field).ok_or_else(|| IrError {
                            message: format!("内部错误：元组字段 '{field}' 解析失败（函数 {}）", self.cur_fn),
                        })?;
                        let agg_ty = self.llvm_ty(&base_ty);
                        let ft = self.llvm_ty(&tfs[idx].ty);
                        let tmp = self.new_reg();
                        self.line(&format!("{tmp} = extractvalue {agg_ty} {bv}, {idx}"));
                        Ok((tmp, ft))
                    }
                    TypeSpec::Class(class_name) => {
                        // 取对象地址：变量/字段链 → 地址；否则不可寻址
                        let (base_ptr, base_llvm) = self.gen_class_addr(base)?;
                        let info = self
                            .sem
                            .classes
                            .get(class_name)
                            .cloned()
                            .ok_or_else(|| IrError {
                                message: format!("内部错误：类 '{class_name}' 无信息（函数 {}）", self.cur_fn),
                            })?;
                        let idx = info.field_index.get(field).copied().ok_or_else(|| IrError {
                            message: format!("内部错误：类 '{class_name}' 无字段 '{field}'（函数 {}）", self.cur_fn),
                        })?;
                        let fty = info.fields[idx].ty.clone().expect("字段类型已在类收集时解析");
                        let f_llvm = self.llvm_ty(&fty);
                        let ptr = self.new_reg();
                        self.line(&format!(
                            "{ptr} = getelementptr {base_llvm}, ptr {base_ptr}, i32 0, i32 {idx}"
                        ));
                        let val = self.new_reg();
                        self.line(&format!("{val} = load {f_llvm}, ptr {ptr}"));
                        Ok((val, f_llvm))
                    }
                    _ => Err(IrError {
                        message: format!(
                            "内部错误：字段访问的对象不是元组/类（{}，函数 {}）",
                            type_name_of(&base_ty),
                            self.cur_fn
                        ),
                    }),
                }
            }
            Expr::MethodCall { receiver, method, args, .. } => {
                // 方法调用：实例方法（receiver 地址作 this 首参）或静态方法（无 this）
                // ——与语义层同一判定：receiver 是未绑定变量且名字是类名 → 静态
                if let Expr::Var(rname) = receiver.as_ref()
                    && !self.scope_has(rname)
                    && self.sem.classes.contains_key(rname)
                {
                    return self.gen_static_call(rname, method, args);
                }
                self.gen_instance_call(receiver, method, args)
            }
        }
    }

    /// 下标访问：`base[index]` → GEP + load。
    ///
    /// base 可以是表变量（VarBind.ty 形如 `[N x T]`）或字符串（取第 i 个字符），
    /// index 为整数（i64 或可扩展）。
    /// 注意：表变量是聚合类型 `[N x T]`，不能整体 load 到寄存器，因此直接取
    /// VarBind 中保存的 alloca 指针做 GEP（与标量变量的 load 路径不同）。
    /// 字符串是 ptr 的 alloca（先 load 拿到串首指针），按字节 GEP + load + zext 成 char(i32)。
    fn gen_index(&mut self, base: &Expr, index: &Expr) -> Result<(String, &'static str), IrError> {
        // base 必须是表/字符串变量：查作用域拿到 alloca 指针 + 类型名（不做 load）
        let Expr::Var(name) = base else {
            return Err(IrError {
                message: "下标访问仅支持表/字符串变量（base[index] 的 base 必须是变量）".into(),
            });
        };
        let bind = self.lookup_var(name).cloned().ok_or_else(|| IrError {
            message: format!("内部错误：下标访问的变量 '{name}' 未入作用域（函数 {}）", self.cur_fn),
        })?;
        let base_ptr = bind.value;
        let base_ty = bind.ty;
        // 下标值：整数（i64 直接使用，窄整数先扩展）
        let (idx_val, idx_ty) = self.gen_expr(index)?;
        let idx_val = self.extend_int_to_i64(&idx_val, idx_ty, index)?;
        // 字符串下标：s[i] → 取第 i 个字节，zext 成 char（i32）。
        // 通过语义类型区分字符串（LLVM "ptr" 无法区分字符串与裸指针）。
        if matches!(self.sem_ty_of(base), Some(TypeSpec::Named(TyKw::Str))) {
            // 字符串变量是 alloca ptr，先 load 拿到串首指针
            let str_ptr = self.new_reg();
            self.line(&format!("{str_ptr} = load ptr, ptr {base_ptr}"));
            let ptr = self.new_reg();
            self.line(&format!("{ptr} = getelementptr i8, ptr {str_ptr}, i64 {idx_val}"));
            let byte = self.new_reg();
            self.line(&format!("{byte} = load i8, ptr {ptr}"));
            // char 在 LLVM 中为 i32：字节零扩展
            let ch = self.new_reg();
            self.line(&format!("{ch} = zext i8 {byte} to i32"));
            return Ok((ch, "i32"));
        }
        // 表下标：数组类型必须可解析
        let Some(elem_ty) = parse_array_elem_ty(base_ty) else {
            return Err(IrError {
                message: format!("内部错误：下标访问的对象不是数组类型（{}）", base_ty),
            });
        };
        // GEP：基址数组第 0 维、第 idx 元素
        let ptr = self.new_reg();
        self.line(&format!("{ptr} = getelementptr {base_ty}, ptr {base_ptr}, i64 0, i64 {idx_val}"));
        let val = self.new_reg();
        self.line(&format!("{val} = load {elem_ty}, ptr {ptr}"));
        Ok((val, elem_ty))
    }

    fn gen_binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<(String, &'static str), IrError> {
        let (lv, lt) = self.gen_expr(lhs)?;
        let (rv, _rt) = self.gen_expr(rhs)?;
        // 字符串操作：拼接（+）与比较（== != < > <= >=）走运行时函数。
        // 通过语义类型判断（LLVM 类型名 "ptr" 无法区分字符串与裸指针）。
        let lhs_is_str = matches!(self.sem_ty_of(lhs), Some(TypeSpec::Named(TyKw::Str)));
        let rhs_is_str = matches!(self.sem_ty_of(rhs), Some(TypeSpec::Named(TyKw::Str)));
        if lhs_is_str || rhs_is_str {
            return self.gen_binary_str(op, lv, rv);
        }
        // 类型以左侧为准（语义已保证一致）
        let ty = lt;
        // 是否为浮点类型（f32→float / f64→double）
        let is_float = ty == "double" || ty == "float";
        // 无符号性：查语义类型（LLVM 类型名 "i32" 无法区分 u32/i32）
        let lhs_sem = self.sem_ty_of(lhs);
        let is_unsigned = matches!(
            lhs_sem,
            Some(TypeSpec::Named(TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64))
        );
        let tmp = self.new_reg();
        let instr: String = match op {
            BinaryOp::Add => {
                if is_float {
                    format!("fadd {ty} {lv}, {rv}")
                } else {
                    format!("add {ty} {lv}, {rv}")
                }
            }
            BinaryOp::Sub => {
                if is_float {
                    format!("fsub {ty} {lv}, {rv}")
                } else {
                    format!("sub {ty} {lv}, {rv}")
                }
            }
            BinaryOp::Mul => {
                if is_float {
                    format!("fmul {ty} {lv}, {rv}")
                } else {
                    format!("mul {ty} {lv}, {rv}")
                }
            }
            BinaryOp::Div => {
                if is_float {
                    format!("fdiv {ty} {lv}, {rv}")
                } else if is_unsigned {
                    // 无符号除法
                    format!("udiv {ty} {lv}, {rv}")
                } else {
                    // 有符号除法
                    format!("sdiv {ty} {lv}, {rv}")
                }
            }
            BinaryOp::Mod => {
                if is_unsigned {
                    format!("urem {ty} {lv}, {rv}")
                } else {
                    format!("srem {ty} {lv}, {rv}")
                }
            }
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le
            | BinaryOp::Ge => {
                // 浮点比较用 fcmp，整数用 icmp
                if is_float {
                    let fcmp = match op {
                        BinaryOp::Eq => "oeq",
                        BinaryOp::NotEq => "one",
                        BinaryOp::Lt => "olt",
                        BinaryOp::Gt => "ogt",
                        BinaryOp::Le => "ole",
                        BinaryOp::Ge => "oge",
                        _ => unreachable!(),
                    };
                    format!("fcmp {fcmp} {ty} {lv}, {rv}")
                } else {
                    // 无符号整数用 u 系列比较（u8/u16/u32/u64）
                    let icmp = match op {
                        BinaryOp::Eq => "eq",
                        BinaryOp::NotEq => "ne",
                        BinaryOp::Lt => {
                            if is_unsigned { "ult" } else { "slt" }
                        }
                        BinaryOp::Gt => {
                            if is_unsigned { "ugt" } else { "sgt" }
                        }
                        BinaryOp::Le => {
                            if is_unsigned { "ule" } else { "sle" }
                        }
                        BinaryOp::Ge => {
                            if is_unsigned { "uge" } else { "sge" }
                        }
                        _ => unreachable!(),
                    };
                    format!("icmp {icmp} {ty} {lv}, {rv}")
                }
            }
            BinaryOp::And => format!("and i1 {lv}, {rv}"),
            BinaryOp::Or => format!("or i1 {lv}, {rv}"),
        };
        self.line(&format!("{tmp} = {instr}"));
        // 比较/逻辑结果为 i1
        let result_ty = match op {
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge
            | BinaryOp::And
            | BinaryOp::Or => "i1",
            _ => ty,
        };
        Ok((tmp, result_ty))
    }

    /// 字符串二元运算：拼接（+）与比较（== != < > <= >=）。
    ///
    /// - 拼接：`malloc(len1+len2+1)` → memcpy 两段 → 末尾写 \0，返回新缓冲区（ptr）。
    /// - 比较：`strcmp(lv, rv)` 结果按有符号与 0 比较，返回 i1。
    ///
    /// 语义层已保证两侧都是字符串，这里 lv/rv 均为 ptr（全局常量或变量 load 结果）。
    fn gen_binary_str(
        &mut self,
        op: BinaryOp,
        lv: String,
        rv: String,
    ) -> Result<(String, &'static str), IrError> {
        // 拼接：动态分配新串
        if op == BinaryOp::Add {
            let llen = self.new_reg();
            self.line(&format!("{llen} = call i64 @strlen(ptr {lv})"));
            let rlen = self.new_reg();
            self.line(&format!("{rlen} = call i64 @strlen(ptr {rv})"));
            let total = self.new_reg();
            self.line(&format!("{total} = add i64 {llen}, {rlen}"));
            // 分配 len1+len2+1 字节（末尾 \0）
            let size = self.new_reg();
            self.line(&format!("{size} = add i64 {total}, 1"));
            let buf = self.new_reg();
            self.line(&format!("{buf} = call ptr @malloc(i64 {size})"));
            // 拷贝第一段到 buf[0..len1)
            self.line(&format!(
                "call void @llvm.memcpy.p0.p0.i64(ptr {buf}, ptr {lv}, i64 {llen}, i1 false)"
            ));
            // buf[len1..] 起始指针
            let off = self.new_reg();
            self.line(&format!("{off} = getelementptr i8, ptr {buf}, i64 {llen}"));
            // 拷贝第二段到 buf[len1..len1+len2)
            self.line(&format!(
                "call void @llvm.memcpy.p0.p0.i64(ptr {off}, ptr {rv}, i64 {rlen}, i1 false)"
            ));
            // 末尾写 \0
            let end = self.new_reg();
            self.line(&format!("{end} = getelementptr i8, ptr {buf}, i64 {total}"));
            self.line(&format!("store i8 0, ptr {end}"));
            return Ok((buf, "ptr"));
        }
        // 比较：strcmp 返回值与 0 做有符号比较
        let cmp = self.new_reg();
        self.line(&format!("{cmp} = call i32 @strcmp(ptr {lv}, ptr {rv})"));
        let icmp = match op {
            BinaryOp::Eq => "eq",
            BinaryOp::NotEq => "ne",
            BinaryOp::Lt => "slt",
            BinaryOp::Gt => "sgt",
            BinaryOp::Le => "sle",
            BinaryOp::Ge => "sge",
            _ => unreachable!("字符串二元运算只允许 + 与比较"),
        };
        let res = self.new_reg();
        self.line(&format!("{res} = icmp {icmp} i32 {cmp}, 0"));
        Ok((res, "i1"))
    }

    /// 函数调用生成：内置 println/len → printf/strlen；用户函数 → call。
    fn gen_call(&mut self, name: &str, args: &[Expr]) -> Result<(String, &'static str), IrError> {
        if name == "println" {
            return self.gen_println(args);
        }
        // 内置 len：字符串长度（语义已保证单字符串参数）
        if name == "len" {
            let (v, _t) = self.gen_expr(&args[0])?;
            let len = self.new_reg();
            self.line(&format!("{len} = call i64 @strlen(ptr {v})"));
            return Ok((len, "i64"));
        }
        // 用户函数调用
        let sig = self
            .sem
            .funcs
            .get(name)
            .cloned()
            .ok_or_else(|| IrError {
                message: format!("内部错误：函数 '{name}' 无签名（函数 {}）", self.cur_fn),
            })?;
        let mut arg_list = Vec::new();
        // 参数类型以函数签名为准：字面量可能被语义适配（如 i32 参数传 42 字面量），
        // 而字面量 gen_expr 返回 i64，需要按签名类型生成
        for (a, want_ty) in args.iter().zip(sig.param_tys.iter()) {
            let (v, _t) = self.gen_expr(a)?;
            let aty = self.llvm_ty(want_ty);
            arg_list.push(format!("{aty} {v}"));
        }
        let ret_llvm = self.llvm_ty(&sig.ret_ty);
        let tmp = self.new_reg();
        if sig.ret_ty.is_void() {
            self.line(&format!("call void @{}({})", name, arg_list.join(", ")));
            Ok((tmp, "void"))
        } else {
            self.line(&format!("{tmp} = call {ret_llvm} @{}({})", name, arg_list.join(", ")));
            Ok((tmp, ret_llvm))
        }
    }

    // ---------- 类相关生成（P8） ----------

    /// 构造调用：`类名(实参...)` → insertvalue 链构建结构体值（P8）。
    ///
    /// 字段顺序与语义层拍平顺序一致（父类字段在前）。每个字段：
    /// - 有对应实参 → 用实参值；
    /// - 无实参（缺省）→ 用字段默认值（字面量）；无默认值 → 零值（0/0.0/false/null）。
    fn gen_construct(
        &mut self,
        class_name: &str,
        info: &ClassInfo,
        args: &[Expr],
    ) -> Result<(String, &'static str), IrError> {
        let agg_ty = self.llvm_ty(&TypeSpec::Class(class_name.to_string()));
        let mut cur = "undef".to_string();
        for (i, f) in info.fields.iter().enumerate() {
            let fty = f.ty.clone().expect("字段类型已在类收集时解析");
            let f_llvm = self.llvm_ty(&fty);
            // 实参在前（语义已保证 args.len() <= fields.len()）
            let val = if let Some(a) = args.get(i) {
                let (v, _t) = self.gen_expr(a)?;
                v
            } else {
                // 缺省：默认值字面量；无 → 零值
                match &f.init {
                    Some(init_expr) => {
                        let (v, _t) = self.gen_expr(init_expr)?;
                        v
                    }
                    None => zero_value(f_llvm).to_string(),
                }
            };
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = insertvalue {agg_ty} {cur}, {f_llvm} {val}, {i}"));
            cur = tmp;
        }
        Ok((cur, agg_ty))
    }

    /// 静态方法调用：`类名.方法(实参...)`（无 this 隐藏参数）。
    ///
    /// 方法名 mangling：`@<定义类>$<方法名>`（method_owner 给出实际定义类）。
    fn gen_static_call(&mut self, class_name: &str, method: &str, args: &[Expr]) -> Result<(String, &'static str), IrError> {
        let info = self
            .sem
            .classes
            .get(class_name)
            .cloned()
            .ok_or_else(|| IrError {
                message: format!("内部错误：类 '{class_name}' 无信息（函数 {}）", self.cur_fn),
            })?;
        let sig = info.methods.get(method).cloned().ok_or_else(|| IrError {
            message: format!("内部错误：类 '{class_name}' 无方法 '{method}'（函数 {}）", self.cur_fn),
        })?;
        if !sig.is_static {
            return Err(IrError {
                message: format!(
                    "内部错误：实例方法 '{method}' 被当作静态方法调用（函数 {}）",
                    self.cur_fn
                ),
            });
        }
        // 定义类：继承中同名方法可能由父类定义
        let owner = info.method_owner.get(method).cloned().unwrap_or_else(|| class_name.to_string());
        let mname = format!("{owner}${method}");
        self.emit_method_call(&mname, &sig, args, None)
    }

    /// 实例方法调用：`obj.方法(实参...)`（obj 地址作隐藏 this 首参）。
    ///
    /// receiver 必须是可寻址的类实例（变量/this/字段链）——语义已保证，
    /// gen_class_addr 内部做地址解析；方法名 mangling 同上。
    fn gen_instance_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<(String, &'static str), IrError> {
        // receiver 语义类型 → 类名
        let recv_ty = self.sem_ty_of(receiver).ok_or_else(|| IrError {
            message: format!("内部错误：方法调用缺少 receiver 类型（函数 {}）", self.cur_fn),
        })?;
        let TypeSpec::Class(class_name) = &recv_ty else {
            return Err(IrError {
                message: format!(
                    "内部错误：方法调用的对象不是类（{}，函数 {}）",
                    type_name_of(&recv_ty),
                    self.cur_fn
                ),
            });
        };
        let info = self
            .sem
            .classes
            .get(class_name)
            .cloned()
            .ok_or_else(|| IrError {
                message: format!("内部错误：类 '{class_name}' 无信息（函数 {}）", self.cur_fn),
            })?;
        let sig = info.methods.get(method).cloned().ok_or_else(|| IrError {
            message: format!("内部错误：类 '{class_name}' 无方法 '{method}'（函数 {}）", self.cur_fn),
        })?;
        if sig.is_static {
            return Err(IrError {
                message: format!(
                    "内部错误：静态方法 '{method}' 被当作实例方法调用（函数 {}）",
                    self.cur_fn
                ),
            });
        }
        // receiver 地址（this 隐藏参数）
        let (this_ptr, _this_llvm) = self.gen_class_addr(receiver)?;
        // 定义类（继承遮蔽时取实际定义类）
        let owner = info.method_owner.get(method).cloned().unwrap_or_else(|| class_name.to_string());
        let mname = format!("{owner}${method}");
        self.emit_method_call(&mname, &sig, args, Some(&this_ptr))
    }

    /// 方法调用公共发射：参数列表组装 + call 指令（可选 this 首参）。
    fn emit_method_call(
        &mut self,
        mname: &str,
        sig: &MethodSig,
        args: &[Expr],
        this_ptr: Option<&str>,
    ) -> Result<(String, &'static str), IrError> {
        let mut arg_list = Vec::new();
        // this 首参：receiver 地址（ptr）
        if let Some(tp) = this_ptr {
            arg_list.push(format!("ptr {tp}"));
        }
        // 普通参数（类型以方法签名为准，字面量按签名类型写）
        for (a, want_ty) in args.iter().zip(sig.param_tys.iter()) {
            let (v, _t) = self.gen_expr(a)?;
            let aty = self.llvm_ty(want_ty);
            arg_list.push(format!("{aty} {v}"));
        }
        let ret_llvm = self.llvm_ty(&sig.ret_ty);
        let tmp = self.new_reg();
        if sig.ret_ty.is_void() {
            self.line(&format!("call void @{mname}({})", arg_list.join(", ")));
            Ok((tmp, "void"))
        } else {
            self.line(&format!("{tmp} = call {ret_llvm} @{mname}({})", arg_list.join(", ")));
            Ok((tmp, ret_llvm))
        }
    }

    /// 求类实例表达式的内存地址（供字段 GEP 与方法调用 this 使用）。
    ///
    /// 支持：
    /// - 变量（VarBind：alloca 指针 / by_ptr 的 this 参数指针）→ 直接返回绑定地址；
    /// - 字段链（obj.a.b）→ 递归：先取 obj 地址，再逐级 GEP 到字段。
    ///
    /// 返回 (地址寄存器, 该地址指向的结构体 LLVM 类型)。
    /// 语义层已保证表达式类型为类，此处仅内部防御。
    fn gen_class_addr(&mut self, expr: &Expr) -> Result<(String, &'static str), IrError> {
        match expr {
            // 变量/this：绑定地址即对象地址（alloca 或 by_ptr 参数）
            Expr::Var(name) => {
                let bind = self.lookup_var(name).cloned().ok_or_else(|| IrError {
                    message: format!("内部错误：变量 '{name}' 未入作用域（函数 {}）", self.cur_fn),
                })?;
                // by_ptr（this 参数）：value 即对象指针，直接使用；普通变量：alloca 指针。
                // 两者对 GEP 等价，这里统一按绑定地址返回。
                let _ = bind.by_ptr;
                Ok((bind.value, bind.ty))
            }
            // 字段链：取 base 地址 → GEP 到字段偏移
            Expr::FieldAccess { base, field, .. } => {
                let (base_ptr, base_llvm) = self.gen_class_addr(base)?;
                // base 语义类型必须是类（字段偏移取拍平 field_index）
                let base_ty = self.sem_ty_of(base).ok_or_else(|| IrError {
                    message: format!("内部错误：字段链缺少基类型（函数 {}）", self.cur_fn),
                })?;
                let TypeSpec::Class(class_name) = &base_ty else {
                    return Err(IrError {
                        message: format!(
                            "内部错误：字段链的基类型不是类（{}，函数 {}）",
                            type_name_of(&base_ty),
                            self.cur_fn
                        ),
                    });
                };
                let info = self
                    .sem
                    .classes
                    .get(class_name)
                    .cloned()
                    .ok_or_else(|| IrError {
                        message: format!("内部错误：类 '{class_name}' 无信息（函数 {}）", self.cur_fn),
                    })?;
                let idx = info.field_index.get(field).copied().ok_or_else(|| IrError {
                    message: format!(
                        "内部错误：类 '{class_name}' 无字段 '{field}'（函数 {}）",
                        self.cur_fn
                    ),
                })?;
                let fty = info.fields[idx].ty.clone().expect("字段类型已在类收集时解析");
                let f_llvm = self.llvm_ty(&fty);
                let ptr = self.new_reg();
                self.line(&format!(
                    "{ptr} = getelementptr {base_llvm}, ptr {base_ptr}, i32 0, i32 {idx}"
                ));
                Ok((ptr, f_llvm))
            }
            _ => Err(IrError {
                message: format!(
                    "内部错误：类实例必须可寻址（变量/this/字段链），函数 {}",
                    self.cur_fn
                ),
            }),
        }
    }

    /// println 生成：按参数类型选 printf 格式串。
    fn gen_println(&mut self, args: &[Expr]) -> Result<(String, &'static str), IrError> {
        if args.is_empty() {
            // 空 println → 只换行
            let fmt = self.fmt_global("\n");
            self.line(&format!("call i32 (ptr, ...) @printf(ptr @{fmt})"));
            return Ok((self.new_reg(), "void"));
        }
        let (v, t) = self.gen_expr(&args[0])?;
        // 查询语义类型：区分 char/i32 与有符号/无符号（LLVM 类型名无法区分）
        let sem = self.sem_ty_of(&args[0]);
        let is_unsigned = matches!(
            sem,
            Some(TypeSpec::Named(TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64))
        );
        let is_char = matches!(sem, Some(TypeSpec::Named(TyKw::Char)));
        // 按类型选 (格式串, 传参类型, 传参值)
        let (fmt, arg_ty, arg): (&str, &str, String) = match t {
            "double" => ("%f\n", "double", v),
            // f32 → f64（printf 变参提升规则），%f
            "float" => {
                let ext = self.new_reg();
                self.line(&format!("{ext} = fpext float {v} to double"));
                ("%f\n", "double", ext)
            }
            // 字符串：直接打印内容
            "ptr" => ("%s\n", "ptr", v),
            "i1" => {
                // bool 直接打印 0/1（v0.1 简化，后续版本转 true/false 文本）
                let ext = self.new_reg();
                self.line(&format!("{ext} = zext i1 {v} to i64"));
                ("%lld\n", "i64", ext)
            }
            // 窄整数（i8/i16）：提升到 i32 后按符号性选 %d/%u
            "i8" | "i16" => {
                let ext = self.new_reg();
                if is_unsigned {
                    self.line(&format!("{ext} = zext {t} {v} to i32"));
                    ("%u\n", "i32", ext)
                } else {
                    self.line(&format!("{ext} = sext {t} {v} to i32"));
                    ("%d\n", "i32", ext)
                }
            }
            // char → %c；i32 按符号性 %d/%u
            "i32" if is_char => ("%c\n", "i32", v),
            "i32" => {
                if is_unsigned {
                    ("%u\n", "i32", v)
                } else {
                    ("%d\n", "i32", v)
                }
            }
            // i64/u64：%lld/%llu
            "i64" => {
                if is_unsigned {
                    ("%llu\n", "i64", v)
                } else {
                    ("%lld\n", "i64", v)
                }
            }
            _ => ("%lld\n", "i64", v),
        };
        let g = self.fmt_global(fmt);
        self.line(&format!("call i32 (ptr, ...) @printf(ptr @{g}, {arg_ty} {arg})"));
        Ok((self.new_reg(), "void"))
    }

    // ---------- 工具 ----------

    /// 查询表达式的语义类型（区分有符号/无符号；LLVM 类型名无法区分 u32/i32）。
    fn sem_ty_of(&self, e: &Expr) -> Option<TypeSpec> {
        self.sem.expr_types.get(&(e as *const Expr as usize)).cloned()
    }

    /// 将整数统一扩展到 i64（for 循环变量固定为 i64；按符号性 sext/zext）。
    fn extend_int_to_i64(
        &mut self,
        val: &str,
        ty: &'static str,
        e: &Expr,
    ) -> Result<String, IrError> {
        if ty == "i64" {
            return Ok(val.to_string());
        }
        // 查语义类型判断无符号（uN → zext，其余 → sext）
        let unsigned = matches!(
            self.sem_ty_of(e),
            Some(TypeSpec::Named(TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64))
        );
        let ext = self.new_reg();
        if unsigned {
            self.line(&format!("{ext} = zext {ty} {val} to i64"));
        } else {
            self.line(&format!("{ext} = sext {ty} {val} to i64"));
        }
        Ok(ext)
    }

    /// 当前作用域可变引用。
    fn cur_scope_mut(&mut self) -> &mut HashMap<String, VarBind> {
        self.scopes.last_mut().expect("作用域栈不应为空")
    }

    /// 从作用域栈查找变量（从内到外）。
    fn lookup_var(&self, name: &str) -> Option<&VarBind> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    /// 变量名是否已在作用域中（用于区分方法调用的 receiver 是变量还是类名）。
    fn scope_has(&self, name: &str) -> bool {
        self.lookup_var(name).is_some()
    }

    /// 当前函数/方法的返回类型（Return 生成按签名类型适配字面量）。
    ///
    /// 普通函数名查 funcs 表；方法名形如 `类$方法`，从 classes 表的方法签名查
    /// （方法不在 funcs 表中，返回类型不能回落为 i64——如方法返回 string/类）。
    fn current_ret_ty(&self) -> TypeSpec {
        // 方法名：`类$方法`
        if let Some((class_name, method_name)) = self.cur_fn.split_once('$')
            && let Some(info) = self.sem.classes.get(class_name)
            && let Some(sig) = info.methods.get(method_name)
        {
            return sig.ret_ty.clone();
        }
        // 普通函数（或兜底）
        self.sem
            .funcs
            .get(&self.cur_fn)
            .map(|s| s.ret_ty.clone())
            .unwrap_or(TypeSpec::Named(TyKw::I64))
    }

    /// 新临时寄存器名（%1, %2, ...）。
    fn new_reg(&mut self) -> String {
        self.reg += 1;
        format!("%{}", self.reg)
    }

    /// 新基本块标签名。
    fn new_label(&mut self, prefix: &str) -> String {
        self.reg += 1;
        format!("{}.{}", prefix, self.reg)
    }

    /// 输出一行（带当前缩进）。
    fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// 判断当前基本块是否已终止（最后一条非空指令是 `ret` 或 `unreachable`）。
    ///
    /// 用于分支生成：若分支内已 return（块已终结），不能再追加 `br` 跳转，
    /// 否则会在 `ret` 后生成死代码指令，LLVM 会报「指令编号不连续」错误。
    fn block_terminated(&self) -> bool {
        self.out
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| {
                let t = l.trim();
                t.starts_with("ret ") || t.starts_with("unreachable")
            })
            .unwrap_or(false)
    }

    /// 基本块起始：输出标签行。
    fn block_start(&mut self, label: &str) {
        // 标签行不缩进
        self.out.push_str(&format!("{label}:\n"));
        self.indent();
    }

    /// 基本块结束：恢复缩进（由调用者保证下一个 block_start/函数尾）。
    fn block_end(&mut self) {
        self.dedent();
    }

    fn indent(&mut self) {
        self.out.push_str("  ");
    }

    fn dedent(&mut self) {
        // 简化：直接修改末尾（实际靠 line() 前的缩进管理）
        // 为保持简单，缩进统一用两空格前缀逻辑：这里留空实现
        let _ = ();
    }

    /// TypeSpec → LLVM 类型名。
    ///
    /// 元组/类类型都映射为 LLVM 字面结构体 `{T1, T2, ...}`：
    /// - 元组：递归拼接字段类型文本；
    /// - 类：取语义层拍平后的字段（父类字段在前，顺序即 GEP 偏移）。
    ///
    /// 动态分配后 Box::leak 获得 'static 生命周期，并用 ty_cache 去重
    /// （同一形状的类型只泄漏一份，编译器进程短期运行可接受）。
    fn llvm_ty(&mut self, t: &TypeSpec) -> &'static str {
        match t {
            TypeSpec::Named(_) => t.llvm_ty(),
            TypeSpec::Tuple(fields) => {
                let inner: Vec<&str> = fields.iter().map(|f| self.llvm_ty(&f.ty)).collect();
                let text = format!("{{{}}}", inner.join(", "));
                if let Some(s) = self.ty_cache.get(&text) {
                    return s;
                }
                // 先缓存再泄漏：text 作为缓存键与泄漏的 'static 字符串共用一份堆内存
                let leaked: &'static str = Box::leak(text.into_boxed_str());
                self.ty_cache.insert(leaked.to_string(), leaked);
                leaked
            }
            TypeSpec::Class(class_name) => {
                // 类 → 拍平字段结构体：字段类型已在语义层解析为 Some。
                // 类必然已收集（语义层保证），此处 expect 兜底（与元组字段解析一致）。
                let info = self
                    .sem
                    .classes
                    .get(class_name)
                    .unwrap_or_else(|| panic!("内部错误：类 '{class_name}' 无信息（函数 {}）", self.cur_fn));
                let inner: Vec<&str> = info
                    .fields
                    .iter()
                    .map(|f| {
                        let ft = f.ty.as_ref().expect("字段类型已在类收集时解析");
                        self.llvm_ty(ft)
                    })
                    .collect();
                let text = format!("{{{}}}", inner.join(", "));
                if let Some(s) = self.ty_cache.get(&text) {
                    return s;
                }
                let leaked: &'static str = Box::leak(text.into_boxed_str());
                self.ty_cache.insert(leaked.to_string(), leaked);
                leaked
            }
        }
    }

    /// 字符串全局常量（去重）。
    fn string_global(&mut self, s: &str) -> String {
        self.str_count += 1;
        let name = format!(".str.{}", self.str_count);
        let (len, escaped) = escape_ir_string(s);
        // 写入全局常量缓冲，run() 结束时统一输出到模块级
        self.globals.push_str(&format!(
            "@{name} = private unnamed_addr constant [{len} x i8] c\"{escaped}\"\n"
        ));
        name
    }

    /// 格式串全局常量（去重缓存）。
    fn fmt_global(&mut self, fmt: &str) -> String {
        if let Some(name) = self.fmt_cache.get(fmt) {
            return name.clone();
        }
        self.str_count += 1;
        let name = format!(".str.{}", self.str_count);
        let (len, escaped) = escape_ir_string(fmt);
        // 写入全局常量缓冲，run() 结束时统一输出到模块级
        self.globals.push_str(&format!(
            "@{name} = private unnamed_addr constant [{len} x i8] c\"{escaped}\"\n"
        ));
        self.fmt_cache.insert(fmt.to_string(), name.clone());
        name
    }
}

// ---------- 模块级工具 ----------

/// TypeSpec 的展示名（错误信息用；与前端语义层 type_name 规则一致）。
fn type_name_of(t: &TypeSpec) -> &'static str {
    match t {
        TypeSpec::Named(k) => match k {
            TyKw::I8 => "i8",
            TyKw::I16 => "i16",
            TyKw::I32 => "i32",
            TyKw::I64 => "i64",
            TyKw::U8 => "u8",
            TyKw::U16 => "u16",
            TyKw::U32 => "u32",
            TyKw::U64 => "u64",
            TyKw::F32 => "f32",
            TyKw::F64 => "f64",
            TyKw::Bool => "bool",
            TyKw::Char => "char",
            TyKw::Str => "string",
            TyKw::Void => "void",
            _ => "类型",
        },
        TypeSpec::Tuple(_) => "元组",
        TypeSpec::Class(_) => "类",
    }
}

/// 类型的零值文本（构造调用缺省字段兜底）：数字 0、浮点 0.0、bool false、指针 null。
fn zero_value(llvm_ty: &str) -> &'static str {
    match llvm_ty {
        "ptr" => "null",
        "double" | "float" => "0.0",
        "i1" => "false",
        _ => "0",
    }
}

/// 解析元组字段访问 `access`，返回字段下标（与前端语义层同规则）。
///
/// 支持三种形式：
/// - 命名：`.x` / `.q`（按字段名查找）；
/// - 位置：`.Item1`、`.Item2` …（1 起编号）；
/// - 数字：`.0`、`.1` …（0 起编号）。
///
/// 找不到返回 None（语义层已保证合法，此处仅内部防御）。
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

/// 变量名 mangling：防止与 LLVM 保留名冲突（当前原样 + 前缀）。
fn mangle(name: &str) -> String {
    format!("%{}", name)
}

/// 从 LLVM 数组类型名 `[N x T]` 中解析元素类型名 `T`。
///
/// 用于下标访问（GEP 后 load 的元素类型）。M2 只支持标量元素
/// （i1/i8/i16/i32/i64/float/double/ptr），返回静态字符串；非数组或嵌套返回 None。
/// 从 LLVM 数组类型名 `[N x T]` 中解析元素类型名 `T`。
///
/// 用于下标访问（GEP 后 load 的元素类型）。M2 只支持标量元素
/// （i1/i8/i16/i32/i64/float/double/ptr），返回静态字符串；非数组或嵌套返回 None。
fn parse_array_elem_ty(arr_ty: &str) -> Option<&'static str> {
    parse_array_shape(arr_ty).map(|(_, elem)| elem)
}

/// 从 LLVM 数组类型名 `[N x T]` 中解析 (长度 N, 元素类型名 T)。
///
/// 用于表遍历（生成 0..N 计数器循环）与下标访问。返回静态字符串。
fn parse_array_shape(arr_ty: &str) -> Option<(usize, &'static str)> {
    let rest = arr_ty.strip_prefix('[')?;
    let (len, elem) = rest.split_once(" x ")?;
    let len: usize = len.parse().ok()?;
    let elem = elem.strip_suffix(']')?;
    let elem = match elem {
        "i1" => Some("i1"),
        "i8" => Some("i8"),
        "i16" => Some("i16"),
        "i32" => Some("i32"),
        "i64" => Some("i64"),
        "float" => Some("float"),
        "double" => Some("double"),
        "ptr" => Some("ptr"),
        _ => None,
    }?;
    Some((len, elem))
}

/// 浮点字面量的 IR 文本（保证含小数点）。
fn format_float(v: f64) -> String {
    if v == v.trunc() && v.is_finite() && v.abs() < 1e15 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

/// 转义为 LLVM IR 字符串常量（返回 (长度含 \00, 转义文本)）。
fn escape_ir_string(s: &str) -> (usize, String) {
    let mut bytes: Vec<u8> = s.as_bytes().to_vec();
    bytes.push(0); // \00 结尾
    let len = bytes.len();
    let mut out = String::new();
    for b in bytes {
        match b {
            b'"' => out.push_str("\\22"),
            b'\\' => out.push_str("\\5C"),
            b'\n' => out.push_str("\\0A"),
            b'\t' => out.push_str("\\09"),
            b'\r' => out.push_str("\\0D"),
            0 => out.push_str("\\00"),
            0x20..=0x7E => out.push(b as char),
            other => out.push_str(&format!("\\{other:02X}")),
        }
    }
    (len, out)
}
