//! 后端：调用 LLVM 工具链（clang/lld）把优化后的 IR 编译链接为可执行文件或库。
//!
//! 对应《编译原理》的目标代码生成与链接阶段，具体工作交给 LLVM：
//! - `clang optimized.ll -o output.exe`（内部自动完成汇编与链接，生成可执行文件）
//! - `clang -c optimized.ll -o output.o` + `llvm-ar rcs libxxx.a output.o`（生成静态库）
//! - 交叉编译：给 clang 传 `--target=<三元组>`（如 `--target=x86_64-pc-windows-msvc`）
//!
//! LLVM 工具查找顺序（`bundled_llvm_bin`，随发行版 vendored LLVM 优先）：
//! 1. 环境变量 `TIE_LLVM_HOME` 指定安装根目录的 `bin\` 子目录；
//! 2. 当前可执行文件同目录的 `llvm\bin`（release bin/llvm/bin/*.exe）；
//! 3. 系统 PATH；
//! 4. 固定安装目录兜底（D:\LLVM\bin 等）。
//! 前两者只做「目录存在」判定，具体工具缺失时自动回退到后两者，不会硬失败。
//!
//! `-fuse-ld=lld` 仅在 vendored LLVM 场景生效（clang 位于上述前两者目录内且
//! 同目录有 lld-link.exe）；本机开发（clang 来自 PATH/固定目录，VS link.exe
//! 可用）保持默认 link.exe 链接——lld 解析 Rust staticlib CRT 符号有缺陷。

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
    // -fuse-ld=lld 仅对「vendored LLVM」生效：clang 来自 TIE_LLVM_HOME 或
    // 可执行文件同目录 llvm\bin（发布包场景，用户无 MSVC → 用随包 lld-link）。
    // 本机开发（clang 来自 PATH/固定目录，VS link.exe 可用）不加——lld 解析
    // Rust staticlib（tie_interp.lib）的 CRT 符号有缺陷（printf undefined），
    // 必须保留默认 link.exe 行为（repl 自举等依赖）。
    if let Some(bin) = bundled_llvm_bin() {
        if bin.join("lld-link.exe").exists() && clang.starts_with(&bin) {
            cmd.arg("-fuse-ld=lld");
        }
    }
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

/// 查找随发行版捆绑/指定的 LLVM 工具所在 bin 子目录。
///
/// vendored LLVM 分发模型下，优先使用随包工具而非系统 PATH：
/// 1. 环境变量 `TIE_LLVM_HOME` = LLVM 安装根目录（如 `D:\LLVM`，其 `bin\` 子目录含工具）；
/// 2. 当前可执行文件同目录的 `llvm\bin`（release 的 bin/llvm/bin/*.exe）。
///
/// 返回的目录仅代表「存在」，具体工具（clang.exe 等）由调用方进一步判定；
/// 两者都不可用时返回 `None`，调用方回退到 PATH / 固定安装目录。
pub(crate) fn bundled_llvm_bin() -> Option<std::path::PathBuf> {
    // 1. 环境变量 TIE_LLVM_HOME：其 bin\ 子目录存在即采用
    if let Some(home) = std::env::var_os("TIE_LLVM_HOME") {
        let bin = Path::new(&home).join("bin");
        if bin.is_dir() {
            return Some(bin);
        }
    }
    // 2. 当前可执行文件同目录的 llvm\bin（release bin/llvm/bin/）
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("llvm").join("bin")))
        .filter(|bin| bin.is_dir())
}

/// 查找 clang 可执行文件（捆绑 LLVM → PATH → 常见安装位置）。
fn find_clang() -> Option<std::path::PathBuf> {
    // 1. 捆绑/指定的 LLVM（TIE_LLVM_HOME 或当前目录 llvm\bin）
    if let Some(bin) = bundled_llvm_bin() {
        let p = bin.join("clang.exe");
        if p.exists() {
            return Some(p);
        }
    }
    // 2. PATH
    if let Some(path) = which("clang") {
        return Some(path);
    }
    // 3. 常见安装位置兜底
    for dir in ["D:\\LLVM\\bin", "C:\\Program Files\\LLVM\\bin", "C:\\LLVM\\bin"] {
        let p = Path::new(dir).join("clang.exe");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 查找 llvm-ar 可执行文件（捆绑 LLVM → PATH → 常见安装位置）。
fn find_llvm_ar() -> Option<std::path::PathBuf> {
    // 1. 捆绑/指定的 LLVM（TIE_LLVM_HOME 或当前目录 llvm\bin）
    if let Some(bin) = bundled_llvm_bin() {
        let p = bin.join("llvm-ar.exe");
        if p.exists() {
            return Some(p);
        }
    }
    // 2. PATH
    if let Some(path) = which("llvm-ar") {
        return Some(path);
    }
    // 3. 常见安装位置兜底
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
