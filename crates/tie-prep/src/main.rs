//! tie-prep 独立 CLI：预处理工具（四段式第一段）。
//!
//! 用法：
//! ```text
//! tie-prep <input.tie>           清理后正文输出到 stdout，角色信息输出到 stderr
//! tie-prep <input.tie> --info    只输出角色与头部信息（人类可读），不输出正文
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
  1. 清理代码（去 BOM、统一换行、剥离头部）
  2. 识别文件类型（解析文件头 // tie: 指令）
  3. 判定角色，供调用方自动转交对应工具链

选项:
  --info      只打印角色与头部信息，不输出正文
  --version   显示版本号与内部代号
  -h, --help  显示本帮助

输出约定:
  默认模式  清理后的正文 → stdout；角色信息 → stderr
  --info    角色信息 → stdout
";

/// 内部代号（架构代号）：与主入口 tie 保持一致。
const CODENAME: &str = "Harbor";

/// 正式发行版号（年份.修订号）：与主入口 tie 保持一致。
const RELEASE_VERSION: &str = "2026.1";

fn main() -> ExitCode {
    let args = env::args().skip(1);
    let mut input: Option<String> = None;
    let mut info_only = false;

    for arg in args {
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

    // 读取源码
    let source = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("错误: 读取 {input} 失败: {e}");
            return ExitCode::FAILURE;
        }
    };

    // 预处理：清理 + 识别文件类型 + 角色判定
    let result = tie_prep::preprocess(&source);

    if info_only {
        println!("文件: {input}");
        println!("角色: {}", result.role);
        println!("头部: {}", if result.headers.is_empty() {
            "(无)".to_string()
        } else {
            result
                .headers
                .iter()
                .map(|h| format!("// tie:{}", h.raw))
                .collect::<Vec<_>>()
                .join(" | ")
        });
        return ExitCode::SUCCESS;
    }

    // 默认模式：正文 → stdout，角色信息 → stderr
    if let Err(e) = io::stdout().write_all(result.cleaned_source.as_bytes()) {
        eprintln!("错误: 写入 stdout 失败: {e}");
        return ExitCode::FAILURE;
    }
    eprintln!("[tie-prep] 文件: {input} | 角色: {} | 头部: {}", result.role, result.headers.len());
    ExitCode::SUCCESS
}
