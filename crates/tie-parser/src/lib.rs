//! tie 语言工作区测试桩。
//!
//! 本模块仅为打通编译链路的占位实现，
//! 后续由各模块的真实实现替换。

use tie_ast::ast_placeholder;
use tie_lexer::lexer_placeholder;

/// tie-parser 占位函数：串联词法器与 AST 的命名。
pub fn parser_placeholder() -> String {
    format!("{}+{}", lexer_placeholder(), ast_placeholder())
}
