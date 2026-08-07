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
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use tie_prep::preprocess::FileRole;
use tie_llvm::driver::{CompileOptions, CompileOutcome};
use tie_llvm::optimizer::OptLevel;

/// 命令行参数（编译类选项透传给 tie-llvm）。
struct Args {
    input: Option<PathBuf>,
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
}

/// 使用说明。
const USAGE: &str = "\
tie 语言总入口（四段式调度器 + REPL）

用法:
  tie                进入 REPL 交互模式（逐行解释执行）
  tie --lsp          启动语言服务器（LSP over stdio，供编辑器接入）
  tie <input.tie> [选项]   编译并执行脚本文件

流程:
  1. tie-prep 预处理（清理代码 + 识别文件类型）
  2. 按角色自动转交工具链（logic/library → tie-llvm 编译；
     data/ui/db → 对应工具链，后续版本）

选项:
  -o <file>      指定输出文件路径（默认: 输入同名 .exe；library 角色默认 .a）
  -O0|-O1|-O2|-O3
                 优化级别（默认: -O2）
  --target <三元组>
                 交叉编译目标（如 win-x64 / x86_64-pc-windows-msvc，默认: 本机）
  --emit-ir      只生成 LLVM IR（.ll），不继续编译
  --keep-ir      保留中间 IR 文件
  --prep-only    只执行预处理并打印识别结果，不编译
  --lsp          以语言服务器模式运行（读 stdin 的 LSP 消息并写 stdout）
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
/// 逐行读取输入并交给解释器执行。当前依赖 tie-interp 占位实现，
/// 后续版本支持多行语句、历史记录与表达式求值。
fn repl() -> ExitCode {
    println!("tie REPL（输入 :quit 退出）");
    let stdin = io::stdin();
    loop {
        print!("> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => return ExitCode::SUCCESS, // EOF（Ctrl+Z / Ctrl+D）
            Ok(_) => {}
            Err(e) => {
                eprintln!("读取输入失败: {e}");
                return ExitCode::FAILURE;
            }
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == ":quit" {
            return ExitCode::SUCCESS;
        }
        // 交给解释器执行（v0.1：占位，输出确认信息）
        let interp = tie_interp::interp_placeholder();
        println!("{line} → {interp}");
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args = env::args().skip(1);
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut opt_level: Option<OptLevel> = None;
    let mut emit_ir_only = false;
    let mut keep_ir = false;
    let mut prep_only = false;
    let mut target: Option<String> = None;
    let mut lsp_mode = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
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
            "--emit-ir" => emit_ir_only = true,
            "--keep-ir" => keep_ir = true,
            "--prep-only" => prep_only = true,
            "--lsp" => lsp_mode = true,
            other if other.starts_with('-') => return Err(format!("未知选项: {other}")),
            other => {
                if input.is_some() {
                    return Err("只能指定一个输入文件".into());
                }
                input = Some(PathBuf::from(other));
            }
        }
    }

    Ok(Args { input, output, opt_level, emit_ir_only, keep_ir, prep_only, target, lsp_mode })
}

fn main() -> ExitCode {
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
    let Some(input) = args.input.as_ref() else {
        eprintln!("错误: 缺少输入文件，使用 --help 查看用法\n\n{USAGE}");
        return ExitCode::from(2);
    };

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
