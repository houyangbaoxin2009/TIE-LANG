# CHANGELOG

tie 语言项目的变更记录，按里程碑组织。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。
里程碑命名：**M0–M4 = 预开发版本**（正式发行前的语言核心基础建设）；**Harbor（2026.1）架构：M0 = 正式发行版基础、M1 = VSCode 插件、M2 = 标准库**。

## [Harbor M3 阶段一] 预处理器自举：核心逻辑 tie 语言化 — 2026-08-08

编译器自举第一阶段：`tie-prep` 的预处理核心逻辑（头部提取 / 角色判定 / 正文重建）
完全用 tie 语言重写，Rust 侧降为解释执行壳。

### 新增
- **`prep/core.tie`（tie 语言自写预处理模块）**：`namespace prep` 内含 9 个函数
  （`is_whitespace` / `slice` / `trim` / `starts_with` / `split_lines` / `join_lines` /
  `header_kind` / `detect_role` / `process`），完全基于语言底座原语
  （`str_char` / `len` / 拼接 / `table_new_*`），自包含不依赖任何 import。
  入口 `func process(src: string) -> string` 输出协议文本
  （`ROLE:` / `HEADERS:n` / `H:raw`×n / `BODY:m` / 正文 m 字节），
  与原 Rust 版预处理行为逐条对齐（头部区扫描、角色判定顺序、正文重建）。
  编译期内嵌进 tie-prep 二进制（`include_str!`），发布无需额外文件。
- **`tie-prep` 重构为解释执行壳**：字节规范化（去 BOM、CRLF→LF）留壳层
  → `eval_call("prep::process", src)` → `parse_protocol` 解析协议文本还原
  `PreprocessResult`。新增协议解析器等；`--prep-only` CLI 行为不变。
- **`prep/indent.tie`（转换器模块示例，可扩展性证明）**：顶层
  `func process(src: string) -> string` 把制表符缩进替换为 4 空格，
  完全基于语言底座原语。tie-prep 新增 `run_module(module, entry, source)`
  通用入口 + CLI `--module <file.tie>` 挂载选项——**新增转换器只需写一个
  tie 模块，零 Rust 改动**（M3 目标"使其可扩展"的直接证据）。

### 变更
- **打破循环依赖（M3 自举的关键）**：tie-prep 新增依赖 tie-interp（解释执行模块）；
  原 tie-frontend 依赖 tie-prep（import 展开复用其清理逻辑）会形成
  `frontend → prep → interp → frontend` 环——故 tie-frontend 移除 tie-prep 依赖，
  import 展开自带轻量 `clean_source`（去 BOM / CRLF 归一 / 剥头部行，语义与
  原 preprocess 一致）。
- tie-frontend 语义层：`ns_call_full_name` 支持**裸调用按当前命名空间前缀补全**
  （命名空间内函数互调返回表时注册键是全名，如 `prep::split_lines`）。
- tie-interp：**跨函数作用域隔离**（`Env.scope_base`）——函数 A 调用 B 时，
  B 的局部变量声明不再与 A 的同名局部变量冲突（查找/赋值/声明检查只作用于
  当前函数的 `scopes[scope_base..]` 段）。此前同名局部变量在嵌套调用下会被
  误报「变量 'n' 重复声明」。

### 测试
- 全工作区测试通过（frontend 112 / interp 58 / llvm 31 / lsp 75 / prep 4 → **6**）；
  prep 4 个既有测试改用新自举实现跑通（无头默认逻辑 / 数据角色 / 双头分离 / 内容区注释），
  新增 2 个扩展性测试（`run_module` 挂载 indent.tie 转换器验证 tab→4 空格、
  缺失入口函数报错可读不 panic）
- 端到端：`examples/hello.tie`（logic 编译运行）、`examples/lib_math.tie`
  （library 编译静态库）、`tie --prep-only`（角色/头部识别）全部通过

### 规划（后续 M）
- **新增 `docs/plans/` 设计规划目录**，为阶段一收尾时排定的三个后续里程碑产出设计文档：
  - `switch-pattern-matching.md`——switch 模式匹配增强（多值 `case 1, 2:` / 区间
    `case 3..7:` / 守卫 `case 8 when flag:` / 类型匹配 `case string:`），含 AST/语义/
    IR/解释器四层实现方案与验收标准；
  - `namespace-single-file.md`——单文件命名空间（命名空间内函数可见性 `func (ns) name`、
    `import "x.tie" as alias` 前缀重命名、跨文件冲突隔离），语义层解析顺序扩展；
  - `unified-func-style.md`——统一 func 写法（标准库去 `str_` 冗余前缀、返回类型书写
    规范、调用写法统一），不含语法破坏性变更。

## [Harbor M2.2] 正则表达式 + tie:script 模块协议基础 — 2026-08-08

### 新增
- **正则表达式内置函数（双路径）**：`regex_match`（部分匹配即真）/ `regex_find`（首个匹配片段）/
  `regex_find_all`（全部匹配片段，字符串动态表）/ `regex_replace`（全部替换，to 支持 `$1` 捕获引用）/
  `regex_group`（首个匹配的第 i 个捕获组，i=0 为整个匹配）。Rust `regex` 引擎（RE2 无回溯），
  解释路径与编译路径共用同一份 C ABI 桥实现，行为逐字节一致；模式非法 → 运行时错误（两路径文本一致）。
- **`eval_call(name, arg)`（双路径）**：调用已注册用户函数（顶层裸名或 `命名空间::函数` 全名），
  字符串值直传（不经源码文本转义，换行/引号原样直传），返回结果字符串（void → 空串）。
  这是 **tie:script 模块协议的执行基础**——框架先 `eval` 模块文件注册入口
  `func process(src: string) -> string`，再 `eval_call` 以字符串值直传源码调用，拿回处理结果。
  编译路径经 C ABI 桥共享同一 thread_local Session，跨 eval 持久。
- **`file_delete(path)`（双路径）**：删除文件，返回 bool（不存在/不可删 → false）。
  解释路径 `std::fs::remove_file`，编译路径 libc `remove()`，行为一致。
- **`str.str_find`（std 库，tie 实现）**：返回子串首次出现的字符索引（从 0 起），未找到返回 -1；
  空子串命中位置 0。与 `str_contains` 同扫描模式。

### 变更
- tie-interp 新增 `regex` 依赖（Rust regex 引擎）；C ABI 桥新增 `tie_eval_call` /
  `tie_regex_match` / `tie_regex_find` / `tie_regex_find_all` / `tie_regex_replace` / `tie_regex_group`
- tie-llvm IR 层：`regex_find_all` 返回字符串动态表（与 list_dir 同机制）；
  返回堆串的正则调用与 `eval_call` 独立语句时立即释放（无泄漏）
- `examples/std_demo.tie`：新增 `str.str_find` 断言与 `file_delete` 清理临时文件
  （原先"无内置删除文件函数，临时文件保留"）

### 测试
- 全工作区测试通过（frontend 112 / interp **58** / llvm **31** / lsp 75 / prep 4 = **280**）
- 新增：interp 正则 5 个 + eval_call 3 个 + file_delete 1 个；llvm 正则生成桥调用与声明、
  eval_call 生成桥调用与声明、file_delete 生成 remove 调用
- 端到端：`examples/regex_demo.tie`（P1 正则五原语编译运行全通过）、
  `examples/script_demo.tie`（eval 注册模块 + eval_call 多行直传/命名空间/void 入口）、
  `examples/std_demo.tie`（str_find + file_delete）编译运行全通过

## [Harbor M2.1.4] tie-lsp 语义增强：嵌套命名空间 + 参数跳转 + 语义高亮 — 2026-08-08

### 变更
- **嵌套命名空间 hover / 跳转 / 补全修复**：`ns_query_name` 改为收集完整命名空间链
  （`ns_chain` 沿 `.` 反向收集），`tcmsg.error.no_file` 与语义层注册全名
  `tcmsg::error::no_file` 对齐——hover 命中签名、跳转命中定义、`tcmsg.error.` 补全
  该层函数、`tcmsg.` 补全子命名空间成员
- **参数跳转**：`collect_defs` 把函数/方法形参也登记进变量定义表，
  函数体/方法体内引用形参名可跳转到参数声明处
- **semanticTokens 语义高亮（新增）**：`textDocument/semanticTokens/full` 返回
  全量语义 token（14 类标准类型：namespace / class / function / method / property /
  variable / parameter 等）。分类规则：定义名（func/method/class/namespace 后）、
  命名空间链段（链首/中间段 → namespace，末段函数 → function）、实例成员访问
  （`p.dist(` → method、`p.x` → property）、形参声明 → parameter、类引用 → class、
  函数调用 → function、其余 → variable
- **VSCode 扩展语法文件**：`namespace` 关键字与 `::` 运算符高亮；
  命名空间调用链前缀（`tcmsg.` 等）着色为 `support.namespace.tie`

### 测试
- 全工作区测试通过（lsp 60 → **75**）：嵌套命名空间 hover/跳转/补全（4）、
  参数/方法参数跳转（2）、语义高亮链段/参数/方法字段分类（3）、
  semanticTokens legend 等
- `cargo build --workspace` 零错误；release 构建 + 真实二进制 LSP 冒烟
  （semanticTokensProvider 声明 / 诊断无错误 / namespace·function 分类）通过

## [Harbor M2.1.3] import 展开模块化 + tie-lsp 跨文件支持 — 2026-08-08

### 变更
- **import 展开抽离为 tie-frontend 共享模块**：新增 `crates/tie-frontend/src/imports.rs`，
  把原先内联在 tie-llvm driver 的 import 展开逻辑（递归加载被导入文件 + 循环导入检测）
  上移到 tie-frontend，供编译器（tie-llvm）与语言服务器（tie-lsp）复用同一实现。
  tie-llvm driver 删除本地重复实现（-74 行），新增 `CompileError::Import` 错误分支。
- **tie-lsp 接入 import 展开（跨文件语义）**：诊断 / hover / 跳转定义 / 补全四项能力
  均按文档所在目录（`uri_base_dir`）展开 import 后再做语义分析——
  `str.str_split` / `csv.csv_read` / `math.abs` 等跨文件命名空间调用不再误报
  「未声明变量」，hover 与补全也能命中被导入文件中的函数。
- tie-frontend 新增依赖 tie-prep（import 展开需要复用其清理逻辑）。

### 测试
- 全工作区测试通过（frontend 112 / interp 49 / llvm 28 / lsp **60** / prep 4 = **253**）
- 新增真实文件端到端测试：`examples/csv_demo.tie` didOpen 诊断为空；
  hover `str.str_split` 返回跨文件函数签名（首个调用位置自动定位）

## [Harbor M2.1.2] std 库与示例统一命名空间语法 — 2026-08-08

### 变更
- **std 库全面命名空间化**（与 tcmsg 一致的命名空间形式，废弃裸函数调用）：
  - `std/string.tie` → `namespace str`（`str.str_trim` / `str.str_split` 等；`string` 是类型关键字，命名空间名用 `str`）
  - `std/math.tie` → `namespace math`（`math.abs` / `math.deg_to_rad` 等）
  - `std/assert.tie` → `namespace assert`（`assert.assert` / `assert.assert_eq` / `assert.assert_neq`）
  - `std/format.tie` → `namespace format`（`format.format_int` / `format.format_pad` 等）
  - `std/csv.tie` → `namespace csv`（`csv.csv_read` / `csv.csv_cells`），内部跨命名空间调用改为 `str.str_split` / `str.str_slice`（`str_char` 是底座原语，保持裸调用）
  - `std/tcmsg.tie` 内部 `str_starts_with` → `str.str_starts_with`（跨命名空间调用）
- **examples 全部改用命名空间调用**：`csv_demo` / `format_demo` / `std_demo` / `std_math_demo` /
  `tcmsg_demo`（`assert.assert`）、`import_main`+`lib_math`（`mathlib.*`）、
  `import_nested`+`lib_math2`/`lib_util`（`math2.*` / `util.*`，嵌套跨命名空间调用）

### 修复
- **命名空间函数返回动态表无法推断元素类型**（预存 bug）：语义层
  `dynamic_table_elem_ty` / `table_arg_elem_ty` 只支持裸调用（`Expr::Call`），
  `str.str_split` 等命名空间调用（`Expr::MethodCall`）返回表时调用点报
  「标注 table，初始化必须是表字面量 / table_new_* / 返回表的函数调用」。
  方案：新增辅助函数 `ns_call_full_name`（Call/MethodCall → 注册全名如 `str::str_split`），
  两处表元素类型推断统一走该解析；裸调用不做 funcs 校验（内建 `table_new_*` 不注册进 funcs，
  校验会误杀）
- **IR 层动态表变量初始化只识别裸调用**：`gen_dyn_table_var` 仅匹配 `Expr::Call`，
  `var p = str.str_split(...)` 走了新建空表分支（运行返回空表，len=0）。
  方案：MethodCall 初始化复用 `gen_expr` 调用分发；`dyn_table_elem_ty` 经语义层
  `resolved_calls`（表达式地址 → 全名）查 `table_ret_elems`
- tie-interp 测试 `std_format_helpers` 同步为命名空间调用（`format.format_int` 等）

### 测试
- 全工作区测试通过（frontend 112 / interp 49 / llvm 28 / lsp 53 / prep 4 = **246**）
- 命名空间返回表链路验证：`str.str_split` 编译 + 运行（len / table_at / 下标 / table_push 全通过）；
  std 库 6 文件独立编译为 `.a` 全成功；csv/format/std/std_math/tcmsg/import 系列示例编译运行全通过

## [Harbor M2.1.1] Windows 控制台 UTF-8 修复 — 2026-08-08

### 修复
- **Windows 控制台中文乱码**：工具链全部输出 UTF-8 字节，而控制台默认代码页为 GBK（936），
  导致中文显示为乱码（如 `REPL�?` = `REPL)。`）。
  方案：各 CLI 入口（tie / tie-prep / tie-frontend / tie-llvm / tie-lsp）启动时调用
  `init_console_utf8()`（tie-prep 与 tie-frontend 库各提供一份），通过 Windows API
  `SetConsoleOutputCP(65001)` / `SetConsoleCP(65001)` 把控制台输入/输出代码页切换为 UTF-8。
  REPL 场景：tie.exe 先切代码页再启动 repl.exe（子进程继承控制台），同样生效。
- **直接运行 tie.exe 一闪而过**：`find_repl_exe()` 只查 env / exe 同目录 / 当前目录三处，
  开发期 repl.exe 位于 `repl\` 子目录找不到，导致无参数运行时立即报错退出。
  方案：新增第 4 个查找路径（workspace 标准布局 `repl/repl.exe`）。
- **报错窗口一闪而过**：REPL 外壳缺失等报错路径新增 `pause_before_exit()`，
  仅当 stdin 是交互式终端时暂停（按任意键退出）；管道/重定向/CI 场景不阻塞。
- **REPL 输出顺序错乱与提示符不换行**：repl.exe（C 程序）编译路径的 print/println
  走 C `printf`（C stdio 缓冲），而 eval 解释路径的 print 走 Rust stdout（Rust 缓冲），
  两套缓冲写同一 fd 顺序错乱（`print("你好")` 的输出跑到欢迎语之前）。
  方案：编译路径的 `print`（不换行）在 printf 后追加 `fflush(stdout)` 立即刷出；
  REPL 外壳对无返回值（print 副作用）的输入补一个换行，提示符不再挤在同一行。

### 变更
- tie-prep 与 tie-frontend 的 Cargo.toml 展开 workspace lints，
  `unsafe_code` 从 forbid 放宽为 allow（仅用于 `init_console_utf8` 的 Windows API 调用）。

### 测试
- `cargo build --workspace` 零错误；直接运行 tie/tie-prep/tie-frontend/tie-llvm/tie-lsp
  中文输出正常；REPL 输出 `你好，世界` UTF-8 字节完整（E4 BD A0 ... E7 95 8C）；
  管道场景不暂停，交互终端才暂停。

## [Harbor M2.1] 默认值参数与 tcmsg 控制台信息库 — 2026-08-08

### 新增
- **命名空间（namespace）**：`namespace tcmsg { }` 块式声明（C# 风格），`::` 路径访问 +
  `.` 成员调用（`tcmsg.error.no_file(...)` / `tcmsg::error.no_file(...)`）；
  命名空间内裸调用自动前缀补全（`inner()` → `tcmsg::error::inner`）
- **动态表运行时**：`table_new_i64/f64/string/bool`（建空表）+ `table_push`（追加）+
  `table_at`（下标读取，越界报错）+ `len(table)`，编译（IR 走 C ABI 桥）与解释两路径一致；
  支持表字面量作函数实参（IR 按动态表构造）
- **新底座原语**：`list_dir`（目录列表 → 字符串动态表）、`table_*` 系列
- **std/string.tie 新增 `str_split`**：按分隔符切分为字符串动态表（连续分隔符产生空元素，
  与主流语言 split 语义一致）
- **std/csv.tie**：CSV 解析/生成（基于 str_split + 表操作）
- **std/format.tie**：格式化工具（format_int / format_pad 等）
- **默认值参数（语言特性）**：函数参数可带默认值，调用时可省略可选参数：
  - 语法：`func greet(name: string, prefix: string = "Hello")`（`参数名: 类型 = 字面量`）
  - 规则：可选参数**必须连续排在必选参数之后**；默认值限字面量（数/布尔/字符/字符串/空表 `[]`，
    非空表字面量/变量引用报「默认值必须是字面量」）；默认值类型须与形参类型匹配
  - 语义：函数调用点按实参数**区间检查**（`期望 N 个参数` / `期望 N-M 个参数`，N = 必选参数个数）；
    方法参数默认值暂不支持（报「方法默认值参数留待 M3」）
  - 双路径实现：LLVM 函数签名不变（含全部形参），缺省实参在**调用点**补齐（IR 层）；
    解释器 `call_fn` 区间检查 + 求值默认值补齐
- **语言底座原语 `msg_get_lang`**：读取消息系统当前语言（与 `msg_set_lang` 配套，返回 string），
  语义/IR/interp/C ABI 桥（`tie_msg_get_lang`）四层贯通，供标准库按当前语言匹配文本
- **`std/tcmsg.tie` 控制台信息库（命名空间形式，旧扁平 `tcmsg_*` API 废弃）**：
  - `tcmsg.register(key, lang, content)` / `tcmsg.t(key)` / `tcmsg.set_lang(lang)`
    （透传底座原语 `msg_register` / `msg_t` / `msg_set_lang`，回退规则：当前语言 → zh → 键本身）
  - `tcmsg.error/warn/info(key)` 分级打印（前缀「错误: / 警告: / 信息: 」+ 当前语言翻译）
  - **综合方案形态** `tcmsg::error.no_file(langs: table, texts: table = [])`（默认值参数落地实例）：
    - 方案 A（调用方传 `texts`）：`len(texts) > 0` → 按当前语言**前缀匹配** `langs`（地区码 `zh-cn`
      匹配基础码 `zh`）取对应文本，未命中回退第一个文本——文本随调用自包含
    - 方案 B（省略 `texts`，空表默认值）：`msg_t("error.no_file")` 查字典——单一事实来源
  - 依赖 `std/string.tie`（`str_starts_with` 前缀匹配；import 展开不去重，调用方只导入 tcmsg.tie 即可）
- **示例**：`examples/namespace_demo.tie`（命名空间）、`examples/table_dynamic.tie`（动态表）、
  `examples/csv_demo.tie`、`examples/format_demo.tie`、`examples/list_dir_demo.tie`、
  `examples/args_demo.tie`、`examples/tcmsg_demo.tie`（多语言登记/切换/查询、方案 A/B、回退规则断言）

### 修复
- 命名空间函数 `table` 形参布局元数据注册键用裸名 `f.name` 而非全名 `cur_fn` →
  命名空间函数内 table 下标访问报「缺少布局元数据」；改为以完整命名注册（预存 bug）
- 参数个数错误消息出现重复「个」字（`期望 1 个 个参数`）→ `param_count_desc` 返回区间描述
  （`"N"` / `"N-M"`），与模板「期望 {} 个参数」解耦
- `text` 是类型关键字不可作参数名 → `tcmsg.register` 参数名 `text` 改为 `content`

### 测试
- 全工作区测试通过（frontend 112 / interp 49 / llvm 28 / lsp 53 / prep 4 = **246**）
- 新增 frontend +2：默认值参数省略实参合法且类型校验、默认值参数限制规则
- 新增 interp +2：`eval_default_arg_省略与传参`（省略/显式传参/超参区间报错）、
  `eval_default_arg_tcmsg综合方案`（方案 B 查字典 / 方案 A 调用方文本）

### 文档
- docs/language.md：§6 函数新增「默认值参数」语法说明
- README.md：路线图 M2 条目补充 tcmsg 控制台信息库与默认值参数

## [Harbor M2] 标准库（std / math） — 2026-08-07

### 新增
- **语言底座原语（仅语言自身无法表达的部分，Rust 实现，三层 semantic/IR/interp 贯通）**共 21 个：
  - 文件：`file_read` / `file_write` / `file_append` / `file_exists`
  - 字符串与转换：`str_char`（Unicode 字符访问）、`to_string`、`parse_int`、`parse_float`
  - 进程与系统：`exit`、`time_now`（Unix 秒）、`rand_range`（`[min,max)` 区间随机）
  - 数学（libc，编译/解释两路径行为一致）：`sqrt` / `sin` / `cos` / `tan` / `exp` / `log` / `pow` / `floor` / `ceil` / `round`
  - `len` 扩展：支持 `table`（返回元素个数）
- **tie 语言自写标准库 `std/`（贯彻"能 tie 就 tie"）：**
  - `std/assert.tie` — `assert` / `assert_eq` / `assert_neq`（失败打印错误并退出）
  - `std/string.tie` — `str_trim` / `str_slice` / `str_contains` / `str_starts_with` / `str_ends_with` / `str_replace`（基于 `str_char`+循环拼接）
  - `std/math.tie` — `abs` / `max_i` / `min_i` / `clamp` / `is_odd` / `is_even` / `sign_i` / `deg_to_rad` / `rad_to_deg` 等（纯算术实现）
- **示例**：`examples/std_primitives.tie`、`examples/std_math_primitives.tie`（底座原语演示）、`examples/std_demo.tie`（文件+字符串+断言）、`examples/std_math_demo.tie`（数学库+原语）

### 变更
- 字符串/数字原语返回堆串统一走 tie-interp C ABI（`tie_free_result` 回收，无泄漏）；数学纯标量走 libc
- `to_string`/`parse_*` 共用同一 Rust 实现，保证编译与解释两路径**逐字节一致**（如 `to_string(1.0)→"1"`）
- `.gitignore` 增加 `/std/*.a`（标准库编译产物）

### 测试
- 全工作区测试通过（frontend 88 / interp 32 / llvm 28 / tie 53 / lsp 4）
- 新增 interp 单测覆盖 21 原语 + `len(table)` 边界（多字节 UTF-8、越界、文件读写往返、追加、parse 非法输入、rand 越界）

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
