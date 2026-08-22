# tie 并发模型设计（原生 actor + trm 协程）

> 状态：设计（i 已收敛，含语法/语义示例）
> 日期：2026-08-22
> 定位：把 tie 的并发能力定为一等公民 **actor 原生语法** + 底层 **trm 协程**。
> 取代早前「Go 共享内存 + 线程」初稿；`atomic` 部分为已实现能力，其余为设计。
> 先决依赖：trm 引擎层（M4 协程/调度/GC）。

---

## 1. 一句话总览

tie 的并发**不在语言层自造线程模型**，而是两层分工：

- **语义**：`actor` 是 tie **原生语法**（消息契约复用既有 `port`/`impl`），同步/异步用显式
  关键字，句柄 move-only — 竞态在语言层消除。
- **执行**：`actor` 编译到 trm 引擎的**协程**上；M:N 调度、GC、栈迁移、mailbox 全由
  trm 提供（路线 B），tie 层只做语义糖。

一句话：**actor 是语义，trm 是执行**。

---

## 2. 现状盘点（以代码为准）

| 能力域 | 现状 | 依据 |
| --- | --- | --- |
| 内存管理 | ✅ 表作用域确定性释放（无 GC）| 语言核心（路线 A）|
| 移动语义 | ✅ S1.3 / S1.5 smove（跨线程所有权转移底座）| docs/plans/roadmap.md |
| unsafe/ptr/slice/asm | ✅ S1.2 | docs/plans/unsafe-model.md |
| 原子类型 | ✅ `atomic<T>`（load/store/atomicrmw/cmpxchg + 内存序）| types.tie K_ATOMIC；sinfer.tie |
| port/impl 接口 | ✅（actor 消息契约复用）| 语言已支持 |
| actor / 协程 / 通道 | ✗ 无 | std/ 无对应文件 |
| trm 引擎（协程/GC/M:N）| ✗ 规划中（M4 后）| docs/plans/trm-arch.md |

> 结论：`atomic`、`unsafe`、`port`、move 均已就位；缺的正是「actor 语法 + trm 协程」两条。

---

## 3. 设计目标与约束

**目标**
- 后端服务承载、UI 响应、CPU 并行；数据竞争在**语言层**消除（actor 免锁）。
- 语义严谨、行为可预期；同步/异步显式，句柄所有权显式。

**约束**
- 保持 0-Rust 自举：actor 编译到 tieir，由 trm 执行，不引 Rust 运行时。
- 双路线：actor/协程是**路线 B（trm VM）**能力；路线 A（纯编译零依赖）无 GC，
  actor 语义可先用 1:1/协作式垫底或暂缺。
- 每期自举闭环 + regress + probe 验收；编译零错误。

---

## 4. 并发模型：原生 actor + trm 协程

**决策**：不采用「共享内存线程」为主，而是 **actor（原生语法）为语言面**；
跨 actor 共享可变内存仅作为 `unsafe` 逃生口。分层：

```
tie 应用层
│
L4  业务并发模式（actor / worker pool）        ← actor 原生语法（语言一等公民）
L3  消息契约（port/impl 复用）                  ← 方法签名即消息类型；sync/async 关键字
L2  同步原语（atomic / unsafe 共享逃生口）       ← atomic 已实现
L1  协程 / M:N 调度 / GC / mailbox              ← trm 引擎层（M4）
```

- 「共享内存」不再是一层，只是 L2 逃生口；actor 消息是唯一主数据通路。
- Erlang actor 与 Go channel 的差别由 trm 底层吸收：actor 的 mailbox 即消息通道。

---

## 5. actor 原生语法

### 5.1 基础形态

```tie
actor Counter {
    var count: i64 = 0                       // 私有状态，仅本协程读写 → 免锁

    pub sync func inc(by: i64) -> i64 {      // 同步 RPC：发消息 + 阻塞等结果
        count = count + by
        return count
    }
    pub async func reset() {                 // 异步投递：入队即返回
        count = 0
    }
}

var c = Counter.spawn()                      // 唯一句柄（move-only）
var v = c.inc(5)                             // 同步 RPC → 5
c.reset()                                    // 异步：不阻塞调用方
```

### 5.2 同步/异步 = 显式关键字

| 关键字 | 语义 | 返回值 |
| --- | --- | --- |
| `pub sync func …` | 同步 RPC：入队 + 阻塞直到 actor 应答 | 可有返回类型 |
| `pub async func …` | 异步投递：入队即返回（fire-and-forget）| 必须 `void` |

- 缺省不加 sync/async = **编译错误**（强制显式意图，避免隐含阻塞/异步行为）。

### 5.3 句柄 move-only

```tie
var a = Account.spawn()     // 唯一句柄
a.deposit(100)              // 方法调用 = 借用，句柄仍可用
var b = a                   // ✗ 复制 → 编译错误（move-only）
var b = move a              // ✓ 转移所有权（一等公民句柄，只能 move）
```

- 句柄对标闭包 env 的 move 捕获语义，复用 S1.3 移动语义；复制被编译器拒绝。

### 5.4 port 复用：消息契约外置

```tie
port Balance {
    pub func deposit(self, amt: i64) -> i64
    pub func withdraw(self, amt: i64) -> i64
}

actor Account {
    var bal: i64 = 0
}

impl Balance for Account {
    pub sync func deposit(self, amt: i64) -> i64 {
        self.bal = self.bal + amt
        return self.bal
    }
    pub sync func withdraw(self, amt: i64) -> i64 {
        if amt > self.bal { panic("overdraft") }
        self.bal = self.bal - amt
        return self.bal
    }
}

var acc: ref Balance = Account.spawn()   // port 句柄即 actor 句柄，类型安全
var b = acc.deposit(100)                 // 同步 RPC → 100（串行）
var c = acc.withdraw(40)                 // → 60
```

> `actor` ≈ 有 mailbox 的协程版 `port` 实现。方法签名即消息类型；契约能作形参、
> 接口注入，几乎零新机制。

### 5.5 语义规则（默认方案）

| 规则 | 语义 |
| --- | --- |
| 串行处理 | mailbox 单消费者，一次一条消息 → 状态免锁 |
| 调用方阻塞 | `sync` RPC 阻塞直到应答 |
| 默认不重入 | actor 阻塞在对外 RPC 时**不**处理其它入站消息；需要则标 `reentrant` |
| 失败传播 | 处理器 `panic` → 应答带失败，调用方原地 raise |
| move 边界 | 参数 move 进消息、结果 move 回；大表零拷贝所有权转移 |
| 共享逃生口 | 真跨 actor 共享可变内存走 `unsafe` + `atomic` |

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

## 6. trm 承担的部分（协程执行层）

| 职责 | 归属 | 说明 |
| --- | --- | --- |
| M:N 协程调度 | trm 引擎层 | 栈迁移 / 抢占（Go 风格）由 trm 提供 |
| GC | trm 引擎层 | 使抢占 M:N + 共享引用安全（路线 B）|
| mailbox / 消息队列 | trm 引擎层 | actor 消息通道的实现细节 |
| 底事件/定时 | trm 引擎层 | 通道阻塞 / select / 时钟挂起 |

- tie 层**不实现调度器**；`actor` 语法编译到 tieir，由 trm 执行。
- 前置：trm T2（interp 吃 tieir）→ T5（平台层）→ **M4 协程/通道**（见 trm-arch.md）。

---

## 7. 同步原语（L2 逃生口）

- `atomic<T>`：已实现，补齐常用内存序别名 `relaxed/acquire/release/acq_rel/seq_cst`。
- 非 unsafe 代码默认仅 `seq_cst`；弱序仅在 unsafe 显式标注。
- `Mutex`/`RwLock`：**不作为主路径**（主路径是 actor 消息）；仅供「unsafe 共享内存」逃生口，
  底层 bridge 到 CRITICAL_SECTION / pthread。是否进入正式 API 待定。

---

## 8. 通道 / select（弱化为底层）

- 一阶不单独暴露通用 `Chan<T>` 给业务；actor 消息即通道抽象。
- `select`/超时/多路选择若需要，作为 actor 消息 / 协程挂起的库层补充，随 trm M4 提供。

---

## 9. async/await（附：继承消息机制）

- 依赖 actor 消息 + trm 协程挂起；`pub async func` 已是投递侧异步。
- 真正的 `await` 表达式 / 协程内暂停，等 trm M4 协程就绪后叠加；一期不做栈切换语法。

---

## 10. 分期与验收

| 期 | 内容 | 类型 | 底层 | 验收 |
| --- | --- | --- | --- | --- |
| **一期（语法定稿）** | actor 语法：sync/async 关键字 + move-only 句柄 + port 契约 | 语言级 | tieir 语义（可先用 1:1/协作垫底）| actor probe 编译运行；复制句柄报错 |
| **二期（trm 协程）** | actor 编译到 trm 协程；M:N 调度 + mailbox | 系统级 | trm M4 | 多 actor 并发 probe + 状态隔离检查 |
| **三期（逃生口/沉淀）** | unsafe 共享 + atomic 补齐；若需 `select`/超时 | 库级 | trm + atomic | 竞争基准 + UI 事件驱动样例 |

- 每期独立提交、双端推送；`roadmap.md` P1 并发项按此分解。
- 依赖门槛：二期整体押在 trm M4；一期可先行把 tieir 语义与语法钉死。

---

## 11. 参考对照

| tie（actor 原生 + trm 协程）| Erlang | Go | Rust |
| --- | --- | --- | --- |
| actor 原生语法 + port 契约 | process + 消息 | goroutine + channel | `std::sync::mpsc` |
| sync/async 显式关键字 | 同步返回/异步发 | channel 显式阻塞 | 返回值/发送 |
| move-only 句柄 | process id（复制安全）| goroutine 无句柄 | `Send`/`move` |
| trm 协程（M:N+GC）| BEAM 调度 | runtime 调度 | OS 线程 1:1 |

---

## 12. 相关文档与待决问题

- 相关：docs/designs/（本文件）、docs/language-comparison.md（缺口与路线）、
  docs/plans/trm-arch.md（引擎/M4 协程）、docs/plans/roadmap.md、docs/plans/unsafe-model.md。
- 待决：
  1. `reentrant` 的默认关闭是否过严——是否需要对「纯数据型 actor」自动开重入。
  2. `unsafe` 共享是否需要正经 `Mutex` 进正式 API，还是保留 atomic 即可。
  3. actor 句柄是否要「不可 clone 但可 `=` 重新 spawn」之外的其它转移形式。