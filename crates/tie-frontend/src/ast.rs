//! 抽象语法树（AST）节点定义。
//!
//! 解析器产出 AST，IR 生成器消费 AST。为了承载语义分析结果，
//! 部分节点携带类型信息（如 [VarDeclStmt] 的 `ty`）。

use super::lexer::{Span, TyKw};

/// 类型标注：基本类型关键字或元组类型。
///
/// 注意：元组字段（[TupleField]）内联在类型里而非用「标记 + 元数据表」——
/// 元组需要结构比较（types_match）、出现在类型标注（无表达式地址可做键）、
/// 进入函数签名并支持嵌套，这些场景都必须内联携带字段类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSpec {
    /// 显式标注的基本类型（i8..u64/f32/f64/bool/char/string/void/code）
    Named(TyKw),
    /// 元组类型 `(T1, T2)` / `(x: T1, y: T2)`（元素 ≥1，字段名可空）
    Tuple(Vec<TupleField>),
    /// 类类型（用户自定义 OOP 类型，P8）
    Class(String),
}

/// 元组的一个字段：可选字段名 + 类型（名字进类型，供 `.x` 命名访问）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleField {
    /// 字段名（`(x: i64)` 的 `x`）；`None` 表示位置字段（`(i64, string)`）
    pub name: Option<String>,
    pub ty: TypeSpec,
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
    /// 注意：`code`、`num`/`text`/`misc`、`table` 不在此映射中——
    /// 它们都是编译期概念，语义分析阶段会被展开/校验后转化为具体类型，
    /// 不应出现在 IR 生成阶段。
    ///
    /// 元组（Tuple）也不在此映射：元组映射为字面结构体类型 `{i64, ptr}`，
    /// 需要泄漏 + 去重缓存，由 IR 生成器的 llvm_ty 包装函数处理。
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
            // 编译期类型，IR 生成阶段不应出现（前端应已展开/校验）。
            TypeSpec::Named(TyKw::Code | TyKw::Num | TyKw::Text | TyKw::Misc | TyKw::Table) => {
                unreachable!(
                    "code/num/text/misc/table 是编译期类型，语义分析阶段应已展开为具体类型"
                )
            }
            TypeSpec::Tuple(_) => {
                unreachable!("元组类型映射为字面结构体，需由 IR 生成器的 llvm_ty 包装处理（含泄漏与缓存）")
            }
            TypeSpec::Class(_) => {
                unreachable!("类类型映射为字面结构体，需由 IR 生成器的 llvm_ty 包装处理（含泄漏与缓存）")
            }
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

    /// 是否为宽类型（num/text/misc，类别框，语义分析时展开为具体类型）。
    pub fn is_wide(&self) -> bool {
        matches!(self, TypeSpec::Named(k) if k.is_wide())
    }

    /// 是否为表类型（table，代表数组与高级数组）。
    pub fn is_table(&self) -> bool {
        matches!(self, TypeSpec::Named(TyKw::Table))
    }

    /// 宽类型是否接受某个具体类型（类别框的归属判断）。
    ///
    /// - num：接受全部数类型（整数/浮点）
    /// - text：接受字符串与字符
    /// - misc：接受其余类型（bool/void/code/table）
    pub fn wide_accepts(&self, actual: &TypeSpec) -> bool {
        match self {
            TypeSpec::Named(TyKw::Num) => actual.is_number(),
            TypeSpec::Named(TyKw::Text) => {
                matches!(actual, TypeSpec::Named(TyKw::Str | TyKw::Char))
            }
            TypeSpec::Named(TyKw::Misc) => !actual.is_wide(),
            _ => false,
        }
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
    /// 表达式语句（调用等）
    Expr(ExprStmt),
    /// 赋值语句 `x = expr`（对已声明变量的重新赋值）
    Assign(AssignStmt),
    /// return 语句
    Return(ReturnStmt),
    /// if/else 分支
    If(IfStmt),
    /// while 循环
    While(WhileStmt),
    /// for 循环（`for x in 0..10` / `for x in arr`）
    For(ForStmt),
    /// switch 多分支选择
    Switch(SwitchStmt),
    /// import 导入其他 tie 文件（`import "./x.tie" [as 别名]`，仅顶层）
    Import(ImportStmt),
    /// 类定义 `class Name [extends Parent] { 字段/方法 }`（P8，仅顶层）
    Class(ClassDefStmt),
    /// 字段赋值 `obj.field = expr`（P8，对已存在实例字段的写入）
    FieldAssign(FieldAssignStmt),
}

/// import 语句：把其他 tie 文件的顶层函数并入当前文件。
#[derive(Debug, Clone)]
pub struct ImportStmt {
    /// 被导入文件的路径（相对当前文件所在目录，如 `"./lib.tie"`）
    pub path: String,
    /// 可选别名（`as 别名`）；当前阶段仅解析保留，后续版本用于命名空间限定
    pub alias: Option<String>,
    pub span: Span,
}

/// 赋值语句：对已声明的变量重新赋值（const 不可变）。
#[derive(Debug, Clone)]
pub struct AssignStmt {
    /// 被赋值的目标变量名
    pub target: String,
    /// 新值表达式
    pub value: Expr,
    pub span: Span,
}

/// 变量声明语句。
#[derive(Debug, Clone)]
pub struct VarDeclStmt {
    pub name: String,
    /// 显式类型标注（`var x: i64`），`None` 表示自动推导
    pub ty: Option<TypeSpec>,
    /// 初始值表达式
    pub init: Expr,
    /// 是否不可变（`const` 声明）
    pub is_const: bool,
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

/// 类定义语句（P8）：`class Name [extends Parent] { 字段… 方法… }`。
///
/// 仅允许出现在文件顶层（与 import 相同）；字段与方法并列在类体内。
#[derive(Debug, Clone)]
pub struct ClassDefStmt {
    /// 类名（全局唯一，不能与函数名/其他类名冲突）
    pub name: String,
    /// 父类名（`extends Parent`）；`None` 表示无继承
    pub parent: Option<String>,
    /// 自身字段（不含继承的；拍平由语义层完成）
    pub fields: Vec<ClassField>,
    /// 自身方法（不含继承的；遮蔽解析由语义层完成）
    pub methods: Vec<MethodDefStmt>,
    pub span: Span,
}

/// 类的一个字段声明：`var name[: Ty] [= 默认值]`（P8）。
///
/// 默认值限编译期字面量（构造缺省时兜底）；字段恒可变（const 字段留后续版本）。
#[derive(Debug, Clone)]
pub struct ClassField {
    pub name: String,
    /// 显式类型标注（`var count: i64`）；`None` 由默认值推导
    pub ty: Option<TypeSpec>,
    /// 默认值（`var count = 0`）；`None` 表示构造时必须传参
    pub init: Option<Expr>,
    pub span: Span,
}

/// 类的一个方法定义（P8）：`[static] method name(params) -> Ty { body }`。
///
/// 与 [FnDefStmt] 同构 + `is_static`；不复用 FnDefStmt 以免污染顶层函数构造点。
/// 实例方法体内 `this` 绑定当前实例（语义层处理）。
#[derive(Debug, Clone)]
pub struct MethodDefStmt {
    pub name: String,
    /// 静态方法：不绑定 this，通过 `类名.方法名()` 调用
    pub is_static: bool,
    pub params: Vec<Param>,
    /// 返回类型（省略时为 void）
    pub ret_ty: TypeSpec,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// 字段赋值语句（P8）：`obj.field = expr`。
///
/// 与 [AssignStmt] 分开：AssignStmt 的 `target: String` 快速路径是既有工作代码，
/// 新增变体零触碰现有路径。base 限变量或 this（语义层校验）。
#[derive(Debug, Clone)]
pub struct FieldAssignStmt {
    /// 实例表达式（限 Var/this；寄存器中的类值不可寻址，P8 报错）
    pub base: Box<Expr>,
    /// 字段名
    pub field: String,
    /// 新值表达式
    pub value: Expr,
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

/// switch 多分支选择语句。
#[derive(Debug, Clone)]
pub struct SwitchStmt {
    /// 被匹配的表达式（subject）
    pub subject: Expr,
    /// case 分支列表（顺序即源码顺序）
    pub cases: Vec<SwitchCase>,
    /// default 分支体（可选，无则空）
    pub default_body: Vec<Stmt>,
    pub span: Span,
}

/// switch 的一个 case 分支：`case 值: 语句…`。
#[derive(Debug, Clone)]
pub struct SwitchCase {
    /// case 匹配值（编译期字面量：整数/字符/布尔/字符串）
    pub value: Expr,
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
    /// 字符（UTF-32 单字符）
    CharLit(char),
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
    /// 表字面量 `[col, col; row, row]`（高级数组/表，逗号分列、分号分行）
    TableLit { cells: Vec<TableCell>, span: Span },
    /// 下标访问 `arr[0]`（表/数组元素读取）
    Index { base: Box<Expr>, index: Box<Expr>, span: Span },
    /// 元组字面量 `(1, "a")` / `(x: 1, y: 2)`（C# 风格；元素 ≥1）
    TupleLit { fields: Vec<(Option<String>, Expr)>, span: Span },
    /// 字段访问（读）`base.field`（P8 统一变体）：
    ///
    /// - base 是元组 → 元组字段（`.x` 命名 / `.Item1` / `.0`，语义层按 tuple_field_index 解析）；
    /// - base 是类实例 → 类字段（`.x` 命名，field_index 解析）。
    ///
    /// 同一变体管两种，语义层按 base 的推导类型分发。
    FieldAccess { base: Box<Expr>, field: String, span: Span },
    /// 方法调用 `obj.m(args)`（实例）/ `MyClass.m(args)`（静态）（P8）
    ///
    /// receiver 是变量/this → 实例方法（receiver 地址作隐藏 this 参数）；
    /// receiver 是类名 → 静态方法（无 this）。同一变体管两种，语义层区分。
    MethodCall { receiver: Box<Expr>, method: String, args: Vec<Expr>, span: Span },
}

/// 表单元格：`value` 或 `id:value`（id 可选，可为数字下标或带引号字符串键）。
#[derive(Debug, Clone)]
pub struct TableCell {
    /// 显式 id；`None` 表示普通位置元素（按隐式编号）
    pub id: Option<TableId>,
    /// 元素值（任意类型表达式）
    pub value: Expr,
    /// 所属行号（从 0 起），由解析器按 `;` 切分记录
    pub row: usize,
}

/// 表元素 id：数字（下标）或字符串（命名键）。
#[derive(Debug, Clone)]
pub enum TableId {
    /// 数字下标（不加引号），如 `[0:1, 1:2]`
    Num(i64),
    /// 字符串键（必须加双引号），如 `["a":1]`
    Str(String),
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
