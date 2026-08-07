//! tie 语言解释器：树遍历求值 AST，内置标准库。
//!
//! 设计职责：直接解释执行 AST（REPL 交互模式与脚本执行），
//! 与编译路径（tie-llvm）共享 tie-frontend 前端产物（词法/语法）。
//!
//! 架构（REPL 自举）：
//! - 纯**动态**求值：不复用语义分析（`analyze`），Value 自带类型，运行时检查；
//!   —— REPL 场景 `func f(){}` 后再 `f()`、`var x=1` 后 `x+1` 都依赖持久 Session，
//!     静态 analyze 只认单次程序内的函数表，无法满足。
//! - **两趟解析**：先 parse_program 原样解析（顶层 func 定义 → 注册进 Session）；
//!   失败则包装 `func main() { <code> }` 再解析（表达式/语句统一）；
//! - **Session 持久化**：globals + funcs 跨多次 eval 保留（REPL 连续输入的基础）；
//! - C ABI 导出（供编译后的 repl.exe 通过 staticlib 调用）：
//!   [tie_eval_expr]、[tie_read_line]、[tie_free_result]，均 catch_unwind 包裹
//!   （panic 跨 extern "C" 是 UB），Session 用 thread_local RefCell（递归 eval
//!   时 Mutex 会重入死锁）。
//!
//! 说明：本 crate 覆盖 workspace 的 `unsafe_code = "forbid"`（见 Cargo.toml），
//! 因为 C ABI 导出需要解引用裸指针——这是唯一允许 unsafe 的 crate。

use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

use tie_frontend::ast::{Expr, FnDefStmt, Stmt};
use tie_frontend::lexer::tokenize;
use tie_frontend::parser::parse_program;

/// C ABI 桥：求值一段 tie 代码，返回格式化结果串或错误串（调用方负责 [tie_free_result]）。
///
/// 顶层定义（func）注册进 Session；其余代码包装成 `func main() { ... }` 后
/// 按函数体求值，返回最后一条表达式语句的值。
#[unsafe(no_mangle)]
pub extern "C" fn tie_eval_expr(code: *const c_char) -> *mut c_char {
    c_guard(|| {
        let code = unsafe { c_char_to_string(code)? };
        with_session(|session| session.eval(&code))
    })
}

/// C ABI 桥：从 stdin 读取一行（去除行尾换行），返回新分配的字符串。
///
/// EOF（Ctrl+Z / Ctrl+D）时直接退出进程（与 Rust REPL 行为一致，规避空串歧义）；
/// 读取前先刷新 stdout（保证编译版 `print("> ")` 提示符先显示，Windows 控制台有缓冲）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_read_line() -> *mut c_char {
    c_guard(|| {
        use std::io::Write;
        // 刷新 stdout：提示符 print("> ") 不换行，不刷可能不显示
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => std::process::exit(0), // EOF → 退出（与现 Rust repl() 行为一致）
            Ok(_) => {}
            Err(e) => return Err(format!("读取输入失败: {e}")),
        }
        // 去除行尾 \r\n（Windows 控制台输入会带 \r）
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    })
}

/// C ABI 桥：释放 [tie_eval_expr] / [tie_read_line] 返回的字符串。
#[unsafe(no_mangle)]
pub extern "C" fn tie_free_result(p: *mut c_char) {
    if !p.is_null() {
        // CString::from_raw 回收由 into_raw 移交的堆内存
        unsafe {
            drop(CString::from_raw(p));
        }
    }
}

// ---------- M2 标准库 floor 内置函数 C ABI 桥 ----------
//
// 设计说明：以下桥函数是「Rust 层唯一实现」的 9 个 floor 原语中返回字符串/需解析的
// 那部分。编译路径（IR 层）与解释路径（tie-interp）**共用同一份 Rust 实现**，
// 保证两路径行为逐字节一致（这是 M2 标准库正确性的关键——其余 std 库用 tie 语言自写）。
//
// 返回字符串的桥（file_read / str_char / to_string）沿用 read_line 的堆串模式：
// 返回 `CString::into_raw` 分配的 `*mut c_char`，调用方用完必须 `tie_free_result` 释放。
// 失败（file_read 读不到文件）返回 NULL，由调用方统一输出错误消息（两路径消息一致）。

/// C ABI 桥：读取文件全部内容，返回新分配的字符串；失败返回 NULL。
///
/// 失败时返回 NULL（而非错误串），由调用方（IR 层 / 解释器）统一输出错误消息，
/// 保证编译与解释两路径的错误文本一致。
#[unsafe(no_mangle)]
pub extern "C" fn tie_file_read(path: *const c_char) -> *mut c_char {
    let path = match unsafe { c_char_to_string(path) } {
        Ok(p) => p,
        Err(_) => return std::ptr::null_mut(),
    };
    match std::fs::read_to_string(&path) {
        Ok(contents) => string_to_c_char(contents),
        Err(_) => std::ptr::null_mut(),
    }
}

/// C ABI 桥：取字符串第 i 个 Unicode 码点（按字符计数，非字节），返回新分配的字符串；
/// 越界（含负数下标）返回空串。
// 用 Rust `chars().nth(i)` 解码 UTF-8 码点，天然支持多字节字符（如中文）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_str_char(s: *const c_char, i: i64) -> *mut c_char {
    let s = unsafe { c_char_to_string(s).unwrap_or_default() };
    let ch = if i < 0 { None } else { s.chars().nth(i as usize) };
    string_to_c_char(ch.map(|c| c.to_string()).unwrap_or_default())
}

/// C ABI 桥：i64 → 十进制字符串（与 Rust `{}` 默认格式一致）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_to_string_i64(v: i64) -> *mut c_char {
    string_to_c_char(v.to_string())
}

/// C ABI 桥：f64 → 字符串（与 Rust `{}` 默认格式一致，最短往返表示）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_to_string_f64(v: f64) -> *mut c_char {
    string_to_c_char(v.to_string())
}

/// C ABI 桥：解析 i64。成功置 `ok=1` 并返回解析值；失败置 `ok=0` 返回 0。
// 解析语义与 Rust `str::parse::<i64>` 完全一致（编译/解释两路径共用，保证一致）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_parse_int(s: *const c_char, ok: *mut i8) -> i64 {
    let parsed = match unsafe { c_char_to_string(s) } {
        Ok(s) => s.parse::<i64>().ok(),
        Err(_) => None,
    };
    match parsed {
        Some(v) => {
            unsafe { *ok = 1; }
            v
        }
        None => {
            unsafe { *ok = 0; }
            0
        }
    }
}

/// C ABI 桥：解析 f64。成功置 `ok=1` 并返回解析值；失败置 `ok=0` 返回 0.0。
// 解析语义与 Rust `str::parse::<f64>` 完全一致。
#[unsafe(no_mangle)]
pub extern "C" fn tie_parse_float(s: *const c_char, ok: *mut i8) -> f64 {
    let parsed = match unsafe { c_char_to_string(s) } {
        Ok(s) => s.parse::<f64>().ok(),
        Err(_) => None,
    };
    match parsed {
        Some(v) => {
            unsafe { *ok = 1; }
            v
        }
        None => {
            unsafe { *ok = 0; }
            0.0
        }
    }
}

/// C ABI 桥：返回 Unix 纪元秒数（i64）。
///
/// 编译路径（IR 层）与解释路径共用本桥（SystemTime），保证两路径返回一致的时间戳。
#[unsafe(no_mangle)]
pub extern "C" fn tie_time_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

// 线程局部 xorshift 随机数状态（RNG 状态）。
//
// 首次调用时用 SystemTime 播种（保证两次运行产生不同序列）；
// 之后每次调用做 xorshift64 变换。xorshift 简单快速，足够随机数用途。
thread_local! {
    static RNG_STATE: std::cell::Cell<u64> = std::cell::Cell::new(seed_rng());
}

/// 从当前时间（纳秒）初始化 RNG 种子；种子为 0 时置 1（xorshift 全 0 会卡死）。
fn seed_rng() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    if t == 0 { 1 } else { t }
}

/// 生成下一个随机 u64（xorshift64）。
fn next_rand() -> u64 {
    RNG_STATE.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        x
    })
}

/// C ABI 桥：返回 [min, max) 内的随机整数。
///
/// 成功置 `ok=1` 并返回 [min, max) 内的值；`max <= min` 时置 `ok=0` 返回 0
/// （由调用方据此输出错误消息，两路径文本一致）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_rand_range(min: i64, max: i64, ok: *mut i8) -> i64 {
    if max <= min {
        unsafe { *ok = 0; }
        return 0;
    }
    unsafe { *ok = 1; }
    // 取模映射到 [0, max-min)，再加 min → [min, max)
    let span = (max - min) as u64;
    min + (next_rand() % span) as i64
}

/// 把 C 字符串（NUL 结尾）读为 Rust String。
unsafe fn c_char_to_string(p: *const c_char) -> Result<String, String> {
    if p.is_null() {
        return Err("内部错误: 收到空指针".into());
    }
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .map(|s| s.to_string())
        .map_err(|e| format!("内部错误: 输入不是合法 UTF-8: {e}"))
}

/// 把 Rust String 转为 C 字符串（堆分配，调用方负责释放）。
fn string_to_c_char(s: String) -> *mut c_char {
    // 注：String 无内部 NUL，CString::new 不会失败
    CString::new(s).unwrap_or_default().into_raw()
}

/// 包一层 catch_unwind：解释器内部 panic（如除零）不得穿过 extern "C"（UB）。
fn c_guard<F>(f: F) -> *mut c_char
where
    F: FnOnce() -> Result<String, String>,
{
    let result = catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err("内部错误: 解释器崩溃".into()));
    string_to_c_char(result.unwrap_or_else(|e| e))
}

/// 解释器会话：REPL 持久作用域（globals + funcs），thread_local 单线程持有。
///
/// 用闭包注入而非返回引用：thread_local! 的 with 拿不到 'static 引用，
/// 改为「传入闭包、在 thread_local 上下文中执行」——借用生命周期在闭包内闭合。
/// 选 thread_local 而非 Mutex：递归 eval（eval 内再调 eval）在同一线程重入，
/// Mutex 会死锁；RefCell 单线程重入安全（borrow 顺序内层先归还）。
fn with_session<F, T>(f: F) -> T
where
    F: FnOnce(&mut Session) -> T,
{
    thread_local! {
        static SESSION: std::cell::RefCell<Session> = std::cell::RefCell::new(Session::new());
    }
    SESSION.with(|s| f(&mut s.borrow_mut()))
}

/// 解释器会话：跨多次 eval 持久保存的顶层作用域。
#[derive(Default)]
pub struct Session {
    /// 顶层变量（REPL 中 `var x = 1` 后 `x + 1` 依赖）
    globals: std::collections::HashMap<String, Value>,
    /// 顶层函数（REPL 中 `func f() {}` 后 `f()` 依赖）
    funcs: std::collections::HashMap<String, FnDefStmt>,
}

impl Session {
    /// 新建空会话。
    pub fn new() -> Self {
        Self::default()
    }

    /// 求值一段代码：顶层定义注册，否则包装为 main 体执行，返回格式化结果或错误。
    pub fn eval(&mut self, code: &str) -> Result<String, String> {
        // 第一趟：原样解析（顶层定义）
        let tokens = tokenize(code).map_err(|e| e.to_string())?;
        if let Ok(program) = parse_program(&tokens) {
            if !program.stmts.is_empty() {
                return self.register_top_level(program);
            }
        }
        // 第二趟：包装成 `func main() { ... }`（ASI 在换行处补分号）
        let wrapped = format!("func main() {{\n{code}\n}}");
        let tokens = tokenize(&wrapped).map_err(|e| e.to_string())?;
        let program = parse_program(&tokens).map_err(|e| e.to_string())?;
        // 取 main 的函数体执行（动态求值，不跑 analyze）
        let main = program
            .stmts
            .iter()
            .find_map(|s| match s {
                Stmt::FnDef(f) if f.name == "main" => Some(f),
                _ => None,
            })
            .ok_or_else(|| "内部错误: 包装后的 main 未找到".to_string())?;
        let mut env = Env::new(self);
        // REPL 顶层：直接遍历 main 体语句执行（不压作用域），
        // 使顶层 `var` 声明落入 globals、跨行持久；嵌套块自己压作用域。
        let mut last = None;
        for stmt in &main.body {
            match env.exec_stmt(stmt)? {
                Flow::Normal(v) => last = v,
                flow @ Flow::Return(_) => {
                    // return 传播到顶层（用户输入 `return 5`）：取其值
                    return Ok(match flow {
                        Flow::Return(v) => v.to_repl_string(),
                        _ => unreachable!(),
                    });
                }
            }
        }
        // 正常结束：返回最后表达式语句的值（void/纯声明 → 空串）
        Ok(match last {
            Some(v) => v.to_repl_string(),
            None => String::new(),
        })
    }

    /// 注册顶层定义（func → funcs；class/import → v1 暂不支持）。
    fn register_top_level(&mut self, program: tie_frontend::ast::Program) -> Result<String, String> {
        let mut count = 0;
        for stmt in &program.stmts {
            match stmt {
                Stmt::FnDef(f) => {
                    self.funcs.insert(f.name.clone(), f.clone());
                    count += 1;
                }
                Stmt::Class(_) => return Err("REPL v1 暂不支持类定义".into()),
                Stmt::Import(_) => return Err("REPL v1 暂不支持 import".into()),
                _ => return Err("顶层只允许函数/类/import 定义".into()),
            }
        }
        Ok(format!("已定义 {count} 个函数"))
    }
}

/// 语句执行的控制流结果：正常（可能带最后表达式值）或 return。
enum Flow {
    /// 正常结束：Option 是最后一条表达式语句的值
    Normal(Option<Value>),
    /// return 语句：携带返回值
    Return(Value),
}

/// 求值环境：作用域栈 + 指向 Session 的借用。
struct Env<'a> {
    session: &'a mut Session,
    scopes: Vec<std::collections::HashMap<String, Value>>,
}

impl<'a> Env<'a> {
    fn new(session: &'a mut Session) -> Self {
        Self { session, scopes: Vec::new() }
    }

    /// 变量查找：作用域栈 → 顶层 globals。
    fn lookup(&self, name: &str) -> Option<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v.clone());
            }
        }
        self.session.globals.get(name).cloned()
    }

    /// 变量赋值：作用域栈内找到则改，否则写顶层 globals。
    fn assign(&mut self, name: &str, value: Value) -> Result<(), String> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return Ok(());
            }
        }
        if self.session.globals.contains_key(name) {
            self.session.globals.insert(name.to_string(), value);
            return Ok(());
        }
        Err(format!("变量 '{name}' 未声明"))
    }

    /// 变量是否已声明（作用域栈或顶层）。
    fn is_declared(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains_key(name)) || self.session.globals.contains_key(name)
    }

    /// 执行一条语句。
    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Flow, String> {
        match stmt {
            Stmt::VarDecl(v) => {
                let val = self.eval_expr(&v.init)?;
                if self.is_declared(&v.name) {
                    return Err(format!("变量 '{}' 重复声明", v.name));
                }
                // REPL 顶层（作用域栈空）声明 → globals，跨行持久；
                // 嵌套块/函数体内的声明 → 当前作用域
                match self.scopes.last_mut() {
                    Some(scope) => {
                        scope.insert(v.name.clone(), val);
                    }
                    None => {
                        self.session.globals.insert(v.name.clone(), val);
                    }
                }
                Ok(Flow::Normal(None))
            }
            Stmt::Expr(e) => {
                let val = self.eval_expr(&e.expr)?;
                Ok(Flow::Normal(Some(val)))
            }
            Stmt::Assign(a) => {
                // 复合赋值（M4）：load 目标当前值 → 与右值做二元运算 → 写回
                match a.op {
                    // 普通赋值：直接求右值并写回
                    None => {
                        let val = self.eval_expr(&a.value)?;
                        self.assign(&a.target, val)?;
                    }
                    // 复合赋值（+= -= *= /= %= &= |= ^= <<= >>=）：
                    // 目标必须已声明（语义/运行时均校验）；BinaryOp 是 Copy，直接取出
                    Some(op) => {
                        let cur = self
                            .lookup(&a.target)
                            .ok_or_else(|| format!("变量 '{}' 未声明", a.target))?;
                        let rv = self.eval_expr(&a.value)?;
                        let val = self.eval_binary(op, cur, rv)?;
                        self.assign(&a.target, val)?;
                    }
                }
                Ok(Flow::Normal(None))
            }
            Stmt::Return(r) => {
                let val = match &r.expr {
                    Some(e) => self.eval_expr(e)?,
                    None => Value::Void,
                };
                Ok(Flow::Return(val))
            }
            Stmt::If(i) => {
                let cond = self.eval_expr(&i.cond)?;
                if cond.is_truthy()? {
                    self.exec_block(&i.then_branch)
                } else {
                    self.exec_block(&i.else_branch)
                }
            }
            Stmt::While(w) => {
                let mut last = None;
                while self.eval_expr(&w.cond)?.is_truthy()? {
                    match self.exec_block(&w.body)? {
                        Flow::Normal(v) => last = v,
                        flow @ Flow::Return(_) => return Ok(flow),
                    }
                }
                Ok(Flow::Normal(last))
            }
            Stmt::For(f) => {
                let iter = self.eval_expr(&f.iter)?;
                let mut last = None;
                match iter {
                    Value::Range(start, end) => {
                        for i in start..end {
                            self.scopes.push(std::collections::HashMap::new());
                            self.scopes.last_mut().unwrap().insert(f.var.clone(), Value::Int(i));
                            let flow = self.exec_block(&f.body);
                            self.scopes.pop();
                            match flow? {
                                Flow::Normal(v) => last = v,
                                flow @ Flow::Return(_) => return Ok(flow),
                            }
                        }
                    }
                    _ => return Err("for 的迭代对象必须是范围（0..10）".into()),
                }
                Ok(Flow::Normal(last))
            }
            Stmt::Switch(_) => Err("REPL v1 暂不支持 switch".into()),
            Stmt::Import(_) => Err("REPL v1 暂不支持 import".into()),
            Stmt::Class(_) => Err("REPL v1 暂不支持类定义".into()),
            Stmt::FnDef(f) => {
                // 函数体内的嵌套函数定义 → 注册进 funcs（从简）
                self.session.funcs.insert(f.name.clone(), f.clone());
                Ok(Flow::Normal(None))
            }
            Stmt::FieldAssign(_) => Err("REPL v1 暂不支持字段赋值（类）".into()),
        }
    }

    /// 执行语句块（压一层作用域），return 向上传播。
    fn exec_block(&mut self, body: &[Stmt]) -> Result<Flow, String> {
        self.scopes.push(std::collections::HashMap::new());
        let mut last = None;
        for stmt in body {
            match self.exec_stmt(stmt)? {
                Flow::Normal(v) => last = v,
                flow @ Flow::Return(_) => {
                    self.scopes.pop();
                    return Ok(flow);
                }
            }
        }
        self.scopes.pop();
        Ok(Flow::Normal(last))
    }

    /// 求值表达式。
    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::IntLit(v) => Ok(Value::Int(*v)),
            Expr::FloatLit(v) => Ok(Value::Float(*v)),
            Expr::StrLit(s) => Ok(Value::Str(s.clone())),
            Expr::CharLit(c) => Ok(Value::Char(*c)),
            Expr::BoolLit(b) => Ok(Value::Bool(*b)),
            Expr::Var(name) => self
                .lookup(name)
                .ok_or_else(|| format!("变量 '{name}' 未声明")),
            Expr::Unary { op, operand, .. } => {
                // 自增/自减（M4）：操作数必须是变量（语义层保证），
                // 需可写访问（load 当前值 → ±1 → assign 写回），先走专门路径
                if matches!(
                    op,
                    tie_frontend::ast::UnaryOp::PreInc
                        | tie_frontend::ast::UnaryOp::PreDec
                        | tie_frontend::ast::UnaryOp::PostInc
                        | tie_frontend::ast::UnaryOp::PostDec
                ) {
                    return self.eval_inc_dec(*op, operand);
                }
                let v = self.eval_expr(operand)?;
                match op {
                    tie_frontend::ast::UnaryOp::Neg => match v {
                        Value::Int(n) => Ok(Value::Int(-n)),
                        Value::Float(n) => Ok(Value::Float(-n)),
                        _ => Err("一元负号只能作用于数字".into()),
                    },
                    tie_frontend::ast::UnaryOp::Not => match v {
                        Value::Bool(b) => Ok(Value::Bool(!b)),
                        _ => Err("逻辑非只能作用于布尔".into()),
                    },
                    // 自增/自减已在上方 eval_inc_dec 提前返回，此处不可达
                    tie_frontend::ast::UnaryOp::PreInc
                    | tie_frontend::ast::UnaryOp::PreDec
                    | tie_frontend::ast::UnaryOp::PostInc
                    | tie_frontend::ast::UnaryOp::PostDec => {
                        unreachable!("自增/自减已在 eval_inc_dec 中处理")
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let l = self.eval_expr(lhs)?;
                let r = self.eval_expr(rhs)?;
                self.eval_binary(*op, l, r)
            }
            Expr::Ternary { cond, then_expr, else_expr, .. } => {
                // 三目运算 `cond ? then : else`（M4）：短路求值——
                // 先求条件，真则求 then，假则求 else（只求所选分支）。
                // 条件判断与 if 语句一致（is_truthy：必须布尔）。
                let c = self.eval_expr(cond)?;
                if c.is_truthy()? {
                    self.eval_expr(then_expr)
                } else {
                    self.eval_expr(else_expr)
                }
            }
            Expr::Range { start, end, .. } => {
                let s = match self.eval_expr(start)? {
                    Value::Int(n) => n,
                    _ => return Err("范围起点必须是整数".into()),
                };
                let e = match self.eval_expr(end)? {
                    Value::Int(n) => n,
                    _ => return Err("范围终点必须是整数".into()),
                };
                Ok(Value::Range(s, e))
            }
            Expr::Call { name, args, .. } => {
                let arg_vals = self.eval_args(args)?;
                self.call_fn(name, arg_vals)
            }
            Expr::Index { .. } => Err("REPL v1 暂不支持下标访问（表）".into()),
            Expr::TableLit { cells, .. } => {
                // 表字面量：逐元素求值（M2 单行纯位置表；len 用）
                let mut vals = Vec::with_capacity(cells.len());
                for cell in cells {
                    vals.push(self.eval_expr(&cell.value)?);
                }
                Ok(Value::Table(vals))
            }
            Expr::TupleLit { .. } => Err("REPL v1 暂不支持元组".into()),
            Expr::FieldAccess { .. } => Err("REPL v1 暂不支持字段访问（类/元组）".into()),
            Expr::MethodCall { .. } => Err("REPL v1 暂不支持方法调用（类）".into()),
        }
    }

    /// 求值实参列表。
    fn eval_args(&mut self, args: &[Expr]) -> Result<Vec<Value>, String> {
        args.iter().map(|a| self.eval_expr(a)).collect()
    }

    /// 自增/自减（M4）：`++x` / `--x` / `x++` / `x--`。
    ///
    /// 操作数必须是变量（语义层保证），流程：load 当前值 → ±1 → assign 写回；
    /// 前缀（++x/--x）返回**新值**，后缀（x++/x--）返回**旧值**。
    /// 整数与浮点都支持（±1 / ±1.0）。
    fn eval_inc_dec(
        &mut self,
        op: tie_frontend::ast::UnaryOp,
        operand: &Expr,
    ) -> Result<Value, String> {
        let Expr::Var(name) = operand else {
            return Err("自增/自减只支持变量".into());
        };
        // 取当前值（变量必须已声明）
        let cur = self
            .lookup(name)
            .ok_or_else(|| format!("变量 '{name}' 未声明"))?;
        // 按当前值类型计算新值：整数 ±1，浮点 ±1.0
        let new = match &cur {
            Value::Int(n) => {
                let d = if matches!(op, tie_frontend::ast::UnaryOp::PreInc | tie_frontend::ast::UnaryOp::PostInc) {
                    1
                } else {
                    -1
                };
                Value::Int(n + d)
            }
            Value::Float(n) => {
                let d = if matches!(op, tie_frontend::ast::UnaryOp::PreInc | tie_frontend::ast::UnaryOp::PostInc) {
                    1.0
                } else {
                    -1.0
                };
                Value::Float(n + d)
            }
            _ => return Err("自增/自减只能作用于数字".into()),
        };
        // 写回变量
        self.assign(name, new.clone())?;
        // 前缀返回新值，后缀返回旧值
        Ok(if matches!(op, tie_frontend::ast::UnaryOp::PreInc | tie_frontend::ast::UnaryOp::PreDec) {
            new
        } else {
            cur
        })
    }

    /// 二元运算求值（动态类型检查）。
    fn eval_binary(
        &mut self,
        op: tie_frontend::ast::BinaryOp,
        l: Value,
        r: Value,
    ) -> Result<Value, String> {
        use tie_frontend::ast::BinaryOp;
        // 字符串 + 字符串 → 拼接（编译路径语义）
        if op == BinaryOp::Add {
            if let (Value::Str(a), Value::Str(b)) = (&l, &r) {
                return Ok(Value::Str(format!("{a}{b}")));
            }
        }
        // 字符串比较：按 strcmp 语义（编译路径用 strcmp）
        if matches!(
            op,
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge
        ) {
            if let (Value::Str(a), Value::Str(b)) = (&l, &r) {
                let ord = a.cmp(b);
                return Ok(Value::Bool(match op {
                    BinaryOp::Eq => ord == std::cmp::Ordering::Equal,
                    BinaryOp::NotEq => ord != std::cmp::Ordering::Equal,
                    BinaryOp::Lt => ord == std::cmp::Ordering::Less,
                    BinaryOp::Gt => ord == std::cmp::Ordering::Greater,
                    BinaryOp::Le => ord != std::cmp::Ordering::Greater,
                    BinaryOp::Ge => ord != std::cmp::Ordering::Less,
                    _ => unreachable!(),
                }));
            }
        }
        // 数字运算（克隆以保留 l/r 供错误分支取类型名；Value 非 Copy）
        match (l.clone(), r.clone()) {
            (Value::Int(a), Value::Int(b)) => Ok(match op {
                BinaryOp::Add => Value::Int(a + b),
                BinaryOp::Sub => Value::Int(a - b),
                BinaryOp::Mul => Value::Int(a * b),
                BinaryOp::Div => {
                    if b == 0 {
                        return Err("除零错误".into());
                    }
                    Value::Int(a / b)
                }
                BinaryOp::Mod => {
                    if b == 0 {
                        return Err("除零错误（取模）".into());
                    }
                    Value::Int(a % b)
                }
                BinaryOp::Eq => Value::Bool(a == b),
                BinaryOp::NotEq => Value::Bool(a != b),
                BinaryOp::Lt => Value::Bool(a < b),
                BinaryOp::Gt => Value::Bool(a > b),
                BinaryOp::Le => Value::Bool(a <= b),
                BinaryOp::Ge => Value::Bool(a >= b),
                BinaryOp::And | BinaryOp::Or => {
                    return Err("整数不能做逻辑运算（需布尔）".into());
                }
                // M4 位运算（仅整数）：按位与/或/异或
                BinaryOp::BitAnd => Value::Int(a & b),
                BinaryOp::BitOr => Value::Int(a | b),
                BinaryOp::BitXor => Value::Int(a ^ b),
                // M4 移位：Rust 移位量是 u32 且越界会 panic，
                // 保守限制移位量必须在 0..64 范围内（i64 位宽）
                BinaryOp::Shl => {
                    if !(0..64).contains(&b) {
                        return Err("左移量必须在 0..64 范围内".into());
                    }
                    Value::Int(a << b as u32)
                }
                BinaryOp::Shr => {
                    if !(0..64).contains(&b) {
                        return Err("右移量必须在 0..64 范围内".into());
                    }
                    Value::Int(a >> b as u32)
                }
            }),
            (Value::Float(a), Value::Float(b)) => match op {
                BinaryOp::Add => Ok(Value::Float(a + b)),
                BinaryOp::Sub => Ok(Value::Float(a - b)),
                BinaryOp::Mul => Ok(Value::Float(a * b)),
                BinaryOp::Div => Ok(Value::Float(a / b)),
                BinaryOp::Mod => Ok(Value::Float(a % b)),
                BinaryOp::Eq => Ok(Value::Bool(a == b)),
                BinaryOp::NotEq => Ok(Value::Bool(a != b)),
                BinaryOp::Lt => Ok(Value::Bool(a < b)),
                BinaryOp::Gt => Ok(Value::Bool(a > b)),
                BinaryOp::Le => Ok(Value::Bool(a <= b)),
                BinaryOp::Ge => Ok(Value::Bool(a >= b)),
                BinaryOp::And | BinaryOp::Or => Err("浮点数不能做逻辑运算（需布尔）".into()),
                // M4 位运算/移位只支持整数（语义层已拦，此处防御）
                BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl
                | BinaryOp::Shr => Err("位运算/移位只支持整数".into()),
            },
            (Value::Bool(a), Value::Bool(b)) => match op {
                BinaryOp::And => Ok(Value::Bool(a && b)),
                BinaryOp::Or => Ok(Value::Bool(a || b)),
                BinaryOp::Eq => Ok(Value::Bool(a == b)),
                BinaryOp::NotEq => Ok(Value::Bool(a != b)),
                // M4 位运算/移位/算术对布尔不合法（明确报错，与其余运算符一致）
                BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl
                | BinaryOp::Shr => Err("位运算/移位只支持整数".into()),
                _ => Err("布尔只能做逻辑运算与相等比较".into()),
            },
            _ => Err(format!(
                "类型不匹配: {} {} {}",
                l.type_name(),
                op_display(op),
                r.type_name()
            )),
        }
    }

    /// 函数调用：内置函数 → 用户函数 → 报错。
    fn call_fn(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        match name {
            "println" => {
                let line = args.iter().map(|v| v.to_print_string()).collect::<Vec<_>>().join("");
                println!("{line}");
                Ok(Value::Void)
            }
            "print" => {
                use std::io::Write;
                let line = args.iter().map(|v| v.to_print_string()).collect::<Vec<_>>().join("");
                let _ = std::io::stdout().write_all(line.as_bytes());
                let _ = std::io::stdout().flush();
                Ok(Value::Void)
            }
            "len" => {
                if args.len() != 1 {
                    return Err("len 需要一个参数".into());
                }
                match &args[0] {
                    Value::Str(s) => Ok(Value::Int(s.len() as i64)),
                    Value::Table(cells) => Ok(Value::Int(cells.len() as i64)),
                    _ => Err("len 只支持字符串或表".into()),
                }
            }
            "read_line" => {
                // 通过 C ABI 读一行（与编译路径一致：含 stdout 刷新与 EOF 退出）
                let p = tie_read_line();
                // 不安全：read_line 保证返回合法 NUL 结尾 UTF-8 字符串
                let s = unsafe { c_char_to_string(p).unwrap_or_default() };
                tie_free_result(p);
                Ok(Value::Str(s))
            }
            "eval" => {
                if args.len() != 1 {
                    return Err("eval 需要一个字符串参数".into());
                }
                match &args[0] {
                    Value::Str(code) => {
                        let result = self.session.eval(code)?;
                        Ok(Value::Str(result))
                    }
                    _ => Err("eval 需要一个字符串参数".into()),
                }
            }
            // ---------- M2 标准库 floor 内置函数 ----------
            //
            // 文件类：file_read 走 C ABI 桥（与编译路径共用同一份 Rust 实现），
            // file_write/file_append/file_exists 直接用 Rust std::fs（编译路径用 libc，
            // 两者行为一致：写成功/追加增长/存在性检查）。
            "file_read" => {
                if args.len() != 1 {
                    return Err("file_read 需要一个字符串参数".into());
                }
                let Value::Str(path) = &args[0] else {
                    return Err("file_read 需要一个字符串参数".into());
                };
                let p = CString::new(path.as_str()).unwrap_or_default();
                let r = tie_file_read(p.as_ptr());
                // 失败（NULL）→ 报错（错误消息与编译路径 printf 文本一致）
                if r.is_null() {
                    return Err(format!("运行时错误: file_read 无法读取文件 '{path}'"));
                }
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "file_write" | "file_append" => {
                if args.len() != 2 {
                    return Err(format!("{name} 需要两个字符串参数"));
                }
                let (Value::Str(path), Value::Str(content)) = (&args[0], &args[1]) else {
                    return Err(format!("{name} 需要两个字符串参数"));
                };
                let ok = if name == "file_write" {
                    // 覆盖写（create + truncate）
                    std::fs::write(path, content).is_ok()
                } else {
                    // 追加写（不存在则创建）
                    use std::io::Write;
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .and_then(|mut f| f.write_all(content.as_bytes()))
                        .is_ok()
                };
                Ok(Value::Bool(ok))
            }
            "file_exists" => {
                if args.len() != 1 {
                    return Err("file_exists 需要一个字符串参数".into());
                }
                let Value::Str(path) = &args[0] else {
                    return Err("file_exists 需要一个字符串参数".into());
                };
                Ok(Value::Bool(std::path::Path::new(path).exists()))
            }
            // 字符串类：str_char / to_string 走 C ABI 桥（与编译路径共用实现，
            // 保证 UTF-8 码点索引与数字格式化两路径逐字节一致）。
            "str_char" => {
                if args.len() != 2 {
                    return Err("str_char 需要字符串与整数参数".into());
                }
                let (Value::Str(s), Value::Int(i)) = (&args[0], &args[1]) else {
                    return Err("str_char 需要字符串与整数参数".into());
                };
                let p = CString::new(s.as_str()).unwrap_or_default();
                let r = tie_str_char(p.as_ptr(), *i);
                let out = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(out))
            }
            "to_string" => {
                if args.len() != 1 {
                    return Err("to_string 需要一个数字参数".into());
                }
                // 数字重载：整数走 i64 桥，浮点走 f64 桥（与编译路径按实参类型分派一致）
                let r = match &args[0] {
                    Value::Int(n) => tie_to_string_i64(*n),
                    Value::Float(f) => tie_to_string_f64(*f),
                    _ => return Err("to_string 需要一个数字参数".into()),
                };
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            // 解析类：走 C ABI 桥（与编译路径共用同一份 Rust parse）。
            // 非法输入 → 报错（错误消息与编译路径 printf 文本一致）。
            "parse_int" => {
                if args.len() != 1 {
                    return Err("parse_int 需要一个字符串参数".into());
                }
                let Value::Str(s) = &args[0] else {
                    return Err("parse_int 需要一个字符串参数".into());
                };
                let mut ok: i8 = 0;
                let p = CString::new(s.as_str()).unwrap_or_default();
                let v = tie_parse_int(p.as_ptr(), &mut ok);
                if ok == 0 {
                    return Err(format!("运行时错误: parse_int 参数 '{s}' 不是合法的整数"));
                }
                Ok(Value::Int(v))
            }
            "parse_float" => {
                if args.len() != 1 {
                    return Err("parse_float 需要一个字符串参数".into());
                }
                let Value::Str(s) = &args[0] else {
                    return Err("parse_float 需要一个字符串参数".into());
                };
                let mut ok: i8 = 0;
                let p = CString::new(s.as_str()).unwrap_or_default();
                let v = tie_parse_float(p.as_ptr(), &mut ok);
                if ok == 0 {
                    return Err(format!("运行时错误: parse_float 参数 '{s}' 不是合法的浮点数"));
                }
                Ok(Value::Float(v))
            }
            // 进程控制：exit 刷新 stdout 后终止进程（编译路径：fflush + libc exit）。
            "exit" => {
                if args.len() != 1 {
                    return Err("exit 需要一个整数参数".into());
                }
                let Value::Int(code) = args[0] else {
                    return Err("exit 需要一个整数参数".into());
                };
                use std::io::Write;
                // 刷新 stdout：保证已输出内容在退出前可见（Windows 控制台有缓冲）
                let _ = std::io::stdout().flush();
                std::process::exit(code as i32);
            }
            // ---------- M2 数学/时间/随机 floor 内置函数 ----------
            //
            // 数学函数（sqrt/sin/cos/tan/exp/log/pow/floor/ceil/round）直接用 Rust f64
            // 方法，编译路径用 libm（@sqrt/@sin/...），两者对同一输入结果一致（IEEE 754）。
            // time_now / rand_range 走 C ABI 桥（与编译路径共用同一份 Rust 实现）。
            "time_now" => {
                if !args.is_empty() {
                    return Err("time_now 不需要参数".into());
                }
                Ok(Value::Int(tie_time_now()))
            }
            "rand_range" => {
                if args.len() != 2 {
                    return Err("rand_range 需要两个整数参数".into());
                }
                let (Value::Int(min), Value::Int(max)) = (&args[0], &args[1]) else {
                    return Err("rand_range 需要两个整数参数".into());
                };
                let mut ok: i8 = 0;
                let v = tie_rand_range(*min, *max, &mut ok);
                if ok == 0 {
                    return Err("运行时错误: rand_range 参数范围无效（max 必须大于 min）".into());
                }
                Ok(Value::Int(v))
            }
            "sqrt" | "sin" | "cos" | "tan" | "exp" | "log" | "floor" | "ceil" | "round" => {
                if args.len() != 1 {
                    return Err(format!("{name} 需要一个数字参数"));
                }
                let x = match &args[0] {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => return Err(format!("{name} 需要一个数字参数")),
                };
                // log 需要 x > 0：x<=0 报错（与编译路径 fcmp 检查一致）
                if name == "log" && x <= 0.0 {
                    return Err("运行时错误: log 参数必须大于 0".into());
                }
                let r = match name {
                    "sqrt" => x.sqrt(),
                    "sin" => x.sin(),
                    "cos" => x.cos(),
                    "tan" => x.tan(),
                    "exp" => x.exp(),
                    "log" => x.ln(),
                    "floor" => x.floor(),
                    "ceil" => x.ceil(),
                    "round" => x.round(),
                    _ => unreachable!(),
                };
                Ok(Value::Float(r))
            }
            "pow" => {
                if args.len() != 2 {
                    return Err("pow 需要两个数字参数".into());
                }
                let x = match &args[0] {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => return Err("pow 需要两个数字参数".into()),
                };
                let y = match &args[1] {
                    Value::Int(n) => *n as f64,
                    Value::Float(f) => *f,
                    _ => return Err("pow 需要两个数字参数".into()),
                };
                Ok(Value::Float(x.powf(y)))
            }
            _ => {
                // 用户函数（REPL 中定义的）
                if let Some(f) = self.session.funcs.get(name).cloned() {
                    if f.params.len() != args.len() {
                        return Err(format!(
                            "函数 '{name}' 期望 {} 个参数，实际 {} 个",
                            f.params.len(),
                            args.len()
                        ));
                    }
                    // 压新作用域绑定参数
                    self.scopes.push(std::collections::HashMap::new());
                    for (p, v) in f.params.iter().zip(args) {
                        self.scopes.last_mut().unwrap().insert(p.name.clone(), v);
                    }
                    let result = self.exec_block(&f.body);
                    self.scopes.pop();
                    // 处理 return 传播
                    match result? {
                        Flow::Normal(Some(v)) => Ok(v),
                        Flow::Normal(None) => Ok(Value::Void),
                        Flow::Return(v) => Ok(v),
                    }
                } else {
                    Err(format!("未定义的函数 '{name}'"))
                }
            }
        }
    }
}

/// 解释器值：动态类型（自带类型标签，运行时检查）。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Str(String),
    /// 范围 `start..end`（for 迭代用）
    Range(i64, i64),
    /// 表（单行元素集合；len 用，M2 范围）
    Table(Vec<Value>),
    /// 无值（void）
    Void,
}

impl Value {
    /// 类型名（错误提示用）。
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "整数",
            Value::Float(_) => "浮点数",
            Value::Bool(_) => "布尔",
            Value::Char(_) => "字符",
            Value::Str(_) => "字符串",
            Value::Range(_, _) => "范围",
            Value::Table(_) => "表",
            Value::Void => "void",
        }
    }

    /// 真值判断（if/while 条件）。
    pub fn is_truthy(&self) -> Result<bool, String> {
        match self {
            Value::Bool(b) => Ok(*b),
            _ => Err(format!("条件必须是布尔，实际是 {}", self.type_name())),
        }
    }

    /// 打印格式（println 输出，与编译路径一致：bool → 0/1）。
    pub fn to_print_string(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
            Value::Char(c) => c.to_string(),
            Value::Str(s) => s.clone(),
            Value::Range(s, e) => format!("{s}..{e}"),
            // 表：输出元素集合（仅 REPL 展示；编译路径不直接打印表）
            Value::Table(cells) => format!(
                "[{}]",
                cells.iter().map(|c| c.to_print_string()).collect::<Vec<_>>().join(", ")
            ),
            Value::Void => String::new(),
        }
    }

    /// REPL 结果格式（eval 返回给外壳展示，bool 用 true/false）。
    pub fn to_repl_string(&self) -> String {
        match self {
            Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            other => other.to_print_string(),
        }
    }
}

/// 运算符的可读名称（错误提示用）。
fn op_display(op: tie_frontend::ast::BinaryOp) -> &'static str {
    use tie_frontend::ast::BinaryOp;
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Eq => "==",
        BinaryOp::NotEq => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Gt => ">",
        BinaryOp::Le => "<=",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
        // M4 位运算/移位（错误提示用）
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷求值：新会话 + eval。
    fn ev(code: &str) -> Result<String, String> {
        Session::new().eval(code)
    }

    #[test]
    fn eval_arithmetic() {
        assert_eq!(ev("1 + 2").unwrap(), "3");
        assert_eq!(ev("10 - 3 * 2").unwrap(), "4");
        assert_eq!(ev("7 / 2").unwrap(), "3"); // 整数除法
        assert_eq!(ev("7 % 3").unwrap(), "1");
    }

    #[test]
    fn eval_string_concat() {
        assert_eq!(ev("\"foo\" + \"bar\"").unwrap(), "foobar");
    }

    #[test]
    fn eval_compare() {
        assert_eq!(ev("1 < 2").unwrap(), "true");
        assert_eq!(ev("\"abc\" == \"abc\"").unwrap(), "true");
        assert_eq!(ev("3 >= 4").unwrap(), "false");
    }

    #[test]
    fn eval_vars() {
        assert_eq!(ev("var x = 10; x * 2").unwrap(), "20");
        assert_eq!(ev("var x = 1; x = x + 1; x").unwrap(), "2");
    }

    #[test]
    fn eval_if_while() {
        // 注意：块语句（if/while/for）以 } 结束，后不能加分号；块内语句需分号
        assert_eq!(ev("if 1 < 2 { 10; } else { 20; }").unwrap(), "10");
        assert_eq!(ev("if 1 > 2 { 10; } else { 20; }").unwrap(), "20");
        assert_eq!(ev("var i = 0; while i < 3 { i = i + 1; } i").unwrap(), "3");
        assert_eq!(ev("var s = 0; for i in 1..4 { s = s + i; } s").unwrap(), "6");
    }

    #[test]
    fn eval_functions() {
        let func = "func add(a: i64, b: i64) -> i64 { return a + b; }";
        assert_eq!(ev(func).unwrap(), "已定义 1 个函数");
        let mut s = Session::new();
        s.eval(func).unwrap();
        assert_eq!(s.eval("add(20, 22)").unwrap(), "42");
    }

    #[test]
    fn eval_recursion() {
        let mut s = Session::new();
        s.eval("func fact(n: i64) -> i64 { if n <= 1 { return 1; } return n * fact(n - 1); }")
            .unwrap();
        assert_eq!(s.eval("fact(5)").unwrap(), "120");
    }

    #[test]
    fn eval_errors() {
        assert!(ev("1 / 0").is_err());
        assert!(ev("undefined_var").is_err());
        assert!(ev("1 + true").is_err());
    }

    #[test]
    fn eval_return_top() {
        assert_eq!(ev("return 5").unwrap(), "5");
    }

    #[test]
    fn eval_persistent_vars() {
        let mut s = Session::new();
        s.eval("var x = 1").unwrap();
        assert_eq!(s.eval("x + 1").unwrap(), "2");
    }

    // ---------- M4 运算符扩展测试 ----------

    #[test]
    fn eval_bitwise() {
        assert_eq!(ev("5 & 3").unwrap(), "1");
        assert_eq!(ev("5 | 8").unwrap(), "13");
        assert_eq!(ev("5 ^ 1").unwrap(), "4");
        assert_eq!(ev("8 >> 2").unwrap(), "2");
        assert_eq!(ev("1 << 3").unwrap(), "8");
        // 负数右移：算术右移（高位补 1）
        assert_eq!(ev("-8 >> 2").unwrap(), "-2");
        // 移位量越界 → 报错（不 panic）
        assert!(ev("1 << 64").is_err());
    }

    #[test]
    fn eval_compound_assign() {
        assert_eq!(ev("var x: i64 = 1; x += 2; x").unwrap(), "3");
        assert_eq!(ev("var x: i64 = 12; x -= 1; x").unwrap(), "11");
        assert_eq!(ev("var x: i64 = 3; x *= 4; x").unwrap(), "12");
        assert_eq!(ev("var x: i64 = 10; x /= 2; x").unwrap(), "5");
        assert_eq!(ev("var x: i64 = 5; x %= 3; x").unwrap(), "2");
        // 位运算复合赋值
        assert_eq!(ev("var x: i64 = 5; x &= 3; x").unwrap(), "1");
        assert_eq!(ev("var x: i64 = 5; x |= 8; x").unwrap(), "13");
        assert_eq!(ev("var x: i64 = 5; x ^= 1; x").unwrap(), "4");
        assert_eq!(ev("var x: i64 = 1; x <<= 3; x").unwrap(), "8");
        assert_eq!(ev("var x: i64 = 8; x >>= 2; x").unwrap(), "2");
        // 字符串复合拼接
        assert_eq!(ev("var s = \"a\"; s += \"b\"; s").unwrap(), "ab");
    }

    #[test]
    fn eval_ternary() {
        assert_eq!(ev("1 > 0 ? 10 : 20").unwrap(), "10");
        assert_eq!(ev("1 < 0 ? 10 : 20").unwrap(), "20");
        // 嵌套三目 + 变量
        assert_eq!(ev("var x: i64 = 5; (x > 0 ? 1 : 0) + 1").unwrap(), "2");
    }

    #[test]
    fn eval_inc_dec() {
        // 语句形式：后缀/前缀自增自减后变量值
        assert_eq!(ev("var x = 1; x++; x").unwrap(), "2");
        assert_eq!(ev("var x = 1; ++x; x").unwrap(), "2");
        assert_eq!(ev("var x = 2; x--; x").unwrap(), "1");
        assert_eq!(ev("var x = 2; --x; x").unwrap(), "1");
        // 表达式形式：后缀返回旧值、前缀返回新值
        assert_eq!(ev("var x = 1; x++ + x").unwrap(), "3"); // x++ → 1，随后 x=2 → 1+2
        assert_eq!(ev("var x = 1; ++x + x").unwrap(), "4"); // ++x → 2，随后 x=2 → 2+2
        // 浮点自增
        assert_eq!(ev("var x = 1.5; x++; x").unwrap(), "2.5");
    }

    // ---------- M2 标准库 floor 内置函数测试 ----------

    /// 生成唯一临时文件路径（避免并行测试相互干扰），并清场确保从干净状态开始。
    fn temp_path(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("tie_floor_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// 把文件路径转义成 tie 字符串字面量可直接嵌入的形式
    /// （Windows 路径含 `\`，而 tie 词法把 `\x` 当转义序列，需双写）。
    fn escaped_path(p: &std::path::Path) -> String {
        p.to_str().unwrap().replace('\\', "\\\\")
    }

    #[test]
    fn builtin_file_write_read_roundtrip() {
        let p = temp_path("roundtrip.txt");
        let path = escaped_path(&p);
        // 写 → 读回：内容一致（含换行与多字节 UTF-8）
        let code = format!(
            "var w = file_write(\"{path}\", \"hello\\n你好\\n\"); \
             var r = file_read(\"{path}\"); \
             (w ? \"ok\" : \"fail\") + \":\" + r"
        );
        assert_eq!(ev(&code).unwrap(), "ok:hello\n你好\n");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn builtin_file_append_grows() {
        let p = temp_path("append.txt");
        let path = escaped_path(&p);
        // 覆盖写一行 → 追加一行 → 读回两行
        let code = format!(
            "file_write(\"{path}\", \"line1\\n\"); \
             file_append(\"{path}\", \"line2\\n\"); \
             file_read(\"{path}\")"
        );
        assert_eq!(ev(&code).unwrap(), "line1\nline2\n");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn builtin_file_exists_true_false() {
        let p = temp_path("exists.txt");
        let path = escaped_path(&p);
        // 写前不存在 → false；写后存在 → true
        let code = format!(
            "var a = file_exists(\"{path}\"); \
             file_write(\"{path}\", \"x\"); \
             var b = file_exists(\"{path}\"); \
             (a ? \"0\" : \"1\") + (b ? \"1\" : \"0\")"
        );
        assert_eq!(ev(&code).unwrap(), "11");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn builtin_file_read_missing_errors() {
        let p = temp_path("missing.txt");
        let path = escaped_path(&p);
        // 文件不存在 → 运行时错误（文本与编译路径一致）
        let err = ev(&format!("file_read(\"{path}\")")).unwrap_err();
        assert!(err.contains("运行时错误: file_read 无法读取文件"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn builtin_str_char_index_and_utf8() {
        // ASCII 索引
        assert_eq!(ev("str_char(\"hello\", 1)").unwrap(), "e");
        // 多字节 UTF-8：按码点计数（"你好" 的 0 号字符是 "你"）
        assert_eq!(ev("str_char(\"你好\", 0)").unwrap(), "你");
        assert_eq!(ev("str_char(\"你好\", 1)").unwrap(), "好");
        // 混合中英文
        assert_eq!(ev("str_char(\"a你b\", 1)").unwrap(), "你");
        // 越界（下标 == 长度 / 更大）→ 空串
        assert_eq!(ev("str_char(\"hello\", 5)").unwrap(), "");
        assert_eq!(ev("str_char(\"hello\", 99)").unwrap(), "");
        // 负数下标 → 空串
        assert_eq!(ev("str_char(\"hello\", -1)").unwrap(), "");
    }

    #[test]
    fn builtin_to_string_numeric() {
        // i64 → 十进制
        assert_eq!(ev("to_string(42)").unwrap(), "42");
        assert_eq!(ev("to_string(-7)").unwrap(), "-7");
        assert_eq!(ev("to_string(0)").unwrap(), "0");
        // f64 → Rust {} 默认格式（最短往返表示）
        assert_eq!(ev("to_string(3.14)").unwrap(), "3.14");
        assert_eq!(ev("to_string(1.0)").unwrap(), "1");
        assert_eq!(ev("to_string(0.5)").unwrap(), "0.5");
    }

    #[test]
    fn builtin_parse_int_valid_invalid() {
        assert_eq!(ev("parse_int(\"123\")").unwrap(), "123");
        assert_eq!(ev("parse_int(\"-42\")").unwrap(), "-42");
        assert_eq!(ev("parse_int(\"0\")").unwrap(), "0");
        // 非法输入 → 运行时错误（文本与编译路径一致）
        let err = ev("parse_int(\"abc\")").unwrap_err();
        assert!(err.contains("运行时错误: parse_int 参数 'abc' 不是合法的整数"));
        // 部分合法（"12abc"）也报错（Rust 严格解析，与 strtoll 宽松解析不同）
        let err2 = ev("parse_int(\"12abc\")").unwrap_err();
        assert!(err2.contains("不是合法的整数"));
    }

    #[test]
    fn builtin_parse_float_valid_invalid() {
        assert_eq!(ev("parse_float(\"3.5\")").unwrap(), "3.5");
        assert_eq!(ev("parse_float(\"-0.25\")").unwrap(), "-0.25");
        // 整数字符串也能解析为浮点
        assert_eq!(ev("parse_float(\"2\")").unwrap(), "2");
        // 非法输入 → 运行时错误
        let err = ev("parse_float(\"abc\")").unwrap_err();
        assert!(err.contains("运行时错误: parse_float 参数 'abc' 不是合法的浮点数"));
    }

    #[test]
    fn builtin_exit_terminates_process() {
        // 子进程模式：执行 exit 内置，进程以指定码退出（不返回）
        if std::env::var_os("TIE_TEST_EXIT_CHILD").is_some() {
            let _ = Session::new().eval("exit(7)");
            unreachable!("exit(7) 应终止子进程");
        }
        // 父进程模式：重启本测试作为子进程（进程隔离，避免杀死测试运行器），断言退出码
        let exe = std::env::current_exe().expect("获取测试可执行文件路径");
        let out = std::process::Command::new(&exe)
            .args(["--exact", "tests::builtin_exit_terminates_process", "--nocapture"])
            .env("TIE_TEST_EXIT_CHILD", "1")
            .output()
            .expect("启动子进程");
        assert_eq!(out.status.code(), Some(7));
    }

    // ---------- M2 数学/时间/随机 floor 内置函数测试 ----------

    #[test]
    fn builtin_time_now_positive() {
        // Unix 纪元秒数应 > 0（2026 年显然大于 0）
        let t: i64 = ev("time_now()").unwrap().parse().unwrap();
        assert!(t > 0);
    }

    #[test]
    fn builtin_rand_range_bounds() {
        // 循环 100 次：值必须在 [min, max) 内
        for _ in 0..100 {
            let v: i64 = ev("rand_range(5, 10)").unwrap().parse().unwrap();
            assert!((5..10).contains(&v), "rand_range(5,10) 返回 {v} 越界");
        }
        // 单元素范围 [7, 8) → 恒为 7
        assert_eq!(ev("rand_range(7, 8)").unwrap(), "7");
        // 负数范围
        let v: i64 = ev("rand_range(-10, 0)").unwrap().parse().unwrap();
        assert!((-10..0).contains(&v));
    }

    #[test]
    fn builtin_rand_range_invalid_errors() {
        // max <= min → 运行时错误（文本与编译路径一致）
        let err = ev("rand_range(5, 5)").unwrap_err();
        assert!(err.contains("运行时错误: rand_range 参数范围无效"));
        let err2 = ev("rand_range(10, 5)").unwrap_err();
        assert!(err2.contains("运行时错误: rand_range 参数范围无效"));
    }

    #[test]
    fn builtin_math_sqrt() {
        assert_eq!(ev("sqrt(4)").unwrap(), "2");
        assert_eq!(ev("sqrt(2.25)").unwrap(), "1.5");
        // 负数 → NaN（IEEE 语义，与编译路径一致）
        assert_eq!(ev("sqrt(-1)").unwrap(), "NaN");
    }

    #[test]
    fn builtin_math_sin_cos_tan() {
        // 弧度制：sin(0)=0, cos(0)=1, tan(0)=0
        assert_eq!(ev("sin(0)").unwrap(), "0");
        assert_eq!(ev("cos(0)").unwrap(), "1");
        assert_eq!(ev("tan(0)").unwrap(), "0");
        // sin(π/2) ≈ 1
        let v: f64 = ev("sin(1.5707963267948966)").unwrap().parse().unwrap();
        assert!((v - 1.0).abs() < 1e-12);
    }

    #[test]
    fn builtin_math_exp_log() {
        assert_eq!(ev("exp(0)").unwrap(), "1");
        assert_eq!(ev("log(1)").unwrap(), "0");
        assert_eq!(ev("log(2.718281828459045)").unwrap(), "1");
        // log(0) / log(-1) → 运行时错误（文本与编译路径一致）
        let err = ev("log(0)").unwrap_err();
        assert!(err.contains("运行时错误: log 参数必须大于 0"));
        let err2 = ev("log(-1)").unwrap_err();
        assert!(err2.contains("运行时错误: log 参数必须大于 0"));
    }

    #[test]
    fn builtin_math_pow() {
        assert_eq!(ev("pow(2, 3)").unwrap(), "8");
        assert_eq!(ev("pow(2, 0.5)").unwrap(), "1.4142135623730951");
        assert_eq!(ev("pow(10, 2)").unwrap(), "100");
    }

    #[test]
    fn builtin_math_floor_ceil_round() {
        // floor/ceil/round（round 为四舍五入远离零）
        assert_eq!(ev("floor(3.7)").unwrap(), "3");
        assert_eq!(ev("floor(-3.7)").unwrap(), "-4");
        assert_eq!(ev("ceil(3.2)").unwrap(), "4");
        assert_eq!(ev("ceil(-3.2)").unwrap(), "-3");
        assert_eq!(ev("round(3.5)").unwrap(), "4");
        assert_eq!(ev("round(2.5)").unwrap(), "3");
        assert_eq!(ev("round(-2.5)").unwrap(), "-3");
    }

    #[test]
    fn builtin_len_table() {
        // len(表)：元素个数（单行 [1,2,3] → 3）
        assert_eq!(ev("len([1, 2, 3])").unwrap(), "3");
        assert_eq!(ev("len([])").unwrap(), "0");
        // len(字符串表)
        assert_eq!(ev("len([\"a\", \"b\"])").unwrap(), "2");
        // len(字符串) 保持原行为（字节数）
        assert_eq!(ev("len(\"hello\")").unwrap(), "5");
    }
}
