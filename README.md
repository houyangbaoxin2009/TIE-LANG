# tie

> ⚠️ **早期开发阶段**：语言设计与实现仍在快速演进，语法、语义与工具链随时可能变更，暂不建议用于生产。

tie 是一门**通用编程语言**：用一门语言写逻辑、写界面、写数据库、当数据交换格式。
同一个 `.tie` 文件扮演什么角色，由文件头（Header）声明——四段式架构（预处理 → 前端 → 中端 → 后端）。

> 语法规范见 [docs/language.md](docs/language.md)；本文件是工程入口（用法、结构、流水线、路线图）。

## 文档目录

| 文档 | 内容 |
| --- | --- |
| [README.md](README.md) | 本文件：工程入口（快速开始、CLI、结构、流水线、路线图） |
| [docs/language.md](docs/language.md) | 语法规范：文件结构、类型系统、语句/控制流、函数、语法速查表 |

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
tie <input.tie> [-o output] [-O0|-O1|-O2|-O3] [--emit-ir] [--keep-ir] [--prep-only]
tie             # 无参数 → 进入 REPL 交互模式（tie-interp 解释执行）
```

| 选项 | 说明 |
| --- | --- |
| `-o <file>` | 指定输出可执行文件路径（默认：输入同名 `.exe`） |
| `-O0..-O3` | 优化级别（默认 `-O2`），映射到 `opt -O2` |
| `--emit-ir` | 只生成 LLVM IR（.ll），不继续编译 |
| `--keep-ir` | 保留中间 IR 文件 |
| `--prep-only` | 只做预处理（tie-prep）并打印识别结果 |
| `-h, --help` | 显示帮助 |

流程：`tie-prep` 预处理（清理代码 + 识别文件类型）→ 按角色自动转交工具链
（`logic`/`library` → tie-llvm 编译；`data`/`ui`/`db` → 对应工具链，后续版本）。

子工具可单独使用：

- `tie-prep <file.tie>` —— 纯预处理
- `tie-llvm <file.tie>` —— 直接编译（不经过角色分派）
- `tie-interp <file.tie>` —— 直接解释执行

## 工程结构

```text
tie/
├── crates/
│   ├── tie-prep/      预处理：清理代码、提取头、识别文件角色（logic/ui/db/data/library）
│   ├── tie-frontend/  前端：词法（含 ASI）→ 语法 → 语义（符号表/类型检查），自研
│   ├── tie-llvm/      中端+后端驱动：AST → LLVM IR 文本生成；调用 opt/clang/lld
│   ├── tie-interp/    解释执行（占位，REPL 用）
│   └── tie/           CLI 主入口：角色分派调度器 + REPL
├── docs/language.md   语法规范
├── examples/          示例程序（hello.tie / wide.tie / test_wide.tie）
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

## 早期开发路线图

| 里程碑 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 词法（含 ASI）+ 语法 + 语义 + IR 生成 + LLVM 后端打通，跑通 `println`/算术/变量 | ✅ 完成 |
| M1 | 控制流 if/while/for、函数调用、string 处理 | ✅ 完成 |
| M2 | 复合类型（表/数组）、`import`、头类型分派（data） | 进行中 |
| M3 | class/OOP、库编译、`--target` 交叉 | 规划 |
| M4 | `// tie:ui` 界面、`// tie:db` 数据库、GNU 后端、性能优化 | 规划 |
