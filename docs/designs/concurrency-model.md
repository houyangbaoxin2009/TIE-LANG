# tie 并发模型设计（原生 actor + 凭据门禁，零运行时）

> 状态：**设计定稿**（2026-08-23 会话对齐：actor 语法收敛 + unsafe=凭据门禁）
> 日期：2026-08-23
> 定位：把 tie 的并发能力定为一等公民 **actor 原生语法**（默认安全）
>  + **unsafe 凭据门禁**（老鸟越界三域 mem/ext/share）。
> 一期实现：actor 语法定稿 + **零运行时纯编译**（1:1 OS 线程 + mailbox 就地 codegen）。
> 哲学：**小白用安全语法写好项目；一切越界能力归凭据，老鸟持证使用。**

---

## 1. 一句话总览

tie 的并发**不在语言层对小白自造线程模型**，而是两条路分工：

- **安全默认（所有人）**：`actor` 是 tie **原生语法**，消息传递把竞态在语言层消除。
  同步/异步显式（默认同步 + `async`），句柄可复制，可选 `move` 收紧。
- **越界逃生（老鸟）**：跨 actor/线程共享等一切不安全能力，归 **`guard<cap>` 凭据门禁**
  （三域 mem/ext/share），持证使用，可委派/对象绑定/层级回收/审计。
- **执行**：`actor` **完全纯编译**——tiec 把它直接降到 **LLVM → 原生**，用 **1:1 OS 线程**
  跑 mailbox（串行消费）。不碰 trm、不引 VM/解释器、无运行期 JIT，产物是零依赖原生 exe。

一句话：**小白用 actor，老鸟持凭据；actor 完全纯编译（路线 A），trm 不参与 actor 执行**。

---

## 2. 现状盘点（以代码为准）

| 能力域 | 现状 | 依据 |
| --- | --- | --- |
| 内存管理 | ✅ 表作用域确定性释放（无 GC）| 语言核心（路线 A）|
| 移动语义 | ✅ S1.3 / S1.5 smove（跨线程所有权转移底座）| docs/plans/roadmap.md |
| unsafe/ptr/slice/asm | ✅ S1.2 | docs/plans/unsafe-model.md |
| 原子类型 | ✅ `atomic<T>`（load/store/atomicrmw/cmpxchg + 内存序）| types.tie K_ATOMIC；sinfer.tie |
| port/impl 接口 | ✅（actor 消息契约复用）| 语言已支持 |
| actor | ✗ 无（一期实现）| 语言级（tiec codegen，零运行时）|
| trm（tie 运行时）| 独立于 actor（actor 不用它）| docs/plans/trm-arch.md |

> 结论：`atomic`、`unsafe`、`port`、move 均已就位；缺的正是 **actor 语法**（一期 pure-compile 落地）。

---

## 3. 设计目标与约束

**目标**
- 后端服务承载、UI 响应、CPU 并行；数据竞争在**语言层**消除（actor 免锁）。
- 语义严谨、行为可预期；同步/异步显式，句柄所有权显式。

**约束**
- 保持 0-Rust 自举：actor **完全 pure-compile 到原生**（tiec codegen，零运行时），不引任何运行时。
- actor 现走**路线 A 纯编译零依赖**（1:1 OS 线程 + mailbox 就地 codegen）；与 trm 解耦，
  不 gate 于任何运行时。
- 每期自举闭环 + regress + probe 验收；编译零错误。

---

## 4. 并发模型：原生 actor + 凭据门禁

**决策**：不采用「共享内存线程」为主，而是 **actor（原生语法）为语言面**；
跨 actor 共享可变内存仅作为 `unsafe` 逃生口。分层：

```
tie 应用层
│
L4  业务并发模式（actor / worker pool）        ← actor 原生语法（语言一等公民）
L3  消息契约（actor 内直声明）                  ← 方法签名即消息类型；默认同步
L2  同步原语（atomic / `guard<share>` 越界逃生口）← atomic 已实现
L1  执行层（1:1 OS 线程 + mailbox）             ← tiec 就地 codegen（零运行时）
```

- 「共享内存」不再是一层，只是 L2 逃生口；actor 消息是唯一主数据通路。
- Erlang actor 与 Go channel 的差别由执行层吸收：actor 的 mailbox 即消息通道，
  由 tiec 编译进每份产物（不依赖任何运行时）。

---

## 5. actor 原生语法

### 5.1 基础形态

```tie
actor Counter {
    var count: i64 = 0                       // 私有状态，仅本 actor 线程读写 → 免锁

    // 默认同步：方法签名即消息，调用方阻塞等结果
    pub func inc(by: i64) -> i64 {
        count = count + by
        return count
    }
    // async：投递即返回（fire-and-forget）
    pub async func reset() {
        count = 0
    }
}

var c = run Counter()                        // run(构造) → 返回可复制句柄
var v = c.inc(5)                             // 同步 RPC → 5（阻塞）
c.reset()                                    // async 投递，不等
run Logger().log("start")                    // 语句位 run → fire-and-forget（不占句柄）
```

- **`run` 是专用关键字**：表达式位置 = 创建并取句柄；语句位置 = fire-and-forget 投递。
- 同步/异步：**默认同步**（有返回类型 = RPC 等结果），`async` 关键字 = 投递即返回。

### 5.2 同步/异步

| 写法 | 语义 | 返回值 |
| --- | --- | --- |
| `pub func m(…)` | 同步 RPC：入队 + 阻塞直到 actor 应答 | 可有返回类型 |
| `pub async func m(…)` | 异步投递：入队即返回（fire-and-forget）| 必须 `void` |

- **缺省 = 同步（默认路径，不强制写关键字）**；需要 fire-and-forget 才显式 `async`。

### 5.3 句柄：可复制 + 可选 move

```tie
var a = run Account()        // 句柄（运行期一张 actor 表管所有权 & 生命周期）
a.deposit(100)               // 方法调用，句柄仍可用
var b = a                    // ✓ 复制句柄（同一 actor，Erlang PID 式）
var own = move a             // ✓ 可选 move 收紧所有权（move 后 a 作废）
```

- 可复制句柄对标 Erlang PID；需要严格所有权时用 `move a`（复用移动语义底座）。

### 5.4 消息契约：actor 内直声明（port 渐进）

```tie
actor Account {
    var bal: i64 = 0
    pub func deposit(amt: i64) -> i64 {
        bal = bal + amt
        return bal
    }
    pub func withdraw(amt: i64) -> i64 {
        if amt > bal { panic("overdraft") }
        bal = bal - amt
        return bal
    }
}
```

- 一期：actor 内直接声明消息方法（自足单文件）。
- 后续需要跨 actor / 接口注入时，再外置到 `port` + `impl`，语法不占用 actor 内声明。

### 5.5 语义规则（定稿）

| 规则 | 语义 |
| --- | --- |
| 串行处理 | mailbox 单消费者，一次一条消息 → 状态免锁 |
| 调用方阻塞 | 有返回的 RPC 阻塞直到应答 |
| 默认不重入 | actor 阻塞在对外 RPC 时**不**处理其它入站消息；需要则标 `reentrant` |
| 失败传播 | 处理器 `panic` → 应答带失败，调用方原地 raise |
| move 边界 | 参数 move 进消息、结果 move 回；大表零拷贝所有权转移 |
| 越界逃生 | 真跨 actor 共享可变内存走 **`guard<share>` 凭据**（见 §7）|

### 5.6 死锁与重入

默认不重入 → 环 `A → B → A` 会死锁。宁可死锁（安全）不外乱序；单点打破：

```tie
actor Router {
    var registry: map
    pub sync reentrant func route(id: i64) -> i64 {
        return registry.get(id).deposit(0)   // 等待期间允许处理其它消息 → 破环
    }
}
```

---

## 6. 执行层：actor 零运行时，纯编译原生语法

**硬约束：tie 的运行时（若存在）即 trm；而 actor 是原生语法，不需要任何运行时。**
tiec 直接把 actor 降到 LLVM → 原生：mailbox、1:1 OS 线程、应答槽全部**由编译器就地
代码生成**（直接调 OS 原语：`CreateThread` / `CRITICAL_SECTION` / `WaitForSingleObject`
或 pthread 对应物），**不链接任何运行时库**（连 `std/runtime.a` 都不进）。

| 职责 | 归属 | 实现（tiec 直接代码生成，零运行时） |
| --- | --- | --- |
| 邮箱 / 消息队列 | tiec codegen | 就地生成互斥 + 条件变量 + 队列（直接 OS 原语）|
| 执行模型 | tiec codegen | **1:1 OS 线程**（`CreateThread`/pthread 直接调用）|
| 同步 RPC 应答 | tiec codegen | 应答槽 + 信号量 / `WaitForSingleObject` |
| 状态隔离 / 所有权 | 语言语义 | 单线程独占 + move 所有权转移 |

- **actor 不需要运行时**：一切机制由 tiec 编译进产物本身；产物零依赖、无 VM、无解释器。
- 术语：tie 的运行时（如有）即 trm 之概念；actor 作为原生语法与 trm 解耦，互不强依赖。
- 二期只做语义与凭据增强（§10），不改执行模型与零运行时承诺。

---

## 7. 同步原语（L2 逃生口 = **凭据门禁**）

unsafe 越界统一走 **凭据门禁**（完整设计见 [unsafe-model.md](../plans/unsafe-model.md) §13）：

```tie
var g = unsafe.get(share)                 // 「并发共享」凭据（move-only guard<share>）
unsafe use g { buf[0] = compute() }       // 持证越界：跨线程/Actor 共享可变
unsafe with(share) { ... }                // 作用域临时凭据，退块自动回收
var g2 = g.delegate(share)                // 限制委托/衰减
var og = unsafe.get(share -> buf)         // 对象绑定：只能碰 buf
var child = g.branch(); unsafe.revoke(g)  // 层级撤销（父亡子亡）
unsafe.audit(g)                           // 运行期审计调用链
#[unsafe(share)] fn agg() -> i64 { ... }  // 函数级便捷（隐式持证）
```

- `atomic<T>`：**已实现**；非 unsafe 代码默认仅 `seq_cst`，弱序须持 `guard<share>` 显式标注。
- `Mutex`/`RwLock`：**不作为主路径**（主路径是 actor 消息）；仅凭据逃生口底层 bridge 到
  CRITICAL_SECTION / pthread，是否进正式 API 待定。

### 7.1 actor 不安全语法新成员（A/B/C 组，2026-08-23 定稿）

> actor 安全根基 =「消息传递 + 状态私有 + 串行消费」。**不安全语法新成员** = 打破这三条
> 之一、暴露底层能力的语法。默认（安全）actor 保持一期纯编译零运行时；老鸟用下列成员
> 越界。与 [trm 定稿](../designs/trm-final-design.md) §8 一致：**默认解耦（路线 A）+
> unsafe 显式接入（路线 B）**。
>
> 三组定位：
> - **A 组** = 打破「状态私有」：跨 actor 共享可变内存（`guard<share>` 逃生口）
> - **B 组** = 打破「消息传递/串行」：原生共享消息（unsafe 开多参数 + 指针/slice）
> - **C 组** = 打破「执行模型」：接入 trm 运行时（`#[unsafe.trm]`，M:N 协程 + GC + 反射）
>
> **通用 `#[]` 属性通道（2026-08-23 定）**：现有 `#[]` 仅支持 `#[macro]`（写死在
> `parse_proc_macro_attr`）。扩展为通用属性解析器，接受命名空间式属性：
> - `#[unsafe.share]` / `#[unsafe.trm]` / `#[unsafe.mem]` / `#[unsafe.ext]`——unsafe 凭据
>   门禁，**点号 `ns.sub` 风格**，与 tie 命名空间方法调用 `ns.method()` 一致
> - `#[tag.xxx]`——**goto 跳转标签**（见 7.1.5），`tag.` 前缀区分于属性
> - `#[macro]`——过程宏标记（保留兼容）
> `#[]` 里紧跟声明 = 属性；紧跟语句 + 标签 = 跳转标签（靠 `tag.` 前缀 + 上下文区分）。

#### 7.1.1 A 组：跨 actor 共享内存（`guard<share>` 基础 + 对象绑定）

```tie
var g  = unsafe.get(share)             // 基础凭据（move-only guard<share>）
unsafe use g { shared.buf[0] = x }     // 持证跨 actor 读写共享内存
#[unsafe.share] fn agg() -> i64 { }   // 函数级便捷（隐式持证，命名空间属性）

var og = unsafe.get(share -> table_ptr) // 对象绑定：只能碰这张表，防滥用任意内存
```

- 一期落地：`unsafe.get(share)` / `unsafe use g {}` / `#[unsafe.share]` / **对象绑定**
  `unsafe.get(share -> obj)`。
- 对象绑定收窄越界面：凭据绑定到具体对象，只能碰它，不能动任意内存。
- 依赖：`guard<share>` 类型 + 凭据操作集落地（编译器新内置，见 §13）。

#### 7.1.2 B 组：原生共享消息（unsafe 开多参数 + 指针/slice，move 所有权）

```tie
#[unsafe.trm] actor Worker {                          // 或方法级 #[unsafe.share]
    #[unsafe.share] pub func proc(buf: ptr<u8>, n: i64) -> i64 { ... }
    // ptr/slice 作消息参数：move 进消息（所有权转移给接收 actor），结果 move 回
}
```

- 安全 actor 保持**单参标量**（一期）；unsafe 声明的方法开 **多参数 + `ptr<T>`/`slice<T>`**。
- **move 所有权**：指针/slice 参数 move 进消息队列（所有权转移），结果 move 回调用方，
  跨 actor 零拷贝共享缓冲（复用移动语义底座）。
- 仅 `#[unsafe.share]` / unsafe 上下文方法可用指针参数；安全路径不暴露指针消息。

#### 7.1.3 C 组：接入 trm 运行时（`#[unsafe.trm]`，属性标记，两级）

```tie
#[unsafe.trm] actor Huge {                 // actor 级：整个 actor 消息处理由 trm 调度
    var cache: map
    pub func big() -> i64 { ... }           // 随之 actor 接入 trm

    #[unsafe.trm] pub func hot() -> i64 { }  // 方法级：仅此方法走 trm
    pub func cool() -> i64 { }              // 未标 = 保持路线 A（若 actor 级已标则随 actor）
}
```

- **actor 级**：整 actor 消息处理由 trm 调度（M:N 协程 + GC + 反射）。
- **方法级**：仅该方法接入 trm。
- 两级叠加：actor 级标注后，方法级标注作显式确认；无标注方法随之 actor 级。
- **语义：语法零改动**，只换执行/调度机制（路线 A 纯编译 ↔ 路线 B trm）。
- 风格统一：`#[unsafe.trm]` 命名空间属性，沿用 unsafe-model §13 属性门禁精神。

#### 7.1.4 三组协同与语义护栏

- **A 组**给跨 actor 共享内存逃生口；**B 组**给原生共享消息零拷贝通道；**C 组**给接入
  trm 运行时的语法门。三者正交可叠加。
- **安全 actor（无任何标注）**：保持一期纯编译零运行时，状态私有、消息串行、无共享——
  默认安全不受影响。
- 指针/slice 消息只在 unsafe 声明的 actor/方法内可用；共享内存只在持 `guard<share>`
  时可用；接入 trm 只在 `#[unsafe.trm]` 标注处发生。

#### 7.1.5 goto 与跳转标签（语义边界定稿，2026-08-23）

> tie 现有控制流：结构化 `if/while/for/switch` + **循环标签（E5）** `标识符: while/for`
> + `break L` / `continue L`（跳多层循环）。**无 goto**。
> goto 服务于低层复杂状态机 / 平台实现 / 中断流（结构化表达生硬或需重复 flag）。
>
> **标签命名空间分离**：循环标签用**冒号**式 `L:`，goto 标签用 **[x] 式** `#[tag.x]`，
> 二者完全独立、互不干扰——编译期用**循环标签栈 + goto 标签表**两套独立校验。

##### 语法

```tie
#[tag.loop]                    // 标签（语句位置，代表「此处可被 goto 抵达」）
do_work()
#[tag.exit]
cleanup()

unsafe goto #loop              // 跳转（仅 unsafe 上下文；# 引用 goto 标签）
unsafe goto #exit
```

- **标签** `#[tag.name]`：语句位置，标定一条语句前为跳转点。
- **跳转** `unsafe goto #name`：无条件改指。`#` 前缀区别于循环标签 `标识符:`。

##### 作用域与判定规则（编译器强制，违反 = 编译错误）

**R1 同函数**：goto 目标必须是**同函数内**定义的 `#[tag.x]`。跨函数 goto = 编译错误
（函数是控制流边界，禁止「千里奔袭」进另一函数）。

**R2 同块或已执行外层块**：goto 目标必须满足其一：
- 与 goto 处于**同一语句块**（函数顶层块 / 同一 `if/while/for/switch/unsafe` 体内部）；
- 或**位于 goto 的外层块**（向后跳到已建立作用域的块头，用于循环模拟）。
- **禁止跳入更内层块**（`if/while/for/switch/unsafe` 的体内标签）——进入内层块 = 该块
  作用域未建立，破坏确定性释放。违反 = 编译错误。

**R3 不越过初始化**：goto 从当前位置到目标之间**不得经过任何局部 `var`/`const` 声明**
（不得跳过变量初始化后使用之）。R1+R2 已框定同块/外层，R3 是最后防线——保证目标点上
引用的每个局部变量必然已初始化。违反 = 编译错误。

> 三条合起来的效果：goto 只允许在**同一块内任意前/后跳**，或**向后跳同一函数已执行过的
> 外层块头**。任何当前块内变量生命周期完整，块结束统一释放不受干扰。

##### 释放 / 所有权安全

- tie 靠**表作用域确定性释放**。R2（不跨内层块）+ R3（不跳过初始化）保证：任意 goto
  路径上的局部变量都能被完整初始化 → 完整释放，**无泄漏、无双重释放**。goto 不建立也不
  破坏任何作用域，只是控制流改指。
- goto 向后（循环模拟）导致的重复执行由各次独立作用域承载（`#[tag.top] ... unsafe goto #top`
  每轮是独立的块生命周期），语义确定。

##### 与现有语法交互

- `break` / `continue` / `return`：语义**不受 goto 影响**；它们不改变 goto 标签的有效性。
- `goto` 不建立循环上下文：不进入程序上的异常（即有异常）`lp_stack`。`unsafe goto`
  之后的 `break`/`continue` **不因经过 goto 而改变所属循环**（仍按词法所在循环匹配）。
- 死代码：goto 之后不可达、且没有被任何标签引用的语句 → opt 清理，**不报错**
  （对齐结构化语言的无限循环/死代码容忍）。

##### 实现映射（tiec codegen，零运行时）

- `#[tag.x]` → 目标基本块起点；`unsafe goto #x` → LLVM 无条件 `br` 到标签对应块。
- 标签表：模块级 `goto_tags`（name → 当前函数块指针表），语义校验时收集、irgen 时定位。
- 死块由 opt DCE 清；goto 合法路径由 R1-R3 静态校验，无运行期成本。

##### 例子（合法）

```tie
unsafe fn poll() {                     // 状态机：同一块内前/后跳
    #[tag.again]
    var r = read()
    if r == 0 { unsafe goto #again }   // 向后跳同块头部（自身块，R2 合法）
    #[tag.done]
    finish(r)
}
```

##### 反例（编译错误）

```tie
unsafe fn bad() {
    var x = 1
    #[tag.inner]
    if x {
        unsafe goto #outer              // ✗ target 在外层块，goto 在内层 if 体
    }
    #[tag.outer]
    ...
}
```

---

## 8. 通道 / select（弱化为底层）

- 一阶不单独暴露通用 `Chan<T>` 给业务；actor 消息即通道抽象。
- `select`/超时/多路选择若需要，作为 actor 消息 + 运行期阻塞的库层补充，后续视需提供
  （走前 compile 的 mailbox 原语，不依赖 trm）。

---

## 9. async/await（附：继承消息机制）

- 依赖 actor 消息 + 运行期阻塞；`pub async func` 已是投递侧异步。
- 真正的 `await` 表达式 / 方法内暂停，在纯编译路线 A 上实现（阻塞 + 应答槽回调）；
  一期先不做栈切换语法，后续再叠加。

---

## 10. 分期与验收

| 期 | 内容 | 底层 | 验收 |
| --- | --- | --- | --- |
| **一期（语法定稿 + 纯编译执行）** | actor 语法：`run` 创建/fire-and-forget + 默认同步 + `async` + 可复制句柄 + 内直消息；mailbox + 1:1 OS 线程；`guard<share>` 最小闭环（get/use/with/函数级便捷） | 语言级（tiec codegen，零运行时） | actor probe 编译运行；同步 RPC / async 投递 / fire-and-forget 行为正确 |
| **二期（actor 进阶 + 凭据完成）** | `reentrant` 重入、值类型消息零拷贝、跨文件 actor；`guard<share>` 完整（delegate / 对象绑定 / branch+revoke / audit）+ atomic 弱序 | 语言级（tiec codegen，零运行时） | 重入破环 probe；多 actor 并发 + 状态隔离；凭据全操作探针 |
| **三期（actor unsafe 语法 + trm 接入）** | actor 不安全语法新成员（§7.1）：A 组共享内存（`guard<share>` 基础+对象绑定）、B 组原生共享消息（unsafe 多参数 + ptr/slice，move 所有权）、C 组接入 trm（`#[unsafe.trm]` 两级，需 trm 引擎 P2 GC/M:N 后置）+ goto/标签（§7.1.5）；同步扩展通用 `#[]` 属性通道 | A/B/goto 纯编译零运行时；C 依赖 trm 路线 B | 跨 actor 共享探针；原生缓冲消息探针；`#[unsafe.trm]` actor 由 trm 调度探针；goto 标签探针 |
| **四期（全凭据面）** | `guard<mem>` / `guard<ext>` 落地，覆盖 §1.2 全部 7 类 unsafe 能力 | 语言级（tiec codegen，零运行时） | 竞争基准 + UI 事件驱动样例 |

- 每期独立提交、双端推送；`roadmap.md` P1 并发项按此分解。
- **实现顺序（2026-08-23 定）**：A → B → C；A/B 走纯编译（路线 A）；C 接入 trm，
  gate 于 trm 引擎 P2（GC + M:N）。通用 `#[]` 属性通道随 A 组先行扩展。
- **默认 actor 一律纯编译（路线 A），与 trm 默认解耦**；老鸟用 `#[unsafe.trm]` 显式接入
  路线 B——与 trm 定稿 §8 一致（默认安全，unsafe 显式解锁）。
- 海量 M:N 并发为**独立立项选项**（不 gate actor），未来需要再评估。

---

## 11. 参考对照

| tie（actor 原生 + 凭据门禁）| Erlang | Go | Rust |
| --- | --- | --- | --- |
| actor 原生语法 + 内直消息 | process + 消息 | goroutine + channel | `std::sync::mpsc` |
| 默认同步 + `async` 关键字 | 同步返回/异步发 | channel 显式阻塞 | 返回值/发送 |
| 可复制句柄 + 可选 move | process id（复制安全）| goroutine 无句柄 | `Send`/`move` |
| 零运行时 · tiec codegen → 原生（1:1 线程；M:N 为独立项）| BEAM 调度 | runtime 调度 | OS 线程 1:1 |
| unsafe = `guard<cap>` 凭据门禁 | — | — | `unsafe` + 借用检查 |

---

## 12. 相关文档与待决问题

- 相关：[docs/language-comparison.md](../language-comparison.md)（缺口与路线）、
  [docs/designs/trm-final-design.md](../designs/trm-final-design.md)（trm 定稿，§8 actor 接入、
  §10 能力分期）、[docs/plans/trm-arch.md](../plans/trm-arch.md)（规划稿）、
  [docs/plans/roadmap.md](../plans/roadmap.md)、[docs/plans/unsafe-model.md](../plans/unsafe-model.md)
  （含 §13 凭据门禁设计）。
- 已收敛（2026-08-23）：actor 创建关键字=`run`；默认同步+`async`；句柄可复制+可选 move；
  消息=actor 内直声明（port 渐进）；unsafe=凭据门禁三域 mem/ext/share+委派/对象绑定/层级回收/审计。
- actor 不安全语法新成员（2026-08-23，§7.1）：A 组 `guard<share>` 共享内存（基础+对象绑定）、
  B 组 unsafe 多参数+ptr/slice move 消息、C 组 `#[unsafe.trm]` 两级接入 trm、goto/标签
  （`#[tag.x]` + `unsafe goto #x`）；顺序 A→B→C，属性 `#[]` 用命名空间点号风格扩展
  （`#[unsafe.share]` / `#[unsafe.trm]` / `#[tag.x]`）。
- 待决：
  1. `reentrant` 的默认关闭是否过严——是否需要对「纯数据型 actor」自动开重入。
  2. `Mutex`/`RwLock` 是否凭据门禁配 `guard<share>` 进正式 API，还是保留 atomic 即可。
  3. 一期 `run` 的 1:1 线程创建：起线程的入口 ABI 如何把 tie 方法做成线程起点（复用 cb_ptr thunk）。
  4. 通用 `#[]` 属性解析器的实现：`#[macro]` 兼容 + `#[unsafe.x]` + `#[tag.x]` 三种形态的
     解析与消歧。
- goto 语义边界已定（2026-08-23，§7.1.5）：R1 同函数 / R2 同块或外层块 / R3 不越过初始化；
  与循环标签（E5 `标识符:`）命名空间分离；作用域确定性释放不受破坏。