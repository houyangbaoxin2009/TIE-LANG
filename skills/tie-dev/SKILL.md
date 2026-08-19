---
name: tie-dev
description: 用 tie 语言开发软件——tie 语法与类型系统、文件角色与头声明、编译运行（tiec）、标准库、包管理器、工程化工作流。在 F:\Projects\tie 仓库或任何 tie 项目中编写 .tie 应用代码时加载。
whenToUse: 编写 tie 源文件（.tie）、编译运行 tie 程序、使用标准库（std/）、组织多文件项目、使用包管理器（pkg/）时使用。
---

# tie 开发技能（用 tie 写软件）

tie：静态类型、四段式编译（预处理→前端→中端→后端 LLVM）的通用编程语言。
目标：全领域通用——写逻辑、写界面、写数据库、当数据交换格式。
编译器 **tiec 由 tie 100% 自写**（自举闭环，0-Rust）；发布包内置精简 LLVM 工具链，解压即用。
本文档是**应用开发者向**：用 tie 写程序，不涉及编译器内部开发。

## 1. 快速开始

```bash
compiler\tiec.exe examples\hello.tie    # 编译 → examples\hello.exe（默认 -O2）
examples\hello.exe                       # 运行
compiler\tiec.exe                        # 无参数 → 进入 REPL 交互模式
compiler\tiec.exe repl\repl.tie          # 构建 REPL 外壳
```

- 输入 `.tie` 源文件，输出本机可执行文件（logic/script 角色）或静态库 `.a`（class 角色）；
- 发行 zip 内置 `bin/llvm/`（clang/opt/llvm-ar/lld-link），设置 `TIE_LLVM_HOME` 指向它即可开箱即用，无需单独安装 LLVM；
- 链接用户程序需要 `std/runtime.a`（tie 自写运行时静态库），tiec 自动定位；
- LLVM 工具发现顺序：`TIE_LLVM_HOME\bin` → tiec.exe 同目录 `llvm\bin` → `PATH` → 固定目录（`D:\LLVM\bin` 等）。

## 2. 文件角色（头声明）

源文件**最前面几行**用 `type tie<X>` 声明角色，角色决定编译行为：

| 头部声明                            | 角色       | 编译行为                   |
| ------------------------------- | -------- | ---------------------- |
| `type tie<logic>`（默认）           | 逻辑代码     | 可执行文件，须含 `func main()` |
| `type tie<script>`              | 脚本       | 可执行文件                  |
| `type tie<class>` / `type tie`  | 类/库      | 静态库 `.a`，不生成 main      |
| `type tie<data>`                | 数据交换     | 纯数据（类似 JSON），可被 import |
| `type tie<ir>`                  | IR       | 直接生成 `.ll`，不继续链接       |
| `type tie<port>`                | 接口       | 端口/对外接口声明              |
| `type tie<ui>` / `type tie<db>` | 界面 / 数据库 | 对应工具链未实现，勿用            |

- 未声明头按 `logic` 处理；文件名 `xxx.<角色>.tie` 可作默认角色，但头部声明优先；
- 头只允许出现在文件头部；优化级别 / 交叉编译目标**仅 CLI**（`-O2` / `--target`），不放头部；
- 头部与内容之间允许空行分隔。

## 3. 类型系统

基本类型：`i8 i16 i32 i64 i128 u8 u16 u32 u64 f32 f64 bool trit char string void`。
复合类型：`table<T>`（动态数组）、`map`（键值表）、元组 `(T1, T2)`、struct、enum、`fn(A)->R`（函数类型）、`code`（编译期代码片段，宏用）。

```tie
var x = 5                    // 推导 i64（整数字面量默认 i64）
var f = 3.14                 // f64
var n: i32 = 1               // 显式标注
const s: string = "hi"       // 不可变
var t: table<i64> = [1, 2, 3]
var m: map = ["a": 1, "b": 2]
var p: trit = zero           // 平衡三进制 -1/0/+1（true/zero/false）
```

窄整数后缀：`42i32 / 7u8 / 1.5f32`；转换 `as_*`；溢出检查 `checked_*` 族。

## 4. 控制流

```tie
if x > 3 { } else if x > 1 { } else { }
while i < 10 { i = i + 1 }
for i in 0..10 { }                       // 范围：含 0 不含 10
for item in arr { }                      // 遍历表
switch n {
    case 1, 2:                           // 多值：任一相等即命中（逗号分隔）
        println("one or two")
    case 3..7:                           // 区间：3 ≤ n < 7（整数/字符，左闭右开）
        println("three to six")
    case 8 when flag:                    // 守卫：值相等 且 flag 为真
        println("eight and flag")
    case string:                         // 类型匹配：subject 为宽类型/动态容器时
        println("a string")
    default:                             // 可省略
        println("other")
}
```

- `case` 值支持整数、字符、布尔、负数、字符串、区间、多值组合（`case 1, 3..5 when cond:`）；
- **无 break、无 fallthrough**：一个 case 执行完自动跳出；守卫不满足落入下一个 case；
- 浮点区间不支持；普通静态类型变量上做类型匹配 → 语义层报错（恒真/恒假无意义）；
- `&&` / `||` 短路求值：条件里带副作用的调用须嵌套 if（避免被多读）。

## 5. 函数

```tie
func add(a: i64, b: i64) -> i64 { return a + b }        // 返回类型可省略（默认 void）
func greet(name: string, prefix: string = "Hello") { }  // 默认值参数（须连续排在必选之后）
func sum(xs: ...i64) -> i64 { ... }                     // 变参
func main() { println(add(1, 2)) }
```

### 5.1 ref 表参数（按引用传递，仅限表）

```tie
func fill(x: ref table<i64>) {
    table_push(x, 99)      // 内容修改写回调用方
    x[0] = 42
}
func replace(x: ref table<i64>) {
    x = table_new_i64()    // 重绑定也写回调用方实参槽
}
var t = table_new_i64()
fill(t)                    // len=3, t[0]=42
```

规则：实参必须是**可寻址的动态表变量**（表字面量/下标/调用结果不可取地址 → 编译错误）；非 ref 表参数保持值语义。

### 5.2 extern 函数声明（调用 libc / C 符号）

```tie
extern fn system(cmd: string) -> i32;    // 仅文件顶层；参数/返回仅标量
extern fn rand() -> i32;
println(rand())
println(system("exit 0"))
```

REPL / 解释路径不支持调用 extern（仅编译路径）。

### 5.3 多值返回（元组）

```tie
func divmod(a: i64, b: i64) -> (q: i64, r: i64) { return (a / b, a % b) }
var (q, r) = divmod(17, 5)     // 解构：q=3, r=2
var p = (x: 3, y: 4)           // 命名元组
println(p.x)                   // 命名访问；t.x / t.Item1 / t.0 三种形式等价
```

## 6. 数据结构

### 6.1 table（动态数组）与 map（键值表）

```tie
var arr: table = [1, 2, 3]
table_push(arr, 4)             // 追加（t[i] = v 在 i ≥ len 时静默失败，追加必须 table_push）
var e = arr[1]                 // 下标读（越界索引赋值无效，不追加）
for item in arr { }            // 遍历
var m: map<string> = ["x": "hi"]
var v = m["a"]                 // 下标读
m["c"] = 3                     // 下标写：不存在则插入
var k = len(m)                 // 条目数
```

- map 键恒为字符串，值类型全表一致，按键 **strcmp 字节序**有序存储（查找 O(log n)）；
- map 输出/打印按键排序，不依赖插入序；
- **map 不能作全局变量**（语法层拒绝）；仅 `table<T>` 支持顶层全局（`var g: table<i64>;`，跨函数持久）；
- 表字面量：`[1, 2, 3]`；二维表 `[1,2;3,4]` 语法可解析但语义报错，勿用。

### 6.2 struct（纯数据 + 命名空间方法）

```tie
struct Point {
    var x: i64 = 0             // 字段：var name[: Ty] [= 默认值]
    var y: i64 = 0
}
namespace Point {
    pub func dist(p: Point) -> i64 { return p.x * p.x + p.y * p.y }  // 实例方法（首参=接收者，按引用）
    pub func origin() -> Point { return Point(0, 0) }                 // 静态风格
}
var p = Point(3, 4)            // 构造（按字段声明顺序传参）；Point() / Point(1) 缺省用默认值
p.x = 5                        // 字段直写
println(p.dist())              // 方法转发 → Point::dist(&p)
```

- struct 是值类型（LLVM 内联布局，无 GC）；逻辑放**命名空间函数**，`obj.method()` 自动转发；
- **寄存器中的 struct 值不可直接访问字段/调方法**：`Point(0).x` / `make().dist()` 报「需要可寻址」——必须先 `var o = Point(0)` 存入变量；
- 方法函数必须 `pub`（否则被私有拦截）；
- 继承：`struct Dog extends Animal`（字段拍平，子在前父在后），方法沿继承链查找（子→父），无虚表/无动态分派；
- 子 struct 字段必须跨继承链唯一；继承环 / 字段重名 → 编译错误。

### 6.3 enum（ADT，tag + payload）

```tie
enum Color { Red Green Blue }                  // 无数据变体
enum Shape { Circle(i64) Rect(i64, i64) }      // 带 payload
enum Option<T> { Some(T) None }                // 泛型 enum
var c = Shape.Circle(5)                        // 构造
switch c {
    case Shape.Circle: println("circle")       // tag 匹配（case 名 → tag 整数比较链）
    case Shape.Rect:   println("rect")
    default: ...
}
```

- 静态结构体布局（tag + payload 槽），可作 struct 字段/函数参数/返回值；
- payload 目前支持 i64 等标量（string/f64 等宽类型暂不支持）。

### 6.4 泛型

```tie
func max<T>(a: T, b: T) -> T { if a > b { return a }; return b }   // 调用点推断
struct Box<T> { var v: T }
Box<i64>(42)           // 显式类型实参
max<i64>(9, 4)
pick<A, B>(a: A, b: B) -> A   // 多类型参数
```

编译期单态化展开 + 符号 mangling（`max$i64`）+ 嵌套类型实参（`Box<table<i64>>`）。

## 7. 面向接口：port（S2.4，已实现）

```tie
port Drawable {
    pub func draw(self, ctx: i64) -> string
    pub func bounds(self) -> i64
}
struct Button { var label: string; var w: i64; var h: i64 }
impl Drawable for Button {
    pub func draw(self, ctx: i64) -> string { return "Button(" + self.label + ")" }
    pub func bounds(self) -> i64 { return self.w * self.h }
}
// 泛型约束静态分发：T 实参化时校验「必须 impl Drawable」，编译期绑定具体实现
func render_all<T: Drawable>(d: T, ctx: i64) {
    println(d.draw(ctx))       // 单态化 → Button::draw / Text::draw
}
// 动态分发：unsafe 块把 struct 装箱为 port 值（data + vtable），异构容器 table<Drawable>
unsafe { var d: Drawable = button; d.draw(ctx) }   // 走 vtable 间接调用
```

- 静态分发（泛型约束）零开销；动态分发需要 `unsafe` 装箱 + 全局 vtable；
- `impl` 完整性检查：缺方法 → 编译错误。

## 8. 闭包与函数值

```tie
var d = func(x: i64) -> i64 { return x * 2 }          // 无捕获闭包
func apply(f: fn(i64) -> i64, x: i64) -> i64 { return f(x) }  // 高阶
var g: fn(i64) -> i64 = add1                           // 命名函数提升
func make() -> fn(i64) -> i64 { ... return func(x: i64) -> i64 { ... } }  // 闭包返回
```

闭包值 = `{env, entry}` 聚合，捕获变量 move 进 env（堆分配），调用走 call_indirect。

## 9. 错误处理

```tie
import "../../std/result.tie"        // 预置 Result<T,E> / Option<T>
func div(a: i64, b: i64) -> Result<i64, string> {
    if b == 0 { return Result.Err("z") }
    return Result.Ok(a / b)
}
var v = div(10, 2) ?                 // ? 解包：Err 提前 return，Ok 解包 payload
panic("致命错误")                      // 打印 + exit(1)
```

`?` 仅限返回 Result/Option 的函数内使用。

## 10. 宏 / 元编程

```tie
macro double(x: code) -> code {
    return `( ($x) * 2 )
}
macro make_getter(field: code) -> code {
    var nm = gensym("get")
    return `{
        func $nm() -> i64 { return self.$field }
    };
}
var v = double(3 + 4)    // 表达式宏：14
```

- code 三形态：准引用（`` ` ``）/ 插值（`$x`）/ gensym（`gensym("名")` 防命名冲突）；
- 函数式宏展开 pass（mexpand）；语句级宏 / 跨文件宏 / 过程宏为遗留项；
- 宏体里准引用末尾的 `}` 后要加分号（防 ASI 吞掉闭合）。

## 11. 模块化：import / 命名空间 / 标准库

```tie
import "./lib_math.tie" as math    // 导入其他 tie 文件（相对路径）
using math;                        // 引入命名空间：公有函数可裸调用
```

- 命名空间内函数默认**私有**（仅同命名空间可见），`pub func` 显式导出；顶层函数恒公有；
- `import "x.tie" as f2`：别名是唯一入口（原前缀被屏蔽）；
- 多 using 同名函数 → 裸调用歧义报错；
- 函数递归加载内联、重复导入去重；跨文件语义（命名空间调用）不误报未声明变量。

### 标准库（std/，35 文件，`import "../../std/xxx.tie"` + `using xxx;`）

```
文本：string / ascii / utf / bytes / format / regex / json / csv / encoding
数据结构：sort / collection / set / deque / graph / linalg / optsearch / radix
数学：math / exmath / random
IO/系统：fs / path / args / process / time / version / intern / assert
网络：net / http / http_server
其他：crypto / db / result / runtime
```

常用：

```tie
import "../../std/string.tie"  using string;
import "../../std/fs.tie"      using fs;
import "../../std/process.tie" using process;

var s = string.trim("  hi  ")            // 去空白
process.exec_code("exit 0")              // 0（libc system 退出码）
process.exec_output("echo hello")        // "hello\n"（stdout+stderr 合并捕获）
intern.intern("abc")                     // 字符串 → 稳定整数 id（std/intern.tie）
```

注意：`std/db.tie` 参数名用 `txt`（`text` 是类型关键字不能作参数名）；`len(s)` 是字节数，`str_len(s)` 按 Unicode 码点（中文 1 字 = 3 字节 1 码点，遍历用 `str_len` 才不会错位）。

## 12. 工程化

### 12.1 CLI 选项

```bash
tiec <input.tie> [-o <out>] [-O0|-O1|-O2|-O3] [--target <三元组>]
                [--emit-ir] [--keep-ir] [--prep-only] [--config <f>] [--help]
```

| 选项              | 说明                                                    |
| --------------- | ----------------------------------------------------- |
| `-o <file>`     | 输出路径（logic/script 默认输入同名 `.exe`；class/type 默认同名 `.a`） |
| `-O0..-O3`      | 优化级别（默认 `-O2`，映射 `opt`）                               |
| `--target`      | 交叉编译目标（如 `win-x64` / `x86_64-pc-windows-msvc`，默认本机）   |
| `--emit-ir`     | 只生成 LLVM IR（`.ll`），不继续编译                              |
| `--keep-ir`     | 保留中间 IR                                               |
| `--prep-only`   | 只做预处理（角色识别）并打印结果                                      |
| `--config <f>`  | 构建配置文件（分层合并：CLI > 项目 config > 用户 > 内置默认）              |
| `--profile <p>` | 构建 profile（dev/release，Cargo 风格）                      |
| `--backend <b>` | 后端选择（win32 唯一可用）                                      |
| `--lsp`         | 语言服务器模式（stdio）                                        |

退出码：`0` 成功 / `1` 编译失败 / `2` 参数错误。

### 12.2 库编译

```bash
tiec lib_math.tie                  # class 角色（type tie<class>）→ lib_math.a
tiec lib_math.tie -o lib_math.lib  # MSVC 兼容 .lib（同一 COFF 归档，不同扩展名）
```

导出符号为 `命名空间$函数`（如 `mathlib$add`），C/其他语言可链接消费。

### 12.3 包管理器（pkg/，tie 自写）

```bash
tiec pkg\main.tie -o pkg\pkg.exe    # 构建 pkg.exe
tie init <项目名>                    # 初始化（tie.pkg 清单 + main.tie 模板）
tie add path:./lib_math             # 本地源 / git+https://... / log@^1.2（registry 约束）
tie install                         # 解析 + 拉取依赖到 .tie/deps/，生成 tie.lock
tie build / tie run                 # 编译 / 编译并运行
tie publish                         # 打包发布（.tar.gz + git tag + push）
tie search <关键字> / tie info <包>  # 查询注册表
```

### 12.4 多文件并行编译与缓存

```tie
// tie.config（type tie<data>）
[
    "advanced": [ "enabled": true, "threads": 0 ],      // 0 = 按 CPU 核数
    "cache": [ "size": 268435456, "storage": "memory", "path": ".tie-cache" ],
]
```

## 13. 编译期会报错的写法（负例，勿生成）

| 场景                                              | 错误关键词                                    |
| ----------------------------------------------- | ---------------------------------------- |
| `Point(0).x` / `make().get()`（寄存器 struct 值直接访问） | 「需要可寻址」                                  |
| 方法函数未加 `pub` 被 `obj.method()` 调用                | 「私有函数…不可在命名空间之外调用」                       |
| 继承环 / 子字段与父字段重名                                 | 「struct 继承形成环」/「字段名必须跨继承链唯一」             |
| struct 体内定义方法                                   | 「struct 体不允许方法定义」                        |
| 字段无类型标注且无默认值                                    | 「字段必须标注类型或有默认值」                          |
| 函数体内定义 struct / import / 嵌套函数                   | 「顶层只允许…」                                 |
| const 变量重新赋值                                    | 「不可变变量不能赋值」                              |
| 类型不匹配（i64 赋给标注 i32）                             | 「类型不匹配」                                  |
| 元组空解构 `var () = ...`                            | 「空解构 () 不支持」                             |
| ref 实参非可寻址 / 非动态表 / 非表类型                        | 「ref 参数需要可寻址的表变量实参」等                     |
| const 全局表 / 顶层 map 全局变量                         | 「const 全局表暂不支持」/ 语法层拒绝                   |
| extern 参数为表或结构体 / 函数体内声明 / REPL 调用              | 「必须标量类型」/「只能出现在文件顶层」/「REPL 不支持调用 extern」 |
| 静态类型变量上做类型匹配（switch）                            | 语义层报错（类型恒定）                              |
| 浮点区间 case                                       | 语义层报错                                    |

## 14. 参考资料索引（写 tie 代码时查阅）

- `docs/language.md`：语法规范（权威）
- `docs/ai-guide.md`：AI 教学指南（语言全景 + 负例）
- `docs/cli.md`：CLI 用法速查（主入口 / 包管理器 / 库编译）
- `docs/tiec.md`：tiec 编译器文档（角色识别 / 运行时依赖 / 已知限制）
- `docs/tie-script.md`：tie:script 模块协议（eval / eval_call）
- `docs/prompt-pack.md`：可粘贴 Prompt 包（自包含简介）
- `examples/`：可运行示例（hello / lib_math / switch_pattern / pkg_demo…）
- `tests/*_probe/`：真实可用代码样例（最新特性语法以此为准）
- `NEW.md` / `CHANGELOG.md`：发行版新鲜事 / 版本变更记录