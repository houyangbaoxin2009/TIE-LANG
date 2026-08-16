# tie

<p align="center">
  <img src="assets/tie-logo-full.svg" alt="tie 语言 Logo" width="600">
</p>

> ⚠️ **早期开发阶段**：语言设计与实现仍在快速演进，语法、语义与工具链随时可能变更，暂不建议用于生产。

tie 是一门**通用编程语言**：用一门语言写逻辑、写界面、写数据库、当数据交换格式。


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
