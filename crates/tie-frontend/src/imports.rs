//! import 展开模块：递归加载被导入文件，把其顶层语句内联进当前程序。
//!
//! 原实现是 tie-llvm 的私有函数（driver.rs 的 `expand_imports`）。LSP 等
//! 工具也需要跨文件分析（识别 `str.split` 等命名空间调用中被导入文件的
//! 函数定义），故提炼为本 crate 的公共 API，供 tie-llvm 与 tie-lsp 共享。

use crate::ast::{Program, Stmt};
use crate::lexer::Span;
use crate::parser::parse_program;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// 清理被导入文件源码：去 BOM、CRLF 归一、剥离头部行。
///
/// 语义与 tie-prep 的 `preprocess` 一致（正文 = 第一个非头部内容行起）。
/// 注意：tie-frontend 处于依赖图底层（被 tie-interp 依赖），而 Harbor M3 自举后
/// tie-prep 将依赖 tie-interp（解释执行 tie 语言编写的预处理模块）——
/// 若此处复用 tie-prep 会形成 `frontend → prep → interp → frontend` 循环依赖，
/// 故 import 展开自带轻量清理，不依赖 tie-prep crate。
fn clean_source(source: &str) -> String {
    let source = source.trim_start_matches('\u{FEFF}');
    let source = source.replace("\r\n", "\n");
    // 剥离头部：文件最前面的连续 `// tie:` 行（允许其间空行）
    let mut body_start = 0;
    let mut in_header_zone = true;
    for (idx, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if in_header_zone {
            if trimmed.is_empty() {
                // 头部区域内的空行：跳过（仍算头部区域）
                continue;
            }
            if trimmed.strip_prefix("// tie:").is_some() {
                continue;
            }
            // 第一个非头部内容行：正文从这里开始
            body_start = idx;
            in_header_zone = false;
        }
    }
    source.lines().skip(body_start).collect::<Vec<_>>().join("\n")
}

/// import 展开错误：携带位置与信息（span 指向 import 语句处）。
#[derive(Debug, Clone)]
pub struct ImportError {
    /// 出错位置（import 语句的 span）
    pub span: Span,
    /// 错误描述（读取失败 / 词法错误 / 语法错误 / 循环导入）
    pub message: String,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "导入错误 @{}:{}: {}", self.span.line, self.span.col, self.message)
    }
}

/// 递归展开 import 语句：加载被导入文件并把其顶层语句内联进当前程序。
///
/// - 路径解析：`import "./x.tie"` 的路径相对**当前文件所在目录**；
/// - 被导入文件自身也可含 import（递归展开，支持多级导入）；
/// - 循环检测：用规范化绝对路径集合记录「展开链」，重复访问即报循环导入。
pub fn expand_imports(program: Program, base_dir: &Path) -> Result<Program, ImportError> {
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
) -> Result<Program, ImportError> {
    let mut out = Vec::new();
    for stmt in program.stmts {
        match stmt {
            Stmt::Import(imp) => {
                let import_path = base_dir.join(&imp.path);
                // 规范化路径（解析 . / ..），用于循环检测
                let canon = fs::canonicalize(&import_path).unwrap_or_else(|_| import_path.clone());
                if !chain.insert(canon.clone()) {
                    return Err(ImportError {
                        span: imp.span,
                        message: format!("循环导入：文件 '{}' 已在导入链中", imp.path),
                    });
                }
                // 读取 + 清理（去 BOM/CRLF/剥离头部）+ 词法 + 语法解析被导入文件
                let source = fs::read_to_string(&import_path).map_err(|e| ImportError {
                    span: imp.span,
                    message: format!("导入文件 {} 读取失败: {e}", import_path.display()),
                })?;
                let cleaned = clean_source(&source);
                let tokens = crate::lexer::tokenize(&cleaned).map_err(|e| ImportError {
                    span: imp.span,
                    message: format!("导入文件 {} 词法错误: {e}", imp.path),
                })?;
                let sub_program = parse_program(&tokens).map_err(|e| ImportError {
                    span: imp.span,
                    message: format!("导入文件 {} 语法错误: {}", imp.path, e.message),
                })?;
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
