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
    BinaryOp, Expr, FieldAssignStmt, FnDefStmt, IndexAssignStmt, Program, Stmt, TableCell,
    TypeSpec, UnaryOp,
};
use tie_frontend::lexer::TyKw;
use tie_frontend::semantic::{ClassInfo, FuncSig, SemanticResult};
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
    /// 循环跳转上下文栈（E1+E5）：break/continue 的目标 label。元素 = (标签, continue 目标, exit 目标)。
    /// continue 目标：while 为条件块（跳到下次条件判断）；for 为自增块（跳到步进处）。
    loop_ctx: Vec<(Option<String>, String, String)>,
    /// alloca 提升缓冲（F1）：函数体执行过程中生成的 alloca 指令文本。
    /// LLVM 规范要求 alloca 位于 entry block——非 entry 块的 alloca 每次执行都会
    /// 重新分配栈空间，循环体内密集 alloca（如表读 ok 标志）会逐次累积导致栈溢出
    /// （0xC00000FD）。统一收集后 flush 到函数 entry 块末尾，根治该缺陷。
    entry_allocas: Vec<String>,
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
        loop_ctx: Vec::new(),
        entry_allocas: Vec::new(),
    };
    generator.run()?;
    // F1：alloca 提升后 entry 块编号可能倒挂（%54 在 %1 之前），LLVM 要求递增，
    // 全局重编号（按文本出现顺序重映射 %N → 1..N）
    let ir = IrGenerator::renumber_ir(&generator.out);
    Ok(IrOutput { ir, used_externs: generator.used_externs })
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
        // remove：file_delete（编译模式用 libc remove，解释模式用 std::fs::remove_file，
        // 两者行为一致：成功删除返回 0/true，不存在/不可删返回非 0/false）
        self.out.push_str("declare i32 @remove(ptr)\n");
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

        // 顶层全局持久变量（M4）：`@name = global Ty 字面量`（静态初始化，
        // 函数内 load/store 访问）。字符串全局存指向常量串的指针。
        self.gen_globals()?;

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
                        param_defaults: f.params.iter().map(|p| p.default.clone()).collect(),
                        ret_ty: f.ret_ty.clone(),
                        // 顶层函数恒公有（与语义层一致）
                        is_pub: true,
                    },
                )),
                _ => None,
            })
            .collect();

        // 生成各函数（顶层 + 命名空间内）：全名 = 顶层裸名 / 命名空间路径::函数名。
        // 递归遍历命名空间体，fn_full_names 提供 FnDefStmt 地址 → 全名映射
        // （与语义层一致；gen_fn 用全名生成 LLVM 符号）。
        for stmt in &self.program.stmts {
            if let Stmt::FnDef(f) = stmt {
                self.gen_fn(f, &f.name, &sigs)?;
            } else if let Stmt::Namespace(ns) = stmt {
                self.gen_ns_fns(&ns.body, &ns.path, &sigs)?;
            }
        }

        // M2.1.8：方法已移出 struct——逻辑是绑定 struct 名的命名空间函数，
        // 由上方 gen_ns_fns 统一生成（@Point$dist 等），无需单独方法生成循环。

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
        if self.used_externs.iter().any(|s| s == "tie_eval_call") {
            self.out.push_str("declare ptr @tie_eval_call(ptr, ptr)\n");
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
        if self.used_externs.iter().any(|s| s == "tie_str_len") {
            self.out.push_str("declare i64 @tie_str_len(ptr)\n");
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
        // 平衡三进制解析（M4 补齐）：tie_parse_trit 返回 i8（-1/0/1），带 ok 标志。
        if self.used_externs.iter().any(|s| s == "tie_parse_trit") {
            self.out.push_str("declare i8 @tie_parse_trit(ptr, ptr)\n");
        }
        // M2 标准库 floor 的时间/随机原语（C ABI 桥，与解释路径共用实现）：
        // tie_time_now 返回 Unix 秒；tie_rand_range 带 ok 标志（max<=min 时置 0）。
        if self.used_externs.iter().any(|s| s == "tie_time_now") {
            self.out.push_str("declare i64 @tie_time_now()\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_rand_range") {
            self.out.push_str("declare i64 @tie_rand_range(i64, i64, ptr)\n");
        }
        // 进程/环境 floor 的 C ABI 桥（与解释路径共用 std::env::args）：
        // tie_arg_count 返回用户参数个数；tie_arg_string 返回第 i 个用户参数（堆串）。
        if self.used_externs.iter().any(|s| s == "tie_arg_count") {
            self.out.push_str("declare i64 @tie_arg_count()\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_arg_string") {
            self.out.push_str("declare ptr @tie_arg_string(i64)\n");
        }
        // 文件系统 floor 的 C ABI 桥：tie_list_dir 返回字符串动态表（DynTable 指针），
        // 目录不存在/读取失败返回 NULL（调用方统一输出错误消息，两路径文本一致）。
        if self.used_externs.iter().any(|s| s == "tie_list_dir") {
            self.out.push_str("declare ptr @tie_list_dir(ptr)\n");
        }
        // P1 正则表达式 floor 的 C ABI 桥（与解释路径共用 regex crate 实现）：
        // tie_regex_match 带 ok 标志（模式非法置 0，合法置 1 并返回 0/1）；
        // tie_regex_find / tie_regex_replace / tie_regex_group 返回堆串（模式非法返回 NULL，
        // 调用方统一输出错误消息）；tie_regex_find_all 返回字符串动态表（模式非法返回 NULL）。
        if self.used_externs.iter().any(|s| s == "tie_regex_match") {
            self.out.push_str("declare i8 @tie_regex_match(ptr, ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_regex_find") {
            self.out.push_str("declare ptr @tie_regex_find(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_regex_find_all") {
            self.out.push_str("declare ptr @tie_regex_find_all(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_regex_replace") {
            self.out.push_str("declare ptr @tie_regex_replace(ptr, ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_regex_group") {
            self.out.push_str("declare ptr @tie_regex_group(ptr, ptr, i64)\n");
        }
        // 消息系统 floor（#25）的 C ABI 桥（进程内可变状态由 Rust 层 thread_local 持有）：
        // tie_msg_set_lang 切换当前语言；tie_msg_register 登记 (键,语言) → 文本；
        // tie_msg_t 查询翻译（当前语言 → 回退 zh → 回退键本身），返回堆串。
        if self.used_externs.iter().any(|s| s == "tie_msg_set_lang") {
            self.out.push_str("declare void @tie_msg_set_lang(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_msg_get_lang") {
            self.out.push_str("declare ptr @tie_msg_get_lang()\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_msg_register") {
            self.out.push_str("declare void @tie_msg_register(ptr, ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_msg_t") {
            self.out.push_str("declare ptr @tie_msg_t(ptr)\n");
        }
        // M4 消息系统增强桥：print_err（stderr 通道）与 msg_t_lang（指定语言查询，
        // 返回堆串；tcmsg 用顶层持久变量表达回退链后逐语言遍历）。
        if self.used_externs.iter().any(|s| s == "tie_print_err") {
            self.out.push_str("declare void @tie_print_err(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_msg_t_lang") {
            self.out.push_str("declare ptr @tie_msg_t_lang(ptr, ptr)\n");
        }
        // 动态表（table_new_*/table_push/table_at）的 C ABI 桥（与解释路径共用实现）：
        // tie_table_new 创建空表（elem_size 决定元素宽度）；tie_table_push_* 追加元素；
        // tie_table_at_* 按下标读取（越界置 ok=0）；tie_table_len 返回元素个数；
        // tie_table_free 释放表内存（作用域弹出时对 owned 表调用，防逐次迭代泄漏）。
        if self.used_externs.iter().any(|s| s == "tie_table_new") {
            self.out.push_str("declare ptr @tie_table_new(i64)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_free") {
            self.out.push_str("declare void @tie_table_free(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_len") {
            self.out.push_str("declare i64 @tie_table_len(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_push_i64") {
            self.out.push_str("declare void @tie_table_push_i64(ptr, i64)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_push_f64") {
            self.out.push_str("declare void @tie_table_push_f64(ptr, double)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_push_string") {
            self.out.push_str("declare void @tie_table_push_string(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_push_bool") {
            self.out.push_str("declare void @tie_table_push_bool(ptr, i1)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_at_i64") {
            self.out.push_str("declare i64 @tie_table_at_i64(ptr, i64, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_at_f64") {
            self.out.push_str("declare double @tie_table_at_f64(ptr, i64, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_at_string") {
            self.out.push_str("declare ptr @tie_table_at_string(ptr, i64, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_at_bool") {
            self.out.push_str("declare i1 @tie_table_at_bool(ptr, i64, ptr)\n");
        }
        // M4 补齐：动态表写入桥（下标赋值 t[i] = v）——与读取桥对称，带 ok 标志
        if self.used_externs.iter().any(|s| s == "tie_table_set_i64") {
            self.out.push_str("declare void @tie_table_set_i64(ptr, i64, i64, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_set_f64") {
            self.out.push_str("declare void @tie_table_set_f64(ptr, i64, double, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_set_string") {
            self.out.push_str("declare void @tie_table_set_string(ptr, i64, ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_table_set_bool") {
            self.out.push_str("declare void @tie_table_set_bool(ptr, i64, i1, ptr)\n");
        }
        // ---------- M4 补齐：系统能力原语 C ABI 桥声明 ----------
        // 与 crates/tie-interp/src/lib.rs 的 #[unsafe(no_mangle)] 导出一一对应。
        // 此前 demo 未实际调用这些原语，extern 声明缺失未暴露；D7 补全。
        if self.used_externs.iter().any(|s| s == "tie_http_get") {
            self.out.push_str("declare ptr @tie_http_get(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_http_get_file") {
            self.out.push_str("declare i8 @tie_http_get_file(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_exec_code") {
            self.out.push_str("declare i64 @tie_exec_code(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_exec_output") {
            self.out.push_str("declare ptr @tie_exec_output(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_untar_gz") {
            self.out.push_str("declare i8 @tie_untar_gz(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_unzip") {
            self.out.push_str("declare i8 @tie_unzip(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_mkdir_all") {
            self.out.push_str("declare i8 @tie_mkdir_all(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_remove_dir_all") {
            self.out.push_str("declare i8 @tie_remove_dir_all(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_copy_dir") {
            self.out.push_str("declare i8 @tie_copy_dir(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_walk_dir") {
            self.out.push_str("declare ptr @tie_walk_dir(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_path_join") {
            self.out.push_str("declare ptr @tie_path_join(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_path_basename") {
            self.out.push_str("declare ptr @tie_path_basename(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_path_dirname") {
            self.out.push_str("declare ptr @tie_path_dirname(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_path_abs") {
            self.out.push_str("declare ptr @tie_path_abs(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_path_normalize") {
            self.out.push_str("declare ptr @tie_path_normalize(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_cwd") {
            self.out.push_str("declare ptr @tie_cwd()\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_get_env") {
            self.out.push_str("declare ptr @tie_get_env(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_set_env") {
            self.out.push_str("declare void @tie_set_env(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_file_copy") {
            self.out.push_str("declare i8 @tie_file_copy(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_file_move") {
            self.out.push_str("declare i8 @tie_file_move(ptr, ptr)\n");
        }
        // ---------- D7：字节流 / 位操作原语 C ABI 桥声明 ----------
        if self.used_externs.iter().any(|s| s == "tie_byte_read") {
            self.out.push_str("declare ptr @tie_byte_read(ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_byte_write") {
            self.out.push_str("declare i8 @tie_byte_write(ptr, ptr)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_bit_read") {
            self.out.push_str("declare i64 @tie_bit_read(ptr, i64)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_bit_write") {
            self.out.push_str("declare i8 @tie_bit_write(ptr, i64, i64)\n");
        }
        if self.used_externs.iter().any(|s| s == "tie_byte_concat") {
            self.out.push_str("declare ptr @tie_byte_concat(ptr, ptr)\n");
        }
        Ok(())
    }

    /// 顶层全局持久变量声明（M4）：`@name = global Ty 字面量`（静态初始化）。
    ///
    /// - 数值/布尔/字符：按类型直接写字面量值；字符串：指向常量串的指针
    ///   （`@name = global ptr @.str.N`，常量串由 string_global 延迟收集）；
    /// - 函数内访问：读 `load Ty, ptr @name`、写 `store Ty %v, ptr @name`（见
    ///   gen_expr 的 Var 分支与 gen_stmt 的 Assign 分支的全局回退）。
    fn gen_globals(&mut self) -> Result<(), IrError> {
        for stmt in &self.program.stmts {
            let Stmt::VarDecl(v) = stmt else { continue };
            let Some(ty) = self.sem.globals.get(&v.name) else {
                continue; // 非全局（函数内声明不会到顶层）；语义层已收集全局
            };
            let llvm_ty = self.llvm_ty(ty);
            let init = match &v.init {
                Expr::IntLit(n) => n.to_string(),
                Expr::FloatLit(f) => format_float(*f),
                Expr::BoolLit(b) => if *b { "1".to_string() } else { "0".to_string() },
                Expr::CharLit(c) => (*c as i32).to_string(),
                Expr::StrLit(s) => format!("@{}", self.string_global(s)),
                _ => {
                    return Err(IrError {
                        message: format!(
                            "内部错误：全局变量 '{}' 初始化不是字面量（语义层应已拦截）",
                            v.name
                        ),
                    })
                }
            };
            self.out
                .push_str(&format!("@{} = global {llvm_ty} {init}\n", v.name));
        }
        Ok(())
    }

    // ---------- 函数生成 ----------

    /// 命名空间体内函数生成（顶层发射循环递归入口）：全名 = 当前路径::函数名，
    /// 嵌套命名空间递归拼接路径。与语义层 collect_ns_funcs 的路径规则一致。
    fn gen_ns_fns(&mut self, stmts: &[Stmt], prefix: &[String], sigs: &HashMap<String, FuncSig>) -> Result<(), IrError> {
        for stmt in stmts {
            match stmt {
                Stmt::FnDef(f) => {
                    let mut segs = prefix.to_vec();
                    segs.push(f.name.clone());
                    let full = segs.join("::");
                    self.gen_fn(f, &full, sigs)?;
                }
                Stmt::Namespace(inner) => {
                    let mut segs = prefix.to_vec();
                    segs.extend(inner.path.iter().cloned());
                    self.gen_ns_fns(&inner.body, &segs, sigs)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn gen_fn(&mut self, f: &FnDefStmt, full_name: &str, sigs: &HashMap<String, FuncSig>) -> Result<(), IrError> {
        // LLVM 符号名：顶层函数 = 裸名；命名空间函数 = 全名转 $（tcmsg::error::no_file
        // → tcmsg$error$no_file，与类方法 mangle 同约定）。
        self.cur_fn = full_name.to_string();
        self.reg = 0;
        self.scopes.clear();

        // 签名行。main 特殊处理：即使 tie 声明为 void，也生成 `define i32 @main`
        // + `ret i32 0`——MSVC CRT 把 main 返回的 EAX 当进程退出码，`ret void`
        // 会残留 printf 等最后调用的返回值（如打印 3 个字符 → 退出码 3）。
        let is_main_entry = full_name == "main";
        let ret_llvm = if is_main_entry && f.ret_ty.is_void() { "i32" } else { self.llvm_ty(&f.ret_ty) };
        // M2.1.8：方法函数（namespace <struct名> 内的函数，首参类型 == 该 struct 名）
        // 首参按**引用**传递（LLVM ptr）——函数内字段修改反映到调用方
        // （与 class 时代的 this 指针机制一致，只是显式首参）。
        let method_receiver = self.is_method_fn(full_name, f.params.first().map(|p| &p.ty));
        let mut params = Vec::new();
        for (i, p) in f.params.iter().enumerate() {
            if method_receiver && i == 0 {
                params.push(format!("ptr {}", mangle(&p.name)));
            } else {
                params.push(format!("{} {}", self.llvm_ty(&p.ty), mangle(&p.name)));
            }
        }
        self.out.push_str(&format!(
            "define {} @{}({}) {{\n",
            ret_llvm,
            ns_symbol(full_name),
            params.join(", ")
        ));
        // 入口块
        self.out.push_str("entry:\n");
        self.indent();

        // 参数入作用域：方法函数首参按引用绑定（by_ptr，直接使用参数指针，
        // 字段 GEP 用该地址）；其余参数 alloca + store。
        // 表参数（table / table<T>，A1）：LLVM 类型恒为 ptr（动态表指针），
        // 按 by_ptr 绑定（表操作经 tie_table_* 桥访问），不 alloca。
        let mut scope = HashMap::new();
        for (i, p) in f.params.iter().enumerate() {
            let is_table_param = p.ty.is_table();
            let ty = if is_table_param { "ptr" } else { self.llvm_ty(&p.ty) };
            let pname = mangle(&p.name);
            if method_receiver && i == 0 {
                // 首参按引用绑定：参数寄存器即对象指针（mangle 已含 % 前缀）
                scope.insert(
                    p.name.clone(),
                    VarBind { value: pname, ty, by_ptr: true },
                );
                continue;
            }
            if is_table_param {
                // 表参数：与动态表变量同布局——alloca ptr 存表指针，
                // 表访问路径统一 `load ptr, ptr`（tie_table_at/set/len 桥）。
                // 若直接绑定参数寄存器，表访问会把它当地址再 load 一层（段错误）。
                // alloca 就地输出（参数区在 entry 顶部，不能用提升缓冲——会晚于 store）。
                let alloca = self.new_reg();
                self.line(&format!("{alloca} = alloca ptr"));
                self.line(&format!("store ptr {pname}, ptr {alloca}"));
                scope.insert(p.name.clone(), VarBind { value: alloca, ty: "ptr", by_ptr: false });
                continue;
            }
            let alloca = self.new_reg();
            self.line(&format!("{alloca} = alloca {ty}"));
            self.line(&format!("store {ty} {pname}, ptr {alloca}"));
            scope.insert(p.name.clone(), VarBind { value: alloca, ty, by_ptr: false });
        }
        self.scopes.push(scope);

        // F1（alloca 提升）：函数体生成进临时缓冲，结束后把函数体内收集的全部
        // alloca 拼接到入口块（参数区之后、函数体之前）。LLVM 规范要求 alloca 位于
        // entry block——循环体内密集 alloca（表读 ok 标志）若留在循环体每次迭代分配
        // 栈空间会逐次累积导致栈溢出（0xC00000FD）。
        let saved_out = std::mem::take(&mut self.out);
        for stmt in &f.body {
            self.gen_stmt(stmt)?;
        }
        // 把提升的 alloca 指令拼到入口块尾部：参数区（saved_out）之后、函数体之前
        let body_out = std::mem::take(&mut self.out);
        let mut allocas = String::new();
        for a in &self.entry_allocas {
            allocas.push_str(a);
            allocas.push('\n');
        }
        self.entry_allocas.clear();
        self.out = saved_out;
        self.out.push_str(&allocas);
        self.out.push_str(&body_out);

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
            if is_main_entry && f.ret_ty.is_void() {
                // void main：ret i32 0（CRT 退出码正确性）
                self.line("ret i32 0");
            } else if f.ret_ty.is_void() {
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

    // ---------- 方法生成（M2.1.8：已并入命名空间函数） ----------
    // 方法 = 绑定 struct 名的命名空间函数（namespace Point { pub func dist(p: Point) }），
    // 由 gen_ns_fns 生成（符号 ns_symbol("Point::dist") = @Point$dist）；p.dist() 调用
    // 由语义层解析为全名，IR 层 MethodCall 生成时把 receiver 作为首实参传入。

    // ---------- 语句生成 ----------

    fn gen_stmt(&mut self, stmt: &Stmt) -> Result<(), IrError> {
        match stmt {
            Stmt::VarDecl(v) => {
                // 动态表变量（table_new_* 或返回表的函数初始化）：运行时 {ptr,len,cap} 结构。
                // 语义层 table_vars 已登记 dynamic=true；IR 生成 table_new 调用并绑定为 ptr。
                if self
                    .sem
                    .table_vars
                    .get(&(self.cur_fn.clone(), v.name.clone()))
                    .map(|info| info.dynamic)
                    .unwrap_or(false)
                {
                    return self.gen_dyn_table_var(v);
                }
                // 表变量：直接生成定长数组布局（alloca [N x T] + 逐元素 store），
                // 长度与元素类型来自语义层 tables 元数据（键 = init 表达式地址）。
                // 未标注表字面量（var arr = [1,2,3]，M4 补齐支持下标访问/赋值）也走此路径。
                // 标注了非 table 类型（var x: i64 = [1,2]）不在此——语义层已拦截报错。
                if v.ty.as_ref().map(|t| t.is_table()).unwrap_or(false)
                    || (v.ty.is_none() && matches!(v.init, Expr::TableLit { .. }))
                {
                    return self.gen_table_var(v);
                }
                // 平衡三进制字面量适配（M4 补齐）：`var t: trit = true/false` 时
                // 直接生成 i8 值（true→1 / false→-1）——语义层无法可靠改写
                // expr_types（跨路径 AST 地址一致性脆弱），在 IR 声明处按标注处理。
                if matches!(v.ty, Some(TypeSpec::Named(TyKw::Trit))) {
                    if let Expr::BoolLit(b) = &v.init {
                        let val: i8 = if *b { 1 } else { -1 };
                        let alloca = self.emit_alloca("i8");
                        self.line(&format!("store i8 {val}, ptr {alloca}"));
                        self.cur_scope_mut().insert(
                            v.name.clone(),
                            VarBind { value: alloca, ty: "i8", by_ptr: false },
                        );
                        return Ok(());
                    }
                    if let Expr::TritLit(t) = &v.init {
                        let alloca = self.emit_alloca("i8");
                        self.line(&format!("store i8 {t}, ptr {alloca}"));
                        self.cur_scope_mut().insert(
                            v.name.clone(),
                            VarBind { value: alloca, ty: "i8", by_ptr: false },
                        );
                        return Ok(());
                    }
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
                let alloca = self.emit_alloca(ty_name);
                self.line(&format!("store {ty_name} {val}, ptr {alloca}"));
                // 变量类型：int/float/bool 等；string 特殊（ptr）
                self.cur_scope_mut()
                    .insert(v.name.clone(), VarBind { value: alloca, ty: ty_name, by_ptr: false });
                Ok(())
            }
            Stmt::FnDef(_) => Ok(()), // 顶层函数，不在此生成
            Stmt::Namespace(_) => Ok(()), // 命名空间体内函数由顶层发射循环生成
            Stmt::Using(_) => Ok(()), // using 引入语句：仅顶层语义作用（可见性/裸调用解析），不生成 IR
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
                            || name == "eval_call"
                            || name == "file_read"
                            || name == "str_char"
                            || name == "to_string"
                            || name == "arg_string"
                            || name == "regex_find"
                            || name == "regex_replace"
                            || name == "regex_group"
                ) {
                    self.mark_used("tie_free_result");
                    self.line(&format!("call void @tie_free_result(ptr {v})"));
                }
                Ok(())
            }
            Stmt::Assign(a) => {
                // 赋值：查找目标变量绑定（函数作用域）或顶层全局持久变量（M4）
                let bind = self.lookup_var(&a.target).cloned();
                let global_ty = if bind.is_none() {
                    self.sem.globals.get(&a.target).cloned()
                } else {
                    None
                };
                let (target_ptr, target_ty) = if let Some(b) = &bind {
                    (b.value.clone(), b.ty.clone())
                } else if let Some(gt) = &global_ty {
                    (format!("@{}", a.target), self.llvm_ty(gt))
                } else {
                    return Err(IrError {
                        message: format!(
                            "内部错误：赋值目标 '{}' 未入作用域（函数 {}）",
                            a.target, self.cur_fn
                        ),
                    });
                };
                match a.op {
                    // 普通赋值：直接求右值并 store（按目标类型，语义已保证类型匹配）
                    None => {
                        let (val, _ty) = self.gen_expr(&a.value)?;
                        self.line(&format!("store {target_ty} {val}, ptr {target_ptr}"));
                    }
                    // 复合赋值（+= -= *= /= %= &= |= ^= <<= >>=，M4）：
                    // load 目标当前值 → 与右值做二元运算 → store 结果回目标。
                    Some(op) => {
                        let (rv, _rty) = self.gen_expr(&a.value)?;
                        let cur = self.new_reg();
                        self.line(&format!("{cur} = load {target_ty}, ptr {target_ptr}"));
                        // 目标是否字符串：LLVM 类型名 "ptr" 无法区分字符串与裸指针。
                        let lhs_is_str = target_ty == "ptr";
                        let rhs_is_unsigned = matches!(
                            self.sem_ty_of(&a.value),
                            Some(TypeSpec::Named(TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64))
                        );
                        let (res, _t) = self.gen_binary_on_regs(
                            op,
                            lhs_is_str,
                            cur,
                            &target_ty,
                            rv,
                            rhs_is_unsigned,
                        )?;
                        self.line(&format!("store {target_ty} {res}, ptr {target_ptr}"));
                    }
                }
                Ok(())
            }
            Stmt::Return(r) => match &r.expr {
                Some(e) => {
                    // 平衡三进制字面量适配（M4 补齐）：函数返回 trit 且 return 表达式
                    // 是 bool 字面量时（`return true/false`），直接按 trit 写出 i8 值。
                    let ret_ty = self.current_ret_ty();
                    if matches!(ret_ty, TypeSpec::Named(TyKw::Trit)) {
                        if let Expr::BoolLit(b) = e {
                            let v: i8 = if *b { 1 } else { -1 };
                            self.line(&format!("ret i8 {v}"));
                            return Ok(());
                        }
                        if let Expr::TritLit(t) = e {
                            self.line(&format!("ret i8 {t}"));
                            return Ok(());
                        }
                    }
                    let (val, _ty) = self.gen_expr(e)?;
                    // 返回类型以当前函数/方法签名为准：字面量可能被语义适配
                    // （如返回 i32 的函数 `return 42`，字面量推导为 i64）。
                    // 方法名形如 `类$方法`，从 classes 表查签名（不在 funcs 表）。
                    let ret_llvm = self.llvm_ty(&ret_ty);
                    // 非字面量场景语义已保证类型一致；字面量直接按签名类型写出常量
                    self.line(&format!("ret {ret_llvm} {val}"));
                    Ok(())
                }
                None => {
                    // void main 的裸 return：ret i32 0（CRT 退出码正确性）
                    if self.cur_fn == "main" && self.current_ret_ty().is_void() {
                        self.line("ret i32 0");
                    } else {
                        self.line("ret void");
                    }
                    Ok(())
                }
            },
            Stmt::If(i) => self.gen_if(i),
            Stmt::While(w) => self.gen_while(w),
            Stmt::For(f) => self.gen_for(f),
            // break/continue（E1+E5）：按循环跳转上下文生成分支跳转。
            // 无标签 → 最近循环；带标签 → 沿栈查找匹配（语义层已校验存在）。
            Stmt::Break(b) => {
                let target = self.find_loop_exit(b.label.as_deref());
                self.line(&format!("br label %{target}"));
                self.mark_block_terminated();
                Ok(())
            }
            Stmt::Continue(c) => {
                let target = self.find_loop_continue(c.label.as_deref());
                self.line(&format!("br label %{target}"));
                self.mark_block_terminated();
                Ok(())
            }
            Stmt::Switch(s) => self.gen_switch(s),
            Stmt::Struct(_) => {
                // struct 定义只在顶层做字段类型（collect_structs），IR 阶段不应出现
                Ok(())
            }
            Stmt::FieldAssign(fa) => self.gen_field_assign(fa),
            // 表下标赋值（M4 补齐）：`t[i] = v` / `t[i] += v`（定长 GEP store / 动态表 set 桥）
            Stmt::IndexAssign(ia) => self.gen_index_assign(ia),
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
        let TypeSpec::Struct(class_name) = &base_ty else {
            return Err(IrError {
                message: format!(
                    "内部错误：字段赋值的对象不是 struct（{}，函数 {}）",
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
                message: format!("内部错误：struct '{class_name}' 无信息（函数 {}）", self.cur_fn),
            })?;
        let idx = info.field_index.get(&fa.field).copied().ok_or_else(|| IrError {
            message: format!(
                "内部错误：struct '{class_name}' 无字段 '{}'（函数 {}）",
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

    /// 表下标赋值（M4 补齐）：`t[i] = v` / `t[i] += v`。
    ///
    /// 目标 t[i] 的定位与读取（gen_index）对称：
    /// - 动态表（table_new_*）：t 是 alloca ptr → load 表指针 → tie_table_set_* 桥写入；
    /// - 定长表（字面量 [N x T]）：GEP 定位 + store；
    /// - 复合赋值（+= 等）：先读旧值 → 运算 → 写回。
    /// 越界由 set 桥 ok 标志拦截 → 运行时错误（文本与读取越界一致）。
    fn gen_index_assign(&mut self, ia: &IndexAssignStmt) -> Result<(), IrError> {
        let Expr::Index { base, index, .. } = ia.target.as_ref() else {
            return Err(IrError {
                message: "内部错误：下标赋值的目标不是 Index（函数 {}）".into(),
            });
        };
        // 下标值：i64
        let (idx_val, idx_ty) = self.gen_expr(index)?;
        let idx_val = self.extend_int_to_i64(&idx_val, idx_ty, index)?;
        // base 必须是表变量（单层；二维 t[i][j] 赋值留待 set 桥递归）
        let Expr::Var(name) = base.as_ref() else {
            return Err(IrError {
                message: "下标赋值暂只支持单层表变量（t[i]）".into(),
            });
        };
        let bind = self.lookup_var(name).cloned().ok_or_else(|| IrError {
            message: format!("内部错误：下标赋值的变量 '{name}' 未入作用域（函数 {}）", self.cur_fn),
        })?;
        let base_ptr = bind.value;
        let base_ty = bind.ty;
        // 动态表：tie_table_set_* 桥写入
        let is_dynamic = self
            .sem
            .table_vars
            .get(&(self.cur_fn.clone(), name.clone()))
            .map(|info| info.dynamic)
            .unwrap_or(false);
        if is_dynamic {
            // 元素类型
            let elem_ty = self.dyn_table_elem_ty(base)?;
            let elem_llvm = self.llvm_ty(&elem_ty);
            let suffix = table_elem_suffix(elem_llvm);
            self.mark_used(&format!("tie_table_set_{suffix}"));
            // t 是 alloca ptr → load 表指针
            let tptr = self.new_reg();
            self.line(&format!("{tptr} = load ptr, ptr {base_ptr}"));
            // 求右值（普通或复合）
            let new_val = match ia.op {
                None => {
                    let (v, _t) = self.gen_expr(&ia.value)?;
                    v
                }
                Some(op) => {
                    // 读旧值（tie_table_at_* 带 ok 标志）
                    self.mark_used(&format!("tie_table_at_{suffix}"));
                    let ok = self.emit_alloca("i1");
                    self.line(&format!("store i1 1, ptr {ok}"));
                    let old = self.new_reg();
                    self.line(&format!(
                        "{old} = call {elem_llvm} @tie_table_at_{suffix}(ptr {tptr}, i64 {idx_val}, ptr {ok})"
                    ));
                    let (rv, _rty) = self.gen_expr(&ia.value)?;
                    let lhs_is_str = elem_llvm == "ptr";
                    let rhs_unsigned = matches!(
                        self.sem_ty_of(&ia.value),
                        Some(TypeSpec::Named(TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64))
                    );
                    let (res, _t) = self.gen_binary_on_regs(
                        op,
                        lhs_is_str,
                        old,
                        elem_llvm,
                        rv,
                        rhs_unsigned,
                    )?;
                    res
                }
            };
            // 写入（set 桥带 ok 标志，越界 → 运行时错误）
            let ok = self.emit_alloca("i1");
            self.line(&format!("store i1 0, ptr {ok}"));
            self.line(&format!(
                "call void @tie_table_set_{suffix}(ptr {tptr}, i64 {idx_val}, {elem_llvm} {new_val}, ptr {ok})"
            ));
            let okv = self.new_reg();
            self.line(&format!("{okv} = load i1, ptr {ok}"));
            let ok_label = self.new_label("index_assign.ok");
            let err_label = self.new_label("index_assign.err");
            self.line(&format!("br i1 {okv}, label %{ok_label}, label %{err_label}"));
            self.block_start(&err_label);
            let tlen = self.table_len_reg(&tptr)?;
            self.gen_runtime_error(
                "运行时错误: table_at 下标越界：索引 %lld 超出长度 %lld",
                &[("i64", idx_val.clone()), ("i64", tlen)],
            );
            self.block_end();
            self.block_start(&ok_label);
            return Ok(());
        }
        // 定长表：数组类型必须可解析 → GEP + store（普通/复合）
        let Some(elem_ty) = parse_array_elem_ty(base_ty) else {
            return Err(IrError {
                message: format!("内部错误：下标赋值的对象不是数组类型（{}）", base_ty),
            });
        };
        let ptr = self.new_reg();
        self.line(&format!("{ptr} = getelementptr {base_ty}, ptr {base_ptr}, i64 0, i64 {idx_val}"));
        match ia.op {
            None => {
                let (val, _t) = self.gen_expr(&ia.value)?;
                self.line(&format!("store {elem_ty} {val}, ptr {ptr}"));
            }
            Some(op) => {
                let (rv, _rty) = self.gen_expr(&ia.value)?;
                let cur = self.new_reg();
                self.line(&format!("{cur} = load {elem_ty}, ptr {ptr}"));
                let lhs_is_str = elem_ty == "ptr";
                let rhs_unsigned = matches!(
                    self.sem_ty_of(&ia.value),
                    Some(TypeSpec::Named(TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64))
                );
                let (res, _t) = self.gen_binary_on_regs(op, lhs_is_str, cur, elem_ty, rv, rhs_unsigned)?;
                self.line(&format!("store {elem_ty} {res}, ptr {ptr}"));
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
        // 布局元数据：标注 table 时在 sem.tables（键 = init 地址）；
        // 未标注表字面量（var arr = [1,2,3]，M4 补齐）在 table_vars（键 = 函数+变量名）。
        let info = self
            .sem
            .tables
            .get(&key)
            .cloned()
            .or_else(|| {
                self.sem
                    .table_vars
                    .get(&(self.cur_fn.clone(), v.name.clone()))
                    .cloned()
            })
            .ok_or_else(|| IrError {
                message: format!("内部错误：表变量 '{}' 缺少布局元数据", v.name),
            })?;
        let elem_llvm = self.llvm_ty(&info.elem_ty);
        let arr_ty = format!("[{} x {}]", info.len, elem_llvm);
        let alloca = self.emit_alloca(&arr_ty);
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

    /// 动态表变量声明：生成 table_new_* 调用并绑定为不透明指针。
    ///
    /// 布局：`alloca ptr` 存表指针（运行时 {ptr,len,cap} 结构），随后调用
    /// `tie_table_new(elem_size)` 创建空表并 store。元素宽度由语义层 table_vars
    /// 的元素类型决定（i64/f64/string=8，bool=1）。
    fn gen_dyn_table_var(&mut self, v: &tie_frontend::ast::VarDeclStmt) -> Result<(), IrError> {
        let info = self
            .sem
            .table_vars
            .get(&(self.cur_fn.clone(), v.name.clone()))
            .cloned()
            .ok_or_else(|| IrError {
                message: format!("内部错误：动态表变量 '{}' 缺少布局元数据", v.name),
            })?;
        let elem_llvm = self.llvm_ty(&info.elem_ty);
        let elem_size = match elem_llvm {
            "i1" => 1,
            _ => 8,
        };
        // 初始化表达式决定表指针来源：
        // - 返回表的函数调用（裸调用 build_numbers(10) 或命名空间调用
        //   str.split(...)）→ 调用该函数取表指针；
        // - 其余（table_new_* 等）→ 直接 tie_table_new 新建空表。
        let tptr = match &v.init {
            // 裸调用：先查语义层解析记录（using/命名空间裸调 → 全名，如 split → str::split），
            // 无记录则按函数名生成调用
            Expr::Call { name: fname, args, .. } => {
                let key = &v.init as *const Expr as usize;
                let full = self
                    .sem
                    .resolved_calls
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| fname.clone());
                let (r, _t) = self.gen_call(&full, args)?;
                r
            }
            // 命名空间调用（MethodCall）：复用 gen_expr 的调用分发
            // （Path/Var/FieldAccess → resolved_calls 全名 → gen_call）
            Expr::MethodCall { .. } => {
                let (r, _t) = self.gen_expr(&v.init)?;
                r
            }
            _ => {
                self.mark_used("tie_table_new");
                let t = self.new_reg();
                self.line(&format!("{t} = call ptr @tie_table_new(i64 {elem_size})"));
                t
            }
        };
        let alloca = self.emit_alloca("ptr");
        self.line(&format!("store ptr {tptr}, ptr {alloca}"));
        // 绑定：变量类型 = ptr（动态表不透明指针，与字符串一致）
        self.cur_scope_mut()
            .insert(v.name.clone(), VarBind { value: alloca, ty: "ptr", by_ptr: false });
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
        // 循环跳转上下文入栈（E1+E5）：continue → cond（下次条件判断），break → exit
        self.loop_ctx.push((w.label.clone(), cond_label.clone(), exit_label.clone()));
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
        // 循环体若已以 return/break/continue 终止，则无需跳回条件块（否则 ret/br 后产生死代码）
        if !self.block_terminated() {
            self.line(&format!("br label %{cond_label}"));
        }
        self.block_end();

        self.loop_ctx.pop();
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
        // 动态表遍历：`for item in t`（t 为 table_new_* 创建的动态表）。
        // 语义层 table_vars 标记 dynamic=true；循环 0..len(t)，每次 tie_table_at 读取。
        if let Expr::Var(name) = &f.iter
            && let Some(bind) = self.lookup_var(name).cloned()
            && bind.ty == "ptr"
            && self
                .sem
                .table_vars
                .get(&(self.cur_fn.clone(), name.clone()))
                .map(|info| info.dynamic)
                .unwrap_or(false)
        {
            return self.gen_for_dyn_table(f, &bind);
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
        let var_alloca = self.emit_alloca("i64");
        self.line(&format!("store i64 {start_val}, ptr {var_alloca}"));

        let cond_label = self.new_label("for.cond");
        let body_label = self.new_label("for.body");
        let step_label = self.new_label("for.step");
        let exit_label = self.new_label("for.exit");
        // 循环跳转上下文入栈（E1+E5）：continue → step（自增后），break → exit
        self.loop_ctx.push((f.label.clone(), step_label.clone(), exit_label.clone()));
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
        // 循环体若已以 return/break/continue 终止，则无需跳自增块（否则产生死代码指令）
        if !self.block_terminated() {
            self.line(&format!("br label %{step_label}"));
        }
        self.block_end();

        // 自增步进块：continue 的目标
        self.block_start(&step_label);
        let next = self.new_reg();
        self.line(&format!("{next} = add i64 {cur}, 1"));
        self.line(&format!("store i64 {next}, ptr {var_alloca}"));
        self.line(&format!("br label %{cond_label}"));
        self.block_end();

        self.loop_ctx.pop();
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
        let idx_alloca = self.emit_alloca("i64");
        self.line(&format!("store i64 0, ptr {idx_alloca}"));
        // 循环变量 alloca（元素类型 T，每次迭代覆盖）
        let item_alloca = self.emit_alloca(elem_ty);

        let cond_label = self.new_label("for.cond");
        let body_label = self.new_label("for.body");
        let step_label = self.new_label("for.step");
        let exit_label = self.new_label("for.exit");
        // 循环跳转上下文入栈（E1+E5）：continue → step（自增后），break → exit
        self.loop_ctx.push((f.label.clone(), step_label.clone(), exit_label.clone()));
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
        // 循环体若已以 return/break/continue 终止，则无需跳自增块（否则产生死代码指令）
        if !self.block_terminated() {
            self.line(&format!("br label %{step_label}"));
        }
        self.block_end();

        // 自增步进块：continue 的目标
        self.block_start(&step_label);
        let next = self.new_reg();
        self.line(&format!("{next} = add i64 {cur}, 1"));
        self.line(&format!("store i64 {next}, ptr {idx_alloca}"));
        self.line(&format!("br label %{cond_label}"));
        self.block_end();

        self.loop_ctx.pop();
        self.block_start(&exit_label);
        Ok(())
    }

    /// 动态表遍历：`for item in t`，生成 0..len(t) 计数器循环。
    ///
    /// 布局：计数器 alloca（i64，0..len(t)）+ 循环变量 alloca（元素类型 T）。
    /// 每次迭代：调用 tie_table_at 读取元素 → store 到循环变量。
    /// 与定长表 gen_for_table 共用计数器/循环变量骨架，仅元素读取走动态表桥。
    fn gen_for_dyn_table(
        &mut self,
        f: &tie_frontend::ast::ForStmt,
        tbl_bind: &VarBind,
    ) -> Result<(), IrError> {
        // 元素类型：来自语义层 table_vars（动态表的 LLVM 类型恒为 "ptr"）
        let elem_ty = self.dyn_table_elem_ty(&f.iter)?;
        let elem_llvm = self.llvm_ty(&elem_ty);
        let suffix = table_elem_suffix(elem_llvm);
        self.mark_used(&format!("tie_table_at_{suffix}"));
        // 表指针：变量是 alloca ptr，先 load 出表指针（在 entry 块，越界在此循环内已由 at 桥处理）
        let tptr = self.new_reg();
        self.line(&format!("{tptr} = load ptr, ptr {}", tbl_bind.value));
        // 计数器 alloca（i64）
        let idx_alloca = self.emit_alloca("i64");
        self.line(&format!("store i64 0, ptr {idx_alloca}"));
        // 循环变量 alloca（元素类型 T，每次迭代覆盖）
        let item_alloca = self.emit_alloca(elem_llvm);

        let cond_label = self.new_label("for.cond");
        let body_label = self.new_label("for.body");
        let step_label = self.new_label("for.step");
        let exit_label = self.new_label("for.exit");
        // 循环跳转上下文入栈（E1+E5）：continue → step（自增后），break → exit
        self.loop_ctx.push((f.label.clone(), step_label.clone(), exit_label.clone()));
        self.line(&format!("br label %{cond_label}"));

        self.block_start(&cond_label);
        let cur = self.new_reg();
        self.line(&format!("{cur} = load i64, ptr {idx_alloca}"));
        // 长度运行时求值：每次进入条件块调用 tie_table_len（寄存器须按分配顺序递增）
        let tlen = self.table_len_reg(&tptr)?;
        let done = self.new_reg();
        self.line(&format!("{done} = icmp sge i64 {cur}, {tlen}"));
        self.line(&format!("br i1 {done}, label %{exit_label}, label %{body_label}"));
        self.block_end();

        self.block_start(&body_label);
        // item = table_at(t, cur)（越界理论上不会发生：cur < len(t) 且表只增不减）
        let ok = self.emit_alloca("i1");
        self.line(&format!("store i1 1, ptr {ok}"));
        let val = self.new_reg();
        self.line(&format!(
            "{val} = call {elem_llvm} @tie_table_at_{suffix}(ptr {tptr}, i64 {cur}, ptr {ok})"
        ));
        self.line(&format!("store {elem_llvm} {val}, ptr {item_alloca}"));
        // 循环变量可见
        self.scopes.push(HashMap::from([(
            f.var.clone(),
            VarBind { value: item_alloca.clone(), ty: elem_llvm, by_ptr: false },
        )]));
        for s in &f.body {
            self.gen_stmt(s)?;
        }
        self.scopes.pop();
        // 循环体若已以 return/break/continue 终止，则无需跳自增块（否则产生死代码指令）
        if !self.block_terminated() {
            self.line(&format!("br label %{step_label}"));
        }
        self.block_end();

        // 自增步进块：continue 的目标
        self.block_start(&step_label);
        let next = self.new_reg();
        self.line(&format!("{next} = add i64 {cur}, 1"));
        self.line(&format!("store i64 {next}, ptr {idx_alloca}"));
        self.line(&format!("br label %{cond_label}"));
        self.block_end();

        self.loop_ctx.pop();
        self.block_start(&exit_label);
        Ok(())
    }
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

        // C5：整数 subject + 全单值整数常量 case（无区间/守卫/字符串）→ 生成
        // LLVM switch 指令（跳转表，O(1) 分派）。否则走下方逐 case 比较链。
        if self.can_emit_switch_table(&s.cases, &is_str_subj, is_char) {
            let default_label = def_label.clone().unwrap_or_else(|| exit_label.clone());
            // 先收集 case 值 → body label 映射（body 块 label 前向引用，后生成）
            let mut table_entries: Vec<String> = Vec::new();
            let mut body_labels: Vec<String> = Vec::new();
            for case in &s.cases {
                let Expr::IntLit(v) = case.patterns[0] else {
                    unreachable!("C5 前置检查已保证整数常量 case");
                };
                let body_label = self.new_label("sw.body");
                table_entries.push(format!("i64 {v}, label %{body_label}"));
                body_labels.push(body_label);
            }
            // switch 指令（当前块内；default 目标：有 default 体 → def_label，否则 exit）
            self.line(&format!(
                "switch i64 {subj}, label %{default_label} [\n    {}]",
                table_entries.join("\n    ")
            ));
            // case 体块（逐个生成，内部自动跳 exit）
            for (case, body_label) in s.cases.iter().zip(body_labels.iter()) {
                self.gen_switch_body(&case.body, body_label, &exit_label)?;
            }
            // default 体块（可选）
            if let Some(def) = &def_label {
                self.gen_switch_body(&s.default_body, def, &exit_label)?;
            }
            self.block_start(&exit_label);
            return Ok(());
        }

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

            // 比较块：subject 与 case 的每个 pattern 比较（模式匹配增强）：
            // - 多值 → 每个值一个比较，OR 合并（任一命中）；
            // - 区间 → start ≤ subj < end 两个比较 AND 合并；
            // - 守卫 → 值比较结果 AND when 条件。
            self.block_start(&cur_cmp);
            let mut conds: Vec<String> = Vec::new();
            for pat in &case.patterns {
                match pat {
                    // 区间 pattern：`case 3..7:` —— start ≤ subj < end（左闭右开）
                    Expr::Range { start, end, .. } => {
                        let (sv, _) = self.gen_expr(start)?;
                        let (ev, _) = self.gen_expr(end)?;
                        // 区间比较类型：字符用 i32（subject 未扩展），整数用 i64（subject 已扩展）
                        let cmp_ty = if is_char { "i32" } else { "i64" };
                        let ge = self.new_reg();
                        self.line(&format!("{ge} = icmp sge {cmp_ty} {subj}, {sv}"));
                        let lt = self.new_reg();
                        self.line(&format!("{lt} = icmp slt {cmp_ty} {subj}, {ev}"));
                        let and = self.new_reg();
                        self.line(&format!("{and} = and i1 {ge}, {lt}"));
                        conds.push(and);
                    }
                    // 类型匹配 pattern：语义层已拦截（静态类型 subject 上报错），IR 不应到达
                    Expr::TypeLit { .. } => {
                        return Err(IrError {
                            message: "内部错误：case 类型匹配不应到达 IR 生成（语义层已拦截）".into(),
                        });
                    }
                    // 字面量 pattern：现有比较逻辑（字符串 strcmp / 其余 cmp_op）
                    _ => {
                        let (case_val, _case_ty) = self.gen_expr(pat)?;
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
                        conds.push(cond);
                    }
                }
            }
            // 多值 OR 合并：任一 pattern 命中即整体命中
            let mut match_cond = conds.remove(0);
            for c in conds {
                let or = self.new_reg();
                self.line(&format!("{or} = or i1 {match_cond}, {c}"));
                match_cond = or;
            }
            // when 守卫：值匹配 且 守卫为真才进入分支体
            if let Some(w) = &case.when {
                let (wv, _w_ty) = self.gen_expr(w)?;
                let and = self.new_reg();
                self.line(&format!("{and} = and i1 {match_cond}, {wv}"));
                match_cond = and;
            }
            let else_target = match &next_cmp {
                Some(l) => l.clone(),
                None => def_label.clone().unwrap_or_else(|| exit_label.clone()),
            };
            self.line(&format!("br i1 {match_cond}, label %{body_label}, label %{else_target}"));
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

    /// C5：能否生成 LLVM switch 跳转表（O(1) 分派）。
    ///
    /// 条件：整数 subject（非字符串/字符/浮点）+ 每个 case 恰一个整数常量字面量
    /// pattern + 无 when 守卫。区间/多值/守卫/字符串走逐 case 比较链（原逻辑）。
    fn can_emit_switch_table(
        &self,
        cases: &[tie_frontend::ast::SwitchCase],
        is_str_subj: &bool,
        is_char: bool,
    ) -> bool {
        if *is_str_subj || is_char {
            return false;
        }
        !cases.is_empty()
            && cases.iter().all(|c| {
                c.when.is_none()
                    && c.patterns.len() == 1
                    && matches!(c.patterns[0], Expr::IntLit(_))
            })
    }

    // ---------- 表达式生成 ----------

    /// 生成表达式，返回 (值名, LLVM 类型名)。
    fn gen_expr(&mut self, expr: &Expr) -> Result<(String, &'static str), IrError> {
        match expr {
            Expr::IntLit(v) => Ok((v.to_string(), "i64")),
            Expr::FloatLit(v) => Ok((format_float(*v), "double")),
            Expr::BoolLit(b) => {
                // 平衡三进制字面量适配（M4 补齐）：语义层把 trit 上下文的
                // true/false 改写为 expr_types=Trit（`var t: trit = true`），
                // 此处按语义类型生成 i8 值（true→1 / false→-1）；否则 i1。
                if matches!(self.sem_ty_of(expr), Some(TypeSpec::Named(TyKw::Trit))) {
                    let v: i8 = if *b { 1 } else { -1 };
                    Ok((v.to_string(), "i8"))
                } else {
                    Ok((if *b { "true".into() } else { "false".into() }, "i1"))
                }
            }
            // 平衡三进制 trit 字面量（M4 补齐）：i8 常量 -1/0/1
            Expr::TritLit(v) => Ok((v.to_string(), "i8")),
            Expr::CharLit(c) => Ok(((*c as i32).to_string(), "i32")),
            Expr::StrLit(s) => {
                let g = self.string_global(s);
                // 字符串：返回全局常量指针（ptr 类型，供 %s / 传参直接使用）
                Ok((format!("@{g}"), "ptr"))
            }
            Expr::TypeLit { ty, .. } => Err(IrError {
                message: format!(
                    "内部错误：case 类型匹配（{}）不应到达 IR 表达式生成（语义层已拦截）",
                    type_name_of(ty)
                ),
            }),
            Expr::Var(name) => {
                // 克隆绑定以结束对 scopes 的借用，随后可安全调用 &mut 方法
                if let Some(bind) = self.lookup_var(name).cloned() {
                    let ty = bind.ty;
                    // i1 类型需要扩展（load i1 无法直接使用），这里统一 load
                    let tmp = self.new_reg();
                    self.line(&format!("{tmp} = load {ty}, ptr {}", bind.value));
                    return Ok((tmp, ty));
                }
                // 未命中函数作用域：顶层全局持久变量（M4）→ load @name
                if let Some(ty) = self.sem.globals.get(name).cloned() {
                    let llvm = self.llvm_ty(&ty);
                    let tmp = self.new_reg();
                    self.line(&format!("{tmp} = load {llvm}, ptr @{name}"));
                    return Ok((tmp, llvm));
                }
                Err(IrError {
                    message: format!("内部错误：变量 '{name}' 未入作用域（函数 {}）", self.cur_fn),
                })
            }
            Expr::Call { name, args, .. } => {
                // 构造调用：类名(...) → insertvalue 链构建结构体值（P8）
                if let Some(info) = self.sem.classes.get(name).cloned() {
                    return self.gen_construct(name, &info, args);
                }
                // 命名空间内裸调用（如 tcmsg::error 内 helper()）：语义层已把调用点
                // 解析为全名记录在 resolved_calls，这里取全名生成调用目标。
                let key = expr as *const Expr as usize;
                if let Some(full) = self.sem.resolved_calls.get(&key) {
                    self.gen_call(full, args)
                } else {
                    self.gen_call(name, args)
                }
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
                        // 平衡三值逻辑非（M4 补齐）：trit 是 i8，取反 = 0 - val
                        //（-1↔1，0 保持）；bool 保持 xor true。
                        if ty == "i8" {
                            let tmp = self.new_reg();
                            self.line(&format!("{tmp} = sub i8 0, {val}"));
                            Ok((tmp, "i8"))
                        } else {
                            let tmp = self.new_reg();
                            self.line(&format!("{tmp} = xor i1 {val}, true"));
                            Ok((tmp, "i1"))
                        }
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
                    TypeSpec::Struct(class_name) => {
                        // 取对象地址：变量/字段链 → 地址；否则不可寻址
                        let (base_ptr, base_llvm) = self.gen_class_addr(base)?;
                        let info = self
                            .sem
                            .classes
                            .get(class_name)
                            .cloned()
                            .ok_or_else(|| IrError {
                                message: format!("内部错误：struct '{class_name}' 无信息（函数 {}）", self.cur_fn),
                            })?;
                        let idx = info.field_index.get(field).copied().ok_or_else(|| IrError {
                            message: format!("内部错误：struct '{class_name}' 无字段 '{field}'（函数 {}）", self.cur_fn),
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
                // M2.1.8 统一分发：语义层已把一切 MethodCall（命名空间调用 / 静态
                // 调用 / struct 实例方法转发）解析为全名记录在 resolved_calls。
                // - receiver 是「可求值实例」（绑定变量/字段链/构造/方法链）→ 实例转发，
                //   实参 = [receiver] + args（方法函数首参 = 隐含接收者）；
                // - 否则（未绑定 Var / Path / 未绑定链）→ 命名空间/静态调用，实参 = args。
                let key = expr as *const Expr as usize;
                let full = self.sem.resolved_calls.get(&key).cloned().ok_or_else(|| {
                    IrError {
                        message: format!(
                            "内部错误：方法调用缺少解析记录（{method}，函数 {}）",
                            self.cur_fn
                        ),
                    }
                })?;
                if self.receiver_is_value(receiver) {
                    self.gen_call_inner(&full, args, Some(receiver))
                } else {
                    self.gen_call(&full, args)
                }
            }
            // 命名空间路径独立出现：语义层已拦截（只能作调用 receiver），IR 层防御
            Expr::Path { .. } => Err(IrError {
                message: format!("内部错误：命名空间路径不能作为值（函数 {}）", self.cur_fn),
            }),
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
        // 下标值：整数（i64 直接使用，窄整数先扩展）
        let (idx_val, idx_ty) = self.gen_expr(index)?;
        let idx_val = self.extend_int_to_i64(&idx_val, idx_ty, index)?;
        // base 是返回表的函数调用（csv.csv_cells(...)[0]）：求值得到动态表指针，
        // 走 tie_table_at（与动态表变量同一路径；函数调用结果直接是 ptr，无需 load）。
        if matches!(base, Expr::Call { .. } | Expr::MethodCall { .. }) {
            let (tptr, _t_ty) = self.gen_expr(base)?;
            let elem_ty = self.dyn_table_elem_ty(base)?;
            let elem_llvm = self.llvm_ty(&elem_ty);
            let suffix = table_elem_suffix(elem_llvm);
            self.mark_used(&format!("tie_table_at_{suffix}"));
            let ok = self.emit_alloca("i1");
            self.line(&format!("store i1 1, ptr {ok}"));
            let val = self.new_reg();
            self.line(&format!(
                "{val} = call {elem_llvm} @tie_table_at_{suffix}(ptr {tptr}, i64 {idx_val}, ptr {ok})"
            ));
            let okv = self.new_reg();
            self.line(&format!("{okv} = load i1, ptr {ok}"));
            let ok_label = self.new_label("table_at.ok");
            let err_label = self.new_label("table_at.err");
            self.line(&format!("br i1 {okv}, label %{ok_label}, label %{err_label}"));
            self.block_start(&err_label);
            let tlen = self.table_len_reg(&tptr)?;
            self.gen_runtime_error(
                "运行时错误: table_at 下标越界：索引 %lld 超出长度 %lld",
                &[("i64", idx_val.clone()), ("i64", tlen)],
            );
            self.block_end();
            self.block_start(&ok_label);
            return Ok((val, elem_llvm));
        }
        // base 必须是表/字符串变量：查作用域拿到 alloca 指针 + 类型名（不做 load）
        let Expr::Var(name) = base else {
            return Err(IrError {
                message: "下标访问仅支持表/字符串变量、表字面量或返回表的函数调用".into(),
            });
        };
        let bind = self.lookup_var(name).cloned().ok_or_else(|| IrError {
            message: format!("内部错误：下标访问的变量 '{name}' 未入作用域（函数 {}）", self.cur_fn),
        })?;
        let base_ptr = bind.value;
        let base_ty = bind.ty;
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
        // 动态表下标：t[i] → tie_table_at（运行时 {ptr,len,cap}，越界报错）。
        // 动态表变量绑定为 ptr，语义层 table_vars 标记 dynamic=true。
        if self
            .sem
            .table_vars
            .get(&(self.cur_fn.clone(), name.clone()))
            .map(|info| info.dynamic)
            .unwrap_or(false)
        {
            let elem_ty = self.dyn_table_elem_ty(base)?;
            let elem_llvm = self.llvm_ty(&elem_ty);
            let suffix = table_elem_suffix(elem_llvm);
            self.mark_used(&format!("tie_table_at_{suffix}"));
            // 表变量是 alloca ptr，先 load 出表指针
            let tptr = self.new_reg();
            self.line(&format!("{tptr} = load ptr, ptr {base_ptr}"));
            let ok = self.emit_alloca("i1");
            self.line(&format!("store i1 1, ptr {ok}"));
            let val = self.new_reg();
            self.line(&format!(
                "{val} = call {elem_llvm} @tie_table_at_{suffix}(ptr {tptr}, i64 {idx_val}, ptr {ok})"
            ));
            let okv = self.new_reg();
            self.line(&format!("{okv} = load i1, ptr {ok}"));
            let ok_label = self.new_label("table_at.ok");
            let err_label = self.new_label("table_at.err");
            self.line(&format!("br i1 {okv}, label %{ok_label}, label %{err_label}"));
            self.block_start(&err_label);
            let tlen = self.table_len_reg(&tptr)?;
            self.gen_runtime_error(
                "运行时错误: table_at 下标越界：索引 %lld 超出长度 %lld",
                &[("i64", idx_val.clone()), ("i64", tlen)],
            );
            self.block_end();
            self.block_start(&ok_label);
            return Ok((val, elem_llvm));
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

    /// 平衡三进制 trit 二元运算生成（M4 补齐）。
    ///
    /// trit 以 i8 存储（值域 -1/0/1）。规则：
    /// - Kleene 逻辑：`&&` = min、`||` = max（icmp + select 实现，与解释路径一致）；
    /// - 饱和算术：`+ - *` 用 i8 运算后 clamp 到 [-1,1]（比较 + select 夹取）；
    /// - 比较：`== != < > <= >=` → icmp i8 → i1；
    /// - 混合：trit × i64 → sext i8→i64 后整数运算（返回 i64）。
    /// `lhs_is_trit`：左侧是否 trit（混合运算时决定 sext 方向与结果类型）。
    fn gen_binary_trit(
        &mut self,
        op: BinaryOp,
        lhs_is_trit: bool,
        lv: String,
        lt: &'static str,
        rv: String,
        rt: &'static str,
    ) -> Result<(String, &'static str), IrError> {
        // 混合 trit×i64：trit 侧 sext 到 i64 后走常规整数运算。
        // 两侧同为 trit（lt=="i8" 且 rt=="i8"）时无需提升。
        let mixed = !(lt == "i8" && rt == "i8");
        // trit 侧寄存器（需 sext 的一侧）与 i64 侧寄存器
        let (trit_reg, int_reg, trit_first) = if lt == "i8" {
            (lv.clone(), rv.clone(), true)
        } else {
            (rv.clone(), lv.clone(), false)
        };
        if mixed {
            // trit sext i8→i64
            let sext = self.new_reg();
            self.line(&format!("{sext} = sext i8 {trit_reg} to i64"));
            let tmp = self.new_reg();
            // 交换律（+ * == !=）顺序无感；非交换（- < > <= >=）按 trit op i64 语义
            let (a, b) = if trit_first {
                (sext.clone(), int_reg.clone())
            } else {
                (int_reg.clone(), sext.clone())
            };
            let instr = match op {
                BinaryOp::Add => format!("add i64 {a}, {b}"),
                BinaryOp::Sub => format!("sub i64 {a}, {b}"),
                BinaryOp::Mul => format!("mul i64 {a}, {b}"),
                BinaryOp::Eq => format!("icmp eq i64 {a}, {b}"),
                BinaryOp::NotEq => format!("icmp ne i64 {a}, {b}"),
                BinaryOp::Lt => format!("icmp slt i64 {a}, {b}"),
                BinaryOp::Gt => format!("icmp sgt i64 {a}, {b}"),
                BinaryOp::Le => format!("icmp sle i64 {a}, {b}"),
                BinaryOp::Ge => format!("icmp sge i64 {a}, {b}"),
                _ => {
                    return Err(IrError {
                        message: format!("trit 与 i64 不支持运算 {:?}", op),
                    })
                }
            };
            self.line(&format!("{tmp} = {instr}"));
            // 算术返回 i64，比较返回 i1
            let res_ty = if matches!(
                op,
                BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge
            ) {
                "i1"
            } else {
                "i64"
            };
            return Ok((tmp, res_ty));
        }
        // ---- 两侧同为 trit（i8 × i8）----
        // Kleene 逻辑（And/Or）与饱和算术（Add/Sub/Mul）生成多指令后直接返回
        // 结果寄存器；比较类生成单条 icmp 指令写入 tmp 后返回。
        match op {
            // Kleene 逻辑：min（&&）/ max（||）——icmp + select
            BinaryOp::And | BinaryOp::Or => {
                let cmp_op = if op == BinaryOp::And { "slt" } else { "sgt" };
                let c = self.new_reg();
                self.line(&format!("{c} = icmp {cmp_op} i8 {lv}, {rv}"));
                let s = self.new_reg();
                // min 时 c 真选 lv（lv<rv），max 时 c 真选 lv（lv>rv）
                self.line(&format!("{s} = select i1 {c}, i8 {lv}, i8 {rv}"));
                return Ok((s, "i8"));
            }
            // 饱和算术：运算后 clamp 到 [-1,1]
            BinaryOp::Add => return Ok(self.trit_arith("add", &lv, &rv)),
            BinaryOp::Sub => return Ok(self.trit_arith("sub", &lv, &rv)),
            BinaryOp::Mul => return Ok(self.trit_arith("mul", &lv, &rv)),
            _ => {}
        }
        // 比较类：单条 icmp 指令
        let tmp = self.new_reg();
        let instr: String = match op {
            BinaryOp::Eq => format!("icmp eq i8 {lv}, {rv}"),
            BinaryOp::NotEq => format!("icmp ne i8 {lv}, {rv}"),
            BinaryOp::Lt => format!("icmp slt i8 {lv}, {rv}"),
            BinaryOp::Gt => format!("icmp sgt i8 {lv}, {rv}"),
            BinaryOp::Le => format!("icmp sle i8 {lv}, {rv}"),
            BinaryOp::Ge => format!("icmp sge i8 {lv}, {rv}"),
            BinaryOp::Div | BinaryOp::Mod => {
                return Err(IrError {
                    message: "trit 不支持除/取模运算（三值无除法）".into(),
                })
            }
            _ => {
                return Err(IrError {
                    message: format!("trit 不支持位运算 {:?}", op),
                })
            }
        };
        self.line(&format!("{tmp} = {instr}"));
        Ok((tmp, "i1"))
    }

    /// trit 饱和算术辅助：`{opcode} i8 lv, rv` 后 clamp 到 [-1,1]。
    /// 返回 (结果寄存器, "i8")。
    fn trit_arith(&mut self, opcode: &str, lv: &str, rv: &str) -> (String, &'static str) {
        // 原始运算（i8 可能溢出，先 sext 到 i64 运算再 clamp，与解释路径一致）
        let sext_l = self.new_reg();
        self.line(&format!("{sext_l} = sext i8 {lv} to i64"));
        let sext_r = self.new_reg();
        self.line(&format!("{sext_r} = sext i8 {rv} to i64"));
        let raw = self.new_reg();
        self.line(&format!("{raw} = {opcode} i64 {sext_l}, {sext_r}"));
        // clamp：raw < -1 → -1；raw > 1 → 1；否则 raw
        let lo = self.new_reg();
        self.line(&format!("{lo} = icmp slt i64 {raw}, -1"));
        let hi = self.new_reg();
        self.line(&format!("{hi} = icmp sgt i64 {raw}, 1"));
        let cl = self.new_reg();
        self.line(&format!("{cl} = select i1 {lo}, i64 -1, i64 {raw}"));
        let ch = self.new_reg();
        self.line(&format!("{ch} = select i1 {hi}, i64 1, i64 {cl}"));
        let trunc = self.new_reg();
        self.line(&format!("{trunc} = trunc i64 {ch} to i8"));
        (trunc, "i8")
    }

    /// 二元运算生成：两侧表达式求值后交给 [gen_binary_on_regs] 统一生成指令。
    fn gen_binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<(String, &'static str), IrError> {
        let (lv, lt) = self.gen_expr(lhs)?;
        let (rv, rt) = self.gen_expr(rhs)?;
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
        // 平衡三进制 trit（M4 补齐）：走专门的 trit 运算生成
        //（Kleene 逻辑 min/max、饱和算术 clamp、trit×i64 sext 混合）。
        let lhs_is_trit = matches!(lhs_sem, Some(TypeSpec::Named(TyKw::Trit)));
        let rhs_is_trit = matches!(self.sem_ty_of(rhs), Some(TypeSpec::Named(TyKw::Trit)));
        if lhs_is_trit || rhs_is_trit {
            return self.gen_binary_trit(op, lhs_is_trit, lv, lt, rv, rt);
        }
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
    /// 普通函数/命名空间函数调用（无隐含接收者）。
    fn gen_call(&mut self, name: &str, args: &[Expr]) -> Result<(String, &'static str), IrError> {
        self.gen_call_inner(name, args, None)
    }

    /// 函数调用实际实现（M2.1.8 起支持 `first` = 实例方法转发的隐含接收者）。
    fn gen_call_inner(
        &mut self,
        name: &str,
        args: &[Expr],
        first: Option<&Expr>,
    ) -> Result<(String, &'static str), IrError> {
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
            // 动态表变量：LLVM 类型为 ptr，语义层 table_vars 标记 dynamic=true。
            // 长度运行时求值：调用 tie_table_len。
            if let Expr::Var(name) = &args[0]
                && self
                    .sem
                    .table_vars
                    .get(&(self.cur_fn.clone(), name.clone()))
                    .map(|info| info.dynamic)
                    .unwrap_or(false)
            {
                let len = self.table_len_reg(&v)?;
                return Ok((len, "i64"));
            }
            // 字符串：strlen
            let len = self.new_reg();
            self.line(&format!("{len} = call i64 @strlen(ptr {v})"));
            return Ok((len, "i64"));
        }
        // 内置 str_len：字符串的 Unicode 码点数（与 str_char 码点索引对齐）。
        // len 返回字节数（strlen），对中文等多字节字符字节数 > 码点数，遍历错位；
        // str_len 返回码点数（chars().count()），供字符串库做码点级遍历边界。
        // 语义层已保证单参数为字符串（非表，表用 len）。
        if name == "str_len" {
            self.mark_used("tie_str_len");
            let (v, _t) = self.gen_expr(&args[0])?;
            let len = self.new_reg();
            self.line(&format!("{len} = call i64 @tie_str_len(ptr {v})"));
            return Ok((len, "i64"));
        }
        // 内置 table_new_*：零参数，创建空动态表，返回不透明指针（运行时 {ptr,len,cap}）。
        // 元素宽度由函数名决定：i64/f64/string=8 字节，bool=1 字节（与 tie-interp 桥一致）。
        if let Some(elem_size) = table_new_elem_size(name) {
            self.mark_used("tie_table_new");
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_table_new(i64 {elem_size})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 table_push：双参数（动态表变量 + 元素），void。
        // 元素类型由语义层保证与表一致；按元素类型选对应 push 桥。
        if name == "table_push" {
            let Expr::Var(tname) = &args[0] else {
                return Err(IrError {
                    message: format!("内部错误：table_push 第 1 个参数不是表变量（函数 {}）", self.cur_fn),
                });
            };
            let bind = self.lookup_var(tname).cloned().ok_or_else(|| IrError {
                message: format!("内部错误：table_push 找不到表变量 '{}'（函数 {}）", tname, self.cur_fn),
            })?;
            // 表变量绑定的是 alloca（存 ptr），需 load 出表指针
            let tptr = self.new_reg();
            self.line(&format!("{tptr} = load ptr, ptr {}", bind.value));
            let (x, x_ty) = self.gen_expr(&args[1])?;
            let suffix = table_elem_suffix(x_ty);
            self.mark_used(&format!("tie_table_push_{suffix}"));
            self.line(&format!("call void @tie_table_push_{suffix}(ptr {tptr}, {x_ty} {x})"));
            return Ok((String::new(), "void"));
        }
        // 内置 table_at：双参数（动态表 + 整数下标），返回表元素类型。
        // 越界 → 运行时错误（ok 标志置 0），文本与解释路径一致。
        if name == "table_at" {
            let (t, _t_ty) = self.gen_expr(&args[0])?;
            let (i, _i_ty) = self.gen_expr(&args[1])?;
            // 元素类型来自语义元数据（表变量查 table_vars，返回表的函数查 table_ret_elems）
            let elem_ty = self.dyn_table_elem_ty(&args[0])?;
            let elem_llvm = self.llvm_ty(&elem_ty);
            let suffix = table_elem_suffix(elem_llvm);
            self.mark_used(&format!("tie_table_at_{suffix}"));
            // ok 标志：alloca i1，桥函数越界时置 0
            let ok = self.emit_alloca("i1");
            self.line(&format!("store i1 1, ptr {ok}"));
            let val = self.new_reg();
            self.line(&format!(
                "{val} = call {elem_llvm} @tie_table_at_{suffix}(ptr {t}, i64 {i}, ptr {ok})"
            ));
            // 检查 ok：0 → 运行时错误（越界）
            let okv = self.new_reg();
            self.line(&format!("{okv} = load i1, ptr {ok}"));
            let ok_label = self.new_label("table_at.ok");
            let err_label = self.new_label("table_at.err");
            self.line(&format!("br i1 {okv}, label %{ok_label}, label %{err_label}"));
            self.block_start(&err_label);
            let tlen = self.table_len_reg(&t)?;
            self.gen_runtime_error(
                "运行时错误: table_at 下标越界：索引 %lld 超出长度 %lld",
                &[("i64", i.clone()), ("i64", tlen)],
            );
            self.block_end();
            self.block_start(&ok_label);
            return Ok((val, elem_llvm));
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
        // 内置 eval_call：两个字符串参数（函数名, 参数），调用已注册用户函数并返回结果字符串。
        // tie:script 模块协议执行基础：与解释路径同走 C ABI 桥（共享 Session），行为一致。
        // 返回值是 tie-interp 分配的堆串，调用方用完必须 tie_free_result 释放。
        if name == "eval_call" {
            self.mark_used("tie_eval_call");
            self.mark_used("tie_free_result");
            let (n, _t) = self.gen_expr(&args[0])?;
            let (a, _t) = self.gen_expr(&args[1])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_eval_call(ptr {n}, ptr {a})"));
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
        // 内置 file_delete：单字符串参数，返回 bool（文件是否删除成功）。
        // 用 libc remove(path)：成功返回 0 → true；失败（不存在/不可删）返回非 0 → false。
        // 与解释路径 std::fs::remove_file 行为一致（均返回 bool、无错误消息）。
        if name == "file_delete" {
            let (p, _t) = self.gen_expr(&args[0])?;
            let r = self.new_reg();
            self.line(&format!("{r} = call i32 @remove(ptr {p})"));
            // remove 返回 0 = 成功
            let ok = self.new_reg();
            self.line(&format!("{ok} = icmp eq i32 {r}, 0"));
            return Ok((ok, "i1"));
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
            let ok = self.emit_alloca("i8");
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
            let ok = self.emit_alloca("i8");
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
        // 内置 parse_trit（M4 补齐）：字符串参数，返回 trit（i8）。
        // 走 C ABI 桥（非法输入 → 运行时错误，文本与解释路径一致）。
        if name == "parse_trit" {
            self.mark_used("tie_parse_trit");
            let (s, _t) = self.gen_expr(&args[0])?;
            let ok = self.emit_alloca("i8");
            self.line(&format!("store i8 0, ptr {ok}"));
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call i8 @tie_parse_trit(ptr {s}, ptr {ok})"));
            let okv = self.new_reg();
            self.line(&format!("{okv} = load i8, ptr {ok}"));
            let is_zero = self.new_reg();
            self.line(&format!("{is_zero} = icmp eq i8 {okv}, 0"));
            let ok_label = self.new_label("parse_trit.ok");
            let err_label = self.new_label("parse_trit.err");
            self.line(&format!("br i1 {is_zero}, label %{err_label}, label %{ok_label}"));
            // 解析失败 → 运行时错误
            self.block_start(&err_label);
            self.gen_runtime_error(
                "运行时错误: parse_trit 参数 '%s' 不是合法的 trit（期望 -1/0/1）",
                &[("ptr", s)],
            );
            self.block_end();
            // 成功块：返回解析值（i8 = trit）
            self.block_start(&ok_label);
            return Ok((tmp, "i8"));
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
            let ok = self.emit_alloca("i8");
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
        // 内置 arg_count：零参数，返回 i64（命令行用户参数个数，不含程序名）。
        // 走 C ABI 桥（与解释路径共用 std::env::args，编译后的 exe 直接读进程 argv）。
        if name == "arg_count" {
            self.mark_used("tie_arg_count");
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call i64 @tie_arg_count()"));
            return Ok((tmp, "i64"));
        }
        // 内置 arg_string：整数参数，返回 string（第 i 个用户命令行参数；越界返回空串）。
        // 返回值是 tie-interp 分配的堆串，调用方用完必须 tie_free_result 释放
        // （独立语句时在 gen_stmt 的 Expr 分支统一释放，与 file_read/str_char 同机制）。
        if name == "arg_string" {
            self.mark_used("tie_arg_string");
            self.mark_used("tie_free_result");
            let (i, i_ty) = self.gen_expr(&args[0])?;
            // 下标统一扩展到 i64（C ABI 桥的参数类型）
            let i64 = self.extend_int_to_i64(&i, i_ty, &args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_arg_string(i64 {i64})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 list_dir：单字符串参数（目录路径），返回字符串动态表（DynTable 指针）。
        // 走 C ABI 桥（与解释路径共用 std::fs::read_dir）；目录无效 → 运行时错误。
        if name == "list_dir" {
            self.mark_used("tie_list_dir");
            let (p, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_list_dir(ptr {p})"));
            // 判断返回 NULL：失败 → 错误块（退出进程），成功 → ok 块继续
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {tmp}, null"));
            let ok_label = self.new_label("list_dir.ok");
            let err_label = self.new_label("list_dir.err");
            self.line(&format!("br i1 {is_null}, label %{err_label}, label %{ok_label}"));
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: list_dir 无法读取目录 '%s'", &[("ptr", p)]);
            self.block_end();
            // 成功块：返回值即表指针（调用方可用 len/for/table_at 访问）
            self.block_start(&ok_label);
            return Ok((tmp, "ptr"));
        }
        // ---------- M4 补齐：系统能力内置函数（M6 包管理器前置） ----------
        //
        // 设计说明（编译/解释两路径一致性的关键）：全部走 tie-interp 的 C ABI 桥
        // （与解释路径共用同一份 Rust 实现，行为逐字节一致）：
        // - 返回堆串（http_get/exec_output/path_*/cwd/get_env）：堆串机制，用完 tie_free_result；
        // - 返回 bool（i8：http_get_file/untar_gz/unzip/mkdir_all/remove_dir_all/copy_dir/
        //   file_copy/file_move）：i8 → icmp ne 0 → i1；
        // - 返回 i64（exec_code）：直接 i64；
        // - void（set_env）：调用后无返回；
        // - 字符串动态表（walk_dir）：与 list_dir 同模式（ptr 表指针 + NULL 错误分支）。

        // 内置 http_get：单字符串参数（URL），返回 string（响应正文）。
        // 失败（桥返回 NULL）→ 运行时错误，文本与解释路径一致。
        if name == "http_get" {
            self.mark_used("tie_http_get");
            self.mark_used("tie_free_result");
            let (u, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_http_get(ptr {u})"));
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {tmp}, null"));
            let ok_label = self.new_label("http_get.ok");
            let err_label = self.new_label("http_get.err");
            self.line(&format!("br i1 {is_null}, label %{err_label}, label %{ok_label}"));
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: http_get 无法访问 URL '%s'", &[("ptr", u)]);
            self.block_end();
            self.block_start(&ok_label);
            return Ok((tmp, "ptr"));
        }
        // 内置 http_get_file：两个字符串参数（URL, 路径），返回 bool（下载成功与否）。
        if name == "http_get_file" {
            self.mark_used("tie_http_get_file");
            let (u, _t) = self.gen_expr(&args[0])?;
            let (p, _t) = self.gen_expr(&args[1])?;
            let r = self.new_reg();
            self.line(&format!("{r} = call i8 @tie_http_get_file(ptr {u}, ptr {p})"));
            let ok = self.new_reg();
            self.line(&format!("{ok} = icmp ne i8 {r}, 0"));
            return Ok((ok, "i1"));
        }
        // 内置 exec_code：单字符串参数（命令行），返回 i64（退出码；启动失败 -1）。
        if name == "exec_code" {
            self.mark_used("tie_exec_code");
            let (c, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call i64 @tie_exec_code(ptr {c})"));
            return Ok((tmp, "i64"));
        }
        // 内置 exec_output：单字符串参数（命令行），返回 string（捕获 stdout；启动失败空串）。
        if name == "exec_output" {
            self.mark_used("tie_exec_output");
            self.mark_used("tie_free_result");
            let (c, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_exec_output(ptr {c})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 untar_gz / unzip：两个字符串参数（归档, 目标目录），返回 bool（解压成功与否）。
        if name == "untar_gz" || name == "unzip" {
            self.mark_used(&format!("tie_{name}"));
            let (f, _t) = self.gen_expr(&args[0])?;
            let (d, _t) = self.gen_expr(&args[1])?;
            let r = self.new_reg();
            self.line(&format!("{r} = call i8 @tie_{name}(ptr {f}, ptr {d})"));
            let ok = self.new_reg();
            self.line(&format!("{ok} = icmp ne i8 {r}, 0"));
            return Ok((ok, "i1"));
        }
        // 内置 mkdir_all / remove_dir_all：单字符串参数（路径），返回 bool（成功与否）。
        if name == "mkdir_all" || name == "remove_dir_all" {
            self.mark_used(&format!("tie_{name}"));
            let (p, _t) = self.gen_expr(&args[0])?;
            let r = self.new_reg();
            self.line(&format!("{r} = call i8 @tie_{name}(ptr {p})"));
            let ok = self.new_reg();
            self.line(&format!("{ok} = icmp ne i8 {r}, 0"));
            return Ok((ok, "i1"));
        }
        // 内置 copy_dir：两个字符串参数（源, 目标目录），返回 bool（复制成功与否）。
        if name == "copy_dir" {
            self.mark_used("tie_copy_dir");
            let (s, _t) = self.gen_expr(&args[0])?;
            let (d, _t) = self.gen_expr(&args[1])?;
            let r = self.new_reg();
            self.line(&format!("{r} = call i8 @tie_copy_dir(ptr {s}, ptr {d})"));
            let ok = self.new_reg();
            self.line(&format!("{ok} = icmp ne i8 {r}, 0"));
            return Ok((ok, "i1"));
        }
        // 内置 walk_dir：单字符串参数（目录），返回字符串动态表（全部文件相对路径）。
        // 与 list_dir 同模式：桥返回 NULL → 运行时错误。
        if name == "walk_dir" {
            self.mark_used("tie_walk_dir");
            let (p, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_walk_dir(ptr {p})"));
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {tmp}, null"));
            let ok_label = self.new_label("walk_dir.ok");
            let err_label = self.new_label("walk_dir.err");
            self.line(&format!("br i1 {is_null}, label %{err_label}, label %{ok_label}"));
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: walk_dir 无法读取目录 '%s'", &[("ptr", p)]);
            self.block_end();
            self.block_start(&ok_label);
            return Ok((tmp, "ptr"));
        }
        // ---------- D7：字节流 / 位操作原语（编解码器底座） ----------
        // 与解释路径一致：走 C ABI 桥（共用同一份 Rust 实现）。
        // 字节表是 i64 动态表（DynTable 指针）；bit_read/bit_write 在桥层按位读写。
        // 内置 byte_read：单字符串参数（路径），返回 i64 字节表；失败 → 运行时错误。
        if name == "byte_read" {
            self.mark_used("tie_byte_read");
            let (p, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_byte_read(ptr {p})"));
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {tmp}, null"));
            let ok_label = self.new_label("byte_read.ok");
            let err_label = self.new_label("byte_read.err");
            self.line(&format!("br i1 {is_null}, label %{err_label}, label %{ok_label}"));
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: byte_read 无法读取文件 '%s'", &[("ptr", p)]);
            self.block_end();
            self.block_start(&ok_label);
            return Ok((tmp, "ptr"));
        }
        // 内置 byte_write：两个参数（路径, 字节表），返回 bool（写成功与否）。
        if name == "byte_write" {
            self.mark_used("tie_byte_write");
            let (p, _t) = self.gen_expr(&args[0])?;
            // 字节表变量是 alloca ptr → load 出表指针
            let (tbl, _tt) = self.gen_expr(&args[1])?;
            let r = self.new_reg();
            self.line(&format!("{r} = call i8 @tie_byte_write(ptr {p}, ptr {tbl})"));
            let ok = self.new_reg();
            self.line(&format!("{ok} = icmp ne i8 {r}, 0"));
            return Ok((ok, "i1"));
        }
        // 内置 bit_read：两个参数（字节表, 位置），返回 i64（第 pos 位 0/1；越界 0）。
        if name == "bit_read" {
            self.mark_used("tie_bit_read");
            let (tbl, _tt) = self.gen_expr(&args[0])?;
            let (pos, pos_ty) = self.gen_expr(&args[1])?;
            let pos64 = self.extend_int_to_i64(&pos, pos_ty, &args[1])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call i64 @tie_bit_read(ptr {tbl}, i64 {pos64})"));
            return Ok((tmp, "i64"));
        }
        // 内置 bit_write：三个参数（字节表, 位置, 位值），返回 bool（越界 false）。
        if name == "bit_write" {
            self.mark_used("tie_bit_write");
            let (tbl, _tt) = self.gen_expr(&args[0])?;
            let (pos, pos_ty) = self.gen_expr(&args[1])?;
            let pos64 = self.extend_int_to_i64(&pos, pos_ty, &args[1])?;
            let (bit, bit_ty) = self.gen_expr(&args[2])?;
            let bit64 = self.extend_int_to_i64(&bit, bit_ty, &args[2])?;
            let r = self.new_reg();
            self.line(&format!("{r} = call i8 @tie_bit_write(ptr {tbl}, i64 {pos64}, i64 {bit64})"));
            let ok = self.new_reg();
            self.line(&format!("{ok} = icmp ne i8 {r}, 0"));
            return Ok((ok, "i1"));
        }
        // 内置 byte_concat：两个字节表参数，返回拼接后的 i64 字节表。
        if name == "byte_concat" {
            self.mark_used("tie_byte_concat");
            let (a, _at) = self.gen_expr(&args[0])?;
            let (b, _bt) = self.gen_expr(&args[1])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_byte_concat(ptr {a}, ptr {b})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 path_join：两个字符串参数，返回 string（拼接路径）。
        if name == "path_join" {
            self.mark_used("tie_path_join");
            self.mark_used("tie_free_result");
            let (a, _t) = self.gen_expr(&args[0])?;
            let (b, _t) = self.gen_expr(&args[1])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_path_join(ptr {a}, ptr {b})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 path_basename / path_dirname / path_abs / path_normalize：单字符串参数，返回 string。
        if matches!(
            name,
            "path_basename" | "path_dirname" | "path_abs" | "path_normalize"
        ) {
            self.mark_used(&format!("tie_{name}"));
            self.mark_used("tie_free_result");
            let (p, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_{name}(ptr {p})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 cwd：零参数，返回 string（当前工作目录）。
        if name == "cwd" {
            self.mark_used("tie_cwd");
            self.mark_used("tie_free_result");
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_cwd()"));
            return Ok((tmp, "ptr"));
        }
        // 内置 get_env：单字符串参数（变量名），返回 string（值；不存在空串）。
        if name == "get_env" {
            self.mark_used("tie_get_env");
            self.mark_used("tie_free_result");
            let (n, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_get_env(ptr {n})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 set_env：两个字符串参数（变量名, 值），void（设置环境变量）。
        if name == "set_env" {
            self.mark_used("tie_set_env");
            let (n, _t) = self.gen_expr(&args[0])?;
            let (v, _t) = self.gen_expr(&args[1])?;
            self.line(&format!("call void @tie_set_env(ptr {n}, ptr {v})"));
            return Ok((String::new(), "void"));
        }
        // 内置 file_copy / file_move：两个字符串参数（源, 目标），返回 bool（成功与否）。
        if name == "file_copy" || name == "file_move" {
            self.mark_used(&format!("tie_{name}"));
            let (s, _t) = self.gen_expr(&args[0])?;
            let (d, _t) = self.gen_expr(&args[1])?;
            let r = self.new_reg();
            self.line(&format!("{r} = call i8 @tie_{name}(ptr {s}, ptr {d})"));
            let ok = self.new_reg();
            self.line(&format!("{ok} = icmp ne i8 {r}, 0"));
            return Ok((ok, "i1"));
        }
        // 内置 msg_set_lang：单字符串参数，void（切换消息系统当前语言）。
        // 走 C ABI 桥（与解释路径共用 thread_local 状态）。
        if name == "msg_set_lang" {
            self.mark_used("tie_msg_set_lang");
            let (l, _t) = self.gen_expr(&args[0])?;
            self.line(&format!("call void @tie_msg_set_lang(ptr {l})"));
            return Ok((String::new(), "void"));
        }
        // 内置 msg_get_lang：零参数，返回 string（当前消息语言）。
        // 返回值是 tie-interp 分配的堆串，调用方用完必须 tie_free_result 释放。
        if name == "msg_get_lang" {
            self.mark_used("tie_msg_get_lang");
            self.mark_used("tie_free_result");
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_msg_get_lang()"));
            return Ok((tmp, "ptr"));
        }
        // 内置 msg_register：三个字符串参数（键, 语言, 文本），void（登记消息；同键同语言覆盖）。
        if name == "msg_register" {
            self.mark_used("tie_msg_register");
            let (k, _t) = self.gen_expr(&args[0])?;
            let (l, _t) = self.gen_expr(&args[1])?;
            let (x, _t) = self.gen_expr(&args[2])?;
            self.line(&format!("call void @tie_msg_register(ptr {k}, ptr {l}, ptr {x})"));
            return Ok((String::new(), "void"));
        }
        // 内置 msg_t：单字符串参数（键），返回 string（当前语言翻译，回退 zh，再回退键本身）。
        // 返回值是 tie-interp 分配的堆串，调用方用完必须 tie_free_result 释放。
        if name == "msg_t" {
            self.mark_used("tie_msg_t");
            self.mark_used("tie_free_result");
            let (k, _t) = self.gen_expr(&args[0])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_msg_t(ptr {k})"));
            return Ok((tmp, "ptr"));
        }
        // 内置 print_err（M4）：单字符串参数，void——向 stderr 输出一行。
        if name == "print_err" {
            self.mark_used("tie_print_err");
            let (s, _t) = self.gen_expr(&args[0])?;
            self.line(&format!("call void @tie_print_err(ptr {s})"));
            return Ok((String::new(), "void"));
        }
        // 内置 msg_t_lang（M4）：两个字符串参数（键, 语言），返回 string（指定语言查询，
        // 未命中返回空串；返回值是堆串，用完必须 tie_free_result 释放）。
        if name == "msg_t_lang" {
            self.mark_used("tie_msg_t_lang");
            self.mark_used("tie_free_result");
            let (k, _t) = self.gen_expr(&args[0])?;
            let (l, _t) = self.gen_expr(&args[1])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_msg_t_lang(ptr {k}, ptr {l})"));
            return Ok((tmp, "ptr"));
        }
        // ---------- P1 正则表达式内置函数 ----------
        //
        // 与解释路径一致：走 C ABI 桥（共用 regex crate 实现），保证两路径行为逐字节一致。
        // 模式非法 → 运行时错误（错误文本与解释路径一致，含非法模式原文）。
        // regex_match 用 ok 标志（模式非法置 0）；regex_find/regex_replace/regex_group
        // 返回堆串（模式非法返回 NULL）；regex_find_all 返回字符串动态表（模式非法返回 NULL）。
        if name == "regex_match" {
            self.mark_used("tie_regex_match");
            let (s, _t) = self.gen_expr(&args[0])?;
            let (p, _t) = self.gen_expr(&args[1])?;
            // 栈上分配 ok 标志（桥写入 0/1），调用后检查：0 → 模式非法错误
            let ok = self.emit_alloca("i8");
            self.line(&format!("store i8 0, ptr {ok}"));
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call i8 @tie_regex_match(ptr {s}, ptr {p}, ptr {ok})"));
            let okv = self.new_reg();
            self.line(&format!("{okv} = load i8, ptr {ok}"));
            let is_zero = self.new_reg();
            self.line(&format!("{is_zero} = icmp eq i8 {okv}, 0"));
            let ok_label = self.new_label("regex_match.ok");
            let err_label = self.new_label("regex_match.err");
            self.line(&format!("br i1 {is_zero}, label %{err_label}, label %{ok_label}"));
            // 模式非法 → 运行时错误
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: regex_match 模式 '%s' 非法", &[("ptr", p)]);
            self.block_end();
            // 成功块：i8 → i1（bool）
            self.block_start(&ok_label);
            let is_true = self.new_reg();
            self.line(&format!("{is_true} = icmp ne i8 {tmp}, 0"));
            return Ok((is_true, "i1"));
        }
        if name == "regex_find" {
            self.mark_used("tie_regex_find");
            self.mark_used("tie_free_result");
            let (s, _t) = self.gen_expr(&args[0])?;
            let (p, _t) = self.gen_expr(&args[1])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_regex_find(ptr {s}, ptr {p})"));
            // 模式非法（NULL）→ 运行时错误
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {tmp}, null"));
            let ok_label = self.new_label("regex_find.ok");
            let err_label = self.new_label("regex_find.err");
            self.line(&format!("br i1 {is_null}, label %{err_label}, label %{ok_label}"));
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: regex_find 模式 '%s' 非法", &[("ptr", p)]);
            self.block_end();
            self.block_start(&ok_label);
            return Ok((tmp, "ptr"));
        }
        if name == "regex_find_all" {
            self.mark_used("tie_regex_find_all");
            let (s, _t) = self.gen_expr(&args[0])?;
            let (p, _t) = self.gen_expr(&args[1])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_regex_find_all(ptr {s}, ptr {p})"));
            // 模式非法（NULL）→ 运行时错误
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {tmp}, null"));
            let ok_label = self.new_label("regex_find_all.ok");
            let err_label = self.new_label("regex_find_all.err");
            self.line(&format!("br i1 {is_null}, label %{err_label}, label %{ok_label}"));
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: regex_find_all 模式 '%s' 非法", &[("ptr", p)]);
            self.block_end();
            // 成功块：返回值即表指针（调用方可用 len/for/table_at 访问）
            self.block_start(&ok_label);
            return Ok((tmp, "ptr"));
        }
        if name == "regex_replace" {
            self.mark_used("tie_regex_replace");
            self.mark_used("tie_free_result");
            let (s, _t) = self.gen_expr(&args[0])?;
            let (p, _t) = self.gen_expr(&args[1])?;
            let (to, _t) = self.gen_expr(&args[2])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_regex_replace(ptr {s}, ptr {p}, ptr {to})"));
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {tmp}, null"));
            let ok_label = self.new_label("regex_replace.ok");
            let err_label = self.new_label("regex_replace.err");
            self.line(&format!("br i1 {is_null}, label %{err_label}, label %{ok_label}"));
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: regex_replace 模式 '%s' 非法", &[("ptr", p)]);
            self.block_end();
            self.block_start(&ok_label);
            return Ok((tmp, "ptr"));
        }
        if name == "regex_group" {
            self.mark_used("tie_regex_group");
            self.mark_used("tie_free_result");
            let (s, _t) = self.gen_expr(&args[0])?;
            let (p, _t) = self.gen_expr(&args[1])?;
            let (i, i_ty) = self.gen_expr(&args[2])?;
            // 下标统一扩展到 i64（C ABI 桥的第三个参数类型）
            let i64 = self.extend_int_to_i64(&i, i_ty, &args[2])?;
            let tmp = self.new_reg();
            self.line(&format!("{tmp} = call ptr @tie_regex_group(ptr {s}, ptr {p}, i64 {i64})"));
            let is_null = self.new_reg();
            self.line(&format!("{is_null} = icmp eq ptr {tmp}, null"));
            let ok_label = self.new_label("regex_group.ok");
            let err_label = self.new_label("regex_group.err");
            self.line(&format!("br i1 {is_null}, label %{err_label}, label %{ok_label}"));
            self.block_start(&err_label);
            self.gen_runtime_error("运行时错误: regex_group 模式 '%s' 非法", &[("ptr", p)]);
            self.block_end();
            self.block_start(&ok_label);
            return Ok((tmp, "ptr"));
        }
        // 用户函数调用。符号名：命名空间函数全名（含 ::）转 $（与 gen_fn 的
        // ns_symbol 一致，保证定义与调用两侧符号同名）。
        let symbol = ns_symbol(name);
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
        // 而字面量 gen_expr 返回 i64，需要按签名类型生成。
        // 默认值参数（可选参数）：实参不足时按签名默认值补齐——LLVM 函数签名不变
        // （含全部形参），缺省实参在调用点直接生成（默认值限字面量/空表，无作用域依赖）。
        for (i, want_ty) in sig.param_tys.iter().enumerate() {
            // 首参（i==0）且是实例转发（first=Some）→ receiver（隐含接收者，无默认值）
            let is_first = first.is_some() && i == 0;
            let a: &Expr = if is_first {
                first.expect("is_first 为真时 first 必为 Some")
            } else {
                // 实参来源：调用方实参；不足时取该形参的默认值表达式
                let j = if first.is_some() { i - 1 } else { i };
                if let Some(a) = args.get(j) {
                    a
                } else {
                    sig.param_defaults
                        .get(i)
                        .and_then(|d| d.as_ref())
                        .ok_or_else(|| IrError {
                            message: format!(
                                "内部错误：函数 '{name}' 缺少第 {} 个实参且无默认值（函数 {}）",
                                i + 1,
                                self.cur_fn
                            ),
                        })?
                }
            };
            // 方法函数（namespace <struct名>，首参类型 == 该 struct 名）首参按**引用**
            // 传递：传 receiver 地址（ptr）。语义层已保证 receiver 可寻址。
            if is_first && self.is_method_fn(name, sig.param_tys.first()) {
                let (ptr, _ptr_llvm) = self.gen_class_addr(a)?;
                arg_list.push(format!("ptr {ptr}"));
                continue;
            }
            // 表字面量实参：table 形参在 LLVM 中是不透明 ptr（动态表），
            // 与定长表变量声明的数组布局不同，这里按动态表构造
            // （tie_table_new + 逐元素 tie_table_push_*），返回表指针。
            // 元素类型/长度来自语义布局元数据（infer_expr 已按表达式地址记录）。
            let (v, _t) = if let Expr::TableLit { cells, .. } = a {
                (self.gen_table_lit_arg(a, cells)?, "ptr")
            } else {
                self.gen_expr(a)?
            };
            arg_list.push(format!("{} {v}", self.llvm_ty(want_ty)));
        }
        let ret_llvm = self.llvm_ty(&sig.ret_ty);
        let tmp = self.new_reg();
        if sig.ret_ty.is_void() {
            self.line(&format!("call void @{}({})", symbol, arg_list.join(", ")));
            Ok((tmp, "void"))
        } else {
            self.line(&format!("{tmp} = call {ret_llvm} @{}({})", symbol, arg_list.join(", ")));
            Ok((tmp, ret_llvm))
        }
    }

    /// 是否为方法函数（M2.1.8）：`namespace <struct名>` 内的函数且首参类型 == 该
    /// struct 名——首参按**引用**（ptr）传递，函数内字段修改反映到调用方。
    ///
    /// 判定：全名 `ns::m` 的命名空间路径末段 == 首参 struct 名
    /// （`Point::dist(p: Point)` → ns 末段 "Point" == "Point"）。
    fn is_method_fn(&self, full: &str, first_ty: Option<&TypeSpec>) -> bool {
        let Some(TypeSpec::Struct(sn)) = first_ty else {
            return false;
        };
        let Some((ns, _)) = full.rsplit_once("::") else {
            return false;
        };
        ns.rsplit("::").next() == Some(sn.as_str())
    }

    /// receiver 是否为「可求值实例」（绑定变量/字段链/构造/方法链）→ 实例转发
    /// （实参插 receiver）。未绑定 Var / Path / 未绑定链 → 命名空间/静态调用。
    fn receiver_is_value(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Var(name) => self.scope_has(name),
            Expr::FieldAccess { base, .. } => self.receiver_is_value(base),
            Expr::MethodCall { .. } | Expr::Call { .. } | Expr::TupleLit { .. } => true,
            _ => false,
        }
    }

    /// 表字面量实参 → 动态表构造（tie_table_new + 逐元素 tie_table_push_*）。
    ///
    /// 背景：table 形参在 LLVM 中是不透明 ptr（运行时 {ptr,len,cap} 结构，见
    /// llvm_ty 的 Named(Table) => "ptr"），与定长表变量声明的数组布局
    /// `[N x T]` 不同——实参按动态表传递，函数体内用 table_len / table_at /
    /// 下标访问（与 table_new_* 创建的动态表行为一致）。
    ///
    /// 元素类型与长度来自语义布局元数据（infer_expr 的 TableLit 分支已按
    /// 表达式地址记录到 result.tables，键 = 表达式地址，与 len 内置同款取键）。
    /// 元素宽度：i1=1 字节，其余（i64/f64/ptr）=8 字节（与 tie-interp 桥一致）。
    /// 空表（元数据缺失）防御：生成空动态表（i64 元素、长度 0）。
    fn gen_table_lit_arg(&mut self, expr: &Expr, cells: &[TableCell]) -> Result<String, IrError> {
        // 查语义布局元数据（元素类型 + 长度）；空表可能无记录，按 i64 空表兜底
        let key = expr as *const Expr as usize;
        let info = self.sem.tables.get(&key);
        let elem_llvm = match info {
            Some(i) => self.llvm_ty(&i.elem_ty),
            None => "i64",
        };
        let elem_size = if elem_llvm == "i1" { 1 } else { 8 };
        let suffix = table_elem_suffix(elem_llvm);
        // 新建空动态表
        self.mark_used("tie_table_new");
        let t = self.new_reg();
        self.line(&format!("{t} = call ptr @tie_table_new(i64 {elem_size})"));
        // 逐元素求值并 push（元素值 LLVM 类型与桥参数类型一致）
        self.mark_used(&format!("tie_table_push_{suffix}"));
        for cell in cells {
            let (v, _vt) = self.gen_expr(&cell.value)?;
            self.line(&format!("call void @tie_table_push_{suffix}(ptr {t}, {elem_llvm} {v})"));
        }
        Ok(t)
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
        let agg_ty = self.llvm_ty(&TypeSpec::Struct(class_name.to_string()));
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

    /// 求 struct 实例表达式的内存地址（供字段 GEP 使用）。
    ///
    /// 支持：
    /// - 变量（VarBind：alloca 指针）→ 直接返回绑定地址；
    /// - 字段链（obj.a.b）→ 递归：先取 obj 地址，再逐级 GEP 到字段。
    ///
    /// 返回 (地址寄存器, 该地址指向的结构体 LLVM 类型)。
    /// 语义层已保证表达式类型为 struct，此处仅内部防御。
    fn gen_class_addr(&mut self, expr: &Expr) -> Result<(String, &'static str), IrError> {
        match expr {
            // 变量：绑定地址即对象地址（alloca 指针）
            Expr::Var(name) => {
                let bind = self.lookup_var(name).cloned().ok_or_else(|| IrError {
                    message: format!("内部错误：变量 '{name}' 未入作用域（函数 {}）", self.cur_fn),
                })?;
                // 普通变量：alloca 指针，GEP 直接用绑定地址
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
                let TypeSpec::Struct(class_name) = &base_ty else {
                    return Err(IrError {
                        message: format!(
                            "内部错误：字段链的基类型不是 struct（{}，函数 {}）",
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
                        message: format!("内部错误：struct '{class_name}' 无信息（函数 {}）", self.cur_fn),
                    })?;
                let idx = info.field_index.get(field).copied().ok_or_else(|| IrError {
                    message: format!(
                        "内部错误：struct '{class_name}' 无字段 '{field}'（函数 {}）",
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
        // print（不换行）用于交互提示（如 repl 外壳的 `print("> ")`）：必须立即刷新
        // stdout，否则 C stdio 缓冲可能不显示提示符；且 repl 中 eval 解释路径的 print
        // 走 Rust stdout（各自 flush），两套缓冲写同一 fd 会乱序，统一即时刷出可避免。
        if !newline {
            self.line("call i32 @fflush(ptr null)");
            // fflush 返回 i32，LLVM 解析器会分配隐式寄存器号，必须消费掉
            let _ = self.new_reg();
        }
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
        // 错误块含两个非 void 的未编号调用（printf、fflush 均返回 i32），
        // LLVM 解析器会为它们各分配一个隐式寄存器号，必须全部消费掉，
        // 否则后续块显式寄存器号会与隐式号冲突（"instruction expected to be numbered"）。
        let _ = self.new_reg();
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

    /// 解析动态表表达式的元素类型（table_at 返回类型 / 下标访问用）。
    ///
    /// 表变量查 table_vars（键 = 当前函数 + 变量名）；返回表的函数调用查 table_ret_elems。
    /// 动态表的 LLVM 类型恒为 "ptr"，元素类型必须从语义元数据取。
    fn dyn_table_elem_ty(&self, expr: &Expr) -> Result<TypeSpec, IrError> {
        match expr {
            Expr::Var(name) => {
                let key = (self.cur_fn.clone(), name.clone());
                self.sem
                    .table_vars
                    .get(&key)
                    .map(|info| info.elem_ty.clone())
                    .ok_or_else(|| IrError {
                        message: format!(
                            "内部错误：动态表变量 '{}' 缺少元素类型元数据（函数 {}）",
                            name, self.cur_fn
                        ),
                    })
            }
            Expr::Call { .. } | Expr::MethodCall { .. } => {
                // 裸调用 / 命名空间调用（str.split）：统一解析全名后查 table_ret_elems
                let key = expr as *const Expr as usize;
                let full = self
                    .sem
                    .resolved_calls
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| {
                        // 无解析记录（非命名空间裸调用）→ 按表达式名字取
                        match expr {
                            Expr::Call { name, .. } => name.clone(),
                            Expr::MethodCall { method, .. } => method.clone(),
                            _ => String::new(),
                        }
                    });
                self.sem
                    .table_ret_elems
                    .get(&full)
                    .and_then(|o| o.clone())
                    .ok_or_else(|| IrError {
                        message: format!(
                            "内部错误：返回表的函数 '{full}' 缺少元素类型元数据（函数 {}）",
                            self.cur_fn
                        ),
                    })
            }
            _ => Err(IrError {
                message: format!("内部错误：table_at 第 1 个参数不是动态表（函数 {}）", self.cur_fn),
            }),
        }
    }

    /// 生成 tie_table_len 调用，返回表长度寄存器（table_at 越界错误消息用）。
    fn table_len_reg(&mut self, t: &str) -> Result<String, IrError> {
        self.mark_used("tie_table_len");
        let len = self.new_reg();
        self.line(&format!("{len} = call i64 @tie_table_len(ptr {t})"));
        Ok(len)
    }

    /// 当前函数/方法的返回类型（Return 生成按签名类型适配字面量）。
    ///
    /// 当前函数的返回类型（Return 生成按签名类型适配字面量）。
    ///
    /// 普通函数/命名空间函数（含方法函数，如 Point::dist）都查 funcs 表。
    fn current_ret_ty(&self) -> TypeSpec {
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

    /// 申请一个 alloca（F1：alloca 提升）。
    ///
    /// 指令不立即输出，而是进入 entry_allocas 缓冲；gen_fn 在函数体前统一 flush
    /// 到 entry block 末尾。保证所有 alloca 位于 entry block（LLVM 规范），
    /// 避免循环体内 alloca 逐次迭代累积栈空间导致栈溢出（0xC00000FD）。
    fn emit_alloca(&mut self, ty: &str) -> String {
        let reg = self.new_reg();
        self.entry_allocas.push(format!("{reg} = alloca {ty}"));
        reg
    }

    /// flush 全部提升的 alloca 到当前输出位置（gen_fn 的 entry 块末尾调用）。
    fn flush_allocas(&mut self) {
        let allocas = std::mem::take(&mut self.entry_allocas);
        for a in &allocas {
            self.line(a);
        }
    }

    /// 全局重编号 IR 文本（F1 配套）：
    ///
    /// alloca 提升后，entry 块的 alloca 指令（如 %54 = alloca i1）在文本上位于
    /// 函数体低编号指令（%1 = call ...）之前，编号倒挂。LLVM 解析器要求指令编号
    /// 严格递增，故按文本出现顺序把 %N 统一重映射为 1..N。
    ///
    /// 行级处理要点：
    /// - `%N = ...` 定义：重映射编号（首次出现分配新号）；
    /// - 无编号指令（call/store/br/ret 等）：分配递增编号（LLVM 允许指令带编号）；
    ///   其中变参 call（`call T (ptr, ...) @f`）经实测消耗 **2 个**编号槽
    ///   （该 LLVM 版本解析变参调用时推进两个期望值），故编号再 +1 保证连续；
    /// - 标签/注释/空行/declare/define：原样保留，不占编号。
    fn renumber_ir(ir: &str) -> String {
        use std::collections::HashMap;
        let mut map: HashMap<u32, u32> = HashMap::new();
        let mut next: u32 = 1;
        let mut result = String::with_capacity(ir.len());
        for line in ir.lines() {
            let trimmed = line.trim_start();
            // 新函数开始：重置编号（各函数编号独立，LLVM 允许跨函数复用）
            if trimmed.starts_with("define ") {
                map.clear();
                next = 1;
                result.push_str(line);
                result.push('\n');
                continue;
            }
            // 标签 / 注释 / 空行 / 声明行：原样保留（不占编号）
            if trimmed.is_empty()
                || trimmed.starts_with(';')
                || trimmed.ends_with(':')
                || trimmed.starts_with("declare ")
                || trimmed.starts_with("attributes ")
                || trimmed.starts_with("source_filename")
                || trimmed.starts_with("target ")
                || trimmed.starts_with("ModuleID")
            {
                result.push_str(line);
                result.push('\n');
                continue;
            }
            // 有编号指令：`%N = ...` → 重映射编号（引用同一 %N 也复用映射）
            let mut rest = trimmed;
            if rest.starts_with('%') {
                let bytes = rest.as_bytes();
                let mut j = 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > 1 {
                    let n: u32 = rest[1..j].parse().unwrap_or(0);
                    let new = *map.entry(n).or_insert_with(|| {
                        let v = next;
                        next += 1;
                        v
                    });
                    result.push_str(&format!("%{new}"));
                    rest = &rest[j..];
                }
            }
            // 重映射行内其余 %N 引用（操作数/调用参数）
            let mut rebuilt = String::new();
            let bytes = rest.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    let mut j = i + 1;
                    let start = j;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j > start {
                        let n: u32 = rest[start..j].parse().unwrap_or(0);
                        let new = *map.entry(n).or_insert_with(|| {
                            let v = next;
                            next += 1;
                            v
                        });
                        rebuilt.push('%');
                        rebuilt.push_str(&new.to_string());
                        i = j;
                        continue;
                    }
                }
                let ch = rest[i..].chars().next().unwrap_or('\0');
                rebuilt.push(ch);
                i += ch.len_utf8();
            }
            // 无编号指令：非 void 的 call 可编号（变参 call 消耗 2 槽）；
            // void 指令（call void/store/br/ret/unreachable）LLVM 禁止命名，保持无编号
            if !trimmed.starts_with('%') {
                if trimmed.starts_with("call ") && !trimmed.starts_with("call void ") {
                    let vararg = trimmed.contains("(ptr, ...)");
                    let new = next;
                    next += 1;
                    if vararg {
                        next += 1;
                    }
                    result.push_str(&format!("%{new} = {rebuilt}"));
                } else {
                    // void 指令：原样保留（引用已重映射）
                    result.push_str(&rebuilt);
                }
            } else {
                result.push_str(&rebuilt);
            }
            result.push('\n');
        }
        result
    }

    /// 输出一行（带当前缩进）。
    fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// 判断当前基本块是否已终止（最后一条非空指令是 `ret` / `unreachable` / `br`）。
    ///
    /// 用于分支生成：若分支内已 return 或 break/continue（块已终结），不能再追加 `br`
    /// 跳转，否则会在 `ret`/`br` 后生成死代码指令，LLVM 会报「指令编号不连续」错误。
    fn block_terminated(&self) -> bool {
        self.out
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| {
                let t = l.trim();
                t.starts_with("ret ") || t.starts_with("unreachable") || t.starts_with("br ")
            })
            .unwrap_or(false)
    }

    /// 查找 break 的跳转目标：无标签 → 最近循环的 exit；带标签 → 沿循环栈匹配（从内向外）。
    fn find_loop_exit(&self, label: Option<&str>) -> String {
        match label {
            None => self.loop_ctx.last().map(|c| c.2.clone()).unwrap_or_default(),
            Some(l) => self
                .loop_ctx
                .iter()
                .rev()
                .find(|c| c.0.as_deref() == Some(l))
                .map(|c| c.2.clone())
                .unwrap_or_default(),
        }
    }

    /// 查找 continue 的跳转目标：无标签 → 最近循环的 continue 目标；
    /// 带标签 → 沿循环栈匹配（从内向外）。
    fn find_loop_continue(&self, label: Option<&str>) -> String {
        match label {
            None => self.loop_ctx.last().map(|c| c.1.clone()).unwrap_or_default(),
            Some(l) => self
                .loop_ctx
                .iter()
                .rev()
                .find(|c| c.0.as_deref() == Some(l))
                .map(|c| c.1.clone())
                .unwrap_or_default(),
        }
    }

    /// 标记当前块已以 br 终止（供 block_terminated 后续判断；无状态，直接依赖输出缓冲）。
    fn mark_block_terminated(&mut self) {
        // block_terminated 已能通过输出缓冲检测 br/ret/unreachable，本方法仅为语义占位
        let _ = ();
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
            // 动态表：运行时 {ptr,len,cap} 结构，IR 层以不透明指针持有（与字符串一致）。
            // 定长表（字面量）不经过此路径（gen_table_var 直接按数组类型布局）。
            // table<T>（A1）：与裸 table 同（编译期元素类型不影响 LLVM 表示）。
            TypeSpec::Named(TyKw::Table) | TypeSpec::Table(_) => "ptr",
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
            TypeSpec::Struct(class_name) => {
                // struct → 拍平字段结构体：字段类型已在语义层解析为 Some。
                // struct 必然已收集（语义层保证），此处 expect 兜底（与元组字段解析一致）。
                let info = self
                    .sem
                    .classes
                    .get(class_name)
                    .unwrap_or_else(|| panic!("内部错误：struct '{class_name}' 无信息（函数 {}）", self.cur_fn));
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
        TypeSpec::Struct(_) => "struct",
        TypeSpec::Table(_) => "table<T>",
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

/// 命名空间全名 → LLVM 符号名：`::` 不是 LLVM 标识符合法字符，统一转为 `$`
/// （与类方法 mangle `类名$方法名` 同约定；顶层函数名不含 `::`，原样返回）。
fn ns_symbol(full_name: &str) -> String {
    full_name.replace("::", "$")
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

/// table_new_* 内置函数名 → 元素宽度（字节）。与 tie-interp 桥的 elem_size 一致：
/// i64/f64/string=8（指针/64 位），bool=1。
fn table_new_elem_size(name: &str) -> Option<i64> {
    match name {
        "table_new_i64" | "table_new_f64" | "table_new_string" => Some(8),
        "table_new_bool" => Some(1),
        _ => None,
    }
}

/// LLVM 元素类型名 → 动态表桥后缀（tie_table_push_*/tie_table_at_*）。
fn table_elem_suffix(llvm_ty: &str) -> &'static str {
    match llvm_ty {
        "i64" => "i64",
        "double" => "f64",
        "ptr" => "string",
        "i1" => "bool",
        _ => "i64", // 防御：其余标量按 i64 处理（语义层已限制元素类型）
    }
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

    // ---------- E1+E5：break/continue + 标签跳转（IR 生成） ----------

    #[test]
    fn break_continue生成分支跳转() {
        // break：while 体内 br 到 loop.exit（break/continue 是简单语句需分号）
        let ir = 编译("func main() {\n    var i = 0\n    while true {\n        i = i + 1\n        if i == 3 { break; }\n    }\n    println(i)\n}");
        assert!(ir.contains("br label %loop.exit."), "break 应跳 loop.exit");
        // continue：while 体内 br 到 loop.cond（下次条件判断）
        let ir2 = 编译("func main() {\n    var i = 0\n    while i < 10 {\n        i = i + 1\n        if i % 2 == 0 { continue; }\n        println(i)\n    }\n}");
        assert!(ir2.contains("br label %loop.cond."), "continue 应跳 loop.cond");
        // for：continue 跳 for.step（自增块），break 跳 for.exit
        let ir3 = 编译("func main() {\n    for i in 0..10 {\n        if i == 2 { continue; }\n        if i == 8 { break; }\n        println(i)\n    }\n}");
        assert!(ir3.contains("br label %for.step."), "for 的 continue 应跳 for.step");
        assert!(ir3.contains("br label %for.exit."), "for 的 break 应跳 for.exit");
        // for 循环本身生成 step 块（范围遍历）
        let ir4 = 编译("func main() {\n    for i in 0..3 {\n        if i == 1 { continue; }\n        break;\n    }\n}");
        assert!(ir4.contains("for.step."), "范围 for 应有 step 块");
    }

    #[test]
    fn break_continue_标签跳转() {
        // 标签 break：break outer 跳外层 for 的 exit
        let ir = 编译("func main() {\n    outer: for a in 0..3 {\n        for b in 0..3 {\n            if a == 1 && b == 1 { break outer; }\n            println(a * 10 + b)\n        }\n    }\n}");
        // 外层循环 exit 块应存在，且 break outer 的 br 指向它（无法直接断言目标名，
        // 断言外层 for.exit 块与 break 的 br 同时出现；str::matches 是子串匹配非正则）
        assert!(ir.contains("for.exit."), "外层循环应有 exit 块");
        assert!(ir.matches("br label %for.exit.").count() >= 1, "应有 br 到外层 exit");
        // 标签 continue：continue outer 跳外层 for 的 step
        let ir2 = 编译("func main() {\n    var n = 0\n    outer: for a in 0..3 {\n        for b in 0..3 {\n            if b == 1 { continue outer; }\n            n = n + 1\n        }\n    }\n    println(n)\n}");
        assert!(ir2.matches("for.step.").count() >= 2, "内外两层 for 都应有 step 块");
    }

    #[test]
    fn break_continue_循环外报错() {
        // 语义层拦截：循环外 break/continue
        let err = 管道("func main() {\n    break;\n}").unwrap_err();
        assert!(err.contains("break 只能出现在循环体内"), "错误：{err}");
        let err = 管道("func main() {\n    continue;\n}").unwrap_err();
        assert!(err.contains("continue 只能出现在循环体内"), "错误：{err}");
        // 未匹配标签
        let err = 管道("func main() {\n    while true {\n        break nope;\n    }\n}").unwrap_err();
        assert!(err.contains("标签 'nope' 未匹配"), "错误：{err}");
        // switch 分支内 break 合法（switch 不消费，语义层应放行——switch 不是循环，
        // break 在 switch 里仍指向外层循环或报错；当前实现：switch 不 push 循环上下文）
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
    fn 正则内置函数生成桥调用与声明() {
        // regex_match：调用 tie_regex_match（带 ok 标志），收集 used_externs
        let out = 编译_输出("func main() {\n    var m = regex_match(\"abc\", \"a\")\n    println(m)\n}");
        assert!(out.ir.contains("call i8 @tie_regex_match(ptr "));
        assert!(out.ir.contains("declare i8 @tie_regex_match(ptr, ptr, ptr)"));
        assert!(out.used_externs.contains(&"tie_regex_match".to_string()));
        // regex_find：调用 tie_regex_find（返回堆串，模式非法分支 + 成功块）
        let out2 = 编译_输出("func main() {\n    var f = regex_find(\"a1b2\", \"\\\\d\")\n    println(f)\n}");
        assert!(out2.ir.contains("call ptr @tie_regex_find(ptr "));
        assert!(out2.ir.contains("declare ptr @tie_regex_find(ptr, ptr)"));
        assert!(out2.used_externs.contains(&"tie_regex_find".to_string()));
        // regex_find_all：调用 tie_regex_find_all（返回字符串动态表）
        let out3 = 编译_输出("func main() {\n    var t = regex_find_all(\"a1b2\", \"\\\\d\")\n    println(len(t))\n}");
        assert!(out3.ir.contains("call ptr @tie_regex_find_all(ptr "));
        assert!(out3.ir.contains("declare ptr @tie_regex_find_all(ptr, ptr)"));
        assert!(out3.used_externs.contains(&"tie_regex_find_all".to_string()));
        // regex_replace：调用 tie_regex_replace（三个字符串参数）
        let out4 = 编译_输出("func main() {\n    var r = regex_replace(\"a1b2\", \"\\\\d\", \"#\")\n    println(r)\n}");
        assert!(out4.ir.contains("call ptr @tie_regex_replace(ptr "));
        assert!(out4.ir.contains("declare ptr @tie_regex_replace(ptr, ptr, ptr)"));
        assert!(out4.used_externs.contains(&"tie_regex_replace".to_string()));
        // regex_group：调用 tie_regex_group（第三个参数整数，扩展为 i64）
        let out5 = 编译_输出("func main() {\n    var g = regex_group(\"a1\", \"(\\\\d)\", 1)\n    println(g)\n}");
        assert!(out5.ir.contains("call ptr @tie_regex_group(ptr "));
        assert!(out5.ir.contains("declare ptr @tie_regex_group(ptr, ptr, i64)"));
        assert!(out5.used_externs.contains(&"tie_regex_group".to_string()));
        // 作为独立语句时，返回堆串的正则调用立即释放
        let out6 = 编译_输出("func main() {\n    regex_find(\"abc\", \"b\")\n}");
        assert!(out6.ir.contains("call void @tie_free_result(ptr %"));
    }

    #[test]
    fn eval_call生成桥调用与声明() {
        // eval_call：调用 tie_eval_call（两个字符串参数）→ 返回堆串
        let out = 编译_输出("func main() {\n    var r = eval_call(\"process\", \"src\")\n    println(r)\n}");
        assert!(out.ir.contains("call ptr @tie_eval_call(ptr "));
        assert!(out.ir.contains("declare ptr @tie_eval_call(ptr, ptr)"));
        assert!(out.used_externs.contains(&"tie_eval_call".to_string()));
        // 未使用 eval_call 时不应输出声明
        let out2 = 编译_输出("func main() {\n    println(1)\n}");
        assert!(!out2.ir.contains("tie_eval_call"));
        // 作为独立语句时，返回堆串的 eval_call 立即释放
        let out3 = 编译_输出("func main() {\n    eval_call(\"process\", \"src\")\n}");
        assert!(out3.ir.contains("call void @tie_free_result(ptr %"));
    }

    #[test]
    fn file_delete生成remove调用() {
        // file_delete：调用 libc remove(path)，返回 i1 = (返回码 == 0)
        let out = 编译_输出("func main() {\n    var ok = file_delete(\"x.txt\")\n    println(ok)\n}");
        assert!(out.ir.contains("call i32 @remove(ptr "));
        assert!(out.ir.contains("declare i32 @remove(ptr)"));
        // 返回值：icmp eq i32 返回码, 0
        assert!(out.ir.contains("= icmp eq i32 %"));
        // 独立语句调用也正常（无堆串，不需释放）
        let out2 = 编译_输出("func main() {\n    file_delete(\"x.txt\")\n}");
        assert!(out2.ir.contains("call i32 @remove(ptr "));
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
    fn switch生成跳转表() {
        // C5：整数 subject + 全整数常量 case → LLVM switch 跳转表（O(1) 分派）
        let ir = 编译("func main() {\n    var n: i64 = 2\n    switch n {\n        case 1:\n            println(1)\n        case 2:\n            println(2)\n        default:\n            println(0)\n    }\n}");
        // switch 指令 + 跳转表条目（i64 值, label）
        assert!(ir.contains("switch i64 %"), "应生成 switch 指令");
        assert!(ir.contains("i64 1, label %sw.body."), "case 1 跳转条目");
        assert!(ir.contains("i64 2, label %sw.body."), "case 2 跳转条目");
        assert!(ir.contains("sw.default."), "default 标签");
        assert!(ir.contains("sw.exit."), "exit 标签");
        // 不再生成逐 case 比较链（整数常量 case 走跳转表）
        assert!(!ir.contains("sw.cmp."), "整数常量 case 不应生成比较链");
    }

    #[test]
    fn switch守卫走比较链() {
        // 带 when 守卫的 case：不走跳转表（C5 前置检查排除），保留逐 case 比较链
        let ir = 编译("func main() {\n    var n: i64 = 8\n    var flag = true\n    switch n {\n        case 8 when flag:\n            println(8)\n        default:\n            println(0)\n    }\n}");
        assert!(ir.contains("sw.cmp."), "守卫场景应保留比较链");
        assert!(ir.contains("= icmp eq i64"));
        // 值匹配 AND 守卫条件
        assert!(ir.contains("= and i1"));
    }

    #[test]
    fn switch多值生成OR合并() {
        // 多值 `case 1, 2:` → 两个 icmp eq 用 or 合并（多值不走跳转表）
        let ir = 编译("func main() {\n    var n: i64 = 2\n    switch n {\n        case 1, 2:\n            println(12)\n        default:\n            println(0)\n    }\n}");
        assert!(ir.contains("= icmp eq i64"));
        // 两个比较结果 OR 合并（至少一个 or i1）
        assert!(ir.contains("= or i1"));
    }

    #[test]
    fn switch区间生成AND合并() {
        // 区间 `case 3..7:` → sge start 与 slt end 两个比较 AND 合并
        let ir = 编译("func main() {\n    var n: i64 = 5\n    switch n {\n        case 3..7:\n            println(5)\n        default:\n            println(0)\n    }\n}");
        // 区间：sge 3 && slt 7（左闭右开）
        assert!(ir.contains("= icmp sge i64"));
        assert!(ir.contains("= icmp slt i64"));
        // 两个比较 AND 合并
        assert!(ir.contains("= and i1"));
    }

    #[test]
    fn switch守卫生成AND合并() {
        // 守卫 `case 8 when flag:` → 值比较结果 AND 守卫条件
        let ir = 编译("func main() {\n    var n: i64 = 8\n    var flag: bool = true\n    switch n {\n        case 8 when flag:\n            println(8)\n        default:\n            println(0)\n    }\n}");
        // 值比较 + 守卫求值 + and 合并
        assert!(ir.contains("= icmp eq i64"));
        assert!(ir.contains("= and i1"));
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
    fn str_len内置函数生成tie_str_len调用() {
        // str_len：字符串码点数（与 str_char 码点索引对齐），编译为 tie_str_len 桥调用
        let out = 编译_输出("func main() {\n    var n: i64 = str_len(\"你好\")\n}");
        assert!(out.ir.contains("call i64 @tie_str_len(ptr @.str."), "IR 应调用 tie_str_len: {}", out.ir);
        assert!(out.ir.contains("declare i64 @tie_str_len(ptr)"), "应声明 tie_str_len extern");
        assert!(out.used_externs.contains(&"tie_str_len".to_string()), "应记录 used_externs");
    }

    #[test]
    fn struct实例方法转发生成调用() {
        // M2.1.8：方法 = 绑定 struct 名的命名空间函数，p.area() 转发为 Point::area(&p)，
        // 首参按引用（ptr）传递——函数内字段修改反映到调用方。
        let ir = 编译("struct Point {\n    var x: i64\n    var y: i64\n}\nnamespace Point {\n    pub func area(p: Point) -> i64 {\n        return p.x * p.y\n    }\n}\nfunc main() {\n    var p = Point(3, 4)\n    println(p.area())\n}");
        // 方法函数签名：首参是引用（ptr，非结构体值）
        assert!(ir.contains("define i64 @Point$area(ptr %p) {"));
        // 字段访问：方法函数体内按拍平偏移 GEP（x→0，y→1）
        assert!(ir.contains("getelementptr {i64, i64}, ptr %p, i32 0, i32 0"));
        assert!(ir.contains("getelementptr {i64, i64}, ptr %p, i32 0, i32 1"));
        // 构造：insertvalue 链构建结构体值；转发调用：receiver 地址作首实参
        assert!(ir.contains("insertvalue {i64, i64}"));
        assert!(ir.contains("= call i64 @Point$area(ptr %"));
    }

    #[test]
    fn struct名静态调用无接收者() {
        // M2.1.8：Point.create(...)（receiver 是 struct 名）→ 命名空间函数，无接收者实参。
        let ir = 编译("struct Point {\n    var x: i64\n    var y: i64\n}\nnamespace Point {\n    pub func create(x: i64, y: i64) -> i64 {\n        return x + y\n    }\n}\nfunc main() {\n    println(Point.create(1, 2))\n}");
        // 签名与普通函数一致：无接收者首参
        assert!(ir.contains("define i64 @Point$create(i64 %x, i64 %y) {"));
        assert!(!ir.contains("Point$create({"));
        // struct 名调用 → 无接收者实参
        assert!(ir.contains("call i64 @Point$create(i64 1, i64 2)"));
    }

    #[test]
    fn struct构造与字段赋值() {
        let ir = 编译("struct Point {\n    var x: i64\n    var y: i64\n}\nfunc main() {\n    var p = Point(3, 4)\n    p.x = 100\n    println(p.x)\n}");
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

    // ---------- F1：alloca 提升 + 全局重编号 ----------

    #[test]
    fn renumber_ir_全局重编号() {
        // alloca 提升场景：entry 块 %54 = alloca 在函数体 %1 = call 之前（编号倒挂）
        let src = "define void @main() {\nentry:\n  %54 = alloca i1\n  %1 = call ptr @tie_table_new(i64 8)\n  store ptr %1, ptr %54\n  br label %loop.cond.2\nloop.cond.2:\n  %19 = load i64, ptr %54\n  %2 = icmp slt i64 %19, 100\n  br i1 %2, label %loop.body.3, label %loop.exit.4\nloop.body.3:\n  %55 = load i64, ptr %54\n  br label %loop.cond.2\nloop.exit.4:\n  ret void\n}";
        let re = super::IrGenerator::renumber_ir(src);
        // 定义行（`%N = `）编号必须严格递增；引用可重复出现（同一寄存器多次使用合法）
        let mut prev = 0u32;
        for line in re.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix('%') {
                let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !num.is_empty() && rest[num.len()..].starts_with(" = ") {
                    let n: u32 = num.parse().unwrap();
                    assert!(n > prev, "定义编号未递增: {n} <= {prev}\n{re}");
                    prev = n;
                }
            }
        }
        // 所有寄存器引用（含提升的 alloca）都保留
        assert!(re.contains("= alloca i1"), "应保留 alloca 定义");
        assert!(re.contains("@tie_table_new"), "函数调用保留");
    }

    #[test]
    fn renumber_ir_无编号指令分配编号() {
        // 无编号指令（call/store/br）与变参 call 混合：行级处理应分配递增编号
        let src = "define void @main() {\nentry:\n  %1 = alloca i64\n  store i64 0, ptr %1\n  br label %l.2\nl.2:\n  %2 = load i64, ptr %1\n  call i32 (ptr, ...) @printf(ptr null, i64 %2)\n  call void @exit(i32 1)\n  %3 = add i64 %2, 1\n  store i64 %3, ptr %1\n  br label %l.2\n}";
        let re = super::IrGenerator::renumber_ir(src);
        // 非 void 指令（alloca/load/call/add）获得递增编号；void 指令（store/br）无编号
        let mut prev = 0u32;
        let mut cmd_count = 0;
        for line in re.lines() {
            let t = line.trim();
            if t.starts_with('%') {
                let num: String = t[1..].chars().take_while(|c| c.is_ascii_digit()).collect();
                if !num.is_empty() && t[1 + num.len()..].starts_with(" = ") {
                    let n: u32 = num.parse().unwrap();
                    assert!(n > prev, "编号未递增: {n} <= {prev}\n{re}");
                    prev = n;
                    cmd_count += 1;
                }
            }
        }
        // 非 void 指令：alloca/load/printf/add = 4 条（变参 call 占 2 槽编号到 5+）
        assert!(cmd_count >= 4, "应有 ≥4 条编号指令，实际 {cmd_count}\n{re}");
        assert!(re.contains("call i32 (ptr, ...) @printf"), "变参 call 保留");
        // void 指令保持无编号（LLVM 禁止命名）
        assert!(re.contains("store i64 0, ptr %1"), "store 应保持无编号");
        assert!(re.contains("br label %l.2"), "br 应保持无编号");
    }
}
