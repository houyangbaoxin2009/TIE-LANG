//! 编译驱动：四段式流水线入口。
//!
//! 架构（四段式）：`预处理 [前端 中间优化 后端]`
//! ```text
//! 源码 .tie
//!   → 预处理（清理代码 + 识别文件类型 + 角色判定）   [preprocess]
//!   → 分派（按角色转交对应工具链）                   [driver::dispatch]
//!   └── 编译工具链（logic/script → 可执行文件；class/type → 静态库 .a）：
//!         → 词法分析（含 ASI）    [tie_frontend::lexer]
//!         → 语法分析              [tie_frontend::parser]
//!         → 语义分析（类型检查）  [tie_frontend::semantic]
//!         → IR 生成              [ir]
//!         → opt 中间优化         [optimizer]
//!         → clang 链接生成可执行 [backend]
//!   └── 其他工具链（data / ui / db / port）：由预处理识别角色后转交，
//!         v0.1 阶段挂接点已就绪（后续版本实现）
//! ```
//!
//! 文件角色由 `type tie` / `type tie<X>` 声明（新文件类型声明系统）或
//! 文件名 `<名>.<角色>.tie` 决定；优化级别与交叉编译目标仅来自 CLI
//! （旧 `// tie:xxx` 头部指令系统已完全移除）。

use crate::backend;
use crate::ir;
use crate::optimizer::{self, OptLevel};
use std::fs;
use std::path::{Path, PathBuf};
use tie_frontend::imports::{self, ImportError};
use tie_frontend::lexer::LexError;
use tie_frontend::parser::{ParseError, parse_program};
use tie_frontend::semantic::{SemanticError, analyze};
use tie_prep::preprocess::{self, FileRole, PreprocessResult};

/// 编译选项（来自 CLI）。
#[derive(Debug, Clone)]
pub struct CompileOptions {
    /// 输入源码路径
    pub input: PathBuf,
    /// 输出产物路径（默认：logic/script 输入同名 .exe；class/type 输入同名 .a）
    pub output: Option<PathBuf>,
    /// 优化级别（None = 默认 O2；仅来自 CLI，头部指令已移除）
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
    /// 产物路径（编译类角色的可执行文件 / 静态库 .a / .ll；其他角色为 None）
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
    /// import 展开错误（读取/词法/语法/循环导入，来自 tie_frontend::imports）
    Import(ImportError),
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
            CompileError::Import(e) => write!(f, "{e}"),
            CompileError::Ir(e) => write!(f, "{e}"),
            CompileError::Optimize(e) => write!(f, "[中间优化] {e}"),
            CompileError::Backend(e) => write!(f, "[后端] {e}"),
            CompileError::Io(msg) => write!(f, "文件操作失败: {msg}"),
        }
    }
}

/// 工具链分派结果：按文件角色决定处理路径。
enum Dispatch {
    /// 编译工具链（logic / script → 链接可执行文件）
    Compile,
    /// 库编译工具链（class / type → 打包静态库 .a）
    CompileLib,
    /// 数据解析工具链（data）
    Data,
    /// UI 工具链（ui）
    Ui,
    /// 数据库工具链（db）
    Db,
    /// 端口/对外接口工具链（port）
    Port,
}

/// 按角色分派工具链（预处理识别 → 自动转交对应工具）。
fn dispatch(role: FileRole) -> Dispatch {
    match role {
        FileRole::Logic | FileRole::Script => Dispatch::Compile,
        FileRole::Class | FileRole::Type => Dispatch::CompileLib,
        FileRole::Data => Dispatch::Data,
        FileRole::Ui => Dispatch::Ui,
        FileRole::Db => Dispatch::Db,
        FileRole::Port => Dispatch::Port,
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

    // 文件名默认角色与头部声明一致性检查：头部声明优先，不一致时警告。
    // `xxx.<角色>.tie` 形式的文件名声明只是默认值，头部 `type tie<X>`
    // 声明是权威——不一致仅提示、不报错，采用头部声明。
    let name_role = opts
        .input
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(FileRole::from_filename);
    if let Some(r) = name_role
        && r != pre.role
    {
        eprintln!(
            "警告: 文件 {} 名称声明为 {}，与头部声明 {} 不一致，已采用头部声明",
            opts.input.display(),
            r,
            pre.role
        );
    }

    // ---- 第 2 段：按角色自动分派对应工具 ----
    match dispatch(pre.role) {
        // 编译类角色（logic/script → 可执行；class/type → 静态库 .a）
        Dispatch::Compile | Dispatch::CompileLib => compile_program(opts, &pre),
        Dispatch::Data => Ok(CompileOutcome {
            message: "[预处理] 识别为 data 数据交换文件，已转交数据解析工具链 —— v0.1 数据交换工具链尚未实现"
                .to_string(),
            artifact: None,
        }),
        Dispatch::Ui => Ok(CompileOutcome {
            message: "[预处理] 识别为 ui 界面文件，已转交 UI 工具链 —— v0.1 UI 工具链尚未实现".to_string(),
            artifact: None,
        }),
        Dispatch::Db => Ok(CompileOutcome {
            message: "[预处理] 识别为 db 数据库文件，已转交数据库工具链 —— v0.1 数据库工具链尚未实现".to_string(),
            artifact: None,
        }),
        Dispatch::Port => Ok(CompileOutcome {
            message: "[预处理] 识别为 port 接口文件，已转交端口工具链 —— v0.1 端口工具链尚未实现".to_string(),
            artifact: None,
        }),
    }
}

/// 编译工具链：前端 → 中端 → 后端（logic/script/class/type 角色）。
fn compile_program(
    opts: &CompileOptions,
    pre: &PreprocessResult,
) -> Result<CompileOutcome, CompileError> {
    // ---- 前端 ----
    // 词法分析（含 ASI 分号补全）
    let tokens = tie_frontend::lexer::tokenize(&pre.cleaned_source).map_err(CompileError::Lex)?;
    // 语法分析
    let program = parse_program(&tokens).map_err(CompileError::Parse)?;
    // import 展开：递归加载被导入文件并内联其顶层函数（含循环检测）。
    // 实现位于 tie-frontend 的 imports 模块（tie-llvm 与 tie-lsp 共享）。
    let base_dir = opts
        .input
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let program = imports::expand_imports(program, &base_dir).map_err(CompileError::Import)?;
    // 语义分析（符号表 + 类型检查）
    let sem = analyze(&program).map_err(CompileError::Semantic)?;

    // 入口检查：logic/script 角色必须定义 main（class/type 库角色不需要）
    if !opts.emit_ir_only
        && matches!(pre.role, FileRole::Logic | FileRole::Script)
        && !sem.funcs.contains_key("main")
    {
        return Err(CompileError::Semantic(SemanticError {
            span: tie_frontend::lexer::Span { line: 1, col: 1 },
            message: "文件角色为 logic/script，必须定义入口函数 main".into(),
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

    // 中端优化 + 后端（opt / 链接 / 归档）
    compile_from_ir(&ir_path, &ir_out, pre, opts)
}

/// 从已生成的 IR 文件继续编译：opt 中间优化 → 按角色生成最终产物。
///
/// 供 [compile_program]（单文件编译）与 tie 总入口的分片并行流水线
/// （多线程编译，各切片的前端可并行、后端共享本函数）复用。
pub fn compile_from_ir(
    ir_path: &Path,
    ir_out: &ir::IrOutput,
    pre: &PreprocessResult,
    opts: &CompileOptions,
) -> Result<CompileOutcome, CompileError> {
    // 优化级别仅来自 CLI（旧头部 `opt=` 选项已随 // tie: 指令系统移除）
    let opt_level = opts.opt_level.unwrap_or_default();

    // 目标三元组仅来自 CLI（旧头部 `target=` 选项已随 // tie: 指令系统移除）；
    // 经平台别名规范化（win-x64 → x86_64-pc-windows-msvc）
    let target = opts
        .target
        .clone()
        .map(|t| normalize_target(t.trim()));

    // opt 中间优化
    let opt_ir_path = ir_path.with_extension("opt.ll");
    optimizer::optimize(ir_path, &opt_ir_path, opt_level).map_err(CompileError::Optimize)?;

    // ---- 后端 ----
    // 按角色区分产物：logic/script → 链接可执行文件；class/type → 编译目标文件 + 打包静态库
    match pre.role {
        FileRole::Logic | FileRole::Script => {
            let exe_path = opts
                .output
                .clone()
                .unwrap_or_else(|| opts.input.with_extension("exe"));
            // REPL 自举：程序用到 tie-interp 库导出（read_line/eval）时，
            // 按需解析并链接 tie-interp 静态库（used_externs 非空才链接）。
            let extra_libs = if ir_out.used_externs.is_empty() {
                Vec::new()
            } else {
                // 跨 target 守卫：interp 库是本机编译产物，交叉编译到其他
                // 平台时无法链接（架构/系统库不匹配），直接报错提示。
                if let Some(t) = &target
                    && t != host_target()
                {
                    return Err(CompileError::Backend(backend::BackendError::RunFailed(
                        format!(
                            "程序使用了 REPL 内置函数（read_line/eval），但目标 '{t}' 与本机 {} 不同：\
                             tie-interp 静态库仅本机构建，暂不支持带 interp 依赖的交叉编译",
                            host_target()
                        ),
                    )));
                }
                let lib = resolve_interp_lib().ok_or_else(|| {
                    CompileError::Backend(backend::BackendError::RunFailed(
                        "程序使用了 REPL 内置函数（read_line/eval），但未找到 tie-interp 静态库。\
                         请先构建 tie-interp（cargo build -p tie-interp --release），\
                         或设置环境变量 TIE_INTERP_LIB 指向 tie_interp.lib"
                            .into(),
                    ))
                })?;
                vec![lib]
            };
            backend::link(&opt_ir_path, &exe_path, target.as_deref(), &extra_libs)
                .map_err(CompileError::Backend)?;
            cleanup_intermediates(ir_path, &opt_ir_path, opts.keep_intermediate);
            Ok(CompileOutcome {
                message: format!("编译成功: {}", exe_path.display()),
                artifact: Some(exe_path),
            })
        }
        FileRole::Class | FileRole::Type => {
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
            cleanup_intermediates(ir_path, &opt_ir_path, opts.keep_intermediate);
            let _ = fs::remove_file(&obj_path);
            Ok(CompileOutcome {
                message: format!("库编译成功: {}", lib_path.display()),
                artifact: Some(lib_path),
            })
        }
        // 理论不可达：compile_from_ir 只由 logic/script/class/type 角色调用
        _ => unreachable!("compile_from_ir 仅处理 logic/script/class/type 角色"),
    }
}

/// 删除中间文件（.ll / .opt.ll），`keep` 为 true 时保留。
fn cleanup_intermediates(ir: &Path, opt_ir: &Path, keep: bool) {
    if !keep {
        let _ = fs::remove_file(ir);
        let _ = fs::remove_file(opt_ir);
    }
}

/// class/type 角色默认输出路径：`<输入名>.a`（如 `lib_math.tie` → `lib_math.a`）。
fn lib_output_path(input: &Path) -> PathBuf {
    input.with_extension("a")
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

/// 本机目标三元组（交叉编译守卫用：interp 库仅本机构建）。
fn host_target() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "aarch64-pc-windows-msvc"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "arm64-apple-darwin"
    } else {
        "unknown-target"
    }
}

/// 解析 tie-interp 静态库路径（REPL 自举：read_line/eval 依赖）。
///
/// 查找顺序：
/// 1. 环境变量 `TIE_INTERP_LIB`（显式指定，最高优先级）；
/// 2. 本机 target/release 或 target/debug（cargo 构建产物）；
/// 3. 当前可执行文件所在目录（发布部署时与 tie.exe 同目录）。
fn resolve_interp_lib() -> Option<PathBuf> {
    // 1. 环境变量显式指定
    if let Some(p) = std::env::var_os("TIE_INTERP_LIB") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    // Windows 静态库文件名 tie_interp.lib；其他平台 libtie_interp.a
    let lib_name = if cfg!(target_os = "windows") {
        "tie_interp.lib"
    } else {
        "libtie_interp.a"
    };
    // 2. cargo 构建产物目录（相对当前工作目录向上找 target）
    for dir in ["target/release", "target/debug"] {
        let p = Path::new(dir).join(lib_name);
        if p.exists() {
            return Some(p);
        }
    }
    // 3. 当前可执行文件所在目录
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.parent().map(|d| d.join(lib_name))?;
        if p.exists() {
            return Some(p);
        }
    }
    None
}
