//! 抽象语法树（AST）节点定义。
//!
//! 解析器产出 AST，IR 生成器消费 AST。为了承载语义分析结果，
//! 部分节点携带类型信息（如 [VarDeclStmt] 的 `ty`）。

use super::lexer::{Span, TyKw};

/// 类型标注：基本类型关键字（复合类型语法后续版本扩展）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeSpec {
    /// 显式标注的基本类型（i8..u64/f32/f64/bool/char/string/void/code）
    Named(TyKw),
}

impl TypeSpec {
    /// LLVM IR 中的类型名。
    ///
    /// 映射规则：
    /// - iN → iN（有符号整数同宽位）
    /// - uN → iN（LLVM 无独立无符号类型，无符号性由指令语义体现）
    /// - f32 → float、f64 → double
    /// - bool → i1、char → i32（Rust 风格 char 为 Unicode 标量，4 字节）
    /// - string → ptr
    ///
    /// 注意：`code` 不在此映射中——它是**编译期类型**，代码片段会在语义
    /// 分析阶段被解析翻译为 AST 子程序，IR 生成阶段已不存在 code 值。
    pub fn llvm_ty(&self) -> &'static str {
        match self {
            TypeSpec::Named(TyKw::I8) | TypeSpec::Named(TyKw::U8) => "i8",
            TypeSpec::Named(TyKw::I16) | TypeSpec::Named(TyKw::U16) => "i16",
            TypeSpec::Named(TyKw::I32) | TypeSpec::Named(TyKw::U32) => "i32",
            TypeSpec::Named(TyKw::I64) | TypeSpec::Named(TyKw::U64) => "i64",
            TypeSpec::Named(TyKw::F32) => "float",
            TypeSpec::Named(TyKw::F64) => "double",
            TypeSpec::Named(TyKw::Bool) => "i1",
            TypeSpec::Named(TyKw::Char) => "i32",
            TypeSpec::Named(TyKw::Str) => "ptr",
            TypeSpec::Named(TyKw::Void) => "void",
            // code 为编译期类型，不产生 LLVM 实体；若 IR 生成阶段仍遇到
            // 说明前端未正确展开 code 片段，属编译器 bug。
            TypeSpec::Named(TyKw::Code) => unreachable!(
                "code 是编译期类型，应在语义分析阶段展开为 AST，不应出现在 IR 生成"
            ),
        }
    }

    /// 是否为 void。
    pub fn is_void(&self) -> bool {
        matches!(self, TypeSpec::Named(TyKw::Void))
    }

    /// 是否为整数类型（含无符号）。
    pub fn is_int(&self) -> bool {
        matches!(self, TypeSpec::Named(k) if k.is_int())
    }

    /// 是否为浮点类型。
    pub fn is_float(&self) -> bool {
        matches!(self, TypeSpec::Named(k) if k.is_float())
    }

    /// 是否为数字类型（整数或浮点）。
    pub fn is_number(&self) -> bool {
        self.is_int() || self.is_float()
    }
}

/// 程序 = 语句列表（头部指令已由 tie-prep 预处理阶段提取）。
#[derive(Debug, Clone)]
pub struct Program {
    /// 顶层语句（函数定义等）
    pub stmts: Vec<Stmt>,
}

/// 语句。
#[derive(Debug, Clone)]
pub enum Stmt {
    /// 变量声明 `let x = expr` / `let x: int = expr`
    VarDecl(VarDeclStmt),
    /// 函数定义 `fn name(params) -> Ty { body }`
    FnDef(FnDefStmt),
    /// 表达式语句（调用、赋值等）
    Expr(ExprStmt),
    /// return 语句
    Return(ReturnStmt),
    /// if/else 分支
    If(IfStmt),
    /// while 循环
    While(WhileStmt),
    /// for 循环（`for x in 0..10` / `for x in arr`）
    For(ForStmt),
}

/// 变量声明语句。
#[derive(Debug, Clone)]
pub struct VarDeclStmt {
    pub name: String,
    /// 显式类型标注（`let x: int`），`None` 表示自动推导
    pub ty: Option<TypeSpec>,
    /// 初始值表达式
    pub init: Expr,
    pub span: Span,
}

/// 函数定义语句。
#[derive(Debug, Clone)]
pub struct FnDefStmt {
    pub name: String,
    pub params: Vec<Param>,
    /// 返回类型（省略时为 void）
    pub ret_ty: TypeSpec,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// 函数参数。
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeSpec,
    pub span: Span,
}

/// 表达式语句。
#[derive(Debug, Clone)]
pub struct ExprStmt {
    pub expr: Expr,
    pub span: Span,
}

/// return 语句。
#[derive(Debug, Clone)]
pub struct ReturnStmt {
    pub expr: Option<Expr>,
    pub span: Span,
}

/// if/else 语句。
#[derive(Debug, Clone)]
pub struct IfStmt {
    pub cond: Expr,
    pub then_branch: Vec<Stmt>,
    /// else 分支：else if 会继续嵌套为 IfStmt
    pub else_branch: Vec<Stmt>,
    pub span: Span,
}

/// while 语句。
#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub cond: Expr,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// for 语句（范围或集合迭代）。
#[derive(Debug, Clone)]
pub struct ForStmt {
    pub var: String,
    /// 迭代对象（`0..10` 会解析为 RangeExpr）
    pub iter: Expr,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// 表达式。
#[derive(Debug, Clone)]
pub enum Expr {
    /// 整数
    IntLit(i64),
    /// 浮点数
    FloatLit(f64),
    /// 字符串
    StrLit(String),
    /// 布尔
    BoolLit(bool),
    /// 变量引用
    Var(String),
    /// 函数调用 `name(args)`
    Call { name: String, args: Vec<Expr>, span: Span },
    /// 一元运算 `!x` / `-x`
    Unary { op: UnaryOp, operand: Box<Expr>, span: Span },
    /// 二元运算 `a + b`
    Binary { op: BinaryOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
    /// 范围 `0..10`
    Range { start: Box<Expr>, end: Box<Expr>, span: Span },
}

/// 一元运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// 二元运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}
