# tiec —— tie 自举编译器

> ⚠️ **早期开发阶段**：tiec 为自举 v2 的产物，随实现持续演进，功能与限制以本文件与源码为准。

tiec 是 tie 语言 **100% 自写**的完整命令行编译器，是自举 v2 计划（`compiler/` 目录）的最终交付物。
它由 tie 语言自身编写，经 Rust 种子编译器编译生成，随后可以编译自身，形成自举闭环。
其命令行行为与消息格式对齐 Rust 老编译器 `crates/tie-llvm`，可作为其替代品使用。

## 1. tiec 是什么

tiec（tie compiler）是一套完整的前端到后端编译流水线，全部代码用 tie 语言编写：

| 能力 | 说明 |
| --- | --- |
| 语言 | 全部源码为 `.tie` 文件（`compiler/` 目录），零 Rust 代码 |
| 前端 | 词法分析（lexer）、语法分析（parser）、语义分析（semantic/checker） |
| 中端 | tie-IR 中间表示（列式表）、AST 到 tie-IR 生成（irgen）、LLVM IR 文本生成（llvmgen） |
| 后端 | 调用 LLVM 工具链（`opt` / `clang` / `llvm-ar` / `lld`）完成优化、汇编与链接 |
| 附赠 | tie 自写解释器（`compiler/interp/`）与 REPL（`compiler/repl.tie`） |
| 行为对齐 | 参数解析、角色分派、消息格式、退出码均对齐 Rust `crates/tie-llvm` |

编译流水线：

```
源码 .tie
   │
   ▼
┌──────────────┐   ┌─────────────────────────┐   ┌──────────────────┐
│ frontend     │ → │ middle + backend 前段    │ → │ backend toolchain │
│ lexer→parser │   │ irgen → llvmgen          │   │ opt / clang / ar  │
│ →semantic    │   │ （tie-IR → LLVM IR 文本） │   │ → 可执行 / .a     │
└──────────────┘   └─────────────────────────┘   └──────────────────┘
```

tiec 把 `.tie` 源文件编译为原生可执行文件（`logic` 角色）或静态库（`library` 角色），
中间产物经 `opt` 优化、`clang` 汇编链接，最终落在目标平台的可执行文件上。

## 2. 与老工具链的关系与自举链

tie 语言目前有两代编译器并存：

| 名称 | 实现语言 | 角色 |
| --- | --- | --- |
| `tie-llvm.exe` | Rust（`crates/tie-llvm`） | 老编译器，Rust 种子，自举链的起点 |
| `tiec.exe` | tie（`compiler/driver.tie` 编译而来） | 新编译器，自举 v2 产物，替代 tie-llvm |

自举链（bootstrap chain）如下：

```
① Rust 种子 tie-llvm.exe 编译 compiler/driver.tie ──► tiec.exe   （bootstrap 界限）
② tiec.exe 编译 compiler/driver.tie              ──► tiec2.exe   （二阶，自举闭环）
③ tiec2.exe 编译 compiler/driver.tie             ──► tiec3.exe   （再自举验证）
```

- **第 ① 步是 bootstrap 界限**：tiec 由 Rust 种子编译产生，这是整个工具链唯一的 Rust 接触点；
- **第 ② 步**：tiec 编译自身成功，证明它可以自举；
- **第 ③ 步**（T5.2 实测打通）：tiec2 再次编译自身并正确编译 `hello.tie`，二阶闭环验证通过；
- **G3 闸门（0-Rust）验证 PASS**：种子界限之后，编译、运行、REPL 全链路不再依赖 Rust。

## 3. 快速开始

### 发布包内已含编译好的二进制

发布包 `bin/` 下已附带现成的 `tiec.exe`（约 2.5 MB）与 `tiec2.exe`（约 2.5 MB），
均已通过冒烟验证（编译 `examples/hello.tie` 并运行输出正确）。解包后可直接使用：

```bash
bin\tiec.exe examples\hello.tie     # 生成 examples\hello.exe
examples\hello.exe                  # 运行
```

发行 zip 同时内置精简 LLVM 工具链（`bin/llvm/`：clang / opt / llvm-ar / lld-link、
头文件与许可文本），解压即用，无需单独安装 LLVM。

### 从源码构建 tiec

前置依赖：Rust（构建种子用）、LLVM 工具链（`opt`、`clang`、`llvm-ar`、`lld`）。

```bash
# ① 构建 Rust 种子
cargo build --release

# ② 用种子编译 tiec 自身
target\release\tie-llvm.exe compiler\driver.tie -o compiler\tiec.exe

# ③ 二阶自举：tiec 编译自身
compiler\tiec.exe compiler\driver.tie -o compiler\tiec2.exe
```

LLVM 工具发现顺序：`TIE_LLVM_HOME\bin` → tie.exe/tiec.exe 同目录 `llvm\bin`（Rust 侧）→ `PATH`
→ 固定目录（`D:\LLVM\bin`、`C:\Program Files\LLVM\bin`、`C:\LLVM\bin`）。
发行版 zip 内置精简 LLVM（`bin/llvm/`），`TIE_LLVM_HOME` 指向它即开箱即用。
链接时若缺少运行时静态库，需要先构建 `std/runtime.a`（见第 5 节）。

## 4. CLI 用法

```
tiec <input.tie> [-o <out>] [-O0|-O1|-O2|-O3] [--target <三元组>]
                 [--emit-ir] [--keep-ir] [--prep-only] [--config <f>] [--help]
```

### 选项

| 选项 | 说明 |
| --- | --- |
| `<input.tie>` | 输入源文件（必需） |
| `-o <file>` | 输出文件路径。logic 角色默认输出输入同名 `.exe`，library 角色默认输出同名 `.a` |
| `-O0` / `-O1` / `-O2` / `-O3` | 优化级别，映射到 `opt -O{0..3}`，默认 `-O2` |
| `--target <三元组>` | 交叉编译目标（如 `x86_64-pc-windows-msvc`），默认本机 |
| `--emit-ir` | 只生成 LLVM IR（`.ll`），不继续编译 |
| `--keep-ir` | 保留中间 IR 文件（`.ll` / `.opt.ll`） |
| `--prep-only` | 只做头部识别并打印识别结果，不编译 |
| `--config <f>` | 协调统筹配置文件（单文件编译时暂忽略） |
| `--help` / `-h` | 显示帮助 |

### 退出码

| 退出码 | 含义 |
| --- | --- |
| `0` | 编译成功 |
| `1` | 编译失败（源码读取失败 / 语法错误 / 语义错误 / 后端错误） |
| `2` | 参数错误（非法选项、缺少参数等） |

### 角色识别

tiec 通过**头部扫描**识别源文件角色：直接读取源文件头部的注释行，
查找 `// tie:xxx` 指令（不依赖 prep 模块的 import 机制）。

| 头部指令 | 角色 | 行为 |
| --- | --- | --- |
| `// tie:logic` | 逻辑代码 | 编译为可执行文件 |
| `// tie:library` | 库文件 | 编译为静态库 `.a` |
| `// tie:data` / `// tie:ui` / `// tie:db` | 数据 / 界面 / 数据库 | 提示对应工具链未实现 |

未声明头时按 `logic` 处理。

### opt / target 优先级

```
CLI 显式（-O2 / --target）  >  头部（// tie:opt=N / // tie:target=）  >  默认（-O2 / 本机）
```

命令行显式指定的选项优先级最高；未显式指定时读取源文件头部的 `// tie:opt=` 与
`// tie:target=`；头部也没有则回落到默认值。

### 示例

```bash
tiec hello.tie                  # 编译 → hello.exe（默认 -O2）
tiec hello.tie -o out.exe -O0   # 指定输出与关闭优化
tiec lib_math.tie               # library 角色 → lib_math.a
tiec hello.tie --emit-ir        # 只生成 hello.ll
tiec hello.tie --keep-ir        # 编译并保留中间 IR
tiec hello.tie --prep-only      # 只打印角色识别结果
tiec --help                     # 帮助
```

## 5. 运行时依赖

tiec 链接用户程序时需要一个运行时静态库：

| 产物 | 说明 |
| --- | --- |
| `std/runtime.a` | tie 自写运行时静态库（T4.5）。由 `std/runtime.tie` 编译而来，提供 `tie_exec_code` / `tie_get_env` / `tie_time_now` 等桥符号，顶层裸函数不 mangle，与语言底座 `extern fn` 声明字节级匹配 |
| `tie_interp.lib` | 历史回退（Rust tie-interp 静态库）。`std/runtime.a` 不存在时作为备选链接 |

**G3 闸门验证**：移走 Rust `tie_interp.lib` 后，tiec 仍能编译并运行
`exec_code` / `time_now` / `get_env` 程序，运行时栈 Rust-free 检查通过。
`std/runtime.a` 是 0-Rust 链路的关键一环，链接用户程序时必需。

### LLVM 工具链依赖

- **工具发现顺序**：`TIE_LLVM_HOME\bin` → tie.exe/tiec.exe 同目录 `llvm\bin`（Rust 侧）→
  `PATH` → 固定目录（`D:\LLVM\bin`、`C:\Program Files\LLVM\bin`、`C:\LLVM\bin`）；
- **vendored 发行说明**：发行版 zip 内置精简 LLVM 工具链（`bin/llvm/`，含 clang / opt /
  llvm-ar / lld-link 与头文件），`TIE_LLVM_HOME` 指向 `bin/llvm` 即开箱即用，无需单独安装 LLVM；
- **`-fuse-ld=lld` 仅 vendored 场景生效**：clang 来自随包 LLVM（`TIE_LLVM_HOME` 已设置 / 命中
  同目录 `llvm\bin`）时，链接命令加 `-fuse-ld=lld`，让随包的 lld-link.exe 在无 MSVC/VS 的机器上
  完成链接；普通开发机（clang 来自 PATH / 固定目录，VS link.exe 可用）保持默认链接器。原因：
  lld 解析 Rust 静态库 tie_interp.lib 存在 CRT 缺陷（`undefined symbol: printf`），开发机必须保留 link.exe；
- **已知限制**：vendored 且无 MSVC/VS 的环境下，链接使用 tie-interp C ABI 桥的程序（REPL 内建
  read_line / eval，或由 Rust 静态库 tie_interp.lib 支撑的 std 函数）会因 lld 的 CRT 解析缺陷报
  `undefined symbol: printf`；普通程序（不经过 interp 桥）用随包 lld 链接正常。随包的
  repl.exe / pkg.exe 在打包机（装有 VS）上预构建，终端用户不受影响——该限制只影响在无 VS 环境下
  重编 interp 桥程序；
- **LLVM 许可**：`third_party/llvm/LICENSE.TXT` 保存 LLVM 官方许可（Apache-2.0 with LLVM
  Exceptions），随包分发为 `bin/llvm/LICENSE.txt`。

## 6. 架构与模块

编译器源码位于 `compiler/` 目录，全部为 tie 语言文件：

```
compiler/
├── driver.tie          CLI 完整编译器入口（T3.2）→ 编译为 tiec.exe
├── driver-lite.tie     T2.9 临时入口（组装验证用，gen_src 路径）
├── repl.tie            REPL 主循环（T4.3）→ 编译为 repl.exe
├── lib/                基础库：interner（字符串池）/ columnar（列式表）/ dispatch（分派表）
├── frontend/           前端：lexer（词法）→ parser（语法）→ semantic（语义，AST 为列式 tag 表 arena）
├── middle/             tie-IR（列式函数/块/指令表）+ pass 管线（AnalysisManager 惰性 + 失效追踪）
├── backend/            后端：irgen（AST → tie-IR）+ llvmgen（tie-IR → LLVM IR 文本，单调 %N 直生）
│                       + toolchain.tie（opt / clang / llvm-ar / lld 驱动）
├── interp/             tie 自写树遍历解释器（value / session / interp / env，T4.1–4.2）
├── proto/              各阶段原型（T1–T2 期间探索用，正式版不依赖）
├── tests/              测试套件：错误 golden 语料 / pass 测试 / interp 行为测试（11 文件 198 断言）/ 运行器
├── tiec.exe / tiec2.exe  编译产物（种子版 / 自举版）
└── README.tie          目录说明（架构注释）
```

### 各模块职责

| 模块 | 职责 |
| --- | --- |
| `driver.tie` | CLI 壳：参数解析、角色分派、消息格式、退出码。组装 frontend（parse_ast / check_ast）+ irgen.gen_ast / llvmgen.emit + backend/toolchain |
| `frontend/lexer` | 词法分析：token 化 + 字符串池 intern |
| `frontend/parser` | 语法分析：生成内存 AST（列式 tag 表），build_protocol 序列化（driver-lite 路径用） |
| `frontend/semantic` | 语义分析：符号表 / 类型检查，输出 check_impl 后的内存状态供 irgen 复用 |
| `middle/` | tie-IR 列式表表示与 pass 基础设施 |
| `backend/irgen` | AST → tie-IR 生成（T5.3 起主路径走 gen_ast 内存态，避免重复前端） |
| `backend/llvmgen` | tie-IR → LLVM IR 文本生成（自举后端单调编号） |
| `backend/toolchain.tie` | 工具链驱动：工具发现、`opt -O{0..3} -S`、`clang` 链接（含 `--target` 交叉、`-Wl,/Brepro` 可复现）、`clang -c` + `llvm-ar rcs` 库编译、进程退出码与 stderr 捕获 |
| `interp/` | 树遍历求值器：Value 编码（节点 id + 平行表）、Session（globals/funcs/AST 归档池）、eval/eval_call、C ABI 桥 tie 化（env.tie） |
| `tests/` | 错误 golden 语料（63 触发）、interp 行为测试、行为等价回归 |

### 性能要点（T5.3）

- 主路径使用 `gen_ast()` 复用前端内存态，**消除前端重复执行**（`gen_src` 仅留给 driver-lite）；
- `build_protocol` 分治 join，`O(n²)` 降到 `O(n log n)`；
- 字符串池 intern 二分定位 + 位移插入，省去冒泡交换；
- AST 内存直拷（copy_ast_tables / append_ast_mem），消除文本协议往返。

## 7. 自举 v2 进度

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| T1.1–T1.5 | 词法/语法原型 + G1 性能闸门 | ✅ 完成 |
| T2.4–T2.6 | 完整词法 / 语法 / 语义分析器（正式版，tie 重写前端） | ✅ 完成 |
| T2.7 | 错误 golden corpus（63 触发 + 语义检查补齐） | ✅ 完成 |
| T2.8 | LLVM IR 文本生成端到端（irgen + llvmgen，opt + clang 编译运行） | ✅ 完成 |
| T2.9 | tiec 组装（driver-lite）+ 行为等价回归（等价率 100%） | ✅ 完成 |
| T3 | tiec 完整编译器：工具链驱动 + CLI + 可复现构建（Brepro） | ✅ 完成 |
| T4.1–T4.4 | 解释器 tie 化：core（eval/eval_call）+ 环境原语 + REPL parity + 测试移植（11 文件 198 断言） | ✅ 完成 |
| T4.5–T4.6 | tie 运行时静态库（`std/runtime.a` 替代 Rust interp 链接）+ G3 闸门（0-Rust） | ✅ 完成 |
| T5.1 | tiec 前端全局表修复（自编译成功）+ G4 性能基准建立 | ✅ 完成 |
| T5.2 | irgen 扩展修复 13 个自举编译 bug + 自举链二阶闭环打通（tiec → tiec2 → tiec3） | ✅ 完成 |
| T5.3 | tiec 性能优化（消除重复前端 / AST 内存传递 / intern 二分 / build_protocol 分治 / print_err 清理），G4 总比 6.9 → 1.09，hello 反超 Rust 种子 | ✅ 完成 |
| T5 后续 | irgen 最小集扩展（前端已就绪，扩展后 G4 覆盖自动扩全） | 📋 进行中 |

## 8. 已知限制与当前状态

- **角色支持**：当前只编译 `logic`（可执行）与 `library`（静态库）；`data` / `ui` / `db` 角色提示未实现；
- **T5 后续进行中**：irgen 最小集扩展仍在推进，部分高级语法特性（如 enum / 函数指针方向的 B1 规划）暂未覆盖；
- **解释器桥限制**：需要指针类型的桥函数（如 file_read / str_char / rand_range / arg_*）无法 tie 化，仍走 Rust 底座转发；
- **自举细节**：tiec 由 Rust 种子编译（唯一 Rust 接触点），此后 0-Rust；老编译器 tie-llvm.exe 仍保留作为种子与对照。

---

本文档随发布包分发，与 `docs/language.md`（语法规范）配合使用。
编译链路、CLI 细节与里程碑更新见根目录 `README.md` 与 `CHANGELOG.md`。
