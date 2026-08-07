//! 编译驱动：四段式流水线入口。
//!
//! 架构（四段式）：`预处理 [前端 中间优化 后端]`
//! ```text
//! 源码 .tie
//!   → 预处理（清理代码 + 识别文件类型 + 角色判定）   [preprocess]
//!   → 分派（按角色转交对应工具链）                   [driver::dispatch]
//!   └── 编译工具链（logic / library）：
//!         → 词法分析（含 ASI）    [tie_frontend::lexer]
//!         → 语法分析              [tie_frontend::parser]
//!         → 语义分析（类型检查）  [tie_frontend::semantic]
//!         → IR 生成              [ir]
//!         → opt 中间优化         [optimizer]
//!         → clang 链接生成可执行 [backend]
//!   └── 其他工具链（data / ui / db）：由预处理识别角色后转交，
//!         v0.1 阶段挂接点已就绪（后续版本实现）
//! ```

use crate::backend;
use crate::ir;
use crate::optimizer::{self, OptLevel};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tie_frontend::ast::{Program, Stmt};
use tie_frontend::lexer::LexError;
use tie_frontend::parser::{ParseError, parse_program};
use tie_frontend::semantic::{SemanticError, analyze};
use tie_prep::preprocess::{self, FileRole, PreprocessResult};

/// 编译选项（来自 CLI）。
#[derive(Debug, Clone)]
pub struct CompileOptions {
    /// 输入源码路径
    pub input: PathBuf,
    /// 输出可执行文件路径（默认：输入同名 .exe；library 角色默认 .a）
    pub output: Option<PathBuf>,
    /// 优化级别（None = 未显式指定，可由头部 `opt=` 覆盖）
    pub opt_level: Option<OptLevel>,
    /// 只生成 IR（.ll），不继续编译
    pub emit_ir_only: bool,
    /// 保留中间文件（.ll），否则编译成功后清理
    pub keep_intermediate: bool,
    /// 目标三元组（交叉编译，如 `x86_64-pc-windows-msvc`；None = 本机默认）
    pub target: Option<String>,
}

/// 编译产物：消息 + 可选产物路径。
#[derive(Debug)]
pub struct CompileOutcome {
    /// 面向用户的描述（"编译成功: x.exe" / "识别为 data 文件…"）
    pub message: String,
    /// 产物路径（编译类角色的可执行文件 / .ll；其他角色为 None）
    pub artifact: Option<PathBuf>,
}

/// 编译错误（携带阶段与来源）。
#[derive(Debug)]
pub enum CompileError {
    /// 源码读取失败
    Read(String),
    /// 前端词法错误
    Lex(LexError),
    /// 前端语法错误
    Parse(ParseError),
    /// 前端语义错误
    Semantic(SemanticError),
    /// IR 生成错误
    Ir(ir::IrError),
    /// 中间优化错误
    Optimize(optimizer::OptError),
    /// 后端错误
    Backend(backend::BackendError),
    /// 写中间文件失败
    Io(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Read(msg) => write!(f, "读取源码失败: {msg}"),
            CompileError::Lex(e) => write!(f, "{e}"),
            CompileError::Parse(e) => write!(f, "{e}"),
            CompileError::Semantic(e) => write!(f, "{e}"),
            CompileError::Ir(e) => write!(f, "{e}"),
            CompileError::Optimize(e) => write!(f, "[中间优化] {e}"),
            CompileError::Backend(e) => write!(f, "[后端] {e}"),
            CompileError::Io(msg) => write!(f, "文件操作失败: {msg}"),
        }
    }
}

/// 工具链分派结果：按文件角色决定处理路径。
enum Dispatch {
    /// 编译工具链（logic / library）
    Compile,
    /// 数据解析工具链（data）
    Data,
    /// UI 工具链（ui）
    Ui,
    /// 数据库工具链（db）
    Db,
}

/// 按角色分派工具链（预处理识别 → 自动转交对应工具）。
fn dispatch(role: FileRole) -> Dispatch {
    match role {
        FileRole::Logic | FileRole::Library => Dispatch::Compile,
        FileRole::Data => Dispatch::Data,
        FileRole::Ui => Dispatch::Ui,
        FileRole::Db => Dispatch::Db,
    }
}

/// 主入口：预处理 → 分派 → 对应工具链处理。
///
/// 返回编译产物（消息 + 路径）。
pub fn compile(opts: &CompileOptions) -> Result<CompileOutcome, CompileError> {
    // ---- 第 1 段：预处理 ----
    // 读取源码（原始文本）
    let source = fs::read_to_string(&opts.input).map_err(|e| CompileError::Read(e.to_string()))?;
    // 清理代码 + 识别文件类型 + 角色判定
    let pre = preprocess::preprocess(&source);

    // ---- 第 2 段：按角色自动分派对应工具 ----
    match dispatch(pre.role) {
        Dispatch::Compile => compile_program(opts, &pre),
        Dispatch::Data => Ok(CompileOutcome {
            message: format!(
                "[预处理] 识别为 data 数据交换文件（头: {:?}），已转交数据解析工具链 —— v0.1 数据交换工具链尚未实现",
                header_texts(&pre)
            ),
            artifact: None,
        }),
        Dispatch::Ui => Ok(CompileOutcome {
            message: format!(
                "[预处理] 识别为 ui 界面文件（头: {:?}），已转交 UI 工具链 —— v0.1 UI 工具链尚未实现",
                header_texts(&pre)
            ),
            artifact: None,
        }),
        Dispatch::Db => Ok(CompileOutcome {
            message: format!(
                "[预处理] 识别为 db 数据库文件（头: {:?}），已转交数据库工具链 —— v0.1 数据库工具链尚未实现",
                header_texts(&pre)
            ),
            artifact: None,
        }),
    }
}

/// 编译工具链：前端 → 中端 → 后端（logic / library 角色）。
fn compile_program(
    opts: &CompileOptions,
    pre: &PreprocessResult,
) -> Result<CompileOutcome, CompileError> {
    // 优化级别优先级：CLI 显式指定 > 头部 `opt=N` > 默认 O2
    let opt_level = opts
        .opt_level
        .or_else(|| header_opt_level(&pre.headers))
        .unwrap_or_default();

    // 目标三元组优先级：CLI 显式指定 > 头部 `target=三元组` > 本机默认（None）
    // 两者都经过平台别名规范化（win-x64 → x86_64-pc-windows-msvc）
    let target = opts
        .target
        .clone()
        .map(|t| normalize_target(t.trim()))
        .or_else(|| header_target(&pre.headers));

    // ---- 前端 ----
    // 词法分析（含 ASI 分号补全）
    let tokens = tie_frontend::lexer::tokenize(&pre.cleaned_source).map_err(CompileError::Lex)?;
    // 语法分析
    let program = parse_program(&tokens).map_err(CompileError::Parse)?;
    // import 展开：递归加载被导入文件并内联其顶层函数（含循环检测）
    let base_dir = opts
        .input
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let program = expand_imports(program, &base_dir)?;
    // 语义分析（符号表 + 类型检查）
    let sem = analyze(&program).map_err(CompileError::Semantic)?;

    // 入口检查：logic 角色必须定义 main（library 不需要）
    if !opts.emit_ir_only
        && pre.role == FileRole::Logic
        && !sem.funcs.contains_key("main")
    {
        return Err(CompileError::Semantic(SemanticError {
            span: tie_frontend::lexer::Span { line: 1, col: 1 },
            message: "文件角色为 logic，必须定义入口函数 main".into(),
        }));
    }

    // ---- 中端 ----
    // AST → LLVM IR 文本
    let ir_out = ir::gen_ir(&program, &sem).map_err(CompileError::Ir)?;

    // 中间文件路径：输入目录/输入名.ll
    let ir_path = opts.input.with_extension("ll");
    fs::write(&ir_path, &ir_out.ir).map_err(|e| CompileError::Io(e.to_string()))?;

    if opts.emit_ir_only {
        return Ok(CompileOutcome {
            message: format!("已生成 LLVM IR: {}", ir_path.display()),
            artifact: Some(ir_path),
        });
    }

    // opt 中间优化
    let opt_ir_path = opts.input.with_extension("opt.ll");
    optimizer::optimize(&ir_path, &opt_ir_path, opt_level).map_err(CompileError::Optimize)?;

    // ---- 后端 ----
    // 按角色区分产物：logic → 链接可执行文件；library → 编译目标文件 + 打包静态库
    match pre.role {
        FileRole::Logic => {
            let exe_path = opts
                .output
                .clone()
                .unwrap_or_else(|| opts.input.with_extension("exe"));
            backend::link(&opt_ir_path, &exe_path, target.as_deref())
                .map_err(CompileError::Backend)?;
            cleanup_intermediates(&ir_path, &opt_ir_path, opts.keep_intermediate);
            Ok(CompileOutcome {
                message: format!("编译成功: {}", exe_path.display()),
                artifact: Some(exe_path),
            })
        }
        FileRole::Library => {
            let lib_path = opts
                .output
                .clone()
                .unwrap_or_else(|| lib_output_path(&opts.input));
            // 第一步：IR → 目标文件（.o）
            let obj_path = opts.input.with_extension("o");
            backend::compile_object(&opt_ir_path, &obj_path, target.as_deref())
                .map_err(CompileError::Backend)?;
            // 第二步：目标文件 → 静态库（.a）
            backend::archive(&obj_path, &lib_path).map_err(CompileError::Backend)?;
            // 清理中间文件（.ll / .o），除非要求保留
            cleanup_intermediates(&ir_path, &opt_ir_path, opts.keep_intermediate);
            let _ = fs::remove_file(&obj_path);
            Ok(CompileOutcome {
                message: format!("库编译成功: {}", lib_path.display()),
                artifact: Some(lib_path),
            })
        }
        // 理论不可达：compile_program 只由 Dispatch::Compile（logic/library）调用
        _ => unreachable!("compile_program 仅处理 logic/library 角色"),
    }
}

/// 删除中间文件（.ll / .opt.ll），`keep` 为 true 时保留。
fn cleanup_intermediates(ir: &Path, opt_ir: &Path, keep: bool) {
    if !keep {
        let _ = fs::remove_file(ir);
        let _ = fs::remove_file(opt_ir);
    }
}

/// library 角色默认输出路径：`lib<输入名>.a`（如 `lib_math.tie` → `liblib_math.a` 不友好，
/// 改为 `<输入名>.a`：`lib_math.a`）。
fn lib_output_path(input: &Path) -> PathBuf {
    input.with_extension("a")
}

/// 递归展开 import 语句：加载被导入文件并把其顶层函数内联进当前程序。
///
/// - 路径解析：`import "./x.tie"` 的路径相对**当前文件所在目录**；
/// - 被导入文件自身也可含 import（递归展开，支持多级导入）；
/// - 循环检测：用规范化绝对路径集合记录「展开链」，重复访问即报循环导入。
fn expand_imports(program: Program, base_dir: &Path) -> Result<Program, CompileError> {
    let mut chain: HashSet<PathBuf> = HashSet::new();
    expand_imports_inner(program, base_dir, &mut chain)
}

/// 递归展开的实际实现。
///
/// `chain` 是跨递归共享的「展开链」集合（规范化绝对路径），
/// 用于循环导入检测——必须在同一递归栈内传递，不能每次新建。
fn expand_imports_inner(
    program: Program,
    base_dir: &Path,
    chain: &mut HashSet<PathBuf>,
) -> Result<Program, CompileError> {
    let mut out = Vec::new();
    for stmt in program.stmts {
        match stmt {
            Stmt::Import(imp) => {
                let import_path = base_dir.join(&imp.path);
                // 规范化路径（解析 . / ..），用于循环检测
                let canon = fs::canonicalize(&import_path)
                    .unwrap_or_else(|_| import_path.clone());
                if !chain.insert(canon.clone()) {
                    return Err(CompileError::Semantic(SemanticError {
                        span: imp.span,
                        message: format!("循环导入：文件 '{}' 已在导入链中", imp.path),
                    }));
                }
                // 读取 + 预处理 + 词法 + 语法解析被导入文件
                let source = fs::read_to_string(&import_path)
                    .map_err(|e| CompileError::Read(format!("导入文件 {} 读取失败: {e}", import_path.display())))?;
                let pre = preprocess::preprocess(&source);
                let tokens = tie_frontend::lexer::tokenize(&pre.cleaned_source)
                    .map_err(|e| CompileError::Lex(LexError {
                        span: imp.span,
                        message: format!("导入文件 {} 词法错误: {e}", imp.path),
                    }))?;
                let sub_program = parse_program(&tokens)
                    .map_err(|e| CompileError::Parse(ParseError {
                        span: imp.span,
                        message: format!("导入文件 {} 语法错误: {}", imp.path, e.message),
                    }))?;
                // 递归展开被导入文件的 import（以该文件所在目录为基准）
                let sub_dir = import_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."));
                let expanded = expand_imports_inner(sub_program, &sub_dir, chain)?;
                out.extend(expanded.stmts);
                // 弹出当前文件，允许同目录下其他文件再次导入它（非循环）
                chain.remove(&canon);
            }
            other => out.push(other),
        }
    }
    Ok(Program { stmts: out })
}

/// 从头部 `opt=N` 选项读取优化级别（`// tie:opt=3`）。
fn header_opt_level(headers: &[preprocess::Header]) -> Option<OptLevel> {
    for h in headers {
        if let Some((key, val)) = h.as_option()
            && key == "opt"
        {
            return match val.trim() {
                "0" => Some(OptLevel::O0),
                "1" => Some(OptLevel::O1),
                "2" => Some(OptLevel::O2),
                "3" => Some(OptLevel::O3),
                _ => None,
            };
        }
    }
    None
}

/// 从头部 `target=...` 选项读取目标三元组（`// tie:target=win-x64`）。
///
/// 支持常见平台别名 → LLVM 三元组映射；未知名称按原样作为三元组透传给 clang
/// （clang 会校验合法性）。
fn header_target(headers: &[preprocess::Header]) -> Option<String> {
    for h in headers {
        if let Some((key, val)) = h.as_option()
            && key == "target"
        {
            return Some(normalize_target(val.trim()));
        }
    }
    None
}

/// 平台别名 → LLVM 三元组；无别名时原样返回。
fn normalize_target(name: &str) -> String {
    match name {
        "win-x64" | "windows-x64" => "x86_64-pc-windows-msvc".to_string(),
        "win-x86" | "windows-x86" => "i686-pc-windows-msvc".to_string(),
        "win-arm64" | "windows-arm64" => "aarch64-pc-windows-msvc".to_string(),
        "linux-x64" | "linux-x86_64" => "x86_64-unknown-linux-gnu".to_string(),
        "linux-arm64" | "linux-aarch64" => "aarch64-unknown-linux-gnu".to_string(),
        "macos-x64" | "darwin-x64" => "x86_64-apple-darwin".to_string(),
        "macos-arm64" | "darwin-arm64" => "arm64-apple-darwin".to_string(),
        other => other.to_string(),
    }
}

/// 头部指令文本列表（错误/消息展示用）。
fn header_texts(pre: &PreprocessResult) -> Vec<&str> {
    pre.headers.iter().map(|h| h.raw.as_str()).collect()
}
