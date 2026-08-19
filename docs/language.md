# tie 语言规范（v0.1 草案）

> ⚠️ **早期开发阶段**：本规范随实现演进，语法与语义可能变更，一切以实现为准。

> 工程结构、CLI 用法、编译流水线与路线图见根目录 [README.md](../README.md)。
> 当前版本：0.1（设计阶段）

## 1. 语言定位

tie 是一门**通用编程语言**，目标是让用户仅用这一门语言就能完成大型项目：

- 写**界面**（UI 文件）
- 写**逻辑**（业务/算法代码）
- 写**数据库**（表结构与数据定义）
- 充当**数据交换格式**（类似 JSON/XML 的角色）

同一个 `.tie` 文件扮演什么角色，由文件**前几行的头（Header）**决定。头与主要内容严格分离。

## 2. 文件结构：头 + 内容

tie 源文件由两部分组成，二者分离、职责明确：

```
┌─────────────────────────── 头（Header）───────────────────────────┐
│  文件前 N 行，声明本文件用途、编译选项、平台目标等元信息。         │
│  以 tie 指令开头，编译器据此分派不同的解析与生成策略。             │
└─────────────────────────── 内容（Body）───────────────────────────┘
│  真正的代码/数据，根据头的类型使用对应的语法子集。                 │
└────────────────────────────────────────────────────────────────────┘
```

### 2.1 头语法

头必须出现在文件**最前面几行**，是真正的语法行（不再是 `// tie:` 注释指令），
连续排列（允许其间空行）直到第一个非头内容行为止：

```c
type tie<logic>    // 本文件是逻辑代码（默认，可省略）
func main() {
    println("hello")
}
```

声明语法：`type tie`（Type 角色，裸形式）或 `type tie<X>`（X 为子类型）。

### 2.2 头类型（文件角色）

| 头部声明              | 角色     | 说明                          |
| ------------------- | ------ | --------------------------- |
| `type tie`        | 泛型入口  | Type 角色，由裸 `type tie` 表达       |
| `type tie<logic>` | 逻辑代码  | 默认角色。可编译为可执行文件或库            |
| `type tie<ui>`    | 界面文件  | 声明 UI 组件树，编译器生成界面代码         |
| `type tie<db>`    | 数据库文件 | 声明表结构/数据，编译器生成建表与读写代码       |
| `type tie<data>`  | 数据交换文件 | 纯数据，类似 JSON，可被其他文件 `import` |
| `type tie<class>` | 类/库文件 | 编译为静态库 `.a`，不生成 main          |
| `type tie<script>` | 脚本     | 脚本文件（编译可执行）                  |
| `type tie<port>`  | 端口/接口  | 端口/对外接口文件                    |
| `type tie<ir>`    | IR 文件   | 直接生成 LLVM IR（.ll），不继续 opt/clang 链接 |

**未声明头**时默认按 `logic` 处理（可执行文件）。
子类型全集：`script` / `data` / `ui` / `class` / `logic` / `port` / `db` / `ir`；
`type` 角色由裸 `type tie` 表达（`type tie<type>` 是格式错误）。
优化级别 / 交叉编译目标等选项不再放头部——`opt` / `target` **仅 CLI**。

### 2.3 头 vs 主要内容分离原则

- 头只允许出现在文件头部（前 N 行），内容区出现 `type tie` 视为普通代码。
- 头与内容之间允许空行分隔。
- `type tie<data>` 文件**只能**含数据声明（如 `record`/字面量表），不含函数体。
- `type tie<ui>` 文件可含 `view`/`layout` 声明 + 事件处理函数。
- 内容单文件即可运行（`logic` 含 `func main()`），也可多文件通过 `import` 组合（见 §7）。
- 文件名 `xxx.<角色>.tie` 可声明默认角色（头部声明优先，不一致时警告并采用头部）。

### 2.4 预处理（Harbor M3 起：tie 语言自举）

**预处理（tie-prep）** 是四段式流水线的第一段，在源码文本层面工作，职责：
去 BOM / CRLF→LF 归一（壳层）→ 提取声明行 → 判定角色 → 重建正文。

Harbor M3 起，预处理**核心逻辑完全用 tie 语言编写**（`prep/core.tie`），
Rust 侧仅剩解释执行壳：

1. 字节规范化（去 BOM、`\r\n`→`\n`）留壳层——tie 字符串字面量无法表达 BOM 字符；
2. 通过 tie-interp `eval` 注册模块，再 `eval_call("prep::process", src)`
   以**字符串值直传**源码调用（不经源码文本转义，换行/引号原样直传）；
3. 模块返回**协议文本**，壳层解析还原角色/正文（声明行已剥离）：

```text
ROLE:logic          ← 角色（type/script/data/ui/class/logic/port/db/ir）
BODY:12             ← 正文码点数（str_len 语义，Rust 侧按字符截取）
<正文恰好 12 个字符> ← 清理后的正文（不含声明行）
```

> 编码约定：BODY 声明的是**码点数**（`str_len`），非字节数。字符串在 tie 中
> 是 UTF-8 编码，`len` 返回字节数而 `str_char`/`str_len` 按 Unicode 码点索引——
> 中文等多字节字符下字节数 ≠ 码点数（一个汉字 3 字节、1 码点），字符串遍历
> 边界必须用 `str_len` 才能保证中文文本不错位（`str_len("你好") == 2` 码点，
> `len("你好") == 6` 字节；用 `len` 做 `str_char` 的循环上界会越界返回空串，
> 导致 trim 尾随空白残留、slice 截取错乱）。

**扩展性**：新增转换器/处理器 = 新增一个 tie 模块（约定顶层
`func process(src: string) -> string`），Rust 侧零改动。
示例 `prep/indent.tie`（制表符→4 空格）可通过
`tie-prep <file> --module prep/indent.tie` 挂载执行。

> tie:script 是上述机制的一般化协议：模块约定、`eval`/`eval_call` 执行语义、
> 协议文本格式与三层调用入口（Rust / CLI / tie 程序内）的完整说明见
> [docs/tie-script.md](tie-script.md)。

## 3. 类型系统（静态类型）

tie 采用**静态类型**（编译期类型检查），后端为 LLVM 强类型 IR 服务。
类型关键字采用 **Rust 风格**：`i8`/`i16`/`i32`/`i64`/`u8`/`u16`/`u32`/`u64`/`f32`/`f64`/`bool`/`char`/`string`/`void`/`code`。

### 3.1 基本类型

| 类型                           | 说明                                   | 对应 LLVM 类型                   |
| ---------------------------- | ------------------------------------ | ---------------------------- |
| `i8` / `i16` / `i32` / `i64` | 有符号整数                                | `i8` / `i16` / `i32` / `i64` |
| `u8` / `u16` / `u32` / `u64` | 无符号整数                                | `i8` / `i16` / `i32` / `i64` |
| `f32` / `f64`                | 浮点数                                  | `float` / `double`           |
| `bool`                       | 布尔                                   | `i1`                         |
| `trit`                       | 平衡三进制（三值逻辑 -1/0/+1，数论常用）             | `i8`                         |
| `char`                       | 单字符（UTF-32）                          | `i32`                        |
| `string`                     | 字符串（不可变）                             | `ptr`（i8*，长度前缀）              |
| `void`                       | 无返回值（仅函数）                            | `void`                       |
| `code`                       | 代码片段（**编译期类型**：解析翻译为 AST 子程序，无运行时实体） | —（IR 阶段已展开）                  |

### 3.2 平衡三进制 trit（三值逻辑，M4 补齐）

`trit` 是**平衡三进制**类型（ternary digit），值域 `-1/0/+1`——数论与三值逻辑常用
（类似 bool 的三值扩展：`true`/`unknown`/`false`）。

**字面量**：`true`（+1）/ `zero`（0）/ `false`（-1）——`true`/`false` 在 `trit`
标注上下文中适配为 trit 值（裸 `true` 仍为 `bool`）：

```c
var p: trit = true    // +1
var z: trit = zero    //  0
var n: trit = false   // -1
```

**Kleene 三值逻辑**（`&&` = min，`||` = max，`!` = 取反）：

| `&&` | +1 | 0 | -1 |  `\|\|` | +1 | 0 | -1 | `!` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **+1** | +1 | 0 | -1 | **+1** | +1 | +1 | +1 | +1→-1 |
| **0** | 0 | 0 | -1 | **0** | +1 | 0 | 0 | 0→0 |
| **-1** | -1 | -1 | -1 | **-1** | +1 | 0 | -1 | -1→+1 |

**饱和算术**（clamp 到 [-1,1]）：`trit + - * trit` → `trit`（如 `1+1=1` 饱和）；
`trit` 与 `i64` 混合运算 → `i64`（sext 提升，如 `1+5=6`）；`div`/`mod` 不允许。

**比较**：`trit == != < > <= >=`（与 trit 或 i64）→ `bool`。

**转换**：`to_string(trit)` → `"-1"/"0"/"1"`；`parse_trit(s)` → `trit`（非法输入报错）。

**典型用途**：三路比较（`compare(a,b)` 返回 -1/0/+1）、数论算法（平衡三进制数）、
三态逻辑（是与否之外的"未知"）。

### 3.3 宽类型（加速编译）

宽类型关键词覆盖一组相关类型，编译器无需精确推断即可快速编译；同时提高准确率与代码可读性：

| 关键词        | 覆盖范围                          | 说明        |
| ---------- | ----------------------------- | --------- |
| `num`（数）   | 全部数类型：整数（i8..u64）、浮点（f32/f64） | 数字字面量均可匹配 |
| `text`（文）  | 字符串与字符：`string`、`char`        | 文本数据      |
| `misc`（其他） | 其余全部类型：`bool`、`void`、`code` 等 | 兜底类型      |

宽类型是**编译期概念**（类别框）：声明时只校验初始化表达式属于该类别，
变量在作用域内以**推导出的具体类型**参与运算（整数字面量→`i64`，浮点字面量→`f64`），
IR 生成阶段不出现宽类型。

使用示例：

```c
var a: num = 42        // 数类型，不必写明 i64
var b: num = 3.14      // 数类型，不必写明 f64
var s: text = "hi"     // 文类型，不必写明 string
var c: misc = true     // 其他类型
```

### 3.4 表类型（复合类型）

`table` 代表**表**：数组（全部元素无 id）与高级数组（含 id 元素）的统一容器。

**表字面量**：`[col, col; row, row]`——逗号分隔**列**（元素），分号分隔**行**，
id 可选（无 id 即普通位置元素）：

```c
var arr: table = [1, 2, 3]              // 单行 3 列（无 id，隐式下标 0..2）
var arr2: table = [0:1, 1:2, 2:3]       // 单行 3 列（显式数字 id = 下标）
var tbl: table = ["a":1, "b":2, "c":3]  // 单行 3 列（字符串 id，须加双引号）
var mix: table = [1, "b":2, 3]          // 单行混合（id 可有可无）
var grid: table = [1, 2; 3, 4]          // 2 行 2 列（分号分行）
```

- **逗号 `,`**：分隔同一行的元素（列）。
- **分号 `;`**：分隔行。
- **id 形式**：可选；显式写出的 id 可为**数字**（下标，不加引号）或**字符串**（命名键，**必须加双引号**，这是 tie 的底层逻辑——字符串字面量一律显式引号，杜绝与裸标识符歧义）；省略 id 时按位置隐式编号。
- **value 类型**：任意类型（基本类型/复合类型），同一表内元素类型必须一致。
- 全部元素无 id → 行为等同普通数组（按下标访问）。
- 存在 id 元素 → 同时支持按下标与按 id 访问（表语义）。
- 空表 `[]` 合法，元素类型暂定 `i64`。

> **当前实现状态**：表运行时支持**单行纯位置表 + 数字下标**（存取、遍历已实现）。
> 字符串 id 表即键值表 **map**（`["a":1]` 语法），已随 E3 + T0.5 落地（见 §3.4.1）；
> 二维表（`[1,2;3,4]`）语法可解析，但语义阶段报"留待 M3"，暂不可用。

**带元素类型的表 `table<T>`（A1，自举前置）**：参数/变量可标注元素类型，
函数体内对表参数的 `t[i]` 下标访问、`len(t)`、`for x in t` 遍历的元素类型静态确定：

```c
func sum(t: table<i64>) -> i64 {        // 函数内可直接 t[i] 下标读取（元素 i64）
    var s: i64 = 0
    var i: i64 = 0
    while i < len(t) {
        s = s + t[i]
        i = i + 1
    }
    return s
}
func join(t: table<string>) -> string { /* 字符串表参数同样可直接下标读取 */ }
```

- 元素类型限标量（i64/f64/string/bool 等）；`table` 裸类型仍可作参数（元素约定为 string）；
- 实参校验：表字面量/动态表变量的元素类型须与 `table<T>` 一致（编译期报错）；
- **跨函数传表不再需要"逗号分隔字符串序列"规避**——表参数可直接下标访问
  （自举编译器 AST 跨函数传递的基础）。

### 3.4.1 键值表 map / map\<T\>（E3；排序键二分 T0.5 已实现）

`map` 是**键值表**（字符串 id 表的高级形式，编译器符号表的容器），元素类型
为 `string -> value`：字面量 `["a":1]`（键为字符串，**必须加双引号**）、
`m["k"]` 下标读写、`len(m)` 取条目数：

```c
var m: map = ["a":1, "b":2]     // 键值表：string -> i64
var n: map<string> = ["x":"hi"] // map<T>：显式值类型 T（map = map<i64>，默认值类型）
var v = m["a"]                  // 下标读：1
m["c"] = 3                      // 下标写：不存在则插入，存在则覆盖
var k = len(m)                  // 条目数：3
```

**排序键二分（T0.5）**：map 内部**键恒按 strcmp 字节序有序存储**（插入 = 二分
定位 + memmove 平移，查找 = 二分 + strcmp 直比，均零分配），10k 次查找由
线性扫描的 ≈ 6.27s 降至 ≈ 2.7ms（~2295×，见 `scripts/bench/map-bench.tie`）。

**行为契约变化（T0.5）**：map 的输出展示（REPL/打印）**按键排序**
（`{a: 1, m: 2, z: 3}`），**不再依赖插入序**——插入 `["z":3, "a":1]`
打印为 `{a: 1, z: 3}`。依赖插入序的旧行为已不存在。

**约束**：

- 键恒为字符串（字面量必须加双引号，与 §3.4 字符串 id 规则一致）；
- 元素类型同构：值类型在整个表中一致（`map<string>` 显式标注）；
- **map 不能作全局变量**（顶层 `var m: map;` 语法层即拒绝，见 §3.4.2）；
- 空 map 字面量 `[]` 与空表字面量存在语法歧义（map 场景建议用
  非空初始，见 map-bench 注释）。

### 3.4.2 顶层表全局变量（T0.4 已实现）

顶层可声明**表全局变量**：`var g: table<T>;`——无初始化器 = 默认空动态表，
也可 `= []`（空表字面量）：

```c
var g: table<i64>;          // 顶层全局动态表：跨函数持久，main 入口运行时创建
var h: table<string> = [];  // 显式空表初始化（等价默认）

func add(x: i64) {
    table_push(g, x)        // 函数内直接读写：push / 下标 / len / for
}
func dump() -> i64 {
    return len(g)
}
func main() {
    add(1)
    add(2)
    println(dump())         // 2（跨函数累加，main 入口持久）
    println(g[0])           // 1（main 直接下标读取）
}
```

**语义**：

- **跨函数持久**：全局表在 main 入口运行时创建，进程内所有函数共享同一
  实例（table_push / 下标写 / len / for 遍历均可直接使用）；
- **可作 ref 实参**：全局表是"可寻址表变量"，可传给 `ref table<T>` 形参
  （§6.2），函数内修改直接写回全局（intern 库即此模式，见 §7.2）；
- 全局表元素类型可标注（`table<i64>`），省略时按裸 `table` 约定为 `string`。

**约束（编译期报错）**：

- **const 全局表暂不支持**：`const g: table<i64>;` → `const 全局表暂不支持：'g'（表在 main 入口运行时创建，无法静态初始化）`；
- 全局表初始化器**只能是空表 `[]`**（或省略）；非空表字面量初始化 → 报错；
- **仅 `table<T>` 支持全局**；`map` 作全局变量在语法层即拒绝；
- 非表类型全局变量限标量（i8..u64/f32/f64/bool/char/string）；
- 全局表名与函数名冲突 / 重复定义 → 编译期报错。

**参考验收**：`tests/language/global_table.tie`（运行输出 2/1/2）、
`tests/language/global_table_const.tie`（负例 @3:1）。

### 3.5 元组类型（复合类型）

元组是**固定长度、元素可异构**的值类型（C# 风格）：用圆括号包裹、逗号分隔元素。
元素 ≥ 1（空元组 `()` 不支持）；元素可有名字（命名元组）也可匿名。

**类型标注**：`(T1, T2)` 或 `(name: T1, name: T2)`（字段名可选）：

```c
func divmod(a: i64, b: i64) -> (q: i64, r: i64) {   // 命名元组作为返回类型（多返回值）
    return (a / b, a % b)                            // 返回元组字面量
}
```

**字面量**：`(1, "a")` / `(x: 1, y: 2)`（元素 ≥ 1；空元组 `()` 不支持）：

```c
var t = (10, 20)                 // 推导为 (i64, i64)
var p = (x: 3, y: 4)             // 推导为 (x: i64, y: i64)
```

**字段访问**（三种形式等价）：

- 命名访问：`t.x`（仅命名元组可用）
- 位置访问：`t.Item1`、`t.Item2` …（1 起编号，C# 风格）
- 数字下标：`t.0`、`t.1` …（0 起编号）

```c
println(t.Item1)   // 10（位置访问）
println(t.0)       // 10（数字下标）
println(p.x)       // 3（命名访问）
```

**解构**：`var (a, b) = 元组表达式` 一次声明多个变量（编译期展开为临时变量 + 字段访问）：

```c
var (q, r) = divmod(17, 5)   // q = 3, r = 2
const (s, t) = divmod(9, 2)  // 解构变量同样可 const
```

**类型匹配规则**：

- 元组之间比较**长度与字段类型**（字段名是编译期标签，`(x: i64, y: i64)` 与 `(i64, i64)` 可互换）；
- 字段级字面量适配：`var w: (i32, i64) = (7, 8)` 合法（7 适配 i32）；
- 支持嵌套：`(name: string, scores: (i64, i64))`。

**本期限制**：

- `println` 不能直接打印元组（请逐字段打印）；
- 元组不支持 `==` / `!=` 等比较运算（逐字段比较留待后续版本）；
- 字段名重复（`(x: 1, x: 2)`）、字段越界（`t.Item3`）在编译期报错。

### 3.6 类型标注与推导

变量声明用 `var`（可变）或 `const`（不可变）；不标注类型时由编译器推导（整数字面量默认 `i64`，浮点字面量默认 `f64`）：

```c
var x = 5             // 可变变量，自动推导为 i64
var f = 3.14          // 自动推导为 f64
var n: i32 = 1        // 显式标注为 i32
const s: string = "hi"   // 不可变变量，初始化后不可再赋值
var arr: table = [1, 2, 3]
```

### 3.7 泛型（编译期单态化，2026-08-14 实现）

tie 支持用户定义的泛型：**泛型函数**与**泛型 struct**。类型参数在声明处
显式写出 `<T1, T2, ...>`，调用/构造点从实参类型推断（必要时可显式写出）。
实例化机制为**编译期单态化**——每个具体类型参数组合展开为独立代码，零运行时
开销（tie 无函数指针、无运行时类型信息，单态化是唯一自然路径）。

**泛型函数**：

```c
func max<T>(a: T, b: T) -> T {
    if a > b {
        return a
    }
    return b
}

func pick<A, B>(a: A, b: B) -> A {
    return a
}
```

- 类型参数列表紧跟函数名，逗号分隔，允许 1 个或多个；
- 类型参数可在形参类型、返回类型、函数体内出现（作类型使用）；
- 类型参数名允许任意标识符，作用域内**遮蔽内置类型名**（如 `func f<int>(x: int)`
  中 `int` 是类型参数而非内置类型）。

**泛型 struct**：

```c
struct Box<T> {
    var value: T
}
```

- 类型参数列表紧跟 struct 名；字段类型可引用类型参数；
- struct 构造 `Box(实参)` 可从构造实参推断；`Box<i64>()`（无参构造）必须显式。

**实例化（使用点）**：

```c
var m1 = max(3, 5)           // T = i64（调用点推断）
var m2 = max(3.5, 1.2)       // T = f64
var m3 = max<i64>(9, 4)      // 显式类型实参
var b = Box<i64>(42)         // 显式 + 构造
var b2 = Box(3.14)           // 构造参数推断（T = f64）
var nb = Box<table<i64>>(t)  // 嵌套：实参可为内建泛型
```

- **调用点推断**：逐实参匹配形参类型中类型参数的位置；同一类型参数多处推断
  必须一致，不一致报错（`类型参数 T 推断冲突: i64 vs f64`）；
- **显式类型实参优先**：`max<i64>(...)` 指定部分不参与推断；未指定部分继续推断；
- **无法推断**：无信息实参（如 `identity<T>()` 无参调用）→ 报错要求显式指定
  （`无法推断类型参数 T（调用点无足够信息，请显式指定类型实参）`）；
- 返回类型含类型参数且由参数推断 → 调用表达式类型 = 替换后的返回类型。

**泛型 struct 方法（数据逻辑分离）**：方法定义在绑定 struct 名的命名空间，
接收者类型为 `Box<T>` 时 T 从接收者实例类型推断：

```c
namespace Box {
    pub func get<T>(b: Box<T>) -> T {
        return b.value
    }
}
var v: i64 = b.get()     // T 从接收者 Box<i64> 推断
```

**符号 mangling（单态化展开产物）**：实例化函数/struct 以
`全名 + '$' + 类型实参片段` 命名（如 `max$i64`、`Box$table_i64`、`pick$i64$string`），
片段递归规范化（`table<i64>` → `table_i64`、`map<string>` → `map_string`）。
单态化展开产物走现有类型检查：模板体内对 T 的操作在替换为具体类型后被现有
类型系统验证，错误在实例化点报告。

**内建泛型**：`table<T>` / `map<T>` 保持内建，不改为用户泛型；用户泛型实参
可为内建泛型实例，用户泛型函数形参可为 `table<T>`（T 为类型参数时：
`func sum<T>(xs: table<T>) -> T`——实例化时整体替换）。

**错误用例**：推断冲突 / 无法推断 / 实例化后类型错误 / 实例化深度超限
（递归自引用模板触发 64 层防御）。

**参考验收**：`tests/language/generics.tie`（9 个正例：推断/显式/嵌套/方法/多类型
参数）、`tests/language/generics_neg.tie`（5 个负例）。

### 3.8 枚举 enum（ADT 标签联合，2026-08-15 实现）

tie 支持 **Rust 风格 ADT 枚举**：无数据变体（C 风格常量组）与带数据变体
（payload 元组式），并支持泛型 enum。枚举在 LLVM 层表示为静态结构体
`{ i64 tag, i64×K 槽 }`（K = 最大变体 payload 字段数），零运行时开销。

**无数据变体**：

```c
enum Color {
    Red
    Green
    Blue
}
```

**带数据变体（payload）**：

```c
enum Shape {
    Circle(i64)
    Rect(i64, i64)
}
```

**泛型 enum**：

```c
enum Option<T> {
    Some(T)
    None
}
```

- 变体之间无分隔符（ASI 自动分号，与 struct 字段同风格）。
- payload 为类型列表（元组式，无字段名）。
- 泛型：`enum Name<T1, T2>`，变体 payload 可引用类型参数（编译期单态化，
  与泛型 struct 同机制）。

**构造**：

```c
var c = Color.Red            // 无 payload 变体 = 常量
var s = Shape.Circle(5)      // 带 payload 变体 = 构造调用
var o = Option.Some(42)      // 泛型构造（实参推断 T = i64）
var c2: Color = Color.Green  // 类型注解
```

- `EnumName.Variant`（无括号）引用无 payload 变体 → 常量值；
- `EnumName.Variant(args...)`（带括号）构造带 payload 变体；
- 带 payload 变体裸引用（无参数）→ 报错"需要 payload 参数"。

**匹配（switch）**：

```c
switch s {
    case Shape.Circle: println("circle")
    case Shape.Rect:   println("rect")
    default:           println("other")
}
```

- switch 对象可为枚举（case 变体引用解析为 tag 常量，走整数比较链）。

**当前限制（一期）**：
- payload 类型白名单：整数族（i8..u64）/bool/char/trit；f32/f64、string、
  struct、table/map payload 暂不支持（报"变体 payload 暂不支持类型"）；
- 枚举值 `==`/`!=` 比较暂不支持（报"枚举暂不支持 == 比较"）；
- REPL（interp）暂不支持 enum 定义（报"REPL v1 暂不支持 enum 定义"）；
- 泛型 enum 无 payload 变体裸引用（如 `Option.None`）无法推断 T → 报错。

**参考验收**：`tests/language/enum.tie`（无数据/带数据/泛型/struct 字段/跨函数）、
`tests/language/enum_neg.tie`（重名变体/payload 白名单/== 比较/函数名冲突）。

## 4. 语句与分隔符（ASI 自动补全）

### 4.1 分号规则（核心特性）

**单行一条语句时，行尾不需要写分号，编译器自动补全。**

- 换行处语句已完整 → 自动插入分号（类似 JS 的 ASI）。
- 同一行写多条语句 → 必须用分号显式分隔。
- 语句跨行（如函数调用换行、`if` 条件跨行）→ 行尾按"语句是否完整"判断，不完整则不补。

```c
var a = 1          // 行尾自动补 ;
var b = 2          // 行尾自动补 ;
var c = 1; var d = 2   // 同一行必须显式分号

var sum = a
    + b            // 上一行表达式未结束（+ 在下一行），不补分号

var arr = [
    1,
    2,
]                  // 括号内换行不补分号
```

### 4.2 补全判定规则（实现要点）

1. 遇到换行时，若当前语句"语法上可结束"，则补 `;`。
2. 括号 `()` `[]` `{}` 未闭合 → 不补。
3. 行尾是二元运算符 / `,` / `.` / 开括号 → 语句未结束，不补。
4. 行尾是关键字（如 `else`、`in`）→ 不补，与上一结构连读。

实现位置：**词法分析阶段**（token 流层面插入 `Semi` token），供解析器直接消费。

## 5. 控制流

```c
if cond { } else if cond2 { } else { }     // 条件分支
while cond { }                              // 循环
for i in 0..10 { }                          // 范围循环
for item in arr { }                         // 集合循环
for c in s.chars() { }                      // 字符串码点迭代（逐字符，Unicode 安全）
return expr                                 // 返回值（可省略分号）
break                                       // 退出最近循环（E1）
continue                                    // 跳最近循环的下一次迭代（E1）
```

**字符串码点迭代** `for c in s.chars()`：按 Unicode 码点逐字符遍历（s 为
string），循环变量 c 是单字符 string。与字节索引分离——中文等多字节字符
（UTF-8 一个汉字 3 字节）每次迭代得一个完整字符，无需手动 utf8_seq_len
步进。等价手写：

```c
var pos: i64 = 0
while pos < len(s) {
    var l = utf8_seq_len(s, pos)
    var cp = utf8_char_at(s, pos)
    // ... 处理单字符 str_from_code(cp) ...
    pos = pos + l
}
```

随机码点索引（O(1)）：先 `utf.to_chars(s)` 转码点表再下标访问；码点数用
`utf.codepoint_count(s)`（与字节数 `utf.byte_len(s)` 分离）。

### 5.1 break / continue 与循环标签（E1+E5）

**break**：立即退出所在循环（while/for），继续执行循环之后的语句。
**continue**：跳过本次迭代剩余语句，进入循环的下一次迭代——while 重新判断条件；
for 先执行步进（自增）再判断条件。

**循环标签（E5）**：给循环命名后，`break L` / `continue L` 可跳出/跳过多层嵌套循环：

```c
outer: for a in 0..10 {                     // 标签 outer
    for b in 0..10 {
        if b == 5 { break outer }           // 直接跳出两层循环
        if b == 2 { continue outer }        // 跳到外层 for 的下一次迭代（内层重置）
    }
}
```

- 标签语法：`标识符: while/for …`（语句开头的 `标识符 : while|for` 才被识别为循环标签）；
- 无标签 `break` / `continue` 作用于最近一层循环；
- 带标签跳转：标签必须匹配某个外层循环（从内向外查找，编译期校验，未匹配报错）；
- `break` / `continue` 只能出现在循环体内（编译期报错）；
- switch 分支天然自动退出（无 fallthrough），分支内无需 break。

### 5.1 switch 多分支（M1 已实现；模式匹配增强 M2 后）

```c
switch n {                                  // 多分支：case 值: 后接语句（无 break，无 fallthrough）
    case 1, 2:                              // 多值：任一相等即命中（逗号分隔）
        println("one or two")
    case 3..7:                              // 区间：3 ≤ n < 7（左闭右开，仅整数/字符）
        println("three to six")
    case 8 when flag:                       // 守卫：值匹配 且 flag 为真才进入
        println("eight and flag")
    case string:                            // 类型匹配：subject 为动态类型容器时才允许
        println("a string")
    default:                                // 可省略；守卫不满足时落入下一个 case
        println("other")
}
```

**语法**：`switch 对象 { case 模式[, 模式]... [when 条件]: 语句… default: 语句… }`。

**case 模式类型**（每个 pattern 须与 switch 对象类型一致）：

- **字面量**：整数 / 浮点 / 字符 / 布尔 / 负数 / 字符串；
- **区间**：`case 3..7:`（整数）或 `case 'a'..'e':`（字符）——左闭右开，`start < end`；
  浮点区间明确不支持；
- **类型匹配**：`case string:` / `case i64:`——按对象的动态类型匹配，仅在宽类型/动态
  容器对象（表、元组等）上有意义；普通静态类型对象上报错（类型恒定，恒真/恒假无意义）。

**规则**：

- **多值** = 多个相等比较的 OR 合并；与区间/守卫可自由组合（`case 1, 3..5 when cond:`）；
- **守卫** `when`：值为真才进入分支体，守卫不满足时落入下一个 case（顺序匹配）；
- 无 break、无 fallthrough（一个 case 执行完自动跳出）；
- `default` 可选且至多一个，全不匹配时执行；
- 重复的 case 值/区间在语义层报错。

**双路径一致**：编译（IR 展开为比较链：多值 OR、区间 `sge && slt` AND、守卫 AND）与
解释（tie-interp 按 Value 动态求值）行为一致。

## 6. 函数

```c
func add(a: i64, b: i64) -> i64 {
    return a + b
}

func main() {                 // 程序入口（logic 文件）
    println(add(1, 2))
}
```

- 一等函数：函数可赋值给变量、作为参数传递（后续版本）。
- 方法重载：后续版本。

### 6.1 默认值参数（M2.1 已实现）

函数参数可带默认值，调用时可省略可选参数：

```c
func greet(name: string, prefix: string = "Hello") -> string {
    return prefix + ", " + name
}

func main() {
    println(greet("World"))              // 省略 prefix → 用默认值 "Hello, World"
    println(greet("World", "Hi"))        // 显式传参 → "Hi, World"
}
```

**语法**：`参数名: 类型 = 字面量`（在参数列表中，形如 `prefix: string = "Hello"`）。

**规则**：

- 可选参数**必须连续排在必选参数之后**（一旦出现默认值，其后参数都必须有默认值），
  否则编译报错。
- 默认值限**字面量**：数字 / 布尔 / 字符 / 字符串 / 空表 `[]`（与类字段默认值规则一致）。
  非空表字面量、变量引用等表达式默认值 → 编译报错「默认值必须是字面量」。
- 默认值类型须与形参类型匹配（如 `i64` 形参不能给 `"x"` 字符串默认值）。
- 调用点按实参数**区间检查**：少传（< 必选个数）或多传（> 总个数）都报
  `期望 N 个参数` / `期望 N-M 个参数`（N = 必选参数个数，M = 总参数个数）。
- **方法参数默认值暂不支持**（报「方法默认值参数留待 M3」），仅普通函数可用。

**实现要点**（双路径一致）：

- 函数签名不变（LLVM 含全部形参），缺省实参在**调用点**补齐——默认值无函数体作用域依赖，
  直接按字面量求值即可；
- 解释器（tie-interp）与编译器（IR 层）行为一致：省略可选参数 → 默认值，显式传参 → 覆盖。

**标准库示例**（`ext/log.tie` 综合方案，空表 `[]` 作「未提供」信号）：

```c
func no_file(langs: table, texts: table = []) -> string {
    if len(texts) > 0 {
        return texts[0]        // 方案 A：调用方直接给文本
    }
    return msg_t("error.no_file")   // 方案 B：查字典（当前语言 → zh → 键本身）
}
```

### 6.2 ref 表参数按引用传递（T0.3 已实现）

形参类型前加 `ref` 修饰，声明该表参数**按引用传递**。**仅限表参数**：
非表类型使用 `ref` → 编译期报错。

```c
// fill：函数内对 ref 表参数做内容修改（push / 下标写），调用方变量直接可见。
// replace：函数内重绑定 x = table_new_i64()（换新表），调用方 t 跟随指向新表。
func fill(x: ref table<i64>) {
    table_push(x, 99)
    table_push(x, 100)
    x[0] = 42
}

func replace(x: ref table<i64>) {
    x = table_new_i64()
    table_push(x, 7)
}

func main() {
    var t = table_new_i64()
    table_push(t, 1)
    fill(t)
    println(len(t))             // 3（原 1 + push 99/100）
    println(t[0])               // 42（被 fill 改写）
    replace(t)
    println(len(t))             // 1（重绑定写回：t 现指向新表）
    println(t[0])               // 7
}
```

**语义（真引用）**：函数内对该参数的一切修改都写回调用方实参槽：

- **内容修改写回**：`table_push` 追加、`x[i] = v` 下标写 → 调用方变量直接可见；
- **变量重绑定写回**：`x = table_new_i64()` 等重绑定 → 调用方实参跟随指向新表。

**与值语义（非 ref 表参数）的区别**：

| 行为 | `ref table<T>` | 非 ref 表参数 |
| --- | --- | --- |
| 内容修改（push/下标写） | 写回调用方（真引用） | IR 路径共享同一动态表指针，内容修改仍可见；interp 路径值拷贝隔离 |
| 变量重绑定（`x = ...`） | 写回调用方 | 只改形参局部绑定，调用方不受影响 |

> 注：非 ref 表参数的 IR 路径当前与 ref 共享底层动态表指针（内容修改可见），
> 但重绑定隔离；interp 路径为值拷贝（内容修改也隔离）。需严格内容隔离时
> 显式传值拷贝（见 std 库惯例）。

**调用点约束（编译期报错）**：

- 实参必须是**可寻址的表变量**（动态表）：表字面量 `g([1, 2])`、下标、调用
  结果等无变量槽可写回 → `调用 'g' 的 ref 参数需要可寻址的表变量实参（字面量/下标/调用结果不可取地址）`；
- 实参必须是**动态表变量**（`table_new_*` 创建，定长表变量不满足）→
  `调用 'g' 的 ref 参数实参 't' 必须是动态表变量（table_new_* 创建）`。

**参考验收**：`tests/language/byref_table.tie`（运行输出 3/42/99/100/1/7）、
`tests/language/byref_table_neg.tie`（负例 @6:7）。

### 6.3 extern 函数声明（T0.7 已实现）

顶层声明**外部 C 函数符号**（无函数体），链接期由 clang 解析 libc（Windows
为 msvcrt）符号：

```c
// 顶层裸名声明：extern fn 函数名(参数列表) -> 返回类型;
extern fn system(cmd: string) -> i32;   // libc system：执行命令，返回退出码
extern fn rand() -> i32;                // libc rand：无参数返回 int
```

**语法**：

- `extern fn` 为固定标识符序列（`fn` 是关键字，不可省略或替换）；
- 参数类型 / 返回类型**仅限标量**：`i8`..`u64` / `f32` / `f64` / `bool` /
  `char` / `string` / `void`（返回 `void` 可省略 `-> void`）；
- 声明后即可像普通函数调用（IR 发射 LLVM `declare`，调用点 `call`）；
- extern 声明只能出现在**文件顶层**（函数体内 → 编译期报错）。

**约束（编译期报错）**：

- 参数为表/结构体等容器类型 → `extern 函数 'foo' 的参数 't' 必须是标量类型（i8..u64/f32/f64/bool/char/string），实际是 table<i64>`；
- 返回类型非标量且非 void → 同理报「返回类型必须是标量…或 void」；
- 与已有函数同名重复声明 → `extern 函数 'foo' 与已有函数重复定义`；
- extern 不能带默认值参数 / 不能带 ref 形参；
- **REPL / 解释路径不支持调用 extern**（仅编译路径可用）：REPL 调用 →
  `REPL 不支持调用 extern 函数 'foo'（仅编译路径可用，请用 tie-llvm 编译运行）`。标准库 std/process.tie
  的 `exec_code` / `exec_output` 因此在 REPL 中不可用，需 `tie` 编译运行。

**参考验收**：`tests/language/extern_decl.tie`（运行 rand 随机值/0/3/hello）、
`tests/language/extern_decl_neg.tie`（负例）。

## 7. 模块与数据交换

```c
import "./ui/main.tie" as ui     // 导入其他 tie 文件（按头类型分派）
import "./config.tie" as cfg     // 导入 data 文件 → 类型化为只读数据表
```

- `type tie<data>` 文件可被任何其他文件导入，作为数据交换格式使用。
- 头类型不同，导入后可见的符号集不同（`data` 只导出数据，`logic` 导出函数）。
- **当前实现状态（M2）**：`import` 已实现——导入文件中的函数递归加载、内联可用。
  import 展开逻辑位于 tie-frontend 的 `imports` 模块，编译器（tie-llvm）与语言服务器
  （tie-lsp）共享同一实现；语言服务器的诊断 / hover / 跳转定义 / 补全同样支持跨文件语义。
  `data` 文件导入为只读数据表、按头类型分派可见符号集仍属规划（后续版本）。

### 7.1 单文件命名空间（M2.1.7 已实现）

命名空间成为**真正的模块边界**：命名空间内函数默认**私有**（仅同命名空间可见），
`pub func` 显式导出后跨命名空间 / 跨文件可调；`import` 可用别名重命名入口；
`using` 把已导入命名空间的公有函数引入当前文件，支持裸调用。

```c
// 文件 tools.tie（library）：
namespace fmt {
    pub func public_api() -> string { return helper() + "!" }  // 导出：跨命名空间可调
    func helper() -> string { return "ok" }                     // 私有：仅 fmt 内可见
    namespace inner {
        pub func deep() -> string { return "deep" }             // 嵌套命名空间导出
    }
}

// 文件 main.tie（logic）：
import "./tools.tie" as f2        // 别名：原前缀 fmt 被屏蔽（唯一入口），必须用 f2
using f2.inner;                   // 引入嵌套命名空间，公有函数可裸调用
func main() {
    println(f2.public_api())      // 别名访问 → fmt::public_api
    println(deep())               // using 裸调用 → fmt::inner::deep
    // println(fmt.public_api())  // 错误：fmt 已被别名 f2 取代（唯一入口）
    // println(helper())          // 错误：helper 是私有函数
}
```

规则要点：

- **可见性**：命名空间内函数默认私有；`pub func` 显式导出。同命名空间内裸调用不受限；
  跨命名空间（含 import 展开后的跨文件）调用私有函数 → 编译期报错。顶层函数恒公有。
- **别名唯一入口**：`import "./x.tie" as f2` 后，原命名空间前缀在导入方**不可用**，
  必须用别名访问（避免两个文件同名命名空间冲突）。
- **using 引入**：`using fmt;` / `using fmt.inner;` / `using f2.inner;`（别名 + 子路径），
  目标必须是已 import 引入的命名空间前缀或别名；引入后其公有函数可**裸名调用**。
  多个 using 都含同名函数 → 裸调用歧义报错（改用前缀调用）。
- 裸调用解析顺序：顶层裸名 → 当前命名空间前缀补全 → using 引入的命名空间（唯一候选）。

#### 7.1.1 单文件形式（无花括号）

`namespace foo` 后不加花括号（独占一行，或以 `;` 结尾），表示**从声明处起整份文件
的剩余内容都属于命名空间 foo**：

```c
namespace foo            // 等价于 namespace foo;
func add(a: i64, b: i64) -> i64 { return a + b }
```

- `namespace foo` 换行（ASI 自动补分号）与手写 `namespace foo;` 产生**完全一致的 AST**；
- 单文件模式下的嵌套递归生效：`namespace a` 后出现块式/单文件 `namespace b` → b 是 a 的成员；
- 块式 `namespace foo { ... }` 依旧可用，与单文件形式并存。

### 7.2 标准库：字符串池 intern（T0.6 已实现）与进程原语 process（T0.7）

#### 7.2.1 字符串池 intern（std/intern.tie）

`std/intern.tie` 提供**字符串 → 稳定整数 id** 的登记机制（命名空间 `intern`），
供编译器/符号表把 O(len) 字符串比较降为 O(1) 整数比较：

```c
import "../../std/intern.tie"
using intern;

func main() {
    var a = intern.intern("abc")   // 首次登记 → id 0
    var b = intern.intern("abc")   // 同串 → 同一 id 0
    var c = intern.intern("xyz")   // 新串 → id 1（id 从 0 递增分配）
    println(intern.lookup(a))      // "abc"（id → 原串）
    println(intern.lookup(999))    // ""（未登记 id 返回空串哨兵）
    println(intern.interned_len()) // 2（已登记的不同串数量）
}
```

**接口**（命名空间 `intern`）：

| 函数 | 签名 | 语义 |
| --- | --- | --- |
| `intern` | `intern(s: string) -> i64` | 登记串并返回稳定 id：同串同 id（id 从 0 递增），重复登记返回原 id 不新增条目 |
| `lookup` | `lookup(id: i64) -> string` | id → 原串（O(1) 下标）；未登记 id（负数或越界）返回空串哨兵 `""` |
| `interned_len` | `interned_len() -> i64` | 已登记的不同串数量 |

**实现要点**：三个并行全局动态表（T0.4 全局表特性的首次生产消费）——
`intern_pool`（id → 串，登记序）、`intern_keys` / `intern_vals`（串 → id，
键按 strcmp 升序，正查二分 O(log n)）。池为**模块级全局状态**：跨函数、
跨模块（import 文本内联）调用 id 稳定。

**约定**：`lookup` 对未登记 id 返回空串 `""` 哨兵，与编译器侧 `-1`
「无名字」哨兵是**不同约定**——调用方须先判空串再判 `-1`。

**参考验收**：`tests/language/intern.tie`（全部断言通过）。

#### 7.2.2 进程原语 process（std/process.tie，基于 extern）

`std/process.tie` 用 T0.7 extern 声明**重新实现进程原语**（命名空间
`process`），替代/补充内置 `exec_code` / `exec_output` 的 C ABI 桥——
0-Rust 路径（阶段 4）的关键证明：tie 程序直接声明并调用 libc 函数：

```c
import "../../std/process.tie"
using process;

func main() {
    println(process.exec_code("exit 0"))   // 0（libc system 的退出码）
    println(process.exec_code("exit 3"))   // 3
    println(process.exec_output("echo hello"))  // "hello\n"（重定向 + 文件读回）
}
```

**接口**（命名空间 `process`）：

| 函数 | 签名 | 语义 |
| --- | --- | --- |
| `exec_code` | `exec_code(cmd: string) -> i32` | 执行命令并返回退出码（包装 extern `system`） |
| `exec_output` | `exec_output(cmd: string) -> string` | 执行命令并返回全部输出（stdout+stderr 合并；重定向临时文件 → `file_read` → 删除） |

**要点**：

- `system` 的 C 原型是 `int system(const char*)`，用 **`i32`** 声明精确匹配
  （tie 无 i32→i64 隐式转换，i64 读负退出码有 RAX 高位风险）；
- `exec_output` 不用 popen（tie 无指针类型，无法处理 FILE*/char*），用
  `cmd > tmpfile 2>&1` 重定向 + 文件读回等价实现；
- 二者均依赖 extern 调用，**REPL 中不可用**（仅编译路径，见 §6.3）。

**参考验收**：`tests/language/extern_decl.tie`（输出 rand 随机值/0/3/hello）。

#### 7.2.3 文件系统原语（file_* 内置，UTF-8 路径安全）

文件系统内置原语 `file_read` / `file_write` / `file_append` / `file_exists` /
`file_delete` / `file_size` / `file_is_dir` / `file_is_file`（M2 起；**2026-08-14 起
全部迁移 UTF-8 安全桥**——Rust `std::fs` 实现、Windows 宽字符 API，中文/Unicode 路径
原生支持。曾用 libc `fopen` / `remove`，按 ANSI 代码页（GBK）误读 UTF-8 字节，
中文路径的存在性检查/写入/删除必失败）。

**接口**（语言底座原语）：

| 原语 | 签名 | 语义 |
| --- | --- | --- |
| `file_read` | `file_read(path: string) -> string` | 读取文本全文；失败返回空串 |
| `file_write` | `file_write(path: string, content: string) -> bool` | 覆盖写入 |
| `file_append` | `file_append(path: string, content: string) -> bool` | 追加写入（文件不存在则创建） |
| `file_exists` | `file_exists(path: string) -> bool` | 存在性检查（可读探测，目录/不可读 → false） |
| `file_delete` | `file_delete(path: string) -> bool` | 删除文件（不存在 → false） |
| `file_size` | `file_size(path: string) -> i64` | 字节大小；失败（不存在等）返回 -1 |
| `file_is_dir` | `file_is_dir(path: string) -> bool` | 路径是否为目录 |
| `file_is_file` | `file_is_file(path: string) -> bool` | 路径是否为普通文件 |

**完整封装**：`std/fs`（命名空间 `fs`，Rust std::fs 风格 API）：读取
`read_to_string` / `read_text` / `read_bytes` / `read_lines`，写入 `write` /
`write_text` / `append` / `append_text` / `write_lines`，元数据 `exists` /
`is_file` / `is_dir` / `size`，删除 `remove_file` / `delete` / `remove_dir_all` /
`remove_all`，目录 `create_dir_all` / `mkdir_all` / `read_dir` / `list` / `walk` /
`copy_dir`，复制移动 `copy` / `rename` / `move`，归档 `untar_gz` / `unzip`。
全部基于上述 UTF-8 桥，中文路径安全。

## 8. 数据结构与逻辑分离（struct / 命名空间函数 / 继承）

**M2.1.8**：`class` 改名为 `struct` 并成为**纯数据**（只含字段）；逻辑（方法）移出为
**绑定 struct 名的命名空间函数**（`namespace Point { pub func dist(p: Point) }`）。
`obj.method()` 调用由编译器**转发**为命名空间函数（首参 = 接收者，按引用传递）。
`this`/`static` 关键字随之废弃（`this` 变普通标识符，接收者改为显式首参）。

### 8.1 struct 定义与字段

```c
struct Point {
    var x: i64 = 0        // 字段：var name[: Ty] [= 默认值]；默认值可省略（缺省为类型零值）
    var y: i64 = 0
}

// 逻辑 = 绑定 struct 名的命名空间函数（方法）：
namespace Point {
    pub func dist(p: Point) -> i64 {          // 实例方法：首参 = 接收者（按引用）
        return p.x * p.x + p.y * p.y
    }
    pub func origin() -> Point {              // 静态风格：无接收者，struct 名调用
        return Point(0, 0)
    }
}
```

- struct 体**只允许字段**；方法语法出现在 struct 体内 → 报错并提示用命名空间函数定义。
- 字段类型：显式标注（`var x: i64`）优先；否则从默认值字面量推导；两者皆无 → 编译报错。
- 字段访问：`obj.x` 读（`GEP + load`）、`obj.x = 值` 写（`GEP + store`）。

### 8.2 构造

`Struct名(实参…)` 是**构造表达式**（值类型，非 `new` 引用）：

```c
var p = Point(3, 4)        // 按字段声明顺序传参
var q = Point()            // 全部用默认值
var r = Point(1)           // 部分实参：缺省字段用默认值
```

### 8.3 方法调用（转发）

- 实例方法：`obj.method(...)` → 编译器转发为 `命名空间函数(obj, ...)`——
  接收者作为**首参按引用传递**（LLVM ptr），函数内字段修改反映到调用方；
- 静态风格：`Point.origin(...)`（receiver 是 struct 名）→ 直接调用，无接收者实参；
- 方法函数必须 `pub`（与普通命名空间函数一致，私有则转发被拦截）；
- 方法函数首参类型 == struct 名即视为「实例方法」（引用传递）；否则为普通参数。

### 8.4 继承（字段复用）

```c
struct Animal {
    var name: string
}
struct Dog extends Animal {          // 字段拍平：父 struct 字段在前
    var breed: string
}
namespace Animal {
    pub func sound(a: Animal) -> string { return "..." }
}
namespace Dog {
    pub func sound(d: Dog) -> string { return "Woof" }   // 遮蔽父 struct 同名方法
}
```

- 布局：子 struct 实例 = 父 struct 字段 + 自身字段（拍平，无嵌套指针）。
- 方法解析：`obj.method()` 沿继承链查找（子 → 父）；子类同名遮蔽父类；
  子实例调用父类方法时接收者地址直接可用（字段布局前缀一致）。
- **限制**：无向上转型（子类不能当父类用）、无虚方法/动态分派、字段名必须跨继承链唯一（否则报错）、继承不得成环（否则报错）。

### 8.5 语义限制（编译期报错）

- struct 定义仅允许出现在**文件顶层**（函数体内定义 → 语法错误）。
- struct 实例字段访问/方法调用要求对象**可寻址**：`Point(0).x`、`make().dist()` 报错
  （寄存器中的 struct 值无内存地址；请先存入变量再访问）。
- 方法函数必须 `pub` 才可被 `obj.method()` 转发；无接收者方法经实例调用 → 参数个数报错。
- struct 名与函数名冲突、struct 重复定义 → 报错。
- 本期不支持：对象比较（`==`）、`println` 直接打印对象、方法重载、析构。

### 8.6 与元组的关系

- 二者都是**字面结构体值类型**（LLVM `{...}`）；元组字段可匿名/数字/命名访问，struct 字段仅命名访问。
- 元组字段访问对寄存器值开放（`divmod(9,2).q` 合法）；struct 字段访问要求可寻址对象。

## 9. 语法速查表

### 9.1 所有关键词

| 关键词         | 用途                       | 示例                           |
| ----------- | ------------------------ | ---------------------------- |
| `func`      | 函数定义                     | `func main() { }`            |
| `var`       | 可变变量声明                   | `var x = 1`                  |
| `const`     | 不可变变量声明                  | `const s = "hi"`             |
| `if`        | 条件分支                     | `if a > b { }`               |
| `else`      | 否则分支（可连 `if`）            | `else { }` / `else if x { }` |
| `while`     | 循环                       | `while i < 10 { }`           |
| `for`       | 遍历（范围/集合）                | `for i in 0..10 { }`         |
| `in`        | `for` 的遍历对象              | `for item in arr { }`        |
| `return`    | 函数返回                     | `return a + b`               |
| `break`     | 退出循环（E1；可带标签）          | `break` / `break outer`      |
| `continue`  | 跳循环下一次迭代（E1；可带标签）     | `continue` / `continue outer` |
| `switch`    | 多分支（M1 已实现）              | `switch n { case 1: }`       |
| `case`      | switch 分支（值/区间/类型匹配，可多值） | `case 1, 2:` / `case 3..7:`  |
| `default`   | switch 默认分支（可省略）         | `default:`                   |
| `when`      | switch 守卫条件（模式匹配增强）      | `case 8 when flag:`          |
| `import`    | 导入其他 tie 文件（M2 已实现）      | `import "./x.tie" as x`      |
| `as`        | 导入别名（M2.1.7 起为唯一入口）      | `import "./x.tie" as x`      |
| `namespace` | 命名空间声明（M2 已实现）           | `namespace tcmsg { }` / `namespace foo`（单文件，包裹文件剩余内容） |
| `pub`       | 公有可见性标记（M2.1.7 已实现）      | `pub func public_api()`      |
| `using`     | 引入命名空间公有函数（M2.1.7 已实现）   | `using fmt;`                 |
| `struct`    | 数据结构定义（纯数据，M2.1.8）       | `struct Point { }`           |
| `extends`   | 继承父 struct（P8/M2.1.8）     | `struct Dog extends Animal`  |
| `ref`       | 表形参按引用传递（T0.3 已实现）       | `func f(x: ref table<i64>)`  |
| `extern`    | 外部 C 函数声明（T0.7 已实现）       | `extern fn system(c: string) -> i32;` |
| `true`      | 布尔真字面量                   | `var b = true`               |
| `false`     | 布尔假字面量                   | `var b = false`              |

> `let`/`fn` 为早期名称，已分别由 `var`/`const` 与 `func` 取代；`class`/`static`/`this`
> 已随 M2.1.8 废弃（`struct` 取代 `class`，接收者改为显式首参），均不再作为关键字。

### 9.2 所有类型

| 类型         | 类别    | 说明                       | 对应 LLVM 类型              |
| ---------- | ----- | ------------------------ | ----------------------- |
| `i8`       | 基本类型  | 有符号 8 位整数                | `i8`                    |
| `i16`      | 基本类型  | 有符号 16 位整数               | `i16`                   |
| `i32`      | 基本类型  | 有符号 32 位整数               | `i32`                   |
| `i64`      | 基本类型  | 有符号 64 位整数               | `i64`                   |
| `u8`       | 基本类型  | 无符号 8 位整数                | `i8`                    |
| `u16`      | 基本类型  | 无符号 16 位整数               | `i16`                   |
| `u32`      | 基本类型  | 无符号 32 位整数               | `i32`                   |
| `u64`      | 基本类型  | 无符号 64 位整数               | `i64`                   |
| `f32`      | 基本类型  | 单精度浮点                    | `float`                 |
| `f64`      | 基本类型  | 双精度浮点                    | `double`                |
| `bool`     | 基本类型  | 布尔                       | `i1`                    |
| `char`     | 基本类型  | 单字符（UTF-32）              | `i32`                   |
| `string`   | 基本类型  | 字符串（不可变）                 | `ptr`                   |
| `void`     | 基本类型  | 无返回（仅函数）                 | `void`                  |
| `code`     | 编译期类型 | 代码片段（展开为 AST 子程序）        | —（编译期）                  |
| `num`      | 宽类型   | 全部数类型（整数 + 浮点）           | —（展开为具体类型）              |
| `text`     | 宽类型   | `string` 与 `char`        | —（展开为具体类型）              |
| `misc`     | 宽类型   | 其余全部类型（bool/void/code 等） | —（展开为具体类型）              |
| `table`    | 复合类型  | 表（数组 + 高级数组），元素同构        | 单行纯位置表已实现；字符串 id 表即 map（§3.4.1）；二维表 M3 |
| `map`      | 复合类型  | 键值表（string → value，E3；排序键二分 T0.5） | 键恒 strcmp 字节序有序，查找/插入二分 |
| `(T1, T2)` | 复合类型  | 元组（固定长度、元素可异构，可命名）       | 字面结构体 `{T1, T2}`        |
| `类名`       | 复合类型  | 类实例（值类型对象，P8）            | 字面结构体 `{字段…}`           |

> 宽类型与 `code`/`table` 均为编译期概念：语义分析阶段展开为具体类型或校验归类，
> IR 生成阶段不出现。表字面量 `[col, col; row]` 语法见 §3.3；元组语法见 §3.4；类见 §8。

### 9.3 所有符号

| 符号                          | 名称/用途     | 说明                                               |
| --------------------------- | --------- | ------------------------------------------------ |
| `(` `)`                     | 圆括号       | 分组、函数调用、参数列表                                     |
| `{` `}`                     | 花括号       | 块作用域（函数体、分支、循环体）                                 |
| `[` `]`                     | 方括号       | 表字面量、下标访问                                        |
| `,`                         | 逗号        | 分隔参数、表列                                          |
| `;`                         | 分号        | 显式分隔语句（行尾可省略，见 §4）                               |
| `:`                         | 冒号        | 类型标注 `x: i64`、表 id 与值 `"a":1`                    |
| `.`                         | 点         | 元组/类字段访问 `t.x` / `t.Item1` / `t.0` / `obj.field` |
| `..`                        | 范围        | `0..10`（for 遍历）；`case 3..7:`（switch 区间匹配，左闭右开）   |
| `->`                        | 箭头        | 函数返回类型 `-> i64`                                  |
| `+`                         | 加         | 整数/浮点加法                                          |
| `-`                         | 减（或一元负号）  | 整数/浮点减法、取负                                       |
| `*`                         | 乘         | 整数/浮点乘法                                          |
| `/`                         | 除         | 整数/浮点除法                                          |
| `%`                         | 取模        | 仅整数                                              |
| `=`                         | 赋值        | `x = 1`                                          |
| `+=` `-=` `*=` `/=` `%=`    | 复合赋值（算术）  | `x += 1` 等价 `x = x + 1`；字符串仅支持 `+=`（拼接）          |
| `&=` `\|=` `^=` `<<=` `>>=` | 复合赋值（位运算） | `x &= 1` 等价 `x = x & 1`；仅整数                      |
| `&`                         | 按位与       | 仅整数                                              |
| `\|`                        | 按位或       | 仅整数                                              |
| `^`                         | 按位异或      | 仅整数                                              |
| `<<`                        | 左移        | 仅整数                                              |
| `>>`                        | 右移        | 仅整数（有符号算术右移 / 无符号逻辑右移）                           |
| `? :`                       | 三目        | `c ? a : b`，右结合，条件必须为 `bool`，两分支类型一致，短路求值        |
| `++`                        | 自增（一元）    | 前缀返回新值、后缀返回旧值；操作数必须为可写数字变量（`x++` / `++x`）        |
| `--`                        | 自减（一元）    | 前缀返回新值、后缀返回旧值；操作数必须为可写数字变量（`x--` / `--x`）        |
| `==`                        | 相等        | 比较，返回 `bool`                                     |
| `!=`                        | 不等        | 比较，返回 `bool`                                     |
| `<`                         | 小于        | 比较，返回 `bool`                                     |
| `>`                         | 大于        | 比较，返回 `bool`                                     |
| `<=`                        | 小于等于      | 比较，返回 `bool`                                     |
| `>=`                        | 大于等于      | 比较，返回 `bool`                                     |
| `&&`                        | 逻辑与       | 两侧必须为 `bool`                                     |
| `\|\|`                      | 逻辑或       | 两侧必须为 `bool`                                     |
| `!`                         | 逻辑非（一元）   | 操作数必须为 `bool`                                    |
| `//`                        | 行注释       | 行内 `//` 及之后内容忽略                                    |
| `/*` `*/`                   | 块注释       | 跨行注释                                             |
| `"..."`                     | 字符串字面量    | 支持转义 `\n` `\t` `\\` `\"` `\'` `\0`               |
| `'...'`                     | 字符字面量     | 单字符                                              |
| 数字字面量                       | 整数/浮点     | `42`（i64）、`3.14`（f64）、支持指数 `1.5e-3`              |
| 标识符                         | 变量/函数名    | `[A-Za-z_][A-Za-z0-9_]*`                         |
