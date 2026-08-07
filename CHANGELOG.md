# CHANGELOG

tie 语言项目的变更记录，按里程碑（M0→M4）组织。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

## [LSP] 语言服务器 — 2026-08-07

### 新增
- `tie-lsp` 语言服务器：基于 JSON-RPC 2.0 over stdio，与编辑器（VSCode 等）通信
- 三阶段诊断：复用 tie-frontend 词法/语法/语义分析，错误 → LSP 诊断推送（fail-fast）
- 文档同步：`didOpen` / `didChange`（全量）/ `didClose`，变更即推送诊断
- `hover`：函数签名（`func name(params) -> Ret`）与类信息（含 `extends` 父类）
- 协议自研：仅依赖 serde/serde_json，不引入 lsp-server 等现成框架
- 接入主入口：`tie --lsp` 启动语言服务器（与 `tie-lsp` 等价）；核心主循环提炼为库入口
  `tie_lsp::run_server()`，独立二进制与主命令复用同一实现

## [M3] class/OOP — 2026-08-07

### 新增
- `class` 类定义：值类型对象（LLVM 字面结构体 `{字段…}`），字段 `var name[: Ty] [= 默认值]`
- 构造表达式 `类名(实参…)`：按字段声明顺序传参，缺省字段用默认值（无默认值则类型零值）
- 实例方法 `method m(params) -> Ty`：方法体内 `this` 绑定当前对象；静态方法 `static method`：无 `this`
- 字段访问：`obj.field` 读（GEP+load）、`obj.field = 值` 写（GEP+store 直写）
- 继承 `class C extends P`：字段拍平（父类字段在前）+ 方法同名遮蔽（复用式，无 vtable）
- 语义校验：类仅顶层定义；字段名跨继承链唯一；继承环检测；寄存器类值不可寻址报错；
  静态方法须类名调用、实例方法须实例调用；类名与函数名/类名冲突检测
- `--target <三元组>` 交叉编译：CLI 参数与头部 `// tie:target=` 双通道，支持平台别名
  （`win-x64`、`linux-x64`、`macos-arm64` 等 → LLVM 三元组）与直接三元组透传
- `library` 角色静态库编译：IR → 目标文件（`clang -c`）→ 静态库（`llvm-ar rcs`，`.a`），不要求 main
- 编译流程按角色分派产物：`logic` → 可执行文件；`library` → `.a` 静态库
- `tie-frontend` 独立 CLI：词法/语法/语义三阶段可单独运行，`--tokens`/`--ast`/`--check` 调试视图

### 修复
- 语义分析 collect 阶段借用冲突（E0502）

### 文档
- docs/language.md：新增 §8 面向对象完整章节；关键词/类型/符号速查表同步
- README.md：CLI 表新增 `--target` 与库编译说明；路线图 M3 标记完成

## [M2] 复合类型 / 元组 / import — 2026-08-06

### 新增
- 元组类型：字面量/命名与位置访问（`t.x` / `t.Item1` / `t.0`）、多值返回、解构 desugar
- `import` 多文件导入：递归加载内联函数
- 字符串操作：拼接、比较、长度、下标取字符
- `switch` 多分支选择语句（支持字符串）
- 表运行时：下标访问与表遍历
- 赋值语句与字符字面量
- var/const/func 关键词、宽类型 num/text/misc、表类型 table

### 修复
- 分支 return 死代码

### 文档
- docs/language.md：类型系统改为 Rust 风格，修正 code 类型语义

## [M1] 控制流 / 函数 / string — 2026-07

### 新增
- 控制流：if/else、while、for 遍历
- 函数调用与定义
- 字符串处理

## [M0] 基础打通 — 2026-07

### 新增
- 词法分析（含 ASI 自动分号补全）
- 语法分析（含文件头解析）
- 语义分析（符号表/类型检查）
- LLVM IR 文本生成 + opt/clang/lld 后端链路
- 跑通 `println` / 算术 / 变量
