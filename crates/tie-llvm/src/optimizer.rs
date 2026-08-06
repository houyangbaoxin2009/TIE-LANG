//! 中端优化：调用 LLVM `opt` 对生成的 IR 做中间优化。
//!
//! 对应《编译原理》的中间代码优化阶段，具体优化交给 LLVM 完成，
//! 本模块只负责定位 `opt` 并按其 CLI 约定组织参数。

use std::path::Path;
use std::process::Command;

/// 中间优化错误。
#[derive(Debug)]
pub enum OptError {
    /// opt 可执行文件不存在
    NotFound,
    /// opt 调用失败（含 stderr 信息）
    RunFailed(String),
}

impl std::fmt::Display for OptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptError::NotFound => write!(f, "未找到 LLVM opt，请确认 LLVM 已安装并在 PATH 中"),
            OptError::RunFailed(msg) => write!(f, "opt 优化失败: {msg}"),
        }
    }
}

/// 优化级别（映射到 opt -O0..O3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    O0,
    O1,
    #[default]
    O2,
    O3,
}

impl OptLevel {
    /// opt 命令行参数。
    pub fn flag(self) -> &'static str {
        match self {
            OptLevel::O0 => "-O0",
            OptLevel::O1 => "-O1",
            OptLevel::O2 => "-O2",
            OptLevel::O3 => "-O3",
        }
    }
}

/// 执行中间优化：`opt -O2 input.ll -o output.ll`。
///
/// 输入输出均为 LLVM IR 文本文件。优化失败的场景（如 IR 非法）
/// 会在驱动层转换为用户可见的错误。
pub fn optimize(input: &Path, output: &Path, level: OptLevel) -> Result<(), OptError> {
    // 依次查找 PATH 与常见安装位置
    let opt = find_opt().ok_or(OptError::NotFound)?;
    run_opt(&opt, input, output, level)
}

/// 查找 opt 可执行文件（PATH → D:\LLVM\bin 等常见位置）。
fn find_opt() -> Option<std::path::PathBuf> {
    if let Some(path) = which("opt") {
        return Some(path);
    }
    // 常见 LLVM 安装目录兜底
    for dir in ["D:\\LLVM\\bin", "C:\\Program Files\\LLVM\\bin", "C:\\LLVM\\bin"] {
        let p = Path::new(dir).join("opt.exe");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 在 PATH 中查找可执行文件。
fn which(name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(format!("{name}.exe"));
        if candidate.exists() {
            return Some(candidate);
        }
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// 实际执行 opt。
fn run_opt(
    opt: &std::path::Path,
    input: &Path,
    output: &Path,
    level: OptLevel,
) -> Result<(), OptError> {
    let out = Command::new(opt)
        .arg(level.flag())
        .arg("-S") // 输出文本 IR 而非 bitcode
        .arg(input)
        .arg("-o")
        .arg(output)
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(OptError::RunFailed(
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        )),
        Err(e) => Err(OptError::RunFailed(e.to_string())),
    }
}
