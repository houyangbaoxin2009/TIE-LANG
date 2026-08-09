//! tie-frontend 独立 CLI：前端工具（词法 → 语法 → 语义）。
//!
//! 与 tie-prep / tie-llvm / tie-interp 一样可单独使用，用于调试与教学：
//! 直接读入 `.tie` 源文件，依次执行前端三阶段，输出分析结果或诊断。
//!
//! 用法：
//! ```text
//! tie-frontend <input.tie>           三阶段全跑：输出统计，出错打印首个错误
//! tie-frontend <input.tie> --tokens  只输出 token 流（含 ASI 补全的分号）
//! tie-frontend <input.tie> --ast     只输出 AST（调试视图，{:#?} 格式）
//! tie-frontend <input.tie> --check   只做语义检查：成功静默，失败打印错误
//! tie-frontend -h | --help           显示帮助
//! ```
//!
//! 注意：`// tie:` 头部指令在词法层被当作行注释跳过（与 tie-llvm 一致，
//! 前端不关心文件角色，那是 tie-prep 的职责）。

use std::env;
use std::fs;
use std::process::ExitCode;

use tie_frontend::lexer::{tokenize, Token, TokenKind};
use tie_frontend::parser::parse_program;
use tie_frontend::semantic::analyze;

/// 使用说明。
const USAGE: &str = "\
tie 语言前端工具（词法 → 语法 → 语义）

用法:
  tie-frontend <input.tie> [选项]

功能:
  1. 词法分析（含 ASI 自动分号补全）
  2. 语法分析（递归下降生成 AST）
  3. 语义分析（符号表 + 类型检查）

选项:
  --tokens    只输出 token 流（含 ASI 补全的分号），不继续解析
  --ast       只输出 AST 调试视图，不继续语义分析
  --check     只做语义检查（成功静默；失败打印首个错误）
  --version   显示版本号与内部代号
  -h, --help  显示本帮助

默认模式（无选项）: 三阶段全跑，输出统计信息；任一出错即打印首个错误。
";

/// 命令行参数解析结果。
struct Args {
    input: String,
    mode: Mode,
}

/// 运行模式。
enum Mode {
    /// 三阶段全跑，输出统计
    All,
    /// 只输出 token 流
    Tokens,
    /// 只输出 AST
    Ast,
    /// 只做语义检查
    Check,
}

/// 内部代号（架构代号）：与主入口 tie 保持一致。
const CODENAME: &str = "Harbor";

/// 正式发行版号（年份.修订号）：与主入口 tie 保持一致。
const RELEASE_VERSION: &str = "2026.1";

/// 手动解析命令行参数（不引入 clap 依赖，保持轻量）。
fn parse_args() -> Result<Args, String> {
    let mut input: Option<String> = None;
    let mut mode = Mode::All;

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                // 组件版本号（x.y.z）+ 发行版号（年份.修订号）+ 内部代号
                println!(
                    "tie-frontend {} (发行版 {} \"{}\")",
                    env!("CARGO_PKG_VERSION"),
                    RELEASE_VERSION,
                    CODENAME
                );
                std::process::exit(0);
            }
            "--tokens" => mode = Mode::Tokens,
            "--ast" => mode = Mode::Ast,
            "--check" => mode = Mode::Check,
            other if other.starts_with('-') => {
                return Err(format!("未知选项: {other}"));
            }
            other => {
                if input.is_some() {
                    return Err("只能指定一个输入文件".into());
                }
                input = Some(other.to_string());
            }
        }
    }

    let input = input.ok_or("缺少输入文件，使用 --help 查看用法")?;
    Ok(Args { input, mode })
}

/// token 种类的可读描述（调试输出用）。
fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(n) => format!("标识符 '{n}'"),
        TokenKind::Int(v) => format!("整数 {v}"),
        TokenKind::Float(v) => format!("浮点 {v}"),
        TokenKind::Str(_) => "字符串".into(),
        TokenKind::CharLit(c) => format!("字符 '{c}'"),
        TokenKind::TypeKw(t) => format!("类型 '{}'", t.as_str()),
        other => format!("{other:?}"),
    }
}

/// 打印 token 流（含位置；ASI 补全的分号无独立标记，与手写分号同显示）。
fn dump_tokens(tokens: &[Token]) {
    println!("共 {} 个 token（含 Eof）:", tokens.len());
    for t in tokens {
        println!("  {}:{}  {}", t.span.line, t.span.col, describe(&t.kind));
    }
}

fn main() -> ExitCode {
    // 启动即把 Windows 控制台切到 UTF-8，保证中文输出不乱码
    tie_frontend::init_console_utf8();

    // 解析参数（错误直接输出并退出）
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("错误: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    // 读取源码
    let source = match fs::read_to_string(&args.input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("错误: 读取 {} 失败: {e}", args.input);
            return ExitCode::FAILURE;
        }
    };

    // ---- 第 1 阶段：词法分析（含 ASI） ----
    let tokens = match tokenize(&source) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("词法错误 @{}:{}: {}", e.span.line, e.span.col, e.message);
            return ExitCode::FAILURE;
        }
    };

    // 只输出 token 流：到此为止
    if matches!(args.mode, Mode::Tokens) {
        dump_tokens(&tokens);
        return ExitCode::SUCCESS;
    }

    // ---- 第 2 阶段：语法分析 ----
    let program = match parse_program(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("语法错误 @{}:{}: {}", e.span.line, e.span.col, e.message);
            return ExitCode::FAILURE;
        }
    };

    // 只输出 AST：到此为止
    if matches!(args.mode, Mode::Ast) {
        println!("{program:#?}");
        return ExitCode::SUCCESS;
    }

    // ---- 第 3 阶段：语义分析 ----
    let sem = match analyze(&program) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("语义错误 @{}:{}: {}", e.span.line, e.span.col, e.message);
            return ExitCode::FAILURE;
        }
    };

    // --check：成功静默
    if matches!(args.mode, Mode::Check) {
        return ExitCode::SUCCESS;
    }

    // ---- 默认模式：输出统计 ----
    // 顶层语句统计
    let (fn_count, struct_count, import_count) =
        program.stmts.iter().fold((0, 0, 0), |(f, c, i), s| match s {
            tie_frontend::ast::Stmt::FnDef(_) => (f + 1, c, i),
            tie_frontend::ast::Stmt::Struct(_) => (f, c + 1, i),
            tie_frontend::ast::Stmt::Import(_) => (f, c, i + 1),
            _ => (f, c, i),
        });

    println!("前端分析通过: {}", args.input);
    println!("  token 数: {}", tokens.len() - 1); // 减去 Eof
    println!("  顶层函数: {fn_count}");
    println!("  顶层 struct: {struct_count}");
    println!("  import 语句: {import_count}");
    println!("  函数签名: {}", sem.funcs.len());
    println!("  类信息: {}", sem.classes.len());
    println!("  表达式类型推断条目: {}", sem.expr_types.len());
    println!("  表（table）布局: {}", sem.tables.len());
    ExitCode::SUCCESS
}
