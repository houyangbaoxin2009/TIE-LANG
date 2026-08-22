# CLI 用法

> tie 命令行用法速查。工程入口、快速开始与工程结构见 [README.md](../README.md)。

## 主入口 `tie`

`tie`（四段式调度器，合并原 tie-cli 职责）：

```
tie <input.tie>... [-o output] [-O0|-O1|-O2|-O3] [--target <三元组>] [--emit-ir] [--keep-ir] [--shared] [--prep-only] [--config <file>]
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
| `--shared`            | 编译为动态库（library 角色 → Windows `.dll` / Linux `.so`；输出扩展名 `.dll`/`.so` 也可触发，M5）                                                                           |
| `--prep-only`         | 只做预处理（tie-prep）并打印识别结果                                                                                                                                        |
| `--config <file>`     | 指定构建配置文件（S3.1：默认查当前目录 `config.data.tie`，分层合并：CLI > 项目 config > 用户 `~/.config/tie/config.data.tie` > 内置默认）                                                     |
| `--profile <p>`       | 构建 profile（dev/release，覆盖配置顶层 `profile` 键；Cargo 风格，S3.1）                                                                                                      |
| `--backend <b>`       | 后端实现选择（win32/LLVM 工具链；其余 port 尚未接入，S3.1）                                                                                                                      |
| `--module <file.tie>` | tie-prep：挂载自定义 tie 转换器模块（顶层 `process(src)->string`），输出为模块转换结果（Harbor M3 可扩展性）                                                                                 |
| `--lsp`               | 以语言服务器模式运行（读 stdin 的 LSP 消息、写 stdout，等价于 `tie-lsp`）                                                                                                           |
| `-h, --help`          | 显示帮助                                                                                                                                                          |

流程：`tie-prep` 预处理（清理代码 + 识别文件类型）→ 按角色自动转交工具链。

## 包管理器子命令

**包管理器（Harbor M6，tie 语言自写）**：Rust 入口识别首个参数为子命令
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

端到端演示见 [examples/pkg_demo.md](../examples/pkg_demo.md) 与 `examples/demo_pkg/`。
构建 `pkg.exe`：`compiler/tiec.exe pkg/main.tie -o pkg/pkg.exe`

## 多文件并行编译

配置文件开启 `advanced.enabled = true` 后，可一次编译多个输入文件。

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

## 库编译

库编译示例（`type tie<class>` 角色，定义函数不定义 main）：

```bash
tie examples/lib_math.tie          # → examples/lib_math.a（经 clang -c 生成 .o，llvm-ar rcs 打包）
tie examples/lib_math.tie -o lib_math.lib   # → MSVC 兼容静态库 .lib（COFF 归档，同一产物不同扩展名）
```

- 静态库 `.a` / `.lib`（Windows 上均为 COFF 归档）：导出符号为 `命名空间$函数`
  （如 `mathlib$add`），C/其他语言可链接消费；
- 动态库（`.dll` / `.so`）编译为 Harbor M5 内容（见 docs/plans/dynamic-library.md）。

## 子工具

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

Rust 编译器（tiec_rust）已归档至独立仓库 [tiec_rust](https://github.com/tie-lang/tiec_rust)。
