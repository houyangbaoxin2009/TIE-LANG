//! tie-prep 独立 CLI：预处理工具（四段式第一段）。
//!
//! 用法：
//! ```text
//! tie-prep <input.tie>           清理后正文输出到 stdout，角色信息输出到 stderr
//! tie-prep <input.tie> --info    只输出角色信息（人类可读），不输出正文
//! tie-prep <input.tie> --module prep/indent.tie
//!                                 挂载自定义 tie 转换器模块（顶层 process(src)->string），
//!                                 输出为模块处理后的文本（Harbor M3 可扩展性）
//! tie-prep -h | --help           显示帮助
//! ```
//!
//! 管道友好：默认模式把清理后的正文写到 stdout，可直接接给编译器
//! （`tie-prep a.tie | tie-llvm -` 由后续版本支持）。

use std::env;
use std::fs;
use std::io::{self, Write};
use std::process::ExitCode;

/// 使用说明。
const USAGE: &str = "\
tie 语言预处理工具（四段式第一段）

用法:
  tie-prep <input.tie> [选项]

功能:
  1. 清理代码（去 BOM、统一换行、剥离声明行）
  2. 识别文件类型（解析文件头 type tie / type tie<X> 声明）
  3. 判定角色，供调用方自动转交对应工具链
  4. 挂载 tie 模块（--module）做自定义转换（Harbor M3 可扩展性）

选项:
  --info            只打印角色信息，不输出正文
  --module <file>   挂载模块：解释执行该 tie 文件，调用其 process(src)->string
                    输出转换结果（证明新增转换器只需写 tie 模块、不改 Rust）
  --version         显示版本号与内部代号
  -h, --help        显示本帮助

输出约定:
  默认模式  清理后的正文 → stdout；角色信息 → stderr
  --info    角色信息 → stdout
";

/// 内部代号（架构代号）：与主入口 tie 保持一致。
const CODENAME: &str = "Harbor";

/// 正式发行版号（年份.修订号）：与主入口 tie 保持一致。
const RELEASE_VERSION: &str = "2026.1";

fn main() -> ExitCode {
    // 启动即把 Windows 控制台切到 UTF-8，保证中文输出不乱码
    tie_prep::init_console_utf8();

    let args = env::args().skip(1);
    let mut input: Option<String> = None;
    let mut info_only = false;
    let mut module: Option<String> = None;

    // 逐个解析参数（--module 需要吞下一个参数）
    let mut iter = args.peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                // 组件版本号（x.y.z）+ 发行版号（年份.修订号）+ 内部代号
                println!(
                    "tie-prep {} (发行版 {} \"{}\")",
                    env!("CARGO_PKG_VERSION"),
                    RELEASE_VERSION,
                    CODENAME
                );
                return ExitCode::SUCCESS;
            }
            "--info" => info_only = true,
            "--module" => {
                // 下一个参数是模块文件路径
                let Some(path) = iter.next() else {
                    eprintln!("错误: --module 需要一个模块文件参数\n\n{USAGE}");
                    return ExitCode::from(2);
                };
                if module.is_some() {
                    eprintln!("错误: 只能指定一个 --module\n\n{USAGE}");
                    return ExitCode::from(2);
                }
                module = Some(path);
            }
            other if other.starts_with('-') => {
                eprintln!("错误: 未知选项 {other}\n\n{USAGE}");
                return ExitCode::from(2);
            }
            other => {
                if input.is_some() {
                    eprintln!("错误: 只能指定一个输入文件\n\n{USAGE}");
                    return ExitCode::from(2);
                }
                input = Some(other.to_string());
            }
        }
    }

    let Some(input) = input else {
        eprintln!("错误: 缺少输入文件\n\n{USAGE}");
        return ExitCode::from(2);
    };

    // --module 与 --info 语义冲突：info 展示预处理的角色信息，模块模式下
    // 输出的是模块转换结果，无角色概念。
    if module.is_some() && info_only {
        eprintln!("错误: --module 不能与 --info 同时使用\n\n{USAGE}");
        return ExitCode::from(2);
    }

    // 读取源码
    let source = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("错误: 读取 {input} 失败: {e}");
            return ExitCode::FAILURE;
        }
    };

    // 挂载自定义 tie 模块（Harbor M3 可扩展性）：解释执行模块文件，
    // 调用其顶层 process(src) -> string，原始源码直传，输出转换结果。
    if let Some(module_path) = module {
        let module_src = match fs::read_to_string(&module_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("错误: 读取模块 {module_path} 失败: {e}");
                return ExitCode::FAILURE;
            }
        };
        // 模块入口约定：顶层 process(src: string) -> string
        let out = match tie_prep::run_module(&module_src, "process", &source) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("错误: 模块 {module_path} 执行失败: {e}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(e) = io::stdout().write_all(out.as_bytes()) {
            eprintln!("错误: 写入 stdout 失败: {e}");
            return ExitCode::FAILURE;
        }
        eprintln!("[tie-prep] 文件: {input} | 模块: {module_path} | 转换完成");
        return ExitCode::SUCCESS;
    }

    // 预处理：清理 + 识别文件类型 + 角色判定
    let result = tie_prep::preprocess(&source);

    if info_only {
        println!("文件: {input}");
        println!("角色: {}", result.role);
        // 文件名默认角色仅供参考（头部声明优先于文件名声明）
        if let Some(fr) = tie_prep::role_from_filename(&input) {
            println!("文件名默认角色: {fr}（头部声明优先）");
        }
        return ExitCode::SUCCESS;
    }

    // 默认模式：正文 → stdout，角色信息 → stderr
    if let Err(e) = io::stdout().write_all(result.cleaned_source.as_bytes()) {
        eprintln!("错误: 写入 stdout 失败: {e}");
        return ExitCode::FAILURE;
    }
    eprintln!("[tie-prep] 文件: {input} | 角色: {}", result.role);
    ExitCode::SUCCESS
}
