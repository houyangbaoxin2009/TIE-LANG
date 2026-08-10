# 自举阶段 2 实现任务书：semantic / ir / main + Rust 壳整合

> 状态：**任务书**（2026-08-10 编写，供新 AI 直接实施）
> 所属：自举里程碑阶段 2 剩余任务（docs/plans/self-hosting.md 阶段 2）
> 已完成：`compiler/lexer.tie`（ab99602）、`compiler/parser.tie` + `compiler/PROTOCOL.md`（2a676a5）
> 本文档：semantic → ir → main → Rust 壳 → 自举校验，全链路规格与验收标准
> 参考实现（权威行为基准）：crates/tie-frontend/src/semantic.rs（6119 行）、
> crates/tie-llvm/src/ir.rs（5111 行）、crates/tie-llvm/src/driver.rs（394 行）、
> crates/tie-llvm/src/backend.rs、crates/tie-llvm/src/optimizer.rs

## 0. 新 AI 开工前必读

1. **先读** `compiler/PROTOCOL.md`（token/AST 协议，唯一权威编号表）与 `compiler/lexer.tie`、
   `compiler/parser.tie`（既有模块的架构模式：自包含、全局标量、协议文本累积）。
2. **运行环境**：`cargo build --workspace` + 调试用 `target/debug/tie-llvm.exe`（编译
   tie 程序）、`target/debug/tie-frontend.exe`（对照 Rust 版输出）。
3. **验证驱动模式**：临时 tie 程序用内置 `eval`/`eval_call` 注册模块并调用
   （`eval_call("lexer::tokenize", src)`；命名空间函数在 tie 源码里用**点号**
   `parser.parse(...)` 调用，`::` 只用于 eval_call 字符串全名）。
4. **链接陷阱**：改 interp/语义/IR 后必须 `cargo build -p tie-interp -p tie-llvm`
   且 `cargo build --release -p tie-interp`（tie-llvm 的 resolve_interp_lib 优先
   target/release 的旧 lib，否则运行时"未定义的函数"）。
5. **tie 语言约束**（全部模块适用，见各模块文档）：
   - 不能 import（interp eval 注册路径）；模块自包含；
   - 全局变量必须标量（表不能作全局变量）→ 用全局字符串/标量 + 函数参数传表；
   - `&&`/`||` **非短路**（条件里带副作用调用必须嵌套 if）；
   - `num`/`text`/`char`/`code`/`misc`/`bool`/`table`/`map` 是类型关键字，不能作变量/参数名；
   - 协议文本跨 C ABI（eval_call）会被 CString 截断 `\0` → 字符串字段一律转义表示。

---

## 1. 模块 3：semantic.tie（语义分析器）

### 1.1 接口

```
parser::parse 输出 AST: 协议文本 → semantic::check(ast_text) -> string
成功：OK:<expr_type_count>
失败：ERR:<line> <col> <message>   （与 §4 错误协议一致；消息与 Rust 版逐字对齐）
```

### 1.2 AST 协议解析（输入侧）

AST 协议格式见 PROTOCOL.md §5。semantic 需要把协议文本解析为**内存中的可遍历结构**。
由于 tie 全局不能存表，采用与 parser 相同的模式：

- `AST:<n> <pool>` 头 + 逐行节点 + `POOL:<m>` + 池行；
- 解析为**局部表**（函数内 table）：`node_tags: table<i64>`、`node_lines`、
  `node_cols`、`node_names`（池 id）、`node_vals`、`node_aux`、
  `node_children: table<table<i64>>`（每个节点的子节点 id 表）、`pool: table<string>`；
- **关键设计决策**：节点行 id = 行序，parse 输出是**追加式文本**，semantic 必须
  先完整解析进局部表再遍历。由于 semantic 的检查函数需要随机访问节点
  （查父/子/兄弟），而 tie 表是值传递——**用模块级「工作表」全局变量不行**
  （全局必须标量）。方案：**所有检查函数接收节点表参数**（`table<i64>` 等），
  或把节点表编码为**全局字符串** + 按 id 惰性解析（id 是行号，用 find_char 定位
  第 i 行）。推荐后者：`node_row(i) -> string`（取第 i 行文本）+ `field(s, k)` 解析
  字段，避免表参数在递归中反复拷贝（性能）。
- 行解析辅助：`row_field(line, k) -> string`（空格分隔字段，k=0..）、
  `row_children(line) -> table<i64>`（第 6 字段起是子节点 id）。

### 1.3 类型系统表示（tie 侧）

AST 类型节点 tag 200-224（PROTOCOL.md §5.4）。semantic 内部用**类型编号表**
（table<i64>）表示 TypeSpec：
- `ty_tag`: 200-224 基本/table/map/tuple/struct；
- `ty_name`: 池 id（struct 名 / 元组字段名）；
- `ty_child`: 子类型节点 id（table<T>/map<T> 的元素类型）；
- 元组类型：`ty_fields: table<table<i64>>`（每字段 [name_id, ty_node]）。

类型判断辅助（对应 Rust 版）：
- `is_int(t)`：tag ∈ {200,203,204,205,206,207,208,209}（i64/i32/i16/i8/u64/u32/u16/u8）；
- `is_float(t)`：tag ∈ {201,202}；`is_number(t)` = is_int || is_float；
- `is_bool_like(t)`：tag == 210（bool）或 trit(214)；
- `type_name(t) -> string`：对应 Rust `type_name`（i8..map 名字 / table<X> / map<X> /
  tuple / struct 名）；
- `types_compatible(a, b)`：表/元组递归比较（见 Rust 版 4058 行）；
- `types_match(want, got, init_expr)`：字面量适配规则（见 §1.7）。

### 1.4 符号表与上下文（全局标量 + map）

semantic 需要多张「名字 → 信息」表，用 **E3 键值表 map**（值类型 i64 = 节点 id 或
类型 tag）：

| 表（全局字符串承载或 map） | 内容 | 对应 Rust |
| --- | --- | --- |
| `sm_funcs`（map：函数全名→签名节点 id） | 签名 = [param_tys..., ret_ty, is_pub] | result.funcs |
| `sm_globals`（map：全局变量名→类型节点 id） | M4 顶层持久变量 | result.globals |
| `sm_const_globals`（map 或表） | const 全局名集合 | const_globals |
| `sm_const_vars`（map） | 函数内 const 变量 | const_vars |
| `sm_table_vars`（map：`cur_fn::name`→TableInfo 节点） | 表变量布局 | table_vars |
| `sm_table_ret_elems`（map：函数全名→元素类型节点） | 返回表的函数 | table_ret_elems |
| `sm_classes`（map：struct 名→拍平信息节点） | 继承拍平 | classes |
| `sm_fn_full_names`（map：FnDef 节点 id→全名） | 命名空间函数全名 | fn_full_names |
| `sm_resolved_calls`（map：调用节点 id→全名） | 裸调用解析结果 | resolved_calls |
| `sm_using_prefixes`（map：序→路径） | using 引入前缀 | using_prefixes |
| `sm_import_views`（map：序→[alias, ns_paths]） | 导入视图 | import_views |

> **map 的可用性**：E3 键值表（`["a":1]` 字面量、`m["k"]` 读写、len）在 interp 与
> 编译路径均可用（interp Value::Map）。值类型需同构——用 i64（节点 id/类型 tag）
> 或 `table<i64>` 嵌套。注意：**map 键查不到时 interp 报"map_get 键不存在"**，检查
> 存在性需先用 `contains_key`（若有该内置）或 try 捕获——若 tie 无 contains_key
> 内置，则用「哨兵值」约定（如 -1 = 不存在）。

### 1.5 三遍分析流程（对应 Rust `analyze` 106-316 行）

```
check(ast_text):
  1. 解析 AST 协议 → 节点访问层就绪
  2. 第零遍 collect_imports_using：顶层 import/using（M2.1.7）
  3. 第一遍收集函数签名：
     - 顶层 FnDef：注册裸名签名（重复→报错"函数 '{}' 重复定义"）
     - Namespace：collect_ns_funcs 递归注册全名（"路径::函数名"，
       重复→"函数 '{}' 在命名空间 '{}' 中重复定义"）
     - 顶层 VarDecl（全局持久变量）：显式标量类型 + 字面量初始化 + 命名冲突
       （4 条错误消息见 §1.9）
  4. collect_structs：struct 名登记/冲突检查 → 逐个 flatten_struct（继承拍平 +
     环检测 + 字段唯一性 + 字段类型解析）
  5. 表返回预扫描 fixpoint：scan_return_table_elem + collect_local_dyn_tables，
     直到不再新增（内置 list_dir/walk_dir/byte_read/byte_concat/regex_find_all
     预登记 table_ret_elems）
  6. 第二遍 check_fn / check_ns_stmts（逐函数检查，含命名空间内）
  7. 返回 OK:（成功）或首个 ERR:
```

### 1.6 check_fn（对应 597-697 行）

1. `cur_fn` = 全名（顶层裸名 / ns::路径::函数名）；
2. 参数入作用域（重复→"参数 '{}' 重复"）；
3. 默认值参数校验：可选参数连续排在必选后 + 默认值限字面量/空表 + 类型匹配
   （3 条错误消息）；
4. 表参数登记 table_vars（`table<T>` 元素类型 = T；裸 table 默认 string；
   dynamic=true）；
5. 逐语句 check_stmt（作用域随块嵌套，返回类型传入）。

### 1.7 check_stmt（对应 840-1543 行）

**作用域管理**：Rust 用 `HashMap<String, TypeSpec>` 传引用；tie 用**全局字符串
累积作用域**（`sc_entries` 每行 `name type`）+ 块级 push/pop（记录行数边界）。
变量查找：当前作用域栈（从内到外）→ 全局变量表。

| 语句 | 检查内容 | 关键错误消息 |
| --- | --- | --- |
| VarDecl | 推断 init 类型；显式类型匹配（宽类型类别框/table/map/tuple 特例）；const 记录 | "变量 '{}' 类型不匹配：标注 {}，表达式推导为 {}"、"变量 '{}' 标注 {} 不匹配初始化表达式的类型 {}"、"变量 '{}' 不能用 void 表达式初始化"、二维表/字符串 id 表 M3 报错、表字面量元素类型登记 tables/table_vars |
| Assign | 目标已声明（作用域→全局）；const 不可赋值；复合赋值按运算符 | "赋值目标 '{}' 未声明"、"不能给 const 变量 '{}' 赋值"、"赋值类型不匹配：变量 '{}' 类型为 {}，表达式为 {}" |
| Return | 类型匹配（含元组覆盖写、动态表返回记录） | "return 类型不匹配：函数返回 {}，实际返回 {}" |
| If | 条件 bool（else-if 链迭代检查，**不递归**——语义层已改迭代，tie 版同样迭代） | "if 条件必须是 bool" |
| While | 条件 bool；标签入栈/出栈 | "while 条件必须是 bool" |
| For | iter 是范围（元素 i64）或表（元素类型查 table_vars） | "for 迭代对象仅支持范围（0..10）或表变量，实际是 {}" |
| Break/Continue | 循环上下文 + 标签匹配（loop_labels 栈） | "{kw} 只能出现在循环体内"、"{kw} 的标签 '{l}' 未匹配任何外层循环" |
| Switch | subject 类型限数字/bool/char/string；case pattern 校验（字面量/区间/类型匹配）+ when 布尔 + 重复检测 | "switch 对象仅支持数字、布尔、字符或字符串类型，实际是 {}"、"case 区间两端必须是整数或字符字面量（浮点区间不支持）"、"case 区间必须 start < end（左闭右开）"、"重复的 case 值 {}"、"when 守卫必须是布尔表达式，实际是 {}" 等 |
| Expr | 推断类型记录 | — |
| FieldAssign | base 是 struct 实例 + 字段存在 + 类型匹配 + 复合赋值 | "字段赋值的对象必须是 struct 实例，实际是 {}"、"类 '{}' 没有字段 '{}'"、"字段赋值类型不匹配：'{class}.{field}' 类型为 {}，表达式为 {}" |
| IndexAssign | base 是表（table_vars）或 map；下标整数/字符串键；值类型匹配 | "下标赋值的对象必须是表，实际是 {}"、"下标必须是整数，实际是 {}"、"键值表值类型不匹配：map 值为 {}，表达式为 {}"、"下标赋值类型不匹配：表元素类型为 {}，表达式为 {}" |
| FnDef（函数体内） | 不支持 | "函数体内不支持嵌套函数定义" |
| Import/Using（函数体内） | 仅顶层 | "import 语句只能出现在文件顶层"、"using 语句只能出现在文件顶层" |
| Struct（函数体内） | 仅顶层 | "struct 定义只能出现在文件顶层" |

### 1.8 infer_expr（对应 1631-3513 行，最核心）

| 表达式 | 推断/检查 | 关键错误消息 |
| --- | --- | --- |
| IntLit | i64 | — |
| FloatLit | f64 | — |
| BoolLit | bool | — |
| TritLit | trit | — |
| StrLit/CharLit | string/char | — |
| TypeLit | 普通上下文报错 | "类型字面量 {} 只能用作 switch 的 case 类型匹配" |
| Var | 作用域→全局 | "未声明的变量 '{name}'" |
| Path | 不能作值 | "命名空间路径 '{}' 不能作为值使用（只能用于调用，如 '{}::xxx()'）" |
| Call（struct 构造） | 类名命中：参数 ≤ 字段数、逐个类型匹配 | "构造 '{name}' 最多 {} 个参数（字段数），实际 {} 个"、"构造 '{name}' 参数类型不匹配：字段 '{}' 期望 {}，实际 {}" |
| Call（内置函数） | 见 §1.9 内置函数表 | 各函数专属消息 |
| Call（用户函数） | resolve_bare_call 解析 → 可见性 → 参数个数区间 → 逐参类型 + A1 表元素校验 | "未定义的函数 '{name}'"、"函数 '{call_name}' 期望 {} 个参数，实际 {} 个"、"调用 '{call_name}' 参数类型不匹配：期望 {}，实际 {}"、"调用 '{call_name}' 表参数元素类型不匹配：期望 table<{}>，实际 table<{}>" |
| Unary | Neg 数字 / Not bool 或 trit / 自增自减可写数字变量 | "取负运算的操作数必须是数字"、"逻辑非的操作数必须是 bool"、"不能对 const 变量 '{name}' 自增/自减"、"自增/自减的操作数必须是可写数字变量" |
| Binary | 两侧类型一致（trit×i64 例外）；按运算符分组：算术/字符串+/trit/比较/逻辑/位运算 | "二元运算两侧类型不一致：{} 与 {}"、"算术运算符不能用于 {}"、"取模运算只支持整数"、"trit 不支持除/取模运算（三值无除法）"、"比较运算符不能用于 {}"、"逻辑运算符两侧必须是 bool（或两侧同为 trit）"、"位运算只支持整数，不能用于 {}"、"元组暂不支持比较运算（逐字段比较留待后续版本）" |
| Ternary | 条件 bool + 两分支类型一致 | "三目条件必须是 bool"、"三目两分支类型不一致：{} 与 {}" |
| Range | 两端整数 | "范围两端必须是整数" |
| TableLit | 元素类型一致（空表 i64）；map 检测（字符串 id 全有/全无）；嵌套表 | "表元素类型不一致：{} 与 {}"、"键值表元素必须全部带字符串键（[\"a\":1]），不能混用位置元素" |
| Index | base 表/map/字符串；下标整数/字符串键；元素类型查 tables/table_vars/table_ret_elems | "键值表下标必须是字符串键，实际是 {}"、"下标必须是整数，实际是 {}"、"下标访问的对象必须是表或字符串，实际是 {}"、"下标访问的调用 '{}' 不是返回表的函数" |
| TupleLit | 逐字段推断 + 字段名查重 + 空元组拒绝 | "元组字段名 '{n}' 重复"、"空元组 () 不支持" |
| FieldAccess | base 元组（命名/ItemN/数字）或 struct（可寻址 + 字段存在） | "元组没有字段 '{field}'（元组字段为 {}）"、"类 '{}' 没有字段 '{field}'"、"struct 实例 '{class_name}' 的字段访问需要可寻址对象（变量/字段链）"、"字段访问 '.' 的对象必须是元组或类实例，实际是 {}" |
| MethodCall | 命名空间调用（Path/未绑定链 + import 映射）→ 全名解析 + 可见性 + 参数；struct 方法转发（沿继承链查 `T::method`，首参接收者，可寻址） | "命名空间函数 '{full}' 未定义"、"命名空间函数 '{full}' 期望 {} 个参数，实际 {} 个"、"调用 '{full}' 参数类型不匹配：期望 {}，实际 {}"、"struct '{struct_name}'（含继承链）没有方法 '{method}'：请在 namespace {struct_name} 中定义 pub func {method}(首参: {struct_name}, ...)"、"方法调用的对象必须是 struct 实例或 struct 名，实际是 {}"、"方法调用的对象需要可寻址的 struct 实例（变量/字段链），{} 不可取地址"、"方法 '{full}' 首参类型不匹配：期望 {}，实际 {}" |

**表达式结果类型记录**：成功推断后把「节点 id → 类型」记入 `sm_expr_types`（map，
供 IR 阶段查询）。这是 SemanticResult.expr_types 的 tie 对应。

### 1.9 内置函数签名表（infer_expr 的 Call 分支，对应 1709-2742 行）

按「参数个数 + 参数类型 + 返回类型」分组实现（每组一个函数），消息格式统一
`{name}() 期望 N 个参数，实际 {} 个` / `{name}() 参数必须是字符串，实际是 {}` 等：

| 组 | 函数 | 参数 | 返回 |
| --- | --- | --- | --- |
| println/print | 任意（元组拒绝） | void |
| len | 1（string/表/map） | i64 |
| str_len | 1（string） | i64 |
| table_new_i64/f64/string/bool | 0 | table |
| table_push | 2（表变量, 元素） | void |
| table_at | 2（表, 整数下标） | 元素类型 |
| read_line | 0 | string |
| eval | 1（string） | string |
| eval_call | 2（string, string） | string |
| file_read/file_delete/file_exists | 1（string） | string/bool/bool |
| file_write/file_append | 2（string, string） | bool |
| str_char | 2（string, 整数） | string |
| char_code | 1（string） | i64 |
| to_string | 1（数字/trit） | string |
| parse_int/parse_float/parse_trit | 1（string） | i64/f64/trit |
| exit | 1（整数） | void |
| time_now/arg_count/msg_get_lang/cwd | 0 | i64/i64/string/string |
| rand_range/arg_string | 2 整数 / 1 整数 | i64 / string |
| list_dir/walk_dir | 1（string） | table |
| http_get/exec_output/path_* /get_env | 1（string） | string |
| mkdir_all/remove_dir_all | 1（string） | bool |
| exec_code | 1（string） | i64 |
| byte_read/byte_write/bit_read/bit_write/byte_concat | D7 字节原语 | table/bool/i64/bool/table |
| http_get_file/untar_gz/unzip/copy_dir/file_copy/file_move | 2（string, string） | bool |
| path_join | 2（string, string） | string |
| set_env | 2（string, string） | void |
| msg_set_lang/msg_t/print_err | 1（string） | void/string/void |
| msg_register | 3（string×3） | void |
| msg_t_lang | 2（string, string） | string |
| sqrt/sin/cos/tan/exp/log/floor/ceil/round | 1（数字） | f64 |
| pow | 2（数字, 数字） | f64 |
| regex_match/regex_find/regex_find_all | 2（string, string） | bool/string/table |
| regex_replace/regex_group | 3 | string |

> **重要**：完整签名校验细节以 Rust 版 semantic.rs 对应分支为准（逐个核对参数
> 类型消息）。消息中的 `{name}` 用实际函数名。

### 1.10 命名空间/import/using 解析（M2.1.7，对应 354-537 行）

- `collect_imports_using`：import 收集视图（alias + ns_paths）；using 目标必须命中
  已导入前缀/别名（"using 目标 '{}' 未导入：using 只能引用 import 引入的命名空间前缀或别名"）、
  重复 using 报错；
- `resolve_bare_call`：裸名 → ns_stack 前缀补全 → using 前缀（多候选歧义报错）
  —— 对应"裸调用 '{name}' 有歧义：多个 using 引入的命名空间都有该函数，请用命名空间前缀调用"；
- `map_import_prefix`：别名唯一入口（原前缀被屏蔽）——"命名空间前缀 '{}' 已被别名 '{}' 取代（import as 唯一入口），不能直接使用"；
- `check_visibility`：私有函数仅同命名空间可调——"命名空间函数 '{full}' 是私有函数（默认私有，`pub func` 显式导出），不能跨命名空间调用"；
- `ns_path_segments` / `ns_prefix_exists` / `ns_call_full_name`：receiver 路径拍平与存在性。

### 1.11 semantic 验收标准

1. 对全仓 tie 文件（examples/std/ext/prep/pkg/repl/compiler）`check` 全部返回 `OK:`，
   与 Rust 版 `tie-frontend --check` 结果一致（成功静默 / 失败首错对齐）；
2. 构造错误样例（每类至少 1 个：未声明变量/类型不匹配/const 赋值/循环外 break/
   switch 类型错/表元素不匹配/私有调用/struct 字段缺失/方法转发等），错误消息
   **逐字对齐** Rust 版（用 tie-frontend 输出对照）；
3. 与 parser 联合：`lexer→parser→semantic` 全链路通过。

---

## 2. 模块 4：ir.tie（LLVM IR 生成）

### 2.1 接口

```
semantic::check 输出 OK 后 → ir::gen(ast_text) -> string
成功：IR: 开头的 LLVM IR 文本（首行 "; ModuleID = 'tie'"）
失败：ERR:<line> <col> <message>
```

输入仍是 AST 协议文本（ir 自行解析节点表，不依赖 semantic 的内部表——但需要
**类型推断结果**。决策：semantic 的 `sm_expr_types`（节点 id→类型）通过附加协议
段传给 ir，或 ir 在生成时对每个表达式节点**现场重推断类型**。推荐前者：semantic
输出 `TYPES:<n>` 段（每行 `node_id ty_tag [ty_name] [ty_child]`），ir 读取后按
节点 id 查类型。**此协议段需补充进 PROTOCOL.md §5**（semantic 输出格式由
`OK:` 扩展为 `OK:<expr_count>` + `TYPES:` 段，或独立协议，由实施者定并更新文档）。

### 2.2 LLVM IR 输出结构（对应 ir.rs 87-201 行）

```
; ModuleID = 'tie'
source_filename = "input.tie"

declare i32 @printf(ptr, ...)
declare i64 @strlen(ptr)
declare i32 @strcmp(ptr, ptr)
declare ptr @malloc(i64)
declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)
declare ptr @fopen(ptr, ptr)
declare i64 @fwrite(ptr, i64, i64, ptr)
declare i32 @fclose(ptr)
declare i32 @fflush(ptr)
declare void @exit(i32)
declare i32 @remove(ptr)
declare double @sqrt(double) ... @round(double)     ; 数学原语
（全局变量 @name = global Ty 字面量）
define i32 @main() { ... }                          ; void main → i32 + ret i32 0
define <ret> @func$name(...) { ... }                ; 命名空间函数 = @ns$func（$ 连接）
...
（字符串常量 @.str.N = private constant [N x i8] c"..."）
（按需 declare tie_* 桥：tie_read_line/tie_eval_expr/tie_eval_call/tie_free_result/
  tie_file_read/tie_str_char/tie_char_code/tie_str_len/tie_to_string_i64/f64/
  tie_parse_int/float/trit/tie_time_now/tie_rand_range/tie_arg_count/tie_arg_string/
  tie_list_dir/tie_regex_* 等，仅用到的输出）
```

### 2.3 生成器状态（tie 全局标量）

| 状态 | 用途 |
| --- | --- |
| `ir_out`（全局字符串） | 主输出累积 |
| `ir_globals`（全局字符串） | 延迟输出的全局常量 |
| `ir_reg`（i64） | 寄存器计数器（%1, %2...） |
| `ir_str_count`（i64） | 字符串常量编号 |
| `ir_used_externs`（全局字符串，每行一个） | 按需 extern 声明收集 |
| `ir_scope`（全局字符串累积） | 变量绑定栈（name → 寄存器/地址 + LLVM 类型） |
| `ir_cur_fn`（string） | 当前函数全名 |
| `ir_loop_ctx`（全局字符串累积） | 循环上下文（label 与出口/continue 块名） |
| `ir_entry_allocas`（全局字符串累积） | F1：entry block 延迟 alloca |
| `ir_ty_cache` / `ir_fmt_cache` | 类型/格式字符串缓存（可省略） |

### 2.4 核心生成函数（对应 ir.rs 函数清单）

| tie 函数 | 对应 Rust | 说明 |
| --- | --- | --- |
| gen_ir | gen_ir/run | 模块头 + 全局 + 各函数 + 延迟全局 + 按需 extern |
| gen_globals | gen_globals | `@name = global Ty 字面量` |
| gen_ns_fns | gen_ns_fns | 递归命名空间函数（全名 @ns$func） |
| gen_fn | gen_fn | 函数签名 + 参数 alloca（entry）+ 体 + return 处理 |
| gen_stmt | gen_stmt | 语句分派（var/if/while/for/switch/return/expr/assign/field/index） |
| gen_expr | gen_expr | 表达式（返回 [寄存器, LLVM 类型]） |
| gen_binary | gen_binary_on_regs | 二元运算（含字符串拼接/比较/trit/位运算） |
| gen_call | gen_call_inner | 调用（内置/用户/构造/命名空间/方法） |
| gen_index | gen_index | 表下标（定长/动态/字符串/map） |
| gen_if/while/for/switch | 对应 | 控制流（for 拆 range/表/动态表三种） |
| gen_printf | gen_printf | println/print（元组拒绝已由语义层拦截） |
| gen_runtime_error | 对应 | 运行时错误（printf + exit） |
| new_reg/new_label/emit_alloca | 对应 | 寄存器/标签/延迟 alloca（F1） |
| renumber_ir | renumber_ir | 全局重编号（alloca 提升后编号倒挂修复） |

### 2.5 关键 IR 生成细节（必须对齐）

1. **类型映射**（tie 类型 → LLVM）：i8/i16/i32/i64 → i8/i16/i32/i64；u* → 对应位宽
   i*；f32/f64 → float/double；bool → i1；char → i32；string → ptr；table 定长 →
   `[N x T]`，动态 → `{ptr, i64, i64}`；map → `{ptr, i64, i64}`（16 字节元素）；
   tuple → `{T1, T2, ...}`；struct → 拍平字段 `{...}`；
2. **F1 alloca 提升**：所有 alloca 发射到 entry block 末尾（`ir_entry_allocas`
   累积，函数收尾统一 flush），命名用独立计数器避免与运行期块冲突；
3. **C5 switch 跳转表**：整数 case 生成 LLVM `switch`（can_emit_switch_table 前置
   检查）；字符串 case 走 strcmp 比较链；
4. **main 签名**：void main → `define i32 @main()` + `ret i32 0`（MSVC CRT 用
   EAX 当退出码）；非 void main 按返回类型；
5. **字符串常量**：`@.str.N = private constant [N x i8] c"..."` + 长度前缀
   （string 是 {ptr, len} 结构）；
6. **命名空间函数符号**：`@ns$func`（`::` 替换为 `$`）；
7. **按需 extern**：只有用到的 tie_* 桥才 declare（used_externs 收集）；
8. **renumber_ir**：F1 提升后 entry 块编号可能倒挂，按文本出现顺序重映射 %N。

### 2.6 ir 验收标准

1. 对全仓 tie 文件 `gen` 输出合法 LLVM IR，`opt -O2` + `clang` + `lld` 链接成功
   （调用 tie-llvm 的 backend 工具链）；
2. 编译出的可执行文件运行结果与 Rust 版编译产物**一致**（同一输入文件，比较
   stdout + 退出码）；
3. `--emit-ir` 输出与 Rust 版**逐字节一致**（或允许寄存器编号/命名差异的白名单
   对照——先做语义等价对照，再逐步收紧到字节一致，这是自举闭环的验收核心）；
4. 自举两轮：用 tie 编译的 `compile` 编译 compiler/*.tie 自身，产物与第一轮一致。

---

## 3. 模块 5：main.tie（compile 入口）

### 3.1 接口

```
compiler::compile(src: string) -> string    // 源码 → LLVM IR 文本
流程：lexer::tokenize(src) → parser::parse(tokens) → semantic::check(ast)
      → ir::gen(ast) → 返回 IR 文本（或 ERR:）
```

- namespace compiler，顶层注册全部模块（lexer/parser/semantic/ir 用 eval 逐个注册）；
- `compile` 是 eval_call 约定入口（恰好 1 个字符串参数）；
- 出错时返回 `ERR:<line> <col> <message>`（与各阶段错误协议一致，Rust 壳识别
  `ERR:` 前缀转为编译错误）。

### 3.2 Rust 壳整合（crates/tie-llvm/src/driver.rs 改造）

当前 driver 调用 `tie_frontend` 全流程 + ir.rs。改造为：

1. 编译开始时读入 `compiler/lexer.tie`、`parser.tie`、`semantic.tie`、`ir.tie`、
   `main.tie` 五个文件源码；
2. 用 `tie_interp::Session` 逐个 eval 注册（`session.eval(module_src)?`）；
3. 对输入文件：读源码 → 规范化（去 BOM/CRLF，与现 driver 一致）→
   `session.eval_call("compiler::compile", src)` → 得到 IR 文本或 ERR:；
4. 后续流程不变：IR 文本写临时文件 → `opt -O2` → `clang -c` → `lld` 链接 →
   可执行文件（backend.rs/optimizer.rs 保持 Rust）；
5. **保留 Rust 版全流程作为对照**（自举校验用）：新增 `--selfhost-check` 选项，
   同一输入分别走 Rust 版与 tie 版 compile，比较 IR 输出（见 §4）。

> 注意：eval_call 每次新建 Session 会丢失模块注册——模块注册要**每进程一次**
> （静态 lazy 或全局 once），compile 调用复用同一 Session。tie_interp 的
> `with_session` 是 thread_local 单例，Rust 壳直接调用 `Session::eval` 多次注册
> 后 `eval_call` 即可（同一 Session 持久）。

---

## 4. 自举闭环校验（验收核心）

### 4.1 两轮编译一致性

```
第 1 轮：Rust 壳（tie 版 compile）编译 compiler/*.tie → compiler.exe（或 .ll）
第 2 轮：compiler.exe 编译 compiler/*.tie → 相同产物
验收：两轮 .ll 输出逐字节一致（寄存器编号/常量命名也一致，因为同一生成器）
```

### 4.2 双实现对照（tie vs Rust）

- 对 examples/std/ext/prep/pkg/repl/compiler 全部文件：
  - lexer：`tie-frontend --tokens` ↔ lexer 协议（已有对照脚本模式）；
  - parser：`tie-frontend --ast` ↔ parser AST 协议（结构对照：节点 tag 序列 +
    关键字段）；错误场景逐字对照；
  - semantic：`tie-frontend --check` ↔ semantic 输出（成功/首错对齐）；
  - ir：`tie-llvm --emit-ir` ↔ ir 输出（语义等价 → 逐步字节一致）；
- 运行级对照：同一文件分别编译运行，stdout + 退出码一致。

### 4.3 验收门槛（全部满足才算阶段 2 完成）

1. compiler/ 目录 100% tie 源码（lexer/parser/semantic/ir/main + PROTOCOL.md）；
2. Rust 侧仅剩：语言底座 + tie:script 壳（eval 注册 + eval_call）+ 后端工具调用；
3. 全仓文件经 tie 版 compile 编译运行行为与 Rust 版一致；
4. 自举两轮输出一致；
5. workspace 编译零错误、测试全绿。

---

## 5. 实施顺序建议

1. **semantic.tie**（最大，约 2500 行）：先搭 AST 协议解析层 + 类型辅助，再
   check_stmt 语句骨架，再 infer_expr 表达式（内置函数表分组实现），再命名空间
   解析，最后 collect_structs + 表返回 fixpoint。每完成一块即用 Rust 版对照。
2. **PROTOCOL.md 补充**：semantic 输出格式（OK + TYPES 段）与 ir 输入格式。
3. **ir.tie**（约 2000 行）：先模块头 + 全局 + 函数骨架，再表达式/语句，再表/
   struct/元组/switch，最后 F1 alloca + renumber。
4. **main.tie + Rust 壳**：驱动改造（eval 注册 + eval_call）+ 对照开关。
5. **自举闭环**：两轮校验 + 全仓回归 + 文档（README/CHANGELOG）更新。

## 6. 相关文件

| 文件 | 作用 |
| --- | --- |
| compiler/PROTOCOL.md | token/AST 协议权威（§5 待补 semantic/ir 段） |
| compiler/lexer.tie / parser.tie | 既有模块（架构模式参考） |
| crates/tie-frontend/src/semantic.rs | semantic 权威行为基准（6119 行） |
| crates/tie-llvm/src/ir.rs | IR 生成权威基准（5111 行） |
| crates/tie-llvm/src/driver.rs / backend.rs / optimizer.rs | Rust 壳整合点 |
| docs/plans/self-hosting.md | 阶段 2 总规划（B1/C1 编码方案） |
| crates/tie-frontend/src/ast.rs | AST 类型定义（协议映射基准） |

---

## 附录 A：语义错误消息全集（190 条，与 Rust 版逐字对齐）

> 从 crates/tie-frontend/src/semantic.rs 提取（message: format! 与 message: 字面量）。
> 实施时以本附录为对齐基准：错误消息文本必须逐字一致（占位符 {} 按上下文填充）。
> 注意：此清单按提取顺序排序，部分消息格式串含换行续行（如 struct 方法提示），
> 实施时以 Rust 源码实际文本为准。
- `'{}' 不是表变量`
- `{kw} 的标签 '{l}' 未匹配任何外层循环`
- `{kw} 只能出现在循环体内`
- `{name}() 参数必须是数字（i64/f64），实际是 {}`
- `{name}() 参数必须是字符串，实际是 {}`
- `{name}() 期望 0 个参数，实际 {} 个`
- `{name}() 期望 1 个参数，实际 {} 个`
- `{name}() 期望 2 个参数，实际 {} 个`
- `比较运算符不能用于 {}`
- `变量 '{}' 标注 {} 不匹配初始化表达式的类型 {}`
- `变量 '{}' 标注 table，初始化必须是表字面量 [...] 或 table_new_* / 返回表的函数调用`
- `变量 '{}' 不能用 void 表达式初始化`
- `变量 '{}' 类型不匹配：标注 {}，表达式推导为 {}`
- `表参数 '{}' 的元素类型未知，无法确定 table_at 返回类型`
- `表元素类型不一致：{} 与 {}`
- `不能对 const 变量 '{name}' 自增/自减`
- `不能给 const 变量 '{}' 赋值`
- `参数 '{}' 的默认值必须是字面量（数/布尔/字符/字符串或空表 []）`
- `参数 '{}' 默认值类型不匹配：期望 {}，实际 {}`
- `参数 '{}' 缺少默认值：可选参数（带默认值）必须连续排在必选参数之后`
- `参数 '{}' 重复`
- `调用 '{call_name}' 表参数元素类型不匹配：期望 table<{}>，实际 table<{}>`
- `调用 '{call_name}' 参数类型不匹配：期望 {}，实际 {}`
- `调用 '{full}' 参数类型不匹配：期望 {}，实际 {}`
- `二元运算两侧类型不一致：{} 与 {}`
- `方法 '{full}' 期望 {required}-{total} 个参数（含接收者对象），实际 {n_args} 个`
- `方法 '{full}' 首参类型不匹配：期望 {}，实际 {}`
- `方法调用的对象必须是 struct 实例或 struct 名，实际是 {}`
- `方法调用的对象需要可寻址的 struct 实例（变量/字段链），{} 不可取地址`
- `父 struct '{p}' 未定义`
- `复合赋值类型不匹配：目标类型 {} 与表达式 {}`
- `复合赋值取模只支持整数，目标类型是 {}`
- `复合赋值位运算只支持整数，目标类型是 {}`
- `复合赋值运算符不能用于 {}`
- `赋值类型不匹配：变量 '{}' 类型为 {}，表达式为 {}`
- `赋值目标 '{}' 未声明`
- `构造 '{name}' 参数类型不匹配：字段 '{}' 期望 {}，实际 {}`
- `构造 '{name}' 最多 {} 个参数（字段数），实际 {} 个`
- `函数 '{}' 在命名空间 '{}' 中重复定义`
- `函数 '{}' 重复定义`
- `函数 '{call_name}' 期望 {} 个参数，实际 {} 个`
- `函数 '{call_name}' 是命名空间 '{prefix}' 的私有函数（默认私有，`pub func` 显式导出），不可在命名空间之外调用`
- `函数 '{full}' 返回的表元素类型未知`
- `函数 '{full}' 未定义或不是返回表的函数`
- `函数 '{name}' 返回的表元素类型未知，无法确定 '{}' 的元素类型`
- `函数 '{name}' 未定义或不是返回表的函数`
- `键值表下标必须是字符串键，实际是 {}`
- `键值表值类型不匹配：map 值为 {}，表达式为 {}`
- `类 '{class_name}' 没有字段 '{}'`
- `类 '{class_name}' 没有字段 '{field}'`
- `类型字面量 {} 只能用作 switch 的 case 类型匹配`
- `裸调用 '{name}' 有歧义：多个 using 引入的命名空间都包含该函数，请改用命名空间前缀调用`
- `命名空间 '{}' 重复 using`
- `命名空间函数 '{full}' 期望 {} 个参数，实际 {} 个`
- `命名空间函数 '{full}' 未定义`
- `命名空间路径 '{}' 不能作为值使用（只能用于调用，如 '{}::xxx()'）`
- `命名空间前缀 '{}' 已被别名 '{}' 取代（import as 唯一入口），请改用别名访问`
- `内部错误：类 '{class_name}' 无信息`
- `内部错误：struct '{class_name}' 无信息`
- `内部错误：struct '{name}' 无定义`
- `全局变量 '{}' 必须是标量类型（i8..u64/f32/f64/bool/char/string），实际是 {}`
- `全局变量 '{}' 必须显式标注类型（如 var x: i64）`
- `全局变量 '{}' 初始化类型不匹配：期望 {}，实际 {}`
- `全局变量 '{}' 的初始化必须是字面量（数/布尔/字符/字符串）`
- `全局变量 '{}' 与函数名冲突`
- `全局变量 '{}' 重复定义`
- `三目两分支类型不一致：{} 与 {}`
- `算术运算符不能用于 {}`
- `未定义的函数 '{name}'`
- `未声明的变量 '{name}'`
- `位运算只支持整数，不能用于 {}`
- `下标必须是整数，实际是 {}`
- `下标访问的调用 '{}' 未解析（内部错误）`
- `下标访问的调用 '{full}' 不是返回表的函数`
- `下标访问的对象必须是表或字符串，实际是 {}`
- `下标赋值的对象必须是表，实际是 {}`
- `下标赋值类型不匹配：表元素类型为 {}，表达式为 {}`
- `元组没有字段 '{field}'（元组字段为 {}）`
- `元组字段名 '{n}' 重复`
- `重复的 case 区间 {key}`
- `重复的 case 值 {}`
- `字段 '{}' 必须标注类型或有默认值`
- `字段 '{}' 无类型标注，默认值必须是字面量（当前是表达式）`
- `字段 '{name}.{}' 与继承链中的字段重名（字段名必须跨继承链唯一）`
- `字段访问 '.' 的对象必须是元组或类实例，实际是 {}`
- `字段赋值的对象必须是 struct 实例，实际是 {}`
- `字段赋值类型不匹配：'{class_name}.{}' 类型为 {}，表达式为 {}`
- `arg_count() 期望 0 个参数，实际 {} 个`
- `arg_string() 参数必须是整数，实际是 {}`
- `arg_string() 期望 1 个参数，实际 {} 个`
- `bit_read() 第 1 个参数必须是字节表，实际是 {}`
- `bit_read() 第 2 个参数必须是整数，实际是 {}`
- `bit_read() 期望 2 个参数，实际 {} 个`
- `bit_write() 第 1 个参数必须是字节表，实际是 {}`
- `bit_write() 期望 3 个参数，实际 {} 个`
- `bit_write() 位置/位值必须是整数，实际是 {}`
- `byte_concat() 参数必须是字节表，实际是 {}`
- `byte_concat() 期望 2 个参数，实际 {} 个`
- `byte_read() 参数必须是字符串，实际是 {}`
- `byte_read() 期望 1 个参数，实际 {} 个`
- `byte_write() 第 1 个参数必须是字符串，实际是 {}`
- `byte_write() 第 2 个参数必须是字节表，实际是 {}`
- `byte_write() 期望 2 个参数，实际 {} 个`
- `case 类型匹配（{}）仅支持宽类型或动态容器对象，当前 switch 对象是静态类型 {}`
- `case 区间类型与 switch 对象类型 {} 不匹配`
- `case 值类型 {} 与 switch 对象类型 {} 不匹配`
- `char_code() 第 1 个参数必须是字符串，实际是 {}`
- `char_code() 期望 1 个参数，实际 {} 个`
- `cwd() 期望 0 个参数，实际 {} 个`
- `eval_call() 参数必须是字符串，实际是 {}`
- `eval_call() 期望 2 个参数，实际 {} 个`
- `eval() 参数必须是字符串，实际是 {}`
- `eval() 期望 1 个参数，实际 {} 个`
- `exec_code() 参数必须是字符串，实际是 {}`
- `exec_code() 期望 1 个参数，实际 {} 个`
- `exit() 参数必须是整数，实际是 {}`
- `exit() 期望 1 个参数，实际 {} 个`
- `file_delete() 参数必须是字符串，实际是 {}`
- `file_delete() 期望 1 个参数，实际 {} 个`
- `file_exists() 参数必须是字符串，实际是 {}`
- `file_exists() 期望 1 个参数，实际 {} 个`
- `file_read() 参数必须是字符串，实际是 {}`
- `file_read() 期望 1 个参数，实际 {} 个`
- `for 迭代对象仅支持范围（0..10）或表变量，实际是 {}`
- `len() 参数必须是字符串、表或键值表，实际是 {}`
- `len() 期望 1 个参数，实际 {} 个`
- `list_dir() 参数必须是字符串，实际是 {}`
- `list_dir() 期望 1 个参数，实际 {} 个`
- `msg_get_lang() 期望 0 个参数，实际 {} 个`
- `msg_register() 参数必须是字符串，实际是 {}`
- `msg_register() 期望 3 个参数，实际 {} 个`
- `msg_set_lang() 参数必须是字符串，实际是 {}`
- `msg_set_lang() 期望 1 个参数，实际 {} 个`
- `msg_t_lang() 参数必须是字符串，实际是 {}`
- `msg_t_lang() 期望 2 个参数，实际 {} 个`
- `msg_t() 参数必须是字符串，实际是 {}`
- `msg_t() 期望 1 个参数，实际 {} 个`
- `parse_float() 参数必须是字符串，实际是 {}`
- `parse_float() 期望 1 个参数，实际 {} 个`
- `parse_int() 参数必须是字符串，实际是 {}`
- `parse_int() 期望 1 个参数，实际 {} 个`
- `parse_trit() 参数必须是字符串，实际是 {}`
- `parse_trit() 期望 1 个参数，实际 {} 个`
- `path_join() 参数必须是字符串，实际是 {}`
- `path_join() 期望 2 个参数，实际 {} 个`
- `pow() 参数必须是数字（i64/f64），实际是 {}`
- `pow() 期望 2 个参数，实际 {} 个`
- `print 不支持元组参数（类型 {}）`
- `print_err() 参数必须是字符串，实际是 {}`
- `print_err() 期望 1 个参数，实际 {} 个`
- `println 不支持元组参数（类型 {}）`
- `rand_range() 参数必须是整数，实际是 {}`
- `rand_range() 期望 2 个参数，实际 {} 个`
- `read_line() 期望 0 个参数，实际 {} 个`
- `regex_group() 第 1 个参数必须是字符串，实际是 {}`
- `regex_group() 第 2 个参数必须是字符串，实际是 {}`
- `regex_group() 第 3 个参数必须是整数，实际是 {}`
- `regex_group() 期望 3 个参数，实际 {} 个`
- `regex_replace() 参数必须是字符串，实际是 {}`
- `regex_replace() 期望 3 个参数，实际 {} 个`
- `return 类型不匹配：函数返回 {}，实际返回 {}`
- `set_env() 参数必须是字符串，实际是 {}`
- `set_env() 期望 2 个参数，实际 {} 个`
- `str_char() 第 1 个参数必须是字符串，实际是 {}`
- `str_char() 第 2 个参数必须是整数，实际是 {}`
- `str_char() 期望 2 个参数，实际 {} 个`
- `str_len() 参数必须是字符串，实际是 {}`
- `str_len() 期望 1 个参数，实际 {} 个`
- `struct '{}' 重复定义`
- `struct '{struct_name}'（含继承链）没有方法 '{method}'：请在 \`
- `                             namespace {struct_name} 中定义 pub func {method}(首参: {struct_name}, ...)`
- `struct 继承形成环（含 '{name}'）`
- `struct 名 '{}' 与函数名冲突`
- `struct 实例 '{class_name}' 的字段访问需要可寻址对象（变量/字段链），`
- `switch 对象仅支持数字、布尔、字符或字符串类型，实际是 {}`
- `table_at() 第 1 个参数必须是表，实际是 {}`
- `table_at() 期望 2 个参数（表, 下标），实际 {} 个`
- `table_at() 下标必须是整数，实际是 {}`
- `table_push() 第 1 个参数必须是表，实际是 {}`
- `table_push() 期望 2 个参数（表, 元素），实际 {} 个`
- `table_push() 元素类型不匹配：表 '{}' 的元素是 {}，推入的是 {}`
- `table_push() 找不到表变量 '{}' 的元素类型`
- `table_push() 只能用于动态表（table_new_* 创建），'{}' 是定长表`
- `time_now() 期望 0 个参数，实际 {} 个`
- `to_string() 参数必须是数字（i64/f64），实际是 {}`
- `to_string() 期望 1 个参数，实际 {} 个`
- `trit 只能与 trit 或 i64 做算术，不能与 {}`
- `using 目标 '{}' 未导入：using 只能引用 import 引入的命名空间前缀或别名`
- `walk_dir() 参数必须是字符串，实际是 {}`
- `walk_dir() 期望 1 个参数，实际 {} 个`
- `when 守卫必须是布尔表达式，实际是 {}`
