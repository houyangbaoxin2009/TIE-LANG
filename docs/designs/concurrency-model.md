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
| **三期（全凭据面）** | `guard<mem>` / `guard<ext>` 落地，覆盖 §1.2 全部 7 类 unsafe 能力 | 语言级（tiec codegen，零运行时） | 竞争基准 + UI 事件驱动样例 |

- 每期独立提交、双端推送；`roadmap.md` P1 并发项按此分解。
- **每一期 actor 执行一律纯编译（路线 A），与 trm 完全解耦**。
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
  [docs/plans/trm-arch.md](../plans/trm-arch.md)（引擎/M4 协程）、
  [docs/plans/roadmap.md](../plans/roadmap.md)、[docs/plans/unsafe-model.md](../plans/unsafe-model.md)
  （含 §13 凭据门禁设计）。
- 已收敛（2026-08-23）：actor 创建关键字=`run`；默认同步+`async`；句柄可复制+可选 move；
  消息=actor 内直声明（port 渐进）；unsafe=凭据门禁三域 mem/ext/share+委派/对象绑定/层级回收/审计。
- 待决：
  1. `reentrant` 的默认关闭是否过严——是否需要对「纯数据型 actor」自动开重入。
  2. `Mutex`/`RwLock` 是否凭据门禁配 `guard<share>` 进正式 API，还是保留 atomic 即可。
  3. 一期 `run` 的 1:1 线程创建：起线程的入口 ABI 如何把 tie 方法做成线程起点（复用 cb_ptr thunk）。