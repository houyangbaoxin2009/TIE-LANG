//! 中端：AST → LLVM IR 文本生成。
//!
//! 职责：把语义分析通过的 AST 翻译为 LLVM IR（文本形式 .ll）。
//! 后续的中间优化交给 LLVM `opt` 完成，本模块只负责生成合法的 IR。
//!
//! # 简化约定（v0.1）
//! - 变量使用 alloca/store/load 模式（依赖 opt 的 mem2reg 提升）
//! - println 通过声明 `printf` 实现，按参数类型选择格式串
//! - 函数入口块命名为 `entry`，控制流块命名为 `if.then`/`if.else`/`loop.cond` 等

use crate::frontend::ast::{BinaryOp, Expr, FnDefStmt, Program, Stmt, TypeSpec, UnaryOp};
use crate::frontend::lexer::TyKw;
use crate::frontend::semantic::{FuncSig, SemanticResult};
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

        // 收集函数签名（与语义一致）
        let sigs: HashMap<String, FuncSig> = self
            .program
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::FnDef(f) => Some((
                    f.name.clone(),
                    FuncSig {
                        param_tys: f.params.iter().map(|p| p.ty).collect(),
                        ret_ty: f.ret_ty,
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
        let ret_llvm = self.llvm_ty(f.ret_ty);
        let mut params = Vec::new();
        for p in &f.params {
            params.push(format!("{} {}", self.llvm_ty(p.ty), mangle(&p.name)));
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
            let ty = self.llvm_ty(p.ty);
            let pname = mangle(&p.name);
            let alloca = self.new_reg();
            self.line(&format!("{alloca} = alloca {ty}"));
            self.line(&format!("store {ty} {pname}, ptr {alloca}"));
            scope.insert(p.name.clone(), VarBind { value: alloca, ty });
        }
        self.scopes.push(scope);

        // 函数体
        for stmt in &f.body {
            self.gen_stmt(stmt)?;
        }

        // 结尾：无 return 时补默认返回
        let last = self.out.trim_end();
        let needs_ret = !last.ends_with("ret ") && !last.ends_with("ret void");
        if needs_ret {
            if f.ret_ty.is_void() {
                self.line("ret void");
            } else {
                // 非 void 且缺 return：语义已拦截，这里兜底返回 0
                self.line(&format!("ret {} 0", self.llvm_ty(f.ret_ty)));
            }
        }

        self.dedent();
        self.out.push_str("}\n\n");
        self.scopes.pop();
        let _ = sigs;
        Ok(())
    }

    // ---------- 语句生成 ----------

    fn gen_stmt(&mut self, stmt: &Stmt) -> Result<(), IrError> {
        match stmt {
            Stmt::VarDecl(v) => {
                let (val, ty) = self.gen_expr(&v.init)?;
                // 声明类型以语义为准
                let ty_name = v.ty.map(|t| self.llvm_ty(t)).unwrap_or(ty);
                let alloca = self.new_reg();
                self.line(&format!("{alloca} = alloca {ty_name}"));
                self.line(&format!("store {ty_name} {val}, ptr {alloca}"));
                // 变量类型：int/float/bool 等；string 特殊（ptr）
                self.cur_scope_mut()
                    .insert(v.name.clone(), VarBind { value: alloca, ty: ty_name });
                Ok(())
            }
            Stmt::FnDef(_) => Ok(()), // 顶层函数，不在此生成
            Stmt::Expr(e) => {
                self.gen_expr(&e.expr)?;
                Ok(())
            }
            Stmt::Return(r) => match &r.expr {
                Some(e) => {
                    let (val, _ty) = self.gen_expr(e)?;
                    // 返回类型以函数签名为准：字面量可能被语义适配
                    // （如返回 i32 的函数 `return 42`，字面量推导为 i64）
                    let ret_ty = self
                        .sem
                        .funcs
                        .get(&self.cur_fn)
                        .map(|s| s.ret_ty)
                        .unwrap_or(TypeSpec::Named(TyKw::I64));
                    let ret_llvm = self.llvm_ty(ret_ty);
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
        }
    }

    fn gen_if(&mut self, i: &crate::frontend::ast::IfStmt) -> Result<(), IrError> {
        let (cond, _) = self.gen_expr(&i.cond)?;
        let then_label = self.new_label("if.then");
        let else_label = self.new_label("if.else");
        let merge_label = self.new_label("if.merge");
        self.line(&format!("br i1 {cond}, label %{then_label}, label %{else_label}"));

        // then 分支
        self.block_start(&then_label);
        self.scopes.push(HashMap::new());
        for s in &i.then_branch {
            self.gen_stmt(s)?;
        }
        self.scopes.pop();
        self.line(&format!("br label %{merge_label}"));
        self.block_end();

        // else 分支
        self.block_start(&else_label);
        self.scopes.push(HashMap::new());
        for s in &i.else_branch {
            self.gen_stmt(s)?;
        }
        self.scopes.pop();
        self.line(&format!("br label %{merge_label}"));
        self.block_end();

        self.block_start(&merge_label);
        Ok(())
    }

    fn gen_while(&mut self, w: &crate::frontend::ast::WhileStmt) -> Result<(), IrError> {
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
        self.line(&format!("br label %{cond_label}"));
        self.block_end();

        self.block_start(&exit_label);
        Ok(())
    }

    fn gen_for(&mut self, f: &crate::frontend::ast::ForStmt) -> Result<(), IrError> {
        // 仅支持 `for x in start..end`（范围）
        let Expr::Range { start, end, .. } = &f.iter else {
            return Err(IrError {
                message: format!("for 迭代对象仅支持范围（start..end），当前不支持（函数 {}）", self.cur_fn),
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
            VarBind { value: var_alloca.clone(), ty: "i64" },
        )]));
        for s in &f.body {
            self.gen_stmt(s)?;
        }
        self.scopes.pop();
        // 自增
        let next = self.new_reg();
        self.line(&format!("{next} = add i64 {cur}, 1"));
        self.line(&format!("store i64 {next}, ptr {var_alloca}"));
        self.line(&format!("br label %{cond_label}"));
        self.block_end();

        self.block_start(&exit_label);
        Ok(())
    }

    // ---------- 表达式生成 ----------

    /// 生成表达式，返回 (值名, LLVM 类型名)。
    fn gen_expr(&mut self, expr: &Expr) -> Result<(String, &'static str), IrError> {
        match expr {
            Expr::IntLit(v) => Ok((v.to_string(), "i64")),
            Expr::FloatLit(v) => Ok((format_float(*v), "double")),
            Expr::BoolLit(b) => Ok((if *b { "true".into() } else { "false".into() }, "i1")),
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
            Expr::Call { name, args, .. } => self.gen_call(name, args),
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
        }
    }

    fn gen_binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<(String, &'static str), IrError> {
        let (lv, lt) = self.gen_expr(lhs)?;
        let (rv, _rt) = self.gen_expr(rhs)?;
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

    /// 函数调用生成：内置 println → printf；用户函数 → call。
    fn gen_call(&mut self, name: &str, args: &[Expr]) -> Result<(String, &'static str), IrError> {
        if name == "println" {
            return self.gen_println(args);
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
            let aty = self.llvm_ty(*want_ty);
            arg_list.push(format!("{aty} {v}"));
        }
        let ret_llvm = self.llvm_ty(sig.ret_ty);
        let tmp = self.new_reg();
        if sig.ret_ty.is_void() {
            self.line(&format!("call void @{}({})", name, arg_list.join(", ")));
            Ok((tmp, "void"))
        } else {
            self.line(&format!("{tmp} = call {ret_llvm} @{}({})", name, arg_list.join(", ")));
            Ok((tmp, ret_llvm))
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
        self.sem.expr_types.get(&(e as *const Expr as usize)).copied()
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
    fn llvm_ty(&self, t: TypeSpec) -> &'static str {
        t.llvm_ty()
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

/// 变量名 mangling：防止与 LLVM 保留名冲突（当前原样 + 前缀）。
fn mangle(name: &str) -> String {
    format!("%{}", name)
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
