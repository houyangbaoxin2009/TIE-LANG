//! tie-llvm：tie 语言编译器命令行入口（可单独使用）。
//!
//! 用法：
//! ```text
//! tie-llvm <input.tie> [-o 输出文件] [-O0|O1|O2|O3] [--emit-ir] [--keep-ir]
//! ```
//!
//! 流水线（四段式）：tie-prep 预处理 → 自研前端 → LLVM IR → opt 优化 → clang/lld 链接。

use tie_llvm::driver::CompileOptions;
use tie_llvm::optimizer::OptLevel;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

/// 命令行参数解析结果。
struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
    opt_level: Option<OptLevel>,
    emit_ir_only: bool,
    keep_ir: bool,
    target: Option<String>,
}

/// 使用说明文本。
const USAGE: &str = "\
tie 语言编译器（前端自研 + LLVM 中后端）

用法:
  tie-llvm <input.tie> [选项]

选项:
  -o <file>      指定输出文件路径（默认: 输入同名 .exe；library 角色默认 .a）
  -O0|-O1|-O2|-O3
                 优化级别（默认: -O2）
  --target <三元组>
                 交叉编译目标（如 x86_64-pc-windows-msvc / win-x64，默认: 本机）
  --emit-ir      只生成 LLVM IR（.ll），不继续编译
  --keep-ir      保留中间 IR 文件
  -h, --help     显示本帮助
";

/// 手动解析命令行参数（不引入 clap 依赖，保持轻量）。
fn parse_args() -> Result<Args, String> {
    let mut args = env::args().skip(1);
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut opt_level: Option<OptLevel> = None;
    let mut emit_ir_only = false;
    let mut keep_ir = false;
    let mut target: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-o" => {
                output = Some(PathBuf::from(
                    args.next().ok_or("-o 后缺少输出文件路径")?,
                ));
            }
            "-O0" => opt_level = Some(OptLevel::O0),
            "-O1" => opt_level = Some(OptLevel::O1),
            "-O2" => opt_level = Some(OptLevel::O2),
            "-O3" => opt_level = Some(OptLevel::O3),
            "--target" => {
                target = Some(args.next().ok_or("--target 后缺少目标三元组")?);
            }
            // 兼容 `--target=<三元组>` 写法
            other if other.starts_with("--target=") => {
                target = Some(other["--target=".len()..].to_string());
            }
            "--emit-ir" => emit_ir_only = true,
            "--keep-ir" => keep_ir = true,
            other if other.starts_with('-') => {
                return Err(format!("未知选项: {other}"));
            }
            other => {
                if input.is_some() {
                    return Err("只能指定一个输入文件".into());
                }
                input = Some(PathBuf::from(other));
            }
        }
    }

    let input = input.ok_or("缺少输入文件，使用 --help 查看用法")?;
    Ok(Args { input, output, opt_level, emit_ir_only, keep_ir, target })
}

fn main() -> ExitCode {
    // 解析参数（错误直接输出并退出）
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("错误: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let opts = CompileOptions {
        input: args.input,
        output: args.output,
        opt_level: args.opt_level,
        emit_ir_only: args.emit_ir_only,
        keep_intermediate: args.keep_ir,
        target: args.target,
    };

    // 编译（四段式：预处理 → 前端 → 中端 → 后端）
    match tie_llvm::driver::compile(&opts) {
        Ok(outcome) => {
            println!("{}", outcome.message);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
