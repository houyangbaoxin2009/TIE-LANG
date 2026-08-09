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

use tie_frontend::ast::{BinaryOp, Expr, FnDefStmt, Stmt, TypeSpec};
use tie_frontend::lexer::{tokenize, TyKw};
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

/// C ABI 桥：调用已注册用户函数（字符串参数），返回结果字符串或错误串
/// （调用方负责 [tie_free_result]；语义与解释路径 eval_call 一致）。
///
/// tie:script 预处理模块协议的执行基础：框架先 eval 模块文件注册 `process` 函数，
/// 再经本桥以字符串值直传源码调用（不经源码文本转义），拿回处理结果。
#[unsafe(no_mangle)]
pub extern "C" fn tie_eval_call(name: *const c_char, arg: *const c_char) -> *mut c_char {
    c_guard(|| {
        let name = unsafe { c_char_to_string(name)? };
        let arg = unsafe { c_char_to_string(arg)? };
        with_session(|session| session.eval_call(&name, &arg))
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

/// C ABI 桥：解析 trit 字符串（"-1"/"0"/"1"）。成功置 ok=1 返回 -1/0/1；失败置 ok=0 返回 0。
/// 与 parse_int 同模式（编译/解释两路径共用，保证错误文本一致）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_parse_trit(s: *const c_char, ok: *mut i8) -> i8 {
    let parsed = match unsafe { c_char_to_string(s) } {
        Ok(s) => match s.as_str() {
            "-1" => Some(-1),
            "0" => Some(0),
            "1" => Some(1),
            _ => None,
        },
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

// ---------- 进程/环境 floor 内置函数 C ABI 桥 ----------
//
// 设计说明：arg_count / arg_string 走 std::env::args()（进程运行时查询真实 argv），
// 编译路径（IR 层）与解释路径共用本桥，保证两路径返回一致：
// - 编译后的 exe：静态链接本库后，std::env::args() 读取进程命令行（Windows 走
//   GetCommandLineW，Linux 走 /proc/self/cmdline），与 C main 的 argv 一致；
// - 解释路径（tie-interp / REPL）：解释器进程自身的命令行参数，REPL 无用户参数 → 0。
// argv 约定（文档化）：arg_count 返回「程序名之后」的用户参数个数（`prog a b c` → 3）；
// arg_string(i) 按 0 基索引用户参数（arg_string(0)="a"），越界（负数或 >= 个数）返回空串。

/// C ABI 桥：返回命令行用户参数个数（argv[0] 程序名之后的数量；`prog a b c` → 3）。
///
/// 实现：`std::env::args()` 至少含程序名（argv[0]），用户参数 = 总数 - 1；
/// 极端环境（无 argv）时用 saturating_sub 保证下限 0（>= 0 契约）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_arg_count() -> i64 {
    std::env::args().count().saturating_sub(1) as i64
}

/// C ABI 桥：返回第 i 个用户命令行参数（0 基，跳过 argv[0] 程序名），返回新分配的字符串；
/// 越界（负数下标或 >= 参数个数）返回空串。
///
/// 与 tie_str_char 同一堆串模式：返回 `CString::into_raw` 分配的 `*mut c_char`，
/// 调用方用完必须 `tie_free_result` 释放。
#[unsafe(no_mangle)]
pub extern "C" fn tie_arg_string(i: i64) -> *mut c_char {
    // 收集用户参数（skip(1) 跳过 argv[0] 程序名），按 i 索引；i 越界 → None → 空串
    let s = if i < 0 {
        None
    } else {
        std::env::args().skip(1).nth(i as usize)
    };
    string_to_c_char(s.unwrap_or_default())
}

// ---------- 消息系统（#25）floor C ABI 桥 ----------
//
// 设计说明：tie 语言没有全局可变状态（编译程序无顶层 var），而消息系统的
// 「当前语言 + 字典」恰恰是跨函数共享的可变状态，tie 自身无法表达——这是
// 「实在不行才 Rust」的典型场景。状态由 Rust 层 thread_local 持有：
// - lang：当前语言（默认 "zh"），msg_set_lang 切换；
// - dict：(键, 语言) → 文本 的字典，msg_register 登记（同键同语言覆盖）；
// - msg_t 查询顺序：当前语言 → 回退 "zh" → 再回退「键本身」。
// 编译路径（IR 层）与解释路径共用本桥（thread_local 各线程独立，两路径单线程一致）。

/// 消息系统运行时状态（thread_local；语言与字典随调用累积）。
#[derive(Default)]
struct MsgState {
    /// 当前语言（默认 "zh"）
    lang: String,
    /// (键, 语言) → 文本 的消息字典
    dict: std::collections::HashMap<(String, String), String>,
}

thread_local! {
    static MSG_STATE: std::cell::RefCell<MsgState> = std::cell::RefCell::new(MsgState::default());
}

/// C ABI 桥：切换消息系统当前语言（如 "zh" / "en"）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_msg_set_lang(lang: *const c_char) {
    let lang = unsafe { c_char_to_string(lang).unwrap_or_default() };
    MSG_STATE.with(|s| s.borrow_mut().lang = lang);
}

/// C ABI 桥：读取消息系统当前语言，返回新分配的堆串（调用方用完必须 tie_free_result）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_msg_get_lang() -> *mut c_char {
    let lang = MSG_STATE.with(|s| s.borrow().lang.clone());
    string_to_c_char(lang)
}

/// C ABI 桥：登记一条消息文本（键 + 语言 + 文本；同键同语言覆盖旧文本）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_msg_register(key: *const c_char, lang: *const c_char, text: *const c_char) {
    let key = unsafe { c_char_to_string(key).unwrap_or_default() };
    let lang = unsafe { c_char_to_string(lang).unwrap_or_default() };
    let text = unsafe { c_char_to_string(text).unwrap_or_default() };
    MSG_STATE.with(|s| {
        s.borrow_mut().dict.insert((key, lang), text);
    });
}

/// C ABI 桥：查询键的翻译文本，返回新分配的堆串（调用方用完必须 tie_free_result）。
///
/// 查询顺序：当前语言 → 回退 "zh" → 回退「键本身」（未登记时原样返回键）。
/// M4 起消息系统状态（级别/回退链）由 tie 语言自身用顶层持久变量表达，
/// 本桥保持最小查询能力；指定语言查询见 tie_msg_t_lang。
#[unsafe(no_mangle)]
pub extern "C" fn tie_msg_t(key: *const c_char) -> *mut c_char {
    let key = unsafe { c_char_to_string(key).unwrap_or_default() };
    let out = MSG_STATE.with(|s| {
        let s = s.borrow();
        // 1) 当前语言
        if let Some(text) = s.dict.get(&(key.clone(), s.lang.clone())) {
            return text.clone();
        }
        // 2) 回退 zh
        if s.lang != "zh"
            && let Some(text) = s.dict.get(&(key.clone(), "zh".to_string()))
        {
            return text.clone();
        }
        // 3) 回退键本身
        key.clone()
    });
    string_to_c_char(out)
}

/// C ABI 桥：向 stderr 输出一行（消息系统的 error/warn/debug 通道；info 走 stdout 的
/// println——M4 控制台消息库按级别区分输出通道）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_print_err(text: *const c_char) {
    let text = unsafe { c_char_to_string(text).unwrap_or_default() };
    eprintln!("{text}");
}

/// C ABI 桥：按**指定语言**查询键的翻译文本（不做回退，命中返回文本、未命中返回空串），
/// 返回新分配的堆串（调用方用完必须 tie_free_result）。
///
/// M4 起消息系统的回退语言链由 tie 语言自身用顶层持久变量表达，tcmsg 遍历回退链时
/// 逐个调用本桥做指定语言查询（替代固定 zh 回退的 msg_t）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_msg_t_lang(key: *const c_char, lang: *const c_char) -> *mut c_char {
    let key = unsafe { c_char_to_string(key).unwrap_or_default() };
    let lang = unsafe { c_char_to_string(lang).unwrap_or_default() };
    let out = MSG_STATE.with(|s| {
        s.borrow()
            .dict
            .get(&(key, lang))
            .cloned()
            .unwrap_or_default()
    });
    string_to_c_char(out)
}

// ---------- 动态表（table_new_* / table_push / table_at）C ABI 桥 ----------
//
// 设计说明（编译/解释两路径一致性的关键）：动态表在**编译路径**（IR 层）表示为
// 「堆分配的 {data, len, cap, elem_size} 结构体指针」，表值本身是 ptr（与字符串
// 一致，便于函数参数/返回传递）。所有动态表操作（新建/追加/读取/长度/释放）都走
// 本桥——Rust 层是唯一实现，保证编译路径行为与解释路径（Value::Table(Vec)）一致：
// - 扩容：容量翻倍（初始 8）；
// - 越界错误文本：由 IR 层（table_at 的 ok 标志）与解释路径共用同一句中文消息。
// 内存：data 缓冲区用 Rust 分配器（alloc/realloc/dealloc），仅由本桥分配与释放
// （tie_table_free），自洽无混用；程序退出时泄漏可接受（与 C 一致），但循环内创建
// 的动态表在作用域结束时由 IR 发射 tie_table_free 释放，避免逐次迭代泄漏。

/// 动态表运行时结构（C 布局；仅本桥内部使用，IR 侧只持有不透明 ptr）。
///
/// `pub` 仅为满足 C ABI 导出函数签名（`*mut DynTable`）的可见性要求；
/// 字段不公开，外部（IR 层）只把它当不透明指针传递。
#[repr(C)]
pub struct DynTable {
    /// 元素缓冲区指针（cap==0 时为 null）
    data: *mut std::ffi::c_void,
    /// 当前元素个数
    len: i64,
    /// 缓冲区容量（元素个数）
    cap: i64,
    /// 单个元素字节数（i64/f64/ptr=8，bool=1）
    elem_size: i64,
}

/// 扩容：len==cap 时把缓冲区容量翻倍（初始 8），用 Rust 分配器 realloc。
///
/// 安全：data 由本桥分配（alloc/realloc），cap 记录旧容量，realloc 的旧布局
/// 由「旧容量 × 元素大小」精确还原，满足 Rust 分配器契约。
unsafe fn dyn_table_grow(t: &mut DynTable) {
    if t.len < t.cap {
        return;
    }
    let elem = t.elem_size as usize;
    let old_cap = t.cap as usize;
    let new_cap = if old_cap == 0 { 8 } else { old_cap * 2 };
    let align = std::mem::align_of::<i64>();
    let new_layout =
        std::alloc::Layout::from_size_align(new_cap * elem, align).expect("动态表扩容布局合法");
    let new_data = if t.data.is_null() {
        unsafe { std::alloc::alloc(new_layout) }
    } else {
        let old_layout = std::alloc::Layout::from_size_align(old_cap * elem, align)
            .expect("动态表旧布局合法");
        unsafe { std::alloc::realloc(t.data as *mut u8, old_layout, new_cap * elem) }
    };
    t.data = new_data as *mut std::ffi::c_void;
    t.cap = new_cap as i64;
}

/// C ABI 桥：新建空动态表（元素字节大小由调用方传入：i64/f64/ptr=8，bool=1）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_new(elem_size: i64) -> *mut DynTable {
    let layout = std::alloc::Layout::new::<DynTable>();
    let ptr = unsafe { std::alloc::alloc(layout) } as *mut DynTable;
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        std::ptr::write(
            ptr,
            DynTable { data: std::ptr::null_mut(), len: 0, cap: 0, elem_size },
        );
    }
    ptr
}

/// C ABI 桥：释放动态表（先释放元素缓冲区，再释放结构体本身）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_free(t: *mut DynTable) {
    if t.is_null() {
        return;
    }
    unsafe {
        let tbl = &mut *t;
        if !tbl.data.is_null() {
            let size = (tbl.cap as usize) * (tbl.elem_size as usize);
            let layout = std::alloc::Layout::from_size_align(size, std::mem::align_of::<i64>())
                .expect("动态表释放布局合法");
            std::alloc::dealloc(tbl.data as *mut u8, layout);
        }
        std::alloc::dealloc(t as *mut u8, std::alloc::Layout::new::<DynTable>());
    }
}

/// C ABI 桥：返回动态表当前长度（元素个数）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_len(t: *mut DynTable) -> i64 {
    if t.is_null() {
        return 0;
    }
    unsafe { (*t).len }
}

/// C ABI 桥：向动态表追加 i64 元素。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_push_i64(t: *mut DynTable, x: i64) {
    unsafe {
        let tbl = &mut *t;
        dyn_table_grow(tbl);
        (tbl.data as *mut i64).add(tbl.len as usize).write(x);
        tbl.len += 1;
    }
}

/// C ABI 桥：向动态表追加 f64 元素。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_push_f64(t: *mut DynTable, x: f64) {
    unsafe {
        let tbl = &mut *t;
        dyn_table_grow(tbl);
        (tbl.data as *mut f64).add(tbl.len as usize).write(x);
        tbl.len += 1;
    }
}

/// C ABI 桥：向动态表追加字符串元素（存储借用指针，不复制、不释放）。
///
/// 约定：被追加的串指针必须比表存活更久（tie 无显式 free，串要么是全局常量、
/// 要么由变量持有到程序退出），因此表中存借用指针是安全的。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_push_string(t: *mut DynTable, x: *const c_char) {
    unsafe {
        let tbl = &mut *t;
        dyn_table_grow(tbl);
        (tbl.data as *mut *const c_char).add(tbl.len as usize).write(x);
        tbl.len += 1;
    }
}

/// C ABI 桥：向动态表追加 bool 元素（C 侧按 i8 传递）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_push_bool(t: *mut DynTable, x: i8) {
    unsafe {
        let tbl = &mut *t;
        dyn_table_grow(tbl);
        (tbl.data as *mut i8).add(tbl.len as usize).write(x);
        tbl.len += 1;
    }
}

/// C ABI 桥：读取动态表第 i 个 i64 元素。越界（负数或 >= len）置 ok=0 并返回 0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_at_i64(t: *mut DynTable, i: i64, ok: *mut i8) -> i64 {
    unsafe {
        let tbl = &*t;
        if i < 0 || i >= tbl.len {
            *ok = 0;
            return 0;
        }
        *ok = 1;
        (tbl.data as *const i64).add(i as usize).read()
    }
}

/// C ABI 桥：读取动态表第 i 个 f64 元素。越界置 ok=0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_at_f64(t: *mut DynTable, i: i64, ok: *mut i8) -> f64 {
    unsafe {
        let tbl = &*t;
        if i < 0 || i >= tbl.len {
            *ok = 0;
            return 0.0;
        }
        *ok = 1;
        (tbl.data as *const f64).add(i as usize).read()
    }
}

/// C ABI 桥：读取动态表第 i 个字符串元素（返回借用指针，调用方不得释放）。
/// 越界置 ok=0 并返回空指针。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_at_string(t: *mut DynTable, i: i64, ok: *mut i8) -> *mut c_char {
    unsafe {
        let tbl = &*t;
        if i < 0 || i >= tbl.len {
            *ok = 0;
            return std::ptr::null_mut();
        }
        *ok = 1;
        (tbl.data as *const *mut c_char).add(i as usize).read()
    }
}

/// C ABI 桥：读取动态表第 i 个 bool 元素（C 侧按 i8 返回）。越界置 ok=0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_at_bool(t: *mut DynTable, i: i64, ok: *mut i8) -> i8 {
    unsafe {
        let tbl = &*t;
        if i < 0 || i >= tbl.len {
            *ok = 0;
            return 0;
        }
        *ok = 1;
        (tbl.data as *const i8).add(i as usize).read()
    }
}

// ---------- M4 补齐：动态表写入（下标赋值 t[i] = v）C ABI 桥 ----------
//
// 与读取桥（tie_table_at_*）对称：按元素类型写入第 i 个槽位。
// 越界（负数或 >= len）置 ok=0 不写入（由调用方输出与读取一致的越界错误）；
// 合法则置 ok=1 并原地写入（动态表是可变堆结构，写后表自洽）。

/// C ABI 桥：写动态表第 i 个 i64 元素。越界置 ok=0，不写入。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_set_i64(t: *mut DynTable, i: i64, x: i64, ok: *mut i8) {
    unsafe {
        let tbl = &mut *t;
        if i < 0 || i >= tbl.len {
            *ok = 0;
            return;
        }
        *ok = 1;
        (tbl.data as *mut i64).add(i as usize).write(x);
    }
}

/// C ABI 桥：写动态表第 i 个 f64 元素。越界置 ok=0，不写入。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_set_f64(t: *mut DynTable, i: i64, x: f64, ok: *mut i8) {
    unsafe {
        let tbl = &mut *t;
        if i < 0 || i >= tbl.len {
            *ok = 0;
            return;
        }
        *ok = 1;
        (tbl.data as *mut f64).add(i as usize).write(x);
    }
}

/// C ABI 桥：写动态表第 i 个字符串元素（存借用指针，不复制——与 table_push_string
/// 同一约定：调用方保证指针比表存活更久）。越界置 ok=0，不写入。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_set_string(t: *mut DynTable, i: i64, x: *const c_char, ok: *mut i8) {
    unsafe {
        let tbl = &mut *t;
        if i < 0 || i >= tbl.len {
            *ok = 0;
            return;
        }
        *ok = 1;
        (tbl.data as *mut *const c_char).add(i as usize).write(x);
    }
}

/// C ABI 桥：写动态表第 i 个 bool 元素（C 侧按 i8）。越界置 ok=0，不写入。
#[unsafe(no_mangle)]
pub extern "C" fn tie_table_set_bool(t: *mut DynTable, i: i64, x: i8, ok: *mut i8) {
    unsafe {
        let tbl = &mut *t;
        if i < 0 || i >= tbl.len {
            *ok = 0;
            return;
        }
        *ok = 1;
        (tbl.data as *mut i8).add(i as usize).write(x);
    }
}

// ---------- D7：字节流 / 位操作原语（M4 补齐延伸） C ABI 桥 ----------
//
// 设计说明（编解码器底座）：JPEG/MP3/LZ4 等多媒体/压缩算法需要**逐字节文件 IO**
// 与**位级打包**。tie 语言无法自举文件字节级读写（文本 file_read 不适用），
// 且表参数元素类型静态未知（tie 侧无法遍历字节表）——因此：
// - 表参数（字节表）的**遍历在 Rust 桥层完成**（tie 侧只传表指针）；
// - byte_read 返回 i64 动态表（0..255）；byte_write 接收 i64 表写文件；
// - bit_read/bit_write 在**调用方用表操作**实现（见 eval 分支），桥只做字节级 IO。
// 两路径一致：编译（IR）走桥，解释（interp）走同一桥。

/// C ABI 桥：读取文件全部字节，返回 i64 动态表（每个元素 0..255）；失败返回 NULL。
///
/// 表元素：i64（字节值），调用方用 table_at/下标读取（table_ret_elems 记录 i64）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_byte_read(path: *const c_char) -> *mut DynTable {
    let path = match unsafe { c_char_to_string(path) } {
        Ok(p) => p,
        Err(_) => return std::ptr::null_mut(),
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return std::ptr::null_mut(),
    };
    let t = tie_table_new(8); // i64 元素
    if t.is_null() {
        return t;
    }
    for b in bytes {
        tie_table_push_i64(t, b as i64);
    }
    t
}

/// C ABI 桥：把 i64 动态表（元素 0..255）写入文件；成功返回 1（bool true），失败返回 0。
///
/// 表元素读取在桥层完成（tie 侧只传表指针——规避表参数元素类型未知限制）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_byte_write(path: *const c_char, bytes: *mut DynTable) -> i8 {
    let path = match unsafe { c_char_to_string(path) } {
        Ok(p) => p,
        Err(_) => return 0,
    };
    if bytes.is_null() {
        return 0;
    }
    unsafe {
        let tbl = &*bytes;
        let mut buf: Vec<u8> = Vec::with_capacity(tbl.len as usize);
        for i in 0..tbl.len {
            let v = (tbl.data as *const i64).add(i as usize).read();
            buf.push(v.clamp(0, 255) as u8);
        }
        match std::fs::write(&path, &buf) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    }
}

/// C ABI 桥：读字节表第 pos 位（LSB 序：第 pos 位 = byte[pos/8] 的 (pos%8) 位）。
/// 越界返回 0。返回 0/1。
#[unsafe(no_mangle)]
pub extern "C" fn tie_bit_read(bytes: *mut DynTable, pos: i64) -> i64 {
    if bytes.is_null() || pos < 0 {
        return 0;
    }
    unsafe {
        let tbl = &*bytes;
        let byte_idx = pos / 8;
        if byte_idx >= tbl.len {
            return 0;
        }
        let v = (tbl.data as *const i64).add(byte_idx as usize).read();
        let bit = pos % 8;
        ((v >> bit) & 1)
    }
}

/// C ABI 桥：写字节表第 pos 位（LSB 序）为 bit（0/1）；越界返回 0（失败），成功返回 1。
#[unsafe(no_mangle)]
pub extern "C" fn tie_bit_write(bytes: *mut DynTable, pos: i64, bit: i64) -> i8 {
    if bytes.is_null() || pos < 0 {
        return 0;
    }
    unsafe {
        let tbl = &mut *bytes;
        let byte_idx = pos / 8;
        if byte_idx >= tbl.len {
            return 0;
        }
        let cur = (tbl.data as *mut i64).add(byte_idx as usize).read();
        let bit_idx = pos % 8;
        let new = if bit != 0 {
            cur | (1 << bit_idx)
        } else {
            cur & !(1 << bit_idx)
        };
        (tbl.data as *mut i64).add(byte_idx as usize).write(new);
        1
    }
}

/// C ABI 桥：拼接两个字节表，返回新 i64 动态表。
#[unsafe(no_mangle)]
pub extern "C" fn tie_byte_concat(a: *mut DynTable, b: *mut DynTable) -> *mut DynTable {
    let t = tie_table_new(8);
    if t.is_null() {
        return t;
    }
    unsafe {
        for tbl in [a, b] {
            if tbl.is_null() {
                continue;
            }
            let tb = &*tbl;
            for i in 0..tb.len {
                let v = (tb.data as *const i64).add(i as usize).read();
                tie_table_push_i64(t, v);
            }
        }
    }
    t
}

/// C ABI 桥：列出目录中的文件名（仅文件名，不含路径），返回字符串动态表；失败返回 NULL。
///
/// 实现：`std::fs::read_dir` 枚举目录（条目顺序由文件系统给出，不排序），
/// 每个条目名作为借用指针推入表（泄漏的堆串，遵守「字符串元素是借用指针」的
/// 动态表约定；程序退出时回收，可接受）。目录不存在/无权限等读取失败 → NULL，
/// 由调用方（IR 层 / 解释器）统一输出错误消息，保证两路径的错误文本一致。
#[unsafe(no_mangle)]
pub extern "C" fn tie_list_dir(path: *const c_char) -> *mut DynTable {
    let path = match unsafe { c_char_to_string(path) } {
        Ok(p) => p,
        Err(_) => return std::ptr::null_mut(),
    };
    // 收集条目名（只取文件名部分；单个条目读取失败跳过）
    let entries: Vec<String> = match std::fs::read_dir(&path) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => return std::ptr::null_mut(),
    };
    // 新建字符串动态表（elem_size=8，与 table_new_string 一致），逐个推入条目名
    let t = tie_table_new(8);
    if t.is_null() {
        return t;
    }
    for name in entries {
        // 泄漏堆串作为表的借用元素（与 table_push_string 同一约定，调用方不释放）
        let c = CString::new(name).unwrap_or_default();
        tie_table_push_string(t, c.into_raw());
    }
    t
}

// ---------- M4 补齐：系统能力 floor 内置函数 C ABI 桥 ----------
//
// 设计说明（M6 包管理器前置）：包管理器需要「路径/环境/文件/目录/进程/网络/解压」
// 系统能力，tie 语言自身无法自举（操作系统 API 属语言底座原语），Rust 层唯一实现，
// 编译路径（IR 层）与解释路径（tie-interp）共用，行为逐字节一致。
// 返回堆串的桥（string 类）沿用 read_line 模式：CString::into_raw，调用方 tie_free_result；
// 返回 bool 的桥按 i8（0/1）传递；返回动态表的桥复用 DynTable（字符串表）约定。

/// C ABI 桥：HTTP GET 指定 URL，返回响应正文（新分配堆串）；请求失败返回 NULL。
///
/// 首版实现：Rust std TcpStream 手写最小 HTTP/1.1 GET（零新依赖），仅支持 http://；
/// https（TLS）留待后续（可换 reqwest）。失败（连接/解析/非 200）→ NULL，
/// 由调用方统一输出错误消息（两路径消息一致）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_http_get(url: *const c_char) -> *mut c_char {
    let url = match unsafe { c_char_to_string(url) } {
        Ok(u) => u,
        Err(_) => return std::ptr::null_mut(),
    };
    match http_get_impl(&url) {
        Ok(body) => string_to_c_char(String::from_utf8_lossy(&body).into_owned()),
        Err(_) => std::ptr::null_mut(),
    }
}

/// C ABI 桥：HTTP GET 下载到本地文件，成功返回 1（bool true），失败返回 0。
/// 正文按原始字节写入（不经过有损转换，二进制包 tar.gz/zip 可完整下载）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_http_get_file(url: *const c_char, path: *const c_char) -> i8 {
    let url = match unsafe { c_char_to_string(url) } {
        Ok(u) => u,
        Err(_) => return 0,
    };
    let path = match unsafe { c_char_to_string(path) } {
        Ok(p) => p,
        Err(_) => return 0,
    };
    match http_get_impl(&url) {
        Ok(body) => std::fs::write(&path, &body).is_ok() as i8,
        Err(_) => 0,
    }
}

/// C ABI 桥：执行命令（整条命令行），返回退出码（启动失败返回 -1）。
/// 跨平台：Windows 用 `cmd /C`，其他平台用 `sh -c` 包装整条命令行。
#[unsafe(no_mangle)]
pub extern "C" fn tie_exec_code(cmd: *const c_char) -> i64 {
    let cmd = match unsafe { c_char_to_string(cmd) } {
        Ok(c) => c,
        Err(_) => return -1,
    };
    exec_cmd(&cmd)
        .map(|s| s.code().unwrap_or(-1) as i64)
        .unwrap_or(-1)
}

/// C ABI 桥：执行命令并捕获 stdout（返回新分配堆串；stderr 透传；启动失败返回空串）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_exec_output(cmd: *const c_char) -> *mut c_char {
    let cmd = match unsafe { c_char_to_string(cmd) } {
        Ok(c) => c,
        Err(_) => return string_to_c_char(String::new()),
    };
    string_to_c_char(exec_output_impl(&cmd))
}

/// C ABI 桥：解压 tar.gz 归档到 dest 目录，成功返回 1（bool true），失败返回 0。
/// 实现：flate2 GzDecoder 解 gzip 层 → tar Archive::unpack 解 tar 层（自动建目录）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_untar_gz(file: *const c_char, dest: *const c_char) -> i8 {
    let file = match unsafe { c_char_to_string(file) } {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let dest = match unsafe { c_char_to_string(dest) } {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let result: std::io::Result<()> = (|| {
        // 目标目录需先存在（tar 解包不自动建根目录）；不存在则创建
        std::fs::create_dir_all(&dest)?;
        let f = std::fs::File::open(&file)?;
        let gz = flate2::read::GzDecoder::new(f);
        let mut ar = tar::Archive::new(gz);
        ar.unpack(&dest)
    })();
    result.is_ok() as i8
}

/// C ABI 桥：解压 zip 归档到 dest 目录，成功返回 1（bool true），失败返回 0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_unzip(file: *const c_char, dest: *const c_char) -> i8 {
    let file = match unsafe { c_char_to_string(file) } {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let dest = match unsafe { c_char_to_string(dest) } {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let result: Result<(), Box<dyn std::error::Error>> = (|| {
        // 目标目录需先存在（zip extract 不自动建根目录）；不存在则创建
        std::fs::create_dir_all(&dest)?;
        let f = std::fs::File::open(&file)?;
        let mut zip = zip::ZipArchive::new(f)?;
        zip.extract(&dest)?;
        Ok(())
    })();
    result.is_ok() as i8
}

/// C ABI 桥：递归创建多级目录，成功返回 1（bool true），失败返回 0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_mkdir_all(path: *const c_char) -> i8 {
    let path = match unsafe { c_char_to_string(path) } {
        Ok(p) => p,
        Err(_) => return 0,
    };
    std::fs::create_dir_all(&path).is_ok() as i8
}

/// C ABI 桥：递归删除目录（含内容），成功返回 1（bool true），失败返回 0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_remove_dir_all(path: *const c_char) -> i8 {
    let path = match unsafe { c_char_to_string(path) } {
        Ok(p) => p,
        Err(_) => return 0,
    };
    std::fs::remove_dir_all(&path).is_ok() as i8
}

/// C ABI 桥：递归复制目录（src → dest，自动建 dest），成功返回 1（bool true），失败返回 0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_copy_dir(src: *const c_char, dest: *const c_char) -> i8 {
    let src = match unsafe { c_char_to_string(src) } {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let dest = match unsafe { c_char_to_string(dest) } {
        Ok(d) => d,
        Err(_) => return 0,
    };
    copy_dir_impl(std::path::Path::new(&src), std::path::Path::new(&dest)).is_ok() as i8
}

/// C ABI 桥：递归列出目录下全部**文件**的相对路径（字符串动态表）；目录无效返回 NULL。
/// 复用 tie_list_dir 的字符串动态表约定（泄漏堆串作为借用元素）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_walk_dir(path: *const c_char) -> *mut DynTable {
    let path = match unsafe { c_char_to_string(path) } {
        Ok(p) => p,
        Err(_) => return std::ptr::null_mut(),
    };
    let base = std::path::Path::new(&path);
    let mut out: Vec<String> = Vec::new();
    walk_dir_impl(base, base, &mut out);
    // 目录无效（read_dir 失败 → out 为空）与空目录无法区分：用 read_dir 探测一次
    if out.is_empty() && std::fs::read_dir(base).is_err() {
        return std::ptr::null_mut();
    }
    let t = tie_table_new(8);
    if t.is_null() {
        return t;
    }
    for name in out {
        let c = CString::new(name).unwrap_or_default();
        tie_table_push_string(t, c.into_raw());
    }
    t
}

/// C ABI 桥：拼接两个路径（a + 分隔符 + b），返回新分配堆串。
#[unsafe(no_mangle)]
pub extern "C" fn tie_path_join(a: *const c_char, b: *const c_char) -> *mut c_char {
    let a = unsafe { c_char_to_string(a).unwrap_or_default() };
    let b = unsafe { c_char_to_string(b).unwrap_or_default() };
    let p = std::path::Path::new(&a).join(&b);
    string_to_c_char(p.to_string_lossy().into_owned())
}

/// C ABI 桥：取路径的最后一段（文件名/目录名），返回新分配堆串；无父段返回原串。
#[unsafe(no_mangle)]
pub extern "C" fn tie_path_basename(p: *const c_char) -> *mut c_char {
    let p = unsafe { c_char_to_string(p).unwrap_or_default() };
    let s = std::path::Path::new(&p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(p);
    string_to_c_char(s)
}

/// C ABI 桥：取路径的父目录，返回新分配堆串；无父目录返回空串。
#[unsafe(no_mangle)]
pub extern "C" fn tie_path_dirname(p: *const c_char) -> *mut c_char {
    let p = unsafe { c_char_to_string(p).unwrap_or_default() };
    let s = std::path::Path::new(&p)
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    string_to_c_char(s)
}

/// C ABI 桥：把路径转绝对路径（基于当前工作目录），返回新分配堆串；失败返回原串。
#[unsafe(no_mangle)]
pub extern "C" fn tie_path_abs(p: *const c_char) -> *mut c_char {
    let p = unsafe { c_char_to_string(p).unwrap_or_default() };
    let s = std::path::absolute(&p)
        .map(|a| a.to_string_lossy().into_owned())
        .unwrap_or(p);
    string_to_c_char(s)
}

/// C ABI 桥：规范化路径（解析 . 与 .. 、合并重复分隔符），返回新分配堆串。
#[unsafe(no_mangle)]
pub extern "C" fn tie_path_normalize(p: *const c_char) -> *mut c_char {
    let p = unsafe { c_char_to_string(p).unwrap_or_default() };
    string_to_c_char(normalize_path(&p))
}

/// C ABI 桥：返回当前工作目录（新分配堆串）；获取失败返回空串。
#[unsafe(no_mangle)]
pub extern "C" fn tie_cwd() -> *mut c_char {
    let s = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    string_to_c_char(s)
}

/// C ABI 桥：读取环境变量，返回新分配堆串；变量不存在返回空串。
#[unsafe(no_mangle)]
pub extern "C" fn tie_get_env(name: *const c_char) -> *mut c_char {
    let name = unsafe { c_char_to_string(name).unwrap_or_default() };
    let s = std::env::var(&name).unwrap_or_default();
    string_to_c_char(s)
}

/// C ABI 桥：设置环境变量（透传给本进程与后续子进程）。
#[unsafe(no_mangle)]
pub extern "C" fn tie_set_env(name: *const c_char, val: *const c_char) {
    let name = unsafe { c_char_to_string(name).unwrap_or_default() };
    let val = unsafe { c_char_to_string(val).unwrap_or_default() };
    // 2024 edition 下 set_var 为 unsafe（多线程环境写环境变量需调用方保证单线程）
    unsafe { std::env::set_var(name, val) };
}

/// C ABI 桥：复制文件（src → dest），成功返回 1（bool true），失败返回 0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_file_copy(src: *const c_char, dest: *const c_char) -> i8 {
    let src = match unsafe { c_char_to_string(src) } {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let dest = match unsafe { c_char_to_string(dest) } {
        Ok(d) => d,
        Err(_) => return 0,
    };
    std::fs::copy(&src, &dest).is_ok() as i8
}

/// C ABI 桥：移动/重命名文件（src → dest），成功返回 1（bool true），失败返回 0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_file_move(src: *const c_char, dest: *const c_char) -> i8 {
    let src = match unsafe { c_char_to_string(src) } {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let dest = match unsafe { c_char_to_string(dest) } {
        Ok(d) => d,
        Err(_) => return 0,
    };
    std::fs::rename(&src, &dest).is_ok() as i8
}

// ---------- HTTP/进程/目录/路径 内部实现（两路径共用） ----------

/// 手写 HTTP/1.1 GET：仅 http://（https 留待 reqwest）。返回正文**原始字节**或 Err。
///
/// 正文按字节切分返回（不经过 UTF-8 有损转换）——http_get_file 下载二进制包
/// （tar.gz/zip）依赖这一点；http_get（文本接口）由调用方再转字符串。
fn http_get_impl(url: &str) -> Result<Vec<u8>, ()> {
    use std::io::{Read, Write};
    if !url.starts_with("http://") {
        return Err(());
    }
    let rest = &url["http://".len()..];
    // 拆 host[:port] 与 path
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match host_port.find(':') {
        Some(i) => (
            &host_port[..i],
            host_port[i + 1..].parse::<u16>().unwrap_or(80),
        ),
        None => (host_port, 80),
    };
    let mut stream = std::net::TcpStream::connect((host, port)).map_err(|_| ())?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).map_err(|_| ())?;
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).map_err(|_| ())?;
    // 状态行需为 200（HTTP/1.1 200 / HTTP/1.0 200）：按字节读 ASCII 状态行
    if !(resp.starts_with(b"HTTP/1.1 200") || resp.starts_with(b"HTTP/1.0 200")) {
        return Err(());
    }
    // 取响应头之后的正文字节（\r\n\r\n 分隔；字节级定位，正文原样返回）
    match resp.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => Ok(resp[i + 4..].to_vec()),
        None => Err(()),
    }
}

/// 跨平台执行命令：Windows 用 cmd /C，其他平台用 sh -c。
#[cfg(windows)]
fn exec_cmd(cmd: &str) -> Option<std::process::ExitStatus> {
    std::process::Command::new("cmd").args(["/C", cmd]).status().ok()
}
#[cfg(not(windows))]
fn exec_cmd(cmd: &str) -> Option<std::process::ExitStatus> {
    std::process::Command::new("sh").args(["-c", cmd]).status().ok()
}

/// 跨平台执行命令并捕获 stdout（stderr 透传；启动失败返回空串）。
#[cfg(windows)]
fn exec_output_impl(cmd: &str) -> String {
    std::process::Command::new("cmd")
        .args(["/C", cmd])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}
#[cfg(not(windows))]
fn exec_output_impl(cmd: &str) -> String {
    std::process::Command::new("sh")
        .args(["-c", cmd])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// 递归复制目录（src 的全部内容 → dest；自动创建 dest 与子目录）。
fn copy_dir_impl(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_p = entry.path();
        let dest_p = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_impl(&src_p, &dest_p)?;
        } else if ty.is_file() {
            std::fs::copy(&src_p, &dest_p)?;
        }
    }
    Ok(())
}

/// 递归收集目录下全部文件的相对路径（相对于 base）。
fn walk_dir_impl(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                walk_dir_impl(&p, base, out);
            } else if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Ok(rel) = p.strip_prefix(base) {
                    out.push(rel.to_string_lossy().into_owned());
                }
            }
        }
    }
}

/// 规范化路径：解析 . 与 ..、合并重复分隔符、Windows 反斜杠统一为系统分隔符。
fn normalize_path(p: &str) -> String {
    let path = std::path::Path::new(p);
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // 忽略「根/前缀上的 ..」；否则弹出一段（若存在非 .. 段）
                if matches!(
                    parts.last(),
                    Some(_) if !parts.last().unwrap().to_string_lossy().eq("..")
                ) {
                    parts.pop();
                }
            }
            _ => parts.push(comp.as_os_str().to_os_string()),
        }
    }
    if parts.is_empty() {
        return ".".to_string();
    }
    let joined: std::path::PathBuf = parts.iter().collect();
    joined.to_string_lossy().into_owned()
}

// ---------- P1 正则表达式 floor 内置函数 C ABI 桥 ----------
//
// 设计说明：正则引擎由 Rust regex crate 提供（RE2 风格、无回溯、UTF-8 感知），
// tie 语言无法自举完整正则引擎，属语言底座原语（与 file_read/str_char 同一层级）。
// 编译路径（IR 层）与解释路径（tie-interp）**共用同一份 Rust 实现**，行为逐字节一致。
//
// 错误约定：模式非法（Regex::new 失败）——
// - tie_regex_match：置 ok=0（合法时 ok=1，返回匹配结果 0/1）；
// - 其余返回堆串/表的：返回 NULL，由调用方统一输出错误消息（两路径消息一致）。

/// C ABI 桥：正则匹配（s 中是否存在 pattern 的匹配，部分匹配即可）。
/// 模式合法 → ok=1 并返回 0/1；模式非法 → ok=0 返回 0。
#[unsafe(no_mangle)]
pub extern "C" fn tie_regex_match(s: *const c_char, pattern: *const c_char, ok: *mut i8) -> i8 {
    let s = match unsafe { c_char_to_string(s) } {
        Ok(v) => v,
        Err(_) => {
            unsafe { *ok = 0; }
            return 0;
        }
    };
    let pattern = match unsafe { c_char_to_string(pattern) } {
        Ok(v) => v,
        Err(_) => {
            unsafe { *ok = 0; }
            return 0;
        }
    };
    match regex::Regex::new(&pattern) {
        Ok(re) => {
            unsafe { *ok = 1; }
            if re.is_match(&s) { 1 } else { 0 }
        }
        Err(_) => {
            unsafe { *ok = 0; }
            0
        }
    }
}

/// C ABI 桥：正则查找——返回 s 中第一个匹配片段；无匹配返回空串；模式非法返回 NULL。
#[unsafe(no_mangle)]
pub extern "C" fn tie_regex_find(s: *const c_char, pattern: *const c_char) -> *mut c_char {
    let s = match unsafe { c_char_to_string(s) } {
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    };
    let pattern = match unsafe { c_char_to_string(pattern) } {
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    };
    match regex::Regex::new(&pattern) {
        Ok(re) => match re.find(&s) {
            Some(m) => string_to_c_char(m.as_str().to_string()),
            None => string_to_c_char(String::new()),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

/// C ABI 桥：正则查找全部——返回所有匹配片段组成的字符串动态表；模式非法返回 NULL。
#[unsafe(no_mangle)]
pub extern "C" fn tie_regex_find_all(s: *const c_char, pattern: *const c_char) -> *mut DynTable {
    let s = match unsafe { c_char_to_string(s) } {
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    };
    let pattern = match unsafe { c_char_to_string(pattern) } {
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    };
    match regex::Regex::new(&pattern) {
        Ok(re) => {
            let t = tie_table_new(8);
            if t.is_null() {
                return t;
            }
            for m in re.find_iter(&s) {
                // 泄漏堆串作为表的借用元素（与 tie_list_dir 同一约定，调用方不释放）
                let c = CString::new(m.as_str()).unwrap_or_default();
                tie_table_push_string(t, c.into_raw());
            }
            t
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// C ABI 桥：正则替换——把 s 中所有 pattern 匹配替换为 to；模式非法返回 NULL。
/// to 支持捕获引用（$1、$name），与 Rust regex replace_all 语义一致。
#[unsafe(no_mangle)]
pub extern "C" fn tie_regex_replace(
    s: *const c_char,
    pattern: *const c_char,
    to: *const c_char,
) -> *mut c_char {
    let s = match unsafe { c_char_to_string(s) } {
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    };
    let pattern = match unsafe { c_char_to_string(pattern) } {
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    };
    let to = match unsafe { c_char_to_string(to) } {
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    };
    match regex::Regex::new(&pattern) {
        Ok(re) => string_to_c_char(re.replace_all(&s, to.as_str()).into_owned()),
        Err(_) => std::ptr::null_mut(),
    }
}

/// C ABI 桥：正则捕获组——返回 s 中第一个匹配的第 i 个捕获组（i=0 为整个匹配）；
/// 无匹配或组号越界返回空串；模式非法返回 NULL。
#[unsafe(no_mangle)]
pub extern "C" fn tie_regex_group(s: *const c_char, pattern: *const c_char, i: i64) -> *mut c_char {
    let s = match unsafe { c_char_to_string(s) } {
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    };
    let pattern = match unsafe { c_char_to_string(pattern) } {
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    };
    match regex::Regex::new(&pattern) {
        Ok(re) => match re.captures(&s) {
            Some(caps) => match caps.get(i as usize) {
                Some(m) => string_to_c_char(m.as_str().to_string()),
                None => string_to_c_char(String::new()),
            },
            None => string_to_c_char(String::new()),
        },
        Err(_) => std::ptr::null_mut(),
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

    /// 调用已注册的用户函数（顶层裸名或命名空间全名），传入一个字符串参数，返回其返回值字符串。
    ///
    /// 设计：tie:script 预处理模块协议的执行基础——模块约定入口函数
    /// `func process(src: string) -> string`，框架先 eval 模块文件注册函数，
    /// 再 eval_call 以**字符串值**直传源码（不经源码文本转义），拿回处理结果。
    /// 无返回值（void）→ 返回空串；函数不存在/参数不符 → 报错。
    pub fn eval_call(&mut self, name: &str, arg: &str) -> Result<String, String> {
        let f = self
            .funcs
            .get(name)
            .cloned()
            .ok_or_else(|| format!("eval_call: 未定义的函数 '{name}'"))?;
        // 参数检查：期望恰好 1 个参数（模块入口约定），且为字符串
        let required = f.params.iter().take_while(|p| p.default.is_none()).count();
        if required > 1 || f.params.len() < 1 {
            return Err(format!(
                "eval_call: 函数 '{name}' 必须恰好接收 1 个字符串参数（实际形参 {} 个）",
                f.params.len()
            ));
        }
        // 函数体执行期间，裸调用按本函数命名空间前缀补全（与 call_fn 用户分支同机制）
        let mut env = Env::new(self);
        let saved_ns = std::mem::take(&mut env.cur_ns);
        if name.contains("::") {
            let mut segs: Vec<String> = name.split("::").map(|s| s.to_string()).collect();
            segs.pop(); // 去掉函数名，剩命名空间路径
            env.cur_ns = segs;
        }
        // 压新作用域绑定参数：第一个参数绑定字符串值，其余（可选）用默认值补齐
        env.scopes.push(std::collections::HashMap::new());
        for (i, p) in f.params.iter().enumerate() {
            let v = if i == 0 {
                Value::Str(arg.to_string())
            } else if let Some(d) = &p.default {
                env.eval_expr(d)?
            } else {
                return Err(format!("eval_call: 函数 '{name}' 缺少第 {} 个参数且无默认值", i + 1));
            };
            env.scopes.last_mut().unwrap().insert(p.name.clone(), v);
        }
        // 函数作用域从参数作用域开始（被调函数内声明不污染调用者，跨函数隔离）
        env.scope_base = env.scopes.len() - 1;
        let result = env.exec_block(&f.body);
        env.scopes.pop();
        env.cur_ns = saved_ns;
        // 处理 return 传播
        match result? {
            Flow::Normal(Some(v)) => Ok(v.to_repl_string()),
            Flow::Normal(None) => Ok(String::new()),
            Flow::Return(v) => Ok(v.to_repl_string()),
        }
    }

    /// 注册顶层定义（func → funcs；顶层持久变量 → globals；class/import → v1 暂不支持）。
    ///
    /// 命名空间（Namespace）递归注册：体内函数以全名（路径段::函数名）进 funcs，
    /// 使 `tcmsg::error.no_file(...)` 路径调用与命名空间内裸调用都能命中。
    /// M4 顶层 var/const：求值初始化后存入 globals（跨函数共享的可变状态）。
    fn register_top_level(&mut self, program: tie_frontend::ast::Program) -> Result<String, String> {
        let mut count = 0;
        for stmt in &program.stmts {
            match stmt {
                Stmt::FnDef(f) => {
                    self.funcs.insert(f.name.clone(), f.clone());
                    count += 1;
                }
                Stmt::Namespace(ns) => {
                    count += self.register_ns_funcs(&ns.body, &ns.path)?;
                }
                Stmt::VarDecl(v) => {
                    // 顶层持久变量：求值初始化后存入会话 globals（跨函数共享）
                    let mut env = Env::new(self);
                    let val = env.eval_expr(&v.init)?;
                    env.session.globals.insert(v.name.clone(), val);
                }
                Stmt::Struct(_) => return Err("REPL v1 暂不支持 struct 定义".into()),
                Stmt::Import(_) => return Err("REPL v1 暂不支持 import".into()),
                Stmt::Using(_) => return Err("REPL v1 暂不支持 using".into()),
                _ => return Err("顶层只允许函数/类/import/using/命名空间/全局变量定义".into()),
            }
        }
        Ok(format!("已定义 {count} 个函数"))
    }

    /// 递归注册命名空间体内函数（全名 = 当前路径::函数名），支持嵌套命名空间。
    fn register_ns_funcs(&mut self, stmts: &[Stmt], prefix: &[String]) -> Result<usize, String> {
        let mut count = 0;
        for stmt in stmts {
            match stmt {
                Stmt::FnDef(f) => {
                    let mut segs = prefix.to_vec();
                    segs.push(f.name.clone());
                    let full = segs.join("::");
                    self.funcs.insert(full, f.clone());
                    count += 1;
                }
                Stmt::Namespace(inner) => {
                    let mut segs = prefix.to_vec();
                    segs.extend(inner.path.iter().cloned());
                    count += self.register_ns_funcs(&inner.body, &segs)?;
                }
                _ => {}
            }
        }
        Ok(count)
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
    /// 当前执行函数的命名空间前缀（空 = 顶层函数）。
    /// 命名空间内裸调用（如 tcmsg::error 内 helper()）据此补全为全名
    /// （helper → tcmsg::error::helper）；由 call_fn 执行函数体时设置/恢复。
    cur_ns: Vec<String>,
    /// 当前函数的作用域栈起点（scope_base.. 属于当前函数）。
    ///
    /// 为什么需要：函数 A 调用函数 B 时，B 的局部变量声明不应与 A 的同名
    /// 局部变量冲突（跨函数隔离）。lookup/assign/is_declared 只访问
    /// `scopes[scope_base..]`，函数调用压参后 base 指向参数作用域。
    /// REPL 顶层为 0（整个栈都属于顶层）。
    scope_base: usize,
}

impl<'a> Env<'a> {
    fn new(session: &'a mut Session) -> Self {
        Self { session, scopes: Vec::new(), cur_ns: Vec::new(), scope_base: 0 }
    }

    /// 变量查找：当前函数作用域栈（scope_base 起）→ 顶层 globals。
    fn lookup(&self, name: &str) -> Option<Value> {
        for scope in self.scopes[self.scope_base..].iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v.clone());
            }
        }
        self.session.globals.get(name).cloned()
    }

    /// 变量赋值：当前函数作用域（scope_base 起）内找到则改，否则写顶层 globals。
    fn assign(&mut self, name: &str, value: Value) -> Result<(), String> {
        for scope in self.scopes[self.scope_base..].iter_mut().rev() {
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

    /// 变量是否已声明（当前函数作用域 scope_base 起，或顶层）。
    fn is_declared(&self, name: &str) -> bool {
        self.scopes[self.scope_base..]
            .iter()
            .any(|s| s.contains_key(name))
            || self.session.globals.contains_key(name)
    }

    /// 执行一条语句。
    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Flow, String> {
        match stmt {
            Stmt::VarDecl(v) => {
                let mut val = self.eval_expr(&v.init)?;
                // 平衡三进制字面量适配（M4 补齐）：标注 trit 且初始化是 bool 字面量时
                // 转换为 trit（true→+1 / false→-1）——与编译路径的字面量适配语义一致。
                if matches!(v.ty, Some(TypeSpec::Named(TyKw::Trit)))
                    && let Value::Bool(b) = &val
                {
                    val = Value::Trit(if *b { 1 } else { -1 });
                }
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
                    // 表遍历：`for x in t`——逐元素绑定循环变量（定长字面量表与动态表
                    // 在解释器里都是 Value::Table(Vec)，统一处理）。
                    Value::Table(cells) => {
                        for item in cells {
                            self.scopes.push(std::collections::HashMap::new());
                            self.scopes.last_mut().unwrap().insert(f.var.clone(), item);
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
            Stmt::Switch(s) => {
                // switch 模式匹配（与编译路径语义一致）：
                // 顺序匹配每个 case —— 任一 pattern 命中 且 when 守卫为真才执行；
                // 全不匹配 → default（可省略）。
                let subject = self.eval_expr(&s.subject)?;
                let mut last = None;
                let mut matched = false;
                for c in &s.cases {
                    // 逐个 pattern 匹配（多值：任一命中即本 case 命中）
                    let mut hit = false;
                    for pat in &c.patterns {
                        match pat {
                            // 区间 pattern：start ≤ subject < end（左闭右开，整数/字符）
                            Expr::Range { start, end, .. } => {
                                let sv = match self.eval_expr(start)? {
                                    Value::Int(n) => n,
                                    Value::Char(ch) => ch as i64,
                                    _ => return Err("case 区间起点必须是整数或字符".into()),
                                };
                                let ev = match self.eval_expr(end)? {
                                    Value::Int(n) => n,
                                    Value::Char(ch) => ch as i64,
                                    _ => return Err("case 区间终点必须是整数或字符".into()),
                                };
                                // subject 转数值比较（整数/字符）
                                let v = match &subject {
                                    Value::Int(n) => *n,
                                    Value::Char(ch) => *ch as i64,
                                    _ => continue, // 类型不匹配 → 不命中
                                };
                                if v >= sv && v < ev {
                                    hit = true;
                                    break;
                                }
                            }
                            // 类型匹配 pattern：按 Value 变体匹配（动态类型）
                            Expr::TypeLit { ty, .. } => {
                                if value_matches_ty(&subject, ty) {
                                    hit = true;
                                    break;
                                }
                            }
                            // 字面量 pattern：== 比较（字符串/数字/字符/布尔）
                            _ => {
                                let pv = self.eval_expr(pat)?;
                                if let Value::Bool(b) = self.eval_binary(BinaryOp::Eq, subject.clone(), pv)? {
                                    if b {
                                        hit = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if !hit {
                        continue;
                    }
                    // when 守卫：值为真才进入（守卫不满足 → 落入下一个 case）
                    if let Some(w) = &c.when {
                        if !self.eval_expr(w)?.is_truthy()? {
                            continue;
                        }
                    }
                    match self.exec_block(&c.body)? {
                        Flow::Normal(v) => last = v,
                        flow @ Flow::Return(_) => return Ok(flow),
                    }
                    matched = true;
                    break;
                }
                // 全不匹配 → default 分支（可省略）
                if !matched {
                    match self.exec_block(&s.default_body)? {
                        Flow::Normal(v) => last = v,
                        flow @ Flow::Return(_) => return Ok(flow),
                    }
                }
                Ok(Flow::Normal(last))
            }
            Stmt::Import(_) => Err("REPL v1 暂不支持 import".into()),
            Stmt::Using(_) => {
                // using 引入语句：REPL v1 不支持 import，using 亦无意义；
                // 顶层注册路径已报错，函数体内（不应出现）防御性空操作。
                Ok(Flow::Normal(None))
            }
            Stmt::Struct(_) => Err("REPL v1 暂不支持 struct 定义".into()),
            Stmt::FnDef(f) => {
                // 函数体内的嵌套函数定义 → 注册进 funcs（从简）
                self.session.funcs.insert(f.name.clone(), f.clone());
                Ok(Flow::Normal(None))
            }
            Stmt::Namespace(_) => {
                // 命名空间声明：REPL 顶层已由 register_top_level 递归注册，
                // 函数体内（不应出现）防御性空操作。
                Ok(Flow::Normal(None))
            }
            Stmt::FieldAssign(_) => Err("REPL v1 暂不支持字段赋值（类）".into()),
            // 表下标赋值（M4 补齐）：`t[i] = v` / `t[i] += v`。
            // target 是 Index 链：从最外层表变量逐层下钻到目标元素，写回。
            Stmt::IndexAssign(ia) => {
                let (target_name, mut cells, idx) = self.resolve_index_target(&ia.target)?;
                // 越界写：负数或超出长度 → 报错（与读取越界同文本）
                if idx < 0 || idx as usize >= cells.len() {
                    return Err(format!(
                        "运行时错误: table_at 下标越界：索引 {idx} 超出长度 {}",
                        cells.len()
                    ));
                }
                let ui = idx as usize;
                // 读旧值（复合赋值用）；普通赋值直接求新值
                let new_val = match ia.op {
                    Some(op) => {
                        let old = cells[ui].clone();
                        let rv = self.eval_expr(&ia.value)?;
                        self.eval_binary(op, old, rv)?
                    }
                    None => self.eval_expr(&ia.value)?,
                };
                cells[ui] = new_val;
                self.assign(&target_name, Value::Table(cells))?;
                Ok(Flow::Normal(None))
            }
        }
    }

    /// 解析下标赋值目标 `t[i]`（M4 补齐）：返回 (表变量名, 表元素 Vec, 下标 i64)。
    ///
    /// 首版支持单层 `t[i]`（target 是 Index{base: Var(t), index: i}）。
    fn resolve_index_target(
        &mut self,
        target: &Expr,
    ) -> Result<(String, Vec<Value>, i64), String> {
        let Expr::Index { base, index, .. } = target else {
            return Err("下标赋值的目标必须是表元素访问（t[i]）".into());
        };
        // base 必须是表变量（单层）
        let Expr::Var(name) = base.as_ref() else {
            return Err("下标赋值暂只支持单层表变量（t[i]）；二维表元素暂不支持写".into());
        };
        let cur = self
            .lookup(name)
            .ok_or_else(|| format!("变量 '{name}' 未声明"))?;
        let Value::Table(cells) = cur else {
            return Err(format!("下标赋值的对象必须是表，实际是 {}", cur.type_name()));
        };
        let idx_val = self.eval_expr(index)?;
        let Value::Int(idx) = idx_val else {
            return Err("下标必须是整数".into());
        };
        Ok((name.clone(), cells, idx))
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
            // 平衡三进制 trit 字面量（M4 补齐）
            Expr::TritLit(v) => Ok(Value::Trit(*v)),
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
                        // 平衡三值逻辑非（M4 补齐）：-1↔1，0 保持
                        Value::Trit(t) => Ok(Value::Trit(-t)),
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
            Expr::TypeLit { .. } => Err("类型字面量只能用作 switch 的 case 类型匹配".into()),
            Expr::Call { name, args, .. } => {
                // table_push 需要**原地修改**表变量（Value::Table 是 Vec，按值传参
                // 无法写回作用域），故在 eval_expr 特判：直接对变量做 push 并写回。
                if name == "table_push" {
                    return self.eval_table_push(args);
                }
                let arg_vals = self.eval_args(args)?;
                // 裸调用名解析：先查裸名（顶层/内置函数），查不到则按当前命名空间
                // 前缀补全（命名空间内函数互调，如 tcmsg::error 内 helper()）。
                let resolved = if self.session.funcs.contains_key(name) {
                    name.clone()
                } else if !self.cur_ns.is_empty() {
                    let mut segs = self.cur_ns.clone();
                    segs.push(name.clone());
                    let full = segs.join("::");
                    if self.session.funcs.contains_key(&full) {
                        full
                    } else {
                        name.clone()
                    }
                } else {
                    name.clone()
                };
                self.call_fn(&resolved, arg_vals)
            }
            Expr::Index { base, index, .. } => {
                // 表下标：t[i] 读取第 i 个元素（越界 → 运行时错误，文本与编译路径一致）
                let b = self.eval_expr(base)?;
                let i = self.eval_expr(index)?;
                let Value::Int(i) = i else {
                    return Err("下标必须是整数".into());
                };
                match b {
                    Value::Table(cells) => {
                        if i < 0 || i as usize >= cells.len() {
                            return Err(format!(
                                "运行时错误: 下标越界：索引 {i} 超出长度 {}",
                                cells.len()
                            ));
                        }
                        Ok(cells[i as usize].clone())
                    }
                    _ => Err("下标访问仅支持表".into()),
                }
            }
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
            Expr::MethodCall { receiver, method, args, .. } => {
                // 命名空间函数调用：receiver 是路径（tcmsg::error）、未绑定变量
                // （tcmsg，单段）或 FieldAccess 链（tcmsg.error，点分），方法名即
                // 函数名 → 全名 tcmsg::error::no_file，与编译路径（resolved_calls）一致。
                let ns_prefix: Option<Vec<String>> = match receiver.as_ref() {
                    Expr::Path { segments, .. } => Some(segments.clone()),
                    Expr::Var(rname) if !self.is_declared(rname) => {
                        // 单段命名空间：funcs 中存在 `rname::` 前缀键即视为命名空间，
                        // 无条件按全名调用（全名缺失时由 call_fn 报"未定义的函数"）。
                        if self.has_ns_prefix(&[rname.clone()]) {
                            Some(vec![rname.clone()])
                        } else {
                            None
                        }
                    }
                    Expr::FieldAccess { .. } => {
                        // 点分命名空间链：递归拍平 receiver 为路径段，
                        // 存在该前缀的命名空间函数即视为命名空间调用。
                        if let Some(segs) = self.ns_segments(receiver) {
                            if self.has_ns_prefix(&segs) {
                                Some(segs)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some(segs) = ns_prefix {
                    let mut segs = segs;
                    segs.push(method.clone());
                    let full = segs.join("::");
                    let arg_vals = self.eval_args(args)?;
                    return self.call_fn(&full, arg_vals);
                }
                // 其余方法调用（struct 实例转发）：REPL v1 暂不支持 struct 值
                Err("REPL v1 暂不支持 struct 方法调用".into())
            }
            // 命名空间路径独立出现：只能作调用 receiver，解释器防御报错
            Expr::Path { segments, .. } => Err(format!(
                "命名空间路径 '{}' 不能作为值使用（只能用于调用）",
                segments.join("::")
            )),
        }
    }

    /// 求值实参列表。
    fn eval_args(&mut self, args: &[Expr]) -> Result<Vec<Value>, String> {
        args.iter().map(|a| self.eval_expr(a)).collect()
    }

    /// 命名空间路径段提取（解释层版）：把 `tcmsg.error` 的 FieldAccess 链/Var
    /// 递归拍平为 ["tcmsg","error"]。条件：每个标识符都未声明（非变量）。
    fn ns_segments(&self, expr: &Expr) -> Option<Vec<String>> {
        match expr {
            Expr::Var(name) if !self.is_declared(name) => {
                Some(vec![name.clone()])
            }
            Expr::FieldAccess { base, field, .. } => {
                let mut segs = self.ns_segments(base)?;
                segs.push(field.clone());
                Some(segs)
            }
            _ => None,
        }
    }

    /// 命名空间前缀存在判定：funcs 中是否存在以 `路径段::` 开头的函数键。
    /// 用于把「未绑定 Var/FieldAccess 链」识别为命名空间（如 tcmsg.hello() 的
    /// tcmsg 是命名空间而非变量）。存在即视为命名空间，即使目标全名未注册
    /// 也走命名空间调用路径（由 call_fn 产生"未定义的函数"错误）。
    fn has_ns_prefix(&self, segs: &[String]) -> bool {
        let prefix = format!("{}::", segs.join("::"));
        self.session.funcs.keys().any(|k| k.starts_with(&prefix))
    }

    /// 动态表追加：`table_push(t, x)`——对表变量 t 原地 push 元素 x 并写回作用域。
    ///
    /// 与编译路径一致：第 1 个参数必须是表变量（Value::Table），元素类型由语义层
    /// 静态校验（解释器动态求值，此处只做运行时类型检查）。
    fn eval_table_push(&mut self, args: &[Expr]) -> Result<Value, String> {
        if args.len() != 2 {
            return Err("table_push 需要表与元素参数".into());
        }
        let Expr::Var(name) = &args[0] else {
            return Err("table_push 的第 1 个参数必须是表变量".into());
        };
        let val = self.eval_expr(&args[1])?;
        let cur = self
            .lookup(name)
            .ok_or_else(|| format!("变量 '{name}' 未声明"))?;
        let Value::Table(mut cells) = cur else {
            return Err("table_push 的第 1 个参数必须是表".into());
        };
        cells.push(val);
        self.assign(name, Value::Table(cells))?;
        Ok(Value::Void)
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
            // ---------- M4 补齐：平衡三进制 trit 运算 ----------
            //
            // Kleene 三值逻辑（-1=false, 0=unknown, +1=true）：
            //   &&  = min（任一 false 则 false，否则都非 false 时取较小真值）
            //   ||  = max（任一 true 则 true）
            //   !   = 取反（-1↔1，0 保持）——见 eval_unary
            // 算术（饱和/clamp 到 [-1,1]）：trit ± * trit → trit；
            // 混合 trit × i64 → i64（sext 提升）；比较 trit vs trit/i64 → bool。
            (Value::Trit(a), Value::Trit(b)) => match op {
                BinaryOp::And => Ok(Value::Trit(a.min(b))),
                BinaryOp::Or => Ok(Value::Trit(a.max(b))),
                // i8 算术会溢出（debug panic），提升 i64 再饱和截断
                BinaryOp::Add => Ok(Value::Trit(clamp_trit(a as i64 + b as i64))),
                BinaryOp::Sub => Ok(Value::Trit(clamp_trit(a as i64 - b as i64))),
                BinaryOp::Mul => Ok(Value::Trit(clamp_trit(a as i64 * b as i64))),
                BinaryOp::Eq => Ok(Value::Bool(a == b)),
                BinaryOp::NotEq => Ok(Value::Bool(a != b)),
                BinaryOp::Lt => Ok(Value::Bool(a < b)),
                BinaryOp::Gt => Ok(Value::Bool(a > b)),
                BinaryOp::Le => Ok(Value::Bool(a <= b)),
                BinaryOp::Ge => Ok(Value::Bool(a >= b)),
                BinaryOp::Div | BinaryOp::Mod => Err("trit 不支持除/取模运算（三值无除法）".into()),
                BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl
                | BinaryOp::Shr => Err("位运算/移位只支持整数".into()),
            },
            // trit × i64 混合：提升为 i64 算术（与编译路径 sext 一致）
            (Value::Trit(a), Value::Int(b)) => eval_trit_int(op, a, b),
            (Value::Int(a), Value::Trit(b)) => eval_trit_int_rev(op, a, b),
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
            // ---------- 动态表构造/读取内置函数 ----------
            //
            // 解释路径用 Value::Table(Vec) 表示表（不经过 C ABI 桥的堆结构体），
            // 但行为与编译路径一致：新建空表 → push 追加 → len 取运行时长度 →
            // table_at 读取（越界报同一句中文错误，文本与 IR 层 table_at 完全一致）。
            "table_new_i64" | "table_new_f64" | "table_new_string" | "table_new_bool" => {
                if !args.is_empty() {
                    return Err(format!("{name} 不需要参数"));
                }
                Ok(Value::Table(Vec::new()))
            }
            "table_at" => {
                if args.len() != 2 {
                    return Err("table_at 需要表与整数下标参数".into());
                }
                let (Value::Table(cells), Value::Int(i)) = (&args[0], &args[1]) else {
                    return Err("table_at 需要表与整数下标参数".into());
                };
                if *i < 0 || *i as usize >= cells.len() {
                    return Err(format!(
                        "运行时错误: table_at 下标越界：索引 {i} 超出长度 {}",
                        cells.len()
                    ));
                }
                Ok(cells[*i as usize].clone())
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
            "eval_call" => {
                // 调用已注册用户函数（字符串参数）→ 返回值字符串（void → 空串）。
                // tie:script 模块协议执行基础：与 C ABI 桥 tie_eval_call 同语义。
                if args.len() != 2 {
                    return Err("eval_call 需要两个字符串参数（函数名, 参数）".into());
                }
                let (Value::Str(name), Value::Str(arg)) = (&args[0], &args[1]) else {
                    return Err("eval_call 需要两个字符串参数（函数名, 参数）".into());
                };
                let result = self.session.eval_call(name, arg)?;
                Ok(Value::Str(result))
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
            "file_delete" => {
                // 删除文件：成功返回 true，失败（不存在/不可删）返回 false。
                // 与 file_write/file_append 同模式：解释路径用 std::fs，
                // 编译路径用 libc remove()，两者行为一致（均返回 bool、无错误消息）。
                if args.len() != 1 {
                    return Err("file_delete 需要一个字符串参数".into());
                }
                let Value::Str(path) = &args[0] else {
                    return Err("file_delete 需要一个字符串参数".into());
                };
                Ok(Value::Bool(std::fs::remove_file(path).is_ok()))
            }
            // ---------- 消息系统（#25）内置函数 ----------
            //
            // 与编译路径一致：走 C ABI 桥 tie_msg_*（共用同一份 thread_local 状态），
            // 保证两路径消息注册/查询行为逐字节一致（含回退 zh / 回退键本身）。
            "msg_set_lang" => {
                if args.len() != 1 {
                    return Err("msg_set_lang 需要一个字符串参数".into());
                }
                let Value::Str(lang) = &args[0] else {
                    return Err("msg_set_lang 需要一个字符串参数".into());
                };
                let l = CString::new(lang.as_str()).unwrap_or_default();
                tie_msg_set_lang(l.as_ptr());
                Ok(Value::Void)
            }
            "msg_get_lang" => {
                if !args.is_empty() {
                    return Err("msg_get_lang 不需要参数".into());
                }
                // 读取当前语言（堆串，用完释放，与 msg_t 同机制）
                let p = tie_msg_get_lang();
                let s = unsafe { c_char_to_string(p).unwrap_or_default() };
                tie_free_result(p);
                Ok(Value::Str(s))
            }
            "msg_register" => {
                if args.len() != 3 {
                    return Err("msg_register 需要三个字符串参数（键, 语言, 文本）".into());
                }
                let (Value::Str(key), Value::Str(lang), Value::Str(text)) =
                    (&args[0], &args[1], &args[2])
                else {
                    return Err("msg_register 需要三个字符串参数（键, 语言, 文本）".into());
                };
                let k = CString::new(key.as_str()).unwrap_or_default();
                let l = CString::new(lang.as_str()).unwrap_or_default();
                let x = CString::new(text.as_str()).unwrap_or_default();
                tie_msg_register(k.as_ptr(), l.as_ptr(), x.as_ptr());
                Ok(Value::Void)
            }
            "msg_t" => {
                if args.len() != 1 {
                    return Err("msg_t 需要一个字符串参数".into());
                }
                let Value::Str(key) = &args[0] else {
                    return Err("msg_t 需要一个字符串参数".into());
                };
                let k = CString::new(key.as_str()).unwrap_or_default();
                // 返回堆串，用完必须释放（与 file_read 同机制）
                let p = tie_msg_t(k.as_ptr());
                let s = unsafe { c_char_to_string(p).unwrap_or_default() };
                tie_free_result(p);
                Ok(Value::Str(s))
            }
            // ---------- 消息系统增强（M4）内置函数 ----------
            // 与编译路径一致：走 C ABI 桥。M4 起消息系统的级别/回退链状态由 tie 语言
            // 自身用顶层持久变量表达（纯 tie），本桥只保留 I/O 通道（print_err）与
            // 指定语言查询（msg_t_lang）。
            "print_err" => {
                if args.len() != 1 {
                    return Err("print_err 需要一个字符串参数".into());
                }
                let Value::Str(s) = &args[0] else {
                    return Err("print_err 需要一个字符串参数".into());
                };
                let c = CString::new(s.as_str()).unwrap_or_default();
                tie_print_err(c.as_ptr());
                Ok(Value::Void)
            }
            "msg_t_lang" => {
                if args.len() != 2 {
                    return Err("msg_t_lang 需要两个字符串参数（键, 语言）".into());
                }
                let (Value::Str(key), Value::Str(lang)) = (&args[0], &args[1]) else {
                    return Err("msg_t_lang 需要两个字符串参数（键, 语言）".into());
                };
                let k = CString::new(key.as_str()).unwrap_or_default();
                let l = CString::new(lang.as_str()).unwrap_or_default();
                // 返回堆串，用完必须释放（与 msg_t 同机制）
                let p = tie_msg_t_lang(k.as_ptr(), l.as_ptr());
                let s = unsafe { c_char_to_string(p).unwrap_or_default() };
                tie_free_result(p);
                Ok(Value::Str(s))
            }
            // 目录类：list_dir 走 C ABI 桥（与编译路径共用 std::fs::read_dir），
            // 返回字符串动态表（文件名集合）；目录无效 → 报错（错误文本与编译路径一致）。
            // 表在解释器里是 Value::Table(Vec)：读出桥表的全部字符串条目后调用
            // tie_table_free 释放「指针数组 + 表结构」；字符串元素按借用约定由桥泄漏
            //（与 table_push_string 一致，调用方不释放），因此读出的 Value::Str 是拷贝。
            "list_dir" => {
                if args.len() != 1 {
                    return Err("list_dir 需要一个字符串参数".into());
                }
                let Value::Str(path) = &args[0] else {
                    return Err("list_dir 需要一个字符串参数".into());
                };
                let p = CString::new(path.as_str()).unwrap_or_default();
                let t = tie_list_dir(p.as_ptr());
                // 失败（NULL）→ 报错（错误消息与编译路径 printf 文本一致）
                if t.is_null() {
                    return Err(format!("运行时错误: list_dir 无法读取目录 '{path}'"));
                }
                // 读出全部条目（元素类型 string，data 缓冲区为 *const c_char 数组）
                let mut cells: Vec<Value> = Vec::new();
                unsafe {
                    let tbl = &*t;
                    for i in 0..tbl.len {
                        let ptr = (tbl.data as *const *const c_char).add(i as usize).read();
                        cells.push(Value::Str(c_char_to_string(ptr).unwrap_or_default()));
                    }
                }
                // 释放桥表（指针数组 + 结构体；字符串元素按约定泄漏，不重复释放）
                tie_table_free(t);
                Ok(Value::Table(cells))
            }
            // ---------- M4 补齐：系统能力内置函数（M6 包管理器前置） ----------
            //
            // 与编译路径一致：走 C ABI 桥（共用同一份 Rust 实现），保证两路径行为逐字节一致。
            // 分类：返回堆串（http_get/exec_output/path_*/cwd/get_env）调用后释放；
            // 返回 bool（i8：http_get_file/untar_gz/unzip/mkdir_all/remove_dir_all/
            // copy_dir/file_copy/file_move）；返回 i64（exec_code）；void（set_env）；
            // 字符串动态表（walk_dir，复用 list_dir 的表读出模式）。
            "http_get" => {
                if args.len() != 1 {
                    return Err("http_get 需要一个字符串参数".into());
                }
                let Value::Str(url) = &args[0] else {
                    return Err("http_get 需要一个字符串参数".into());
                };
                let p = CString::new(url.as_str()).unwrap_or_default();
                let r = tie_http_get(p.as_ptr());
                // 失败（NULL）→ 报错（错误消息与编译路径一致）
                if r.is_null() {
                    return Err(format!("运行时错误: http_get 无法访问 URL '{url}'"));
                }
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "http_get_file" => {
                if args.len() != 2 {
                    return Err("http_get_file 需要两个字符串参数".into());
                }
                let (Value::Str(url), Value::Str(path)) = (&args[0], &args[1]) else {
                    return Err("http_get_file 需要两个字符串参数".into());
                };
                let u = CString::new(url.as_str()).unwrap_or_default();
                let p = CString::new(path.as_str()).unwrap_or_default();
                Ok(Value::Bool(tie_http_get_file(u.as_ptr(), p.as_ptr()) != 0))
            }
            "exec_code" => {
                if args.len() != 1 {
                    return Err("exec_code 需要一个字符串参数".into());
                }
                let Value::Str(cmd) = &args[0] else {
                    return Err("exec_code 需要一个字符串参数".into());
                };
                let c = CString::new(cmd.as_str()).unwrap_or_default();
                Ok(Value::Int(tie_exec_code(c.as_ptr())))
            }
            "exec_output" => {
                if args.len() != 1 {
                    return Err("exec_output 需要一个字符串参数".into());
                }
                let Value::Str(cmd) = &args[0] else {
                    return Err("exec_output 需要一个字符串参数".into());
                };
                let c = CString::new(cmd.as_str()).unwrap_or_default();
                let r = tie_exec_output(c.as_ptr());
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "untar_gz" => {
                if args.len() != 2 {
                    return Err("untar_gz 需要两个字符串参数".into());
                }
                let (Value::Str(file), Value::Str(dest)) = (&args[0], &args[1]) else {
                    return Err("untar_gz 需要两个字符串参数".into());
                };
                let f = CString::new(file.as_str()).unwrap_or_default();
                let d = CString::new(dest.as_str()).unwrap_or_default();
                Ok(Value::Bool(tie_untar_gz(f.as_ptr(), d.as_ptr()) != 0))
            }
            "unzip" => {
                if args.len() != 2 {
                    return Err("unzip 需要两个字符串参数".into());
                }
                let (Value::Str(file), Value::Str(dest)) = (&args[0], &args[1]) else {
                    return Err("unzip 需要两个字符串参数".into());
                };
                let f = CString::new(file.as_str()).unwrap_or_default();
                let d = CString::new(dest.as_str()).unwrap_or_default();
                Ok(Value::Bool(tie_unzip(f.as_ptr(), d.as_ptr()) != 0))
            }
            "mkdir_all" => {
                if args.len() != 1 {
                    return Err("mkdir_all 需要一个字符串参数".into());
                }
                let Value::Str(p) = &args[0] else {
                    return Err("mkdir_all 需要一个字符串参数".into());
                };
                let p = CString::new(p.as_str()).unwrap_or_default();
                Ok(Value::Bool(tie_mkdir_all(p.as_ptr()) != 0))
            }
            "remove_dir_all" => {
                if args.len() != 1 {
                    return Err("remove_dir_all 需要一个字符串参数".into());
                }
                let Value::Str(p) = &args[0] else {
                    return Err("remove_dir_all 需要一个字符串参数".into());
                };
                let p = CString::new(p.as_str()).unwrap_or_default();
                Ok(Value::Bool(tie_remove_dir_all(p.as_ptr()) != 0))
            }
            "copy_dir" => {
                if args.len() != 2 {
                    return Err("copy_dir 需要两个字符串参数".into());
                }
                let (Value::Str(src), Value::Str(dest)) = (&args[0], &args[1]) else {
                    return Err("copy_dir 需要两个字符串参数".into());
                };
                let s = CString::new(src.as_str()).unwrap_or_default();
                let d = CString::new(dest.as_str()).unwrap_or_default();
                Ok(Value::Bool(tie_copy_dir(s.as_ptr(), d.as_ptr()) != 0))
            }
            "walk_dir" => {
                if args.len() != 1 {
                    return Err("walk_dir 需要一个字符串参数".into());
                }
                let Value::Str(path) = &args[0] else {
                    return Err("walk_dir 需要一个字符串参数".into());
                };
                let p = CString::new(path.as_str()).unwrap_or_default();
                let t = tie_walk_dir(p.as_ptr());
                // 失败（NULL）→ 报错（错误消息与编译路径一致）
                if t.is_null() {
                    return Err(format!("运行时错误: walk_dir 无法读取目录 '{path}'"));
                }
                // 读出全部条目（元素类型 string，data 缓冲区为 *const c_char 数组）
                let mut cells: Vec<Value> = Vec::new();
                unsafe {
                    let tbl = &*t;
                    for i in 0..tbl.len {
                        let ptr = (tbl.data as *const *const c_char).add(i as usize).read();
                        cells.push(Value::Str(c_char_to_string(ptr).unwrap_or_default()));
                    }
                }
                // 释放桥表（指针数组 + 结构体；字符串元素按约定泄漏，不重复释放）
                tie_table_free(t);
                Ok(Value::Table(cells))
            }
            // ---------- D7：字节流 / 位操作原语（编解码器底座） ----------
            // 与编译路径一致：走 C ABI 桥（共用同一份 Rust 实现）。
            // byte_read 返回 i64 动态表（0..255）；byte_write 收 i64 表写文件；
            // bit_read/bit_write 直接操作字节表（i64 表元素按位读写）；
            // byte_concat 拼接两个字节表。
            "byte_read" => {
                if args.len() != 1 {
                    return Err("byte_read 需要一个字符串参数".into());
                }
                let Value::Str(path) = &args[0] else {
                    return Err("byte_read 需要一个字符串参数".into());
                };
                let p = CString::new(path.as_str()).unwrap_or_default();
                let t = tie_byte_read(p.as_ptr());
                // 失败（NULL）→ 报错
                if t.is_null() {
                    return Err(format!("运行时错误: byte_read 无法读取文件 '{path}'"));
                }
                // 读出全部字节（元素类型 i64）
                let mut cells: Vec<Value> = Vec::new();
                unsafe {
                    let tbl = &*t;
                    for i in 0..tbl.len {
                        let v = (tbl.data as *const i64).add(i as usize).read();
                        cells.push(Value::Int(v));
                    }
                }
                tie_table_free(t);
                Ok(Value::Table(cells))
            }
            "byte_write" => {
                if args.len() != 2 {
                    return Err("byte_write 需要两个参数（路径, 字节表）".into());
                }
                let Value::Str(path) = &args[0] else {
                    return Err("byte_write 第 1 个参数必须是字符串".into());
                };
                // 构造字节表（Value::Table 的元素为 Int，逐个 push 到桥表）
                let t = tie_table_new(8);
                let Value::Table(cells) = &args[1] else {
                    return Err("byte_write 第 2 个参数必须是字节表".into());
                };
                for cell in cells {
                    if let Value::Int(v) = cell {
                        tie_table_push_i64(t, *v);
                    }
                }
                let p = CString::new(path.as_str()).unwrap_or_default();
                let ok = tie_byte_write(p.as_ptr(), t);
                tie_table_free(t);
                Ok(Value::Bool(ok != 0))
            }
            "bit_read" => {
                if args.len() != 2 {
                    return Err("bit_read 需要两个参数（字节表, 位置）".into());
                }
                let Value::Table(cells) = &args[0] else {
                    return Err("bit_read 第 1 个参数必须是字节表".into());
                };
                let Value::Int(pos) = args[1] else {
                    return Err("bit_read 第 2 个参数必须是整数".into());
                };
                let t = tie_table_new(8);
                for cell in cells {
                    if let Value::Int(v) = cell {
                        tie_table_push_i64(t, *v);
                    }
                }
                let r = tie_bit_read(t, pos);
                tie_table_free(t);
                Ok(Value::Int(r))
            }
            "bit_write" => {
                if args.len() != 3 {
                    return Err("bit_write 需要三个参数（字节表, 位置, 位值）".into());
                }
                let Value::Table(cells) = &args[0] else {
                    return Err("bit_write 第 1 个参数必须是字节表".into());
                };
                let Value::Int(pos) = args[1] else {
                    return Err("bit_write 第 2 个参数必须是整数".into());
                };
                let Value::Int(bit) = args[2] else {
                    return Err("bit_write 第 3 个参数必须是整数".into());
                };
                let t = tie_table_new(8);
                for cell in cells {
                    if let Value::Int(v) = cell {
                        tie_table_push_i64(t, *v);
                    }
                }
                let ok = tie_bit_write(t, pos, bit);
                tie_table_free(t);
                Ok(Value::Bool(ok != 0))
            }
            "byte_concat" => {
                if args.len() != 2 {
                    return Err("byte_concat 需要两个字节表参数".into());
                }
                let Value::Table(cells_a) = &args[0] else {
                    return Err("byte_concat 第 1 个参数必须是字节表".into());
                };
                let Value::Table(cells_b) = &args[1] else {
                    return Err("byte_concat 第 2 个参数必须是字节表".into());
                };
                let ta = tie_table_new(8);
                for cell in cells_a {
                    if let Value::Int(v) = cell {
                        tie_table_push_i64(ta, *v);
                    }
                }
                let tb = tie_table_new(8);
                for cell in cells_b {
                    if let Value::Int(v) = cell {
                        tie_table_push_i64(tb, *v);
                    }
                }
                let t = tie_byte_concat(ta, tb);
                tie_table_free(ta);
                tie_table_free(tb);
                if t.is_null() {
                    return Err("byte_concat 拼接失败".into());
                }
                let mut cells: Vec<Value> = Vec::new();
                unsafe {
                    let tbl = &*t;
                    for i in 0..tbl.len {
                        let v = (tbl.data as *const i64).add(i as usize).read();
                        cells.push(Value::Int(v));
                    }
                }
                tie_table_free(t);
                Ok(Value::Table(cells))
            }
            "path_join" => {
                if args.len() != 2 {
                    return Err("path_join 需要两个字符串参数".into());
                }
                let (Value::Str(a), Value::Str(b)) = (&args[0], &args[1]) else {
                    return Err("path_join 需要两个字符串参数".into());
                };
                let a = CString::new(a.as_str()).unwrap_or_default();
                let b = CString::new(b.as_str()).unwrap_or_default();
                let r = tie_path_join(a.as_ptr(), b.as_ptr());
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "path_basename" => {
                if args.len() != 1 {
                    return Err("path_basename 需要一个字符串参数".into());
                }
                let Value::Str(p) = &args[0] else {
                    return Err("path_basename 需要一个字符串参数".into());
                };
                let p = CString::new(p.as_str()).unwrap_or_default();
                let r = tie_path_basename(p.as_ptr());
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "path_dirname" => {
                if args.len() != 1 {
                    return Err("path_dirname 需要一个字符串参数".into());
                }
                let Value::Str(p) = &args[0] else {
                    return Err("path_dirname 需要一个字符串参数".into());
                };
                let p = CString::new(p.as_str()).unwrap_or_default();
                let r = tie_path_dirname(p.as_ptr());
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "path_abs" => {
                if args.len() != 1 {
                    return Err("path_abs 需要一个字符串参数".into());
                }
                let Value::Str(p) = &args[0] else {
                    return Err("path_abs 需要一个字符串参数".into());
                };
                let p = CString::new(p.as_str()).unwrap_or_default();
                let r = tie_path_abs(p.as_ptr());
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "path_normalize" => {
                if args.len() != 1 {
                    return Err("path_normalize 需要一个字符串参数".into());
                }
                let Value::Str(p) = &args[0] else {
                    return Err("path_normalize 需要一个字符串参数".into());
                };
                let p = CString::new(p.as_str()).unwrap_or_default();
                let r = tie_path_normalize(p.as_ptr());
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "cwd" => {
                if !args.is_empty() {
                    return Err("cwd 不需要参数".into());
                }
                let r = tie_cwd();
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "get_env" => {
                if args.len() != 1 {
                    return Err("get_env 需要一个字符串参数".into());
                }
                let Value::Str(n) = &args[0] else {
                    return Err("get_env 需要一个字符串参数".into());
                };
                let n = CString::new(n.as_str()).unwrap_or_default();
                let r = tie_get_env(n.as_ptr());
                let s = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(s))
            }
            "set_env" => {
                if args.len() != 2 {
                    return Err("set_env 需要两个字符串参数".into());
                }
                let (Value::Str(n), Value::Str(v)) = (&args[0], &args[1]) else {
                    return Err("set_env 需要两个字符串参数".into());
                };
                let n = CString::new(n.as_str()).unwrap_or_default();
                let v = CString::new(v.as_str()).unwrap_or_default();
                tie_set_env(n.as_ptr(), v.as_ptr());
                Ok(Value::Void)
            }
            "file_copy" => {
                if args.len() != 2 {
                    return Err("file_copy 需要两个字符串参数".into());
                }
                let (Value::Str(src), Value::Str(dest)) = (&args[0], &args[1]) else {
                    return Err("file_copy 需要两个字符串参数".into());
                };
                let s = CString::new(src.as_str()).unwrap_or_default();
                let d = CString::new(dest.as_str()).unwrap_or_default();
                Ok(Value::Bool(tie_file_copy(s.as_ptr(), d.as_ptr()) != 0))
            }
            "file_move" => {
                if args.len() != 2 {
                    return Err("file_move 需要两个字符串参数".into());
                }
                let (Value::Str(src), Value::Str(dest)) = (&args[0], &args[1]) else {
                    return Err("file_move 需要两个字符串参数".into());
                };
                let s = CString::new(src.as_str()).unwrap_or_default();
                let d = CString::new(dest.as_str()).unwrap_or_default();
                Ok(Value::Bool(tie_file_move(s.as_ptr(), d.as_ptr()) != 0))
            }
            // ---------- P1 正则表达式内置函数 ----------
            //
            // 与编译路径一致：走 C ABI 桥（共用 regex crate 实现），保证两路径行为逐字节一致。
            // 模式非法 → 报错（错误消息与编译路径 printf 文本一致，含非法模式原文）。
            "regex_match" => {
                if args.len() != 2 {
                    return Err("regex_match 需要两个字符串参数".into());
                }
                let (Value::Str(s), Value::Str(p)) = (&args[0], &args[1]) else {
                    return Err("regex_match 需要两个字符串参数".into());
                };
                let s = CString::new(s.as_str()).unwrap_or_default();
                let p = CString::new(p.as_str()).unwrap_or_default();
                let mut ok: i8 = 0;
                let r = tie_regex_match(s.as_ptr(), p.as_ptr(), &mut ok);
                if ok == 0 {
                    return Err(format!("运行时错误: regex_match 模式 '{p:?}' 非法"));
                }
                Ok(Value::Bool(r != 0))
            }
            "regex_find" => {
                if args.len() != 2 {
                    return Err("regex_find 需要两个字符串参数".into());
                }
                let (Value::Str(s), Value::Str(p)) = (&args[0], &args[1]) else {
                    return Err("regex_find 需要两个字符串参数".into());
                };
                let s = CString::new(s.as_str()).unwrap_or_default();
                let p = CString::new(p.as_str()).unwrap_or_default();
                let r = tie_regex_find(s.as_ptr(), p.as_ptr());
                // 模式非法（NULL）→ 报错；无匹配返回空串（非 NULL）
                if r.is_null() {
                    return Err(format!("运行时错误: regex_find 模式 '{p:?}' 非法"));
                }
                let out = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(out))
            }
            "regex_find_all" => {
                if args.len() != 2 {
                    return Err("regex_find_all 需要两个字符串参数".into());
                }
                let (Value::Str(s), Value::Str(p)) = (&args[0], &args[1]) else {
                    return Err("regex_find_all 需要两个字符串参数".into());
                };
                let s = CString::new(s.as_str()).unwrap_or_default();
                let p = CString::new(p.as_str()).unwrap_or_default();
                let t = tie_regex_find_all(s.as_ptr(), p.as_ptr());
                // 模式非法（NULL）→ 报错
                if t.is_null() {
                    return Err(format!("运行时错误: regex_find_all 模式 '{p:?}' 非法"));
                }
                // 读出全部匹配片段（元素类型 string，data 缓冲区为 *const c_char 数组）
                let mut cells: Vec<Value> = Vec::new();
                unsafe {
                    let tbl = &*t;
                    for i in 0..tbl.len {
                        let ptr = (tbl.data as *const *const c_char).add(i as usize).read();
                        cells.push(Value::Str(c_char_to_string(ptr).unwrap_or_default()));
                    }
                }
                tie_table_free(t);
                Ok(Value::Table(cells))
            }
            "regex_replace" => {
                if args.len() != 3 {
                    return Err("regex_replace 需要三个字符串参数".into());
                }
                let (Value::Str(s), Value::Str(p), Value::Str(to)) =
                    (&args[0], &args[1], &args[2])
                else {
                    return Err("regex_replace 需要三个字符串参数".into());
                };
                let s = CString::new(s.as_str()).unwrap_or_default();
                let p = CString::new(p.as_str()).unwrap_or_default();
                let to = CString::new(to.as_str()).unwrap_or_default();
                let r = tie_regex_replace(s.as_ptr(), p.as_ptr(), to.as_ptr());
                if r.is_null() {
                    return Err(format!("运行时错误: regex_replace 模式 '{p:?}' 非法"));
                }
                let out = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(out))
            }
            "regex_group" => {
                if args.len() != 3 {
                    return Err("regex_group 需要两个字符串参数与一个整数参数".into());
                }
                let (Value::Str(s), Value::Str(p), Value::Int(i)) =
                    (&args[0], &args[1], &args[2])
                else {
                    return Err("regex_group 需要两个字符串参数与一个整数参数".into());
                };
                let s = CString::new(s.as_str()).unwrap_or_default();
                let p = CString::new(p.as_str()).unwrap_or_default();
                let r = tie_regex_group(s.as_ptr(), p.as_ptr(), *i);
                if r.is_null() {
                    return Err(format!("运行时错误: regex_group 模式 '{p:?}' 非法"));
                }
                let out = unsafe { c_char_to_string(r).unwrap_or_default() };
                tie_free_result(r);
                Ok(Value::Str(out))
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
                // 平衡三进制（M4 补齐）：trit → "-1"/"0"/"1"（与编译路径 i8 格式化一致）
                let r = match &args[0] {
                    Value::Int(n) => tie_to_string_i64(*n),
                    Value::Float(f) => tie_to_string_f64(*f),
                    Value::Trit(t) => string_to_c_char(t.to_string()),
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
            // 平衡三进制解析（M4 补齐）：接受 "-1"/"0"/"1"，非法输入报错
            //（走 C ABI 桥，与编译路径共用同一份 Rust 实现，错误文本一致）。
            "parse_trit" => {
                if args.len() != 1 {
                    return Err("parse_trit 需要一个字符串参数".into());
                }
                let Value::Str(s) = &args[0] else {
                    return Err("parse_trit 需要一个字符串参数".into());
                };
                let mut ok: i8 = 0;
                let p = CString::new(s.as_str()).unwrap_or_default();
                let v = tie_parse_trit(p.as_ptr(), &mut ok);
                if ok == 0 {
                    return Err(format!(
                        "运行时错误: parse_trit 参数 '{s}' 不是合法的 trit（期望 -1/0/1）"
                    ));
                }
                Ok(Value::Trit(v))
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
            // 进程/环境类：arg_count / arg_string 走 C ABI 桥（与编译路径共用
            // std::env::args，保证两路径返回一致的命令行参数）。
            "arg_count" => {
                if !args.is_empty() {
                    return Err("arg_count 不需要参数".into());
                }
                Ok(Value::Int(tie_arg_count()))
            }
            "arg_string" => {
                if args.len() != 1 {
                    return Err("arg_string 需要 1 个整数参数".into());
                }
                let Value::Int(i) = args[0] else {
                    return Err("arg_string 需要一个整数参数".into());
                };
                // 走 C ABI 桥取堆串（越界返回空串），用完释放
                let p = tie_arg_string(i);
                let s = unsafe { c_char_to_string(p).unwrap_or_default() };
                tie_free_result(p);
                Ok(Value::Str(s))
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
                // 用户函数（REPL 中定义的；命名空间函数以全名注册）
                if let Some(f) = self.session.funcs.get(name).cloned() {
                    // 参数个数区间检查（默认值参数）：实参数必须在 [必选数, 总形参数] 内。
                    // 必选数 = 无默认值的形参数（可选参数连续排在尾部，语义层已保证）。
                    let required = f.params.iter().take_while(|p| p.default.is_none()).count();
                    if args.len() < required || args.len() > f.params.len() {
                        return Err(format!(
                            "函数 '{name}' 期望 {}-{} 个参数，实际 {} 个",
                            required,
                            f.params.len(),
                            args.len()
                        ));
                    }
                    // 函数体执行期间，裸调用按本函数命名空间前缀补全：
                    // 全名 "tcmsg::error::no_file" → 前缀 ["tcmsg","error"]
                    let saved_ns = std::mem::take(&mut self.cur_ns);
                    if name.contains("::") {
                        let mut segs: Vec<String> = name.split("::").map(|s| s.to_string()).collect();
                        segs.pop(); // 去掉函数名，剩命名空间路径
                        self.cur_ns = segs;
                    }
                    // 保存调用者的作用域起点，压参后本函数作用域从参数作用域开始
                    //（被调函数内声明与调用者同名变量不再冲突，跨函数隔离）。
                    let saved_base = self.scope_base;
                    // 压新作用域绑定参数：实参在前，缺省参数按默认值表达式求值补齐
                    //（默认值限字面量/空表，eval_expr 直接求值，无作用域依赖）。
                    self.scopes.push(std::collections::HashMap::new());
                    for (i, p) in f.params.iter().enumerate() {
                        let v = if let Some(v) = args.get(i) {
                            v.clone()
                        } else if let Some(d) = &p.default {
                            self.eval_expr(d)?
                        } else {
                            return Err(format!(
                                "函数 '{name}' 缺少第 {} 个参数且无默认值",
                                i + 1
                            ));
                        };
                        self.scopes.last_mut().unwrap().insert(p.name.clone(), v);
                    }
                    self.scope_base = self.scopes.len() - 1;
                    let result = self.exec_block(&f.body);
                    self.scopes.pop();
                    self.scope_base = saved_base;
                    self.cur_ns = saved_ns;
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
    /// 平衡三进制 trit（M4 补齐）：值域 -1/0/+1
    Trit(i8),
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
            Value::Trit(_) => "trit",
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
            // 平衡三进制：-1/0/1（编译路径 i8 格式化一致）
            Value::Trit(t) => t.to_string(),
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

/// 类型匹配：Value 的动态类型是否等于 TypeSpec（switch 的 case 类型匹配 pattern）。
///
/// 与编译路径的语义对齐：case string: 匹配 Value::Str，case i64: 匹配 Value::Int 等。
/// 宽类型（num/text/misc）按类别框归属；元组/类类型暂不支持（interp 无对应 Value 变体）。
fn value_matches_ty(v: &Value, ty: &TypeSpec) -> bool {
    match ty {
        // 整数：全部归为 Value::Int（interp 动态值不区分整数位宽）
        TypeSpec::Named(
            TyKw::I8 | TyKw::I16 | TyKw::I32 | TyKw::I64 | TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64,
        ) => matches!(v, Value::Int(_)),
        // 浮点：全部归为 Value::Float
        TypeSpec::Named(TyKw::F32 | TyKw::F64) => matches!(v, Value::Float(_)),
        TypeSpec::Named(TyKw::Bool) => matches!(v, Value::Bool(_)),
        // 平衡三进制 trit（M4 补齐）：switch 的 case trit: 匹配 Value::Trit
        TypeSpec::Named(TyKw::Trit) => matches!(v, Value::Trit(_)),
        TypeSpec::Named(TyKw::Char) => matches!(v, Value::Char(_)),
        TypeSpec::Named(TyKw::Str) => matches!(v, Value::Str(_)),
        TypeSpec::Named(TyKw::Table) => matches!(v, Value::Table(_)),
        TypeSpec::Named(TyKw::Void) => matches!(v, Value::Void),
        // 宽类型（类别框）：num 接受 Int/Float；text 接受 Str/Char；misc 接受其余
        TypeSpec::Named(TyKw::Num) => matches!(v, Value::Int(_) | Value::Float(_)),
        TypeSpec::Named(TyKw::Text) => matches!(v, Value::Str(_) | Value::Char(_)),
        TypeSpec::Named(TyKw::Misc) => !matches!(
            v,
            Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Char(_) | Value::Str(_)
        ),
        // code 是编译期概念，interp 无对应值；元组/类暂不支持
        TypeSpec::Named(TyKw::Code) | TypeSpec::Tuple(_) | TypeSpec::Struct(_) => false,
    }
}

/// 把 i64 运算结果饱和截断到 trit 值域 [-1, 1]（Kleene 饱和算术）。
fn clamp_trit(v: i64) -> i8 {
    if v < -1 {
        -1
    } else if v > 1 {
        1
    } else {
        v as i8
    }
}

/// trit 与 i64 的混合运算（M4 补齐）：trit 提升为 i64（sext）后做整数运算。
///
/// 参数顺序：`trit` 值在前（a 是 trit，b 是 i64）。交换律运算符（+ * == !=）
/// 对顺序无感；非交换（- < > <= >=）由调用方按「trit op i64 / i64 op trit」
/// 调整实参顺序后调用。
fn eval_trit_int(
    op: tie_frontend::ast::BinaryOp,
    a: i8,
    b: i64,
) -> Result<Value, String> {
    use tie_frontend::ast::BinaryOp;
    let ai = a as i64;
    Ok(match op {
        BinaryOp::Add => Value::Int(ai + b),
        BinaryOp::Sub => Value::Int(ai - b),
        BinaryOp::Mul => Value::Int(ai * b),
        BinaryOp::Eq => Value::Bool(ai == b),
        BinaryOp::NotEq => Value::Bool(ai != b),
        BinaryOp::Lt => Value::Bool(ai < b),
        BinaryOp::Gt => Value::Bool(ai > b),
        BinaryOp::Le => Value::Bool(ai <= b),
        BinaryOp::Ge => Value::Bool(ai >= b),
        _ => {
            return Err(format!(
                "trit 与 i64 不支持该运算: {}",
                op_display(op)
            ))
        }
    })
}

/// i64 与 trit 的混合运算（i64 在前，如 `5 - trit`）：按 i64 op trit 语义计算。
fn eval_trit_int_rev(
    op: tie_frontend::ast::BinaryOp,
    a: i64,
    b: i8,
) -> Result<Value, String> {
    use tie_frontend::ast::BinaryOp;
    let bi = b as i64;
    Ok(match op {
        BinaryOp::Add => Value::Int(a + bi),
        BinaryOp::Sub => Value::Int(a - bi),
        BinaryOp::Mul => Value::Int(a * bi),
        BinaryOp::Eq => Value::Bool(a == bi),
        BinaryOp::NotEq => Value::Bool(a != bi),
        BinaryOp::Lt => Value::Bool(a < bi),
        BinaryOp::Gt => Value::Bool(a > bi),
        BinaryOp::Le => Value::Bool(a <= bi),
        BinaryOp::Ge => Value::Bool(a >= bi),
        _ => {
            return Err(format!(
                "i64 与 trit 不支持该运算: {}",
                op_display(op)
            ))
        }
    })
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

    // ---------- P1 正则表达式内置函数（解释路径） ----------

    #[test]
    fn eval_regex_match() {
        // 部分匹配即真（RE2 语义）；不匹配为 false
        assert_eq!(ev("regex_match(\"hello world\", \"wo\")").unwrap(), "true");
        assert_eq!(ev("regex_match(\"hello\", \"^h\")").unwrap(), "true");
        assert_eq!(ev("regex_match(\"hello\", \"^x\")").unwrap(), "false");
        assert_eq!(ev("regex_match(\"a1b2\", \"\\\\d\")").unwrap(), "true");
        // 非法模式 → 报错
        let err = ev("regex_match(\"abc\", \"[\")").unwrap_err();
        assert!(err.contains("regex_match 模式"), "错误：{err}");
    }

    #[test]
    fn eval_regex_find() {
        // 返回首个匹配片段
        assert_eq!(ev("regex_find(\"abc123def\", \"\\\\d+\")").unwrap(), "123");
        assert_eq!(ev("regex_find(\"hello\", \"lo\")").unwrap(), "lo");
        // 无匹配返回空串
        assert_eq!(ev("regex_find(\"hello\", \"xyz\")").unwrap(), "");
        // 非法模式 → 报错
        let err = ev("regex_find(\"abc\", \"(\")").unwrap_err();
        assert!(err.contains("regex_find 模式"), "错误：{err}");
    }

    #[test]
    fn eval_regex_find_all() {
        // 返回全部匹配片段组成的字符串表
        assert_eq!(
            ev("len(regex_find_all(\"a1b22c333\", \"\\\\d+\"))").unwrap(),
            "3"
        );
        assert_eq!(
            ev("regex_find_all(\"a1b22c333\", \"\\\\d+\")[0]").unwrap(),
            "1"
        );
        assert_eq!(
            ev("regex_find_all(\"a1b22c333\", \"\\\\d+\")[1]").unwrap(),
            "22"
        );
        assert_eq!(
            ev("regex_find_all(\"a1b22c333\", \"\\\\d+\")[2]").unwrap(),
            "333"
        );
        // 无匹配 → 空表
        assert_eq!(ev("len(regex_find_all(\"abc\", \"\\\\d\"))").unwrap(), "0");
    }

    #[test]
    fn eval_regex_replace() {
        // 全部替换
        assert_eq!(
            ev("regex_replace(\"a1b2c3\", \"\\\\d\", \"#\")").unwrap(),
            "a#b#c#"
        );
        // 无匹配 → 原样返回
        assert_eq!(ev("regex_replace(\"abc\", \"x\", \"y\")").unwrap(), "abc");
        // 捕获引用 $1
        assert_eq!(
            ev("regex_replace(\"2026-08-08\", \"(\\\\d+)-(\\\\d+)-(\\\\d+)\", \"$3/$2/$1\")").unwrap(),
            "08/08/2026"
        );
    }

    #[test]
    fn eval_regex_group() {
        // 第 0 组 = 整个匹配；第 1 组起为捕获组
        assert_eq!(
            ev("regex_group(\"hello world\", \"(\\\\w+) (\\\\w+)\", 0)").unwrap(),
            "hello world"
        );
        assert_eq!(
            ev("regex_group(\"hello world\", \"(\\\\w+) (\\\\w+)\", 1)").unwrap(),
            "hello"
        );
        assert_eq!(
            ev("regex_group(\"hello world\", \"(\\\\w+) (\\\\w+)\", 2)").unwrap(),
            "world"
        );
        // 组号越界 / 无匹配 → 空串
        assert_eq!(
            ev("regex_group(\"hello world\", \"(\\\\w+) (\\\\w+)\", 5)").unwrap(),
            ""
        );
        assert_eq!(ev("regex_group(\"abc\", \"(\\\\d)\", 1)").unwrap(), "");
    }

    // ---------- P2 eval_call（tie:script 模块协议执行基础） ----------

    #[test]
    fn eval_call_调用已注册函数与命名空间() {
        // eval_call：调用已注册用户函数（字符串参数）→ 返回值字符串。
        // tie:script 模块协议：先 eval 模块文件注册入口，再以字符串值直传调用。
        let mut s = Session::new();
        // 顶层裸名入口
        s.eval("func process(src: string) -> string {\n    return \"[\" + src + \"]\"\n}\n")
            .unwrap();
        assert_eq!(s.eval_call("process", "hello").unwrap(), "[hello]");
        // 命名空间全名
        s.eval(
            "namespace mod {\n\
             \x20func upper(src: string) -> string {\n\
             \x20\x20\x20\x20return src\n\
             \x20}\n\
             }\n",
        )
        .unwrap();
        assert_eq!(s.eval_call("mod::upper", "x").unwrap(), "x");
        // void 函数 → 空串
        s.eval("func noop(src: string) {\n}\n").unwrap();
        assert_eq!(s.eval_call("noop", "any").unwrap(), "");
    }

    #[test]
    fn eval_call_未定义函数与参数错误() {
        let mut s = Session::new();
        s.eval("func f(x: string) -> string {\n    return x\n}\n").unwrap();
        // 未定义的函数 → 报错
        let err = s.eval_call("missing", "a").unwrap_err();
        assert!(err.contains("未定义的函数 'missing'"), "错误：{err}");
        // 0 参数函数 → 报错（模块入口必须恰好 1 个字符串参数）
        s.eval("func g() -> string {\n    return \"g\"\n}\n").unwrap();
        let err = s.eval_call("g", "a").unwrap_err();
        assert!(err.contains("必须恰好接收 1 个字符串参数"), "错误：{err}");
    }

    #[test]
    fn eval_call_表达式级分发() {
        // 表达式级调用：s.eval("eval_call(...)") 走 call_fn 分发 → 桥 → Session::eval_call，
        // 与 REPL 内手输 eval_call 的真实路径一致。
        let mut s = Session::new();
        s.eval("func process(src: string) -> string {\n    return \"[\" + src + \"]\"\n}\n")
            .unwrap();
        assert_eq!(s.eval("eval_call(\"process\", \"hi\")").unwrap(), "[hi]");
        // 参数个数错误 → 报错
        let err = s.eval("eval_call(\"process\")").unwrap_err();
        assert!(err.contains("eval_call 需要两个字符串参数"), "错误：{err}");
        // 未定义函数 → 报错（错误文本来自 Session::eval_call）
        let err = s.eval("eval_call(\"nope\", \"x\")").unwrap_err();
        assert!(err.contains("未定义的函数 'nope'"), "错误：{err}");
    }

    #[test]
    fn eval_namespace_call() {
        // 命名空间函数路径调用：tcmsg.error.hello() → 全名 tcmsg::error::hello。
        // 分两阶段（REPL 语义）：先注册定义，再执行表达式。
        let mut s = Session::new();
        s.eval(
            "namespace tcmsg {\n\
             \x20func hello() -> string {\n\
             \x20\x20\x20\x20return \"ns hello\"\n\
             \x20}\n\
             \x20namespace error {\n\
             \x20\x20\x20\x20func hello() -> string {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return \"err hello\"\n\
             \x20\x20\x20\x20}\n\
             \x20\x20\x20\x20func inner() -> string {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return \"inner\"\n\
             \x20\x20\x20\x20}\n\
             \x20\x20\x20\x20func call_inner() -> string {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return inner()\n\
             \x20\x20\x20\x20}\n\
             \x20}\n\
             }\n",
        )
        .unwrap();
        assert_eq!(s.eval("tcmsg.hello()").unwrap(), "ns hello");
        assert_eq!(s.eval("tcmsg.error.hello()").unwrap(), "err hello");
        assert_eq!(s.eval("tcmsg.error.call_inner()").unwrap(), "inner");
    }

    #[test]
    fn eval_ns_missing_fn_err() {
        let mut s = Session::new();
        s.eval("namespace tcmsg {\nfunc ok() -> string {\nreturn \"x\"\n}\n}\n")
            .unwrap();
        let err = s.eval("tcmsg.missing()").unwrap_err();
        assert!(err.contains("未定义的函数 'tcmsg::missing'"), "错误：{err}");
    }

    #[test]
    fn eval_ns_table_lit_arg() {
        // 用户核心 API 形态：命名空间函数 + 表字面量实参
        // （tcmsg::error.no_file(["zh-cn","en-us"])）——解释路径。
        // 表字面量实参在解释器里求值为 Value::Table，按位置传给函数参数。
        let mut s = Session::new();
        s.eval(
            "namespace tcmsg {\n\
             \x20namespace error {\n\
             \x20\x20\x20\x20func no_file(langs: table) -> string {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return \"File not found\"\n\
             \x20\x20\x20\x20}\n\
             \x20}\n\
             }\n",
        )
        .unwrap();
        assert_eq!(
            s.eval("tcmsg::error.no_file([\"zh-cn\", \"en-us\"])").unwrap(),
            "File not found"
        );
        // 函数体内消费表实参：len(表) 返回元素个数（与编译路径一致）
        s.eval(
            "namespace tcmsg {\n\
             \x20func count(langs: table) -> i64 {\n\
             \x20\x20\x20\x20return len(langs)\n\
             \x20}\n\
             }\n",
        )
        .unwrap();
        assert_eq!(s.eval("tcmsg.count([\"zh-cn\", \"en-us\"])").unwrap(), "2");
    }

    #[test]
    fn eval_default_arg_省略与传参() {
        // 默认值参数（解释路径）：省略可选参数用默认值，显式传参覆盖。
        let mut s = Session::new();
        s.eval(
            "func greet(name: string, prefix: string = \"Hello\") -> string {\n\
             \x20\x20\x20\x20return prefix + \", \" + name\n\
             }\n",
        )
        .unwrap();
        assert_eq!(s.eval("greet(\"World\")").unwrap(), "Hello, World");
        assert_eq!(s.eval("greet(\"World\", \"Hi\")").unwrap(), "Hi, World");
        // 超参 → 报错（区间检查）
        let err = s.eval("greet(\"World\", \"Hi\", \"x\")").unwrap_err();
        assert!(err.contains("期望 1-2 个参数"), "错误：{err}");
    }

    #[test]
    fn eval_default_arg_tcmsg综合方案() {
        // tcmsg 综合方案形态（解释路径）：langs 必选 + texts 可选（空表默认值）。
        // 省略 texts → 查字典（方案 B）；传 texts → 直接返回调用方文本（方案 A）。
        let mut s = Session::new();
        s.eval(
            "namespace tcmsg {\n\
             \x20namespace error {\n\
             \x20\x20\x20\x20func no_file(langs: table, texts: table = []) -> string {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20msg_register(\"error.no_file\", \"zh\", \"文件不存在\")\n\
             \x20\x20\x20\x20\x20\x20\x20\x20msg_register(\"error.no_file\", \"en\", \"File not found\")\n\
             \x20\x20\x20\x20\x20\x20\x20\x20if len(texts) > 0 {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20return texts[0]\n\
             \x20\x20\x20\x20\x20\x20\x20\x20}\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return msg_t(\"error.no_file\")\n\
             \x20\x20\x20\x20}\n\
             \x20}\n\
             }\n",
        )
        .unwrap();
        // 方案 B：省略 texts → 查字典（当前语言 en → "File not found"）
        s.eval("msg_set_lang(\"en\")").unwrap();
        assert_eq!(
            s.eval("tcmsg::error.no_file([\"zh-cn\", \"en-us\"])").unwrap(),
            "File not found"
        );
        // 方案 A：传 texts → 直接返回调用方文本
        assert_eq!(
            s.eval("tcmsg::error.no_file([\"zh-cn\", \"en-us\"], [\"调用方提供的文本\"])")
                .unwrap(),
            "调用方提供的文本"
        );
    }

    #[test]
    fn eval_msg_三件套含回退规则() {
        // 消息系统：msg_register 登记 (键,语言)→文本；msg_t 查询按
        // 当前语言 → 回退 zh → 回退键本身的顺序（与 C ABI 桥一致）。
        let mut s = Session::new();
        s.eval("msg_register(\"err.no_file\", \"zh\", \"文件不存在\")").unwrap();
        s.eval("msg_register(\"err.no_file\", \"en\", \"File not found\")").unwrap();
        // 默认语言 zh：命中 zh 文本
        assert_eq!(s.eval("msg_t(\"err.no_file\")").unwrap(), "文件不存在");
        // 切换 en：命中 en 文本
        s.eval("msg_set_lang(\"en\")").unwrap();
        assert_eq!(s.eval("msg_t(\"err.no_file\")").unwrap(), "File not found");
        // 切换未登记语言 ja：回退 zh
        s.eval("msg_set_lang(\"ja\")").unwrap();
        assert_eq!(s.eval("msg_t(\"err.no_file\")").unwrap(), "文件不存在");
        // 未登记键：回退键本身
        assert_eq!(s.eval("msg_t(\"err.unknown\")").unwrap(), "err.unknown");
    }

    #[test]
    fn eval_string_concat() {
        assert_eq!(ev("\"foo\" + \"bar\"").unwrap(), "foobar");
    }

    // ---------- M4 补齐：表下标赋值测试 ----------

    #[test]
    fn 表下标赋值动态表读写() {
        // 动态表：table_new + push → 下标赋值 → 读取
        assert_eq!(
            ev("var t = table_new_i64(); table_push(t, 10); table_push(t, 20); t[0] = 99; t[0]").unwrap(),
            "99"
        );
        // 复合下标赋值 t[i] += v
        assert_eq!(
            ev("var t = table_new_i64(); table_push(t, 5); t[0] += 3; t[0]").unwrap(),
            "8"
        );
        // 越界写 → 运行时错误
        let err = ev("var t = table_new_i64(); table_push(t, 1); t[5] = 9").unwrap_err();
        assert!(err.contains("下标越界"), "错误消息：{err}");
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
    fn eval_switch_模式匹配() {
        // switch 模式匹配增强（与编译路径语义一致）：
        // 多值 case 1, 2 / 区间 case 3..7（左闭右开）/ 守卫 when / 字符区间 / 类型匹配。
        // 注意：块语句（switch）以 } 结束，后不能加分号；块内语句需分号。
        // 多值：任一相等即命中
        assert_eq!(ev("var x = 2; switch x { case 1, 2: 10; default: 0; }").unwrap(), "10");
        // 区间命中（3 ≤ 5 < 7）
        assert_eq!(ev("var x = 5; switch x { case 3..7: 20; default: 0; }").unwrap(), "20");
        // 左闭右开：x = 7 不命中 3..7 → default
        assert_eq!(ev("var x = 7; switch x { case 3..7: 20; default: 0; }").unwrap(), "0");
        // 守卫拦截：flag = false → 落入 default
        assert_eq!(ev("var f = false; var x = 8; switch x { case 8 when f: 30; default: 0; }").unwrap(), "0");
        // 守卫放行：值匹配 且 守卫为真 → 进入分支
        assert_eq!(ev("var f = true; var x = 8; switch x { case 8 when f: 30; default: 0; }").unwrap(), "30");
        // 字符区间：'a' ≤ 'b' < 'e'
        assert_eq!(ev("var c = 'b'; switch c { case 'a'..'e': 40; default: 0; }").unwrap(), "40");
        // 类型匹配：interp 动态类型按 Value 变体匹配（语义层拦截静态类型，eval 路径允许）
        assert_eq!(ev("var s = \"hi\"; switch s { case string: 50; default: 0; }").unwrap(), "50");
        // 多值 + 区间 + 守卫组合：任一 pattern 命中 且 守卫为真
        assert_eq!(ev("var f = true; var x = 4; switch x { case 1, 3..5 when f: 60; default: 0; }").unwrap(), "60");
        // default 省略且全不匹配 → 无输出（switch 返回 None）
        assert_eq!(ev("var x = 99; switch x { case 1: 1; } 0").unwrap(), "0");
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

    /// 生成唯一临时目录路径（list_dir 测试用），并清场确保从干净状态开始。
    fn temp_dir_path(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("tie_floor_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    /// 把文件路径转义成 tie 字符串字面量可直接嵌入的形式
    /// （Windows 路径含 `\`，而 tie 词法把 `\x` 当转义序列，需双写）。
    fn escaped_path(p: &std::path::Path) -> String {
        p.to_str().unwrap().replace('\\', "\\\\")
    }

    // ---------- M4 补齐：trit 平衡三进制测试 ----------

    #[test]
    fn trit字面量与格式化() {
        // zero → trit 0；to_print_string → "0"；to_string(trit) → "-1"/"0"/"1"
        assert_eq!(ev("zero").unwrap(), "0");
        assert_eq!(ev("to_string(zero)").unwrap(), "0");
        assert_eq!(ev("to_string(parse_trit(\"1\"))").unwrap(), "1");
        assert_eq!(ev("to_string(parse_trit(\"-1\"))").unwrap(), "-1");
        // parse_trit 非法输入 → 运行时错误
        let err = ev("parse_trit(\"2\")").unwrap_err();
        assert!(err.contains("不是合法的 trit"), "错误消息：{err}");
    }

    #[test]
    fn tritKleene三值逻辑真值表() {
        // Kleene && = min，|| = max；! = 取反
        // true(1)/zero(0)/false(-1) 全 9 组验证
        // &&：1&&1=1, 1&&0=0, 1&&-1=-1, 0&&0=0, 0&&-1=-1, -1&&-1=-1
        assert_eq!(ev("var a: trit = true; var b: trit = true; to_string(a && b)").unwrap(), "1");
        assert_eq!(ev("var a: trit = true; var b: trit = zero; to_string(a && b)").unwrap(), "0");
        assert_eq!(ev("var a: trit = true; var b: trit = false; to_string(a && b)").unwrap(), "-1");
        assert_eq!(ev("var a: trit = zero; var b: trit = false; to_string(a && b)").unwrap(), "-1");
        // ||：1||-1=1, 0||-1=0（max），-1||-1=-1
        assert_eq!(ev("var a: trit = false; var b: trit = true; to_string(a || b)").unwrap(), "1");
        assert_eq!(ev("var a: trit = false; var b: trit = zero; to_string(a || b)").unwrap(), "0");
        assert_eq!(ev("var a: trit = false; var b: trit = false; to_string(a || b)").unwrap(), "-1");
        // !：!1=-1, !-1=1, !0=0
        assert_eq!(ev("var a: trit = true; to_string(!a)").unwrap(), "-1");
        assert_eq!(ev("var a: trit = false; to_string(!a)").unwrap(), "1");
        assert_eq!(ev("var a: trit = zero; to_string(!a)").unwrap(), "0");
    }

    #[test]
    fn trit算术饱和与i64混合() {
        // trit 饱和算术：1+1=1（clamp）、1+0=1、1-1=0、1*1=1
        assert_eq!(ev("var a: trit = true; var b: trit = true; to_string(a + b)").unwrap(), "1");
        assert_eq!(ev("var a: trit = true; var b: trit = false; to_string(a - b)").unwrap(), "1");
        assert_eq!(ev("var a: trit = true; var b: trit = true; to_string(a * b)").unwrap(), "1");
        assert_eq!(ev("var a: trit = true; var b: trit = true; to_string(a - b)").unwrap(), "0");
        // trit + i64 → i64（sext 提升）
        assert_eq!(ev("var a: trit = true; to_string(a + 5)").unwrap(), "6");
        assert_eq!(ev("var a: trit = false; to_string(5 + a)").unwrap(), "4");
        // 比较：trit vs trit / i64 → bool
        assert_eq!(ev("var a: trit = true; var b: trit = false; (a > b) ? \"1\" : \"0\"").unwrap(), "1");
        assert_eq!(ev("var a: trit = true; (a == 1) ? \"1\" : \"0\"").unwrap(), "1");
        assert_eq!(ev("var a: trit = false; (a == 1) ? \"1\" : \"0\"").unwrap(), "0");
        // trit 除法 → 错误
        let err = ev("var a: trit = true; var b: trit = true; to_string(a / b)").unwrap_err();
        assert!(err.contains("trit 不支持除"), "错误消息：{err}");
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
    fn builtin_file_delete_removes_and_missing_false() {
        let p = temp_path("delete.txt");
        let path = escaped_path(&p);
        // 写文件 → 删除 → 存在性 false；删除已删除的文件 → false
        let code = format!(
            "file_write(\"{path}\", \"x\"); \
             var a = file_delete(\"{path}\"); \
             var b = file_exists(\"{path}\"); \
             var c = file_delete(\"{path}\"); \
             (a ? \"1\" : \"0\") + (b ? \"1\" : \"0\") + (c ? \"1\" : \"0\")"
        );
        assert_eq!(ev(&code).unwrap(), "100");
        // 删除不存在的文件 → false
        let p2 = temp_path("never.txt");
        let path2 = escaped_path(&p2);
        assert_eq!(ev(&format!("file_delete(\"{path2}\")")).unwrap(), "false");
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
    fn builtin_list_dir_lists_entries() {
        // 临时目录里放 3 个文件，list_dir 应全部列出（条目顺序由文件系统给出，不排序）
        let dir = temp_dir_path("listdir_ok");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::write(dir.join("c.txt"), "c").unwrap();
        let path = escaped_path(&dir);
        let code = format!(
            r#"var t = list_dir("{path}")
var n = len(t)
var names: string = ""
for x in t {{
    names = names + x + ","
}}
to_string(n) + ":" + names"#
        );
        let out = ev(&code).unwrap();
        // 恰好 3 个条目，且全部包含三个文件名
        assert!(out.starts_with("3:"), "list_dir 长度应为 3，实际: {out}");
        for name in ["a.txt", "b.txt", "c.txt"] {
            assert!(out.contains(name), "list_dir 应包含 {name}，实际: {out}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builtin_list_dir_empty_dir() {
        // 空目录 → 空表（len == 0）
        let dir = temp_dir_path("listdir_empty");
        std::fs::create_dir_all(&dir).unwrap();
        let path = escaped_path(&dir);
        let out = ev(&format!("len(list_dir(\"{path}\"))")).unwrap();
        assert_eq!(out, "0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builtin_list_dir_missing_dir_errors() {
        // 目录不存在 → 运行时错误（文本与编译路径一致）
        let dir = temp_dir_path("listdir_missing");
        let path = escaped_path(&dir);
        let err = ev(&format!("list_dir(\"{path}\")")).unwrap_err();
        assert!(err.contains("运行时错误: list_dir 无法读取目录"));
    }

    // ---------- M4 补齐：系统能力内置函数测试 ----------

    #[test]
    fn builtin_path_join_basename_dirname() {
        // 拼接路径：a + 分隔符 + b（平台分隔符由 Rust Path::join 决定）
        let p = ev("path_join(\"a\", \"b\")").unwrap();
        assert!(p == "a\\b" || p == "a/b");
        // 空段拼接
        let p2 = ev("path_join(\"a\", \"\")").unwrap();
        assert!(p2 == "a\\" || p2 == "a/");
        // basename：取最后一段
        assert_eq!(ev("path_basename(\"a/b/c.txt\")").unwrap(), "c.txt");
        assert_eq!(ev("path_basename(\"c.txt\")").unwrap(), "c.txt");
        // dirname：取父目录
        let d = ev("path_dirname(\"a/b/c.txt\")").unwrap();
        assert!(d == "a\\b" || d == "a/b");
        // dirname 无父段 → 空串
        assert_eq!(ev("path_dirname(\"c.txt\")").unwrap(), "");
    }

    #[test]
    fn builtin_path_abs_normalize_cwd() {
        // 绝对化：相对路径 → 以当前工作目录为基的绝对路径
        let abs = ev("path_abs(\"x.txt\")").unwrap();
        let cwd = ev("cwd()").unwrap();
        assert!(abs.starts_with(&cwd));
        // cwd 非空且含路径分隔符（Windows 含盘符）
        assert!(!cwd.is_empty());
        // normalize：解析 . 与 ..
        let n = ev("path_normalize(\"a/./b/../c\")").unwrap();
        assert!(n == "a\\c" || n == "a/c");
        // normalize 已规范化路径保持不变
        let n2 = ev("path_normalize(\"abc\")").unwrap();
        assert_eq!(n2, "abc");
    }

    #[test]
    fn builtin_env_get_set() {
        // set 后 get 立即读到（同一进程内）
        let code = "set_env(\"TIE_TEST_VAR\", \"hello\"); get_env(\"TIE_TEST_VAR\")";
        assert_eq!(ev(code).unwrap(), "hello");
        // 不存在的变量 → 空串
        assert_eq!(ev("get_env(\"TIE_NO_SUCH_VAR_XYZ\")").unwrap(), "");
        // set 空值 → 读取空串（非删除）
        let code2 = "set_env(\"TIE_TEST_VAR\", \"\"); get_env(\"TIE_TEST_VAR\")";
        assert_eq!(ev(code2).unwrap(), "");
    }

    #[test]
    fn builtin_exec_code_output() {
        // 执行命令取退出码：平台无关的无操作命令（Windows cmd /C exit /b 0）
        let ok = if cfg!(windows) {
            ev("exec_code(\"exit /b 0\")").unwrap()
        } else {
            ev("exec_code(\"true\")").unwrap()
        };
        assert_eq!(ok, "0");
        // exec_output 捕获 stdout：Windows 用 cmd echo，其他平台用 sh echo
        let out = ev("exec_output(\"echo hello\")").unwrap();
        assert!(out.contains("hello"));
    }

    #[test]
    fn builtin_file_copy_move_delete() {
        let src = temp_path("copy_src.txt");
        let dst = temp_path("copy_dst.txt");
        let src_s = escaped_path(&src);
        let dst_s = escaped_path(&dst);
        // 写源文件 → 复制 → 目标内容一致
        let code = format!(
            "file_write(\"{src_s}\", \"abc\"); file_copy(\"{src_s}\", \"{dst_s}\"); file_read(\"{dst_s}\")"
        );
        assert_eq!(ev(&code).unwrap(), "abc");
        // 移动：源消失、目标存在
        let moved = temp_path("move_dst.txt");
        let moved_s = escaped_path(&moved);
        let code2 = format!("file_move(\"{dst_s}\", \"{moved_s}\"); (file_exists(\"{dst_s}\") ? \"0\" : \"1\") + (file_exists(\"{moved_s}\") ? \"1\" : \"0\")");
        assert_eq!(ev(&code2).unwrap(), "11");
        let _ = std::fs::remove_file(&moved);
    }

    #[test]
    fn builtin_dir_operations_and_walk() {
        // 建多级目录 → 存在；写文件 → walk_dir 递归列出相对路径
        let dir = temp_dir_path("walk");
        let sub = dir.join("a").join("b");
        let sub_s = escaped_path(&sub);
        let code = format!(
            "var m = mkdir_all(\"{sub_s}\"); var f = file_write(path_join(\"{sub_s}\", \"x.txt\"), \"1\"); var w = walk_dir(\"{sub_s}\"); (m ? \"1\" : \"0\") + (f ? \"1\" : \"0\") + \":\" + to_string(len(w))"
        );
        let out = ev(&code).unwrap();
        assert!(out.starts_with("11:1"), "实际输出: {out}");
        // walk_dir 无效目录 → 运行时错误
        let missing = temp_dir_path("walk_missing");
        let missing_s = escaped_path(&missing);
        let err = ev(&format!("walk_dir(\"{missing_s}\")")).unwrap_err();
        assert!(err.contains("运行时错误: walk_dir 无法读取目录"));
        // remove_dir_all 递归删除
        let del = format!("remove_dir_all(\"{sub_s}\")");
        assert_eq!(ev(&del).unwrap(), "true");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builtin_untar_gz_roundtrip() {
        // 构造 tar.gz 归档（Rust 侧）→ untar_gz 解压 → 文件内容一致
        let dir = temp_dir_path("tar");
        // 先建归档目录（inner.txt 与 a.tar.gz 均在 dir 下）
        std::fs::create_dir_all(&dir).unwrap();
        let inner = dir.join("inner.txt");
        std::fs::write(&inner, "tar-content").unwrap();
        let tar_path = dir.join("a.tar.gz");
        let tar_s = escaped_path(&tar_path);
        let dest = temp_dir_path("tar_out");
        let dest_s = escaped_path(&dest);
        // 生成 tar.gz
        let file = std::fs::File::create(&tar_path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut ar = tar::Builder::new(enc);
        ar.append_path_with_name(&inner, "inner.txt").unwrap();
        ar.into_inner().unwrap().finish().unwrap();
        // untar_gz 解压到 dest
        let code = format!("untar_gz(\"{tar_s}\", \"{dest_s}\")");
        assert_eq!(ev(&code).unwrap(), "true");
        // 解压后文件内容一致
        let out = dest.join("inner.txt");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "tar-content");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn builtin_unzip_roundtrip() {
        // 构造 zip 归档（Rust 侧）→ unzip 解压 → 文件内容一致
        let dir = temp_dir_path("zip");
        // 先建归档目录（z.txt 与 a.zip 均在 dir 下）
        std::fs::create_dir_all(&dir).unwrap();
        let inner = dir.join("z.txt");
        std::fs::write(&inner, "zip-content").unwrap();
        let zip_path = dir.join("a.zip");
        let zip_s = escaped_path(&zip_path);
        let dest = temp_dir_path("zip_out");
        let dest_s = escaped_path(&dest);
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("z.txt", opts).unwrap();
            std::io::Write::write_all(&mut w, b"zip-content").unwrap();
            w.finish().unwrap();
        }
        let code = format!("unzip(\"{zip_s}\", \"{dest_s}\")");
        assert_eq!(ev(&code).unwrap(), "true");
        let out = dest.join("z.txt");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "zip-content");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn builtin_http_get_failure_errors() {
        // 无网络依赖的可测路径：非法协议 / 连接失败 → 运行时错误
        // （http_get 仅支持 http://；https 与无法连接均报错，文本与编译路径一致）
        let err = ev("http_get(\"https://example.com\")").unwrap_err();
        assert!(err.contains("运行时错误: http_get 无法访问 URL"));
    }

    #[test]
    fn builtin_http_get_file_binary_preserved() {
        // 回归测试（M6 E4）：http_get_file 下载二进制正文必须**逐字节一致**。
        // 此前 http_get_impl 用 from_utf8_lossy 取正文，非法 UTF-8 字节被替换为
        // U+FFFD（3 字节），下载 tar.gz/zip 包会损坏。修复后按原始字节切分返回。
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // 非 UTF-8 二进制正文（0x00/0xff 等无效字节 + 一段 ASCII）
        let payload: Vec<u8> = vec![
            0x1f, 0x8b, 0x08, 0x00, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xff, 0x00,
            0x7f, b'a', b'b', b'c',
        ];
        // 后台线程响应一次 HTTP 200（Connection: close）；只读请求头即回应，
        // 避免 read_to_end 等客户端关写（客户端读完响应前不会关）造成死锁
        let expected = payload.clone();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let mut head = Vec::new();
            loop {
                let n = sock.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&buf[..n]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let resp = [
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n",
                "Content-Length: ",
                &expected.len().to_string(),
                "\r\nConnection: close\r\n\r\n",
            ]
            .concat()
            .into_bytes();
            let mut all = resp;
            all.extend_from_slice(&expected);
            sock.write_all(&all).unwrap();
        });
        let dir = std::env::temp_dir().join(format!("tie_http_bin_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("pkg.tar.gz");
        // Windows 路径含反斜杠：tie 字符串字面量需转义为 \\（否则 \U 等被当转义）
        let out_s = out.to_str().unwrap().replace('\\', "\\\\");
        let code = format!(
            "http_get_file(\"http://{addr}/packages/demo/1.0.0.tar.gz\", \"{out_s}\")",
        );
        assert_eq!(ev(&code).unwrap(), "true");
        handle.join().unwrap();
        let written = std::fs::read(&out).unwrap();
        assert_eq!(written, payload, "下载的二进制正文必须与源逐字节一致");
        let _ = std::fs::remove_dir_all(&dir);
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

    // ---------- 进程/环境 floor 内置函数测试 ----------

    #[test]
    fn builtin_arg_count_matches_env() {
        // 解释器运行在测试进程内：arg_count 应等于进程用户参数个数（skip(1) 跳过程序名）
        let want = std::env::args().skip(1).count() as i64;
        let got: i64 = ev("arg_count()").unwrap().parse().unwrap();
        assert_eq!(got, want, "arg_count() 应与 std::env::args 用户参数个数一致");
        // 参数个数永远 >= 0（无参数进程 → 0）
        assert!(got >= 0);
    }

    #[test]
    fn builtin_arg_string_matches_env() {
        // 逐个对比 tie 的 arg_string 与进程真实参数（skip(1) 跳过 argv[0] 程序名）
        let args: Vec<String> = std::env::args().skip(1).collect();
        for (i, want) in args.iter().enumerate() {
            let got = ev(&format!("arg_string({i})")).unwrap();
            assert_eq!(got, *want, "arg_string({i}) 应与进程参数一致");
        }
        // 越界（下标 == 个数 / 更大 / 负数）→ 空串
        assert_eq!(ev(&format!("arg_string({})", args.len())).unwrap(), "");
        assert_eq!(ev("arg_string(999999999)").unwrap(), "");
        assert_eq!(ev("arg_string(-1)").unwrap(), "");
    }

    #[test]
    fn std_format_helpers() {
        // 加载 std/format.tie（tie 语言自写库，include_str! 保证测试的就是发行版源码）
        // M2.1 起 std 库使用命名空间形式：format.format_int 等。
        let lib = include_str!("../../../std/format.tie");
        let mut s = Session::new();
        s.eval(lib).unwrap(); // 注册库函数
        // format_int：委托 to_string
        assert_eq!(s.eval("format.format_int(42)").unwrap(), "42");
        assert_eq!(s.eval("format.format_int(-7)").unwrap(), "-7");
        assert_eq!(s.eval("format.format_int(0)").unwrap(), "0");
        // format_pad：右对齐、左侧补空格；宽度不足/相等不截断
        assert_eq!(s.eval("format.format_pad(42, 6)").unwrap(), "    42");
        assert_eq!(s.eval("format.format_pad(42, 2)").unwrap(), "42");
        assert_eq!(s.eval("format.format_pad(42, 0)").unwrap(), "42");
        assert_eq!(s.eval("format.format_pad(-7, 4)").unwrap(), "  -7");
        // format_int_hex：小写十六进制、无 0x；负数「-」+ 绝对值
        assert_eq!(s.eval("format.format_int_hex(255)").unwrap(), "ff");
        assert_eq!(s.eval("format.format_int_hex(0)").unwrap(), "0");
        assert_eq!(s.eval("format.format_int_hex(16)").unwrap(), "10");
        assert_eq!(s.eval("format.format_int_hex(4095)").unwrap(), "fff");
        assert_eq!(s.eval("format.format_int_hex(-255)").unwrap(), "-ff");
        assert_eq!(s.eval("format.format_int_hex(3735928559)").unwrap(), "deadbeef");
        // format(bool)：true / false
        assert_eq!(s.eval("format.format(true)").unwrap(), "true");
        assert_eq!(s.eval("format.format(false)").unwrap(), "false");
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

    #[test]
    fn dyn_table_new_push_len() {
        // table_new_i64 → push 1..100 → len 应为 100（动态增长）
        let code = r#"
            var t: table = table_new_i64()
            var i: i64 = 1
            while i <= 100 {
                table_push(t, i)
                i = i + 1
            }
            len(t)
        "#;
        assert_eq!(ev(code).unwrap(), "100");
        // 空表 len 为 0
        assert_eq!(ev("len(table_new_i64())").unwrap(), "0");
    }

    #[test]
    fn dyn_table_at_reads_element() {
        // table_at 读取第 i 个元素（0 基）
        let code = r#"
            var t: table = table_new_i64()
            table_push(t, 10)
            table_push(t, 20)
            table_push(t, 30)
            table_at(t, 1)
        "#;
        assert_eq!(ev(code).unwrap(), "20");
        // 下标访问 t[i] 与 table_at 等价
        let code2 = r#"
            var t: table = table_new_i64()
            table_push(t, 7)
            table_push(t, 8)
            t[0]
        "#;
        assert_eq!(ev(code2).unwrap(), "7");
    }

    #[test]
    fn dyn_table_at_out_of_bounds_errors() {
        // 越界（负数 / >= len）→ 运行时错误，文本与编译路径一致
        let code = r#"
            var t: table = table_new_i64()
            table_push(t, 1)
            table_at(t, 5)
        "#;
        let err = ev(code).unwrap_err();
        assert!(err.contains("运行时错误: table_at 下标越界：索引 5 超出长度 1"), "实际: {err}");
        let err2 = ev("table_at(table_new_i64(), -1)").unwrap_err();
        assert!(err2.contains("运行时错误: table_at 下标越界"), "实际：{err2}");
    }

    #[test]
    fn dyn_table_string_build_and_for() {
        // table_new_string 构建字符串表，for 遍历打印
        let code = r#"
            var t: table = table_new_string()
            table_push(t, "a")
            table_push(t, "b")
            table_push(t, "c")
            var out = ""
            for x in t {
                out = out + x
            }
            out
        "#;
        assert_eq!(ev(code).unwrap(), "abc");
        // table_at 读字符串元素
        let code2 = r#"
            var t: table = table_new_string()
            table_push(t, "hello")
            table_push(t, "world")
            table_at(t, 1)
        "#;
        assert_eq!(ev(code2).unwrap(), "world");
    }

    #[test]
    fn dyn_table_bool_and_f64() {
        // bool 表：push 后 table_at 读取
        let code = r#"
            var t: table = table_new_bool()
            table_push(t, true)
            table_push(t, false)
            table_at(t, 0)
        "#;
        assert_eq!(ev(code).unwrap(), "true");
        // f64 表：push 后 len 与 table_at
        let code2 = r#"
            var t: table = table_new_f64()
            table_push(t, 1.5)
            table_push(t, 2.5)
            len(t)
        "#;
        assert_eq!(ev(code2).unwrap(), "2");
    }
}
