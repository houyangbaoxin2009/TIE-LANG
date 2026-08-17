# tie

<p align="center">
  <img src="assets/tie-logo-full.svg" alt="tie 语言 Logo" width="600">
</p>

> ⚠️ **早期开发阶段**：语言设计与实现仍在快速演进，语法、语义与工具链随时可能变更，暂不建议用于生产。

tie 是一门**通用编程语言**：用一门语言写逻辑、写界面、写数据库、当数据交换格式。
同一个 `.tie` 文件扮演什么角色，由文件头（Header）声明——四段式架构（预处理 → 前端 → 中端 → 后端）。
内置类型含 `trit`（平衡三进制三值逻辑，数论常用）与多进制字面量（`0x`/`0b`/`0o`/`0t`）。
支持**用户泛型**（泛型函数 `func max<T>(a: T, b: T) -> T` + 泛型 struct
`struct Box<T>`，编译期单态化 + 调用点类型推断，tiec 实现）与 **enum 枚举**
（Rust 风格 ADT：无数据/带 payload/泛型变体，静态结构体布局 + switch 匹配，
tiec 实现）。阶段 1 语言地基（2026-08-15）新增三大系统能力：
**unsafe 模型**（`unsafe fn`/`unsafe { }` 块 + `ptr<T>`/`slice<T>`/`atomic<T>` +
`repr(C)` struct + extern 强制 unsafe + `asm!` 内联汇编 + alloc/free，见
[docs/plans/unsafe-model.md](docs/plans/unsafe-model.md)）、**窄整数**
（`i8`/`i16`/`i32`/`u8`/`u16`/`u32`/`u64`/`f32`：后缀字面量 `42i32`、
`as_*` 转换族、`checked_*` 溢出检测族、明确移位语义，见
[docs/plans/int-model.md](docs/plans/int-model.md)）、**角色扩展**
（多角色叠加 `type tie<db:vector, unsafe>` + 角色参数化 + 文件名一致性
= 编译错误，见 [docs/plans/role-model.md](docs/plans/role-model.md)）。
阶段 2 错误处理（2026-08-16，S2.3）新增**错误处理模型**：预置
`enum Result<T, E>` / `enum Option<T>`（`std/result.tie`，import 即用）、
`?` 解包后缀（Err/None 提前返回 + Ok/Some 解包 payload）、`panic("msg")`
致命错误语句，见 [docs/plans/error-model.md](docs/plans/error-model.md)。
阶段 2 闭包/函数值（2026-08-17，S2.2）新增**一等函数值**：`func` 字面量
（闭包，捕获变量 move 进环境）、`fn(A) -> R` 函数类型（作参数/返回/
变量）、命名函数提升（函数名直接作函数值）、高阶函数与间接调用
（`{env, entry}` 闭包值 + call_indirect），探针全过，见
[docs/plans/closure-model.md](docs/plans/closure-model.md)。
阶段 2 字符串模型（2026-08-17，S2.1）升级**字符串内部表示**为
`{ptr,len}` 二进制安全（长度头 8 字节 + UTF-8 数据 + 边界自动 NUL，FFI
零拷贝）：`len(s)` 字节长度 O(1)、新原语 `str_byte(s,i)`（二进制安全取字节）、
`utf8_seq_len(s,i)`/`utf8_char_at(s,i)`（码点迭代）、StringBuilder
（`string_builder()` + `sb_append`/`sb_append_byte` + `sb_build`，原地
追加 + 容量倍增）；tie 桥返回串自动补头统一布局，旧 API（str_len/str_cat/
to_string）行为不变，探针全过 + 自举闭环 IR 逐字节一致，见
[docs/plans/string-model.md](docs/plans/string-model.md)。
阶段 3 库/包模型（2026-08-17，S3.2）新增**库/包能力**：**tieir 二进制
序列化**（IR 分发单元：`--tieir-out` / `--dump-irt`，`compiler/middle/
tieir_ser.tie`）、**多文件包 L1c**（`tie pack`/`tie verify`：包 = 目录 +
tie.pkg，入口声明导出面，包内模块私有）、**MVS 最小版本选择**（P2c：
约束解析取最低满足版本，可复现）、**签名校验**（P5c：pack 生成 signature
内容哈希，install/verify 校验防篡改），见
[docs/plans/package-model.md](docs/plans/package-model.md) 与
[docs/plans/tieir-format.md](docs/plans/tieir-format.md)。


我们的目标：全领域通用，Python的体验，Rust的性能与安全。

## 文档目录

| 文档                                                         | 内容                                                                                                |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| [README.md](README.md)                                     | 本文件：工程入口（快速开始、CLI、结构、流水线、路线图）                                                                     |
| [docs/language.md](docs/language.md)                       | 语法规范：文件结构、类型系统、语句/控制流、函数、面向对象、语法速查表                                                               |
| [docs/language-comparison.md](docs/language-comparison.md) | 语言对比报告：tie vs 42 种语言的特性/标准库全景对比（工业级全栈定位、丰俭由人伸缩谱）                                                  |
| [docs/tiec.md](docs/tiec.md)                               | tiec 自举编译器文档：tiec 是什么、自举链、快速开始、CLI 用法、运行时依赖、架构与进度                                                 |
| [docs/tie-script.md](docs/tie-script.md)                   | tie:script 模块协议：tie 脚本的注册/调用机制、模块约定、协议文本格式、三层调用入口（Rust/CLI/tie 程序内）                               |
| [docs/ai-guide.md](docs/ai-guide.md)                       | AI 教学指南：语言用法 + 负例 + 编译器架构（教 AI 用/开发 tie）                                                          |
| [docs/prompt-pack.md](docs/prompt-pack.md)                 | 可粘贴 Prompt 包：自包含简介，直接发给任何 AI                                                                      |
| [docs/plans/](docs/plans/)                                 | 后续里程碑设计规划（switch 模式匹配 / 单文件命名空间 / 统一 func 写法 / 动态库编译 / 包管理器 / 算法库分类 / 嵌入式基础层 rdu / **泛型系统（已实现**）） |
| [CHANGELOG.md](CHANGELOG.md)                               | 版本变更记录（按里程碑）                                                                                      |

## 快速开始

```bash
# 编译并运行示例（tiec 自举编译器）
compiler\tiec.exe examples\hello.tie
examples\hello.exe

# 无参数 → 进入 REPL
compiler\tiec.exe repl\repl.tie
```

`examples/hello.tie` 输出：

```
Hello, tie!
四段式: 预处理 [前端 中间优化 后端]
50
336
100
x 大于 y
0
1
2
3
4
5
6
7
8
9
```

## 文件类型声明（Header）

同一个 `.tie` 文件扮演什么角色，由文件**头部的类型声明**决定（不再使用 `// tie:xxx`
注释指令）。声明写在文件最前面的连续前导行（允许其间空行），是真正的语法行：

```tie
type tie            # 泛型入口类型（Type 角色，由裸 type tie 表达）
type tie<script>    # 脚本
type tie<data>      # 数据文件（纯数据/数据交换）
type tie<ui>        # 界面文件
type tie<class>     # 类/库文件 → 编译静态库 .a
type tie<logic>     # 逻辑代码（默认角色，可省略）
type tie<port>      # 端口/对外接口文件
type tie<db>        # 数据库文件
type tie<ir>        # IR 文件（直接生成 LLVM IR .ll）
type tie<zd>        # 压缩数据文件（tie:data 二进制变体，主要用文件名 xxx.zd.tie 声明）
```

- **子类型**：`script` / `data` / `ui` / `class` / `logic` / `port` / `db` / `ir` / `zd`；
  `type` 角色本身由**裸 `type tie`**（无尖括号）表达，`type tie<type>` 是格式错误；
- **多角色叠加（S1.4）**：基础角色唯一 + 修饰/参数化角色可叠加：
  `type tie<db:vector, unsafe>`、`type tie<class, unsafe>`；
  参数白名单：`ui:window/web/embedded`、`db:schema/seed/vector`、`data:config/asset`；
- **默认角色**：无声明时默认 `logic`（可执行文件，需 `func main()`）；
- **文件名声明角色**：`xxx.<角色>.tie`
- **角色分派**：

| 角色                            | 工具链                   |
| ----------------------------- | --------------------- |
| `logic` / `script`            | 编译为可执行文件              |
| `class` / `type`              | 编译为静态库 `.a`           |
| `ir`                          | 生成 LLVM IR（.ll），不继续编译 |
| `data` / `ui` / `db` / `port` | 对应工具链，挂接点未实现（提示）      |

## 单文件命名空间

`namespace foo` 后不加花括号（独占一行，或以 `;` 结尾），表示**从声明处起整份文件
的剩余内容都属于命名空间 foo**：

```tie
namespace foo            // 等价于 namespace foo;
func add(a: i64, b: i64) -> i64 { return a + b }
```

`namespace foo` 换行（ASI 自动补分号）与手写 `namespace foo;` 完全等价（AST 一致）；
嵌套单文件命名空间递归生效，块式 `namespace foo { ... }` 仍可用。

## CLI 用法

主入口 `tie`（四段式调度器，合并原 tie-cli 职责）：

```
tie <input.tie>... [-o output] [-O0|-O1|-O2|-O3] [--target <三元组>] [--emit-ir] [--keep-ir] [--prep-only] [--config <file>]
tie --lsp        # 语言服务器模式（LSP over stdio，供编辑器接入）
tie             # 无参数 → 进入 REPL 交互模式（启动 tie 语言自写的 repl.exe，自举）
tie init|add|remove|install|update|build|run|publish|search|info|help   # 包管理器（M6，tie 语言自写）
```

| 选项                    | 说明                                                                                                                                                            |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `-o <file>`           | 指定输出文件路径（logic/script 默认：输入同名 `.exe`；class/type 默认：输入同名 `.a`；单文件编译时生效）                                                                                        |
| `-O0..-O3`            | 优化级别（默认 `-O2`），映射到 `opt -O2`                                                                                                                                  |
| `--target <三元组>`      | 交叉编译目标（如 `win-x64` / `x86_64-pc-windows-msvc`，默认本机）。支持平台别名：`win-x64`、`win-x86`、`win-arm64`、`linux-x64`、`linux-arm64`、`macos-x64`、`macos-arm64`，也可直接写 LLVM 三元组 |
| `--emit-ir`           | 只生成 LLVM IR（.ll），不继续编译                                                                                                                                        |
| `--keep-ir`           | 保留中间 IR 文件                                                                                                                                                    |
| `--prep-only`         | 只做预处理（tie-prep）并打印识别结果                                                                                                                                        |
| `--config <file>`     | 指定构建配置文件（S3.1：默认查当前目录 `config.data.tie`，分层合并：CLI > 项目 config > 用户 `~/.config/tie/config.data.tie` > 内置默认）                                                     |
| `--profile <p>`       | 构建 profile（dev/release，覆盖配置顶层 `profile` 键；Cargo 风格，S3.1）                                                                                                      |
| `--backend <b>`       | 后端实现选择（win32/LLVM 工具链；其余 port 尚未接入，S3.1）                                                                                                                      |
| `--module <file.tie>` | tie-prep：挂载自定义 tie 转换器模块（顶层 `process(src)->string`），输出为模块转换结果（Harbor M3 可扩展性）                                                                                 |
| `--lsp`               | 以语言服务器模式运行（读 stdin 的 LSP 消息、写 stdout，等价于 `tie-lsp`）                                                                                                           |
| `-h, --help`          | 显示帮助                                                                                                                                                          |

**包管理器子命令（Harbor M6，tie 语言自写）**：Rust 入口识别首个参数为子命令
（且非 `.tie` 文件）后，转交 tie 语言自写的 `pkg.exe`（`pkg/main.tie` 经
tie-llvm 编译链接 interp 库生成），完整 CLI 逻辑全部在 tie 侧：

| 子命令                | 说明                                                                                           |
| ------------------ | -------------------------------------------------------------------------------------------- |
| `tie init <项目名>`   | 初始化项目（生成 tie.pkg 清单 + main.tie 模板）                                                           |
| `tie add <依赖>`     | 添加依赖（`path:./lib_math` 本地源 / `git+https://...` git 源 / `log@1.0.0` 或 `log@^1.2` registry 约束） |
| `tie remove <包名>`  | 移除依赖                                                                                         |
| `tie install`      | 解析 + 拉取全部依赖到 `.tie/deps/`，生成/校验 tie.lock（三源：path/git/registry）                               |
| `tie update [包名]`  | 重新解析依赖并更新 tie.lock                                                                           |
| `tie build`        | 编译项目（调用 tie 编译器）                                                                             |
| `tie run`          | 编译并运行项目                                                                                      |
| `tie publish`      | 打包发布（`.tie/dist/<name>-<version>.tar.gz` + `git tag v<version>` + push）                      |
| `tie search <关键字>` | 搜索注册表 index.tie（`TIE_REGISTRY` 可指定基址）                                                        |
| `tie info <包名>`    | 查询注册表包的最高版本与描述                                                                               |
| `tie help`         | 显示包管理器帮助                                                                                     |

端到端演示见 [examples/pkg_demo.md](examples/pkg_demo.md) 与 `examples/demo_pkg/`。
构建 `pkg.exe`：`compiler/tiec.exe pkg/main.tie -o pkg/pkg.exe`

流程：`tie-prep` 预处理（清理代码 + 识别文件类型）→ 按角色自动转交工具链。

**多文件并行编译**：配置文件开启 `advanced.enabled = true` 后，可一次编译多个输入文件。

```tie
// tie.config
type tie<data>
[
    "advanced": [
        "enabled": true,
        "threads": 0,        // 0 = 按 CPU 核数自动
    ],
    "cache": [
        "size": 268435456,   // 256MB
        "storage": "memory", // memory / file
        "path": ".tie-cache",
    ],
]
```

库编译示例（`type tie<class>` 角色，定义函数不定义 main）：

```bash
tie examples/lib_math.tie          # → examples/lib_math.a（经 clang -c 生成 .o，llvm-ar rcs 打包）
tie examples/lib_math.tie -o lib_math.lib   # → MSVC 兼容静态库 .lib（COFF 归档，同一产物不同扩展名）
```

- 静态库 `.a` / `.lib`（Windows 上均为 COFF 归档）：导出符号为 `命名空间$函数`
  （如 `mathlib$add`），C/其他语言可链接消费；
- 动态库（`.dll` / `.so`）编译为 Harbor M5 内容（见 docs/plans/dynamic-library.md）。

子工具可单独使用（Rust 版子工具 tie-prep/tie-frontend/tie-lsp/tie-llvm/tie-interp 已随
[tiec_rust](https://github.com/tie-lang/tiec_rust) 归档；tiec 内嵌等价前端/IR 能力）：

- `compiler/tiec.exe <file.tie> [--emit-ir] [--keep-ir] [--prep-only]` —— 完整编译 / 只出 IR / 只预处理

REPL 自举：REPL 外壳 `repl/repl.tie` 用 tie 语言自身编写（`print` + `read_line` + `eval`），
经 tiec（自举 v2 编译器，自举升格）编译并链接 tie-interp 静态库（C ABI 桥）生成 `repl.exe`。构建：

```bash
compiler/tiec.exe repl/repl.tie             # 链接 interp 库生成 repl/repl.exe
```

`tie` 无参数时按 `TIE_REPL_EXE` → tie.exe 同目录 → 当前目录查找 repl.exe。
LLVM 工具定位同「快速开始」的发现顺序；发行版自带 `bin/llvm/`，用 `TIE_LLVM_HOME` 指向它即可开箱即用。

## 工程结构

```text
tie/
|
├── compiler/         编译器：
│                     - frontend/：词法/语法/语义分析器
│                     - middle/：tie-IR 列式表 + 类型系统
│                     - backend/：irgen + llvmgen+ toolchain
│                     - interp/：解释器
│                     - driver.tie → tiec.exe：CLI壳
│                     - repl.tie → repl.exe：REPL
|
├── prep              预处理器核心模块（tie 语言自写：头部提取/角色判定/正文重建；Harbor M3 自举，编译期内嵌 tie-prep）
|
├── std/              标准库：
│                     - 文本/编码：string 、utf、ascii、encoding、regex、json、set
│                     - 数据结构/算法：sort、collection、crypto、optsearch、graph、linalg、exmath、math、radix
│                     - IO/系统：fs、path、args、http、random、bytes、time、process、intern、version、format、csv、assert、net、deque、db
│                     
├── ext/              扩展库
├── rdu/              嵌入式基础层
├── repl/repl.tie     REPL 外壳
├── tieDB/            tieDB
├── pkg/              包管理器
├── docs/             文档
└── examples/         示例程序
```

> Rust 编译器（tiec_rust）已归档至独立仓库 [tiec_rust](https://github.com/tie-lang/tiec_rust)。
