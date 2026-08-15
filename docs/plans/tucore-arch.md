# 规划：trm.ui 域架构（原 tieuicore/tucore，已并入 trm 运行时套件）

> 状态：**规划**（2026-08-15 设计讨论定稿，未实现）
> **2026-08-15 更新：本文件为 trm.ui 域的详细设计**——tieuicore 已并入
> [trm-arch.md](trm-arch.md)（tie 运行时套件），原 tucore 即 trm 的 ui 域。
> 命名空间：**trm.ui**（原 tucore/tcore 弃用）。本文档架构决策全部保留。
>
> 本文档定义 trm.ui（tie UI 核心，原 tieuicore）的架构。
> 决策汇总：
> **A4**（抽象 API + Win32 起步渐进）+ **H2**（类型化句柄）+ **E3**（事件+信号
> 混合）+ **D2**（命令列表 Paint Commands）+ **F3**（系统字体+位图双轨）
> + **P2**（平台目录分离）+ **L1**（显式生命周期）。
> 设计借鉴：**JVM/.NET 设计思路**（P/Invoke 互操作、延迟绑定、元数据驱动、
> 程序集部署单元、接口抽象）。
> 关联：unsafe 模型（extern/repr(C)）、接口模型（port 抽象面）、包模型
> （P4b 接口依赖）、UI 框架（tieui 消费 tucore）、tieir 格式（导出表）。

## 1. 定位

tieuicore = tieui 的性能核心兼容层（tie 语言编写）：
- 系统 API 的标量化封装（窗口/绘制/事件/字体/输入）
- 直接系统底层（Win32/X11/Wayland/帧缓冲）
- 性能关键路径（绘制/事件/资源管理）全部在此层
- tieui 框架层（纯 tie）只做逻辑编排，调用 tucore 抽象 API

## 2. JVM/.NET 借鉴（设计思路映射）

| JVM/.NET 机制 | tie 对应 | 借鉴点 |
| --- | --- | --- |
| **P/Invoke**（托管调非托管） | extern 声明 + 自动 marshaling | 互操作层模型：string↔char*、ptr 透传、repr(C) 结构体按引用 |
| **延迟绑定**（JVM 符号懒解析 / .NET 程序集加载） | extern 符号动态链接 | tucore 作为动态库（.dll/.so）运行时加载，**P4b 实现选择运行时分发** |
| **元数据驱动**（.NET metadata） | tieir 导出表 + port 声明 | 消费方按导出表生成调用代码（免头文件） |
| **程序集**（.NET assembly 版本化部署单元） | tieir 包 | 版本化、独立部署、依赖图（已定包模型） |
| **接口抽象**（JVM interface / .NET interface） | port（接口模型） | 抽象 API 面 = port 声明，多实现（win32/x11/fb） |
| **BCL 基础类库**（.NET 核心库） | std/tucore 分层 | tucore = 平台相关核心，std = 平台无关逻辑 |

**关键借鉴：延迟绑定/动态加载**——tucore 编译为动态库，extern 符号运行时
解析（LoadLibrary/dlopen），使 backend 实现选择（P4b）在运行时分发，
三端共用同一抽象面。

## 3. 目录结构（A4 抽象 API + P2 平台分离）

```
tucore/（type tie<unsafe> 库，命名空间 tucore）
├── api.tie          抽象 API（port 声明，平台无关）── 框架层依赖面
├── win32/           Win32 实现（extern 封装）
│   ├── window_win32.tie
│   ├── draw_win32.tie
│   └── event_win32.tie
├── x11/             X11 实现（M6 渐进）
├── fb/              帧缓冲实现（嵌入式，M7）
└── shared/          跨平台共享（句柄表/命令列表/字体兜底）
```

- **A4**：抽象面先定（api.tie 的 port 声明），平台实现逐个加
- **P2**：平台目录分离，发布按平台打包
- 渐进：M1 Win32 → M6 X11 → M7 帧缓冲

## 4. 抽象 API（api.tie，port 声明）

```tie
// 抽象面 = port 声明（平台无关签名）
port Window {
    pub func create(self, title: string, w: i64, h: i64) -> Window
    pub func show(self)
    pub func resize(self, w: i64, h: i64)
    pub func close(self)
}

port Painter {
    pub func begin(self) -> PaintCmd
    pub func rect(self, cmd: PaintCmd, x: i64, y: i64, w: i64, h: i64, color: u32)
    pub func text(self, cmd: PaintCmd, s: string, x: i64, y: i64, font: Font)
    pub func end(self, cmd: PaintCmd)
}

port EventSource {
    pub func drain(self) -> table<Event>       // E3 事件队列
    pub func signal_check(self) -> i64          // E3 信号标志
}
```

- 抽象面 = port（接口模型 P1 显式 impl）：win32/x11/fb 各实现
- tieui 框架层只依赖 api.tie 的 port（P4b 接口依赖，--backend 选实现）

## 5. 句柄模型（H2：类型化句柄）

```tie
// 句柄 = struct 包装 i64（系统句柄透传，零开销）
struct Window {
    var h: i64      // 系统句柄（HWND）
    // 私有字段，方法绑定（tie 的 struct 数据/逻辑分离）
}

// 使用：方法语法（obj.method() 转发）
var w = tucore.window_create("App", 800, 600)
w.show()
w.resize(1024, 768)

// 移动语义：句柄 move 不复制底层（安全）
var w2 = w          // move，w 失效
```

- 类型安全：Window 不能传成 Font（编译器检查）
- 零开销：i64 透传，无额外分配
- 与移动语义咬合：句柄 move 语义（唯一所有者）

## 6. 事件模型（E3：事件 + 信号混合）

### 6.1 事件队列（主通道）

```tie
// 批量拉取（E2 语义并入）：一次取完队列
var batch = tucore.event_drain()      // table<Event>
for ev in batch {
    switch ev.kind {
        case MouseMove: ...
        case KeyDown: ...
        case WindowResize: ...
    }
}
```

- 队列：无锁 SPSC（ISR/系统线程 → 主循环，与并发模型咬合）
- 事件 = 值（struct/枚举），含位置/键码/时间戳

### 6.2 信号标志（轻量通知通道）

```tie
// 信号：轻量标志位（系统消息映射），区别于事件队列
// 场景：重绘请求（WM_PAINT）、定时器到期、IO 就绪
var sig = tucore.signal_check()       // 位掩码：1=重绘 2=定时器 4=IO
if sig & 1 != 0 { render() }
if sig & 2 != 0 { on_timer() }
```

- 信号 vs 事件分工：
  - **事件**：有载荷的离散交互（鼠标/键盘/窗口消息）→ 队列
  - **信号**：无载荷的状态通知（重绘/定时器/IO 就绪）→ 位标志
- 效率：信号零分配（位运算），高频通知（每帧重绘）不走队列
- 系统映射：WM_PAINT → 重绘信号；WM_TIMER → 定时器信号；
  鼠标/键盘 → 事件队列

### 6.3 主循环形态（三端同构）

```tie
tucore.init()
var w = tucore.window_create("App", 800, 600)
w.show()

while !tucore.signal_check(& Shutdown) {   // 退出信号
    var batch = tucore.event_drain()       // 事件
    for ev in batch { handle(ev) }
    if tucore.signal_check(& Redraw) {     // 重绘信号
        render()                           // 命令列表 → 系统绘制
    }
}
tucore.shutdown()                          // L1 显式生命周期
```

## 7. 绘制模型（D2：命令列表 Paint Commands）

```tie
// 记录 → 提交（与架构图 Paint List 一致）
var cmd = tucore.paint_begin()            // 开启命令列表
tucore.paint_rect(cmd, 10, 10, 100, 40, 0xFF3366)
tucore.paint_text(cmd, "Hi", 20, 20, font)
tucore.paint_end(cmd)                     // 提交：系统执行绘制

// 脏矩形重绘优化：只记录变化区域
tucore.paint_begin_dirty(cmd, x, y, w, h)
```

- 与系统 API 1:1 映射（GDI/Direct2D/Canvas/帧缓冲都是这个形态）
- webui：命令列表 → Canvas 调用（1:1 翻译，工作量小）
- 嵌入式：命令列表 → 帧缓冲直绘
- 重绘优化基础：脏矩形（信号驱动，见 §6.2）

## 8. 字体（F3：系统字体 + 位图兜底）

```tie
// 系统字体（桌面）：GDI/CoreText/DirectWrite
var font = tucore.font_load_system("Microsoft YaHei", 14)

// 位图字体（嵌入式兜底）：内置 ASCII + CJK 子集
var bfont = tucore.font_load_bitmap("rdu_font")

// 度量：文本宽高（布局引擎依赖）
var w = tucore.font_measure(font, "Hello")
```

- 桌面：系统字体（本地化好、零打包）
- 嵌入式：位图字体（零依赖，rdu 风格）
- 统一抽象：font_measure/font_render 两实现

## 9. 组合式开发（一等设计原则，2026-08-15 补充）

### 9.1 原则

**"一切皆可组合"**——组件、行为、布局、模块四个层次全部支持组合式开发：

| 层次 | 组合机制 | 依托 |
| --- | --- | --- |
| 组件组合 | 组件树嵌套（任意深度）+ children 插槽 | 组件树（所有权树） |
| 行为组合 | 闭包链装饰（logger/auth/guard 叠加） | 闭包模型 C2 |
| 布局组合 | 布局器即组件（row/column/grid/stack 可嵌套） | 组件树 |
| 模块组合 | 包依赖 + port 接口（实现可替换） | 包模型 P4b |

### 9.2 组件组合（UI 层）

```tie
// 组合：Container > Row > [Button, Input]
var row = tui.row(
    tui.button("OK", on_click),     // 子组件 = 闭包回调
    tui.input("name"),
)
var page = tui.container(row, padding = 16)
```

- **children 插槽**：容器组件接收子组件列表（组合点）
- 任意组件可作子组件（同构组合，无继承）
- 组件树 = 所有权树（父拥有子，窗口关闭整树析构）

### 9.3 行为组合（闭包层）

```tie
// 行为装饰链：组合而非继承
var guarded = auth_guard(logger(handler))   // 权限 → 日志 → 业务

// 组合子（高阶函数）：with_auth / with_logging 返回新闭包
func with_logging(f: fn(i64) -> i64) -> fn(i64) -> i64 {
    return func(x: i64) -> i64 {
        log("call " + to_string(x))
        return f(x)
    }
}
```

- 闭包模型（C2）天然支持组合：高阶函数返回闭包
- tie 无继承 → **组合是唯一扩展方式**（设计上强制，符合"组合优于继承"）

### 9.4 布局组合（布局层）

```tie
tui.column([
    tui.row([a, b]),
    tui.expand(c),            // 弹性子组件（占剩余空间）
])
```

- 布局器也是组件 → 布局可嵌套（row 在 column 里）
- 弹性/权重布局（expand/flex）是布局组合的基本算子

### 9.5 tucore API 的组合性

- **命令列表可组合**：子命令列表嵌套 → 提交合并（组件的绘制递归进父列表）
- **事件管线可组合**：过滤器链（前置处理 → 事件 → 后置处理）
- **句柄操作链**：方法链（w.show().resize(..) 或链式调用）

## 10. 生命周期（L1：显式 init/shutdown）

```tie
tucore.init()           // 初始化：加载平台后端（动态链接，延迟绑定）、注册句柄表
...                     // 应用运行
tucore.shutdown()       // 清理：释放资源、卸载后端
```

- 显式调用（main 首行/尾行），与嵌入式 main_loop 匹配
- init 时按 --backend 加载平台实现（延迟绑定，JVM/.NET 借鉴）

## 11. 编译器实现拆解（tiec 自举）

| 模块 | 改动 |
| --- | --- |
| unsafe 扩展 | extern ptr/repr(C) 结构体按引用（M0） |
| tucore api.tie | port 声明（抽象面） |
| tucore win32/ | extern 封装实现（窗口/绘制/事件/字体） |
| 动态库 | tucore 编译为 .dll/.so（延迟绑定，M5 动态库能力） |
| 句柄表 | 共享层：句柄 → 平台对象映射（i64 表） |

## 12. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 分层 | A4：抽象 API + Win32 起步渐进 | A1 单层、A2 域模块、A3 三层全平台 |
| 句柄 | H2：类型化句柄（struct 包装 i64 + move） | H1 裸 i64、H3 引用对象 |
| 事件 | E3：事件队列 + 信号标志混合 | E1 单事件轮询、E2 纯批量 |
| 绘制 | D2：命令列表（Paint Commands） | D1 立即模式、D3 场景图 |
| 字体 | F3：系统字体 + 位图兜底双轨 | F1 纯系统、F2 纯位图 |
| 平台 | P2：目录分离（win32/x11/fb） | 条件编译、符号重定向 |
| 生命周期 | L1：显式 init/shutdown | RAII 自动 |
| 命名空间 | **tucore**（非 tcore） | tcore |
| 借鉴 | JVM/.NET：P/Invoke、延迟绑定、元数据驱动、程序集、接口抽象 | 无 |
| 组合式开发 | 一等设计原则：组件/行为/布局/模块四层全组合（组合优于继承） | 继承式 |

## 13. 未决问题

1. **动态库 vs 静态库**：tucore 延迟绑定需要动态库（.dll/.so）——M5 动态库
   编译能力（docs/plans/dynamic-library.md 规划中）是前置；嵌入式无动态库
   （静态链接，无延迟绑定——P4b 编译期选择）
2. **句柄表的并发**：句柄 → 平台对象映射的访问（主线程专用？文档化）
3. **E3 信号的扩展**：信号位掩码 64 位够用吗（自定义信号留给应用？）
4. **绘制命令的序列化**：命令列表跨平台传输（wasm 场景命令列表编码——
   与 tieir 序列化技术同源）
5. **tucore 的测试策略**：无窗口环境（CI）——命令列表可离线回放验证
   （绘制命令纯数据，可 dump/比较）
