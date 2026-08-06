//! tie-llvm：tie 语言编译工具链（四段式的编译分支）。
//!
//! 提供库接口（供总入口 tie 调用）与 CLI（可单独使用）：
//! - 库：[driver]::compile 完成「前端 → 中端 → 后端」编译流水线
//! - CLI：`tie-llvm <input.tie> [选项]` 独立编译工具
//!
//! 流水线（四段式中段）：tie-prep 预处理 → 自研前端 → LLVM IR → opt 优化 → clang/lld 链接。

pub mod backend;
pub mod driver;
pub mod frontend;
pub mod ir;
pub mod optimizer;
