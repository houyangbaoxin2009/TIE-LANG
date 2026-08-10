# tie 语言 AI 教学指南

> 本文件专为 **AI 助手（LLM）** 编写：目标是"粘贴即用"的完整语言说明书，
> 覆盖语法、语义、已实现/未实现边界与编译器架构。你（AI）应严格按本文件 +
> [language.md](language.md) 工作，不得使用本文件未列出的特性（很可能未实现）。
>
> 更新于 2026-08-10（自举 v2 阶段 0：ref 表参数 / 全局表 / map 排序 / intern / extern 完成后）。

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

## 2. 已实现特性清单（截至 2026-08-10，含自举 v2 阶段 0）

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
    case 1, 2:                             // 多值：任一相等即命中
        println("one or two")
    case 3..7:                             // 区间：3 ≤ n < 7（含 3 不含 7，仅整数/字符）
        println("three to six")
    case 8 when flag:                      // 守卫：值相等 且 flag 为真才进入
        println("eight and flag")
    case string:                           // 类型匹配：subject 为动态类型容器时才允许
        println("a string")
    default:
        println("other")
}
return expr                                // 返回值
```

switch 的 `case` 支持整数、字符（`'a'`）、布尔、负数、字符串；`default` 可省略。

**模式匹配增强**（`case 值[, 值]... [when 条件]:`）：
- **多值**：逗号分隔多个值（或区间），任一匹配即命中；
- **区间**：`case 3..7:` 整数/字符区间，左闭右开（与 `for i in 0..10` 一致）；浮点区间不支持；
- **守卫**：`case 8 when flag:` 值匹配 **且** 守卫为真才进入，守卫不满足落入下一个 case；
- **类型匹配**：`case string:` / `case i64:` 按 subject 的动态类型匹配，仅宽类型/动态容器对象（表、元组）上有意义——普通静态类型变量上语义层报错（类型恒定，恒真/恒假无意义）。

### 2.4 函数

```c
func add(a: i64, b: i64) -> i64 { return a + b }
func main() { println(add(1, 2)) }         // 入口
```

- 返回类型 `-> Ty` 可省略（默认 `void`）。
- **多值返回**用元组（见 2.6）。
- **默认值参数**（M2.1 已实现）：`func greet(name: string, prefix: string = "Hello")`，
  调用时可省略可选参数（必须连续排在必选参数之后）。
- **未实现**：一等函数、重载、函数体内嵌套函数。

### 2.4.1 ref 表参数按引用传递（T0.3 已实现）

形参类型前加 `ref`，表参数**按引用传递**（仅限表，非表类型报错）：

```c
func fill(x: ref table<i64>) {
    table_push(x, 99)
    x[0] = 42
}
func replace(x: ref table<i64>) {
    x = table_new_i64()        // 重绑定：调用方实参跟随指向新表
    table_push(x, 7)
}
func main() {
    var t = table_new_i64()
    table_push(t, 1)
    fill(t)                    // 内容修改写回：len=3，t[0]=42
    println(len(t))            // 3
    println(t[0])              // 42
    replace(t)                 // 重绑定写回：t 现指向含 [7] 的新表
    println(len(t))            // 1
    println(t[0])              // 7
}
```

**规则**：

- 内容修改（push/下标写）与变量重绑定（`x = ...`）都**写回调用方实参槽**；
- 实参必须是**可寻址的动态表变量**：表字面量 `g([1, 2])`、下标、调用结果 →
  `调用 'g' 的 ref 参数需要可寻址的表变量实参（字面量/下标/调用结果不可取地址）`；
- 非 ref 表参数保持值语义（重绑定隔离；interp 路径内容修改也隔离）。

### 2.4.2 extern 函数声明（T0.7 已实现）

顶层声明外部 C 函数符号（无函数体），编译路径链接 clang 解析 libc（msvcrt）：

```c
extern fn system(cmd: string) -> i32;   // libc system
extern fn rand() -> i32;                // libc rand

func main() {
    println(rand())                     // [0, 32767] 随机值
    println(system("exit 0"))           // 0
}
```

**规则**：

- `extern fn` 固定标识符；参数/返回仅标量（i8..u64/f32/f64/bool/char/string/void）；
- 声明只能出现在**文件顶层**（函数体内 → 编译期报错）；
- 表/结构体参数 → `extern 函数 'foo' 的参数 't' 必须是标量类型…实际是 table<i64>`；
- **REPL / 解释路径不支持调用 extern**（仅编译路径）：`REPL 不支持调用 extern 函数 'foo'（仅编译路径可用，请用 tie-llvm 编译运行）`。

### 2.5 表 table（数组）与键值表 map

```c
var arr: table = [1, 2, 3]         // 单行 3 列（无 id）
var arr2: table = [0:1, 1:2, 2:3] // 显式数字 id（= 下标）
var e = arr[1]                     // 下标访问（已实现）
for item in arr { }                // 遍历（已实现）
```

> **注意**：上方表字面量示例中，字符串 id 表（`["a":1]`）即**键值表 map**，
> 已随 E3 + T0.5 落地可用（见 2.5.1）；二维表（`[1,2;3,4]`）语法可解析，
> 但语义阶段报"留待 M3"，**不要使用**。

### 2.5.1 键值表 map / map\<T\>（E3 + T0.5 排序键二分已实现）

```c
var m: map = ["a":1, "b":2]      // 键值表：string -> i64
var n: map<string> = ["x":"hi"]  // 显式值类型（map 默认值类型 i64）
var v = m["a"]                   // 下标读：1
m["c"] = 3                       // 下标写：不存在则插入，存在则覆盖
var k = len(m)                   // 条目数：3
```

**排序键二分（T0.5）**：map 键恒按 **strcmp 字节序**有序存储，查找/插入二分
（零分配），10k 次查找由线性 ≈ 6.27s 降至 ≈ 2.7ms（~2295×，见
`scripts/bench/map-bench.tie`）。

**行为契约**：map 输出/打印**按键排序**（`{a: 1, m: 2, z: 3}`），
**不依赖插入序**。依赖插入序的写法不可用。

**约束**：键恒为字符串（必须加双引号）；值类型全表一致；**map 不能作全局变量**
（语法层即拒绝，见 2.5.2）。

### 2.5.2 顶层表全局变量（T0.4 已实现）

顶层 `var g: table<T>;`（无初始化器 = 默认空动态表；也可 `= []`）：

```c
var g: table<i64>;               // 顶层全局表：跨函数持久，main 入口运行时创建

func add(x: i64) {
    table_push(g, x)
}
func main() {
    add(1)
    add(2)
    println(len(g))              // 2（跨函数累加）
    println(g[0])                // 1（main 直接下标读取）
    println(g[1])                // 2
}
```

**规则**：

- 跨函数持久：push / 下标写 / len / for 遍历均可直接使用；
- **可作 ref 实参**（§2.4.1）；元素类型可标注（`table<i64>`），省略约定 `string`；
- **仅 `table<T>` 支持全局**；**map 不能作全局**（语法层拒绝）；
- **const 全局表暂不支持**：`const g: table<i64>;` → `const 全局表暂不支持：'g'（表在 main 入口运行时创建，无法静态初始化）`；
- 初始化器只能为空表 `[]`（或省略）；非表类型全局变量限标量。

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

### 2.7 import（多文件）与单文件命名空间（M2.1.7）

```c
import "./lib_math.tie" as math   // 导入其他 tie 文件（相对路径字符串）
using math;                       // 引入命名空间：其公有函数可裸调用
```

- **已实现**（M2）：导入文件中的函数递归加载、内联可用；import 展开逻辑集中在
  tie-frontend 的 `imports` 模块（tie-llvm 与 tie-lsp 共享），语言服务器的
  诊断 / hover / 跳转定义 / 补全同样支持跨文件语义（`str.split` 等跨文件
  命名空间调用不会误报未声明变量）。
- **已实现**（M2.1.7，单文件命名空间 = 真模块边界）：
  - 命名空间内函数默认**私有**（仅同命名空间可见），`pub func` 显式导出；
    顶层函数恒公有；私有函数跨命名空间调用 → 编译期报错；
  - `import "x.tie" as f2`：别名**唯一入口**——原命名空间前缀被屏蔽，必须用别名访问；
  - `using fmt;` / `using f2.inner;`：引入已导入命名空间的公有函数，可裸名调用；
    多 using 同名函数 → 裸调用歧义报错。
- **未实现**：`data` 文件导入为只读数据表、按角色分派可见符号集。

### 2.7.1 字符串池 intern（T0.6 已实现，std/intern.tie）

字符串 → 稳定整数 id 的登记机制（命名空间 `intern`），编译器符号表把
O(len) 字符串比较降为 O(1) 整数比较：

```c
import "../../std/intern.tie"
using intern;

func main() {
    var a = intern.intern("abc")   // 首次登记 → id 0
    var b = intern.intern("abc")   // 同串 → 同一 id 0
    var c = intern.intern("xyz")   // 新串 → id 1（id 从 0 递增）
    println(intern.lookup(a))      // "abc"
    println(intern.lookup(999))    // ""（未登记 id 返回空串哨兵）
    println(intern.interned_len()) // 2
}
```

**接口**：`intern(s)->i64`（登记返回稳定 id，同串同 id）、
`lookup(id)->string`（缺失返回空串哨兵）、`interned_len()->i64`。
池为模块级全局状态，跨函数/跨模块 id 稳定。

### 2.7.2 进程原语 process（T0.7 已实现，std/process.tie）

用 extern 声明（链接期 libc 符号）实现的进程原语（命名空间 `process`），
0-Rust 路径的关键证明——tie 直接声明并调用 libc 函数：

```c
import "../../std/process.tie"
using process;

func main() {
    println(process.exec_code("exit 0"))        // 0（libc system 退出码）
    println(process.exec_code("exit 3"))        // 3
    println(process.exec_output("echo hello"))  // "hello\n"（重定向 + 文件读回）
}
```

**接口**：`exec_code(cmd)->i32`（包装 extern `system`，返回退出码）、
`exec_output(cmd)->string`（stdout+stderr 合并捕获，临时文件重定向读回）。
依赖 extern 调用，**REPL 中不可用**（仅编译路径）。

### 2.8 struct 数据与逻辑分离（M2.1.8）

`struct` = **纯数据**（只含字段，值类型，LLVM 内联布局）；逻辑（方法）移出为
**绑定 struct 名的命名空间函数**。`obj.method()` 由编译器转发为命名空间函数
（接收者作首参，按**引用**传递——函数内字段修改反映到调用方）。
`this`/`static`/`class` 已废弃。

```c
struct Point {
    var x: i64 = 0                // 字段：var name[: Ty] [= 默认值]
    var y: i64 = 0
}
namespace Point {
    pub func dist(p: Point) -> i64 {   // 实例方法：首参 = 接收者（按引用）
        return p.x * p.x + p.y * p.y
    }
    pub func origin() -> Point {       // 静态风格：无接收者，struct 名调用
        return Point(0, 0)
    }
}

func main() {
    var p = Point(3, 4)           // 构造表达式（按字段声明顺序传参）
    var q = Point()               // 全部用默认值
    var r = Point(1)              // 部分实参：缺省字段用默认值
    p.x = 5                       // 字段直写
    println(p.dist())             // 实例方法转发 → Point::dist(&p)（25）
    var o = Point.origin()        // 静态风格调用：先存变量（寄存器 struct 值不可直接 .x）
    println(o.x)                  // 0
}
```

> **关键限制**：`Point.origin().x` / `make().dist()` 直接连用会报「需要可寻址」——
> 寄存器中的 struct 值无内存地址，必须先 `var o = ...` 存入变量再访问。
> 方法函数必须 `pub`（否则 `obj.method()` 转发被私有拦截）。

**继承**（字段复用，无虚表/无动态分派/无向上转型）：

```c
struct Animal {
    var name: string
}
struct Dog extends Animal {
    var breed: string
}
namespace Animal {
    pub func sound(a: Animal) -> string { return "..." }
}
namespace Dog {
    pub func sound(d: Dog) -> string { return "Woof" }   // 遮蔽父 struct 方法
}
// Dog 实例布局 = Animal 字段（在前） + 自身字段（拍平）
// obj.method() 沿继承链查找（子 → 父）；子实例调父方法时接收者地址直接可用
```

> 注意：文档早期用过 `str`，**当前类型名是 `string`**（`var name: string`）。

## 3. 编译期会报错的写法（负例）

以下代码**都会在编译期报错**，AI 不要生成：

| 场景 | 错误信息（关键词） |
|---|---|
| `Counter(0).count` / `make().get()`（寄存器中的 struct 值直接访问） | 「字段访问需要可寻址对象」/「方法调用的对象需要可寻址的 struct 实例」 |
| 方法函数未加 `pub` 被 `obj.method()` 调用 | 「私有函数…不可在命名空间之外调用」 |
| 无接收者方法经实例调用 `c.make()` | 「含接收者对象」 |
| 继承环 `struct A extends B` 且 B extends A | 「struct 继承形成环」 |
| 子 struct 字段与父 struct 字段重名 | 「字段名必须跨继承链唯一」 |
| struct 体内定义方法（M2.1.8 后） | 「struct 体不允许方法定义：逻辑请用命名空间函数」 |
| 字段无类型标注且无默认值 | 「字段必须标注类型或有默认值」 |
| 字段默认值是非字面量表达式 | 「默认值必须是字面量」 |
| struct 名与函数名冲突 / struct 重复定义 | 「struct 名与函数名冲突」/「struct 重复定义」 |
| 函数体内定义 struct / import / 嵌套函数 | 「顶层只允许…」/「函数体内不支持…」/「函数体内不支持嵌套函数定义」 |
| const 变量重新赋值 | 「不可变变量不能赋值」 |
| 类型不匹配（如 i64 赋给标注 i32 的变量） | 「类型不匹配」 |
| 元组空解构 `var () = ...` | 「空解构 () 不支持」 |
| 表初始化非表字面量 | 「初始化必须是表字面量」 |
| string 与 i64 直接拼接 | 类型不匹配错误 |
| ref 表参数实参非可寻址（字面量/下标/调用结果） | 「调用 'g' 的 ref 参数需要可寻址的表变量实参（字面量/下标/调用结果不可取地址）」 |
| ref 表参数实参非动态表（定长表变量） | 「调用 'g' 的 ref 参数实参 't' 必须是动态表变量（table_new_* 创建）」 |
| ref 修饰非表类型参数 | 「ref 只能用于表参数」类错误 |
| const 全局表 `const g: table<i64>;` | 「const 全局表暂不支持：'g'（表在 main 入口运行时创建，无法静态初始化）」 |
| 全局表初始化非空表字面量 | 「全局变量 'g' 的初始化器必须是空表 []」 |
| 顶层 map 全局变量 `var m: map;` | 语法层即拒绝 |
| extern 参数/返回为表或结构体 | 「extern 函数 'foo' 的参数 't' 必须是标量类型…实际是 table<i64>」 |
| extern 声明出现在函数体内 | 「extern 声明只能出现在文件顶层」 |
| extern 与已有函数同名重复声明 | 「extern 函数 'foo' 与已有函数重复定义」 |
| REPL 中调用 extern 声明的函数 | 「REPL 不支持调用 extern 函数 'foo'（仅编译路径可用，请用 tie-llvm 编译运行）」 |

## 4. 未实现 / 不要使用的特性

- `ui` / `db` 文件角色（M4 规划）
- 二维表（`[1,2;3,4]`）**运行时**（语法能解析但语义报错）；字符串 id 表（=map）已实现可用
- `data` 文件的 import 数据表化
- 库编译（`library` 角色声明可用，但编译为库的流程未完成）
- `--target` 交叉编译、`--backend=gnu`
- 一等函数、函数重载
- 类：对象比较（`==`）、`println` 打印对象、方法重载、析构
- 裸代码块（函数体内的 `{ }`）、空元组 `()`
- **map 全局变量**（顶层 `var m: map;` 语法层拒绝）
- **const 全局表**（`const g: table<i64>;` 报「const 全局表暂不支持」）
- **extern 在 REPL/解释路径**（仅编译路径可用；含 std/process 的 exec_code/exec_output）

## 5. 编写 tie 代码的硬性规则

1. **类、import、func、extern 声明只出现在文件顶层**；函数体内只有语句。
2. **行尾分号可省略**（ASI 自动补全）；同一行多条语句必须用 `;` 分隔。
3. 类实例要访问字段/调方法，**必须先存入变量**（`var p = Point(0); p.x`），不能 `Point(0).x`。
4. 继承字段跨链唯一；方法遮蔽允许（子类同名覆盖父类）。
5. `string` 字面量加双引号；`char` 单引号。
6. `logic` 文件必须含 `func main()`。
7. 所有语句（变量声明、赋值、表达式、return）以换行或 `;` 结束。
8. **ref 表参数**（§2.4.1）：只用于表形参；实参必须是可寻址的动态表变量。
9. **extern 声明**（§2.4.2）：限标量类型；声明后仅编译路径可调用（REPL 不可用）。
10. **全局表**（§2.5.2）：仅 `table<T>`、无初始化器或 `= []`；map 不能全局、const 暂不支持。

## 6. 完整可运行示例（可直接粘贴验证）

```c
// tie:logic
class Animal {
    var name: string
    func sound() -> string {
        return "..."
    }
}
class Dog extends Animal {
    var breed: string
    func sound() -> string {
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
├── tie-interp/    解释执行：AST 树遍历求值 + eval/eval_call（tie:script 模块协议）+ C ABI 桥（staticlib），REPL 自举核心
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
- 关键函数：`parse_fn_def`、`parse_class`（含 `extends`）、`parse_method`（类内 `func` 定义，
  含 `static`）、`parse_var_decl`（含元组解构 desugar 为临时变量 + 字段访问）、`parse_expr_or_assign`
  （`Ident = ...` → Assign；`obj.field = ...` → FieldAssign）。
- `is_addressable_base`：判断表达式是否可寻址（Var 或 FieldAccess 链）。

### 8.3 AST（ast.rs）

```rust
pub enum TypeSpec { Named(TyKw), Tuple(Vec<TupleField>), Struct(String) }
pub enum Stmt { VarDecl, FnDef, Expr, Assign, Return, If, While, For, Switch, Import, Namespace, Using, Struct, FieldAssign }
pub enum Expr { IntLit, FloatLit, StrLit, CharLit, BoolLit, Var, Call, Unary, Binary, Range,
                TableLit, Index, TupleLit, FieldAccess, MethodCall }
pub struct StructDefStmt { name: String, parent: Option<String>, fields: Vec<ClassField>, span: Span }
pub struct FieldAssignStmt { base: Box<Expr>, field: String, value: Expr, span: Span }
```

### 8.4 语义（semantic.rs）

入口 `analyze(&Program) -> Result<SemanticResult, SemanticError>`，共 **3 遍**：

1. **收集函数签名**：`funcs: HashMap<全名, FuncSig>`（顶层裸名 / 命名空间全名
   `Point::dist`），重复定义报错（允许前向引用）。
2. **collect_structs**：struct 名登记（vs 函数名/struct 名冲突）→ 逐个 `flatten_struct`
   拍平继承链（父 struct 字段在前，`chain: HashSet` 检测继承环）。方法不在 struct 内。
3. **check_fn**：逐函数体 `check_stmt`（作用域为 `HashMap<名称, TypeSpec>`）；
   命名空间函数由 check_ns_stmts 递归覆盖。

关键数据结构：

```rust
pub struct ClassInfo {
    pub parent: Option<String>,
    pub fields: Vec<ClassField>,                 // 拍平字段（含继承），顺序即 LLVM 结构体字段序
    pub field_index: HashMap<String, usize>,     // 字段名 → GEP 偏移（语义与 IR 共用唯一权威）
}
```

- `resolve_class_field_ty`：字段类型解析——显式标注优先；无标注从默认值字面量推导；
  两者皆无 → 报错。
- **方法转发**（`infer_expr` 的 MethodCall 分支）：receiver 类型是 struct →
  沿继承链查 `funcs` 键 `T::method`（子 → 父），方法函数必须 `pub`；首参类型须
  struct 兼容（`struct_assignable`：子类可赋父类）；receiver 必须可寻址（引用传递）。
- `is_addressable_expr`（语义层）：struct 字段访问/方法转发要求对象可寻址
  （Var 或 FieldAccess 链）；寄存器中的 struct 值（构造表达式/方法调用结果）→ 报错。
- `expr_types: HashMap<usize, TypeSpec>`：用表达式地址（`addr_of`）记录推导类型，IR 层查询。
- `tables: HashMap<usize, TableInfo>`：表字面量布局元数据（元素类型/长度）。

## 9. 中端+后端（tie-llvm / ir.rs）

入口 `run(program, semantic_result)`（`IrGenerator`）。

### 9.1 核心设计

- **值类型用字面结构体**：元组/struct → LLVM `{T0, T1, ...}`，通过 `ty_cache` 缓存
  （`HashMap<TypeSpec, String>` 生成 `%tup.N` / `%cls.N` 类型名），避免重复声明。
- **方法 mangle**：方法 = 命名空间函数（`namespace Point`），全名转 `$`
  （`Point::dist` → `@Point$dist`），由 `gen_ns_fns` 统一生成。**方法函数首参**
  （类型 == struct 名）按**引用**传递：签名首参为 `ptr`，用 `by_ptr` 的 `VarBind`
  绑定（不 alloca，直接复用入参指针）——函数内字段修改反映到调用方。
- **方法转发调用**（`gen_call_inner` 的 `first` 参数）：实例 `p.dist()` 时 receiver
  作首实参，`is_method_fn` 判定后传**地址**（`gen_class_addr`）；静态 `Point.dist()`
  走命名空间分支（无接收者实参）。
- **字段读**：`GEP + load`；**字段写**：`GEP + store`（`gen_field_assign`）。
- **构造**（`gen_construct`）：`insertvalue` 链逐字段装配，缺省参数用默认值/零值。
- **元组字段访问**：`extractvalue`（寄存器值直接取，无地址要求）；
  **struct 字段访问**：`gen_class_addr` 先取地址（Var → 绑定地址；FieldAccess 链 → 逐级 GEP），再 load。
- **`current_ret_ty`**：函数/方法内 return 的返回类型查询——查 `funcs`（兜底 `I64`）。

### 9.2 运行流程

1. 先遍历 `Stmt::FnDef` / `Stmt::Namespace` 生成全部函数（`gen_fn` / `gen_ns_fns`，
   含方法函数 `@Point$dist`），`gen_fn` 判定方法函数首参按引用生成。
2. `Stmt::Struct` 不生成代码（纯类型布局，字段由语义层拍平）。
3. import 已在 driver 层递归加载为内联函数（语义分析前）。
4. 生成 `.ll` → 调用 `opt`（优化）→ `clang`（汇编）→ `lld`（链接）。

### 9.3 tie:script 模块协议（eval / eval_call）

tie:script 是「宿主进程 ↔ tie 脚本」的执行协议：约定 `.tie` 模块文件在
解释器会话中被 **`eval` 注册 + `eval_call` 字符串值直传调用**。
完整说明见 [docs/tie-script.md](tie-script.md)，要点：

- **入口约定**：模块顶层定义 `func process(src: string) -> string`
  （可放命名空间，用全名 `mod::process` 调用；void 入口返回空串）；
- **执行语义**：`eval(模块源码)` 把顶层定义收进会话函数表（跨调用持久）；
  `eval_call(全名, 文本)` 以 `Value::Str` **值直传**调用（不经源码文本转义），
  要求入口恰好 1 个必选字符串参数（可带默认值可选参数）；
- **三层入口**：Rust 侧 `tie_prep::run_module`（tie-prep 自举）／CLI 侧
  `tie-prep --module <file.tie>`（转换器扩展）／tie 程序内内置
  `eval` / `eval_call`（编译与解释双路径，见下）；
- **编译路径桥**：内置 `eval`/`eval_call` 在 IR 中生成对 tie-interp 静态库
  C 导出的调用——`tie_eval_expr` / `tie_eval_call`（+ 用后 `tie_free_result`
  释放堆串），链接 `tie_interp.lib`；
- **协议文本**：`eval_call` 只能返回一个字符串，跨层结构化数据用文本协议
  （如 prep/core.tie 的 `ROLE:`/`HEADERS:`/`H:`/`BODY:` 四行前缀 + 字节计数正文）。

## 10. 给编译器开发 AI 的硬性规则

1. **修改 AST 枚举时**：新增变体会破坏 `semantic.rs` / `ir.rs` 的 `match`——需同步
   添加分支并保证 `match` 穷尽。
2. **字段索引唯一权威**：`ClassInfo.field_index`（语义层计算），IR 层不得自行遍历拍平，
   否则偏移错位。
3. **方法函数在 funcs 表**：方法 = 绑定 struct 名的命名空间函数（全名 `Point::dist`），
   与顶层函数统一进 `result.funcs`；struct 名与函数名冲突由 collect_structs 拦截。
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
| 修改 tie:script 协议 / eval / eval_call | crates/tie-interp/src/lib.rs（Session::eval/eval_call、call_fn 分发、C ABI 导出）→ crates/tie-llvm/src/ir.rs（内置调用生成）→ prep/（模块）→ docs/tie-script.md |

