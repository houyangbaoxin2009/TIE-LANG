//! tie 语言工作区测试桩。
//!
//! 本模块仅为打通编译链路的占位实现，
//! 后续由各模块的真实实现替换。

use tie_parser::parser_placeholder;

/// tie-interp 占位函数：返回解释器版本标识。
pub fn interp_placeholder() -> String {
    format!("interp<{}>", parser_placeholder())
}
