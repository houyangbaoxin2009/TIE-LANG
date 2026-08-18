# 规划：trm（tie 运行时套件）——两层架构（引擎层 + 库层）

> 状态：**规划**（2026-08-15 初稿，2026-08-18 修订定稿，未实现）
> 本文档定义 trm（tie runtime suite）——tie 生态的**单一运行时套件**，
> 采用 **JVM 式设计思路**，分两层：
> **引擎层**（tieir 字节码 VM：interp + LLVM ORC JIT）与
> **库层**（常用函数封装 + 工具集成）。
> 一次编译、四端运行（Windows/Linux/macOS/Android），结果一致。
> **吸收 tucore**：原 tucore 降为 trm 库层的 ui 域（trm.ui）。
>
> 决策汇总（2026-08-18 修订）：
> **两层架构**（引擎层：解释器 + JIT；库层：系统封装 + 工具集成）
> + **JVM 式**（统一 API + tieir 字节码 VM + 平台实现层）
> + **tieir 字节码**（interp 启动 + LLVM ORC JIT 热点，混合执行）
> + **四端平台层**（impl-win32 / impl-posix / impl-android）
> + **双路线**（开发者可选：trm VM 或 纯编译零依赖）
> + **随包分发**（LLVM 随 trm 分发）
> + **跨平台一致性**（语义契约 + 测试矩阵）
> + **版本化**（独立版本 + 捆绑默认 + min_tiec 兜底）
> + **session 域**（通用化 + 两级区分）
> 对标：JVM（HotSpot 引擎 + JDK 类库）、.NET（CLR + BCL）。
> 关联：tieconsole（终端/进程/会话）、tiu（ui 域消费方）、unsafe 模型
> （extern/repr(C)）、包模型（动态库 M5）、序列化规范（zd）、
> tieir-format.md（tieir 格式）。

## 1. 定位

**trm = tie 平台运行时**：两层架构，服务所有 tie 组件：

```
┌─ tieconsole ──┐  ┌─ tiu ──┐  ┌─ webui ──┐
└──────┬────────┘  └───┬────┘  └────┬─────┘
       └─────── trm（两层）──────────┘
                │
   ┌────────────▼────────────────────────────┐
   │  库层（封装常用函数 + 集成各种工具）        │
   │  ├── 系统域：terminal/process/fs/env/    │
   │  │          session/clock/net/data/ui    │
   │  └── 工具：zd/json/log/tieDB/compress/   │
   │           regex/http/test/...            │
   ├──────────────────────────────────────────┤
   │  引擎层（JIT + 解释器）                   │
   │  ├── interp（解释器，启动/嵌入式）        │
   │  ├── LLVM ORC JIT（热点编译）             │
   │  └── 平台实现层（impl-win32/posix/android）│
   └──────────────────────────────────────────┘
                ↓ extern（unsafe/ptr/repr(C)）
        Win32 API / POSIX / Android NDK API / 帧缓冲
```

**两层一句话**：引擎层管"怎么执行代码"（tieir → 机器码），
库层管"提供什么能力"（系统 API + 现成工具）。对标 JDK = JVM（引擎）
+ java.* 类库（库）。

### 1.1 JVM 式设计（2026-08-18 定稿）

| JVM 概念 | trm 对应 | 层 |
| --- | --- | --- |
| 字节码（.class） | tieir 字节码 | 分发物 |
| HotSpot 解释器 / JIT | interp（启动/嵌入式）+ LLVM ORC JIT | **引擎层** |
| 类加载器 + 验证 | tieir 反序列化 + 校验 + 签名 | 引擎层 |
| JNI / native 方法 | extern → 平台实现 | 引擎层 |
| Java 标准库（JCL） | trm 库层（系统域 + 工具） | **库层** |
| JDK 附带工具（javac/jar/...） | tie 工具链（tiec/pkg/...） | 库层外 |
| 一次编写到处运行 | 一次编译四端运行 | 整体 |

**"结果一样"的强保证**：同一份 tieir 字节码 + 平台无关的引擎核心 →
执行语义天然一致；平台差异只剩 extern 实现，由**平台语义契约 + 跨平台
测试矩阵**兜底（见 §6）。

### 1.2 双路线：开发者自由选择（关键约束）

**trm 是可选依赖（opt-in）**：

```
路线 A：纯编译（现状保持，零依赖）
  纯 tie 源码 ──tiec──▶ LLVM ──▶ 原生可执行文件（不链接 trm）
  · 纯逻辑程序、编译器自身（tiec）、rdu、纯算法库——默认此路线

路线 B：trm VM（系统/UI 应用主路径）
  tie 源码 ──tiec──▶ tieir 字节码 ──▶ trm 引擎执行 + 库层 API
  · 需要系统能力（终端/UI/进程流/文件/环境）时选择
```

- **import 即选择**：源码 `import "trm:terminal"` → 路线 B；
  不 import trm → 路线 A（产物与现状完全一致）
- **三模式并存**：

| 模式 | 路径 | 产物 | 用途 |
| --- | --- | --- | --- |
| A 纯编译（保留） | tiec → LLVM → 原生 | 单文件 exe | 纯逻辑/CLI/编译器自身 |
| B trm VM（主路径） | tiec → tieir → trm 引擎 | tieir + trm | 跨平台 UI/系统应用 |
| C 混合（后置） | tieir → 引擎 → JIT/内联 | 字节码 + 热点原生 | 性能敏感（后置） |

三模式同一套源码——编译目标决定形态（与 tie:xxx 角色哲学一致）。

## 2. 引擎层（JIT + 解释器层）

### 2.1 职责

**引擎层 = 执行 tieir 字节码**。只做"怎么执行"，不掺业务 API。
对标 JVM 本体（解释器 + JIT + 平台桥）。

```
引擎层
├── interp          解释器（T4 已 tie 化，工程化吃 tieir）
├── LLVM ORC JIT    热点编译（复用 llvmgen + LLVMParseIR）
├── 混合调度        解释启动 → 热点提升 JIT（HotSpot tiered 思路）
├── tieir 加载      反序列化 + 语法校验 + 签名校验（tieir-format §7/§9）
└── 平台实现层      impl-win32 / impl-posix / impl-android（extern 封装）
```

### 2.2 执行策略：混合（interp 启动 + ORC JIT 热点）

```
tieir 字节码
  ├── interp（解释执行）      → 启动快，嵌入式/无 LLVM 环境执行器
  └── LLVM ORC JIT（热点）    → 峰值性能 ≈ 原生，函数级 lazy compile
```

- **复用现有后端**：irgen/llvmgen（tie 写，已打通 tie IR → LLVM IR）——
  引擎直接调用，一行不改。llvmgen 现输出 .ll 文本喂 clang；JIT 路线把
  "喂 clang" 换成 "LLVMParseIR + ORC JIT 执行"。
- **混合调度**（HotSpot tiered 思路）：先解释启动 → 热点函数（计数/采样）
  提升到 ORC JIT → 冷函数可降回解释。
- **函数级 lazy compile**：ORC 原生支持按需编译，冷函数不编译、零开销。
- **interp 的双重价值**：快速启动 + 嵌入式执行器（无 LLVM 环境）。
- **性能预期**：解释器约 10-50x 慢于原生（可接受：UI 瓶颈在绘制）；
  JIT ≈ 原生。性能关键路径可 unsafe extern 下沉到平台实现（应用自选）。

### 2.3 加载流程

```
trm.init()
  → 版本/ABI 校验（§8）
  → 加载 tieir（反序列化 + 语法校验 + 签名校验）
  → interp 解释执行入口函数（启动）
  → 热点检测 → ORC JIT 编译 → 切换执行
```

### 2.4 平台实现层（引擎层底部）

```
trm/impl/
├── impl-win32/      Windows（Win32 API：GDI/Direct2D、消息循环、进程）
├── impl-posix/      Linux + macOS 共用 POSIX 层
│   ├── linux/       X11/Wayland（M6 渐进）
│   └── macos/       AppKit/CoreGraphics（差异点覆盖）
└── impl-android/    Android（NDK：Surface/Canvas、沙盒存储）
```

- **平台实现 = 同名函数不同实现**（`trm_ui_window_create` 四端各自实现）——
  与 JVM 的 native method 同构
- **macOS 与 Linux 共用 POSIX 层** + 各自覆盖差异点
- **目录分离**（P2 决策）：平台代码隔离，发布按平台打包
- **动态库形态**：trm.dll / trm.so / trm.dylib / libtrm.so（Android）
- **webui（wasm）**：编译期 AOT（tieir → wasm），浏览器内执行——
  平台实现层的"第五形态"，走编译期而非运行时 JIT

## 3. 库层（常用函数封装 + 工具集成）

### 3.1 职责

**库层 = 全部业务能力面**。封装系统 API、集成现成工具，
开发者 import 即用。对标 JDK 类库（java.lang/java.util/java.io/...）。
**库层依赖引擎层**（库代码跑在 VM 上），引擎层不依赖库层。

### 3.2 系统域（平台 API 封装）

| 域 | 命名空间 | 内容 | 服务对象 |
| --- | --- | --- | --- |
| terminal | trm.terminal | TTY 检测/原始模式/ANSI/键读取/光标 | tieconsole |
| process | trm.process | spawn/管道流（stdin/stdout）/退出码/信号 | tieconsole/通用 |
| fs | trm.fs | 文件/目录（**包装 std/fs**，统一平台面） | 通用 |
| env | trm.env | 环境变量/平台信息/用户目录 | tieconsole/通用 |
| session | trm.session | 历史/配置/profile（~/.tie/） | 通用（见 §9） |
| clock | trm.clock | 时间/定时器/延时 | 通用 |
| net | trm.net | socket/HTTP（**包装 std/net**，统一平台面） | 通用 |
| data | trm.data | **只做文件 IO 面**（zd 编解码留 std） | 通用 |
| ui | trm.ui | 窗口/绘制/事件/字体/输入（原 tucore 全部） | tiu |

> **fs/net 路径（2026-08-18 定）**：**包装而非移动**——std/fs、std/net
> 保留（纯逻辑部分），trm.fs/trm.net 是统一平台面（包装 std + 平台实现）。

### 3.3 工具集成（现成能力，开箱即用）

库层不仅封装系统 API，还**集成各类工具**为统一入口——对标 JDK 类库的
丰富度（java.util/json/net/http/...）：

| 工具 | 现状基础 | 集成形态 |
| --- | --- | --- |
| zd 序列化 | std（tieDB/persist/zd.tie） | 纯逻辑留 std，data 域包文件 IO 面 |
| json | std/json.tie | 纯逻辑留 std |
| log 日志 | ext/log.tie | 升级统一面（级别/输出/格式） |
| tieDB 数据库 | tieDB/ | 独立于 trm，库层提供访问 API |
| compress 压缩 | ext/compress.tie（zstd/brotli） | 保留 ext，库层入口 |
| regex 正则 | std/regex.tie | 纯逻辑留 std |
| http 客户端 | std/http.tie | 包装为 trm.net 的 http 子面 |
| test 测试 | ext/test.tie | 保留 ext，库层入口 |
| tui 终端 UI | ext/tui.tie | 演进为 trm.ui 的前身/验证场 |

**集成原则**：
- **纯逻辑（无平台依赖）** → std/ext 保留（路线 A 也能用，不绑定 trm）
- **有平台依赖** → 库层统一面（trm.xxx 包装）
- **工具以"库层入口"形态集成**：不搬代码，提供统一 import 面——
  开发者 `import "trm:log"` 拿到 log，底层实现仍是 ext/log.tie

### 3.4 命名空间组织

```tie
namespace trm {
    namespace terminal {
        pub func is_tty(fd: i64) -> bool
        pub func raw_mode(on: bool)
        pub func read_key() -> Key
    }
    namespace process {
        pub func spawn(cmd: string, args: table<string>, pipes: bool) -> Process
        pub func read_stdout(p: Process) -> string
    }
    namespace ui {
        // 原 tucore 全部（窗口/绘制/事件/字体/输入，见 §5）
    }
}
```

- 调用形态：`trm.terminal.is_tty(0)` / `trm.process.spawn(...)` / `trm.ui.window_create(...)`
- 域内方法链：`w.show().resize(..)`（句柄方法，H2 类型化句柄）

## 4. 两层关系

```
应用（路线 B）
  │ import
  ▼
库层（系统域 + 工具集成）      ← 业务 API，依赖引擎
  │ 内部实现：纯 tie 逻辑 / 包装 std / extern 平台桥
  ▼
引擎层（interp + ORC JIT）    ← 执行核心，依赖平台实现
  │ extern
  ▼
系统 API（Win32/POSIX/Android）
```

| 项 | 引擎层 | 库层 |
| --- | --- | --- |
| 职责 | 执行 tieir 字节码 | 提供业务能力 |
| 依赖 | 平台实现（impl-*） | 引擎层 + std/ext |
| 独立演进 | JIT/解释器优化 | 新 API/新工具 |
| 裁剪 | 嵌入式可减 JIT | 按域 import 裁剪 |
| 对标 | JVM 本体 | JDK 类库 |

**依赖方向（硬约束）**：应用 → 库层 → 引擎层 → 平台。引擎层绝不依赖
库层——保证引擎独立、可单独测试、可单独裁剪（嵌入式只要引擎+精简域）。

## 5. ui 域（原 tucore 吸收，决策保留）

原 tucore-arch.md 决策全部保留，命名迁移 + Android 补充：

| 原 tucore 决策 | trm.ui 对应 |
| --- | --- |
| A4 抽象 API + Win32 起步 | trm.ui.api（port 声明）+ trm/ui/impl-*/ |
| H2 类型化句柄 | struct Window/Font/PaintCmd 包装 i64 |
| E3 事件 + 信号混合 | event_drain + signal_check |
| D2 命令列表 | paint_begin/paint_rect/paint_end（**绘制契约**，四端共用） |
| F3 系统字体 + 位图 | font_load_system/font_load_bitmap |
| P2 目录分离 | impl-win32/ impl-posix/ impl-android/ |
| L1 显式生命周期 | trm.init() / trm.shutdown() |
| 组合式开发 | 保留（组件/行为/布局/模块四层） |
| JVM/.NET 借鉴 | 保留（P/Invoke/延迟绑定/元数据） |

**Android 补充**：
- trm.ui 命令列表 → Android Canvas（1:1 映射，与 webui Canvas 桥同构）
  ——**绘制抽象一次设计，桌面/Android/web 三端复用**
- 事件契约 → MotionEvent 归一化（坐标/键码/触摸）
- Activity 生命周期（onPause/onResume）→ 信号（挂起/恢复）+ 显式 init/shutdown

> **绘制契约与事件契约是一等公民**：4 个消费端（Win32/X11/macOS/Android
> Canvas），命令列表纯数据、可序列化、可 diff。

## 6. 跨平台一致性（"结果一样"的保证）

```
1. 平台语义契约（契约文档 + 注释）
   trm.api 每个函数带平台无关语义定义（输入/输出/边界/错误），
   四端实现必须满足契约

2. 跨平台行为测试套件（CI 强制）
   同一套测试跑四端（Win/macOS/Linux 本机，Android 模拟器）：
   - 结果断言：同一输入 → 同一输出
   - 视觉基线：trm.ui 绘制命令 dump 对比（命令列表纯数据，可 diff）
   - 契约测试：边界输入（空串/越界/并发）四端行为一致

3. 平台抽象隔离（编译期保证）
   应用代码只允许触碰 trm.api（port/namespace 面）——
   直接调系统 API = 编译错误（除非显式 unsafe 自证）
```

**测试先行是 JVM 思路的骨**：没有跨平台测试矩阵，"结果一样"就是空话。
跨平台测试矩阵是 trm 的**一等里程碑**（与核心实现同步，不后置）。

## 7. 与 std/ext 的分工

| 层 | 内容 | 平台依赖 |
| --- | --- | --- |
| trm 库层 | 系统 API 门面 + 工具统一入口 | 有（平台实现） |
| std | 纯逻辑库（string/utf/sort/json/regex/math/... + zd 编解码） | 无（纯 tie） |
| ext | 扩展工具（log/compress/test/tui/...） | 大多无 |
| rdu | 嵌入式基础（bits/math/ascii/crc/fixed/rnd） | 无（标量） |

- **data 域（2026-08-18 定）**：zd 编解码（纯逻辑）留 std；
  trm.data 只管文件 IO 面
- **fs/net（2026-08-18 定）**：std 保留，trm 统一平台面（包装）
- **工具集成（2026-08-18 定）**：不搬代码，库层提供统一 import 入口
- **双路线保留**：不 import trm 的程序 = 现状产物（零依赖 exe）

## 8. 版本化（2026-08-18 定稿）

### 8.1 版本号策略：语义化 + ABI 绑定

```
trm@major.minor.patch
  major 变   = ABI 破坏（导出符号改名/删减、结构体布局变化）
  minor 变   = 向后兼容新增（新符号、新域）
  patch 变   = bug 修复（符号不变）
```

- major 绑定 ABI：应用链接 trm@2.x，机器上只有 trm@3.x → 拒绝加载
- 符号导出带 major 后缀（多 major 共存）——**第一版不做**，记入后置

### 8.2 编译器能力绑定：min_tiec

```tie
// tie.pkg（trm 包元数据）
package trm {
    version:  "2.1.0"
    min_tiec: ">= 0.20.0"        // 要求编译器能力（unsafe/ptr/repr(C)/协程）
    abi:      "2"                // ABI major（与 version.major 一致）
}
```

- **检查时机**：编译期（import trm 时 tiec 校验自身 ≥ min_tiec）；
  运行期（trm.init() 加载 DLL 时校验 abi）
- **哲学一致**："编译期决定能力"

### 8.3 与语言版本：独立版本 + 捆绑默认

| 场景 | 机制 |
| --- | --- |
| 语言发布 | 捆绑默认 trm 版本（`--trm-version` 缺省用它） |
| 应用显式指定 | tie.pkg 声明 `depends: [trm@^2.1]`，MVS（S3.2）解析 |
| 机器多版本共存 | `~/.tie/trm/{version}/trm.dll` 版本目录 |
| 语言大版本升级 | min_tiec 兜底——锁 trm@2.x 也安全（ABI 不变） |

### 8.4 加载路径

```
trm.init()
  → 查应用声明版本（tie.pkg / --trm-version）
  → 查 ~/.tie/trm/{version}/trm.dll（版本目录）
  → 缺省版本没装 → 回退语言安装目录的捆绑版本
  → 加载 + ABI 校验 → 符号解析（延迟绑定）
```

### 8.5 依赖分发：随包（2026-08-18 定稿）

- **LLVM 随 trm 分发**（~20-40MB）——复用 vendored LLVM 经验
  （tie-llvm-vendored-dist 已完成）
- 零依赖（应用不用装 LLVM）、版本绑定（编译器已验证 22.1.8）
- 许可：LLVM Apache 2.0，可随 trm 分发，无 GPL 污染

## 9. session 域范围（2026-08-18 定稿）

### 9.1 两级状态区分

```
trm.session 管两类东西，机制完全不同：
├── 用户级（跨进程、跨会话持久）→ ~/.tie/ 或平台配置目录
│     配置 / profile / 历史 / 别名 / 凭据引用
└── 会话级（单进程、运行期）    → 内存 + 可选临时文件
      当前会话环境 / 临时状态 / 未提交修改
```

**规则**：持久的东西才叫 session；运行期临时状态归应用自己，
session 域不做"全局变量存储"。

### 9.2 用户级存储（~/.tie/）

```
~/.tie/
├── config.tiedata        # 全局配置（人读，tie:data）
├── profiles/             # profile（环境预设）
│   └── work.tiedata
├── history/              # 命令历史（按工具分，zd 紧凑格式）
│   ├── tieconsole.zd
│   └── pkg.zd
├── aliases.tiedata       # 别名
├── trm/                  # trm 运行时自身（版本目录，§8）
│   └── 2.1.0/trm.dll
└── cache/                # 可丢弃缓存
```

### 9.3 并发与一致性

- 配置类 → 原子写（临时文件 + rename），读优先
- 历史类 → 追加写 + 文件锁（第一版：单进程追加约定；第二版：跨进程锁）

### 9.4 API 面

```tie
namespace trm {
    namespace session {
        pub func home() -> string                          // ~/.tie/
        pub func path(kind: PathKind, name: string) -> string
        pub func set_home(p: string)                       // 覆盖（测试/便携）
        pub func config_get(key: string) -> Value
        pub func config_set(key: string, v: Value)
        pub func history_append(app: string, entry: zd)
        pub func history_read(app: string) -> table<zd>
        pub func profile_list() -> table<string>
        pub func profile_load(name: string) -> table<Value>
    }
}
```

### 9.5 边界（session 不做什么）

| 不做 | 理由 |
| --- | --- |
| 不做全局变量存储 | 应用运行态归应用自己，session 只管持久 |
| 不做凭据明文存储 | 凭据引用（系统钥匙串/环境变量），不落盘明文 |
| 不做 tieDB 引擎 | session 管"位置与格式约定"，tieDB 管"引擎" |
| 不锁死路径 | set_home 覆盖 + 平台标准目录探测 |

### 9.6 与版本化的咬合

`~/.tie/trm/{version}/` 归 session 域管路径、trm 管内容——单一数据源。

## 10. 形态

### 10.1 主形态：动态库（延迟绑定）

```
trm.dll / trm.so / trm.dylib / libtrm.so（Android）——延迟绑定
  ├── 库层（系统域 + 工具集成）
  └── 引擎层（interp + ORC JIT，随包 LLVM + 平台实现）
```

- **动态库为默认**：extern 符号运行时解析（LoadLibrary/dlopen），
  支持 P4b 实现选择运行时分发
- 前置：M5 动态库编译能力（docs/plans/dynamic-library.md 规划中）

### 10.2 嵌入式子集：trm-embedded

```
trm-embedded（静态链接，无动态库，interp 执行）
  ├── 库层：terminal/process 无（无 OS）；fs → 帧缓冲/闪存抽象；
  │         ui → 帧缓冲直绘；clock/data 精简
  └── 引擎层：interp（无 LLVM，无 JIT）
```

- 与 tie:embedded 角色咬合（编译期裁剪：禁用 spawn/终端/JIT）
- 无动态库环境：静态链接（P4b 编译期选择）

## 11. 编译器实现拆解

| 模块 | 改动 |
| --- | --- |
| unsafe 扩展 | extern ptr/repr(C)（M0，引擎/平台实现全部依赖） |
| tieir 加载 | 反序列化器 + 语法校验（tieir-format §9） |
| interp 工程化 | 现有 interp 吃 tieir 字节码（T4 基础 + 加载器） |
| ORC JIT | LLVM ORC 接入（tieir → llvmgen → LLVMParseIR → JIT） |
| 库层 api | 域命名空间 + port 声明（system/ui 抽象面） |
| 工具集成 | ext/log、ext/compress 等统一入口 |
| 平台实现 | impl-win32 / impl-posix / impl-android（extern 封装） |
| 动态库 | M5 动态库编译（trm.dll/trm.so） |
| 随包 LLVM | vendored LLVM 分发（复用既有经验） |
| 测试矩阵 | 四端契约测试 + 视觉基线（CI） |
| 迁移 | tucore 命名 → trm.ui（无用户，直接改名） |

## 12. 里程碑

| 阶段 | 内容 | 依赖 |
| --- | --- | --- |
| T0 | 语言地基：unsafe + ptr + repr(C) + extern 扩展 | tiec 现有链路 |
| T1 | tieir 加载：反序列化器 + 校验 | T0 |
| T2 | interp 吃 tieir：现有 interp 工程化（先跑通字节码执行） | T1 |
| T3 | ORC JIT 接入：llvmgen → LLVMParseIR → JIT（性能） | T2 |
| T4 | 混合策略：interp 启动 + JIT 热点调度调优 | T3 |
| T5 | 平台实现：impl-win32（terminal/process/fs/ui 起步） | T0 + T4 |
| T6 | 库层 api：统一面 + port 声明 + 语义契约文档 | T5 |
| T7 | 跨平台测试矩阵（四端 CI + 视觉基线） | T5+T6 同步 |
| T8 | 版本化落地（版本目录 + abi 校验 + min_tiec） | T5 |
| T9 | session 域 + 工具集成（log/compress 入口）+ 随包 LLVM | T5 |
| T10 | 平台扩展：impl-posix（Linux/macOS）→ impl-android | T5 后渐进 |

> T2/T3 串行（interp 先通、JIT 后上）；T5-T9 并行；T10 渐进。

## 13. 决策记录（2026-08-18 修订）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| **总体架构** | **两层**：引擎层（JIT+解释器）/ 库层（系统封装+工具集成） | 单层统一面 |
| 引擎层 | interp + LLVM ORC JIT（混合执行） | 纯解释、纯 JIT |
| 库层 | 系统域 9 个 + 工具集成（log/compress/http/test/...） | 仅系统 API |
| JVM 式 | 统一 API + tieir 字节码 VM + 平台实现层 | 纯 API 门面 |
| 字节码 | tieir（既有格式，执行器新增） | 新造 VM 字节码 |
| JIT 引擎 | LLVM ORC（复用 llvmgen，LLVMParseIR） | 自研 JIT、Wasmtime |
| LLVM 分发 | 随包（vendored 经验） | 系统 LLVM |
| 平台 | 四端：Win32 / POSIX（Linux+macOS 共用）/ Android | 单平台起步 |
| 平台组织 | impl-win32 / impl-posix（linux+macos 覆盖）/ impl-android | macOS 单列 |
| 双路线 | 保留纯编译（开发者可选：trm VM 或零依赖 exe） | trm 强制 |
| 跨平台一致 | 语义契约 + 测试矩阵（一等里程碑）+ 平台抽象隔离 | 仅文档约定 |
| 与 tucore | D：trm 吸收 tucore（tucore → trm.ui） | A/B/C |
| fs/net 路径 | 包装（std 保留，trm 统一平台面） | 移动 |
| data 域 | zd 编解码留 std，trm.data 只管文件 IO 面 | 全移 trm |
| 工具集成 | 不搬代码，库层统一 import 入口 | 代码迁移进 trm |
| 形态 | 动态库为默认（延迟绑定）+ trm-embedded 静态子集 | 纯静态 |
| 版本化 | 独立版本 + 捆绑默认 + min_tiec + abi 校验 | 随语言版本 |
| session 域 | 通用化 + 两级区分（用户级持久/会话级不存） | 仅 tieconsole |
| 命名空间 | trm 顶级 + 嵌套域（trm.terminal/trm.ui） | tucore 保留 |
| 与 std | trm = 平台门面，std = 纯逻辑 | 合并 std |

## 14. 未决问题

1. **ORC JIT 跨平台验证**：Android（ARM64）可行性需实验确认
   （NDK 自带 LLVM，理论可行，T3 前验证）
2. **JIT 热点检测策略**：计数 vs 采样——T4 调优时定
3. **多 major 共存**：符号导出带 major 后缀（第一版不做，后置）
4. **跨进程锁**：session 历史并发写（第二版）
5. **tieir 执行语义完整度**：interp 对 tieir 的覆盖范围——T2 时盘点
6. **webui AOT 细化**：tieir → wasm 编译期路径 vs 运行时 interp——M5 定
7. **工具集成边界**：ext 工具哪些升级进库层入口（log/compress 首批，
   其余按需）——随 T9 渐进

## 15. 与 tiu 的关系

| 项 | 关系 |
| --- | --- |
| tiu Backend 模块 | = trm.ui 抽象面（port Backend 实现），四端各有实现 |
| tiu 绘制契约 | trm.ui 命令列表（paint_begin/rect/end）——tiu paint 走命令列表 |
| tiu 事件 | trm.ui event_drain（事件队列 + 信号），tiu 冒泡/重建消费 |
| tiu 生命周期 | trm.init()/shutdown()（L1 显式），tiu app.run 内调用 |
| tiu 主题持久化 | trm.session 的 config/tie:data（主题 token 可落盘） |
| tiu 跨线程 | trm.clock 定时器信号 → 动画驱动；协程/channel（M4 后） |
| tiu 分发 | tieir 字节码 + trm 引擎（路线 B）——tiu 应用默认走路线 B |
