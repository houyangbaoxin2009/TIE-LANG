# vscode-tie — tie 语言支持

tie 语言的 VSCode 扩展：语法高亮、智能缩进、代码片段，并通过 **LSP 语言服务器**（`tie --lsp`）提供诊断、hover、跳转定义与补全。

## 功能清单

| 功能 | 说明 |
| --- | --- |
| 语法高亮 | 关键词 / 类型 / 运算符 / 数字（含 0x 与下划线分隔）/ 字符串 / 注释 / 文件类型声明（`type tie<...>` 头部声明着色）/ 内置函数 |
| 智能缩进 | 函数 / struct / 命名空间 / 控制流行尾 `{` 自动缩进，`}` 自动对齐，`case` / `default` 自动缩进 |
| 代码片段 | `func`、`struct`、`ns-method`（命名空间方法函数）、`if` / `ifelse`、`for`（范围遍历）、`while`、`switch`、`import`、`println` |
| 诊断 | 打开 / 编辑 `.tie` 文件时，错误与纠错实时显示在「问题」面板 |
| Hover | 悬停符号显示类型 / 函数签名等信息 |
| 跳转定义 | 跳转到符号定义处（后端支持后生效） |
| 补全 | 代码补全，含 `.` 触发的成员补全（后端支持后生效） |

> 诊断 / hover / 跳转 / 补全均来自 tie 语言服务器（LSP），扩展只负责连接与展示。

## 依赖

- **VS Code ≥ 1.75**
- **tie 工具链**：需已构建并放入 PATH，保证终端可运行 `tie --lsp`（等价于 `tie-lsp`）。
  - 构建：仓库根目录执行 `cargo build --workspace`
  - 验证：终端执行 `tie --lsp`（无报错即就绪；服务器以 stdio 方式等待输入属正常现象，Ctrl+C 退出）

## 安装

### 开发调试（F5）

1. 安装依赖并构建：

   ```bash
   npm install
   npm run compile
   ```

2. 在 VS Code 中打开本目录（`editor/vscode-tie`），按 `F5` 启动「扩展开发宿主」。
3. 在扩展开发宿主中打开任意 `.tie` 文件即可体验。

### 打包安装（vsix）

```bash
npm install
npm run compile
npx @vscode/vsce package
```

生成 `vscode-tie-0.1.0.vsix` 后，在 VS Code 扩展面板选择「从 VSIX 安装…」。

## 配置

| 配置项 | 说明 |
| --- | --- |
| `tie.lsp.command` | 启动语言服务器的命令，数组形式 `[命令, 参数...]`。默认 `["tie", "--lsp"]`。 |

- **tie 在 PATH 中**：无需配置，默认即可。
- **tie 不在 PATH 或需指定版本**：配置为绝对路径，例如：

  ```json
  { "tie.lsp.command": ["F:/Projects/tie/target/debug/tie.exe", "--lsp"] }
  ```

服务器启动失败时，扩展会弹出提示引导检查安装与配置；服务器 stderr 与客户端日志见「输出 → tie」。

## 开发

```bash
npm run compile   # 类型检查（tsc）+ 打包（esbuild）→ out/extension.js
npm run watch     # 增量编译（开发调试用）
```

- 源码：`src/extension.ts`（vscode-languageclient 客户端）
- 语法：`syntaxes/tie.tmLanguage.json`（TextMate 语法，顶层 scope `source.tie`）
- 语言配置：`language-configuration.json`（注释 / 括号 / 自动闭合 / onEnterRules 智能缩进）
- 片段：`snippets/tie.code-snippets`

## 协议兼容

扩展使用标准 LSP 协议（vscode-languageclient）与 tie-lsp 后端通信，文档同步为全量模式（`textDocumentSync=1`）。后端新增的跳转定义 / 补全为标准方法，扩展无需改动即可自动支持。
