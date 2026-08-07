# 正式发行版设计规划

> 本文档定义 tie 语言**正式发行版（Release）**的版本规则、内部代号、
> 工具链合集组成、工程改造点与发布流程。
> 实施依据：README 路线图 M5（正式发行版）。

## 1. 版本命名规则

正式发行版版本号格式：`年份.修订号`，例如 `2026.1`。

- **年份**：发行年份（4 位数字），如 `2026`。
- **修订号**：该年内的发行序数，从 1 递增（`2026.1` → `2026.2` → …）。
- 版本号与 git tag 一致：`2026.1`（裸版本号，不带 `v` 前缀）。
- Cargo crate 版本号与发行版号**保持一致**：
  `2026.1` 本身是合法 semver（主版本 2026，次版本 1），可直接用于 Cargo。

## 2. 内部代号（架构代号）

每个正式发行版配一个内部代号，代表该版本的架构特征或主题。

| 版本 | 内部代号 | 含义 |
| --- | --- | --- |
| 2026.1 | **Harbor 港湾** | 首个正式版 = 工具链第一次靠岸停泊，形成可交付的稳定形态 |

代号仅用于宣传/文档/产物命名（如安装包名），不进入版本号。

## 3. 工具链合集组成

正式发行版是一套**工具链的合集**：把当前分散的 6 个 crate 二进制
+ REPL 外壳 + 文档打包为一个可安装、可分发的整体。

### 3.1 二进制组件（bin/）

| 组件 | 源 | 职责 |
| --- | --- | --- |
| `tie.exe` | crates/tie | 总入口（四段式调度器 + REPL 启动） |
| `tie-prep.exe` | crates/tie-prep | 预处理（清理代码 + 识别文件角色） |
| `tie-frontend.exe` | crates/tie-frontend | 前端三阶段（词法/语法/语义，调试用） |
| `tie-llvm.exe` | crates/tie-llvm | 编译（IR 生成 + opt/clang/lld 后端） |
| `tie-lsp.exe` | crates/tie-lsp | 语言服务器（LSP over stdio） |
| `tie-interp.exe` | crates/tie-interp | 解释执行 |
| `repl.exe` | repl/repl.tie | REPL 外壳（tie 语言自写，自举产物） |

### 3.2 文档（doc/）

- `README.md`（工程入口）
- `docs/language.md`（语法规范）
- `docs/ai-guide.md`（AI 教学指南）
- `docs/prompt-pack.md`（Prompt 包）
- `CHANGELOG.md`（版本变更记录）
- `LICENSE`（MIT）

### 3.3 示例（examples/）

全部示例 `.tie` 源文件（hello / wide / table / tuple / oop / m4_ops 等）。

### 3.4 编辑器扩展（editor/vscode-tie）

VSCode 扩展（`editor/vscode-tie`）随发行版一起分发，提供语法高亮与
LSP 诊断支持（配合 `tie-lsp`）。

### 3.5 前置依赖（不随包分发）

编译链路调用外部 LLVM 工具链：`opt` / `clang` / `llvm-ar`。
发行版**不捆绑** LLVM（体积过大），依赖用户机器已安装 LLVM（PATH 或
`D:\LLVM\bin` 等常见位置）。安装文档与 `tie --version` 输出中说明此依赖。

## 4. 工程改造点

### 4.1 Cargo 版本号统一

- 根 `Cargo.toml` 的 `[workspace.package]` 增加 `version = "2026.1"`。
- 6 个 crate 的 `Cargo.toml` 改为 `version.workspace = true`（消除版本号散落）。

### 4.2 tie --version

- `tie --version` 输出版本与代号：
  ```
  tie 2026.1 (Harbor)
  ```
- 版本号读取自 `CARGO_PKG_VERSION`（编译期注入），代号为编译期常量。
- 各子工具（tie-prep / tie-frontend / tie-llvm / tie-lsp / tie-interp）
  同步支持 `--version`（复用同一模式）。

### 4.3 打包脚本

新增 `scripts/` 目录：

- `scripts/package.ps1`：Windows 打包脚本。
  1. `cargo build --release`（全 workspace，验证 0 错误）
  2. 复制 6 个 exe + repl.exe 到 `dist/tie-2026.1/bin/`
  3. 复制文档与示例到 `dist/tie-2026.1/`
  4. 复制 VSCode 扩展（editor/vscode-tie）到 `dist/tie-2026.1/editor/vscode-tie/`
  5. 压缩为 `dist/tie-2026.1-win-x64.zip`

### 4.4 README 路线图

M5 里程碑 = 正式发行版，规划内容如上。

## 5. 发布流程

适配自 publish-release 技能（原面向 dotnet，此处 Rust 化）：

1. 推断版本号（年份.修订号，向用户确认）
2. 同步 Cargo 版本号（workspace + `--version` 输出）
3. 更新 CHANGELOG.md
4. 同步文档（README、language.md、ai-guide 等）
5. `cargo build --release` 验证（0 错误）
6. 运行打包脚本生成 zip（+ 可选安装包）
7. 提交并推送（git.franj2.top）
8. 打 git tag（裸版本号 `2026.1`）
9. 创建双平台 Release（GitHub / GitCode），上传安装包与压缩包

## 6. 既定决策

- 打包产物：**仅 zip 压缩包**（`tie-2026.1-win-x64.zip`），安装包后续版本再做
- 编辑器扩展：**包含** `editor/vscode-tie`（随发行版分发）
- 目标平台：**仅 win-x64**（本机可验证；跨平台后续版本）
