//! tie 语言解释器（规划中）。
//!
//! 设计职责：直接解释执行 AST（REPL 交互模式与脚本执行），
//! 与编译路径（tie-llvm）共享 tie-frontend 前端产物。
//!
//! 当前为占位实现，仅验证前端依赖链完整。

use tie_frontend::parser::parse_program;
use tie_frontend::lexer::tokenize;

/// tie-interp 占位函数：验证前端依赖链完整。
///
/// 对空源码做一次「词法 → 语法」全流程，返回解释器版本标识。
pub fn interp_placeholder() -> String {
    let tokens = tokenize("").unwrap_or_default();
    let program = parse_program(&tokens).ok();
    format!("interp<frontend-ok, stmts={}>", program.map_or(0, |p| p.stmts.len()))
}
