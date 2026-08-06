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
use std::fs;
use std::path::PathBuf;
use tie_frontend::lexer::LexError;
use tie_frontend::parser::{ParseError, parse_program};
use tie_frontend::semantic::{SemanticError, analyze};
use tie_prep::preprocess::{self, FileRole, PreprocessResult};

/// 编译选项（来自 CLI）。
#[derive(Debug, Clone)]
pub struct CompileOptions {
    /// 输入源码路径
    pub input: PathBuf,
    /// 输出可执行文件路径（默认：输入同名 .exe）
    pub output: Option<PathBuf>,
    /// 优化级别（None = 未显式指定，可由头部 `opt=` 覆盖）
    pub opt_level: Option<OptLevel>,
    /// 只生成 IR（.ll），不继续编译
    pub emit_ir_only: bool,
    /// 保留中间文件（.ll），否则编译成功后清理
    pub keep_intermediate: bool,
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

    // ---- 前端 ----
    // 词法分析（含 ASI 分号补全）
    let tokens = tie_frontend::lexer::tokenize(&pre.cleaned_source).map_err(CompileError::Lex)?;
    // 语法分析
    let program = parse_program(&tokens).map_err(CompileError::Parse)?;
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
    // clang 链接生成可执行
    let exe_path = opts
        .output
        .clone()
        .unwrap_or_else(|| opts.input.with_extension("exe"));
    backend::link(&opt_ir_path, &exe_path).map_err(CompileError::Backend)?;

    // 清理中间文件（除非要求保留）
    if !opts.keep_intermediate {
        let _ = fs::remove_file(&ir_path);
        let _ = fs::remove_file(&opt_ir_path);
    }

    Ok(CompileOutcome {
        message: format!("编译成功: {}", exe_path.display()),
        artifact: Some(exe_path),
    })
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

/// 头部指令文本列表（错误/消息展示用）。
fn header_texts(pre: &PreprocessResult) -> Vec<&str> {
    pre.headers.iter().map(|h| h.raw.as_str()).collect()
}
