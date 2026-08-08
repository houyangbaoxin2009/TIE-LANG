//! tie：tie 语言总入口（四段式调度器 + REPL）。
//!
//! 四段式架构：`预处理 [前端 中间优化 后端]`
//!
//! `tie` 是日常使用的一般入口（合并了原 tie-cli 职责），功能：
//! 1. **无参数** → 进入 REPL 交互模式（调用 tie-interp，逐行解释执行）；
//! 2. **传入文件** → 执行 .tie 脚本：
//!    - 调用 **tie-prep** 完成预处理（清理代码 + 识别文件类型 + 角色判定）；
//!    - 按文件角色**自动转交对应的工具链**：
//!      - `logic` / `library` → 转交 **tie-llvm**（前端 + 中间优化 + 后端）
//!      - `data` / `ui` / `db` → 识别后提示（对应工具链后续版本实现）
//!
//! 用户也可绕过本入口单独使用 tie-prep（纯预处理）、tie-llvm（直接编译）
//! 或 tie-interp（解释执行）。

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use tie_prep::preprocess::FileRole;
use tie_llvm::driver::{CompileOptions, CompileOutcome};
use tie_llvm::optimizer::OptLevel;

mod cache;
mod config;
mod pipeline;

/// 命令行参数（编译类选项透传给 tie-llvm）。
struct Args {
    /// 输入文件/目录列表（多文件或目录 = 编译项目；启用高级编译时按文件切片并行）
    inputs: Vec<PathBuf>,
    output: Option<PathBuf>,
    opt_level: Option<OptLevel>,
    emit_ir_only: bool,
    keep_ir: bool,
    /// 只做预处理并打印识别结果，不转交任何工具链
    prep_only: bool,
    /// 交叉编译目标三元组（如 x86_64-pc-windows-msvc / win-x64）
    target: Option<String>,
    /// 启动语言服务器（LSP over stdio），复用 tie-lsp 主循环
    lsp_mode: bool,
    /// 协调统筹配置文件（tie:data 格式；缺省时查找当前目录 tie.config）
    config: Option<PathBuf>,
}

/// 内部代号（架构代号）：代表本发行版的架构特征主题。
/// 2026.1 "Harbor 港湾"：首个正式版 = 工具链第一次靠岸停泊。
const CODENAME: &str = "Harbor";

/// 正式发行版号（年份.修订号）：用于发布产物命名与 git tag，
/// 与组件版本号（CARGO_PKG_VERSION，x.y.z）相互独立。
const RELEASE_VERSION: &str = "2026.1";

/// 使用说明。
const USAGE: &str = "\
tie 语言总入口（四段式调度器 + REPL）

用法:
  tie                进入 REPL 交互模式（逐行解释执行）
  tie --lsp          启动语言服务器（LSP over stdio，供编辑器接入）
  tie <input.tie> [选项]   编译并执行脚本文件
  tie <file...|目录> [选项] 编译项目（多文件/目录；需配置文件开启 advanced.enabled）

流程:
  1. tie-prep 预处理（清理代码 + 识别文件类型）
  2. 按角色自动转交工具链（logic/library → tie-llvm 编译；
     data/ui/db → 对应工具链，后续版本）

选项:
  -o <file>      指定输出文件路径（默认: 输入同名 .exe；library 角色默认 .a；
                 仅单文件模式生效）
  -O0|-O1|-O2|-O3
                 优化级别（默认: -O2）
  --target <三元组>
                 交叉编译目标（如 win-x64 / x86_64-pc-windows-msvc，默认: 本机）
  --config <file>
                 协调统筹配置文件（tie:data 格式；缺省查找当前目录 tie.config，
                 无则默认全关闭）。开启 advanced.enabled 后，多文件/目录输入
                 按文件切片、多线程并行编译（每步产物入缓存池，全部切片完成
                 才进入下一步）；可配置线程数、缓存池大小/位置/存储技术
  --emit-ir      只生成 LLVM IR（.ll），不继续编译
  --keep-ir      保留中间 IR 文件
  --prep-only    只执行预处理并打印识别结果，不编译（仅单文件模式）
  --lsp          以语言服务器模式运行（读 stdin 的 LSP 消息并写 stdout）
  --version      显示版本号与内部代号（如 2026.1 (Harbor)）
  -h, --help     显示本帮助

单独使用:
  tie-prep <file.tie>    只做预处理
  tie-frontend <file.tie> 只做前端三阶段（词法/语法/语义，调试与教学用）
  tie-lsp               语言服务器（LSP over stdio，为编辑器提供诊断与 hover）
  tie-llvm <file.tie>   直接编译（不经过角色分派）
  tie-interp <file.tie> 直接解释执行（不经过角色分派）
";

/// REPL 交互模式入口。
///
/// 启动 tie 语言自写的 REPL 外壳 `repl.exe`（自举：外壳本身用 tie 语言编写，
/// 经 tie-llvm 编译并链接 tie-interp 静态库）。查找顺序：
/// 1. 环境变量 `TIE_REPL_EXE`（显式指定）；
/// 2. 当前可执行文件（tie.exe）所在目录的 repl.exe（发布部署常见布局）；
/// 3. 当前工作目录的 repl.exe（开发期：cargo run -p tie 时在 workspace 根）。
/// 找不到时给出构建提示（repl/repl.tie 编译产物）。
fn repl() -> ExitCode {
    let exe = find_repl_exe();
    let Some(exe) = exe else {
        eprintln!(
            "未找到 REPL 外壳 repl.exe。请先构建（自举）:\n\
             1. cargo build --release -p tie-interp\n\
             2. target\\release\\tie-llvm.exe repl\\repl.tie\n\
             3. 将 repl\\repl.exe 放到当前目录或 tie.exe 同目录\n\
             （或用环境变量 TIE_REPL_EXE 指定路径）"
        );
        // Windows 下直接运行（如双击）时窗口会一闪而过，暂停以让用户看到提示
        pause_before_exit();
        return ExitCode::FAILURE;
    };
    // 子进程接管 stdio：REPL 交互（stdin 输入 + stdout 输出）原样透传
    match std::process::Command::new(&exe).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(e) => {
            eprintln!("启动 REPL 失败: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Windows 直接运行（双击）时暂停，防止控制台窗口一闪而过。
///
/// 仅当 stdin 是交互式终端时暂停（等待按任意键）；管道/重定向场景
/// （如 `tie | grep`、CI 脚本）不暂停，保证脚本可自动执行。
#[cfg(windows)]
fn pause_before_exit() {
    use std::io::{IsTerminal, Read};
    // 交互终端才暂停；stdin 被重定向/管道时不暂停
    if std::io::stdin().is_terminal() {
        eprintln!("按任意键退出...");
        let mut buf = [0u8; 1];
        let _ = std::io::stdin().read(&mut buf);
    }
}

/// 非 Windows 平台：终端行为不同，无需暂停。
#[cfg(not(windows))]
fn pause_before_exit() {}

/// 查找 REPL 外壳可执行文件（env → exe 同目录 → 当前目录 → workspace repl/ 目录）。
fn find_repl_exe() -> Option<PathBuf> {
    // 1. 环境变量显式指定
    if let Some(p) = env::var_os("TIE_REPL_EXE") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    // Windows 下 repl.exe；其他平台 repl
    let exe_name = if cfg!(target_os = "windows") { "repl.exe" } else { "repl" };
    // 2. 当前可执行文件（tie.exe）所在目录
    if let Ok(cur) = env::current_exe() {
        if let Some(dir) = cur.parent() {
            let p = dir.join(exe_name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // 3. 当前工作目录
    let p = PathBuf::from(exe_name);
    if p.is_file() {
        return Some(p);
    }
    // 4. workspace 标准布局：repl/repl.exe（开发期在仓库根直接运行 tie.exe）
    let p = PathBuf::from("repl").join(exe_name);
    if p.is_file() {
        return Some(p);
    }
    None
}

fn parse_args() -> Result<Args, String> {
    let mut args = env::args().skip(1);
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut output: Option<PathBuf> = None;
    let mut opt_level: Option<OptLevel> = None;
    let mut emit_ir_only = false;
    let mut keep_ir = false;
    let mut prep_only = false;
    let mut target: Option<String> = None;
    let mut lsp_mode = false;
    let mut config: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                // 组件版本号（x.y.z）+ 发行版号（年份.修订号）+ 内部代号
                // 如: tie 0.1.0 (发行版 2026.1 "Harbor")
                println!(
                    "tie {} (发行版 {} \"{}\")",
                    env!("CARGO_PKG_VERSION"),
                    RELEASE_VERSION,
                    CODENAME
                );
                std::process::exit(0);
            }
            "-o" => output = Some(PathBuf::from(args.next().ok_or("-o 后缺少输出文件路径")?)),
            "-O0" => opt_level = Some(OptLevel::O0),
            "-O1" => opt_level = Some(OptLevel::O1),
            "-O2" => opt_level = Some(OptLevel::O2),
            "-O3" => opt_level = Some(OptLevel::O3),
            "--target" => target = Some(args.next().ok_or("--target 后缺少目标三元组")?),
            // 兼容 `--target=<三元组>` 写法
            other if other.starts_with("--target=") => {
                target = Some(other["--target=".len()..].to_string());
            }
            "--config" => config = Some(PathBuf::from(args.next().ok_or("--config 后缺少配置文件路径")?)),
            "--emit-ir" => emit_ir_only = true,
            "--keep-ir" => keep_ir = true,
            "--prep-only" => prep_only = true,
            "--lsp" => lsp_mode = true,
            other if other.starts_with('-') => return Err(format!("未知选项: {other}")),
            other => inputs.push(PathBuf::from(other)),
        }
    }

    Ok(Args {
        inputs,
        output,
        opt_level,
        emit_ir_only,
        keep_ir,
        prep_only,
        target,
        lsp_mode,
        config,
    })
}

fn main() -> ExitCode {
    // 启动即把 Windows 控制台切到 UTF-8，保证中文输出不乱码
    tie_prep::init_console_utf8();

    // 无参数 → REPL 交互模式
    if env::args().len() == 1 {
        return repl();
    }

    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("错误: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // --lsp：启动语言服务器（LSP over stdio），复用 tie-lsp 主循环
    if args.lsp_mode {
        return tie_lsp::run_server();
    }
    // 有参数时必须指定输入文件
    if args.inputs.is_empty() {
        eprintln!("错误: 缺少输入文件，使用 --help 查看用法\n\n{USAGE}");
        return ExitCode::from(2);
    }

    // ---- 加载协调统筹配置（tie:data；缺省查 tie.config，无则默认全关闭）----
    let config = match config::load(args.config.as_deref()) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("错误: {msg}");
            return ExitCode::FAILURE;
        }
    };

    // ---- 高级编译（多线程分片 + 缓存池）：配置文件开启后接管全部输入 ----
    if config.advanced.enabled {
        // prep-only 在多文件/分片模式下无意义：仅对单文件路径有效，先拒绝
        if args.prep_only {
            eprintln!("错误: --prep-only 仅支持单文件模式（高级编译模式下不可用）");
            return ExitCode::from(2);
        }
        return match pipeline::Pipeline::new(
            &config,
            &args.inputs,
            args.output.clone(),
            args.opt_level,
            args.emit_ir_only,
            args.keep_ir,
            args.target.clone(),
        ) {
            Ok(p) => match p.run() {
                Ok(outcomes) => {
                    let mut failed = false;
                    for o in &outcomes {
                        println!("{}", o.message);
                    }
                    // 若存在失败产物（消息含"失败"），以非零退出
                    if outcomes.iter().any(|o| o.artifact.is_none() && o.message.contains("失败")) {
                        failed = true;
                    }
                    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
                }
                Err(msg) => {
                    eprintln!("{msg}");
                    ExitCode::FAILURE
                }
            },
            Err(msg) => {
                eprintln!("错误: {msg}");
                ExitCode::FAILURE
            }
        };
    }

    // ---- 默认路径：单文件编译（行为与原版本完全一致）----
    let input = &args.inputs[0];

    // ---- 第 1 段：预处理（tie-prep）----
    let source = match fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("错误: 读取 {} 失败: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let pre = tie_prep::preprocess(&source);

    // 打印预处理识别结果
    println!("[tie] 文件: {} | 角色: {} | 头部: {}", input.display(), pre.role, pre.headers.len());

    // --prep-only：只输出识别结果，不转交工具链
    if args.prep_only {
        return ExitCode::SUCCESS;
    }

    // ---- 第 2 段：按角色自动分派 ----
    match dispatch_role(pre.role, &args, input.clone()) {
        Ok(outcome) => {
            println!("{}", outcome.message);
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

/// 按角色转交对应工具链。
fn dispatch_role(role: FileRole, args: &Args, input: PathBuf) -> Result<CompileOutcome, String> {
    match role {
        // logic / library → tie-llvm 编译工具链
        FileRole::Logic | FileRole::Library => {
            let opts = CompileOptions {
                input,
                output: args.output.clone(),
                opt_level: args.opt_level,
                emit_ir_only: args.emit_ir_only,
                keep_intermediate: args.keep_ir,
                target: args.target.clone(),
            };
            tie_llvm::driver::compile(&opts).map_err(|e| e.to_string())
        }
        // data / ui / db → 对应工具链（v0.1 挂接点）
        FileRole::Data => Ok(CompileOutcome {
            message: "[tie] 角色为 data（数据交换文件），已转交数据解析工具链 —— v0.1 尚未实现".to_string(),
            artifact: None,
        }),
        FileRole::Ui => Ok(CompileOutcome {
            message: "[tie] 角色为 ui（界面文件），已转交 UI 工具链 —— v0.1 尚未实现".to_string(),
            artifact: None,
        }),
        FileRole::Db => Ok(CompileOutcome {
            message: "[tie] 角色为 db（数据库文件），已转交数据库工具链 —— v0.1 尚未实现".to_string(),
            artifact: None,
        }),
    }
}
