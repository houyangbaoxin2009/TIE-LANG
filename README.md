# tie

> ⚠️ **早期开发阶段**：语言设计与实现仍在快速演进，语法、语义与工具链随时可能变更，暂不建议用于生产。

tie 是一门**通用编程语言**：用一门语言写逻辑、写界面、写数据库、当数据交换格式。
同一个 `.tie` 文件扮演什么角色，由文件头（Header）声明——四段式架构（预处理 → 前端 → 中端 → 后端）。

> 语法规范见 [docs/language.md](docs/language.md)；本文件是工程入口（用法、结构、流水线、路线图）。

## 文档目录

| 文档 | 内容 |
| --- | --- |
| [README.md](README.md) | 本文件：工程入口（快速开始、CLI、结构、流水线、路线图） |
| [docs/language.md](docs/language.md) | 语法规范：文件结构、类型系统、语句/控制流、函数、面向对象、语法速查表 |
| [docs/ai-guide.md](docs/ai-guide.md) | AI 教学指南：语言用法 + 负例 + 编译器架构（教 AI 用/开发 tie） |
| [docs/prompt-pack.md](docs/prompt-pack.md) | 可粘贴 Prompt 包：自包含简介，直接发给任何 AI |
| [CHANGELOG.md](CHANGELOG.md) | 版本变更记录（按里程碑） |

## 快速开始

前置依赖：Rust（edition 2024）、LLVM 工具链（`opt`、`clang`、`lld`，编译链路的后端部分调用它们）。

```bash
# 构建
cargo build --workspace

# 编译并运行示例
cargo run -p tie -- examples/hello.tie

# 无参数 → 进入 REPL
cargo run -p tie
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

## CLI 用法

主入口 `tie`（四段式调度器，合并原 tie-cli 职责）：

```
tie <input.tie> [-o output] [-O0|-O1|-O2|-O3] [--target <三元组>] [--emit-ir] [--keep-ir] [--prep-only]
tie --lsp        # 语言服务器模式（LSP over stdio，供编辑器接入）
tie             # 无参数 → 进入 REPL 交互模式（启动 tie 语言自写的 repl.exe，自举）
```

| 选项 | 说明 |
| --- | --- |
| `-o <file>` | 指定输出文件路径（logic 默认：输入同名 `.exe`；library 默认：输入同名 `.a`） |
| `-O0..-O3` | 优化级别（默认 `-O2`），映射到 `opt -O2` |
| `--target <三元组>` | 交叉编译目标（如 `win-x64` / `x86_64-pc-windows-msvc`，默认本机）。支持平台别名：`win-x64`、`win-x86`、`win-arm64`、`linux-x64`、`linux-arm64`、`macos-x64`、`macos-arm64`，也可直接写 LLVM 三元组 |
| `--emit-ir` | 只生成 LLVM IR（.ll），不继续编译 |
| `--keep-ir` | 保留中间 IR 文件 |
| `--prep-only` | 只做预处理（tie-prep）并打印识别结果 |
| `--lsp` | 以语言服务器模式运行（读 stdin 的 LSP 消息、写 stdout，等价于 `tie-lsp`） |
| `-h, --help` | 显示帮助 |

流程：`tie-prep` 预处理（清理代码 + 识别文件类型）→ 按角色自动转交工具链
（`logic` → 编译为可执行文件；`library` → 编译为静态库 `.a`；`data`/`ui`/`db` → 对应工具链，后续版本）。

库编译示例（`// tie:library` 角色，定义函数不定义 main）：

```bash
tie examples/lib_math.tie          # → examples/lib_math.a（经 clang -c 生成 .o，llvm-ar rcs 打包）
```

子工具可单独使用：

- `tie-prep <file.tie>` —— 纯预处理
- `tie-frontend <file.tie>` —— 前端三阶段（词法/语法/语义），带 `--tokens`/`--ast`/`--check` 调试视图
- `tie-lsp` —— 语言服务器（LSP over stdio），向编辑器提供诊断 / hover / 跳转定义 / 补全，可与 VSCode 等配合（等价于 `tie --lsp`）
- `tie-llvm <file.tie>` —— 直接编译（不经过角色分派）
- `tie-interp <file.tie>` —— 直接解释执行

REPL 自举：REPL 外壳 `repl/repl.tie` 用 tie 语言自身编写（`print` + `read_line` + `eval`），
经 tie-llvm 编译并链接 tie-interp 静态库（C ABI 桥）生成 `repl.exe`。构建：

```bash
cargo build --release -p tie-interp          # 产出 target/release/tie_interp.lib
target/release/tie-llvm.exe repl/repl.tie    # 链接 interp 库生成 repl/repl.exe
```

`tie` 无参数时按 `TIE_REPL_EXE` → tie.exe 同目录 → 当前目录查找 repl.exe。

## 工程结构

```text
tie/
├── crates/
│   ├── tie-prep/      预处理：清理代码、提取头、识别文件角色（logic/ui/db/data/library）
│   ├── tie-frontend/  前端：词法（含 ASI）→ 语法 → 语义（符号表/类型检查），自研；独立 CLI 可调试
│   ├── tie-llvm/      中端+后端驱动：AST → LLVM IR 文本生成；调用 opt/clang/lld
│   ├── tie-lsp/       语言服务器：JSON-RPC 2.0 over stdio，复用前端三阶段提供诊断 / hover / 跳转定义 / 补全
│   ├── tie-interp/    解释执行：树遍历求值 AST + C ABI 桥（staticlib），REPL 自举核心
│   └── tie/           CLI 主入口：角色分派调度器 + REPL（启动 repl.exe）
├── repl/repl.tie     REPL 外壳（tie 语言自写，自举；编译链接 tie-interp 静态库）
├── docs/language.md   语法规范
├── examples/          示例程序（hello / wide / table / tuple / oop / 负例 oop_neg_* 等）
└── Cargo.toml         workspace（统一 edition/lints/release 配置）
```

## 编译流水线（四段式）

```
源码 .tie
   │
   ▼
┌──────────────┐
│ 预处理 Prep   │  tie-prep：清理代码 + 识别头（角色分派）
└──────────────┘
   │
   ▼
┌──────────────┐   ┌──────────────┐   ┌──────────────┐
│ 前端 Frontend │ → │ 中端 IR/Opt  │ → │ 后端 Backend │
│ 词法→语法→语义│   │ 生成 LLVM IR │   │ opt+clang/lld│
└──────────────┘   └──────────────┘   └──────────────┘
   │ AST             │ 优化交给 LLVM   │ 链接生成可执行
   └─ 自研实现        └─ opt            └─ 仅调用
```

| 阶段 | 内容 | 由谁实现 |
| --- | --- | --- |
| 预处理 | 清理代码、提取头、识别文件角色 | **tie-prep 自研** |
| 前端 | 词法分析（含 ASI）、语法分析（含头解析）、语义分析（符号表/类型检查） | **tie-frontend 自研** |
| 中端 | AST → LLVM IR 文本生成；中间优化 | IR 生成自研（tie-llvm）；**优化交给 `opt`** |
| 后端 | 汇编/目标文件生成、链接 | **交给 `clang`/`lld`** |

后续支持 `--backend=gnu` 切换到 GNU 工具链（gcc/ld）。

## 开发路线图

路线图按**架构**（正式发行版代号）组织：每个架构下有独立的 M 里程碑序列（从 M1 起）。
正式发行前的开发归入**预开发版本**（Pre-release）。

### 预开发版本（M0–M4）

正式发行前的语言核心基础建设，为 Harbor 发行版奠定基础。

| 里程碑 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 词法（含 ASI）+ 语法 + 语义 + IR 生成 + LLVM 后端打通，跑通 `println`/算术/变量 | ✅ 完成 |
| M1 | 控制流 if/while/for、函数调用、string 处理 | ✅ 完成 |
| M2 | 复合类型（表/数组、元组）、`import`、头类型分派（data） | ✅ 完成 |
| M3 | class/OOP、库编译、`--target` 交叉 | ✅ 完成 |
| M4 | 运算符扩展：复合赋值（`+=`/`-=` 等）、位运算（`&`/`\|`/`^`/`<<`/`>>`）、三目 `?:`、自增自减 `++`/`--` | ✅ 完成 |

### Harbor（2026.1）

首个正式发行版架构——工具链第一次靠岸停泊，形成可交付的稳定形态。

| 里程碑 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 正式发行版基础：版本规则（年份.修订号）、内部代号（2026.1 "Harbor"）、工具链合集打包（`scripts/package.ps1` → zip） | ✅ 完成 |
| M1 | VSCode 插件：语法高亮 / 智能缩进 / 代码片段 + LSP 客户端（诊断 / hover / 跳转定义 / 补全），TypeScript 重构 | 进行中 |
