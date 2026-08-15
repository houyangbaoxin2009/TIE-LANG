# 规划：tie 宏/元编程模型（M3 函数式宏 + M4 过程宏渐进 + code 类型落地）

> 状态：**规划**（2026-08-15 设计讨论定稿，未实现）
> 本文档定义 tie 的宏/元编程模型。决策汇总：
> **M3**（函数式宏：tie 自身写宏，编译期 AST→AST）+ **M4**（过程宏：
> token 流 API，后置渐进）+ **C1+C2+C3**（code 类型三形态：AST 片段值 +
> 惰性 thunk + 编译期 eval）+ **T3**（展开时机：编译期为主 + interp eval
> 逃生舱）+ **H2+H3**（卫生性：词法卫生为默认 + 显式 gensym 逃生舱）。
> 关联：code 类型（TK_CODE=15 已占位）、泛型单态化（类型级宏）、闭包模型、
> 接口模型（派生 impl）、自举编译器（tiec 即 tie 写，宏引擎天然可嵌入）。

## 1. 现状

- **无任何宏/元编程机制**（无宏语法、无模板、无 eval 代码生成）
- `code` 类型已占位（types.tie: TK_CODE=15）但标注"编译器 IR 阶段扩展"，零实现
- 唯一"元"能力：`--module` 文件级文本转换（prep 阶段，`process(src)->string`，
  先例 prep/rename_tcmsg_to_log.tie）——文件转换器，非语言内宏
- 泛型单态化已实现（generics.md）：类型级模板，实参化展开

## 2. 宏形态（M3 函数式宏为主，M4 过程宏渐进）

### 2.1 M3：函数式宏（tie 自身写宏）

```tie
// 宏 = 编译期函数：AST → AST（用 tie 语言写，零新语法）
// 声明：macro 关键字标记，参数为 code 类型，返回 code
macro double(x: code) -> code {
    return `( ($x) * 2 )          // 准引用（反引号）：构造 AST 片段
}

// 使用：展开发生在编译期
var v = double(3 + 4)             // → var v = ((3 + 4) * 2)
```

- **用 tie 写宏**：复用语言全部能力（循环/递归/table/泛型），无需宏专用语法
- **准引用**：反引号 `` ` `` 构造 AST 片段；`$x` 插值（把参数 AST 嵌入）
- **卫生性**：H3 显式 gensym（见 §5）
- 与自举精神一致：tiec 本身是 tie 写的，宏引擎 = 编译器内嵌的 tie 函数

### 2.2 M4：过程宏（后置渐进）

```tie
// 过程宏：编译期执行 tie 代码，接收 token 流 → 输出 token 流
proc_macro fn derive_display(input: tokens) -> tokens {
    // 完整任意代码生成能力
    ...
}

// 使用：属性式
#[derive_display]
struct Point { ... }
```

- **M4 后置**：M3 验证后按需再加（token 流 API + 编译期执行环境）
- 适用：派生宏（derive impl/port）、代码生成器（生成样板 impl）
- 与 M3 关系：M3 是"表达式级"（AST→AST 片段），M4 是"项级"（token→token）

### 2.3 排除

- **M1 C 预处理宏**：文本替换无类型安全——哲学冲突
- **M2 Rust 声明宏（macro_rules!）**：模式匹配引擎是独立复杂度——
  函数式宏（M3）用语言自身表达同样能力，更简单

## 3. code 类型落地（C1+C2+C3：三形态并存）

### 3.1 C1：code = AST 片段值

```tie
var expr: code = `(x + 1)       // 准引用：AST 快照（编译期）
var stmts: code = `{ a = 1; b = 2 }
```

- `code` 类型：编译期 AST 片段（TK_CODE 落地）
- 表示：AST 树（复用编译器 AST 结构，arena 分配）
- 用途：宏的参数/返回值（M3 函数式宏的载体）

### 3.2 C2：code = 惰性代码（thunk）

```tie
var lazy: code = { println("hi") }   // 代码块 = 值（未执行）
lazy.call()                          // 执行（类似闭包，但语法级）

// 延迟执行场景：条件编译块、注册回调、模板惰性求值
var on_ready = { init(); render() }
if ready { on_ready.call() }
```

- **惰性 thunk**：代码块作为值持有，显式 `.call()` 执行
- 与闭包（C2 闭包模型）的区别：thunk 是语法级代码块（无捕获环境绑定），
  闭包是值级（有环境）——两者互补
- 可实现为语法糖：`{ stmts }` → 无捕获闭包（C2 闭包模型的地基）

### 3.3 C3：code = 编译期 eval

```tie
// 编译期生成代码并展开（编译期执行 eval）
var generated = eval_compile("func f() { return 42 }")  // 编译期求值
```

- **编译期 eval**：编译期执行代码生成（区别于运行期 eval_code）
- 用途：代码生成器（按配置生成代码）、DSL 编译（编译期计算 → 生成代码）
- 与 T3 的 interp eval_code（运行期）区分：C3 是编译期，见 §4

### 3.4 准引用与插值

```tie
macro make_getter(field: code) -> code {
    var name = gensym("get")     // H3 显式卫生：唯一标识符
    return `(
        func $name() -> i64 {
            return self.$field
        }
    )
}
```

- 反引号 `` ` `` 包裹 = 构造 AST（引用 AST 值）
- `$expr` = 插值（嵌入子 AST）；`$name` 标识符插值
- `@expr` = 拼接（展开列表，如生成多个语句）
- AST 检查：准引用里的代码必须语法合法（编译期报错）

## 4. 展开时机（T3：编译期为主 + interp eval 逃生舱）

- **宏在编译期展开**（AST 阶段），运行期零开销、无宏痕迹
- 展开流程：parser 产出 AST → 宏展开 pass（识别 macro 调用 → 执行宏函数
  → 替换 AST）→ semantic 检查（展开结果参与类型检查）
- 宏展开 pass 在 semantic 之前（展开后的代码要过类型检查）
- 展开递归：宏生成的代码可再含宏调用（展开深度限制，防无限递归）
- **运行期动态生成**：`eval_code` 走 interp（T3 逃生舱，见 §3.3）——
  与编译期宏互不干扰，两种机制并存

## 5. 卫生性（H2+H3：词法卫生默认 + 显式 gensym 逃生舱）

### 5.1 H2 词法卫生（默认，Rust 风格）

```tie
macro swap(a: code, b: code) -> code {
    var tmp = gensym("tmp")         // H3 逃生舱：需要显式唯一名时
    return `(
        var $tmp = $a
        $a = $b
        $b = $tmp
    )
}
```

- **默认词法卫生**：宏内绑定的变量自动与调用方隔离（hygiene 上下文）——
  宏内 `var tmp` 不会捕获调用方的 `tmp`，无需手动改名
- 编译器维护卫生上下文：宏展开引入的绑定标记"宏私有"，
  与调用方符号表隔离（Rust macro hygiene 同款）
- 调用方传入的代码（`$a`）保留调用方上下文（不被宏污染）

### 5.2 H3 显式 gensym（逃生舱）

```tie
// 需要跨卫生边界共享/导出的名称：显式 gensym
macro gen_pub(name_hint: code) -> code {
    var pub_name = gensym("get_")   // 生成宏内+调用方都可见的唯一名
    return `(
        func $pub_name() -> i64 { ... }
    )
}
```

- 场景：宏生成的函数/类型需要被调用方引用（导出符号）、
  宏间共享名称、需要确定性命名时
- `gensym(prefix)`：生成唯一标识符（intern 池 + 计数器），
  默认词法卫生隔离；显式 gensym 突破隔离
- 职责分工：**默认词法卫生管住 90% 的冲突，gensym 管剩余 10% 的
  显式共享需求**——安全与灵活兼得

## 6. 与现有机制的咬合

| 机制 | 关系 |
| --- | --- |
| 泛型单态化 | 泛型 = 类型级宏；宏 = 语法级泛型。互补不重叠 |
| 闭包 C2 | 宏可生成闭包（回调样板生成） |
| 接口 port | 宏可派生 impl（`macro derive_impl` 生成 vtable 样板）——M4 主场景 |
| enum/switch | 宏生成匹配样板（穷尽匹配展开） |
| --module 预处理 | 文件级文本转换保留；宏是语言内 AST 机制，不同层 |
| 错误处理 | 宏展开错误带宏调用位置（调试：展开栈） |

## 7. 编译器实现拆解（tiec 自举）

| 模块 | 改动 |
| --- | --- |
| code 类型 | TK_CODE 落地：AST 片段类型（编译期值） |
| 准引用语法 | 反引号 `` ` `` 解析 + `$`/`@` 插值解析 |
| macro 声明 | `macro` 关键字、参数/返回类型检查（code 类型） |
| 展开 pass | 宏展开（调用识别 → 执行宏函数 → AST 替换）、递归限制、展开栈错误 |
| gensym | 唯一标识符生成（intern 池 + 计数器） |
| proc_macro（M4） | token 流 API + 编译期执行环境 + 属性语法 `#[...]` |
| std 桥 | eval_code（interp 逃生舱，已有 eval 机制） |

## 8. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 宏形态 | M3 函数式宏（tie 写，AST→AST）+ M4 过程宏（后置渐进） | M1 C 宏（排除）、M2 声明宏（排除） |
| code 类型 | **C1+C2+C3 三形态**：AST 片段值 + 惰性 thunk + 编译期 eval | 单一形态 |
| 展开时机 | T3：编译期为主 + interp eval 逃生舱 | T1 纯编译期、T2 纯运行期 |
| 卫生性 | **H2+H3**：词法卫生默认 + 显式 gensym 逃生舱 | H1 非卫生（排除） |
| 排除 | M1 文本宏（无类型安全）、M2 声明宏（独立复杂度） | — |

## 9. 待确认项（讨论未拍板）

1. ~~**展开时机 T3**~~ **已定案**：编译期为主 + interp eval 逃生舱
2. ~~**卫生性 H2+H3**~~ **已定案**：词法卫生默认 + 显式 gensym 逃生舱
3. **宏的可见性**：宏能否跨文件/跨包导出（`pub macro`？宏与库模型/包模型
   的关系——tieir 分发时宏怎么携带）
4. **proc_macro 触发时机**：M4 的属性语法形态（`#[derive_display]` 确认？）

## 10. 未决问题

1. **宏展开与单态化的顺序**：宏先展开 → 再单态化？还是交错？（建议：先宏后
   泛型——宏可生成泛型代码，单态化处理展开结果）
2. **准引用的类型检查**：准引用片段在构造时检查还是展开后检查（展开后统一
   检查——与"展开结果参与类型检查"一致）
3. **宏调试**：展开后代码的可读性（`--expand-macros` 输出展开结果，类似
   `--emit-ir`）
4. **宏与角色约束**：`macro` 声明在哪些角色文件可用（class/port 库宏 vs
   logic 本地宏）
5. **gensym 与 intern 的冲突**：gensym 名在 intern 池的可见性（宏内生成名
   不污染用户符号表）
