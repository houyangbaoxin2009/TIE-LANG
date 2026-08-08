# tie:script——tie 脚本模块协议

> 状态：**已实现**（Harbor M2.2 引入模块协议基础，M3 起自举链路全面投入使用）
> 所属：tie 语言执行体系（解释器层）
> 一句话：**约定入口函数 + 字符串值直传调用**——让「tie 程序」能在宿主进程内被
> 动态注册、用源码文本当输入、拿文本当输出，从而用 tie 语言自身扩展 tie 工具链。

## 1. 它是什么

`tie:script` 是一份**模块执行协议**：约定一个 `.tie` 源文件可以作为「脚本模块」
在解释器会话（`tie_interp::Session`）中被动态加载与调用。核心只有两条：

1. **注册**：把模块源码整体交给 `eval` 执行，顶层 `func` / `namespace` 定义被
   收进会话的函数表（`funcs`），跨多次调用保持；
2. **调用**：用 `eval_call("入口函数全名", 文本)` 以**字符串值**直传一个字符串实参，
   拿回函数返回的字符串。

协议本身不规定模块里写什么逻辑——它只提供「宿主（Rust 程序或另一个 tie 程序）
↔ tie 脚本」之间的**双向管道**，模块可以是转换器、分析器、代码生成器、
协议处理器……任何「一段文本进、一段文本出」的处理单元。

### 为什么用「字符串」作为协议边界

- `eval_call` 的实参**直接绑定为字符串值**（`Value::Str`），不经源码文本转义——
  多行内容、引号、换行原样直传，不会出现「转义地狱」；
- 返回值同理：模块 `return` 一个字符串即完成输出（`void` 入口返回空串）；
- 跨语言边界零结构体依赖，协议文本（见 §4）可承载结构化数据。

### 三个落地场景（既有实现）

| 场景 | 位置 | 说明 |
| --- | --- | --- |
| 预处理器自举 | `prep/core.tie` + `tie-prep` Rust 壳 | 预处理核心逻辑全部 tie 语言化，Rust 仅解释执行（M3 阶段一） |
| CLI 转换器扩展 | `tie-prep --module <file.tie>` | 命令行挂载任意 tie 转换器，Rust 零改动（M3 阶段一） |
| 程序内动态执行 | tie 语言 `eval` / `eval_call` 内置函数 | REPL 与普通 tie 程序也能加载/调用模块（M2.2 起） |

## 2. 核心机制

### 2.1 `eval`——注册或执行的统一入口（`Session::eval`）

```
eval(代码字符串) -> 结果字符串
```

`Session::eval` 有两段逻辑（跟 REPL 一致）：

1. **先尝试顶层解析**：如果代码是顶层定义（`func` / `namespace` / `class` / `import`），
   注册到会话状态（函数进 `funcs` 表并递归注册命名空间），返回
   `已定义 N 个函数`；
2. **否则按表达式/语句执行**：把代码自动包装成 `func main() { ... }` 执行，
   返回最终表达式的可打印字符串。

会话状态（`globals` + `funcs`）跨 `eval` 调用**持久保存**——先 `eval` 注册模块，
后 `eval_call` 调用，正是靠这一点。

### 2.2 `eval_call`——调用已注册函数（`Session::eval_call`）

```
eval_call(函数全名, 字符串参数) -> 结果字符串
```

行为约束（与 `tie_eval_call` C ABI 同源）：

1. **函数必须已注册**（否则报 `eval_call: 未定义的函数 'xxx'`）；
2. **形参约定**：必须恰好接收 1 个**字符串参数**（必选参数 ≤ 1；其余可以是带
   默认值的可选参数，默认值表达式在调用点补齐）。0 参函数或 2+ 必选参数 → 报错
   「必须恰好接收 1 个字符串参数」；
3. 实参**值直传**：第一个参数直接绑定 `Value::Str(arg)`；可选参数用其默认值求值补齐；
4. **命名空间支持**：入口函数可放在 `namespace` 里，用全名 `mod::upper` 调用
   （`::` 分隔）；函数体内部裸调用同样按该命名空间前缀补全；
5. **作用域隔离**：被调函数内声明的变量不污染调用者（新作用域 + `scope_base` 隔离）；
6. 返回：`return expr` → 值的可打印字符串；void（无返回）→ 空串。

### 2.3 字符串值直传（关键设计）

`eval_call` 的实参**不是**把 `arg` 丢回一个解析器——它就是 `Value::Str` 本体。
所以：

```
eval_call("process", "line1\nline2")   // 多行原样传入，无需转义
```

模块内部拿到的 `src` 是完整文本值，`len(src)` 是字符数、`str_char(src, i)` 逐字符
访问——与源码文本的字节/转义表示无关。

## 3. 模块约定

一个 tie:script 模块 = 任意 `.tie` 源文件，满足：

### 3.1 入口约定

顶层必须有一个可被 `eval_call` 调用的入口函数，**约定名 `process`**：

```c
func process(src: string) -> string {
    // …对 src 做处理，返回字符串结果
}
```

- 入口名不强制为 `process`（`eval_call` 接受任意已注册函数名），但 `process` 是
  **框架/CLI 约定名**：`--module` 挂载与 `run_module` 都默认调 `process`；
- 入口也可以放进 `namespace`，用全名 `ns::process` 调用；
- void 入口允许（收到空串返回），适合「纯副作用」模块。

### 3.2 自包含约束（重要）

**模块不能依赖 `import`**——解释器 `eval` 不支持 `import`（REPL v1 限制）。因此：

- 模块内部只能用**语言底座原语**（`str_char` / `len` / 字符串拼接 / `table_new_*` /
  数学/文件等内嵌函数）；
- 需要字符串工具的模块需自备最小实现（如 `prep/core.tie` 内联
  `trim` / `slice` / `is_whitespace` 等，与 `std/string.tie` 等价但不 import）。

这是有意的设计取舍：模块**自包含** → 单文件即可运行 → 携带部署成本最低。

### 3.3 最小示例

```c
// upper.tie —— 全部转大写（示意）
func process(src: string) -> string {
    var n: i64 = len(src)
    var out: string = ""
    var i: i64 = 0
    while i < n {
        var c: string = str_char(src, i)
        // 简化示例：仅转小写字母（'a'..'z'）
        if c >= "a" && c <= "z" {
            out = out + str_char("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 字符位置)
        } else {
            out = out + c
        }
        i = i + 1
    }
    return out
}
```

（生产示例见 `prep/indent.tie`：制表符 → 4 空格的缩进规范模块。）

## 4. 协议文本格式（`prep` 专用）

`prep/core.tie` 的 `process` 返回**协议文本**，Rust 壳解析后还原预处理结果。
格式（每行一条，`\n` 结尾）：

```text
ROLE:logic            ← 第 1 行：角色（logic/ui/db/data/library）
HEADERS:2             ← 第 2 行：头部指令数量
H:opt=2               ← 接下来 n 行：每条头部指令原文（已剥 // tie: 前缀）
H:target=win
BODY:12               ← 再 1 行：正文字符数（字节数，tie len 语义）
<正文恰好 12 字节>     ← 剩余恰好 m 字节正文（可含换行/任意内容）
```

- **正文按字节数精确截取**：不按行、不按 `\n` 拆分，正文内的任何字符都不会
  破坏协议（这是 `BODY:<m>` 用字节计数而不是行数的原因）；
- Rust 壳（`parse_protocol`）逐行识别 4 类前缀（`ROLE:` / `HEADERS:` / `H:` /
  `BODY:`），其余内容全部视为正文，直到累计够 `m` 字节。

> 为何输出协议文本而非结构化对象：`eval_call` 只能返回一个字符串。
> 文本协议是「字符串为边界的互通」的自然延伸（对齐 §1 的设计）。

## 5. 三层调用入口

tie:script 模块可以从三个层面被调用：

### 5.1 `Rust 侧`：`tie_prep::run_module`

```rust
// tie-prep 内部：加载模块源码 → 注册 → 以字符串直传入口调用
pub fn run_module(module_source: &str, entry: &str, source: &str) -> Result<String, String> {
    let mut session = tie_interp::Session::new();
    session.eval(module_source)?;      // 注册模块顶层定义
    session.eval_call(entry, source)   // 字符串值直传调用
}
```

`tie-prep` 的 `preprocess()` 就用它执行 `prep/core.tie`：

```rust
let text = run_module(PREP_MODULE, "prep::process", &source)
    .unwrap_or_else(|e| panic!("预处理模块执行失败: {e}"));
parse_protocol(&text)   // 解析协议文本 → PreprocessResult
```

### 5.2 `CLI 侧`：`tie-prep --module`

```bash
tie-prep <input.tie> --module <module.tie>
# 读出模块文件 → run_module(module_src, "process", 源码)
# 模块返回文本原样写 stdout，[tie-prep] 诊断写 stderr
```

验证（`prep/indent.tie`，VSCode 缩进规范化转换器）：

```bash
tie-prep examples/hello.tie --module prep/indent.tie
```

**扩展性证明**：新增一种「源文本转换器」 = 新增一个 `.tie` 模块文件，
`--module` 挂载即可——**不用改 Rust、不用重编工具链**。

### 5.3 `tie 程序/REPL 侧`：内置 `eval` / `eval_call`

tie 语言把 `eval` / `eval_call` 作为**内置函数**暴露（编译与解释路径都支持，
通过 tie-interp C ABI 桥，见 §6）：

```c
// 例子：名字包装器
var module = "func process(src: string) -> string {\n    return \"[\" + src + \"]\"\n}\n"
var reg = eval(module)                 // 注册 → "已定义 1 个函数"
var out = eval_call("process", "hi")   // "[hi]"
```

- `eval(code)`：注册或执行（与 §2.1 共识相同），返回结果字符串；
- `eval_call(name, arg)`：调用已注册函数，返回结果字符串；
- 命名空间全名 / void 入口行为见 §2.2；
- 端到端验证见 `examples/script_demo.tie`：
  - 多行字符串直传（`line1\nline2`），不被转义；
  - 命名空间入口（`mod::upper`）；
  - void 入口 → 空串。

## 6. 编译路径与 C ABI 桥

tie-llvm（编译路径）与 tie-interp（解释路径）共享同一套 `eval`/`eval_call`
语义，编译路径通过 interp 静态库（`tie_interp.lib`）的 C 导出实现：

| 导出符号 | 作用 | 谁调用 |
| --- | --- | --- |
| `tie_eval_expr(code)` | 求值一段代码（回到 Session::eval） | IR 的 `eval(...)` 调用 |
| `tie_eval_call(name, arg)` | 调用已注册函数（Session::eval_call） | IR 的 `eval_call(...)` 调用 |
| `tie_free_result(p)` | 释放解释器返回的堆字符串 | IR 每次调用后清理 |

IR 生成逻辑（`crates/tie-llvm/src/ir.rs`）：

- 内置 `eval` → `mark_used("tie_eval_expr")` + `tie_free_result`；
- 内置 `eval_call` → `mark_used("tie_eval_call")` + `tie_free_result`；
- 只有用到才 declare（`declare ptr @tie_eval_expr(ptr)` 等），未用不引入符号；
- 返回的堆指针按语义是否消费区分：非尾部表达式调用后立即 `tie_free_result`，
  尾部结果返回给调用方（由调用方释放）——REPL 会话级小泄漏可忽略。

> 因此 tie 程序**编译运行**（`tie xxx.tie`）时也能用 `eval` / `eval_call`——
> `examples/script_demo.tie` 就是编译运行的端到端验证。

## 7. 设计约束与限制

| 项 | 说明 |
| --- | --- |
| 模块不能 import | interp `eval` 不支持 import（REPL v1 局限）→ 模块必须自包含 |
| 类不支持 | `eval` 顶层 `class` 会拒绝（「REPL v1 暂不支持类定义」） |
| 函数参数约束 | 入口恰好 1 个必选字符串参数（可带可选参数） |
| 字符串边界 | 传输物是字符串：跨层结构化数据需自定文本协议（如 §4） |
| 无模块持久性 | `run_module` 每次新建 `Session`；跨调用持久化只存在于同一进程的同一 Session 内 |
| 错误传播 | 模块内错误（未定义函数/参数不符/求值错误）→ `eval_call` 返回 `Err`，
   `run_module` 透传；编译期调用方用 `?` 或 panic 处理 |

## 8. 影响范围与相关文件

| 组件 | 位置 | 说明 |
| --- | --- | --- |
| 协议定义 | 本文件（docs/tie-script.md） | 约定 + 三层调用入口 |
| interp API | crates/tie-interp/src/lib.rs | `Session::eval` / `Session::eval_call` |
| 内置函数 | 同 lib.rs `call_fn` 分发 | tie 语言的 `eval` / `eval_call` |
| C 桥 | lib.rs `#[no_mangle]` 导出的 `tie_eval_expr` / `tie_eval_call` / `tie_free_result` | 编译路径复用 |
| Rust 壳 | crates/tie-prep/src/preprocess.rs | `run_module` + `parse_protocol` |
| 核心模块 | prep/core.tie | 自举的预处理模块（tie 语言） |
| 转换器示例 | prep/indent.tie | 制表符 → 4 空格的独立转换器 |
| 端到端验证 | examples/script_demo.tie | 程序内动态 eval / eval_call |

## 9. 相关文档

- [docs/language.md](language.md) §2.4：预处理自举（协议文本 + eval_call 用法）
- [docs/plans/](plans/)：后续里程碑规划
- [CHANGELOG.md](../CHANGELOG.md)：[Harbor M2.2]（协议基础）与 [M3]（自举/挂载入口）