# 规划（后续 M）：统一 func 写法

> 状态：**规划**（设计完成，待排期实现）
> 所属：tie 语言代码风格与语法一致性
> 依赖：Harbor M3 完成后排期（可与 switch 增强 / 单文件命名空间 同里程碑）

## 1. 背景与现状

tie 的函数语法（`func`）历经 v0.1 演进，当前存在**写法不一致**：

| 方面 | 现状问题 | 例子 |
| --- | --- | --- |
| 命名前缀 | 标准库函数带 `str_`/`tc` 前缀冗余 | `str.trim()` 在 `namespace str` 内仍写 `str_trim` |
| 返回值写法 | `-> (q: i64, r: i64)` 命名元组 vs 无括号 | 混用：有的 `-> string`，有的 `-> (a: i64, b: i64)` |
| 函数入口 | `func main()` 无返回类型，依赖特殊名 | 与其他函数规则不一致（无显式标注） |
| 调用写法 | 裸调用 / 点分全名 / 命名空间内裸调用三种并存 | `no_file()` vs `tcmsg.error.no_file()` |

具体现状（std/）：

- `std/string.tie`：`namespace str { func str_trim(s) -> string }` —— **前缀冗余**
  （命名空间 `str` 已隔离，`str_trim` 的 `str_` 是 M1 全局平面时代的遗留）；
- `std/math.tie`：`namespace math { func abs(x) -> i64 }` —— 无前缀，良好；
- `std/tcmsg.tie`：`namespace tcmsg { func error(key) }` —— 无前缀，良好。

## 2. 目标

统一函数定义与调用的书写规则，消除冗余前缀，让标准库成为风格模板：

| 规则 | 统一后写法 |
| --- | --- |
| 命名空间内函数**不再加前缀** | `namespace str { func trim(s) -> string }` |
| 返回类型统一为 `-> Ty`（无括号，除非多值） | `-> string` / `-> (a: i64, b: i64)` |
| 命名空间内裸调用即可 | 命名空间内直接 `trim(s)`，外部 `str.trim(s)` |
| 公开 API 无前缀，私有才需显式标注 | 结合"单文件命名空间"规划的 `func (ns) name` |

不做（明确排除）：一等函数、泛型函数、重载、方法调用语法（`obj.method()`，
留待类系统扩展）。

## 3. 设计

### 3.1 前缀去冗余（标准库层面，语法不变）

- `std/string.tie`：`str_trim` → `trim`、`str_slice` → `slice`、`str_contains` →
  `contains`、`str_find` → `find`、`str_starts_with` → `starts_with`、
  `str_ends_with` → `ends_with`、`str_replace` → `replace`、`str_split` →
  `split`；
- `str_trim`/`str_slice` 等旧名**保留为别名**（兼容期）或直接删除（v1 前无外部
  用户，倾向直接删除）；
- `prep/core.tie` 中 `slice`/`starts_with`/`split_lines` 等自写函数已是
  无前缀风格，无需改动；
- 调用处同步更新（std 内部互调 + docs 示例 + 测试）。

### 3.2 返回类型书写规范（文档约束 + 语义宽松）

- 单值返回：`-> Ty`（禁止 `-> (Ty)` 包裹）；
- 多值返回：`-> (name: Ty, name2: Ty2)`（命名元组，M2 已支持）；
- `void`：省略 `->` 或显式 `-> void`（语义层两者等价，文档推荐省略）；
- 校验：parser 对 `-> (i64)` 单值包裹报错（可选，低优先级）。

### 3.3 入口函数规范化（低优先级）

- `func main()` 保持现状（语义层特殊识别）；
- 文档注明：入口无返回类型、无参数；`return` 可选（隐式 void）。

### 3.4 调用写法统一（依赖"单文件命名空间"规划）

- **同命名空间内**：裸调用 `trim(s)`；
- **跨命名空间（已导入）**：点分 `str.trim(s)`；
- **不鼓励**：文件内跨命名空间全名调用写死前缀（用 import 引入）。

## 4. 实现步骤

1. 标准库重命名（`std/string.tie` 去前缀；`std/math.tie`、`std/tcmsg.tie` 检查）；
2. 调用处同步（std 互调、examples/、tests/、docs）；
3. 文档：language.md §2.4 函数 / §5 标准库命名规范、ai-guide、prompt-pack；
4. 可选：parser 单值元组返回校验；
5. 测试：std 函数重命名后 frontend/interp/llvm 全绿（回归）；
6. 示例：新增标准库调用示例（无前缀风格）。

## 5. 验收标准

- `std/string.tie` 内函数全部无 `str_` 前缀，`str.trim()` / `str.slice()` 调用
  正常工作（含 import 后裸调用）；
- 全仓库无 `str_trim` 等旧名残留（grep 零命中）；
- workspace 编译零错误、测试全绿（frontend/interp/llvm/lsp/prep 不变）；
- 文档中的函数示例全部为统一写法。

## 6. 影响范围

| 组件 | 影响 |
| --- | --- |
| std/string.tie | 函数重命名（8 个） |
| std/ 其他文件 + prep/core.tie | 无（已无前缀风格） |
| examples/ + tests/ | 调用名同步 |
| tie-frontend | 可选：parser 单值元组返回校验 |
| docs（language/ai-guide/prompt-pack） | 函数章节 + 标准库章节更新 |
| crates（LLVM/interp/lsp） | 无 IR/求值变化（纯改名 + 文档约束） |