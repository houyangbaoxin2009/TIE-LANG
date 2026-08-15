# 规划：trm（tie 运行时套件）——统一系统 API + UI 底层

> 状态：**规划**（2026-08-15 设计讨论定稿，未实现）
> 本文档定义 trm（tie runtime suite）——tie 生态的**单一运行时套件**，
> 跨平台统一系统 API。**吸收 tucore**：原 tucore 降为 trm 的 ui 域。
> 决策汇总：
> **D 方案**（trm 吸收 tucore，单一运行时套件）
> + **域划分**（system 域 + ui 域）+ **命名空间 trm**（嵌套域命名空间）
> + **形态**（动态库延迟绑定 + 嵌入式子集 trm-embedded）。
> 对标：.NET System 命名空间（BCL）、JVM 平台（标准 API 面）。
> 关联：tieconsole（终端/进程/会话）、tieui（ui 域）、unsafe 模型
> （extern/repr(C)）、包模型（动态库 M5）、序列化规范（zd）。

## 1. 定位

**trm = tie 运行时套件**：全部系统能力的统一 API 面，跨平台一致
（Windows/Linux/macOS/嵌入式），服务所有 tie 组件：

```
┌─ tieconsole ──┐  ┌─ tieui ──┐  ┌─ webui ──┐
└──────┬────────┘  └────┬─────┘  └────┬─────┘
       └─────── trm（统一 API 面）────┘
                ├── system 域（终端/进程/文件/环境/会话/时钟/网络）
                └── ui 域（窗口/绘制/事件/字体/输入）← 原 tucore
                ↓ extern（扩展后：ptr/repr(C)）
                Win32 / POSIX / 帧缓冲
```

- 对标 .NET System（BCL）：一个命名空间管一切系统能力
- tieconsole 用 system 域；tieui 用 ui 域；共享底层 extern
- **tucore 名称弃用**：命名空间统一为 trm（迁移：tucore → trm.ui，无用户零负担）

## 1.1 直接编译保留（关键约束：trm 是可选层，非强制运行时）

**trm 不改变 tie 的直接编译能力**——引入 trm 后并未放弃直接编译：

```
模式 A：直接编译（现状保持，零依赖）
  纯 tie 源码 ──tiec──▶ 原生可执行文件（不链接 trm）
  · 纯逻辑程序（无系统 API 需求）保持现状：tiec → LLVM → exe
  · 编译器自身（tiec）、rdu、纯算法库——全部走此模式

模式 B：trm 链接（按需 opt-in）
  需要系统能力（终端/UI/进程流/文件/环境）时，import trm 并按需链接
  · trm.dll/trm.so（动态，延迟绑定）或 trm.a（静态）
  · 用多少链接多少（域粒度裁剪：只要 terminal 就不链 ui）
```

- **trm 是可选依赖（opt-in）**：不 import trm 的程序，产物与现状完全一致
  （单文件原生 exe，零额外依赖）
- **丰俭由人**（tie 既有定位）：纯逻辑 → 直接编译；系统应用 → 按需链接 trm；
  嵌入式 → trm-embedded 静态子集
- **import 即依赖声明**：源码 `import "trm:terminal"` 才链接 terminal 域——
  编译期按 import 图裁剪链接面（域粒度）
- 与包模型一致：trm 是普通包（trm@版本），依赖它才链接它

## 2. 域划分

| 域 | 命名空间 | 内容 | 服务对象 |
| --- | --- | --- | --- |
| terminal | trm.terminal | TTY 检测/原始模式/ANSI/键读取/光标 | tieconsole |
| process | trm.process | spawn/管道流（stdin/stdout）/退出码/信号 | tieconsole/通用 |
| fs | trm.fs | 文件/目录（升级 std/fs 到 trm 统一面） | 通用 |
| env | trm.env | 环境变量/平台信息/用户目录 | tieconsole/通用 |
| session | trm.session | 历史/配置/profile（~/.tie/） | tieconsole |
| clock | trm.clock | 时间/定时器/延时 | 通用 |
| net | trm.net | socket/HTTP（升级 std/net 统一面） | 通用 |
| ui | trm.ui | 窗口/绘制/事件/字体/输入（原 tucore 全部） | tieui |
| data | trm.data | zd 序列化/格式化（整合 tieDB/persist/zd） | 通用 |

> 说明：fs/net/clock/data 升级自 std 已有模块到 trm 统一面——
> trm 是"统一 API 门面"，std 保留纯逻辑库（string/sort/json 等无平台依赖）。
> **域粒度裁剪**：import "trm:xxx" 才链接该域——不 import 则不链接
> （直接编译模式零依赖；域级 opt-in）。

## 3. 命名空间组织

```tie
// 嵌套命名空间：trm 顶级 + 域子命名空间
namespace trm {
    namespace terminal {
        pub func is_tty(fd: i64) -> bool
        pub func raw_mode(on: bool)
        pub func read_key() -> Key
        pub func ansi(s: string)            // ESC 序列输出（前置 \xHH 转义）
    }
    namespace process {
        pub func spawn(cmd: string, args: table<string>, pipes: bool) -> Process
        pub func read_stdout(p: Process) -> string
        pub func exit_code(p: Process) -> i64
    }
    namespace ui {
        // 原 tucore 全部（窗口/绘制/事件/字体/输入，见 §4）
    }
}
```

- 调用形态：`trm.terminal.is_tty(0)` / `trm.process.spawn(...)` / `trm.ui.window_create(...)`
- 域内方法链：`w.show().resize(..)`（句柄方法，原 tucore H2 类型化句柄）

## 4. ui 域（原 tucore 吸收，决策保留）

原 tucore-arch.md 决策全部保留，命名迁移：

| 原 tucore 决策 | trm.ui 对应 |
| --- | --- |
| A4 抽象 API + Win32 起步 | trm.ui.api（port 声明）+ trm/ui/win32/ |
| H2 类型化句柄 | struct Window/Font/PaintCmd 包装 i64 |
| E3 事件 + 信号混合 | event_drain + signal_check |
| D2 命令列表 | paint_begin/paint_rect/paint_end |
| F3 系统字体 + 位图 | font_load_system/font_load_bitmap |
| P2 目录分离 | trm/ui/win32/ x11/ fb/ |
| L1 显式生命周期 | trm.init() / trm.shutdown() |
| 组合式开发 | 保留（组件/行为/布局/模块四层） |
| JVM/.NET 借鉴 | 保留（P/Invoke/延迟绑定/元数据） |

## 5. 形态（动态库 + 嵌入式子集）

### 5.1 主形态：动态库（延迟绑定）

```
trm.dll / trm.so（延迟绑定——JVM/.NET 借鉴）
  ├── system 域（terminal/process/fs/env/session/clock/net/data）
  └── ui 域（窗口/绘制/事件/字体/输入）
```

- 动态库使 extern 符号运行时解析（LoadLibrary/dlopen），
  支持 P4b 实现选择运行时分发
- 前置：M5 动态库编译能力（docs/plans/dynamic-library.md 规划中）

### 5.2 嵌入式子集：trm-embedded

```
trm-embedded（静态链接，无动态库）
  ├── terminal → 无（无终端）
  ├── process → 无（无 OS）
  ├── fs → 帧缓冲/闪存抽象
  ├── ui → 帧缓冲直绘（原 tucore fb 域）
  └── clock/data → 精简
```

- 与 tie:embedded 角色咬合（编译期裁剪：禁用 spawn/终端）
- 无动态库环境：静态链接（P4b 编译期选择）

## 6. 与 std 的分工

| 层 | 内容 | 平台依赖 |
| --- | --- | --- |
| trm | 系统 API 门面（terminal/process/fs/env/session/clock/net/ui/data） | 有（平台实现） |
| std | 纯逻辑库（string/utf/sort/json/regex/math/...） | 无（纯 tie） |
| rdu | 嵌入式基础（bits/math/ascii/crc/fixed/rnd） | 无（标量） |

- trm 依赖 std（如 zd 序列化在 std/data 或 trm/data）
- std 不依赖 trm（纯逻辑）——**std 程序直接编译，无需 trm**
- 边界：**有平台依赖 → trm；纯算法 → std；无堆 → rdu**
- **直接编译保留**：不 import trm 的程序 = 现状产物（零依赖 exe）

## 7. 编译器实现拆解

| 模块 | 改动 |
| --- | --- |
| unsafe 扩展 | extern ptr/repr(C)（M0，trm 全部依赖） |
| trm api | 域命名空间 + port 声明（system/ui 抽象面） |
| trm/win32 | 平台实现（terminal/process/ui extern 封装） |
| trm/posix | 平台实现（Linux/macOS） |
| trm/fb | 嵌入式实现（帧缓冲） |
| 动态库 | M5 动态库编译（trm.dll/trm.so） |
| 迁移 | tucore 命名 → trm.ui（无用户，直接改名） |

## 8. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 与 tucore 关系 | D：trm 吸收 tucore（tucore → trm.ui） | A trm 在上、B 并入 tucore、C 平级 |
| 域划分 | system 9 域 + ui 1 域（terminal/process/fs/env/session/clock/net/data/ui） | 更少/更多域 |
| 命名空间 | trm 顶级 + 嵌套域（trm.terminal/trm.ui） | tucore 保留 |
| 形态 | 动态库（延迟绑定）+ trm-embedded 静态子集 | 纯静态 |
| 直接编译 | **保留**：trm 可选 opt-in，不 import 即零依赖 exe（域粒度裁剪） | trm 强制运行时 |
| 与 std | trm = 平台门面，std = 纯逻辑（有平台依赖 → trm） | 合并 std |

## 9. 未决问题

1. **fs/net 的升级路径**：std/fs → trm.fs 是移动还是包装（std/fs 保留别名？）
2. **data 域归属**：zd 序列化放 trm.data 还是 std（纯逻辑 zd 编解码可留 std，
   trm.data 只管文件 IO 面）
3. **动态库 vs 静态库默认**：桌面默认动态（延迟绑定）还是静态（简单）——
   与包模型 P5c 签名校验的交互
4. **trm 版本化**：trm 作为独立包（trm@2026.1）随语言版本同步？
5. **session 域范围**：仅 tieconsole 用（历史/别名/profile）还是通用状态存储
