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
    /// 用到的 tie-interp 库导出符号（read_line/eval 等 REPL 内置函数）。
    ///
    /// driver 链接阶段据此**按需**链接 tie-interp 静态库：
    /// 非空才链接（普通程序不依赖 interp 库），空则不链接。
    pub used_externs: Vec<String>,
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
    /// 用到的 tie-interp 库导出符号（去重收集；link 阶段按需链接）
    used_externs: Vec<String>,
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
        used_externs: Vec::new(),
        cur_fn: String::new(),
    };
    generator.run()?;
    Ok(IrOutput { ir: generator.out, used_externs: generator.used_externs })
}

impl<'p> IrGenerator<'p> {
    // ---------- 模块级 ----------

    fn run(&mut self) -> Result<(), IrError> {
        // 模块头
        self.out.push_str("; ModuleID = 'tie'\n");
        self.out.push_str("source_filename = \"input.tie\"\n\n");
        // printf 声明（println/print 依赖）
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
        // M2 标准库 floor 的文件/进程原语（libc 符号）：
        // - fopen/fwrite/fclose：file_write/file_append/file_exists（编译模式用 libc，
        //   解释模式用 Rust std::fs，两者行为一致：写成功/追加增长/存在性检查）
        // - fflush/exit：exit() 与运行时错误路径（先刷新 stdout 再退出，保证消息可见）
        self.out.push_str("declare ptr @fopen(ptr, ptr)\n");
        self.out.push_str("declare i64 @fwrite(ptr, i64, i64, ptr)\n");
        self.out.push_str("declare i32 @fclose(ptr)\n");
        self.out.push_str("declare i32 @fflush(ptr)\n");
        self.out.push_str("declare void @exit(i32)\n");
        // M2 标准库 floor 的数学原语（libc/libm 符号，MSVC ucrt 提供）：
        // sqrt/sin/cos/tan/exp/log/pow/floor/ceil/round 是纯标量 f64→f64 运算，
        // 编译模式直接声明 libc 符号（解释模式用 Rust f64 方法，两者 IEEE 754 一致）。
        // 无条件声明（与 fopen 等一致）：未使用的 extern 声明对 LLVM/clang 无害。
        self.out.push_str("declare double @sqrt(double)\n");
        self.out.push_str("declare double @sin(double)\n");
        self.out.push_str("declare double @cos(double)\n");
        self.out.push_str("declare double @tan(double)\n");
        self.out.push_str("declare double @exp(double)\n");
        self.out.push_str("declare double @log(double)\n");
        self.out.push_str("declare double @pow(double, double)\n");
        self.out.push_str("declare double @floor(double)\n");
        self.out.push_str("declare double @ceil(double)\n");
        self.out.push_str("declare double @round(double)\n\n");

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

        // tie-interp 库导出符号声明：仅在用到 read_line/eval 时输出（按需）。
        // 符号与 crates/tie-interp/src/lib.rs 的 #[unsafe(no_mangle)] 导出一一对应。
        if self.used_externs.iter().any(|s| s == "tie_read_line") {
            self.out
                .push_str("declare ptr @tie_read_line()\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_eval_expr") {
            self.out.push_str("declare ptr @tie_eval_expr(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_free_result") {
            self.out.push_str("declare void @tie_free_result(ptr)\n");
        }
        // M2 标准库 floor 的 C ABI 桥（返回堆串/需解析的原语）：
        // 与解释路径共用同一份 Rust 实现，保证两路径行为逐字节一致。
        // 符号与 crates/tie-interp/src/lib.rs 的 #[unsafe(no_mangle)] 导出一一对应。
        if self.used_externs.iter().any(|s| s == "tie_file_read") {
            self.out.push_str("declare ptr @tie_file_read(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_str_char") {
            self.out.push_str("declare ptr @tie_str_char(ptr, i64)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_to_string_i64") {
            self.out.push_str("declare ptr @tie_to_string_i64(i64)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_to_string_f64") {
            self.out.push_str("declare ptr @tie_to_string_f64(double)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_parse_int") {
            self.out.push_str("declare i64 @tie_parse_int(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_parse_float") {
            self.out.push_str("declare double @tie_parse_float(ptr, ptr)\n");
        }
        // M2 标准库 floor 的时间/随机原语（C ABI 桥，与解释路径共用实现）：
        // tie_time_now 返回 Unix 秒；tie_rand_range 带 ok 标志（max<=min 时置 0）。
        if self.used_externs.iter().any(|s| s == "tie_time_now") {
            self.out.push_str("declare i64 @tie_time_now()\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_rand_range") {
            self.out.push_str("declare i64 @tie_rand_range(i64, i64, ptr)\n");
        }
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
                let (v, _ty) = self.gen_expr(&e.expr)?;
                // REPL 内置作为独立语句（结果丢弃）：read_line()/eval() 返回
                // tie-interp 堆串，立即释放避免累积泄漏。
                // 注意：仅处理顶层调用；作为变量初值/参数时 v1 接受会话级小泄漏
                // （REPL 短期会话，量级可忽略）。
                // 返回 tie-interp 堆串的内置作为独立语句（结果丢弃）时立即释放，
                // 避免累积泄漏（与 read_line/eval 同一机制；file_read/str_char/to_string
                // 为 M2 floor 新增的堆串返回原语）。
                if matches!(
                    &e.expr,
                    Expr::Call { name, .. }
                        if name == "read_line"
                            || name == "eval"
                            || name == "file_read"
                            || name == "str_char"
                            || name == "to_string"
                ) {
                    self.mark_used("tie_free_result");
                    self.line(&format!("call void @tie_free_result(ptr {v})"));
                }
                Ok(())
            }
            Stmt::Assign(a) => {
                // 赋值：查找目标变量绑定（语义已保证存在且非 const）
                let bind = self.lookup_var(&a.target).cloned().ok_or_else(|| IrError {
                    message: format!("内部错误：赋值目标 '{}' 未入作用域（函数 {}）", a.target, self.cur_fn),
                })?;
                match a.op {
                    // 普通赋值：直接求右值并 store（按变量的声明类型，语义已保证类型匹配）
                    None => {
                        let (val, _ty) = self.gen_expr(&a.value)?;
                        self.line(&format!("store {} {}, ptr {}", bind.ty, val, bind.value));
                    }
                    // 复合赋值（+= -= *= /= %= &= |= ^= <<= >>=，M4）：
                    // load 目标当前值 → 与右值做二元运算 → store 结果回目标。
                    // 运算指令生成复用 gen_binary_on_regs（与 gen_binary 同一套逻辑）。
                    Some(op) => {
                        let (rv, _rty) = self.gen_expr(&a.value)?;
                        let cur = self.new_reg();
                        self.line(&format!("{cur} = load {}, ptr {}", bind.ty, bind.value));
                        // 目标是否字符串：LLVM 类型名 "ptr" 无法区分字符串与裸指针。
                        // 复合赋值目标为 ptr 的场景只有字符串拼接（+=），
                        // 类/元组/数组不进标量复合赋值（语义层已拦），故用 ptr 近似。
                        let lhs_is_str = bind.ty == "ptr";
                        // 无符号性：右值表达式的语义类型近似（右值非字面量时与目标同型，
                        // 字面量默认 i64 有符号——无符号复合除法/取模/右移是边缘场景，
                        // 早期开发按有符号处理可接受）。
                        let rhs_is_unsigned = matches!(
                            self.sem_ty_of(&a.value),
                            Some(TypeSpec::Named(TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64))
                        );
                        let (res, _t) = self.gen_binary_on_regs(
                            op,
                            lhs_is_str,
                            cur,
                            bind.ty,
                            rv,
                            rhs_is_unsigned,
                        )?;
                        self.line(&format!("store {} {}, ptr {}", bind.ty, res, bind.value));
                    }
                }
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
        // GEP 定位字段地址（普通赋值与复合赋值共用）
        let ptr = self.new_reg();
        self.line(&format!("{ptr} = getelementptr {base_llvm}, ptr {base_ptr}, i32 0, i32 {idx}"));
        match fa.op {
            // 普通赋值：直接 store
            None => {
                let (val, _t) = self.gen_expr(&fa.value)?;
                self.line(&format!("store {f_llvm} {val}, ptr {ptr}"));
            }
            // 复合字段赋值（obj.f += v，M4）：
            // load 字段当前值 → 与右值运算 → store 结果回字段偏移。
            // 运算生成复用 gen_binary_on_regs；字符串字段拼接（obj.s += "x"）同样支持。
            Some(op) => {
                let (rv, _rty) = self.gen_expr(&fa.value)?;
                let cur = self.new_reg();
                self.line(&format!("{cur} = load {f_llvm}, ptr {ptr}"));
                // 无符号性：右值表达式语义类型近似（与标量复合赋值同一简化决策）
                let lhs_is_str = f_llvm == "ptr";
                let rhs_is_unsigned = matches!(
                    self.sem_ty_of(&fa.value),
                    Some(TypeSpec::Named(TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64))
                );
                let (res, _t) = self.gen_binary_on_regs(
                    op,
                    lhs_is_str,
                    cur,
                    f_llvm,
                    rv,
                    rhs_is_unsigned,
                )?;
                self.line(&format!("store {f_llvm} {res}, ptr {ptr}"));
            }
        }
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
                // 自增/自减（M4）：操作数必须是变量（语义层保证），
                // 需直接读写其 alloca（load → 运算 → store），不能先 gen_expr——
                // 否则只拿到 load 出的临时寄存器，无法写回。
                if matches!(
                    op,
                    UnaryOp::PreInc | UnaryOp::PreDec | UnaryOp::PostInc | UnaryOp::PostDec
                ) {
                    return self.gen_inc_dec(*op, operand);
                }
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
                    // 自增/自减已在上方 gen_inc_dec 提前返回，此处不可达
                    UnaryOp::PreInc | UnaryOp::PreDec | UnaryOp::PostInc | UnaryOp::PostDec => {
                        unreachable!("自增/自减已在 gen_inc_dec 中处理")
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => self.gen_binary(*op, lhs, rhs),
            Expr::Ternary { cond, then_expr, else_expr, .. } => {
                // 三目运算 `cond ? then : else`（M4）：短路求值。
                // 结构（与 gen_if 同构的三块 + phi 汇合）：
                //   br i1 %cond, label %tern.then.N, label %tern.else.N
                //   tern.then.N: 求 then；br label %tern.merge.N
                //   tern.else.N: 求 else；br label %tern.merge.N
                //   tern.merge.N: %r = phi T [ then, %tern.then.N ], [ else, %tern.else.N ]
                // 表达式内没有 return 语句（语义层限制），分支内无需 block_terminated
                // 检查，直接 br merge 即可。
                let (cond, _) = self.gen_expr(cond)?;
                let then_label = self.new_label("tern.then");
                let else_label = self.new_label("tern.else");
                let merge_label = self.new_label("tern.merge");
                self.line(&format!("br i1 {cond}, label %{then_label}, label %{else_label}"));

                // then 分支：求 then 分支值
                self.block_start(&then_label);
                let (tv, t_ty) = self.gen_expr(then_expr)?;
                self.line(&format!("br label %{merge_label}"));
                self.block_end();

                // else 分支：求 else 分支值
                self.block_start(&else_label);
                let (ev, _e_ty) = self.gen_expr(else_expr)?;
                self.line(&format!("br label %{merge_label}"));
                self.block_end();

                // 合并块：phi 汇合两分支值（语义层保证两分支同型，类型取 then 分支）
                self.block_start(&merge_label);
                let phi = self.new_reg();
                self.line(&format!(
                    "{phi} = phi {t_ty} [ {tv}, %{then_label} ], [ {ev}, %{else_label} ]"
                ));
                Ok((phi, t_ty))
            }
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

    /// 自增/自减（M4）：对变量 alloca 执行 load → add/sub → store。
    ///
    /// - 前缀（++x/--x）：返回运算后的**新值**；
    /// - 后缀（x++/x--）：返回运算前的**旧值**。
    ///
    /// 简化决策：仅支持变量操作数（[Expr::Var]，语义层已保证）；对象字段的自增/自减
    /// （obj.f++）留后续版本，此处返回明确 IrError（GEP + load + op + store 链未实现）。
    fn gen_inc_dec(&mut self, op: UnaryOp, operand: &Expr) -> Result<(String, &'static str), IrError> {
        // 仅支持变量操作数（语义层已保证；此处防御）
        let Expr::Var(name) = operand else {
            return Err(IrError {
                message: "暂不支持对象字段的自增/自减（M4 简化：++/-- 仅支持变量）".into(),
            });
        };
        // 取变量绑定：alloca 指针 + LLVM 类型名（照抄 gen_stmt Assign 的 lookup_var 用法）
        let bind = self.lookup_var(name).cloned().ok_or_else(|| IrError {
            message: format!("内部错误：自增/自减的变量 '{name}' 未入作用域（函数 {}）", self.cur_fn),
        })?;
        let ty = bind.ty;
        // 自增/自减只对数字类型合法（语义层已保证；此处防御性报错，避免生成非法指令）
        let is_float = ty == "double" || ty == "float";
        if ty == "ptr" || ty.starts_with('{') || ty.starts_with('[') {
            return Err(IrError {
                message: format!("自增/自减只支持数字类型（{} 不能自增/自减）", ty),
            });
        }
        // load 当前值
        let cur = self.new_reg();
        self.line(&format!("{cur} = load {ty}, ptr {}", bind.value));
        // 新值：整数 add/sub 1（LLVM 立即数 1 即 iN 字面量，与任意整数宽度兼容）；
        // 浮点 fadd/fsub 1.0
        let new = self.new_reg();
        let one = if is_float { "1.0" } else { "1" };
        match op {
            UnaryOp::PreInc | UnaryOp::PostInc => {
                let opcode = if is_float { "fadd" } else { "add" };
                self.line(&format!("{new} = {opcode} {ty} {cur}, {one}"));
            }
            UnaryOp::PreDec | UnaryOp::PostDec => {
                let opcode = if is_float { "fsub" } else { "sub" };
                self.line(&format!("{new} = {opcode} {ty} {cur}, {one}"));
            }
            UnaryOp::Neg | UnaryOp::Not => unreachable!("Neg/Not 不走 gen_inc_dec"),
        }
        // store 新值回 alloca
        self.line(&format!("store {ty} {new}, ptr {}", bind.value));
        // 前缀返回新值，后缀返回旧值
        match op {
            UnaryOp::PreInc | UnaryOp::PreDec => Ok((new, ty)),
            UnaryOp::PostInc | UnaryOp::PostDec => Ok((cur, ty)),
            UnaryOp::Neg | UnaryOp::Not => unreachable!("Neg/Not 不走 gen_inc_dec"),
        }
    }

    /// 二元运算生成：两侧表达式求值后交给 [gen_binary_on_regs] 统一生成指令。
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
        // 无符号性：查语义类型（LLVM 类型名 "i32" 无法区分 u32/i32）
        let lhs_sem = self.sem_ty_of(lhs);
        let is_unsigned = matches!(
            lhs_sem,
            Some(TypeSpec::Named(TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64))
        );
        self.gen_binary_on_regs(op, lhs_is_str || rhs_is_str, lv, lt, rv, is_unsigned)
    }

    /// 在已求值的寄存器对上执行二元运算（gen_binary 与复合赋值共用的核心）。
    ///
    /// - `lhs_is_str`：左操作数是字符串（拼接/比较走 [gen_binary_str] 运行时函数）；
    /// - `lv`/`lt`：左操作数寄存器与其 LLVM 类型名（复合赋值中为「load 出的目标当前值」）；
    /// - `rv`：右操作数寄存器；
    /// - `is_unsigned`：无符号标志（决定 udiv/urem/lshr 等指令语义）。
    ///
    /// 返回 (结果寄存器, LLVM 类型名)；比较/逻辑结果为 i1，其余同操作数类型。
    fn gen_binary_on_regs(
        &mut self,
        op: BinaryOp,
        lhs_is_str: bool,
        lv: String,
        lt: &'static str,
        rv: String,
        is_unsigned: bool,
    ) -> Result<(String, &'static str), IrError> {
        // 字符串操作：拼接（+）与比较（== != < > <= >=）走运行时函数
        if lhs_is_str {
            return self.gen_binary_str(op, lv, rv);
        }
        // 类型以左侧为准（语义已保证一致）
        let ty = lt;
        // 是否为浮点类型（f32→float / f64→double）
        let is_float = ty == "double" || ty == "float";
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
            // M4 位运算（仅整数，语义层已保证两侧同为整数类型）：
            // 按位与/或/异或与逻辑 and/or 指令同助记符，仅操作数类型不同
            //（逻辑为 i1，位运算为整数类型）。
            BinaryOp::BitAnd => format!("and {ty} {lv}, {rv}"),
            BinaryOp::BitOr => format!("or {ty} {lv}, {rv}"),
            BinaryOp::BitXor => format!("xor {ty} {lv}, {rv}"),
            // 左移：无符号性不影响 shl
            BinaryOp::Shl => format!("shl {ty} {lv}, {rv}"),
            // 右移：有符号算术右移（ashr），无符号逻辑右移（lshr）
            BinaryOp::Shr => {
                if is_unsigned {
                    format!("lshr {ty} {lv}, {rv}")
                } else {
                    format!("ashr {ty} {lv}, {rv}")
                }
            }
        };
        self.line(&format!("{tmp} = {instr}"));
        // 比较/逻辑结果为 i1；位运算/移位/算术结果同操作数类型
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
            // 位运算/移位/逻辑对字符串不合法（语义层已拦截），此处防御性报错
            // 而非 unreachable panic（match 穷尽 + 错误信息明确）。
            _ => {
                return Err(IrError {
                    message: format!("字符串二元运算只允许 + 与比较（不支持的运算符：{op:?}）"),
                });
            }
        };
        let res = self.new_reg();
        self.line(&format!("{res} = icmp {icmp} i32 {cmp}, 0"));
        Ok((res, "i1"))
    }

    /// 函数调用生成：内置 println/print/len/read_line/eval → printf/strlen/interp 库；
    /// 用户函数 → call。
    fn gen_call(&mut self, name: &str, args: &[Expr]) -> Result<(String, &'static str), IrError> {
        if name == "println" {
            return self.gen_printf(args, true);
        }
        if name == "print" {
            return self.gen_printf(args, false);
        }
        // 内置 len：字符串长度或表元素个数（语义已保证单参数为字符串或表）。
        // 表长度编译期已知（tie 表定长）：表字面量查语义 tables 元数据，表变量查 LLVM
        // 数组类型 `[N x T]` 的 N，均直接输出常量；字符串走 strlen。
        if name == "len" {
            // 表字面量参数：直接查语义元数据（避免 gen_expr 对 TableLit 报错）
            if let Expr::TableLit { .. } = &args[0] {
                let key = &args[0] as *const Expr as usize;
                let info = self.sem.tables.get(&key).ok_or_else(|| IrError {
                    message: format!("内部错误：len 的表字面量缺少布局元数据（函数 {}）", self.cur_fn),
                })?;
                return Ok((info.len.to_string(), "i64"));
            }
            let (v, v_ty) = self.gen_expr(&args[0])?;
            // 表变量：LLVM 类型为 `[N x T]`，长度 N 编译期已知
            if let Some((n, _)) = parse_array_shape(v_ty) {
                return Ok((n.to_string(), "i64"));
            }
            // 字符串：strlen
            let len = self.new_reg();
            self.line(&format!("{len} = call i64 @strlen(ptr {v})"));
            return Ok((len, "i64"));
        }
        // 内置 read_line：零参数，调用 tie-interp 库读一行（REPL 自举）。
        // 语义层已保证无参数；返回值是 tie-interp 分配的堆串，调用方用完必须
        // tie_free_result 释放（repl 场景在 gen_stmt 的 Expr 分支统一释放）。
        if name == "read_line" {
            self.mark_used("tie_read_line");
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_read_line()"));
            return Ok((tmp, "ptr"));
        }
        // 内置 eval：单字符串参数，调用 tie-interp 库动态求值代码（REPL 自举）。
        // 语义层已保证恰好 1 个字符串参数；返回值同上为堆串（调用方负责释放）。
        if name == "eval" {
            self.mark_used("tie_eval_expr");
            self.mark_used("tie_free_result");
            let (v, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_eval_expr(ptr {v})"));
            return Ok((tmp, "ptr"));
        }
        // ---------- M2 标准库 floor 内置函数 ----------
        //
        // 设计说明（编译/解释两路径一致性的关键）：
        // - 返回堆串/需解析的原语（file_read / str_char / to_string / parse_int / parse_float）
        //   走 tie-interp 的 C ABI 桥——与解释路径共用同一份 Rust 实现，保证行为逐字节一致
        //   （如 to_string 的 Rust `{}` 格式与 printf %f 不同：1.0 → "1" vs "1.000000"；
        //   parse 的 Rust 严格解析与 strtoll 宽松解析不同："12abc" 前者报错后者返回 12）。
        // - 文件写/存在性（file_write / file_append / file_exists）编译模式用 libc
        //   （fopen/fwrite/fclose），解释模式用 Rust std::fs，两者均返回 bool、无错误消息，
        //   常规文件行为一致；用二进制模式（"wb"/"ab"/"rb"）避免 Windows 文本模式
        //   把 \n 转成 \r\n（与 std::fs 的字节语义一致）。
        // - 返回堆串的内置（file_read / str_char / to_string）沿用 read_line 的堆串机制：
        //   返回值由 tie-interp 分配，调用方用完必须 tie_free_result（独立语句时在
        //   gen_stmt 的 Expr 分支统一释放），无泄漏、无重复释放。

        // 内置 file_read：单字符串参数，返回 string（读取文件全部内容）。
        // 失败（C ABI 桥返回 NULL）→ 运行时错误，文本与解释路径一致。
        if name == "file_read" {
            self.mark_used("tie_file_read");
            self.mark_used("tie_free_result");
            let (v, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_file_read(ptr {v})"));
            // 判断返回 NULL：失败 → 错误块（退出进程），成功 → ok 块继续
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {tmp}, null"));
            let ok_label = self.new_label("file_read.ok");
            let err_label = self.new_label("file_read.err");
            self.line(&format!("br i1 {is_null}, label %{err_label}, label %{ok_label}"));
            self.block_start(&err_label);
            self.gen_runtime_error(
                "运行时错误: file_read 无法读取文件 '%s'",
                &[("ptr", v)],
            );
            self.block_end();
            // 成功块：返回值即堆串（调用方负责 tie_free_result）
            self.block_start(&ok_label);
            return Ok((tmp, "ptr"));
        }
        // 内置 file_write / file_append：两个字符串参数，返回 bool（成功与否）。
        // file_write 覆盖写（"wb"），file_append 追加写（"ab"）。
        if name == "file_write" || name == "file_append" {
            let (p, _t) = self.gen_expr(&args[0])?;
            let (c, _t) = self.gen_expr(&args[1])?;
            let mode = if name == "file_write" { "wb" } else { "ab" };
            let mode_g = self.string_global(mode);
            let f = self.new_reg();
            self.line(&format!("{f} = call ptr @fopen(ptr {p}, ptr @{mode_g})"));
            // 打开失败 → false；成功 → fwrite + fclose，返回 (写入字节数 == 内容长度)
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {f}, null"));
            let ok_label = self.new_label("file.open.ok");
            let fail_label = self.new_label("file.open.fail");
            let merge_label = self.new_label("file.open.merge");
            self.line(&format!("br i1 {is_null}, label %{fail_label}, label %{ok_label}"));
            // 打开失败：返回 false
            self.block_start(&fail_label);
            self.line(&format!("br label %{merge_label}"));
            self.block_end();
            // 打开成功：写入内容并关闭，返回写入是否完整
            self.block_start(&ok_label);
            let len = self.new_reg();
            self.line(&format!("{len} = call i64 @strlen(ptr {c})"));
            let written = self.new_reg();
            self.line(&format!("{written} = call i64 @fwrite(ptr {c}, i64 1, i64 {len}, ptr {f})"));
            self.line(&format!("call i32 @fclose(ptr {f})"));
            // 非 void 的未编号调用（fclose 返回 i32）会被解析器分配隐式寄存器号，
            // 必须用 new_reg 消费掉，否则后续寄存器编号错位（与 gen_printf 的哑返回同理）
            let _ = self.new_reg();
            let ok = self.new_reg();
            self.line(&format!("{ok} = icmp eq i64 {written}, {len}"));
            self.line(&format!("br label %{merge_label}"));
            self.block_end();
            // 合并块：phi 汇合两分支结果
            self.block_start(&merge_label);
            let res = self.new_reg();
            self.line(&format!("{res} = phi i1 [ false, %{fail_label} ], [ {ok}, %{ok_label} ]"));
            return Ok((res, "i1"));
        }
        // 内置 file_exists：单字符串参数，返回 bool（文件是否存在）。
        // 用 fopen(path, "rb") 探测：能打开即存在。
        // 注：与解释路径 std::fs::exists 的差异——目录/不可读文件会返回 false
        //（fopen 需可读），常规文件测试两者一致。
        if name == "file_exists" {
            let (p, _t) = self.gen_expr(&args[0])?;
            let mode_g = self.string_global("rb");
            let f = self.new_reg();
            self.line(&format!("{f} = call ptr @fopen(ptr {p}, ptr @{mode_g})"));
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {f}, null"));
            let ok_label = self.new_label("file.exists.ok");
            let fail_label = self.new_label("file.exists.fail");
            let merge_label = self.new_label("file.exists.merge");
            self.line(&format!("br i1 {is_null}, label %{fail_label}, label %{ok_label}"));
            // 打开失败：不存在 → false
            self.block_start(&fail_label);
            self.line(&format!("br label %{merge_label}"));
            self.block_end();
            // 打开成功：关闭并返回 true
            self.block_start(&ok_label);
            self.line(&format!("call i32 @fclose(ptr {f})"));
            // 非 void 的未编号调用（fclose 返回 i32）会被解析器分配隐式寄存器号，必须消费掉
            let _ = self.new_reg();
            self.line(&format!("br label %{merge_label}"));
            self.block_end();
            // 合并块：phi 汇合两分支结果
            self.block_start(&merge_label);
            let res = self.new_reg();
            self.line(&format!("{res} = phi i1 [ false, %{fail_label} ], [ true, %{ok_label} ]"));
            return Ok((res, "i1"));
        }
        // 内置 str_char：字符串 + 整数下标，返回 string（第 i 个 Unicode 码点）。
        // 走 C ABI 桥（Rust chars().nth 解码 UTF-8），保证多字节字符两路径一致。
        if name == "str_char" {
            self.mark_used("tie_str_char");
            self.mark_used("tie_free_result");
            let (s, _t) = self.gen_expr(&args[0])?;
            let (i, i_ty) = self.gen_expr(&args[1])?;
            // 下标统一扩展到 i64（C ABI 桥的第二个参数类型）
            let i64 = self.extend_int_to_i64(&i, i_ty, &args[1])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_str_char(ptr {s}, i64 {i64})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 to_string：单数字参数（i64/f64），返回 string（数字格式化）。
        // 按实参类型分派 i64/f64 桥（与解释路径一致）。
        if name == "to_string" {
            let (v, v_ty) = self.gen_expr(&args[0])?;
            // 查语义类型区分浮点/整数（LLVM 类型名 "i64"/"double" 已能区分，语义表兜底）
            let is_float = matches!(
                self.sem_ty_of(&args[0]),
                Some(TypeSpec::Named(TyKw::F32 | TyKw::F64))
            ) || v_ty == "double"
                || v_ty == "float";
            if is_float {
                self.mark_used("tie_to_string_f64");
                // f32 → f64 提升（C ABI 桥接收 double）
                let v64 = if v_ty == "float" {
                    let ext = self.new_reg();
                    self.line(&format!("{ext} = fpext float {v} to double"));
                    ext
                } else {
                    v
                };
                let tmp = self.new_reg();
                self.line(&format!("{tmp} = call ptr @tie_to_string_f64(double {v64})"));
                return Ok((tmp, "ptr"));
            }
            self.mark_used("tie_to_string_i64");
            // 整数统一扩展到 i64（C ABI 桥接收 i64）
            let v64 = self.extend_int_to_i64(&v, v_ty, &args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_to_string_i64(i64 {v64})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 parse_int：字符串参数，返回 i64（非法输入 → 运行时错误）。
        // 走 C ABI 桥（Rust 严格 parse），错误文本与解释路径一致。
        if name == "parse_int" {
            self.mark_used("tie_parse_int");
            let (s, _t) = self.gen_expr(&args[0])?;
            // 栈上分配 ok 标志（桥写入 0/1），调用后检查
            let ok = self.new_reg();
            self.line(&format!("{ok} = alloca i8"));
            self.line(&format!("store i8 0, ptr {ok}"));
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call i64 @tie_parse_int(ptr {s}, ptr {ok})"));
            let okv = self.new_reg();
            self.line(&format!("{okv} = load i8, ptr {ok}"));
            let is_zero = self.new_reg();
            self.line(&format!("{is_zero} = icmp eq i8 {okv}, 0"));
            let ok_label = self.new_label("parse_int.ok");
            let err_label = self.new_label("parse_int.err");
            self.line(&format!("br i1 {is_zero}, label %{err_label}, label %{ok_label}"));
            // 解析失败 → 运行时错误
            self.block_start(&err_label);
            self.gen_runtime_error(
                "运行时错误: parse_int 参数 '%s' 不是合法的整数",
                &[("ptr", s)],
            );
            self.block_end();
            // 成功块：返回解析值
            self.block_start(&ok_label);
            return Ok((tmp, "i64"));
        }
        // 内置 parse_float：字符串参数，返回 f64（非法输入 → 运行时错误）。
        if name == "parse_float" {
            self.mark_used("tie_parse_float");
            let (s, _t) = self.gen_expr(&args[0])?;
            let ok = self.new_reg();
            self.line(&format!("{ok} = alloca i8"));
            self.line(&format!("store i8 0, ptr {ok}"));
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call double @tie_parse_float(ptr {s}, ptr {ok})"));
            let okv = self.new_reg();
            self.line(&format!("{okv} = load i8, ptr {ok}"));
            let is_zero = self.new_reg();
            self.line(&format!("{is_zero} = icmp eq i8 {okv}, 0"));
            let ok_label = self.new_label("parse_float.ok");
            let err_label = self.new_label("parse_float.err");
            self.line(&format!("br i1 {is_zero}, label %{err_label}, label %{ok_label}"));
            // 解析失败 → 运行时错误
            self.block_start(&err_label);
            self.gen_runtime_error(
                "运行时错误: parse_float 参数 '%s' 不是合法的浮点数",
                &[("ptr", s)],
            );
            self.block_end();
            // 成功块：返回解析值
            self.block_start(&ok_label);
            return Ok((tmp, "double"));
        }
        // 内置 exit：整数参数，void（刷新 stdout 后终止进程）。
        // 编译路径：fflush(NULL) 刷新全部流 → libc exit；解释路径：stdout().flush() + exit。
        if name == "exit" {
            let (c, c_ty) = self.gen_expr(&args[0])?;
            self.line("call i32 @fflush(ptr null)");
            // 非 void 的未编号调用（fflush 返回 i32）会被解析器分配隐式寄存器号，必须消费掉
            let _ = self.new_reg();
            // 退出码统一转为 i32（libc exit 签名）：i32 直接用，i64 截断，窄整数符号扩展
            let c32 = if c_ty == "i32" {
                c
            } else if c_ty == "i64" {
                let t = self.new_reg();
                self.line(&format!("{t} = trunc i64 {c} to i32"));
                t
            } else {
                let t = self.new_reg();
                self.line(&format!("{t} = sext {c_ty} {c} to i32"));
                t
            };
            self.line(&format!("call void @exit(i32 {c32})"));
            // 终止当前块（exit 不返回；gen_fn 据此不再补 ret）
            self.line("unreachable");
            return Ok((self.new_reg(), "void"));
        }
        // ---------- M2 数学/时间/随机 floor 内置函数 ----------
        //
        // 数学函数（sqrt/sin/cos/tan/exp/log/floor/ceil/round）走 libc/libm
        // （@sqrt/@sin/...，MSVC ucrt 提供），解释路径用 Rust f64 方法，两者 IEEE 754 一致。
        // 整数实参统一提升为 double（sitofp）。log 需 x>0：x<=0 报错（与解释路径一致）。
        if matches!(
            name,
            "sqrt" | "sin" | "cos" | "tan" | "exp" | "log" | "floor" | "ceil" | "round"
        ) {
            let (v, v_ty) = self.gen_expr(&args[0])?;
            // 整数参数提升为 double（sitofp）；float 提升为 double（fpext）
            let vd = self.promote_to_double(&v, v_ty)?;
            // log 需要 x > 0：x<=0 → 运行时错误（与解释路径 f64::ln 前的检查一致）
            if name == "log" {
                let is_le = self.new_reg();
                self.line(&format!("{is_le} = fcmp ole double {vd}, 0.0"));
                let ok_label = self.new_label("log.ok");
                let err_label = self.new_label("log.err");
                self.line(&format!("br i1 {is_le}, label %{err_label}, label %{ok_label}"));
                self.block_start(&err_label);
                self.gen_runtime_error("运行时错误: log 参数必须大于 0", &[]);
                self.block_end();
                self.block_start(&ok_label);
            }
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call double @{name}(double {vd})"));
            return Ok((tmp, "double"));
        }
        // 内置 pow：两个数字参数，返回 f64（x^y）。
        if name == "pow" {
            let (x, x_ty) = self.gen_expr(&args[0])?;
            let (y, y_ty) = self.gen_expr(&args[1])?;
            let xd = self.promote_to_double(&x, x_ty)?;
            let yd = self.promote_to_double(&y, y_ty)?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call double @pow(double {xd}, double {yd})"));
            return Ok((tmp, "double"));
        }
        // 内置 time_now：零参数，返回 i64（Unix 纪元秒数）。走 C ABI 桥（与解释路径共用）。
        if name == "time_now" {
            self.mark_used("tie_time_now");
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call i64 @tie_time_now()"));
            return Ok((tmp, "i64"));
        }
        // 内置 rand_range：两个整数参数，返回 i64（[min, max) 内随机整数）。
        // 走 C ABI 桥（ok 标志模式，与 parse_int 一致）；max<=min → 运行时错误。
        if name == "rand_range" {
            self.mark_used("tie_rand_range");
            let (min, min_ty) = self.gen_expr(&args[0])?;
            let (max, max_ty) = self.gen_expr(&args[1])?;
            let min64 = self.extend_int_to_i64(&min, min_ty, &args[0])?;
            let max64 = self.extend_int_to_i64(&max, max_ty, &args[1])?;
            let ok = self.new_reg();
            self.line(&format!("{ok} = alloca i8"));
            self.line(&format!("store i8 0, ptr {ok}"));
            let tmp = self.new_reg();
            self.line(&format!(
                "{tmp} = call i64 @tie_rand_range(i64 {min64}, i64 {max64}, ptr {ok})"
            ));
            let okv = self.new_reg();
            self.line(&format!("{okv} = load i8, ptr {ok}"));
            let is_zero = self.new_reg();
            self.line(&format!("{is_zero} = icmp eq i8 {okv}, 0"));
            let ok_label = self.new_label("rand_range.ok");
            let err_label = self.new_label("rand_range.err");
            self.line(&format!("br i1 {is_zero}, label %{err_label}, label %{ok_label}"));
            // 范围无效 → 运行时错误
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: rand_range 参数范围无效（max 必须大于 min）", &[]);
            self.block_end();
            // 成功块：返回随机值
            self.block_start(&ok_label);
            return Ok((tmp, "i64"));
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

    /// println/print 生成：按参数类型选 printf 格式串。
    ///
    /// `newline` 为 true 时格式串末尾追加 `\n`（println），否则不换行（print）。
    fn gen_printf(&mut self, args: &[Expr], newline: bool) -> Result<(String, &'static str), IrError> {
        let nl = if newline { "\n" } else { "" };
        if args.is_empty() {
            // 空 println → 只换行；空 print → 无操作
            if newline {
                let fmt = self.fmt_global("\n");
                self.line(&format!("call i32 (ptr, ...) @printf(ptr @{fmt})"));
            }
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
        // 按类型选 (格式串, 传参类型, 传参值)；格式串统一追加换行/不换行后缀
        let (fmt, arg_ty, arg): (&str, &str, String) = match t {
            "double" => ("%f", "double", v),
            // f32 → f64（printf 变参提升规则），%f
            "float" => {
                let ext = self.new_reg();
                self.line(&format!("{ext} = fpext float {v} to double"));
                ("%f", "double", ext)
            }
            // 字符串：直接打印内容
            "ptr" => ("%s", "ptr", v),
            "i1" => {
                // bool 直接打印 0/1（v0.1 简化，后续版本转 true/false 文本）
                let ext = self.new_reg();
                self.line(&format!("{ext} = zext i1 {v} to i64"));
                ("%lld", "i64", ext)
            }
            // 窄整数（i8/i16）：提升到 i32 后按符号性选 %d/%u
            "i8" | "i16" => {
                let ext = self.new_reg();
                if is_unsigned {
                    self.line(&format!("{ext} = zext {t} {v} to i32"));
                    ("%u", "i32", ext)
                } else {
                    self.line(&format!("{ext} = sext {t} {v} to i32"));
                    ("%d", "i32", ext)
                }
            }
            // char → %c；i32 按符号性 %d/%u
            "i32" if is_char => ("%c", "i32", v),
            "i32" => {
                if is_unsigned {
                    ("%u", "i32", v)
                } else {
                    ("%d", "i32", v)
                }
            }
            // i64/u64：%lld/%llu
            "i64" => {
                if is_unsigned {
                    ("%llu", "i64", v)
                } else {
                    ("%lld", "i64", v)
                }
            }
            _ => ("%lld", "i64", v),
        };
        let g = self.fmt_global(&format!("{fmt}{nl}"));
        self.line(&format!("call i32 (ptr, ...) @printf(ptr @{g}, {arg_ty} {arg})"));
        Ok((self.new_reg(), "void"))
    }

    /// 生成运行时错误：printf(错误消息) → fflush(stdout) → exit(1)。
    ///
    /// `fmt` 是 printf 格式串（可含 `%s` 等占位符），`args` 是 (LLVM 类型, 值) 列表。
    /// 文本与解释路径（tie-interp 返回的 Err）保持一致；先刷新 stdout 保证消息可见。
    /// 末尾 `unreachable` 终止当前基本块（gen_fn 据此不再补 ret）。
    fn gen_runtime_error(&mut self, fmt: &str, args: &[(&str, String)]) {
        let g = self.fmt_global(&format!("{fmt}\n"));
        let arg_str = args
            .iter()
            .map(|(t, v)| format!("{t} {v}"))
            .collect::<Vec<_>>()
            .join(", ");
        if args.is_empty() {
            self.line(&format!("call i32 (ptr, ...) @printf(ptr @{g})"));
        } else {
            self.line(&format!("call i32 (ptr, ...) @printf(ptr @{g}, {arg_str})"));
        }
        self.line("call i32 @fflush(ptr null)");
        // 非 void 的未编号调用（fflush 返回 i32）会被解析器分配隐式寄存器号，必须消费掉
        let _ = self.new_reg();
        self.line("call void @exit(i32 1)");
        self.line("unreachable");
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

    /// 将数字值提升为 double（数学函数实参统一为 f64）。
    ///
    /// double 直接用；float 用 fpext；整数类型（iN）用 sitofp。
    fn promote_to_double(
        &mut self,
        val: &str,
        ty: &'static str,
    ) -> Result<String, IrError> {
        match ty {
            "double" => Ok(val.to_string()),
            "float" => {
                let ext = self.new_reg();
                self.line(&format!("{ext} = fpext float {val} to double"));
                Ok(ext)
            }
            // 整数类型：符号扩展为 double（数值语义一致）
            _ => {
                let ext = self.new_reg();
                self.line(&format!("{ext} = sitofp {ty} {val} to double"));
                Ok(ext)
            }
        }
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

    /// 记录用到的 tie-interp 库导出符号（去重；link 阶段按需链接静态库）。
    fn mark_used(&mut self, symbol: &str) {
        if !self.used_externs.contains(&symbol.to_string()) {
            self.used_externs.push(symbol.to_string());
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

// ---------- 单元测试 ----------

#[cfg(test)]
mod tests {
    use super::{gen_ir, IrOutput};
    use tie_frontend::lexer::tokenize;
    use tie_frontend::parser::parse_program;
    use tie_frontend::semantic::analyze;

    /// 完整编译管道：源码 → 词法 → 语法 → 语义 → LLVM IR 文本。
    ///
    /// 不经过文件系统，正例测试均应通过；任一阶段失败即 panic。
    fn 编译(src: &str) -> String {
        let toks = tokenize(src).expect("词法分析失败");
        let program = parse_program(&toks).expect("语法分析失败");
        let sem = analyze(&program).expect("语义分析失败");
        gen_ir(&program, &sem).expect("IR 生成失败").ir
    }

    /// 完整编译管道（返回 IrOutput：IR 文本 + used_externs）。
    fn 编译_输出(src: &str) -> IrOutput {
        let toks = tokenize(src).expect("词法分析失败");
        let program = parse_program(&toks).expect("语法分析失败");
        let sem = analyze(&program).expect("语义分析失败");
        gen_ir(&program, &sem).expect("IR 生成失败")
    }

    /// 管道结果（负例用）：任一阶段失败返回 Err（含 IR 层防御性报错）。
    fn 管道(src: &str) -> Result<String, String> {
        let toks = tokenize(src).map_err(|e| format!("词法: {e}"))?;
        let program = parse_program(&toks).map_err(|e| format!("语法: {e}"))?;
        let sem = analyze(&program).map_err(|e| format!("语义: {e}"))?;
        gen_ir(&program, &sem).map(|o| o.ir).map_err(|e| format!("IR: {e}"))
    }

    #[test]
    fn 模块头与运行时函数声明() {
        let ir = 编译("func main() {}");
        assert!(ir.contains("; ModuleID = 'tie'"));
        assert!(ir.contains("source_filename = \"input.tie\""));
        // println 依赖的 printf 声明（变参函数）
        assert!(ir.contains("declare i32 @printf(ptr, ...)"));
        // 字符串运行时依赖：长度 / 比较 / 拼接分配 / 拼接拷贝
        assert!(ir.contains("declare i64 @strlen(ptr)"));
        assert!(ir.contains("declare i32 @strcmp(ptr, ptr)"));
        assert!(ir.contains("declare ptr @malloc(i64)"));
        assert!(ir.contains("declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)"));
        // 未使用 read_line/eval 时，不应输出 interp 库声明
        assert!(!ir.contains("tie_read_line"));
        assert!(!ir.contains("tie_eval_expr"));
    }

    #[test]
    fn repl内置函数生成interp调用与声明() {
        // read_line：调用 tie_read_line，收集 used_externs
        let out = 编译_输出("func main() {\n    var line = read_line()\n    println(line)\n}");
        assert!(out.ir.contains("call ptr @tie_read_line()"));
        assert!(out.ir.contains("declare ptr @tie_read_line()"));
        assert!(out.used_externs.contains(&"tie_read_line".to_string()));
        // eval：调用 tie_eval_expr + tie_free_result（作为独立语句时释放堆串）
        let out2 = 编译_输出("func main() {\n    var r = eval(\"1 + 2\")\n    println(r)\n}");
        assert!(out2.ir.contains("call ptr @tie_eval_expr(ptr "));
        assert!(out2.ir.contains("declare ptr @tie_eval_expr(ptr)"));
        assert!(out2.used_externs.contains(&"tie_eval_expr".to_string()));
        // 独立语句形式的 eval()：结果丢弃，生成 free
        let out3 = 编译_输出("func main() {\n    eval(\"var x = 1\")\n}");
        assert!(out3.ir.contains("call void @tie_free_result(ptr %"));
        assert!(out3.used_externs.contains(&"tie_free_result".to_string()));
        // print：不换行（格式串末尾无 \n）
        let out4 = 编译_输出("func main() {\n    print(42)\n}");
        assert!(out4.ir.contains("@printf(ptr @.str."));
        // 普通程序：used_externs 为空（不链接 interp 库）
        let out5 = 编译_输出("func main() {\n    println(1)\n}");
        assert!(out5.used_externs.is_empty());
    }

    #[test]
    fn 函数定义生成签名与入口块() {
        let ir = 编译("func add(a: i64, b: i64) -> i64 {\n    return a + b\n}\nfunc main() {}");
        // 参数按 类型 %名 格式拼入签名，入口块固定命名 entry
        assert!(ir.contains("define i64 @add(i64 %a, i64 %b) {"));
        assert!(ir.contains("entry:"));
        assert!(ir.contains("ret i64 %"));
    }

    #[test]
    fn 变量声明生成alloca与store() {
        let ir = 编译("func main() {\n    var x: i64 = 42\n    var y: i64 = x + 1\n}");
        // 变量采用 alloca/store/load 模式（依赖 opt 的 mem2reg 提升）
        assert!(ir.contains("= alloca i64"));
        assert!(ir.contains("store i64 42, ptr %"));
    }

    #[test]
    fn 算术表达式生成add与mul指令() {
        let ir = 编译("func main() {\n    var x: i64 = (3 + 4) * 5\n}");
        assert!(ir.contains("= add i64 3, 4"));
        assert!(ir.contains("= mul i64 %"));
    }

    #[test]
    fn 用户函数调用生成call指令() {
        let ir = 编译("func add(a: i64, b: i64) -> i64 {\n    return a + b\n}\nfunc main() {\n    var r: i64 = add(1, 2)\n    println(r)\n}");
        // 字面量实参按函数签名类型写出（i64）
        assert!(ir.contains("call i64 @add(i64 1, i64 2)"));
    }

    #[test]
    fn if语句生成条件跳转与三块结构() {
        let ir = 编译("func main() {\n    var x: i64 = 3\n    if x > 1 {\n        println(1)\n    } else {\n        println(0)\n    }\n}");
        assert!(ir.contains("= icmp sgt i64"));
        assert!(ir.contains("br i1 %"));
        assert!(ir.contains("if.then."));
        assert!(ir.contains("if.else."));
        assert!(ir.contains("if.merge."));
    }

    #[test]
    fn while循环生成三块结构() {
        let ir = 编译("func main() {\n    var i: i64 = 0\n    while i < 3 {\n        i = i + 1\n    }\n}");
        assert!(ir.contains("br label %loop.cond."));
        assert!(ir.contains("loop.cond."));
        assert!(ir.contains("loop.body."));
        assert!(ir.contains("loop.exit."));
        assert!(ir.contains("= icmp slt i64"));
    }

    #[test]
    fn for范围循环生成三块与结束条件() {
        let ir = 编译("func main() {\n    for i in 0..10 {\n        println(i)\n    }\n}");
        assert!(ir.contains("for.cond."));
        assert!(ir.contains("for.body."));
        assert!(ir.contains("for.exit."));
        // 循环结束条件：i >= end（sge）
        assert!(ir.contains("= icmp sge i64"));
        // 循环体末尾自增
        assert!(ir.contains("= add i64 %"));
    }

    #[test]
    fn 表遍历生成计数器循环() {
        let ir = 编译("func main() {\n    var arr: table = [10, 20, 30]\n    for item in arr {\n        println(item)\n    }\n}");
        assert!(ir.contains("for.cond."));
        assert!(ir.contains("for.body."));
        assert!(ir.contains("for.exit."));
        // 每次迭代 GEP 取 arr[i] → load 元素 → store 到循环变量
        assert!(ir.contains("getelementptr [3 x i64], ptr %"));
        assert!(ir.contains("= load i64, ptr %"));
    }

    #[test]
    fn switch生成比较链与各分支块() {
        let ir = 编译("func main() {\n    var n: i64 = 2\n    switch n {\n        case 1:\n            println(1)\n        case 2:\n            println(2)\n        default:\n            println(0)\n    }\n}");
        // switch 展开为比较链：sw.cmp 比较块 → sw.body 体块 → sw.default → sw.exit
        assert!(ir.contains("sw.cmp."));
        assert!(ir.contains("sw.body."));
        assert!(ir.contains("sw.default."));
        assert!(ir.contains("sw.exit."));
        assert!(ir.contains("= icmp eq i64"));
    }

    #[test]
    fn 表变量生成数组布局与逐元素写入() {
        let ir = 编译("func main() {\n    var arr: table = [10, 20, 30]\n}");
        // 定长数组布局：alloca [N x T]
        assert!(ir.contains("= alloca [3 x i64]"));
        // 逐元素 GEP 到数组内偏移后 store
        assert!(ir.contains("getelementptr [3 x i64], ptr %"));
        assert!(ir.contains("store i64 10, ptr %"));
        assert!(ir.contains("store i64 20, ptr %"));
        assert!(ir.contains("store i64 30, ptr %"));
    }

    #[test]
    fn 表下标访问生成gep与load() {
        let ir = 编译("func main() {\n    var arr: table = [10, 20, 30]\n    var x: i64 = arr[1]\n    println(x)\n}");
        // 下标访问：GEP 第 0 行第 i 列 → load 元素
        assert!(ir.contains("getelementptr [3 x i64], ptr %"));
        assert!(ir.contains("= load i64, ptr %"));
    }

    #[test]
    fn println生成printf调用与格式串() {
        let ir = 编译("func main() {\n    println(\"hi\")\n    println(42)\n}");
        // 字符串参数：%s 格式；i64 参数：%lld 格式
        assert!(ir.contains("call i32 (ptr, ...) @printf(ptr @.str."));
        assert!(ir.contains("c\"hi\\00\""));
        assert!(ir.contains("c\"%lld\\0A\\00\""));
    }

    #[test]
    fn 字符串拼接生成malloc与memcpy() {
        let ir = 编译("func main() {\n    var s: string = \"Hello\" + \"World\"\n}");
        // 拼接：strlen 两段 → 总长+1 → malloc → memcpy 两段 → 末尾写 \0
        assert!(ir.contains("= call i64 @strlen(ptr @.str."));
        assert!(ir.contains("= call ptr @malloc(i64 %"));
        assert!(ir.contains("call void @llvm.memcpy.p0.p0.i64(ptr %"));
        assert!(ir.contains("store i8 0, ptr %"));
    }

    #[test]
    fn 字符串比较与len内置函数() {
        let ir = 编译("func main() {\n    var e: bool = \"abc\" == \"abd\"\n    var n: i64 = len(\"tie\")\n}");
        // 比较：strcmp 结果与 0 比较；len：strlen
        assert!(ir.contains("= call i32 @strcmp(ptr @.str."));
        assert!(ir.contains("= icmp eq i32 %"));
        assert!(ir.contains("= call i64 @strlen(ptr @.str."));
    }

    #[test]
    fn 类实例方法生成this参数与字段gep() {
        let ir = 编译("class Point {\n    var x: i64\n    var y: i64\n    method area() -> i64 {\n        return this.x * this.y\n    }\n}\nfunc main() {\n    var p = Point(3, 4)\n    println(p.area())\n}");
        // 实例方法签名：隐藏 this 首参（ptr %this）
        assert!(ir.contains("define i64 @Point$area(ptr %this) {"));
        // 字段访问：按拍平偏移 GEP（x→0，y→1）
        assert!(ir.contains("getelementptr {i64, i64}, ptr %this, i32 0, i32 0"));
        assert!(ir.contains("getelementptr {i64, i64}, ptr %this, i32 0, i32 1"));
        // 构造：insertvalue 链构建结构体值；实例调用：receiver 地址作 this 实参
        assert!(ir.contains("insertvalue {i64, i64}"));
        assert!(ir.contains("= call i64 @Point$area(ptr %"));
    }

    #[test]
    fn 类静态方法不接收this() {
        let ir = 编译("class Point {\n    var x: i64\n    var y: i64\n    static method create(x: i64, y: i64) -> i64 {\n        return x + y\n    }\n}\nfunc main() {\n    println(Point.create(1, 2))\n}");
        // 静态方法签名与普通函数一致：无 this 首参
        assert!(ir.contains("define i64 @Point$create(i64 %x, i64 %y) {"));
        assert!(!ir.contains("Point$create(ptr"));
        // 类名调用 → 静态调用（无 this 实参）
        assert!(ir.contains("call i64 @Point$create(i64 1, i64 2)"));
    }

    #[test]
    fn 类构造与字段赋值() {
        let ir = 编译("class Point {\n    var x: i64\n    var y: i64\n}\nfunc main() {\n    var p = Point(3, 4)\n    p.x = 100\n    println(p.x)\n}");
        // 构造：逐字段 insertvalue（字段顺序与拍平顺序一致）
        assert!(ir.contains("insertvalue {i64, i64} undef, i64 3, 0"));
        assert!(ir.contains("insertvalue {i64, i64} %"));
        // 字段赋值：GEP 到字段偏移后 store
        assert!(ir.contains("getelementptr {i64, i64}, ptr %"));
        assert!(ir.contains("store i64 100, ptr %"));
    }

    #[test]
    fn main函数生成i32签名与返回0() {
        let ir = 编译("func main() -> i32 {\n    return 0\n}");
        assert!(ir.contains("define i32 @main() {"));
        assert!(ir.contains("entry:"));
        assert!(ir.contains("ret i32 0"));
    }

    #[test]
    fn 元组字面量生成insertvalue与extractvalue() {
        let ir = 编译("func main() {\n    var t = (10, 20)\n    println(t.Item1)\n}");
        // 字面量：undef 起始逐字段 insertvalue；访问：load 后 extractvalue
        assert!(ir.contains("insertvalue {i64, i64} undef, i64 10, 0"));
        assert!(ir.contains("insertvalue {i64, i64} %"));
        assert!(ir.contains("= extractvalue {i64, i64} %"));
    }

    #[test]
    fn 负例_范围表达式不能单独求值() {
        // 语义层对 Range 只校验两端整数；IR 层防御性报错
        // （范围只能在 for 中作为迭代对象，不能作为普通表达式求值）
        let result = 管道("func main() {\n    var r = 1..3\n}");
        assert!(result.is_err());
    }

    #[test]
    fn 负例_表字面量不能用于非表变量() {
        // 表字面量只允许出现在 table 类型变量声明中（语义层拦截）
        let result = 管道("func main() {\n    var x: i64 = [1, 2]\n}");
        assert!(result.is_err());
    }

    // ---------- M4 运算符扩展测试 ----------

    #[test]
    fn 位运算生成与移位指令() {
        let ir = 编译("func main() {\n    var x: i64 = 5\n    println(x & 3)\n    println(x | 8)\n    println(x ^ 1)\n    println(8 >> 2)\n    println(1 << 3)\n}");
        // 按位与/或/异或：and/or/xor 作用于整数类型（逻辑 And/Or 是 i1，可区分）
        assert!(ir.contains("= and i64 %"));
        assert!(ir.contains("= or i64 %"));
        assert!(ir.contains("= xor i64 %"));
        // 右移：有符号整数 → 算术右移 ashr；左移 → shl（立即数 1/3 即 i64 字面量）
        assert!(ir.contains("= ashr i64 8, 2"));
        assert!(ir.contains("= shl i64 1, 3"));
    }

    #[test]
    fn 复合赋值生成load运算store() {
        let ir = 编译("func main() {\n    var x: i64 = 1\n    x += 2\n    x *= 4\n    x -= 1\n    x /= 2\n    x %= 3\n}");
        // 复合赋值 = load 目标当前值 → 二元运算 → store 回目标
        assert!(ir.contains("= add i64 %"));
        assert!(ir.contains("= mul i64 %"));
        assert!(ir.contains("= sub i64 %"));
        assert!(ir.contains("= sdiv i64 %"));
        assert!(ir.contains("= srem i64 %"));
        assert!(ir.contains("store i64 %"));
    }

    #[test]
    fn 字符串复合拼接生成malloc与memcpy() {
        let ir = 编译("func main() {\n    var s: string = \"a\"\n    s += \"b\"\n}");
        // s += "b"：load 当前串指针 → 走字符串拼接（strlen/malloc/memcpy）→ store 回变量
        assert!(ir.contains("= call i64 @strlen(ptr %"));
        assert!(ir.contains("= call ptr @malloc(i64 %"));
        assert!(ir.contains("store ptr %"));
    }

    #[test]
    fn 三目运算生成phi汇合() {
        let ir = 编译("func main() {\n    var x: i64 = 5\n    println(x > 0 ? 100 : -1)\n    println(x < 0 ? 1 : 2)\n}");
        // 三块结构：tern.then / tern.else / tern.merge + phi 汇合（类型取 then 分支 i64）
        assert!(ir.contains("tern.then."));
        assert!(ir.contains("tern.else."));
        assert!(ir.contains("tern.merge."));
        assert!(ir.contains("= phi i64"));
    }

    #[test]
    fn 自增自减生成load运算与store() {
        let ir = 编译("func main() {\n    var x: i64 = 1\n    x++\n    ++x\n    x--\n    println(x--)\n    println(++x)\n}");
        // 自增/自减 = load 当前值 → add/sub 1 → store 回变量
        assert!(ir.contains("= add i64 %"));
        assert!(ir.contains("= sub i64 %"));
        assert!(ir.contains("store i64 %"));
    }
}
