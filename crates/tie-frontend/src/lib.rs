//! tie 编译器前端：词法分析 → 语法分析 → 语义分析。
//!
//! 对应《编译原理》三阶段结构的前端部分，全部由 tie-frontend 自研实现：
//! - [lexer]：词法分析。把源码字符串切分成 token 流，并在词法层完成
//!   ASI（Automatic Semicolon Insertion，自动分号补全）。
//! - [ast]：抽象语法树节点定义，供解析器与 IR 生成共享。
//! - [parser]：语法分析。递归下降解析 token 流生成 AST，
//!   同时解析文件头部（`// tie:` 指令）并据此分派解析策略。
//! - [semantic]：语义分析。符号表构建与类型检查。
//! - [imports]：import 展开。递归加载被导入文件并内联其顶层语句，
//!   供编译器驱动（tie-llvm）与语言服务器（tie-lsp）共享。

pub mod ast;
pub mod imports;
pub mod lexer;
pub mod parser;
pub mod semantic;

/// 在 Windows 上把控制台输入/输出代码页切换为 UTF-8（65001）。
///
/// tie 工具链所有文本均为 UTF-8 编码；而 Windows 控制台默认代码页常为
/// 936（GBK），直接输出 UTF-8 字节会造成中文乱码。各 CLI 入口（main）
/// 启动时调用本函数，保证中文信息正常显示。非 Windows 平台为无操作。
///
/// 与 [tie_prep::init_console_utf8] 为同一份实现（各自内联，避免跨 crate
/// 引入无谓依赖）；两者都在各自的 CLI 入口调用。
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
