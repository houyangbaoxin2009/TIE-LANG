# CHANGELOG

tie 语言项目的变更记录，按里程碑组织。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。
里程碑命名：**M0–M4 = 预开发版本**（正式发行前的语言核心基础建设）；**Harbor（2026.1）架构：M0 = 正式发行版基础、M1 = VSCode 插件**。

## [Harbor M1] VSCode 插件（编辑器集成） — 2026-08-07

### 新增
- `editor/vscode-tie` 重构为 TypeScript 标准工程（vscode-languageclient + esbuild 打包）：
  语法高亮（含 M4 运算符、宽类型、头指令着色）、智能缩进（onEnterRules）、代码片段、
  VSIX 打包安装（README 含开发调试 / 打包 / 配置说明）
- tie-lsp 新增跳转定义 `textDocument/definition`（函数 / 方法 / 字段 / 类 / 变量）与
  自动补全 `textDocument/completion`（关键词 / 类型 / 内置函数 / 顶层函数 / 类名 +
  `类名.` 成员补全），capabilities 声明 definitionProvider / completionProvider
  （triggerCharacters `["."]`）
- LSP 测试 +20（共 53 通过）：definition 函数 / 方法 / 变量命中与未命中、文档未打开、
  completion 全集 / 类成员 / 未打开文档

### 文档
- README.md：路线图重组为「架构 → M 里程碑」两级（Harbor 架构新增 M1 VSCode 插件条目）；
  tie-lsp 描述更新为
  诊断 / hover / 跳转定义 / 补全
- editor/vscode-tie/README.md：扩展安装（F5 开发调试 / vsix 打包）、配置（tie.lsp.command）、
  功能清单、协议兼容说明

## [Harbor M0] 正式发行版基础 — 2026-08-07

### 新增
- 版本规则确立：正式发行版号 `年份.修订号`（如 2026.1）；内部代号 `2026.1 "Harbor 港湾"`（首个正式版 = 工具链首次靠岸）
- 组件版本号独立化：6 个 crate 各自维护 3 段 semver（初始 `0.1.0`，写入各自 Cargo.toml），
  与发行版号（发布产物/tag 用）分离
- `tie --version` / `tie -V`：输出组件版本 + 发行版号 + 代号（`tie 0.1.0 (发行版 2026.1 "Harbor")`）；
  同步支持于 tie-prep / tie-frontend / tie-llvm / tie-lsp
- 打包脚本 `scripts/package.ps1`：release 构建 → repl.exe 自举 → 组装发行目录
  （bin/doc/examples/editor）→ 生成 `dist/tie-2026.1-win-x64.zip`（win-x64）
- 设计文档 `docs/release.md`：版本规则、内部代号、工具链合集组成、工程改造点、发布流程

### 文档
- README.md：路线图新增 M5（正式发行版）条目
- docs/release.md：正式发行版设计规划

## [M4] 运算符扩展 — 2026-08-07

### 新增
- 复合赋值：`+=` `-=` `*=` `/=` `%=`（算术）与 `&=` `|=` `^=` `<<=` `>>=`（位运算），
  支持变量与对象字段目标（`x += 1`、`obj.s += "x"`）；字符串仅支持 `+=`（拼接）
- 位运算：`&` `|` `^` `<<` `>>`，仅限整数操作数（语义层报"位运算只支持整数"）；
  右移区分有符号算术移位（`ashr`）/ 无符号逻辑移位（`lshr`）
- 三目运算符 `c ? a : b`：右结合、条件必须为 `bool`、两分支类型一致、短路求值
  （LLVM 三块 phi 汇合；解释器短路）
- 自增自减 `++` `--`：前缀返回新值、后缀返回旧值，操作数须为可写数字变量
  （语义层报"自增/自减的操作数必须是可写数字变量"；字段自增在 IR 层返回明确错误，M4 简化）
- 词法：新增 token（`PlusEq`…`ShrEq`、`Amp`/`Pipe`/`Caret`/`Shl`/`Shr`、`Inc`/`Dec`、`Question`），
  支持三字符 `<<=`/`>>=`；`is_bin_op` 不含 `Inc`/`Dec`（ASI 续行语义保持）
- 语法：优先级链扩展为 范围 → 逻辑或 → 逻辑与 → 按位或 → 异或 → 按位与 → 相等 → 关系 → 移位 → 加减 → 乘除模 → 一元
- IR 生成：`gen_binary_on_regs` 抽取复合赋值与二元运算共用核心；三目 phi 汇合

### 修复
- `gen_binary_str` 比较 match 的 `_` 臂从 `unreachable!` 改为返回明确 IrError（避免 panic）

### 测试
- 前端 +14：位运算优先级嵌套、移位、三目与嵌套右结合、复合赋值 op 断言、自增自减前后缀、
  位运算/复合赋值/三目/自增自减类型检查、行尾 M4 运算符不补分号
- LLVM IR +5：位运算与移位指令、复合赋值 load/运算/store、字符串复合拼接 malloc/memcpy、
  三目 phi 汇合、自增自减指令序列
- 解释器 +4：`eval_bitwise`、`eval_compound_assign`、`eval_ternary`、`eval_inc_dec`

### 文档
- README.md：路线图 M4 标记完成
- docs/language.md：§9.3 符号表新增复合赋值、位运算、三目、自增自减及约束说明
- examples/m4_ops.tie：M4 运算符综合示例

## [REPL] 解释执行自举 — 2026-08-07

### 新增
- `tie-interp` 解释器（完整求值器）：树遍历执行 AST，动态类型（int/float/bool/char/string/range/void）
- 两趟解析：顶层 `func` 定义注册进持久 Session；其余代码包装为 `func main() { ... }` 执行，
  顶层 `var` 声明落入 globals、跨行持久（REPL 连续输入 `var x=1` 后 `x+1` 的基础）
- 控制流：if/else、while、for 范围遍历、return 传播（Flow 枚举，非错误通道）
- 内置函数：`println`/`print`/`len`/`read_line`（读 stdin 一行，EOF 退出）/
  `eval`（动态求值代码，递归重入安全，Session 用 thread_local RefCell 避免 Mutex 死锁）
- C ABI 桥（`tie_eval_expr`/`tie_read_line`/`tie_free_result`）：staticlib 产物，
  catch_unwind 包裹（panic 跨 extern "C" 是 UB）
- **自举外壳 `repl/repl.tie`**：REPL 外壳本身用 tie 语言编写（`print("> ")` + `read_line` +
  `eval` + 无限循环），经 tie-llvm 编译并链接 tie-interp 静态库生成 `repl.exe`；
  `tie` 无参数时启动 repl.exe（查找：`TIE_REPL_EXE` 环境变量 → tie.exe 同目录 → 当前目录）
- 编译路径扩展：语义层 `read_line`(→string)/`eval`(string→string)/`print` 内置签名；
  IR 层按需声明并调用 interp 库符号，`IrOutput.used_externs` 记录用到的符号，
  driver 据此**按需链接** tie-interp 静态库（`TIE_INTERP_LIB` 环境变量 / target 目录 / exe 同目录）；
  跨 target 守卫：带 interp 依赖的程序交叉编译时明确报错（interp 库仅本机构建）
- 链接补 Windows 系统库（`ws2_32`/`userenv`/`ntdll`/`bcrypt` 等，Rust staticlib 的 std 依赖）

### 文档
- README.md：REPL 自举说明与工程结构更新（tie-interp 从占位改为完整解释器）

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
