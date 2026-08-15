# 规划：tsp（tie 语言服务器，原 tie-lsp 重写增强）

> 状态：**规划**（2026-08-15 设计讨论定稿，未实现）
> 本文档定义 tsp——tie 语言服务器（Language Server），原 tie-lsp 的
> **tie 重写 + 全量增强**。命名空间/目录：**tsp**（原计划 tlsp 弃用）。
> 决策汇总：
> **R1**（tie 重写服务端，0-Rust 自举路线）+ **全量增强**（16 项能力）
> + **增量分析第一版做**（性能关键）+ **tieconsole 复用确认**（补全引擎共用）
> + **semanticTokens 第一版**（语义高亮）。
> 关联：自举链（tiec/interp/trm 已 tie 化，LSP 是最后 Rust 残留）、
> 编译器前端（lexer/parser/semantic 直接复用）、ir_meta（sym_table 符号索引）、
> tieconsole（补全/高亮复用）、序列化规范（LSP 协议用 JSON——标准协议）。

## 1. 定位与路线

### 1.1 现状

- Rust 版 tie-lsp 已归档（tiec_rust）——**LSP 服务端是自举链最后的 Rust 残留**
- VSCode 客户端已实现（TypeScript + vscode-languageclient，M1 完成）：
  didOpen/didChange/didClose、publishDiagnostics、hover、definition、completion
- 自举后的 tiec 无 LSP 服务端实现（driver 只有 `--lsp` 传参入口）

### 1.2 R1：tie 重写（0-Rust 路线收官）

```
现有：Rust tie-lsp（已归档）+ VSCode 客户端（TypeScript，保留）
重写：compiler/lsp/（tie 写）→ tsp.exe（tiec 编译，0-Rust）
```

**核心优势：编译器前端直接复用**——tsp 直接 import compiler/frontend
（lexer/parser/semantic 全部 tie 写），分析引擎与编译器**同一份代码**：
- 编辑器诊断 = 编译错误（永不漂移）
- 补全/hover/跳转基于真实符号表（非重复实现）

## 2. 目录与命名空间

```
compiler/lsp/（type tie<class>，命名空间 tsp）
├── protocol.tie     LSP 协议（JSON-RPC 编解码，消息类型定义）
├── server.tie       服务端主循环（stdio 读写 + 请求分发）
├── analyze.tie      文档分析（import compiler/frontend 复用）
├── index.tie        符号索引（复用 ir_meta sym_table + import 图）
├── completion.tie   补全（符号表 + 语法上下文）
├── hover.tie        hover（符号签名 + doc_comment 文档注释）
├── diagnostics.tie  诊断（复用 semantic 错误，增量）
├── definition.tie   跳转定义（符号表 + import 图）
├── reference.tie    引用查找（符号索引 + 反向索引）
├── rename.tie       重命名（符号表 + import 图，跨文件）
├── signature.tie    签名帮助（参数列表提示）
├── symbol.tie       文档/工作区符号（复用 parser AST）
├── semantic_tokens.tie  语义令牌（语义高亮）
├── folding.tie      折叠范围（AST 块结构）
├── format.tie       格式化（对齐 prep/indent 转换器思路）
└── quickfix.tie     代码操作（诊断 → 修复建议）
```

## 3. 协议层（protocol.tie）

- LSP = JSON-RPC over stdio——std 已有 json 解析（复用 std/json）
- **LSP 协议用 JSON**（标准协议，不适用 §1.4 "通信用 zd"——协议兼容优先）
- 消息类型：Initialize/Initialized/Shutdown/Exit +
  textDocument/*（didOpen/didChange/didClose/hover/completion/definition/...）
- 并发：LSP 异步请求（服务端循环 + 请求分发）

## 4. 能力全量清单（16 项）

| # | 能力 | 现状 | 增强实现 |
| --- | --- | --- | --- |
| 1 | 诊断 diagnostics | ✅ Rust 版有 | 复用 semantic + 增量（§5） |
| 2 | hover | ✅ | + doc_comment 文档注释 + port 方法签名 |
| 3 | 定义跳转 definition | ✅ | + 跨文件（import 图）+ go-to-type |
| 4 | 补全 completion | ✅ 触发 "." | + 关键字/符号表/参数/泛型实参 |
| 5 | **引用查找 references** | ❌ | 符号索引 + 反向依赖索引 |
| 6 | **重命名 rename** | ❌ | 符号表 + import 图（跨文件） |
| 7 | **签名帮助 signatureHelp** | ❌ | 参数列表提示（复用签名元数据） |
| 8 | **语义令牌 semanticTokens** | ❌ 第一版做 | 语义高亮（比 TextMate 精确） |
| 9 | **文档符号 documentSymbol** | ❌ | 大纲（复用 parser AST） |
| 10 | **工作区符号 workspaceSymbol** | ❌ | 跨文件符号搜索 |
| 11 | **代码操作 quickfix** | ❌ | 诊断 → 修复建议（缺分号/未定义变量） |
| 12 | **折叠 foldingRange** | ❌ | AST 块结构 |
| 13 | **格式化 formatting** | ❌ | 对齐 prep/indent 思路（tie 转换器） |
| 14 | 同步 didOpen/didChange | ✅ | 保留 |
| 15 | 文档高亮 documentHighlight | ❌ | 符号出现位置高亮（复用索引） |
| 16 | 代码透镜 codeLens | ❌ | 函数引用计数/测试标记（后置可选） |

## 5. 增量分析（第一版做，性能关键）

```tie
// 文档变更 → 只重分析该文件 + 反向依赖它的文件
// 符号表缓存：文件 → 符号；import 图反向索引
```

- 大项目（编译器自身 8 模块）编辑不卡顿
- 复用编译器 frontend——分析一致性（编辑器诊断 = 编译错误）
- 缓存粒度：文件级（符号表 + 诊断结果）；import 变更才级联

## 6. 符号索引（index.tie）

- **复用 ir_meta sym_table**：函数/struct/命名空间/全局 + 签名 + 可见性 + span
- import 图：解析 import 得到跨文件依赖（正向 + 反向索引）
- 工作区索引：打开的文件 + import 可达文件（按需加载）

## 7. tieconsole 复用（确认）

```
tsp 服务端 = 分析引擎（tie 写，编译器前端复用）
  ├── VSCode 客户端（现有，保留）
  └── tieconsole 补全/高亮（I2/I3 复用 tsp 能力）
```

- tieconsole 的补全（I2）+ 语义高亮（I3）直接连 tsp 分析能力
- 共享实现：completion.tie / semantic_tokens.tie 双客户端复用
- 协议形态：tieconsole 进程内调用（库模式）或独立 tsp 进程（LSP 模式）——第一版库模式

## 8. 编译器实现拆解

| 模块 | 改动 |
| --- | --- |
| protocol.tie | LSP JSON-RPC 编解码（std/json 已有） |
| server.tie | stdio 循环 + 请求分发（LSP 异步请求） |
| analyze.tie | import compiler/frontend 复用（lexer/parser/semantic） |
| index.tie | 复用 ir_meta sym_table + import 图（正/反向） |
| 增量 | 文件级缓存 + 反向依赖索引 |
| 全量能力 | completion/hover/diagnostics/definition/reference/rename/signature/symbol/semantic_tokens/folding/format/quickfix/documentHighlight |
| tieconsole 桥 | 库模式 API（补全/高亮复用） |
| CLI | `tie --lsp` → 启动 tsp（现状入口保留） |

## 9. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 重写路线 | R1：tie 重写（0-Rust 收官，编译器前端复用） | Rust 保留增强 |
| 命名 | **tsp**（原计划 tlsp 弃用） | tlsp、tie-lsp 保留 |
| 增强范围 | 全量 16 项能力 | 子集 |
| 增量分析 | 第一版做（性能关键） | 后置 |
| tieconsole 复用 | 确认：补全/高亮共用 tsp 引擎 | 独立实现 |
| 语义高亮 | semanticTokens 第一版 | TextMate 保持 |
| 协议 | LSP 标准 JSON（协议兼容优先，不用 zd） | zd |

## 10. 未决问题

1. **并发模型**：LSP 异步请求的调度（tsp 内用协程？——并发模型 M4 落地前
   第一版顺序处理 + 增量缓存补偿）
2. **tieconsole 库模式 API**：补全/高亮的最小接口面（completion(symtab, pos) /
   tokens(ast) 函数形态）
3. **quickfix 修复范围**：第一版修复集（缺分号/未定义变量/未使用变量？）
4. **格式化规则**：与 prep 转换器的关系（格式化 = 转换器特例？）
5. **语义令牌的 token 类型集**：与 VSCode 标准语义令牌类型的映射
   （keyword/type/variable/function/namespace...）
