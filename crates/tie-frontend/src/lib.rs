//! tie 编译器前端：词法分析 → 语法分析 → 语义分析。
//!
//! 对应《编译原理》三阶段结构的前端部分，全部由 tie-frontend 自研实现：
//! - [lexer]：词法分析。把源码字符串切分成 token 流，并在词法层完成
//!   ASI（Automatic Semicolon Insertion，自动分号补全）。
//! - [ast]：抽象语法树节点定义，供解析器与 IR 生成共享。
//! - [parser]：语法分析。递归下降解析 token 流生成 AST，
//!   同时解析文件头部（`// tie:` 指令）并据此分派解析策略。
//! - [semantic]：语义分析。符号表构建与类型检查。

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod semantic;
