//! 后端：调用 LLVM 工具链（clang/lld）把优化后的 IR 编译链接为可执行文件或库。
//!
//! 对应《编译原理》的目标代码生成与链接阶段，具体工作交给 LLVM：
//! - `clang optimized.ll -o output.exe`（内部自动完成汇编与链接，生成可执行文件）
//! - `clang -c optimized.ll -o output.o` + `llvm-ar rcs libxxx.a output.o`（生成静态库）
//! - 交叉编译：给 clang 传 `--target=<三元组>`（如 `--target=x86_64-pc-windows-msvc`）

use std::path::Path;
use std::process::Command;

/// 后端编译错误。
#[derive(Debug)]
pub enum BackendError {
    /// clang 可执行文件不存在
    NotFound,
    /// clang 调用失败（含 stderr）
    RunFailed(String),
    /// llvm-ar 可执行文件不存在
    ArNotFound,
    /// llvm-ar 调用失败（含 stderr）
    ArFailed(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::NotFound => {
                write!(f, "未找到 clang，请确认 LLVM 已安装并在 PATH 中")
            }
            BackendError::RunFailed(msg) => write!(f, "后端编译失败: {msg}"),
            BackendError::ArNotFound => {
                write!(f, "未找到 llvm-ar，请确认 LLVM 已安装并在 PATH 中")
            }
            BackendError::ArFailed(msg) => write!(f, "静态库打包失败: {msg}"),
        }
    }
}

/// 链接生成可执行文件：`clang [--target=T] input.ll [附加静态库] -o output`。
///
/// clang 会自行完成：IR → 汇编 → 目标文件 → 链接（链接 CRT 与系统库）。
/// `target` 为 `Some(三元组)` 时交叉编译（如 `x86_64-pc-windows-msvc`）。
/// `extra_libs` 为附加静态库（如 tie-interp 的 .lib）——REPL 自举用；
/// 附带补 Rust std 依赖的 Windows 系统库（链接 Rust staticlib 必需）。
pub fn link(
    input: &Path,
    output: &Path,
    target: Option<&str>,
    extra_libs: &[std::path::PathBuf],
) -> Result<(), BackendError> {
    let clang = find_clang().ok_or(BackendError::NotFound)?;
    let mut cmd = Command::new(&clang);
    cmd.arg(input).arg("-o").arg(output);
    // 交叉编译：clang 按目标三元组选择后端与系统库
    if let Some(t) = target {
        cmd.arg(format!("--target={t}"));
    }
    // 附加静态库（tie-interp .lib）与 Rust std 的 Windows 系统库依赖：
    // 静态库内的 std 代码引用了 ws2_32/userenv/ntdll/bcrypt 等系统 API，
    // clang 链接时需显式给出（Rust 的 rustc 会自动带上，clang 不会）。
    for lib in extra_libs {
        cmd.arg(lib);
    }
    if !extra_libs.is_empty() {
        cmd.arg("-lws2_32").arg("-luserenv").arg("-lntdll").arg("-lbcrypt");
        cmd.arg("-ladvapi32").arg("-lole32").arg("-lshell32");
    }
    let out = cmd.output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(BackendError::RunFailed(
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        )),
        Err(e) => Err(BackendError::RunFailed(e.to_string())),
    }
}

/// 编译 IR 为独立目标文件：`clang [--target=T] -c input.ll -o output.o`。
///
/// 库编译第一步：生成目标文件（.o/.obj），供静态库打包。
pub fn compile_object(
    input: &Path,
    output: &Path,
    target: Option<&str>,
) -> Result<(), BackendError> {
    let clang = find_clang().ok_or(BackendError::NotFound)?;
    let mut cmd = Command::new(&clang);
    cmd.arg("-c").arg(input).arg("-o").arg(output);
    if let Some(t) = target {
        cmd.arg(format!("--target={t}"));
    }
    let out = cmd.output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(BackendError::RunFailed(
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        )),
        Err(e) => Err(BackendError::RunFailed(e.to_string())),
    }
}

/// 打包静态库：`llvm-ar rcs libxxx.a xxx.o`。
///
/// 库编译第二步：把目标文件归档为静态库（.a）。
pub fn archive(object: &Path, archive: &Path) -> Result<(), BackendError> {
    let ar = find_llvm_ar().ok_or(BackendError::ArNotFound)?;
    let out = Command::new(&ar).arg("rcs").arg(archive).arg(object).output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(BackendError::ArFailed(
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        )),
        Err(e) => Err(BackendError::ArFailed(e.to_string())),
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

/// 查找 llvm-ar 可执行文件（PATH → 常见安装位置）。
fn find_llvm_ar() -> Option<std::path::PathBuf> {
    if let Some(path) = which("llvm-ar") {
        return Some(path);
    }
    for dir in ["D:\\LLVM\\bin", "C:\\Program Files\\LLVM\\bin", "C:\\LLVM\\bin"] {
        let p = Path::new(dir).join("llvm-ar.exe");
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
