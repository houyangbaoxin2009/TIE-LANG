# 规划：tie 错误处理模型（Result/Option + ?传播 + 可配置 panic）

> 状态：**规划**（2026-08-15 设计讨论定稿，未实现）
> 本文档定义 tie 的错误处理模型。决策汇总：
> **E2**（Result/Option 枚举，Rust 风格）+ **P1**（`?` 传播运算符）
> + **F3**（可配置 panic：桌面打印退出 / 嵌入式 halt / wasm trap）
> + **R2**（内外分明：内部错误 panic，外部错误 Result）
> + **C1**（协程错误随 Result 跨 channel 传回）
> + **L1**（std 预置 Result/Option）。
> 明确排除：异常（E3，栈展开成本 + 嵌入式/wasm 不支持 + 哲学冲突）。
> 关联：enum/泛型/switch（已有基础设施）、并发模型（协程/actor/channel）、
> 嵌入式子集（tie:embedded）。

## 1. 现状（零机制）

- 无 panic、无异常、无 try/catch/throw
- 唯一机制：`std/assert.tie` 的 assert 族（打印 + `exit(1)`，不可恢复）
- **基础已就绪**：enum（ADT + payload + 泛型变体）+ switch 匹配 + 泛型单态化
  ——Result/Option 只需预置枚举 + `?` 语法糖

## 2. 基础形态（E2：Result/Option）

### 2.1 预置枚举（std，L1）

```tie
// std 预置（tie:class 库）
enum Result<T, E> {
    Ok(T)
    Err(E)
}

enum Option<T> {
    Some(T)
    None
}
```

- 与用户自定义 enum 无差别（预置只是生态统一）
- 构造：`Result.ok(v)` / `Result.err(e)` / `Option.some(v)` / `Option.none()`
- 解包：switch 匹配（已有）或 `?`（见 §3）

### 2.2 使用示例

```tie
func read_cfg() -> Result<string, string> {
    if !fs.exists("cfg.toml") {
        return Result.err("配置文件不存在")
    }
    return Result.ok(fs.read("cfg.toml"))
}

var res = read_cfg()
switch res {
    case Ok(v):
        println("配置: " + v)
    case Err(e):
        println("错误: " + e)
}
```

## 3. 传播机制（P1：`?` 运算符）

```tie
func load_all() -> Result<table<string>, string> {
    var cfg = try read_cfg() ?       // Err → 自动提前返回 Err(e)
    var data = try fs.read(cfg) ?    // 解包 Ok(v) 绑定到 data
    return Result.ok([data])
}
```

- `expr ?` 语义（仅限函数返回 Result 时）：
  - expr 是 Ok(v) → 解包为 v
  - expr 是 Err(e) → 函数立即返回 Err(e)（类型必须匹配）
- 错误类型一致性：`?` 要求被传播错误类型与函数返回 Err 类型一致
  （或提供 `map_err` 转换）
- 编译期检查：`?` 只能在返回 Result 的函数内使用（否则编译错误）
- 与闭包/协程咬合：闭包内 `?` 返回闭包的 Result（C1 见 §6）

## 4. 不可恢复错误（F3：可配置 panic）

### 4.1 panic 语句

```tie
panic("内存不足")        // 不可恢复错误
```

- 语义：逻辑错误/资源耗尽/内部不变量破坏——不可恢复
- 替代现状 assert+exit（assert 保留为测试断言，panic 为运行时不可恢复）

### 4.2 可配置行为（F3，按目标平台）

| 目标 | panic 行为 |
| --- | --- |
| 桌面（win/linux/macos） | 打印消息 + 退出（exit 码非零） |
| 嵌入式（tie:embedded） | halt（死循环/复位，无打印依赖）或配置回调 |
| webui（wasm） | trap（wasm unreachable）或 JS console 报错 |

- 行为由编译器按 target 配置（不引入异常/栈展开）
- panic 是"最后手段"，正常错误走 Result

## 5. 运行时检查归属（R2：内外分明）

| 错误类别 | 处理方式 | 例子 |
| --- | --- | --- |
| **内部错误**（程序 bug） | panic | 数组越界、除零、下标越界、溢出检查 |
| **外部错误**（环境/输入） | Result | IO 失败、用户输入非法、网络错误、配置缺失 |

```tie
var arr: table<i64> = [1, 2, 3]
var v = arr[5]                  // 越界 → panic（内部错误，程序 bug）

func parse_num(s: string) -> Result<i64, string> {
    // 用户输入解析失败 → Result（外部错误，可恢复）
}
```

- 原则：**能恢复的用 Result，不能恢复的用 panic**
- 编译器生成的检查（越界/除零）默认 panic（不可配置为 Result——
  简化，避免两套语义）

## 6. 协程/异步咬合（C1：错误随 Result 跨 channel）

```tie
// 协程函数返回 Result
async func fetch(url: string) -> Result<string, string> {
    var data = try http_get(url) ?
    return Result.ok(data)
}

// spawn 协程：Result 通过 channel 传回
var task = spawn(fetch, "https://...")
var res = await task             // Result<string, string>
switch res {
    case Ok(v): ...
    case Err(e): ...             // 协程内错误可恢复，主协程处理
}
```

- 协程/actor 的错误是**值**（Result），随 channel/await 传回——无跨协程异常
- actor 消息处理器返回 Result，错误经 reply channel 传回调用方
- **C2（actor panic 隔离）后置**：第一版 actor panic 即进程终止，
  监督树/隔离留待第二版（容错系统）

## 7. 与闭包/接口的咬合

```tie
// 闭包内 ?：传播到闭包返回类型
var parse_all = func(xs: table<string>) -> Result<table<i64>, string> {
    var out: table<i64> = []
    var i: i64 = 0
    while i < len(xs) {
        var v = try parse_num(xs[i]) ?
        table_push(out, v)
        i = i + 1
    }
    return Result.ok(out)
}

// 接口方法返回 Result（port 签名含错误类型）
port FileReader {
    pub func read(self, path: string) -> Result<string, string>
}
```

- 闭包：`?` 在闭包内传播到闭包的 Result 返回（与普通函数一致）
- 接口：port 方法签名可含 Result（错误类型成为接口契约一部分）

## 8. 编译器实现拆解（tiec 自举）

| 模块 | 改动 |
| --- | --- |
| 语法 | `?` 运算符（postfix，语义：Result 解包/提前返回） |
| semantic | `?` 类型检查（返回类型必须 Result、错误类型匹配）、panic 语句检查 |
| irgen | `?` 展开（Ok 解包 + Err 提前返回分支）、panic 调用发射（按 target 配置） |
| llvmgen | panic 目标（桌面：printf+exit；嵌入式：halt 循环；wasm：unreachable） |
| std | Result/Option 预置枚举 + 构造函数 + checked_* 关联（int 模型） |
| 运行时检查 | 越界/除零 → panic 调用点（现状 assert 升级） |

## 9. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 基础形态 | E2：Result/Option 枚举（enum 已有基础设施） | E1 错误码、E3 异常、E4 多返回 |
| 传播 | P1：`?` 运算符（解包 Ok / 提前返回 Err） | 异常自动传播、手动 match |
| panic | F3：可配置（桌面退出/嵌入式 halt/wasm trap） | 仅 assert、桌面专用 |
| 检查归属 | R2：内外分明（内部 panic / 外部 Result） | 全 panic、全 Result |
| 协程错误 | C1：Result 随 channel/await 传回 | C2 actor panic 隔离（后置） |
| 库预置 | L1：std 预置 Result/Option | 用户自定义 |
| 异常 | **排除**（栈展开 + 嵌入式/wasm 不支持 + 哲学冲突） | — |

## 10. 未决问题

1. **错误类型转换**：`?` 传播时 Err 类型不一致的转换（`map_err` 函数形态）
2. **Option 与 Result 互转**：`ok_or` / `and_then` / `unwrap` 等组合子函数族
   （第一版只做 `?` + switch，组合子后置）
3. **panic 消息格式**：是否带位置信息（文件:行）——建议带（调试价值高）
4. **断言与 panic 分工**：assert（测试期）+ panic（运行期）的边界文档化
5. **actor 监督（C2）**：panic 隔离/监督树——第二版设计，需 actor 运行时支持
