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
    /// struct 类型（用户自定义数据结构，纯数据：只含字段）
    Struct(String),
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
            TypeSpec::Struct(_) => {
                unreachable!("struct 类型映射为字面结构体，需由 IR 生成器的 llvm_ty 包装处理（含泄漏与缓存）")
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
    /// 命名空间声明 `namespace tcmsg { ... }`（C# 风格块式；仅顶层，可嵌套/点分）
    Namespace(NamespaceStmt),
    /// using 引入语句（`using fmt2;`，仅顶层）：把已导入命名空间的公有函数
    /// 引入当前文件作用域，之后可裸调用（如 `public_api()`）
    Using(UsingStmt),
    /// struct 定义 `struct Name [extends Parent] { 字段 }`（纯数据，仅顶层）
    Struct(StructDefStmt),
    /// 字段赋值 `obj.field = expr`（P8，对已存在实例字段的写入）
    FieldAssign(FieldAssignStmt),
}

/// import 语句：把其他 tie 文件的顶层函数并入当前文件。
#[derive(Debug, Clone)]
pub struct ImportStmt {
    /// 被导入文件的路径（相对当前文件所在目录，如 `"./lib.tie"`）
    pub path: String,
    /// 可选别名（`as 别名`）；有别名时**唯一入口**——原命名空间前缀在导入方
    /// 不可用，必须用别名访问（M2.1.7 单文件命名空间）
    pub alias: Option<String>,
    /// 被导入文件声明的全部命名空间路径（由 imports.rs 展开时填充；
    /// parser 阶段为空）。语义层据此把「别名/前缀 → 命名空间」映射到全名。
    pub ns_paths: Vec<Vec<String>>,
    pub span: Span,
}

/// using 引入语句：`using fmt2;`（仅顶层，M2.1.7 单文件命名空间）。
///
/// 目标必须是**已通过 import 引入的命名空间前缀或别名**；引入后该命名空间的
/// 公有函数可裸名调用（`public_api()`，不再写 `fmt2.public_api()`）。
/// 同名裸名多候选（多个 using 都含该函数）→ 语义层报歧义错误。
#[derive(Debug, Clone)]
pub struct UsingStmt {
    /// 目标命名空间路径（`using fmt.error` 存为 ["fmt", "error"]；别名单段）
    pub path: Vec<String>,
    pub span: Span,
}

/// 命名空间声明（C# 风格块式）：`namespace tcmsg { ... }`。
///
/// 仅允许出现在文件顶层；路径为点分/嵌套组合出的完整命名空间名
/// （如 `namespace tcmsg.error { }` 或嵌套 `namespace tcmsg { namespace error { } }`）。
/// 体内可含函数定义/类定义/嵌套命名空间；作用域与符号注册由语义层处理。
#[derive(Debug, Clone)]
pub struct NamespaceStmt {
    /// 命名空间路径段（如 `tcmsg.error` 存为 ["tcmsg", "error"]）
    pub path: Vec<String>,
    /// 命名空间体内的语句（函数/类/嵌套命名空间等）
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// 赋值语句：对已声明的变量重新赋值（const 不可变）。
#[derive(Debug, Clone)]
pub struct AssignStmt {
    /// 被赋值的目标变量名
    pub target: String,
    /// 赋值运算符：`None` 为普通赋值 `=`；`Some(op)` 为复合赋值 `+=`/`-=` 等
    pub op: Option<BinaryOp>,
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
    /// 是否公有（`pub func`，M2.1.7）：命名空间内函数默认私有（仅同命名空间
    /// 可见）；显式 pub 后跨命名空间/跨文件（import/using）可调。顶层函数恒公有。
    pub is_pub: bool,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// 函数参数。
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeSpec,
    /// 默认值表达式（可选参数）：调用时省略该实参则用默认值补齐。
    /// 限字面量（含空表 `[]`），与类字段默认值规则一致（避免作用域依赖）。
    pub default: Option<Expr>,
    pub span: Span,
}

/// struct 定义语句：`struct Name [extends Parent] { 字段 }`。
///
/// **纯数据**：只含字段声明（`var name[: Ty] [= 默认值]`），不含方法。
/// 逻辑（操作该数据的行为）通过**绑定 struct 名的命名空间函数**定义：
/// `namespace Point { pub func dist(p: Point) -> i64 { ... } }`，调用时
/// `p.dist()` 由语义层转发为 `Point::dist(p)`（方法函数必须 `pub` 才可转发）。
/// 仅允许出现在文件顶层（与 import 相同）；`extends` 支持字段继承（拍平）。
#[derive(Debug, Clone)]
pub struct StructDefStmt {
    /// struct 名（全局唯一，不能与函数名/其他 struct 名冲突）
    pub name: String,
    /// 父 struct 名（`extends Parent`）；`None` 表示无继承
    pub parent: Option<String>,
    /// 自身字段（不含继承的；拍平由语义层完成）
    pub fields: Vec<ClassField>,
    pub span: Span,
}

/// struct 的一个字段声明：`var name[: Ty] [= 默认值]`。
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
    /// 赋值运算符：`None` 为普通赋值 `=`；`Some(op)` 为复合赋值 `+=`/`-=` 等
    pub op: Option<BinaryOp>,
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

/// switch 的一个 case 分支：`case 值[, 值]... [when 条件]: 语句…`。
///
/// 模式匹配增强（规划 switch-pattern-matching）：
/// - 多值：`case 1, 2:`（任一相等即命中，逗号分隔）；
/// - 区间：`case 3..7:`（`Range` 表达式，含 3 不含 7）；
/// - 守卫：`case 8 when flag:`（值命中 且 守卫为真才进入）；
/// - 类型匹配：`case string:`（`TypeLit`，匹配 subject 的动态类型）。
#[derive(Debug, Clone)]
pub struct SwitchCase {
    /// case 匹配模式列表（多值逗号分隔；每个可为字面量/区间 Range/类型 TypeLit）
    pub patterns: Vec<Expr>,
    /// when 守卫条件（可选；值为真才命中，否则落入下一个 case）
    pub when: Option<Expr>,
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
    /// 三目运算 `cond ? then : else`（M4）
    Ternary { cond: Box<Expr>, then_expr: Box<Expr>, else_expr: Box<Expr>, span: Span },
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
    /// - base 是 struct 实例 → struct 字段（`.x` 命名，field_index 解析）。
    ///
    /// 同一变体管两种，语义层按 base 的推导类型分发。
    FieldAccess { base: Box<Expr>, field: String, span: Span },
    /// 命名空间路径 `a::b::c`（C#/Rust 风格，`::` 分隔；如 `tcmsg::error`）。
    ///
    /// 作为独立表达式仅表示"命名空间路径"；真正的函数调用是
    /// `MethodCall { receiver: Path(...), method: "no_file", .. }`（`tcmsg::error.no_file()`），
    /// 语义层按 receiver 是 Path 解析为命名空间函数调用。
    Path { segments: Vec<String>, span: Span },
    /// 类型字面量（switch 类型匹配 pattern：`case string:` / `case i64:`）。
    ///
    /// 仅在 switch 的 case pattern 位置出现；普通表达式上下文不存在。
    /// 语义层校验：subject 为动态类型容器（宽类型/表/元组）才允许，静态类型上报错。
    TypeLit { ty: TypeSpec, span: Span },
    /// 方法调用 `obj.m(args)`（实例）/ `MyStruct.m(args)`（静态）/ 命名空间调用（M2.1.8）。
    ///
    /// receiver 是变量（类型 T 为 struct）→ **方法转发**：语义层查 `T::m`（命名空间函数，
    /// 沿继承链），实参 = [receiver] + args，等价 `T::m(obj, args)`；
    /// receiver 是 struct 名 → 静态调用 `T::m(args)`（无首参插入）；
    /// receiver 是命名空间路径（Expr::Path / 未绑定链）→ 命名空间函数调用（全名 = 路径段 + 方法名）。
    /// 同一变体管三种，语义层区分。
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
    /// 取负 `-x`
    Neg,
    /// 逻辑非 `!x`
    Not,
    /// 前缀自增 `++x`（M4）：先增后取新值
    PreInc,
    /// 前缀自减 `--x`（M4）：先减后取新值
    PreDec,
    /// 后缀自增 `x++`（M4）：先取旧值后增
    PostInc,
    /// 后缀自减 `x--`（M4）：先取旧值后减
    PostDec,
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
    /// 按位与 `&`（M4，仅整数）
    BitAnd,
    /// 按位或 `|`（M4，仅整数）
    BitOr,
    /// 按位异或 `^`（M4，仅整数）
    BitXor,
    /// 左移 `<<`（M4，仅整数）
    Shl,
    /// 右移 `>>`（M4，仅整数；有符号算术右移、无符号逻辑右移）
    Shr,
}
