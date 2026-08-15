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
tiec 实现）。

> 语法规范见 [docs/language.md](docs/language.md)；本文件是工程入口（用法、结构、流水线、路线图）。

## 文档目录

| 文档 | 内容 |
| --- | --- |
| [README.md](README.md) | 本文件：工程入口（快速开始、CLI、结构、流水线、路线图） |
| [docs/language.md](docs/language.md) | 语法规范：文件结构、类型系统、语句/控制流、函数、面向对象、语法速查表 |
| [docs/language-comparison.md](docs/language-comparison.md) | 语言对比报告：tie vs 42 种语言的特性/标准库全景对比（工业级全栈定位、丰俭由人伸缩谱） |
| [docs/tiec.md](docs/tiec.md) | tiec 自举编译器文档：tiec 是什么、自举链、快速开始、CLI 用法、运行时依赖、架构与进度 |
| [docs/tie-script.md](docs/tie-script.md) | tie:script 模块协议：tie 脚本的注册/调用机制、模块约定、协议文本格式、三层调用入口（Rust/CLI/tie 程序内） |
| [docs/ai-guide.md](docs/ai-guide.md) | AI 教学指南：语言用法 + 负例 + 编译器架构（教 AI 用/开发 tie） |
| [docs/prompt-pack.md](docs/prompt-pack.md) | 可粘贴 Prompt 包：自包含简介，直接发给任何 AI |
| [docs/plans/](docs/plans/) | 后续里程碑设计规划（switch 模式匹配 / 单文件命名空间 / 统一 func 写法 / 动态库编译 / 包管理器 / 算法库分类 / 嵌入式基础层 rdu / **泛型系统（已实现**）） |
| [CHANGELOG.md](CHANGELOG.md) | 版本变更记录（按里程碑） |

## 快速开始

前置依赖：LLVM 工具链（`opt`、`clang`、`lld`，编译链路的后端部分调用它们）；
**发行版 zip 已内置精简 LLVM 工具链（`bin/llvm/`），解压即用，无需单独安装 LLVM**。
LLVM 工具发现顺序：`TIE_LLVM_HOME\bin` → tie.exe/tiec.exe 同目录 `llvm\bin` → `PATH`
→ 常见安装目录（`D:\LLVM\bin`、`C:\Program Files\LLVM\bin`、`C:\LLVM\bin`）。

编译器为 tie 语言自写（tiec，见 `compiler/`），无需 Rust 工具链（Rust 参考编译器
已归档至独立仓库 [tiec_rust](https://github.com/tie-lang/tiec_rust)）。

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
- **默认角色**：无声明时默认 `logic`（可执行文件，需 `func main()`）；
- **文件名约定**：`xxx.<角色>.tie`（如 `app.script.tie`、`schema.db.tie`）可作为默认角色
  （头部声明优先；文件名与头部不一致时**警告并采用头部声明**）；
- **角色分派**：

| 角色 | 工具链 |
| --- | --- |
| `logic` / `script` | 编译为可执行文件 |
| `class` / `type` | 编译为静态库 `.a` |
| `ir` | 生成 LLVM IR（.ll），不继续编译 |
| `data` / `ui` / `db` / `port` | 对应工具链，挂接点未实现（提示） |

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

| 选项 | 说明 |
| --- | --- |
| `-o <file>` | 指定输出文件路径（logic/script 默认：输入同名 `.exe`；class/type 默认：输入同名 `.a`；单文件编译时生效） |
| `-O0..-O3` | 优化级别（默认 `-O2`），映射到 `opt -O2` |
| `--target <三元组>` | 交叉编译目标（如 `win-x64` / `x86_64-pc-windows-msvc`，默认本机）。支持平台别名：`win-x64`、`win-x86`、`win-arm64`、`linux-x64`、`linux-arm64`、`macos-x64`、`macos-arm64`，也可直接写 LLVM 三元组 |
| `--emit-ir` | 只生成 LLVM IR（.ll），不继续编译 |
| `--keep-ir` | 保留中间 IR 文件 |
| `--prep-only` | 只做预处理（tie-prep）并打印识别结果 |
| `--config <file>` | 指定协调统筹配置文件（默认查当前目录 `tie.config`，无则全关闭） |
| `--module <file.tie>` | tie-prep：挂载自定义 tie 转换器模块（顶层 `process(src)->string`），输出为模块转换结果（Harbor M3 可扩展性） |
| `--lsp` | 以语言服务器模式运行（读 stdin 的 LSP 消息、写 stdout，等价于 `tie-lsp`） |
| `-h, --help` | 显示帮助 |

**包管理器子命令（Harbor M6，tie 语言自写）**：Rust 入口识别首个参数为子命令
（且非 `.tie` 文件）后，转交 tie 语言自写的 `pkg.exe`（`pkg/main.tie` 经
tie-llvm 编译链接 interp 库生成），完整 CLI 逻辑全部在 tie 侧：

| 子命令 | 说明 |
| --- | --- |
| `tie init <项目名>` | 初始化项目（生成 tie.pkg 清单 + main.tie 模板） |
| `tie add <依赖>` | 添加依赖（`path:./lib_math` 本地源 / `git+https://...` git 源 / `log@1.0.0` 或 `log@^1.2` registry 约束） |
| `tie remove <包名>` | 移除依赖 |
| `tie install` | 解析 + 拉取全部依赖到 `.tie/deps/`，生成/校验 tie.lock（三源：path/git/registry） |
| `tie update [包名]` | 重新解析依赖并更新 tie.lock |
| `tie build` | 编译项目（调用 tie 编译器） |
| `tie run` | 编译并运行项目 |
| `tie publish` | 打包发布（`.tie/dist/<name>-<version>.tar.gz` + `git tag v<version>` + push） |
| `tie search <关键字>` | 搜索注册表 index.tie（`TIE_REGISTRY` 可指定基址） |
| `tie info <包名>` | 查询注册表包的最高版本与描述 |
| `tie help` | 显示包管理器帮助 |

端到端演示见 [examples/pkg_demo.md](examples/pkg_demo.md) 与 `examples/demo_pkg/`。
构建 `pkg.exe`：`compiler/tiec.exe pkg/main.tie -o pkg/pkg.exe`（自举：tiec 编译，0-Rust）。

流程：`tie-prep` 预处理（清理代码 + 识别文件类型）→ 按角色自动转交工具链
（`logic`/`script` → 编译为可执行文件；`class`/`type` → 编译为静态库 `.a`；`ir` → 直接生成 LLVM IR（.ll）；
`data`/`ui`/`db`/`port` → 对应工具链，挂接点未实现）。

**多文件并行编译（Harbor M3 协调统筹增强）**：配置文件（`type tie<data>` 格式，键 `advanced` /
`cache`）开启 `advanced.enabled = true` 后，可一次编译多个输入文件（目录输入自动展开
其中全部 `.tie` 文件）——按文件分片、三阶段（预处理 → 前端+IR → 后端）并行 + 阶段屏障，
阶段间用缓存池（LRU，`memory` 进程内 / `file` 磁盘目录）中转中间产物。`threads` 为 0
时按 CPU 核数自动；单文件编译行为与原版本完全一致。

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
├── repl/repl.tie     REPL 外壳（tie 语言自写，自举；编译链接 tie-interp 静态库）
├── compiler/         自举 v2 编译器（tie 语言自写，T2–T5 阶段产物）：
│                     - frontend/：词法/语法/语义分析器（T2.4–2.6，自举前端）
│                     - middle/：tie-IR 列式表 + 类型系统（T2.2–2.3）
│                     - backend/：irgen（AST→tie-IR）+ llvmgen（tie-IR→.ll）+
│                       toolchain（opt/clang/llvm-ar 驱动，T2.8/T3.1）
│                     - interp/：解释器（value/session/interp/env，T4.1–4.2）
│                     - driver.tie → tiec.exe：完整 CLI 编译器（T3.2）
│                     - repl.tie → repl.exe：tie 自写解释器 REPL（T4.3）
│                     - tests/interp/：解释器行为测试套件（T4.4，11 文件 198 断言）
├── prep/core.tie     预处理器核心模块（tie 语言自写：头部提取/角色判定/正文重建；Harbor M3 自举，编译期内嵌 tie-prep）
├── prep/indent.tie   转换器模块示例（制表符→4 空格；证明扩展性——新增转换器只需写 tie 模块，`tie-prep --module` 挂载）
├── prep/rename_tcmsg_to_log.tie  转换器模块实战（tcmsg → log 批量改名，tie 语言自写完成真实重构任务）
├── std/              标准库（tie 语言自写，均命名空间形式调用，基于语言底座原语；M4 补全大小写转换/join/repeat/trim 拆分/gcd/lcm/pow_i/sprintf 占位符/csv_write/浮点与字符串断言；2026-08-12 大规模扩展为 20+ 模块）：
│                     - 文本/编码：string（trim/slice/split/join/大小写）、utf（UTF-8 码点/字节工具：codepoint/byte_len/hex_escape）、ascii（字符分类与转换）、encoding（base64/hex/url percent）、regex（正则包装）、json（JSON 解析/序列化，节点式访问器）
│                     - 数据结构/算法：sort（排序数组/二分）、collection（最小堆/栈/KMP）、crypto（crc32/fnv1a 校验）、optsearch（排序/背包/N 皇后等）、graph（图算法）、linalg（矩阵）、exmath（高级数学：霍夫曼/素数/快速幂）、math（基础数学）、radix（进制转换）
│                     - IO/系统：fs（文件系统，Rust std::fs 风格完整 API：read_to_string/write/append/exists/size/is_file/is_dir/remove_file/create_dir_all/read_dir/rename/write_lines 等，**UTF-8 路径安全**——中文路径原生支持）、path（路径：basename/dirname/ext/stem，extern 桥）、args（命令行参数）、http（HTTP GET + URL 编码）、random（随机）、bytes（字节表）、time（计时）、process（进程）、intern（字符串池）、version（版本比较）、format（格式化）、csv、assert（断言）
│                     典型调用：assert.assert / str.split / format.sprintf / csv.csv_write / exmath.huffman_encode / radix.to_str / fs.read_lines / json.parse / utf.byte_len / coll.heap_push
│                     2026-08-15 P0 补全：net（TCP/UDP 网络库：listen/accept/connect/send/recv、udp_bind/send/recv、close）、http_server（HTTP/1.1 服务端框架：请求解析/响应构造/if 链路由）、set（HashSet 语义：有序表+二分，i64/string 双 API）、deque（VecDeque 双端队列）、db（tie:data 数据载体：表 ↔ tie:data 文本）
├── ext/              扩展库（Extension，tie 语言自写，随发行版内置，依赖 std 与语言底座：log 控制台信息库——i18n 消息系统，M4 增强带参消息/级别体系/stderr 通道/批量登记/多级回退；2026-08-12 新增 test 测试框架（断言收集+统计）、bench 基准计时、tui 终端装饰（进度条/文本框，无 ANSI——语言无 \xHH 转义限制）、config 配置解析（KV/INI）、pretty 文本表格；另有 cache/compress/ml/registry/codec（brotli/jpeg/lz4/zstd）；2026-08-15 新增 vecsearch（向量检索：Flat 精确索引——L2/余弦距离 + 展平存储 add/remove/get/search top-k））
├── rdu/              嵌入式基础层（Rudimentary，tie 语言自写，随发行版内置，独立于 std/ext 的第三层——专为嵌入式 MCU/裸机 freestanding 定制：无栈纪律即零原语调用/零动态内存/零数组（禁止一切数组与表功能：动态表、定长表、字面量表、下标读写、table_* 原语）/无递归/无全局可变状态/零运行时依赖，只用 i64/f64/bool 标量，不依赖 std/ext、不 import 任何东西，编译出的 .a 可被裸机直接链接）：
│                     - bits（位操作：set/clear/toggle/test/rol/ror/bswap16/bswap32/bswap64/popcount/clz/ctz）
│                     - math（基础数学纯标量：abs/abs_f/max_i/min_i/max_f/min_f/clamp/clamp_i/is_odd/is_even/avg_f/sign_i/deg_to_rad/rad_to_deg/gcd/lcm/pow_i）
│                     - ascii（ASCII 码点分类/转换：is_digit/is_alpha/is_alnum/is_lower/is_upper/is_print/is_space/to_lower/to_upper）
│                     - crc（增量式校验：crc8/16/32 的 init/update + crc32_final，CRC-32/IEEE 802.3 逐位无查表；fnv1a 的 init/update）
│                     - fixed（Q16.16 定点数：fixed_mul/fixed_div/fixed_floor/fixed_frac）
│                     - rnd（确定性伪随机：xorshift64，调用方持状态）
│                     典型调用：rdu_crc.crc32_init() / rdu_bits.popcount() / rdu_fixed.fixed_mul() / rdu_rnd.xorshift64()
│                     - rdb（嵌入式 db 子集，2026-08-15：纯标量查询条件——cond_eq/cond_range/cond_gt/cond_lt/cmp_i64，tieDB API 的嵌入式孪生子集）
├── tieDB/            tieDB 统一数据库 API（tie 语言自写，2026-08-15）：tiedb.connect/collection/insert/search/remove/size/save/load（本地向量集合 + zd 压缩持久化；ext/vecsearch Flat 索引；规划见 docs/plans/tiedb.md）；persist/zd.tie（tie:zd 序列化：MessagePack 思路 + Protobuf 参考，纯 tie 实现——varint/fixint/定宽/字符串/表/map/record 字段编码 + save/load）
├── pkg/              包管理器（tie 语言自写，M6 E1/E2 + E3/E4）：main.tie CLI 入口
│                     （init/add/remove/install/update/build/run/publish/search/info/help
│                     子命令分派）+ manifest.tie（tie.pkg 清单解析）+ deps.tie（path/git/
│                     registry 三源安装 + 递归依赖解析 + 锁文件落地）+ fetch.tie（git
│                     拉取 + registry 基址/版本选择/下载解压）+ lock.tie（tie.lock 生成/
│                     解析/校验）+ publish.tie（打包 tar.gz + git tag/push）+ search.tie
│                     （注册表搜索/信息查询），经 tiec 编译链接 interp 库生成 pkg.exe
├── docs/language.md   语法规范
├── docs/tie-script.md tie:script 模块协议（eval/eval_call 机制、模块约定、协议文本、三层调用入口）
├── docs/plans/        后续里程碑设计规划（switch 模式匹配 / 单文件命名空间 / 统一 func 写法 / 动态库编译 / 算法库分类 / 嵌入式基础层 rdu）
├── examples/          示例程序（hello / wide / table / tuple / oop / 负例 oop_neg_* 等）
└── .tiec.md           tiec 自举编译器文档（docs/tiec.md）
```

> Rust 参考编译器（种子）已归档至独立仓库 [tiec_rust](https://github.com/tie-lang/tiec_rust)（2026-08-15 剔除，主仓库 0-Rust）。

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
| M3 | struct/OOP、库编译、`--target` 交叉 | ✅ 完成 |
| M4 | 运算符扩展：复合赋值（`+=`/`-=` 等）、位运算（`&`/`\|`/`^`/`<<`/`>>`）、三目 `?:`、自增自减 `++`/`--` | ✅ 完成 |

### Harbor（2026.1）

首个正式发行版架构——工具链第一次靠岸停泊，形成可交付的稳定形态。

| 里程碑 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 正式发行版基础：版本规则（年份.修订号）、内部代号（2026.1 "Harbor"）、工具链合集打包（`scripts/package.ps1` → zip） | ✅ 完成 |
| M1 | VSCode 插件：语法高亮 / 智能缩进 / 代码片段 + LSP 客户端（诊断 / hover / 跳转定义 / 补全），TypeScript 重构 | ✅ 完成 |
| M2 | 标准库：`std/`（文件 / 字符串 / 断言 / CSV / 格式化）+ `math`（数学函数）+ 20+ 语言底座原语 + **`log` 控制台信息库（i18n，M4 起迁入 ext/ 扩展库）** + **默认值参数**（可选参数省略时用字面量默认值）；M2.1.2 起 std 库全部采用命名空间形式（`assert.assert` / `str.split` / `math.abs`）；M2.1.6 起命名空间内函数去 `str_` 前缀（`str.split` / `str.trim`）且方法定义统一 `func` 关键字（`static func`）；M2.1.7 起单文件命名空间成为真模块边界——命名空间内函数默认私有（`pub func` 显式导出）、`using` 引入后裸调用、`import as` 别名唯一入口；M2.1.8 起 **struct 数据与逻辑分离**（`class` 改名 `struct` 为纯数据，方法移出为绑定 struct 名的命名空间函数，`obj.method()` 转发且接收者按引用传递；`this`/`static` 废弃），为自举与生态奠定基础 | ✅ 完成 |
| M3 | 预处理器自举 + 协调统筹：完全用 tie 语言重写 `tie-prep`（编译器自举），并增强为多文件并行编译（配置文件 + 缓存池 + 三阶段并行分片） | ✅ 完成（阶段一：预处理器自举——核心逻辑 tie 语言化 `prep/core.tie`，Rust 壳仅解释执行；阶段二：协调统筹增强——`tie.config` 配置文件 + 缓存池 + 多线程并行分片编译） |
| M4 | 标准库重构 + 扩展库分层：补全常用函数（str 大小写/join/repeat/trim_start/trim_end、math gcd/lcm/pow_i、format sprintf 占位符、csv_write、assert 浮点与字符串断言）+ 内部 using 简化 + **顶层持久变量**（var/const 全局，纯 tie 表达消息状态）+ log 增强（带参消息/级别/stderr/批量登记/多级回退，移入 **ext/** 扩展库）+ 修复 using 表元素解析/调用结果下标等编译器边界 | ✅ 完成 |
| M5 | 动态库编译：`type tie<class>` 输出 `.dll`（win）/ `.so`（linux）——库公有函数 `dllexport` 导出、跨语言调用约定（标量直传/字符串 C ABI 桥）、C 程序 LoadLibrary/dlopen 消费示例（见 docs/plans/dynamic-library.md） | 📋 规划 |
| M6 | 包管理器（E1/E2 ✅ + E3/E4 ✅）：tie 语言自写 CLI（`pkg/main.tie` → `pkg.exe`，自举）+ tie.pkg 清单解析（`manifest.tie`）+ 三源安装（path 复制 / git 浅克隆 / registry 下载解压，`deps.tie` + `fetch.tie`）+ tie.lock 锁文件幂等恢复（`lock.tie`）+ 递归依赖解析（去重/冲突检测）+ `tie update`/`publish`（打包 tar.gz + git tag/push，`publish.tie`）/`search`/`info`（注册表查询，`search.tie`）；tiec 编译生成 pkg.exe（0-Rust 自举）；`init → add（path/git/registry）→ install → build/run` 端到端跑通（见 examples/pkg_demo.md） | ✅ 完成（E1–E4） |

### 自举（Self-hosting，进行中）

**目标**：前端（词法/语法/语义）+ IR 生成完全用 tie 语言重写，tie 编译器能编译自身。
规划见 [docs/plans/self-hosting.md](docs/plans/self-hosting.md)；自举 v2 阶段规划见
`.omo/plans/self-hosting-v2.md`。

**前置能力已落地（2026-08-10）**：

| 障碍 | 方案 | 状态 |
| --- | --- | --- |
| 无 continue/break | E1 continue/break 语句 + E5 循环标签（`L: while` / `break L`） | ✅ 完成 |
| LLVM alloca 栈溢出（0xC00000FD） | F1 alloca 提升到 entry block + 全局重编号 | ✅ 完成 |
| 表参数元素类型静态未知 | A1 `table<T>` 类型参数（四层支持 + 实参校验） | ✅ 完成 |
| 表参数拼接 UB（段错误） | A6 表实参展开为动态表再传指针 | ✅ 完成 |
| 定长表变量实参 IR 缺陷（`[N x T]` 传 ptr） | E0 定长表变量实参展开为动态表（A6 同路径） | ✅ 完成 |
| switch 线性比较链 | C5 整数 case 生成 LLVM switch 跳转表 | ✅ 完成 |
| 字符串 id 表不可用（符号表） | E3 键值表 `map`/`map<T>`：字面量/下标读写/实参（D3 排序数组过渡并存） | ✅ 完成 |
| 无嵌套表（AST 树形结构） | E1 嵌套表 `table<table<T>>`：`>>` 类型参数分裂 + 嵌套下标链 + ptr 元素桥 | ✅ 完成 |
| 无 enum / 无函数指针 | B1 tag 表 AST + C1 字符串分派（前提已就绪，编码修订见规划文档 §3.5） | ✅ enum 完成（2026-08-15）；C1 函数指针仍规划 |

**自举 v2 阶段 0 语言特性（2026-08-10 收官 e816457）**：为 tie 语言自举
补足的五项语言/标准库能力（T0.3–T0.7），全部已实现并验收：

| 特性 | 内容 | 验收 | 状态 |
| --- | --- | --- | --- |
| T0.3 ref 表参数按引用传递 | `ref table<T>` 形参：内容修改与变量重绑定都写回调用方实参槽 | `tests/language/byref_table.tie`（3/42/99/100/1/7） | ✅ 完成 |
| T0.4 顶层表全局变量 | 顶层 `var g: table<T>;`：跨函数持久动态表，可作 ref 实参；const 全局表暂不支持 | `tests/language/global_table.tie`（2/1/2） | ✅ 完成 |
| T0.5 map 排序键二分 | map 查找由线性扫描升级为排序键二分（strcmp 字节序），10k 查找 ~2295×；输出按键排序 | `scripts/bench/map-bench.tie` | ✅ 完成 |
| T0.6 字符串池 intern 库 | `std/intern.tie`：`intern(s)->i64` / `lookup(id)->string` / `interned_len()`，符号比较 O(1) id 化 | `tests/language/intern.tie` | ✅ 完成 |
| T0.7 extern 函数声明 | 顶层 `extern fn` 声明外部 C 符号（IR `declare`，clang 链接 libc）；`std/process.tie` 进程原语 | `tests/language/extern_decl.tie`（rand/0/3/hello） | ✅ 完成 |

**自举 v2 阶段 1–5 进度（2026-08-12）**：

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| T1.1–T1.5 | 词法/语法原型 + G1 性能闸门 | ✅ 完成 |
| T2.4–T2.6 | 完整词法/语法/语义分析器（正式版，tie 重写前端） | ✅ 完成（5536151/a9a8d10） |
| T2.7 | 错误 golden corpus（63 触发 + 语义检查补齐） | ✅ 完成（12b350a） |
| T2.8 | LLVM IR 文本生成端到端（irgen + llvmgen，opt+clang 编译运行） | ✅ 完成（eb15f93） |
| T2.9 | tiec 组装（driver-lite）+ 行为等价回归（等价率 100%） | ✅ 完成（e780353） |
| T3 | tiec 完整编译器：工具链驱动 + CLI + 可复现构建（Brepro） | ✅ 完成（c56c30a） |
| T4.1–T4.4 | 解释器 tie 化：core（eval/eval_call）+ 环境原语 + REPL parity + 测试移植（198 断言） | ✅ 完成（0830ef7/d3adfc0/53fce7d） |
| T4.5–T4.6 | tie 运行时静态库（std/runtime.a 替代 Rust interp 链接）+ G3 闸门（0-Rust） | ✅ 完成（747ef1a/db585f2） |
| T5.1 | tiec 前端全局表修复（自编译成功）+ G4 性能基准（ratio 1.007） | ✅ 完成（dc71e11） |
| T5.3 | tiec 性能优化（消除重复前端/AST 内存传递/intern 二分/build_protocol 分治/print_err 清理）——G4 ratio 6.9 → 1.09，hello 反超 Rust | ✅ 完成（本批） |
| T5 后续 | irgen 最小集扩展（前端已就绪，扩展后 G4 覆盖自动扩全） | 📋 进行中 |
