# 规划（后续 M）：switch 模式匹配增强

> 状态：**已实现**（Harbor M2.1.5，2026-08-08 落地；见 CHANGELOG）
> 所属：tie 语言核心语法增强
> 依赖：Harbor M3（预处理器自举）完成后排期

## 1. 背景与现状

tie 已有 `switch` 语句（M1 实现），当前能力：

- `switch subject { case 值: 语句… default: 语句… }`
- `case` 值限**编译期字面量**：整数 / 字符 / 布尔 / 负数 / 字符串
- 无 break、无 fallthrough（一个 case 执行完自动跳出）
- IR 层展开为**比较链**（`sw.cmp` → 逐 case `icmp eq` → `sw.body`/`sw.default` → `sw.exit`）

现有局限：

1. **单值匹配**：一个 case 只能匹配一个字面量，`case 1: case 2:` 并写（多值）不支持；
2. **无区间匹配**：无法表达 `case 1..10:`（1 到 10 的整数）；
3. **无类型/守卫匹配**：无法按类型匹配或加 `when` 守卫条件；
4. 与 `if/else if` 链相比优势有限——switch 目前只是语法糖。

## 2. 目标

在不破坏现有语义的前提下，将 switch 增强为**类 Rust match / C# switch 表达式**的
模式匹配语句，覆盖高频场景：

| 场景 | 现状 | 目标 |
| --- | --- | --- |
| 多值合一 | 不支持 | `case 1, 2:`（或 `case 1 \| 2:`） |
| 整数区间 | 不支持 | `case 1..10:`（含 1 不含 10，与 for 语义一致） |
| 守卫条件 | 不支持 | `case 1 when x > 3:` |
| 类型匹配 | 不支持 | `case string:` / `case i64:`（匹配 subject 的动态类型） |
| 字符串/枚举多值 | 支持单值 | 同上多值合一 |

不做（明确排除）：元组解构匹配、`@` 绑定、通配 `_` 捕获（留待后续更大版本）。

## 3. 语法设计

```c
switch subject {
    case 1, 2:                 // 多值：任一相等即命中（逗号分隔）
        println("一二")
    case 3..7:                 // 区间：3 ≤ subject < 7（整数，与 0..10 语义一致）
        println("三四五六")
    case 8 when flag:          // 守卫：值相等 且 when 条件为真
        println("八且 flag")
    case string:               // 类型匹配：subject 是 string 类型（配合动态类型/元组）
        println("字符串")
    default:
        println("其他")
}
```

设计要点：

- **case 值与 subject 类型一致性**沿用现有检查（语义层比较字面量类型与 subject 类型）；
- **多值** = 编译期展开为多个相等比较的 OR；与区间/守卫可自由组合
  （`case 1, 3..5 when cond:`）；
- **区间**仅限整数（`i8..i64`）与字符（`'a'..'z'`）；浮点区间因边界含入语义模糊，
  明确不支持；
- **类型匹配**仅在 subject 是 `code`（编译期类型）或表/元组等**动态类型容器**时
  有意义；普通静态类型变量上禁止（类型恒定，恒真/恒假无意义）——语义层报错；
- **default 可省略**（沿用现状）；守卫不满足时落入下一个 case（顺序匹配，与
  Rust match 的 guard-fail → 继续匹配一致）。

## 4. 实现方案

### 4.1 词法/语法（tie-frontend）

- `SwitchCase.value: Expr` 改为 `SwitchCase.patterns: Vec<Expr>`（多值列表）；
- 新增可选 `when: Option<Expr>` 字段；
- 区间复用现有 `Range { start, end }` 表达式（语法已是 `3..7`）；
- parser：`case 值[, 值]... [when 条件]:`。

### 4.2 语义（tie-frontend/semantic.rs）

- 校验每个 pattern 与 subject 类型一致（沿用现有检查）；
- 区间 pattern：检查 start/end 为整数或字符字面量，且 start < end；
- when 守卫：检查为 bool 表达式（与 if 条件同规则）；
- 类型匹配 pattern：仅在 subject 为 `code`/`text` 等宽类型或动态容器时允许；
- 多值全校验通过后，注册 case 数量（供 IR 展开）。

### 4.3 IR 生成（tie-llvm/ir.rs）

- 保持"比较链 + 分支块"总结构（`sw.cmp`/`sw.body.*`/`sw.default`/`sw.exit`）；
- 多值 → 每个值一个 `icmp eq`，OR 合并后跳 body；
- 区间 → 两个比较（`sge start && slt end`）AND 合并；
- 守卫 → 值比较 AND 守卫条件，两条件都满足才跳 body；
- 类型匹配 → 对动态容器做 `is_xxx` 运行时检查（若引入类型标签，见下）。

### 4.4 解释器（tie-interp）

- eval_expr 的 Range 已支持；case 匹配用 `==` 求值 + 短路，语义与 IR 对齐；
- 类型匹配：Value 枚举已有类型标签（Int/Float/Bool/Char/Str/Table…），
  直接按 Value 变体匹配。

### 4.5 自举模块（prep/core.tie）

- 若 switch 增强在自举链路中需要（如 `header_kind` 的多值匹配），同步更新
  `prep/core.tie`，保持"模块自包含 + 协议文本"不变。

## 5. 实现步骤（排期建议：一个 M 里程碑内完成）

1. AST 改造：`SwitchCase` 增加 `patterns`/`when` 字段（保留旧字段迁移期兼容）；
2. parser 解析多值/区间/守卫/类型匹配；
3. 语义层校验（类型一致性 + 区间 + 守卫 bool）；
4. IR 生成（OR/AND 合并比较）；
5. interp 求值对齐；
6. 测试：frontend 语义（合法/非法用例）、IR 展开（多值 OR、区间 AND、守卫）、
   interp 行为一致；
7. 文档：language.md §5 语句、ai-guide §2.3 控制流、prompt-pack 同步；
8. 示例：examples/switch_pattern.tie。

## 6. 验收标准

- `case 1, 2:` / `case 3..7:` / `case 8 when flag:` 在编译（IR 正确展开）与
  解释（interp 结果一致）两条路径行为一致；
- 非法用例（类型不匹配、浮点区间、静态类型上类型匹配、when 非 bool）全部在
  语义层报错，不产生 IR；
- 现有 switch 测试（frontend/llvm/interp 各 1 个）不改动继续通过；
- 全工作区 `cargo build --workspace` 零错误、`cargo test --workspace` 全绿。

## 7. 影响范围

| 组件 | 影响 |
| --- | --- |
| tie-frontend（ast/parser/semantic） | SwitchCase 结构变更 + 新校验 |
| tie-llvm（ir.rs） | switch 展开逻辑扩展 |
| tie-interp | case 匹配求值扩展 |
| docs（language/ai-guide/prompt-pack） | 语法章节更新 |
| std/ + prep/core.tie | 不影响（无 switch 增强依赖） |
