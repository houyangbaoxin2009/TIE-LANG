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
| [docs/cli.md](docs/cli.md)                                 | CLI 用法：主入口选项、包管理器子命令、多文件并行编译、库编译、REPL 自举                                                          |
| [docs/language.md](docs/language.md)                       | 语法规范：文件结构、类型系统、语句/控制流、函数、面向对象、语法速查表                                                               |
| [docs/language-comparison.md](docs/language-comparison.md) | 语言对比报告：tie vs 42 种语言的特性/标准库全景对比（工业级全栈定位、丰俭由人伸缩谱）                                                  |
| [docs/tiec.md](docs/tiec.md)                               | tiec 自举编译器文档：tiec 是什么、自举链、快速开始、CLI 用法、运行时依赖、架构与进度                                                 |
| [docs/tie-script.md](docs/tie-script.md)                   | tie:script 模块协议：tie 脚本的注册/调用机制、模块约定、协议文本格式、三层调用入口（Rust/CLI/tie 程序内）                               |
| [docs/ai-guide.md](docs/ai-guide.md)                       | AI 教学指南：语言用法 + 负例 + 编译器架构（教 AI 用/开发 tie）                                                          |
| [docs/prompt-pack.md](docs/prompt-pack.md)                 | 可粘贴 Prompt 包：自包含简介，直接发给任何 AI                                                                      |
| [NEW.md](NEW.md)                                           | 发行版新鲜事：本发行版的新功能与特色速览                                                                              |
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

## CLI 用法

主入口 `tie`（四段式调度器）与包管理器子命令的完整用法见 [docs/cli.md](docs/cli.md)：
主入口选项表、`tie init/add/install/publish` 等包管理命令、多文件并行编译、
库编译（静态库 `.a`/`.lib`）、子工具与 REPL 自举构建。
