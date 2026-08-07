# tie 语言 AI 教学指南

> 本文件专为 **AI 助手（LLM）** 编写：目标是"粘贴即用"的完整语言说明书，
> 覆盖语法、语义、已实现/未实现边界与编译器架构。你（AI）应严格按本文件 +
> [language.md](language.md) 工作，不得使用本文件未列出的特性（很可能未实现）。
>
> 更新于 2026-08-07（P8 class/OOP 完成后）。

## 0. 一句话定位

tie 是一门**静态类型、四段式编译**的通用语言（预处理 → 前端 → 中端 → 后端），
后端为 LLVM。类/元组/表是**值类型**（非引用、无 GC、无虚表）。

```bash
cargo build --workspace          # 构建
cargo run -p tie -- a.tie        # 编译并运行 a.tie
tie a.tie -o out -O2             # 指定输出与优化级别
tie                             # 无参数 → REPL
```

## 1. 文件头（Header / 角色声明）

文件前几行以 `// tie:` 开头的指令决定文件角色（每行一个，连续排列）：

```c
// tie:logic                     // 逻辑代码（默认角色，可省略）：编译为可执行文件
// tie:data                      // 数据交换：纯数据，类似 JSON，可被 import
// tie:library                   // 库文件：不生成 main
// tie:target=win-x64            // 编译目标（选项 key=value 跟在角色后）
// tie:opt=3                     // 优化级别
```

| 角色 | 说明 |
|---|---|
| `logic`（默认） | 可执行文件；必须含 `func main()` |
| `data` | 纯数据声明，供其他文件 `import` |
| `library` | 编译为库，不生成 main |
| `ui` / `db` | 规划中（M4），**未实现** |

## 2. 已实现特性清单（截至 P8）

以下特性**可以使用**，示例均已验证：

### 2.1 变量与类型

```c
var x = 5                        // 可变，自动推导 i64（整数字面量默认 i64）
var f = 3.14                     // f64（浮点字面量默认 f64）
var n: i32 = 1                   // 显式标注
const s: string = "hi"           // 不可变（const），赋值语句会拒绝重赋值
```

基本类型：`i8` `i16` `i32` `i64` `u8` `u16` `u32` `u64` `f32` `f64` `bool` `char` `string` `void`。
宽类型（编译期类别框，声明后变量以推导的具体类型参与运算）：
`num`（数）/ `text`（string+char）/ `misc`（其余）。

### 2.2 表达式与运算符

```c
var a = 1 + 2 * 3                 // 算术：+ - * / %（% 仅整数）
var b = (a > 3) && (a < 10)      // 比较：== != < > <= >=；逻辑：&& || !（两侧必须 bool）
var c = -a                        // 一元负号
```

### 2.3 控制流

```c
if x > 3 { } else if x > 1 { } else { }    // 条件分支
while i < 10 { i = i + 1 }                 // 循环
for i in 0..10 { }                         // 范围循环（含 0 不含 10）
for item in arr { }                        // 集合循环（遍历表）
switch n {                                 // 多分支：case 值: 后接语句（无 break，无 fallthrough）
    case 1:
        println("one")
    case 2:
        println("two")
    default:
        println("other")
}
return expr                                // 返回值
```

switch 的 `case` 支持整数、字符（`'a'`）、布尔、负数；`default` 可省略。

### 2.4 函数

```c
func add(a: i64, b: i64) -> i64 { return a + b }
func main() { println(add(1, 2)) }         // 入口
```

- 返回类型 `-> Ty` 可省略（默认 `void`）。
- **多值返回**用元组（见 2.6）。
- **未实现**：一等函数、默认参数、重载、函数体内嵌套函数。

### 2.5 表 table（数组）

```c
var arr: table = [1, 2, 3]         // 单行 3 列（无 id）
var arr2: table = [0:1, 1:2, 2:3] // 显式数字 id（= 下标）
var e = arr[1]                     // 下标访问（已实现）
for item in arr { }                // 遍历（已实现）
```

> **重要**：当前（M2）表运行时仅支持**单行纯位置表 + 数字下标**。
> 字符串 id 表（`["a":1]`）、二维表（`[1,2;3,4]`）语法可解析，但语义阶段
> 报"留待 M3"，**不要使用**。

### 2.6 元组（多值返回 / 异构值类型）

```c
func divmod(a: i64, b: i64) -> (q: i64, r: i64) {
    return (a / b, a % b)
}

var t = (10, 20)                  // 推导为 (i64, i64)
var p = (x: 3, y: 4)              // 命名元组 (x: i64, y: i64)
println(t.Item1)                  // 位置访问：Item1 从 1 起编号
println(p.x)                      // 命名访问
var (q, r) = divmod(17, 5)        // 解构：q=3, r=2（编译期 desugar）
```

- 空元组 `()` 不支持；元素 ≥ 1。
- 字段访问三种形式等价：`t.x` / `t.Item1` / `t.0`。
- **未实现**：`println` 打印元组、元组比较运算。

### 2.7 import（多文件）

```c
import "./lib_math.tie" as math   // 导入其他 tie 文件（相对路径字符串）
```

- **已实现**（M2）：导入文件中的函数递归加载、内联可用。
- **未实现**：`data` 文件导入为只读数据表、按角色分派可见符号集。

### 2.8 class / OOP（P8 新增）

```c
class Point {
    var x: i64 = 0                // 字段：var name[: Ty] [= 默认值]
    var y: i64 = 0
    method dist() -> i64 {        // 实例方法：体内 this 绑定当前对象
        return this.x * this.x + this.y * this.y
    }
    static method origin() -> Point {   // 静态方法：无 this
        return Point(0, 0)
    }
}

func main() {
    var p = Point(3, 4)           // 构造表达式（按字段声明顺序传参）
    var q = Point()               // 全部用默认值
    var r = Point(1)              // 部分实参：缺省字段用默认值
    p.x = 5                       // 字段直写
    println(p.dist())             // 实例方法调用（25）
    var o = Point.origin()        // 静态方法调用：先存变量（寄存器类值不可直接 .x）
    println(o.x)                  // 0
}
```

> **P8 关键限制**：`Point.origin().x` 直接连用会报「字段访问需要可寻址对象」——
> 静态方法返回的类值在寄存器中，必须先 `var o = Point.origin()` 存入变量再访问。

**继承**（复用式，无虚表/无动态分派/无向上转型）：

```c
class Animal {
    var name: string
    method sound() -> string { return "..." }
}
class Dog extends Animal {
    var breed: string
    method sound() -> string { return "Woof" }   // 遮蔽父类方法
}
// Dog 实例布局 = Animal 字段（在前） + 自身字段（拍平）
// 方法解析：自身 → 父类逐级；子类同名遮蔽父类
```

> 注意：文档早期用过 `str`，**当前类型名是 `string`**（`var name: string`）。

## 3. 编译期会报错的写法（负例）

以下代码**都会在编译期报错**，AI 不要生成：

| 场景 | 错误信息（关键词） |
|---|---|
| `Counter(0).count` / `make().get()`（寄存器中的类值直接访问） | 「字段访问需要可寻址对象」/「方法调用的对象需要可寻址的类实例」 |
| 静态方法通过实例调用 `c.make()` | 「静态方法必须通过类名调用」 |
| 实例方法通过类名调用 | 「方法调用的对象必须是类实例」 |
| 继承环 `class A extends B` 且 B extends A | 「类继承形成环」 |
| 子类字段与父类字段重名 | 「字段名必须跨继承链唯一」 |
| 字段无类型标注且无默认值 | 「字段必须标注类型或有默认值」 |
| 字段默认值是非字面量表达式 | 「默认值必须是字面量」 |
| 类名与函数名冲突 / 类重复定义 | 「类名与函数名冲突」/「类重复定义」 |
| 函数体内定义类 / import / 嵌套函数 | 「顶层只允许…」/「函数体内不支持…」/「函数体内不支持嵌套函数定义」 |
| const 变量重新赋值 | 「不可变变量不能赋值」 |
| 类型不匹配（如 i64 赋给标注 i32 的变量） | 「类型不匹配」 |
| 元组空解构 `var () = ...` | 「空解构 () 不支持」 |
| 表初始化非表字面量 | 「初始化必须是表字面量」 |
| string 与 i64 直接拼接 | 类型不匹配错误 |

## 4. 未实现 / 不要使用的特性

- `ui` / `db` 文件角色（M4 规划）
- 二维表、字符串 id 表**运行时**（语法能解析但语义报错）
- `data` 文件的 import 数据表化
- 库编译（`library` 角色声明可用，但编译为库的流程未完成）
- `--target` 交叉编译、`--backend=gnu`
- 一等函数、默认参数、函数重载
- 类：对象比较（`==`）、`println` 打印对象、方法重载、析构
- 裸代码块（函数体内的 `{ }`）、空元组 `()`

## 5. 编写 tie 代码的硬性规则

1. **类、import、func 只出现在文件顶层**；函数体内只有语句。
2. **行尾分号可省略**（ASI 自动补全）；同一行多条语句必须用 `;` 分隔。
3. 类实例要访问字段/调方法，**必须先存入变量**（`var p = Point(0); p.x`），不能 `Point(0).x`。
4. 继承字段跨链唯一；方法遮蔽允许（子类同名覆盖父类）。
5. `string` 字面量加双引号；`char` 单引号。
6. `logic` 文件必须含 `func main()`。
7. 所有语句（变量声明、赋值、表达式、return）以换行或 `;` 结束。

## 6. 完整可运行示例（可直接粘贴验证）

```c
// tie:logic
class Animal {
    var name: string
    method sound() -> string {
        return "..."
    }
}
class Dog extends Animal {
    var breed: string
    method sound() -> string {
        return "Woof"
    }
}
func divmod(a: i64, b: i64) -> (q: i64, r: i64) {
    return (a / b, a % b)
}
func main() {
    var d = Dog("Rex", "Golden")
    println(d.name)          // Rex
    println(d.sound())       // Woof
    d.name = "Max"
    var (q, r) = divmod(17, 5)
    println(q + r)           // 5
    for i in 0..3 {
        println(i)           // 0 1 2
    }
    var total: i64 = 0
    var arr: table = [1, 2, 3]
    for item in arr {
        total = total + item
    }
    println(total)           // 6
}
```

> 注意 ASI 规则：**每条语句独占一行**（`return "..."` 与 `}` 不能同行，
> 因为分号只在换行处自动补全；同一行多条语句必须显式 `;`）。

保存为 `demo.tie` 后用 `cargo run -p tie -- demo.tie` 编译运行，输出应为：

```
Rex
Woof
5
0
1
2
6
```

---

# 第二部分：编译器架构（供开发 tie 的 AI 使用）

## 7. 工程结构与数据流

```text
crates/
├── tie-prep/      预处理：清理代码、提取头（// tie:）、识别文件角色（logic/ui/db/data/library）
├── tie-frontend/  前端：lexer（含 ASI）→ parser → semantic，自研
├── tie-llvm/      中端+后端驱动：AST → LLVM IR 文本；调用 opt/clang/lld
├── tie-interp/    解释执行（占位，REPL 用）
└── tie/           CLI 主入口：角色分派调度器 + REPL
```

流水线：`tie-prep`（预处理）→ 按角色分派（logic/library → tie-llvm 编译）→
tie-llvm 内部：AST → `.ll` 文本 → `opt` 优化 → `clang` 汇编 → `lld` 链接 → 可执行文件。

## 8. 前端（tie-frontend）

### 8.1 词法（lexer.rs）

- 产出 `Vec<Token>`，`TokenKind` 含 `Ident/Int/Float/Str/TypeKw/Keyword/Semi/Eof...`。
- **ASI 自动补全在词法层实现**（token 流层面插入 `Semi`）：换行处语句已完整则补 `;`；
  括号未闭合、行尾是二元运算符/`,`/`.`/开括号、行尾是 `else`/`in` 等则不补。

### 8.2 语法（parser.rs）

- 递归下降解析器 `Parser`，入口 `parse_program`。
- 顶层只允许三种语句：`Stmt::FnDef` / `Stmt::Import` / `Stmt::Class`，其他 → 语法错误。
- 关键函数：`parse_fn_def`、`parse_class`（含 `extends`）、`parse_method`（含 `static`）、
  `parse_var_decl`（含元组解构 desugar 为临时变量 + 字段访问）、`parse_expr_or_assign`
  （`Ident = ...` → Assign；`obj.field = ...` → FieldAssign）。
- `is_addressable_base`：判断表达式是否可寻址（Var 或 FieldAccess 链）。

### 8.3 AST（ast.rs）

```rust
pub enum TypeSpec { Named(TyKw), Tuple(Vec<TupleField>), Class(String) }
pub enum Stmt { VarDecl, FnDef, Expr, Assign, Return, If, While, For, Switch, Import, Class, FieldAssign }
pub enum Expr { IntLit, FloatLit, StrLit, CharLit, BoolLit, Var, Call, Unary, Binary, Range,
                TableLit, Index, TupleLit, FieldAccess, MethodCall }
pub struct ClassDefStmt { name: String, parent: Option<String>, fields: Vec<ClassField>,
                          methods: Vec<MethodDefStmt>, span: Span }
pub struct MethodDefStmt { name: String, is_static: bool, params: Vec<Param>,
                           ret_ty: TypeSpec, body: Vec<Stmt>, span: Span }
pub struct FieldAssignStmt { base: Box<Expr>, field: String, value: Expr, span: Span }
```

### 8.4 语义（semantic.rs）

入口 `analyze(&Program) -> Result<SemanticResult, SemanticError>`，共 **4 遍**：

1. **收集函数签名**：`funcs: HashMap<名称, FuncSig>`，重复定义报错（允许前向引用）。
2. **collect_classes**：类名登记（vs 函数名/类名冲突）→ 逐个 `flatten_class` 拍平继承链
   （父类字段在前 + 方法遮蔽，`chain: HashSet` 检测继承环）。
3. **check_fn**：逐函数体 `check_stmt`（作用域为 `HashMap<名称, TypeSpec>`）。
4. **check_method**：逐方法体；实例方法先在作用域插入 `this` → 当前类类型。

关键数据结构：

```rust
pub struct ClassInfo {
    pub parent: Option<String>,
    pub fields: Vec<ClassField>,                 // 拍平字段（含继承），顺序即 LLVM 结构体字段序
    pub field_index: HashMap<String, usize>,     // 字段名 → GEP 偏移（语义与 IR 共用唯一权威）
    pub methods: HashMap<String, MethodSig>,     // 拍平方法（子类遮蔽父类）
    pub method_owner: HashMap<String, String>,   // 方法名 → 实际定义类（mangle 用 @<定义类>$<方法>）
}
```

- `resolve_class_field_ty`：字段类型解析——显式标注优先；无标注从默认值字面量推导；
  两者皆无 → 报错。
- `is_addressable_expr`（语义层）：类字段访问/实例方法调用要求对象可寻址
  （Var 或 FieldAccess 链）；寄存器中的类值（构造表达式/方法调用结果）→ 报错。
- `expr_types: HashMap<usize, TypeSpec>`：用表达式地址（`addr_of`）记录推导类型，IR 层查询。
- `tables: HashMap<usize, TableInfo>`：表字面量布局元数据（元素类型/长度）。

## 9. 中端+后端（tie-llvm / ir.rs）

入口 `run(program, semantic_result)`（`IrGenerator`）。

### 9.1 核心设计

- **值类型用字面结构体**：元组/类 → LLVM `{T0, T1, ...}`，通过 `ty_cache` 缓存
  （`HashMap<TypeSpec, String>` 生成 `%tup.N` / `%cls.N` 类型名），避免重复声明。
- **方法 mangle**：`@<定义类>$<方法名>`（用 `method_owner` 查定义类），签名
  `define ret @C$m(ptr %_this, params...)`——实例方法首参 `ptr %this`，
  用 `by_ptr` 的 `VarBind` 绑定（**不 alloca**，直接复用入参指针）。
- **字段读**：`GEP + load`；**字段写**：`GEP + store`（`gen_field_assign`）。
- **构造**（`gen_construct`）：`insertvalue` 链逐字段装配，缺省参数用默认值/零值。
- **元组字段访问**：`extractvalue`（寄存器值直接取，无地址要求）；
  **类字段访问**：`gen_class_addr` 先取地址（Var → 绑定地址；FieldAccess 链 → 逐级 GEP），再 load。
- **`current_ret_ty`**：方法内 return 的返回类型查询——按 `类$方法` 拆分，查 `classes`，
  兜底查 `funcs`（→ `I64` 兜底）。

### 9.2 运行流程

1. 先遍历 `Stmt::Class` 生成全部方法 `@C$m`（方法间可互相调用）。
2. 再遍历 `Stmt::FnDef` 生成顶层函数（`gen_fn`），含 `main`。
3. import 已在 driver 层递归加载为内联函数（语义分析前）。
4. 生成 `.ll` → 调用 `opt`（优化）→ `clang`（汇编）→ `lld`（链接）。

## 10. 给编译器开发 AI 的硬性规则

1. **修改 AST 枚举时**：新增变体会破坏 `semantic.rs` / `ir.rs` 的 `match`——需同步
   添加分支并保证 `match` 穷尽。
2. **字段索引唯一权威**：`ClassInfo.field_index`（语义层计算），IR 层不得自行遍历拍平，
   否则偏移错位。
3. **类方法与顶层函数分离**：类方法不进 `result.funcs`；用 `result.classes` + `method_owner`。
4. **语义先于 IR**：所有编译期错误在 semantic 层拦截（如可寻址性、继承环、字段重名），
   IR 层出现 `unwrap/expect` 即视为内部错误。
5. **宽类型/`code`/`table` 是编译期概念**：语义阶段展开为具体类型，IR 阶段不出现。
6. **ASI 在词法层**：新增语法时考虑 token 流层面的分号补全影响。
7. 提交消息用中文（见 CHANGELOG.md 的既有风格）。

## 11. 常见开发任务索引

| 任务 | 涉及文件 |
|---|---|
| 新增语句/表达式 | ast.rs（枚举）→ lexer.rs（关键词/token）→ parser.rs（解析）→ semantic.rs（检查）→ ir.rs（生成） |
| 新增类型 | ast.rs（TypeSpec）→ parser.rs（parse_type）→ semantic.rs（types_match）→ ir.rs（llvm_ty） |
| 修改类/OOP 语义 | semantic.rs（collect_classes/flatten_class/check_method） |
| 修改字段布局 | semantic.rs（field_index）→ ir.rs（gen_class_addr/gen_field_assign） |
| 新增 CLI 选项 | crates/tie/（主入口）+ README.md CLI 表 |
| 修改预处理/角色 | crates/tie-prep/ |

