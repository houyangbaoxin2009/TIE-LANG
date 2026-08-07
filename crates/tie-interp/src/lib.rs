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
                let val = self.eval_expr(&a.value)?;
                self.assign(&a.target, val)?;
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
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let l = self.eval_expr(lhs)?;
                let r = self.eval_expr(rhs)?;
                self.eval_binary(*op, l, r)
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
            Expr::TableLit { .. } => Err("REPL v1 暂不支持表字面量".into()),
            Expr::TupleLit { .. } => Err("REPL v1 暂不支持元组".into()),
            Expr::FieldAccess { .. } => Err("REPL v1 暂不支持字段访问（类/元组）".into()),
            Expr::MethodCall { .. } => Err("REPL v1 暂不支持方法调用（类）".into()),
        }
    }

    /// 求值实参列表。
    fn eval_args(&mut self, args: &[Expr]) -> Result<Vec<Value>, String> {
        args.iter().map(|a| self.eval_expr(a)).collect()
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
            },
            (Value::Bool(a), Value::Bool(b)) => match op {
                BinaryOp::And => Ok(Value::Bool(a && b)),
                BinaryOp::Or => Ok(Value::Bool(a || b)),
                BinaryOp::Eq => Ok(Value::Bool(a == b)),
                BinaryOp::NotEq => Ok(Value::Bool(a != b)),
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
                    _ => Err("len 只支持字符串".into()),
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
}
