//! tie-prep：tie 语言预处理工具（四段式第一段）。
//!
//! 四段式架构：`预处理 [前端 中间优化 后端]`
//!
//! 预处理独立于三段式，在源码文本层面工作，职责：
//! 1. **清理代码**：去除 BOM、规范化换行，产出干净的正文源码；
//! 2. **识别文件类型**：解析文件头部的 `type tie` / `type tie<X>` 声明行
//!    （新文件类型声明系统），判定本文件的角色
//!    （type / script / data / ui / class / logic / port / db）；
//!    无声明时默认 role = logic；也可按文件名 `<名>.<角色>.tie` 预判
//!    （[role_from_filename]，头部声明优先于文件名声明）；
//! 3. **角色判定结果**：由驱动/调用方按角色自动转交对应的工具链
//!    （编译器 / 界面工具 / 数据库工具 / 数据解析）。
//!
//! 声明区与内容分离原则：声明行只允许出现在文件最前面的连续行（允许其间空行）；
//! 内容区出现的 `type tie` 与旧式 `// tie:` 注释都是普通内容，不受预处理影响。
//!
//! 本 crate 同时提供：
//! - 库接口 [preprocess]，供 tie-llvm 等编译器集成；
//! - 独立 CLI（tie-prep.exe），可单独作为管道工具使用。

pub mod preprocess;

pub use preprocess::{
    clean_source, preprocess, role_from_filename, run_module, FileRole, PreprocessResult,
};

/// 在 Windows 上把控制台输入/输出代码页切换为 UTF-8（65001）。
///
/// tie 工具链所有文本均为 UTF-8 编码；而 Windows 控制台默认代码页常为
/// 936（GBK），直接输出 UTF-8 字节会造成中文乱码。各 CLI 入口（main）
/// 启动时调用本函数，保证中文信息正常显示。非 Windows 平台为无操作。
pub fn init_console_utf8() {
    // Windows：调用 kernel32 的 SetConsoleOutputCP / SetConsoleCP
    // 把输出/输入代码页都设为 UTF-8（CP_UTF8 = 65001）。
    #[cfg(windows)]
    unsafe {
        // SAFETY: 调用 Windows API，参数为固定常量 65001，无指针、无借用，
        // 返回值忽略（设置失败仅影响显示编码，不影响程序逻辑）。
        unsafe extern "system" {
            /// 设置控制台输出代码页。
            fn SetConsoleOutputCP(w_code_page_id: u32) -> i32;
            /// 设置控制台输入代码页。
            fn SetConsoleCP(w_code_page_id: u32) -> i32;
        }
        // CP_UTF8 = 65001，UTF-8 代码页
        const CP_UTF8: u32 = 65001;
        SetConsoleOutputCP(CP_UTF8);
        SetConsoleCP(CP_UTF8);
    }
    // 非 Windows 平台：控制台原生 UTF-8，无需处理。
    #[cfg(not(windows))]
    {}
}
