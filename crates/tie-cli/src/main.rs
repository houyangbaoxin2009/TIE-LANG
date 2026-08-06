//! tie 语言命令行入口。
//!
//! 当前仅为编译链路占位，后续实现：
//! - 无参数：进入 REPL 交互模式
//! - 传入文件路径：执行 .tie 脚本文件

use tie_interp::interp_placeholder;

fn main() {
    // 占位输出，验证 cli 依赖链完整
    println!("tie CLI: {}", interp_placeholder());
}
