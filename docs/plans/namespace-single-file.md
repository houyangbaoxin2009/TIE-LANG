# 规划（后续 M）：单文件命名空间

> 状态：**已实现**（Harbor M2.1.7，2026-08-09）
> 所属：tie 语言模块化体系（标准库文档化命名）
> 落地：pub 可见性 + using 引入 + import 别名唯一入口（实现详见 CHANGELOG [Harbor M2.1.7]）
> 注：原规划的可见性语法 `func(ns) name` 在实现中改为 **`pub func name`**（更直观的
> 显式导出标记；命名空间内函数默认私有）；`import ... as` 升级为别名**唯一入口**；
> 新增 `using` 引入语句支持裸调用。以下正文保留设计背景，语法以 M2.1.7 实际实现为准。

## 1. 背景与现状

tie 已有命名空间机制（M2 实现）：

- 语法：`namespace path { … }`，path 为点分路径（`math.gcd`）；
- 嵌套：`namespace out { namespace inner { … } }`；
- 调用：点分全名（`tcmsg.error.no_file`）与**裸调用补全**（M3 增强：
  语义层 `ns_call_full_name` 按 `ns_stack` 补全前缀——在 `namespace tcmsg.error`
  内可直接写 `no_file()`）；
- 标准库已拆分到 `std` 目录并在语义层软注册命名（std/gcd.tie 等）。

现有局限：

1. **单文件 = 单"作用域工作区"**：命名空间仅供组织函数名，函数仍全局平面注册，
   无独立的"命名空间作用域"（命名空间内变量/常量不隔离）；
2. **无跨文件命名空间复用**：`import` 拉平所以函数到当前文件，丢失了标准库的
   `tc.*` 名字层级——调用方必须写完整路径或靠补全，无法 `import tc.math` 后
   直接 `gcd()`；
3. **无命名空间级可见性控制**：无法表达"这个函数仅命名空间内可见"（私有）。

## 2. 目标

让命名空间成为真正的**模块边界**：

| 能力 | 现状 | 目标 |
| --- | --- | --- |
| 命名空间内变量/函数作用域 | 平面全局 | 命名空间内**局部函数可见性**（近似私有） |
| 跨文件复用 | 仅软解析 | `import ns.path` 可引入命名空间并保留路径前缀 |
| 裸调用补全 | 仅同文件 ns_stack | 跨文件 + 重命名前缀 |
| 冲突隔离 | 无（同名即冲突） | 不同命名空间同名函数**不冲突**（经路径区分） |

不做（明确排除）：模块系统（`mod`/导出表）、循环导入、动态加载。

## 3. 设计

### 3.1 语法（保持兼容；M2.1.7 实际实现）

```c
// 文件 A：tools.tie
namespace fmt {
    func pad(n: i64) -> i64 { return n + 1 }      // 私有：默认，仅 fmt 内可见
    pub func public_api(n: i64) -> i64 { return pad(n) }  // 显式导出（pub func）
}

// 文件 B：main.tie
import "tools.tie" as fmt2          // 导入并重命名前缀（别名唯一入口：fmt 被屏蔽）
using fmt2;                         // 引入命名空间，公有函数可裸调用
func main() {
    println(fmt2.public_api(1))    // 别名点分调用
    println(public_api(1))         // using 引入后的裸调用
    // println(pad(1))             // 错误：pad 是私有函数，导入方不可见
}
```

设计要点：

- **可见性标记**：`pub func` 显式导出；无标记 = 私有（仅同命名空间可见）。
  顶层函数恒公有，与现状兼容；
- **import 带前缀重命名**：`import "tools.tie" as fmt2`——目标文件的命名空间在导入方
  以 `fmt2` 为**唯一入口**（原前缀被屏蔽，避免同名命名空间跨文件冲突）；
- **using 引入**：`using fmt2;` / `using fmt2.inner;` 把该命名空间的公有函数引入当前
  文件，之后可裸名调用；
- **裸调用解析升级**：顶层裸名 → 当前文件 ns_stack 补全 → using 引入命名空间（唯一候选，
  多候选报歧义）；
- **冲突规则**：不同命名空间的同名函数允许共存（语义表按 `ns::name` 全名注册）。

### 3.2 与 std/ 的关系

- std/ 目录已是"一文件一命名空间"布局，天然适配；
- 文档库命名（M2 已软解析）在新模式下变为**真实导入**：`import "std/math.tie"`
  后 `gcd()`/`lcm()` 按前缀可用；
- 标准库文件内的 `namespace tcmath` 声明作为"默认前缀"，
  import 时可按 as 覆盖。

### 3.3 解析顺序（语义层）

```
裸调用 x():
  1. 当前函数局部 / 全局函数  →  命中则直接用（现状）
  2. 当前文件 ns_stack 前缀     →  补全为 ns.x（现状）
  3. 已导入的命名空间前缀       →  若导入集合中存在 "ns.x" 唯一候选则补全
  4. 多候选/无候选               →  报"未定义函数"（现状）
```

`import` 前缀重命名在 3.2 阶段额外登记"别名 → 原命名空间"映射。

## 5. 实现步骤

1. AST：`FnDefStmt` 增加 `visibility: Option<String>`；新增 `ImportAsStmt`
   （或复用 `Import` 结构加 `alias` 字段）；
2. parser：解析 `func (ns)` 语法与 `import "x.tie" as y`；
3. 语义层 symbol 注册改为 `ns.name` 全名键（`namespaces` 表结构性调整）；
   可见性校验（导入侧访问私有函数报错）；
4. `ns_call_full_name`/`resolve_call` 升级：导入前缀补全；
5. import 加载：按文件加载并将目标命名空间映射到别名；
6. 测试：冲突共存（两文件同名函数）、私有遮蔽（跨文件访问私有报错）、
   别名补全、std 库真实导入端到端；
7. 文档：language.md §7 模块与导入、ai-guide 同步、标准库文档更新；
8. 示例：examples/ns_import。

## 5. 验收标准

- 同名函数在不同命名空间共存，互不覆盖（按全名解析）；
- `func (ns) name` 私有函数：同命名空间内可见、跨命名空间（含导入）不可见；
- `import ... as alias` 后裸调用可通过 `alias.name` 点分调用，且补全生效；
- 现有 M2 命名空间测试全部通过（不破坏原语义）；
- workspace 编译零错误、测试全绿。

## 6. 影响范围

| 组件 | 影响 |
| --- | --- |
| tie-frontend（ast/parser/semantic/imports） | FnDefStmt 可见性、ImportStmt 别名、ns_call 升级、imports 路径解析 |
| tie-lsp | 语义补全（prefix 增加导入命名空间） |
| tie-llvm/tie-interp | 无 IR/求值变化（纯编译期解析） |
| std/ + prep/core.tie | 无影响（preexisting 语义） |