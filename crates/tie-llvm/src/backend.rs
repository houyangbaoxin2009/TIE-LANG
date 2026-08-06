//! 后端：调用 LLVM 工具链（clang/lld）把优化后的 IR 编译链接为可执行文件。
//!
//! 对应《编译原理》的目标代码生成与链接阶段，具体工作交给 LLVM：
//! `clang optimized.ll -o output.exe`（内部自动完成汇编与链接）。

use std::path::Path;
use std::process::Command;

/// 后端编译错误。
#[derive(Debug)]
pub enum BackendError {
    /// clang 可执行文件不存在
    NotFound,
    /// clang 调用失败（含 stderr）
    RunFailed(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::NotFound => {
                write!(f, "未找到 clang，请确认 LLVM 已安装并在 PATH 中")
            }
            BackendError::RunFailed(msg) => write!(f, "后端编译失败: {msg}"),
        }
    }
}

/// 链接生成可执行文件：`clang input.ll -o output`。
///
/// clang 会自行完成：IR → 汇编 → 目标文件 → 链接（链接 CRT 与系统库）。
pub fn link(input: &Path, output: &Path) -> Result<(), BackendError> {
    let clang = find_clang().ok_or(BackendError::NotFound)?;
    let out = Command::new(&clang)
        .arg(input)
        .arg("-o")
        .arg(output)
        // Windows 下避免弹出控制台窗口（GUI 程序用 -mwindows 由后续版本按头类型控制）
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(BackendError::RunFailed(
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        )),
        Err(e) => Err(BackendError::RunFailed(e.to_string())),
    }
}

/// 查找 clang 可执行文件（PATH → 常见安装位置）。
fn find_clang() -> Option<std::path::PathBuf> {
    if let Some(path) = which("clang") {
        return Some(path);
    }
    for dir in ["D:\\LLVM\\bin", "C:\\Program Files\\LLVM\\bin", "C:\\LLVM\\bin"] {
        let p = Path::new(dir).join("clang.exe");
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
