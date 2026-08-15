# 规划（自举）：tie 语言完全自举——AST tag 编码 + 字符串分派 + 路线图

> ⚠️ **已取代（2026-08-10）**：本规划的路线图与实施方式已被
> `.omo/plans/self-hosting-v2.md`（自举 v2：100% tie / 0 Rust / LLVM 三层模块化）
> 取代。本文件保留价值：**§3 B1 tag 表 AST 编码规范**与 **§4 C1 字符串分派模式**
> 被 v2 计划原样复用为编译器内部编码（列式表 + 分派表），其余内容（阶段路线、
> 阶段 2 任务书——已归档至 `archive/compiler-v1/`）作废。
>
> 状态：**已被取代**（B1+C1 编码方案仍有效，作为 v2 内部编码规范）
> 所属：自举里程碑（Harbor 之后）——「用 tie 语言重写 tie 编译器自身」
> 前置：Harbor M2.1.8（struct 数据与逻辑分离）、M3（预处理器自举）、M6（包管理器自举）
> 已完成的铺垫：`prep/core.tie`、`repl/repl.tie`、`pkg/`、`std/` + `ext/` 全部用 tie 语言编写；
> 编译器核心（前端 + IR 生成）仍是 Rust，本规划解决「如何用 tie 重写它们」。

## 1. 背景与现状：自举盘点

### 1.1 已自举（tie 语言自写，可独立编译/解释执行）

| 部分 | 规模 | 说明 |
| --- | --- | --- |
| `prep/core.tie` | 约 276 行 | 预处理器核心（头部提取 / 角色判定 / 正文重建），Rust 壳仅解释执行（tie:script 协议） |
| `repl/repl.tie` | 30 行 | REPL 外壳（`print` + `read_line` + `eval`），编译链接 interp 静态库生成 `repl.exe` |
| `pkg/` | 7 个模块约 3000 行 | 包管理器全部逻辑（main/manifest/deps/fetch/lock/publish/search），Rust 只做子命令识别转发 |
| `std/` + `ext/`（含 `ext/codec/`） | 约 5500 行 | 标准库 11 模块 + 扩展库 9 文件（log/compress/ml/registry/cache/codec 编解码器） |

**已自举总量约 9000 行 tie 源码**，覆盖：预处理、CLI 工具、标准库、扩展库、编解码器。

### 1.2 未自举（仍是 Rust 实现，编译器本体）

| crate | 规模 | 角色 |
| --- | --- | --- |
| `tie-frontend` | 约 10100 行 Rust | 前端三阶段：词法（含 ASI）→ 语法 → 语义（符号表/类型检查）+ import 展开 |
| `tie-llvm` | 约 5400 行 Rust | 中端+后端驱动：AST → LLVM IR 文本生成；调用 opt/clang/lld |
| `tie-interp` | 约 4800 行 Rust | 解释执行：AST 树遍历求值 + C ABI 桥（staticlib） |
| `tie-lsp` | 3233 行 Rust | 语言服务器（复用前端三阶段 + import 展开） |
| `tie` | 1440 行 Rust | CLI 主入口：角色分派调度器 + REPL 启动 |

**编译器本体约 25000 行 Rust 仍未自举。**

### 1.3 完全自举的定义

> **完全自举 = 前端（词法/语法/语义）+ IR 生成用 tie 语言重写。**

即 `tie-frontend` 的词法、语法、语义分析与 `tie-llvm` 的 AST → LLVM IR 文本生成
逻辑全部由 `.tie` 源码实现，Rust 侧只保留：

- 语言底座原语（与 tie-interp/tie-llvm 共享的 C ABI 桥、文件/字符串/进程等系统能力）；
- 调用 LLVM 工具链的薄壳（`opt`/`clang`/`lld` 后端本身不在自举范围，它们对 tie 是外部工具，与对 C 相同）。

`tie-lsp` 与 `tie` 主入口可顺带自举（复用重写后的前端），但不作为自举的核心验收指标。

## 2. 语言能力障碍与决策记录（2026-08-09 定，2026-08-10 状态更新）

重写编译器前，先补齐 tie 语言自身缺失的能力。以下决策已选定方案：

| 编号 | 障碍 | 决策方案 | 状态 |
| --- | --- | --- | --- |
| A1/A6 | 表参数元素类型静态未知（`func f(t: table)` 无法知道元素是 i64 还是 string） | **A1 `table<T>` 类型参数** + **A6 实参先求值到临时动态表修拼接 UB** | ✅ 完成（E0 补定长表变量实参缺陷） |
| B1 | 无 enum（编译器 AST 需要标签联合） | **tag 表 AST**：节点 = `[0]=tag, [1..]=字段` 的表，判别用 `switch` | 📋 规划（前提已就绪：E1 嵌套表 + E3 键值表，编码按 §3.5 修订） |
| C1 | 无函数指针（编译器各阶段需要分派回调） | **字符串分派表**：`table<string>` 存函数名 + `switch` 展开；`eval_call` 兜底 | 📋 规划（前提已就绪：E3 map 可存分派表，字符串 switch 已支持） |
| D3 | 字符串 id 表不可用（`["a":1]` 语义未实现） | **排序数组 + 二分查找**（`std/sort.tie` 过渡）+ 后续 E3 完整字典 | ✅ 完成（E3 键值表 map 已落地——符号表可直接用 map，D3 过渡仍可用） |
| E1/E5 | 无 `continue`/`break`（重写循环逻辑不可用） | **E1 `break`/`continue` 语句** + **E5 标签跳转**（`break L`/`continue L`） | ✅ 完成 |
| F1 | LLVM alloca 栈溢出（深层递归生成 IR 时 alloca 堆积在非 entry block） | **alloca 提升到 entry block** | ✅ 完成 |
| C5 | switch 整数 case 生成低效分支 | **switch 整数 case 生成 LLVM `switch` 指令** | ✅ 完成 |
| E0 | 定长表变量实参 IR 缺陷（`[N x T]` 数组直接传 ptr 形参） | **定长表变量实参展开为动态表**（与 A6 字面量同路径） | ✅ 完成（2026-08-10） |
| E1' | 无嵌套表（AST 树形结构无法表表达） | **嵌套表 `table<table<T>>`**：元素类型可递归为表，`>>` 闭括号分裂 | ✅ 完成（2026-08-10） |
| E3 | 无键值表（`["a":1]` 语义未实现） | **键值表 `map`/`map<T>`**：字面量/下标读写/实参，16 字节元素动态表 | ✅ 完成（2026-08-10） |

### 2.1 A1/A6 表参数元素类型静态未知

**障碍**：当前 `table` 是编译期概念，语义层展开为具体元素类型。函数形参写
`func f(t: table)` 时元素类型静态未知，IR 层无法确定 LLVM 数组元素类型；
且表字面量实参直接按值传递时，若与形参布局不一致会产生拼接 UB。

**方案 A1 `table<T>` 类型参数**：形参写作 `func f(t: table<i64>)`，元素类型参与
签名与类型匹配；泛型表在语义层实例化（先支持单元素类型，不引入完整泛型系统）。

**方案 A6 实参先求值到临时动态表**：函数调用时表实参不求值到字面量布局，
而是先装入运行时动态表（DynTable），按元素类型校验后再传给形参——修复
类型不一致导致的拼接 UB，也让 interp/IR 两路径行为一致。

### 2.2 B1 tag 表 AST（无 enum 的标签联合）

**障碍**：编译器 AST 本质是递归标签联合（`enum Expr { IntLit(i64), Binary{..}, ... }`）。
tie 无 enum、无递归数据结构类型的原生表达。

**方案**：用「tag 表」编码 AST 节点——每个节点是一张表：

- `[0]` = tag（整数，唯一标识节点种类）；
- `[1..]` = 字段，按固定顺序排列（子节点嵌套为表，表套表形成树）。

判别用 `switch node[0]`（C5 优化为 LLVM switch 指令）。完整规范见 §3。

### 2.3 C1 字符串分派表（无函数指针）

**障碍**：编译器各阶段（词法状态机、运算符优先级、IR 生成遍历）需要按名字分派
到不同处理函数，是典型的函数指针/回调场景。tie 无函数类型、无一等函数。

**方案**：分派目标用「字符串函数名」表示，运行时维护 `table<string>` 分派表，
调用点用 `switch 函数名` 静态展开（编译器全量展开，无运行期查表开销）；
无法静态枚举的入口（如 tie:script 跨层回调）用 `eval_call` 兜底。完整说明见 §4。

### 2.4 D3 排序数组 + 二分查找（字符串 id 表不可用）

**障碍**：编译器需要大量「名字 → 信息」映射（符号表、tag 表、关键字表、保留字表），
天然对应字符串 id 表（`["add":1, "sub":2]`），但该特性语义未实现。

**方案**：先用「**排序数组 + 二分查找**」过渡——建表时把（键, 值）对按键排序，
查询用二分查找（`std/sort.tie` 提供排序，`std/optsearch.tie` 已有 quick_sort）。
正确、确定性强，代价是 O(log n) 查询与 O(n log n) 建表；后续能力成熟后
平滑替换为 D1 完整字典（哈希表），调用点接口不变。

### 2.5 E1/E5 continue/break 与标签跳转

**障碍**：编译器重写中需要「提前退出循环/提前开始下一轮」的控制流
（例如 IR 生成时遍历到错误节点提前 return，或词法状态机提前跳出）。
tie 无 `break`/`continue`，只能用标志位 + 条件判断模拟，可读性差。

**方案**：
- **E1**：新增 `break` / `continue` 语句（作用于最近一层循环）；
- **E5**：扩展为标签跳转 `break L` / `continue L`（`L: while ... { break L }`），
  用于跳出嵌套循环（IR 生成的嵌套遍历场景）。

E1 已进入 AST（`Stmt::Break` / `Stmt::Continue`）；E5 的标签解析与 IR 生成待实现。

### 2.6 F1 LLVM alloca 提升到 entry block

**障碍**：tie-llvm 的 IR 生成在循环/分支内为变量分配 alloca 时，alloca 指令落在
非 entry block。`opt -O2` 的 mem2reg 无法提升它们，导致递归/深层嵌套时
栈帧在循环中不断增长，最终栈溢出。

**方案**：IR 生成器维护「当前函数 entry block」，所有 alloca 统一发射到 entry block
末尾（需要时用独立计数器命名避免冲突），运行期变量才用 store/load。
语义等价，栈占用恒定为函数最大活变量数。

### 2.7 C5 switch 整数 case → LLVM switch 指令（附增）

**附**：B1 的 AST 判别是 `switch node[0]`，tag 是连续整数。当前 tie-llvm 把 switch
生成链式 icmp/br 分支；为判别高效，整数 case 的 switch 生成 LLVM `switch` 指令
（跳转表），与 C1 的字符串 switch 展开互补（字符串仍走逐串比较 + 哈希短路）。

## 3. B1 AST tag 编码规范

### 3.1 编码约定

**节点 = 一张表，`[0]` 固定为 tag（i64），`[1..]` 按字段顺序排列。**

- 字面量/标量字段直接存值：`[0, 42]` 是 `IntLit(42)`；
- 子节点字段存「子节点表」：`[5, "add", [0, 1], [0, 2]]` 是 `Call("add", [IntLit 1, IntLit 2])`；
- 表（table）类型字段存嵌套表：`[12, [ [0,1],[0,2],[0,3] ]]` 是 `TableLit([IntLit 1..3])`；
- 可选字段用约定值占位：`null`（i64 最小值或 -1）表示无类型标注 / 无 span；
- span（行列）不进 tag 表主体——调试信息单独存「span 表」按节点 id 索引，避免污染数据。

**解码（还原字段）**：`var tag = node[0]` → `switch tag { ... }`，每个分支按该
节点的字段序读取 `node[1]`、`node[2]`……（顺序即编码顺序，一一对应）。

### 3.2 tag 编号表（草案）

编号分段：**0–99 表达式、100–199 语句、200–299 类型、300+ 保留**。
新增节点种类按序号递增追加，**已分配的编号永不复用**（删除只留档不重排，
避免历史 AST 失效）。

#### 表达式（tag 0–99）

| tag | 节点 | 字段（`[1..]`） |
| --- | --- | --- |
| 0 | IntLit | 值 `[v: i64]` |
| 1 | FloatLit | 值 `[v: f64]` |
| 2 | StrLit | 值 `[s: string]` |
| 3 | BoolLit | 值 `[b: bool]` |
| 4 | Var | 名字 `[name: string]` |
| 5 | Call | 名 `[name: string]`、实参表 `[args: table]` |
| 6 | Binary | 运算符 `[op: i64(运算符编号)]`、左 `[lhs: 节点]`、右 `[rhs: 节点]` |
| 7 | Unary | 运算符 `[op: i64]`、操作数 `[operand: 节点]` |
| 8 | Index | 基址 `[base: 节点]`、下标 `[index: 节点]` |
| 9 | FieldAccess | 基址 `[base: 节点]`、字段名 `[field: string]` |
| 10 | MethodCall | 接收者 `[receiver: 节点]`、方法名 `[method: string]`、实参表 `[args: table]` |
| 11 | TupleLit | 字段表 `[fields: table]`（每个字段 = `[name_or_null, 节点]`） |
| 12 | TableLit | 单元格表 `[cells: table]`（每个 cell = `[id_or_null, value_节点, row]`） |
| 13 | Range | 起点 `[start: 节点]`、终点 `[end: 节点]` |
| 14 | Ternary | 条件 `[cond: 节点]`、真支 `[then_expr: 节点]`、假支 `[else_expr: 节点]` |
| 15 | CharLit | 值 `[c: i32(Unicode 标量)]` |
| 16 | TritLit | 值 `[v: i64(-1/0/1)]` |
| 17 | Path | 段表 `[segments: table]`（命名空间路径 `a::b::c`） |
| 18 | TypeLit | 类型 `[ty: 类型 tag 表]`（switch 类型匹配 pattern 用） |
| 19–99 | 预留 | 后续新增表达式节点 |

#### 语句（tag 100–199）

| tag | 节点 | 字段（`[1..]`） |
| --- | --- | --- |
| 100 | VarDecl | 名 `[name: string]`、类型 `[ty: 类型 tag 表 或 null]`、初值 `[init: 节点]`、不可变 `[is_const: bool]` |
| 101 | Assign | 目标 `[target: string]`、运算符 `[op: i64 或 null]`、值 `[value: 节点]` |
| 102 | Return | 值 `[value: 节点 或 null]` |
| 103 | If | 条件 `[cond: 节点]`、真支语句表 `[then: table]`、假支语句表 `[else: table 或 null]` |
| 104 | While | 条件 `[cond: 节点]`、体语句表 `[body: table]` |
| 105 | For | 迭代变量 `[name: string]`、迭代对象 `[iter: 节点]`、体语句表 `[body: table]` |
| 106 | Switch | 主体 `[subject: 节点]`、case 表 `[cases: table]`（每个 case = `[patterns 表, body 表]`） |
| 107 | Expr | 表达式 `[expr: 节点]`（表达式语句） |
| 108 | IndexAssign | 基址 `[base: 节点]`、下标 `[index: 节点]`、运算符 `[op: i64 或 null]`、值 `[value: 节点]` |
| 109 | FieldAssign | 基址 `[base: 节点]`、字段名 `[field: string]`、值 `[value: 节点]` |
| 110 | FnDef | 名 `[name: string]`、参数表 `[params: table]`（每个 = `[name, 类型 tag 表]`）、返回类型 `[ret_ty: 类型 tag 表]`、公有 `[is_pub: bool]`、体 `[body: table]` |
| 111 | Break | 标签 `[label: string 或 null]`（E1/E5） |
| 112 | Continue | 标签 `[label: string 或 null]`（E1/E5） |
| 113 | Import | 路径 `[path: string]`、别名 `[alias: string 或 null]` |
| 114 | Namespace | 路径段表 `[path: table]`、体 `[body: table]` |
| 115 | Using | 目标路径段表 `[path: table]` |
| 116 | Struct | 名 `[name: string]`、父 `[parent: string 或 null]`、字段表 `[fields: table]`（每个 = `[name, 类型 tag 表, 默认值 节点 或 null]`） |
| 117–199 | 预留 | 后续新增语句节点 |

#### 类型（tag 200–299）

| tag | 类型 | 备注 |
| --- | --- | --- |
| 200 | i64 | |
| 201 | f64 | |
| 202 | f32 | |
| 203 | i32 | |
| 204 | i16 | |
| 205 | i8 | |
| 206 | u64 | |
| 207 | u32 | |
| 208 | u16 | |
| 209 | u8 | |
| 210 | bool | |
| 211 | char | |
| 212 | string | |
| 213 | void | |
| 214 | trit | 平衡三进制 |
| 215 | num | 宽类型 |
| 216 | text | 宽类型 |
| 217 | misc | 宽类型 |
| 218 | code | 编译期概念 |
| 219 | table | 元素类型未知的裸 table |
| 220 | table\<T\> | A1 泛型表：字段 `[elem_ty: 类型 tag 表]` |
| 221 | 元组类型 | 字段 `[fields: table]`（每个 = `[name_or_null, 类型 tag 表]`） |
| 222 | struct 类型 | 字段 `[struct_name: string]` |
| 223–299 | 预留 | 后续新增类型 |

#### 运算符（独立小表，供 Binary/Unary 引用）

| 编号 | 运算符 | 编号 | 运算符 |
| --- | --- | --- | --- |
| 0 | `+` Add | 10 | `&&` And |
| 1 | `-` Sub | 11 | `\|\|` Or |
| 2 | `*` Mul | 12 | `&` BitAnd |
| 3 | `/` Div | 13 | `\|` BitOr |
| 4 | `%` Mod | 14 | `^` BitXor |
| 5 | `==` Eq | 15 | `<<` Shl |
| 6 | `!=` NotEq | 16 | `>>` Shr |
| 7 | `<` Lt | 17 | `-`(一元) Neg |
| 8 | `>` Gt | 18 | `!`(一元) Not |
| 9 | `<=` Le | 19–24 | 前/后缀 `++` `--`（PreInc/PreDec/PostInc/PostDec，M4） |

### 3.3 判别写法示例（可编译风格）

```tie
// tie:logic
// 解码一个 AST 表达式节点并求其"形状描述"（演示 B1 tag 判别写法）

import "./std/string.tie" as str
using str;

func shape(node: table<i64>) -> string {
    var tag = node[0]
    switch tag {
        case 0:       // IntLit
            return "IntLit(" + str.from_i64(node[1]) + ")"
        case 1:       // FloatLit
            return "FloatLit"
        case 2:       // StrLit
            return "StrLit(" + node[1] + ")"
        case 4:       // Var
            return "Var(" + node[1] + ")"
        case 5:       // Call
            return "Call(" + node[1] + ")"
        case 6:       // Binary
            return "Binary(op=" + str.from_i64(node[1]) + ")"
        case 14:      // Ternary
            return "Ternary"
        default:
            return "Unknown(" + str.from_i64(tag) + ")"
    }
}

func main() {
    // IntLit(42)：tag=0，值=42
    var int_node = [0, 42]
    println(shape(int_node))                  // IntLit(42)

    // Call("add", [1, 2])：tag=5，名=add，实参=子节点表
    var call_node = [5, "add", [[0, 1], [0, 2]]]
    println(shape(call_node))                 // Call(add)
}
```

> 代码块中 `str.from_i64` / `node[1]` 为示意；实际以 std/ 最终接口为准。
> 判别核心是 **`switch node[0]`** 一行进入对应分支，字段按 §3.2 表序读取。

### 3.4 编码/解码约定细则

1. **tag 恒在第 0 元素**：任何节点 `node[0]` 必为 tag，解判别零歧义；
2. **字段按表序**：同一 tag 的字段位置固定，编解码一一对应（规范见 §3.2）；
3. **子节点即嵌套表**：树形结构用「表套表」表达，无指针、无引用；
4. **可空字段用 `null`**：统一用 -1 或空串约定值，避免类型混乱；
5. **不可变**：编码后的 AST 表在编译期只读，修改节点 = 重建节点表（纯函数风格，
   利于 interp/IR 两路径共享同一棵 AST）；
6. **符号表/元数据分离**：tag 表只承载结构；类型推导结果、span、绑定信息
   存在旁路表（按节点 id 索引），不污染 AST 本体。

### 3.5 实测修订（2026-08-10）：混合表与表套表的语言边界

原 §3.1 假设「节点 = 一张表，字段可混合类型、子节点即嵌套表」。实测（用
tie-llvm 编译验证）发现两条**语言硬约束**，编码方案据此修订：

| 假设 | 实测结果 | 结论 |
| --- | --- | --- |
| 混合元素表字面量 `[5, "add", [...]]`（i64+string+表） | 语义层拒绝：「表是元素同构的容器」（即使标注 `: table`） | 节点不能直接混合字段类型 |
| 表套表 `[[0,1],[0,2]]` 后 `node[0][0]` 二级下标 | 元素类型被拍平为内层标量（i64），二级下标报错 | 嵌套表下标链当时不可用 |

**修订编码（已就绪的语言能力支撑，示例见 examples/table_enhance_demo.tie）**：

1. **节点池 = 列式并行表（arena）**：所有节点扁平存储，**节点 id = 表下标**。
   每字段一张同构表——天然满足「表元素同构」硬约束，且下标访问 O(1)：

   ```tie
   var node_tags: table<i64>             // 节点 id → tag（判别键）
   var node_names: table<i64>            // 节点 id → 字符串池 id（-1 = 无）
   var node_vals: table<i64>             // 节点 id → 整数值字段（字面量值/运算符编号）
   var node_children: table<table<i64>>  // 节点 id → 子节点 id 列表（**嵌套表**）
   ```

   - 判别：`switch node_tags[id]`（C5 跳转表）；
   - 子节点：`var child = node_children[id][i]` → `node_tags[child]` 递归（E1 下标链）；
   - 字符串字段：存「字符串池 id」（`map` 或 `table<string>` 池，E3）；
   - 增删节点 = push 各列（下标对齐），节点 id 即下标，天然稳定。

2. **嵌套表 `table<table<i64>>` 的落点**：子节点 id 列表（`node_children`）——
   元素全是 i64 表的嵌套表，编译/解释双路径实测可用（`node_children[id][i]` 链）；
   不再作为「节点内部字段」（节点字段全 i64 标量）。

3. **键值表 map（E3）直接可用**：符号表/关键字表/字符串池用 `map`（`m["add"]`
   下标读写、`len(m)`、作实参传递全支持）——D3 排序数组过渡方案可平滑升级。

4. **tag 编号表（§3.2）不变**：tag 是 i64 恒存 `node_tags[id]`，判别 `switch`
   走 C5 跳转表。

**修订后的节点访问（权威示例，与 examples/table_enhance_demo.tie 一致）**：

```tie
// Call("add", [IntLit(1), IntLit(2)]) 的列式编码：
//   node_tags  = [5, 0, 0, 0]        // 0:Call  1..3:IntLit
//   node_names = [0, -1, -1, -1]     // Call 的字符串池 id = 0（"add"）
//   node_vals  = [-1, 42, 1, 2]      // IntLit 值
//   node_children = [[1, 2], [], [], []]  // Call 的子节点 id 表（嵌套表）
var call_id = 0
var tag = node_tags[call_id]              // 5 → 判别
var name = pool_names[node_names[call_id]] // "add"（池还原）
var first_child_val = node_vals[node_children[call_id][0]]  // 1（E1 嵌套链）
```

> 结论：**B1 全部语言前提已就绪**——嵌套表（E1）、键值表（E3）、`>>` 类型参数
> 分裂、递归函数、break/continue、alloca 栈安全。阶段 2（lexer/parser/semantic/IR
> 用 tie 重写）可直接开工。

## 4. C1 字符串分派模式

### 4.1 为什么需要分派

词法分析的状态机、语义检查的表达式/语句分派、IR 生成的节点遍历，都需要
「按一个名字/类别把工作交给对应函数」。没有函数指针时，用字符串函数名
作为分派键。

### 4.2 分派表构造

```tie
// tie:logic
// C1 字符串分派表：把"阶段处理函数名"登记进分派表
// （登记的函数必须是同签名的纯函数，便于 switch 统一调用）

import "./std/string.tie" as str
using str;

// 处理函数族：全部 (node: table<i64>) -> string 签名
pub func handle_lexer(node: table<i64>) -> string { return "lexer" }
pub func handle_parser(node: table<i64>) -> string { return "parser" }
pub func handle_semantic(node: table<i64>) -> string { return "semantic" }

// 分派表：字符串函数名表（D3：按键排序后可二分查找）
func build_dispatch_table() -> table<string> {
    var names: table<string> = ["handle_lexer", "handle_parser", "handle_semantic"]
    return names
}

func dispatch(name: string, node: table<i64>) -> string {
    switch name {
        case "handle_lexer":
            return handle_lexer(node)
        case "handle_parser":
            return handle_parser(node)
        case "handle_semantic":
            return handle_semantic(node)
        default:
            return "unknown handler: " + name
    }
}

func main() {
    var table = build_dispatch_table()
    var node: table<i64> = [0, 42]
    for i in 0..3 {                     // 遍历分派表逐个调用
        var fname = table[i]
        println(dispatch(fname, node))  // lexer / parser / semantic
    }
}
```

### 4.3 与 eval_call 兜底的对比

| 维度 | C1 字符串分派表（主） | eval_call 兜底（备） |
| --- | --- | --- |
| 调用方式 | `switch 函数名` 静态展开，编译期确定分支 | 字符串名字运行时经 tie-interp 会话查函数表 |
| 性能 | 无运行期查找，switch 可优化为跳转表（C5） | 每次调用走解释器查表 + 参数装箱，慢几个数量级 |
| 类型安全 | 参数类型在调用点由编译器校验 | 只能传字符串值（协议约束），无静态类型 |
| 适用场景 | 编译器内部固定分派（词法/语义/IR 遍历） | 跨层回调、tie:script 协议（`process(src)->string`）、运行期扩展插件 |
| 编译依赖 | 需函数先定义，编译器全量枚举 | 函数存在即可，无需编译期可见 |

**用法原则**：编译器本体的分派全部用 C1（静态、可编译、快）；只有真正需要
「运行期按名调用外部模块函数」的边界（如 `--module` 挂载转换器、跨进程回调）
才用 eval_call。

### 4.4 局限（明确记录）

1. **无类型安全**：分派键是字符串，拼错函数名编译期不报错，运行期落入 default
   （或 eval_call 的「未定义函数」错误）。对策：分派键用常量表集中管理 +
   建表时校验名字是否在已知集合内；
2. **无函数值传递**：不能把「一个函数」作为参数传给另一个函数（算法库已用
   采样表/回调名规避）；
3. **switch 展开体积**：分派函数多时 switch 分支线性增长。对策：按阶段拆多个
   分派函数（lexer_dispatch / semantic_dispatch / ir_dispatch），每函数分支数可控；
4. **字符串比较成本**：switch 字符串分支逐串比较。对策：C5 对整数 tag 用 LLVM
   switch；字符串分支用首字符/长度做哈希短路；
5. **重构不友好**：函数改名需同步改分派表。对策：分派表与函数定义放同文件，
   加注释登记。

## 5. 自举阶段路线图

### 阶段 0：语言能力补齐——控制流 + alloca（E1/E5 + F1）

> 目标：让 tie 语言能表达编译器重写所需的全部控制流与深层递归栈安全。

**内容**：

- **E1**：`break` / `continue`（AST 已含 `Stmt::Break` / `Stmt::Continue`）——补全
  parser / semantic / interp / IR 四层，作用于最近一层循环；`break` 无标签时
  语义等价跳出 while/for；
- **E5**：标签跳转 `label: while ...` + `break L` / `continue L`——解析标签、
  语义层做「标签 → 最近匹配循环」绑定与重复标签/未定义标签检查，IR 生成
  br 到循环出口/continue 块；
- **F1**：alloca 提升——`IrGenerator` 维护 entry block 引用，`alloca` 指令统一
  发射到 entry block 末尾；命名用独立计数器，杜绝与运行期块冲突；
- 为阶段 2 的重写预留：`std/sort.tie`（D3 依赖）与 `std/str_binsearch.tie`
  二分查找（键比较表 `table<string>` + `table<i64>` 并行）。

**验收标准**：

- `break`/`continue`/`break L`/`continue L` 在 while/for/嵌套循环中行为正确，
  有负例测试（无循环中 break → 编译期报错）；
- 深层递归（如 10000 层）编译不再栈溢出，IR 中 alloca 全部位于 entry block
  （`--emit-ir` 抽查）；
- workspace 编译零错误、测试全绿。

### 阶段 1：语言能力补齐——表类型参数 + 有序映射（A1/A6 + D3）

> 目标：让 tie 语言能表达编译器重写所需的全部数据结构。

**内容**：

- **A1 `table<T>`**：parser 解析类型参数；semantic 做单元素类型签名匹配；
  interp/IR 生成按元素类型定数组布局（`table<i64>` / `table<string>` 先落地）；
- **A6**：函数调用时表实参先求值到临时动态表（DynTable），按元素类型校验后
  绑定形参；修复拼接 UB 并统一 interp/IR 两路径；
- **D3**：`std/sort.tie`（插入排序/归并排序，供小表与大表）、
  `std/str_binsearch.tie`（并行键值表二分查找，返回 index 或 -1）；
  编译器重写全部「名字 → 信息」映射先用它，接口按 `lookup(table, key) -> i64`
  统一约定，为后续 D1 字典替换留出同接口。

**验收标准**：

- `table<i64>` / `table<string>` 形参实参在 interp 与 IR 两路径行为一致；
- 表实参类型不匹配（`table<i64>` 传入 string 元素）→ 编译期报错；
- D3 工具库有 demo 与测试（键表含重复/缺失查询），二分查找边界正确；
- workspace 编译零错误、测试全绿。

### 阶段 2：核心重写——前端 + IR 生成用 tie（B1/C1 兜底）

> 目标：**完全自举达成**——`tie-frontend` 与 `tie-llvm` 的核心逻辑用 tie 重写，
> 产出编译器源码 `compiler/`（tie 语言写）。

**内容**：

- **词法**（`compiler/lexer.tie`）：源文本 → token 表。字符分类/关键字表用
  D3 有序表 + 二分；状态机用 C1 字符串分派；
- **语法**（`compiler/parser.tie`）：递归下降，产出 B1 tag 表 AST（§3）；
  ASI 在 token 流层面补分号（沿用 Rust 版规则）；
- **语义**（`compiler/semantic.tie`）：3 遍分析（收集函数签名 → collect_structs
  拍平继承 → check_fn），符号表用 D3 有序表；错误收集为错误表（结构化，供 LSP）；
- **IR 生成**（`compiler/ir.tie`）：tag 表 AST → LLVM IR 文本；F1 保证 alloca
  在 entry block；整数 switch 走 C5 LLVM switch 指令；
- **入口整合**：`compiler/main.tie` 走 tie:script 协议暴露
  `compile(src: string) -> string`（输入源码 → 输出 .ll 文本），Rust 壳
  `tie-llvm` 只负责：读文件 → 调 `compile` → 调 `opt`/`clang`/`lld`；
- **自举闭环**：`tie-llvm` 用「tie 写的 compile」编译「tie 写的 compile 自身」，
  第二次输出与第一次逐字节一致（经典自举校验）。

**验收标准**：

- 编译器源码 100% 是 `.tie` 文件（`compiler/` 目录），Rust 侧仅薄壳 +
  后端工具调用；
- 用 tie 编译器编译 `examples/`、`std/`、`pkg/` 全部现有代码，行为与 Rust 版
  完全一致（输出 diff 为空）；
- **自举两轮输出一致**（重新编译自身，产物字节相同）；
- 词法/语法/语义错误信息与 Rust 版对齐（信息文本/行列号）；
- workspace 编译零错误。

### 阶段 3：语言反哺——enum + 函数类型（能力回流编译器）

> 目标：把自举过程中暴露的「用 tag 表硬编码」反哺为语言一等特性，编译器
> 代码可读性提升，并回注给 Rust 版语义/IR 保持双实现一致。

**内容**：

- **enum**：`enum Node { IntLit(i64), Binary{..}, ... }` 编译期 desugar 为
  tag 表（B1 编码天然就是 desugar 目标）——语义层检查穷尽性，IR 层生成
  switch 判别；**✅ enum 已实现（2026-08-15，tiec 全链路：无数据/带数据/
  泛型变体、构造/匹配、静态结构体 `{i64 tag, i64×K 槽}` 布局、单态化）**；
- **函数类型**：`func (node: table<i64>) -> string` 类型，函数值可存入表、
  传给其他函数——C1 字符串分派自动升级为真函数值（分派表从 `table<string>`
  变为函数值表），编译期类型检查补上 C1 的「无类型安全」短板；📋 仍规划；
- 编译器 `compiler/` 逐步迁移到 enum + 函数值，tag 表编码保留为序列化格式
  （跨进程/持久化用）。

**验收标准**：

- enum 与函数类型在语义/IR 两路径可用，有 demo 与测试；
  **enum 部分 ✅（`tests/language/enum.tie` / `enum_neg.tie` / golden err_063-065）**；
- 用新特性重写 `compiler/` 至少一个模块（如 lexer 状态机），行为不变；📋
- 双实现（tie 编译器 vs Rust 编译器）对同一批样例输出一致；📋
- workspace 编译零错误；✅

## 6. 影响范围

| 组件 | 影响 |
| --- | --- |
| crates/tie-frontend | A1 table\<T\> 类型解析、E1/E5 语句解析与语义（阶段 0/1）；阶段 2 起逻辑迁移到 tie，Rust 侧仅保留底座 |
| crates/tie-llvm | F1 alloca 提升、C5 LLVM switch 生成（阶段 0）；阶段 2 起 IR 生成迁移到 tie，Rust 侧调 tie 编译出的 compile |
| crates/tie-interp | A6 表实参动态表、E1/E5 解释执行、A1 类型化表求值 |
| std/ | 新增 `sort.tie`、`str_binsearch.tie`（D3）；供自举编译器复用 |
| compiler/（新，tie 源码） | 自举编译器本体：lexer/parser/semantic/ir/main |
| docs/ | README 路线图、本文件、CHANGELOG |
| scripts/package.ps1 | 发行版收录 compiler/ 与自举校验脚本 |

## 7. 风险与对策

| 风险 | 对策 |
| --- | --- |
| tag 表 AST 无类型约束，字段顺序错误难查 | §3.2 表是唯一权威；编写「编码器/解码器」成对测试，随机 AST 编解码 roundtrip 校验 |
| 字符串分派无类型安全 | 分派键集中常量表 + 建表时白名单校验；阶段 3 迁移到真函数类型 |
| 排序数组查询慢 | 自举期规模小（编译器符号表几百项）可接受；D1 字典就绪后同接口替换 |
| tie 递归深度/性能不足 | F1 栈安全 + LLVM -O2 优化；必要时拆循环替代深递归 |
| 双实现语义漂移（tie vs Rust） | 共享 tag 编号表与验收样例集；每阶段跑「两实现输出 diff」回归 |
| 自举校验不通过 | 阶段 2 验收把「两轮输出一致」作为硬门槛，逐阶段分片推进而不是一次性替换 |

## 8. 相关文件

| 文件 | 作用 |
| --- | --- |
| crates/tie-frontend/src/ast.rs | Rust AST 枚举现状（B1 tag 表的对应基准） |
| crates/tie-llvm/src/ir.rs | 现 IR 生成（F1/C5 改动点） |
| docs/tie-script.md | eval/eval_call 协议（C1 兜底机制） |
| docs/plans/package-manager.md | 前序自举里程碑（tie 写完整工具） |
| docs/plans/algorithm-library.md | D3 排序/查找工具的算法依据 |
| README.md / CHANGELOG.md | 路线图与变更记录同步更新 |

