# 设计定稿：trm（tie runtime suite）最终设计

> 状态：**设计定稿**（2026-08-22 讨论对齐，2026-08-23 定稿）
> 本文档是 trm 的**唯一权威运行时设计**，取代 `docs/plans/trm-arch.md` 作为执行依据。
> 定位：**双层 + 非对称**——纯编译路线 A（保留现状，零依赖）+ trm 运行时路线 B
> （tieir 字节码 + interp 前端 + 可替换后端 + 引擎级 GC + 全 tie 平台实现）。
> 哲学：**纯编译是安全默认（actor/纯逻辑）；运行时是能力增强（GC/M:N/反射/热更/动态）**，
> 老鸟可用 unsafe 显式接入运行时。
> 决策依据：`docs/plans/trm-design-compare.md`（方案对比，此处为定稿）。
> 关联：`docs/designs/concurrency-model.md`（actor 原生语法零运行时）、
> `docs/plans/dynamic-library.md`（M5 平台桥，本文档扩展其边界）、
> `docs/plans/tieir-format.md`（tieir 字节码契据）、`docs/plans/unsafe-model.md`。

## 1. 一句话总览

**trm = tie 平台的运行时套件**。开发者不 import trm = 走纯编译路线 A（现状零依赖）；
import trm = 走路线 B，tiec 产出 tieir 字节码，由 trm 引擎执行。

```
路线 A（纯编译，保留现状，零依赖）
  tie 源码 ──tiec──▶ LLVM ──▶ 原生可执行文件        （actor/纯逻辑/编译器自身/算法库）
路线 B（trm 运行时，原创新路径）
  tie 源码 ──tiec──▶ tieir 字节码 ──▶ trm 引擎执行
    ├── interp 前端（跨端一致解释）
    ├── 可替换后端（LLVM ORC JIT 热点 | wasm/AOT 扩展 | 移动端临时替代）
    ├── 引擎级 GC 层（分代 + 移动 + 精确根扫描，管栈与对象）
    ├── M:N 协程 / async 调度（可迁移栈）
    └── 库层（系统域 + 工具集成，全 tie 写）↔ 平台实现（动态库，全 tie 写）
```

**一句话**：纯编译给所有人稳定零依赖；运行时给线路 B 的老鸟完整能力面（GC、M:N、
反射、动态加载）。两条路同源一套源码，`import` 即选择。

---

## 2. 架构分层

```
┌──────────────────────────────────────────────────────────┐
│  应用（路线 B）  import "trm:terminal" / "trm:ui" ...       │
├──────────────────────────────────────────────────────────┤
│  库层（业务能力面，全 tie 写，静态/动态混合）                │
│  ├── 纯逻辑域：terminal process fs env session clock       │
│  │             net data（语义层）→ 静态编入产物            │
│  │            + 工具集成：log compress http tieDB regex  │
│  └── 平台绑定：trm_platform.dll/.so/.dylib 动态库           │
│              （平台桥 ABI：扩展链面，见 §6.2）              │
├──────────────────────────────────────────────────────────┤
│  引擎层（tieir 执行核心，全 tie 写）                        │
│  ├── 前端 interp（跨端一致解释执行）→ InterpBackend        │
│  ├── 后端接口（Backend trait，可替换）                      │
│  │     └── LLVM ORC JIT（默认热点）| wasm/AOT | 移动临时    │
│  ├── GC 层（引擎级统一 GC，分代+移动+精确根扫描）           │
│  ├── M:N 协程 / async 调度（可迁移栈）                     │
│  ├── tieir 加载（反序列化+语法/签名校验）→ 类加载器           │
│  └── 对象模型 / 反射（运行期内省底座）                      │
├──────────────────────────────────────────────────────────┤
│  平台实现层（全 tie 写，unsafe extern + repr(C) 桥原生）     │
│  impl-win32 / impl-posix(linux) / impl-macos / impl-android│
├──────────────────────────────────────────────────────────┤
│  系统 API：Win32 / POSIX / AppKit / Android NDK            │
└──────────────────────────────────────────────────────────┘
```

**依赖方向（硬约束）**：应用 → 库层 → 引擎层（interp/后端/GC）→ 平台实现层 → 系统 API。
引擎层**不依赖库层**（引擎可独立测试/裁剪/嵌入式）；GC 是引擎层的独立子系统，
interp 与后端都作为执行面通过 GC 管理对象，统一对象身份。

---

## 3. 引擎层

**引擎层 = 执行 tieir 字节码**。只做"怎么执行"，不掺业务 API。对标 JVM 本体
（解释器 + JIT + 类加载器 + GC）。

### 3.1 前端/后端解耦（决策：前后端分离，后端可替换）

```
tieir ──▶ front-end interp（跨端一致，语义基准）
              │
              ▼
        Backend 接口（traits）
              │
  ┌───────────┼──────────────┐
  ▼           ▼              ▼
InterpBackend ORC-JIT       wasm/AOT（扩展）
（前端即后端）   （热点逐函数）   （编译期/移动端）
```

`Backend` 接口（引擎内部）：

```
Backend 接口：
  compile(module: tieir_module) -> Executable   // 字节码模块 → 可执行体
  execute(exe, entry: FuncId, args) -> Value     // 调用入口
  invoke(exe, fn: FuncId, args) -> Value          // 动态调用（沙箱/热更/反射用）
```

- **interp 也实现同接口**（`InterpBackend`）——前后端对引擎调度透明，可热切换。
- **分支策略**：Vanilla 场景 interp 直跑；热点函数提升到 JIT 后端（对标 HotSpot tiered）。
- **跨平台一致性以 interp 为基准**：语义契约 + 测试矩阵在 interp 上定基线；JIT 后端
  须通过同一契约测试（结果一致），平台差异只允许在平台桥。

### 3.2 GC 层（引擎级统一 GC，独立子系统）

**决策：GC 独立成层**——不是 interp/后端附属，而是引擎层独立子系统，管**栈与对象**。
interp 与 JIT 后端共用**同一托管堆与回收器**，单一对象身份，hot/cold 切换无搬运成本。

| 项 | 决策 |
| --- | --- |
| 根扫描 | **精确根扫描**（编译器/JIT 产栈图，interp 与后端通用） |
| 回收策略 | **分代 + 移动式**（新生代复制 / 老年代整理） |
| 协程栈 | **GC 管栈 + 对象**（M:N 可迁移栈是 GC 根来源与移动载体） |
| 与路线 A | 路线 A 无 GC（确定性释放保持现状）；GC 是**路线 B 独有能力** |

- 精确根扫描 → 可迁移栈成立 → M:N 协程成立；这是 GC 与 M:N 能够搭配的先决条件。
- 栈既是根来源也是可移动对象载体：协程迁移时 GC 把栈一起管理，避免重分配丢失根。

### 3.3 tieir 加载（类加载器）

沿用 [tieir-format.md](tieir-format.md) §7：反序列化优先 + 语法校验；可选签名校验
（发布者公钥验签）。校验失败 = 加载报错（安全底线）。`--strip` 去 span/文档段。

### 3.4 对象模型 / 反射（底座）

- 运行期类型查询、自动序列化、跨域身份——是 GC + 序列化 + 调试器 + 动态加载的公共底座。
- 依赖通过 GC 的统一对象身份来建立（P4 立稳，P2 打底）。

---

## 4. library 库层（业务能力面）

库层 = 全部业务能力。对标 JDK 类库。**全 tie 写**。

### 4.1 系统域

| 域 | 命名空间 | 内容 | 交付形态 |
| --- | --- | --- | --- |
| terminal | trm.terminal | TTY/原始模式/ANSI/键读取/光标 | 逻辑静态 + 平台板桥 |
| process | trm.process | spawn/管道流/退出码/信号 | 逻辑静态 + 平台板桥 |
| fs | trm.fs | 文件/目录（包装 std/fs，统一平台面） | 逻辑静态 + 平台板桥 |
| env | trm.env | 环境变量/平台信息/用户目录 | 逻辑静态 + 平台板桥 |
| session | trm.session | 历史/配置/profile（~/.tie/） | 逻辑静态 |
| clock | trm.clock | 时间/定时器/延时 | 逻辑静态 + 平台板桥 |
| net | trm.net | socket/HTTP（包装 std/net） | 逻辑静态 + 平台板桥 |
| data | trm.data | 文件 IO 面（zd 编解码留 std） | 逻辑静态 |
| ui | trm.ui | 窗口/绘制/事件/字体/输入 | 逻辑静态 + 平台板桥 |

### 4.2 工具集成（统一入口，不搬代码）

log/compress/http/test/tui 等：`import "trm:log"` 统一入口，底层实现仍是 ext/*。
纯逻辑工具（json/regex/zd 编解码）留 std（路线 A 也能用，不绑定 trm）。

### 4.3 混合交付（决策）

- **纯逻辑**（terminal/fs/env/net 语义层）→ **静态编译进产物**（tie 源码 import）。
- **平台实现**（impl-win32/posix/macos/android）→ **动态绑定**（平台桥动态库）。

---

## 5. M:N 协程 / async 调度

- **可迁移栈** + M:N 调度器，由 GC 一并管（§3.2）。
- **lean 设计**：async 表达式 / 方法内暂停，在路线 B 上实现（栈可迁移 → 真正可暂停）。
  actor 的 `async` 投递（路线 A 已实现）是投递侧异步；路线 B 才有「方法内暂停」的完整 await。
- 与 actor（路线 A 纯编译）的衔接见 §8。

---

## 6. 平台实现层与平台桥

### 6.1 平台实现层（全 tie 写）

```
impl-win32/   Windows（Win32：GDI/Direct2D、消息循环、进程）
impl-posix/   Linux（X11/Wayland）+ macOS（AppKit 差异点覆盖）
└── linux/  macos/
impl-android/ Android（NDK：Surface/Canvas、沙盒存储）
```

- 同名函数不同实现（`trm_ui_window_create` 四端各自）——与 JVM native method 同构。
- 全部用 tie 写：`unsafe extern` + `repr(C)` 桥原生 API，保持 0-Rust 自举灵魂。

### 6.2 平台桥 ABI（决策：扩展链面）

`trm_platform.dll / .so / .dylib / libtrm.so`（Android）——动态库主形态。

**跨库边界允许的类型（扩展 M5，见 §9 改动说明）：**

| 类型 | 备注 |
| --- | --- |
| 标量 | i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 bool trit char |
| string | tie 内部 `{len,data}` 堆表示 + `tie_free_result` 释放约定 |
| repr(C) pod struct | 仅标量/固定长数组/嵌套 pod（无指针、无变长），布局明确可序列化 |
| 带指针 struct | 需 unsafe 标记，指针不转移所有权（borrow/track 语义） |
| slice | {ptr,len} 共享缓冲，配合 unsafe 柄 |
| void | 返回 |

**约束（不变量）**：任何允许跨库的类型都须占用确定长度、布局可在四端精确复现；
带指针/slice 不得转移所有权，所有权永远留在生成侧。

---

## 7. 版本化 / 分发 / 跨平台一致

### 7.1 版本化（沿用 trm-arch §8）

```
trm@major.minor.patch：major 变 = ABI 破坏（符号改名/减删、结构布局变化）
major 绑定 ABI：应用声明 trm@^x，机器只有别的 major → 拒绝加载
符号导出带 major 后缀（多 major 共存）——第一版不做，记入后置
```

### 7.2 编译器能力绑定（min_tiec）

`tie.pkg` 声明 `version / min_tiec / abi`；编译期（import trm 时 tiec 校验自身 ≥ min_tiec）
+ 运行期（trm.init() 加载校验 abi）。哲学一致："编译期决定能力"。

### 7.3 分发

| 项 | 决策 |
| --- | --- |
| trm 形态 | 动态库延迟绑定（平台桥）+ 库层纯逻辑静态 + trm-embedded 静态子集 |
| LLVM | **随 trm 分发**（vendored 经验）；无 LLVM 环境退纯 interp |
| Android ORC | **用户写临时替代实现**（Android 端不强依赖 ORC，可退 interp 或自研轻量） |
| 加载路径 | `trm.init()` → 查应用声明版本 → `~/.tie/trm/{version}/` → 回退语言捆绑版本 → 加载+ABI 校验 |

### 7.4 跨平台一致（"结果一样"的保证）

```
1. 平台语义契约：trm.api 每函数平台无关语义定义，四端必须满足契约
2. 跨平台行为测试矩阵（CI 强制）：同一套测试跑四端（interp 为语义基准，
   JIT 通过同一契约测试 → 结果一致）
3. 平台抽象隔离：应用只触碰 trm.api（port/namespace 面）；直接调系统 API = 编译错误
   （除非显式 unsafe）
```

> 一致性以 **interp 基准 + 契约矩阵**保证，不依赖"每端 JIT 行为一致"——这是一等设计决策：
> JIT 只是 interp 语义的加速器，契约测试同一套，大幅降低跨平台风险。

---

## 8. 双路线与 actor 衔接（决策：unsafe 可选接 trm）

| 对象 | 默认（安全） | 老鸟（unsafe 显式） |
| --- | --- | --- |
| actor | **路线 A 纯编译**（现状已收官，1:1 线程 + mailbox 零运行时） | **unsafe 显式接入路线 B（trm）** |
| 纯逻辑程序 | 路线 A 纯编译零依赖 | 路线 B（需要 GC/M:N/反射时） |
| 系统/UI 应用 | import trm → 路线 B | 同左 |

- **actor 不被强制接 trm**：保持「actor 原生语法 · 零运行时」的安全默认。
- **老鸟可用 unsafe 显式让 actor 走路线 B（trm）**：就能用上 M:N 协程、GC 对象模型、
  动态加载等运行时能力——语法零改动，仅接入机制不同。
- 由此，concurrency-model §6「actor 与 trm 解耦」的语义被放宽为：
  **默认解耦（路线 A），unsafe 显式接入（路线 B）**。

---

## 9. 对既有决策的改动点

| 原有文档 | 本定稿的改动 |
| --- | --- |
| trm-arch.md | 整体重定向：JIT 从"唯一执行"降为**可替换后端之一**；GC 提升为**引擎级独立层**；跨平台一致从"依赖每端 JIT"改为"interp 基准 + 契约矩阵" |
| dynamic-library.md（M5） | **扩展边界**：从"仅标量+string"扩展为"标量 + string + repr(C) pod struct + 带指针 struct + slice"（含 unsafe/所有权约束）——需更新 regress-m5-dynlib 边界负例为正例 |
| concurrency-model.md §6 | "actor 完全解耦"收窄为"默认解耦，unsafe 可选接入路线 B" |

---

## 10. 能力里程碑（分期）

| 阶段 | 内容 | 依赖 | 验收 |
| --- | --- | --- | --- |
| **P0** | tieir 加载 + 校验 + interp 执行（跨端解释） | tieir_ser 已就绪 | **纯字节码验收**：tieir 加载+校验+interp 跑通一个纯函数 |
| **P1** | 库层起步（terminal/process/fs/env/session）+ 平台桥（扩展链面）+ 动态库集成 | P0 + M5 + 本定稿 §6.2 边界扩展 | `import "trm:terminal"` → tieir → interp 执行 + 平台桥动态加载 |
| **P2** | 引擎级 GC（分代+移动+精确根扫描） + M:N 协程 + 可迁移栈 | P0 | GC 探针 + 协程调度 + 根扫描正确 |
| **P3** | ORC JIT 后端（热点逐函数，对标 HotSpot tiered） | P0 + 栈图（P2） | 热点函数提升 JIT，契约测试与 interp 一致 |
| **P4** | 反射/内省 + 动态加载/热更 + 沙箱/校验强化 + 诊断 | P2（统一对象身份） | 动态加载 tieir + 类型内省 + 诊断 |
| **P5** | 平台扩展细化（impl-posix/macos/android 深入）+ wasm/AOT 可选后端 | P0-P4 | 四端契约矩阵全绿 |

> P0→P1 线性（先字节码执行）；P1 库层可与 P2 并行（库层多用标量+句柄，不强依赖 GC）；
> P3 若成本过高可后置（interp 已是可用形态）；每个阶段独立提交、双端推送、`roadmap.md` 关联段分解。

---

## 11. 决策记录（定稿）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 整体 | 双层 + 非对称（路线 A 纯编译保留 + 线路 B 运行时） | trm 强制、纯编译唯一 |
| 运行时定位 | 运行时 = 能力增强（GC/M:N/反射/热更/动态），非唯一路径 | — |
| 引擎层 | **interp 前端 + 可替换后端**（InterpBackend / ORC-JIT / wasm-AOT） | 固定 JIT、纯 JIT、纯 interp |
| 后端迁移粒度 | **热点逐函数**（HotSpot tiered） | 全模块预编译、显式标记 |
| GC | **引擎级统一 GC，独立层**，interp/JIT 共用堆 | interp 内建、无 GC |
| 根扫描 | **精确根扫描**（interp/JIT 通用栈图） | 保守扫描、混合 |
| 回收策略 | **分代 + 移动式** | mark-sweep 起步 |
| 协程栈 | **GC 管栈 + 对象** | 协程调度自管栈 |
| 库层 | **逻辑静态 + 平台动态混合**，全 tie 写 | 全静态、全动态 |
| 平台桥 ABI | **扩展链面**：标量 + string + repr(C) pod struct + 带指针 struct + slice（unsafe/所有权约束） | 仅标量+string、暂不定义 |
| 实现语言 | **全 tie 写**（库层 + 平台桥 + 引擎 core） | 引入 native、C 平台桥 |
| actor 衔接 | **默认解耦（路线 A）+ unsafe 可选接入路线 B（trm）** | 强制接入、彻底解耦 |
| 跨平台一致 | **interp 基准 + 契约矩阵**（JIT 通过同一测试） | 依赖每端 JIT |
| LLVM | 随包分发；Android 用临时替代；无 LLVM 退 interp | 系统 LLVM、四端必须 ORC |
| 版本化 | 语义化 ABI + min_tiec + 版本目录 + ~/.tie/trm/{v}/ | 随语言版本 |
| 本期交付 | 最终设计文档（本文件） | 方案对比即停 |

---

## 12. 未决 / 后续细化

1. **精确根扫描的栈图实现**：interp 与后端共用一套栈图还是各自产（定稿倾向共用，待 P2 定）。
2. **Android 临时替代**：用户自研轻量执行路径 vs 纯 interp 退化（P5 定）。
3. **反射能力范围**：只读类型查询 vs 运行动态调用（invoke）——P4 定。
4. **wasm/AOT 后端**：是否正式立项（P5，编译期 AOT 替 webui 场景）。
5. **带指针 struct / slice 跨库的所有权模型细化**：与 unsafe-model guard<ext> 对齐（P1 平台桥定）。
6. **是否升级 trm-arch.md**：本定稿为权威，trm-arch 是否降为规划史/删除待定（用户决定）。

---

## 13. 相关文档

- 方案对比（决策依据）：`docs/plans/trm-design-compare.md`
- 规划稿：`docs/plans/trm-arch.md`（被本文档取代，待归档）
- actor 零运行时：`docs/designs/concurrency-model.md`
- 动态库边界（被扩展）：`docs/plans/dynamic-library.md`
- tieir 字节码：`docs/plans/tieir-format.md`
- unsafe 凭据门禁：`docs/plans/unsafe-model.md`
- 实施路线图：`docs/plans/roadmap.md`（S4.1 trm）