# 规划：tiu 开发框架（tieui 正式名）——完全模块化 UI 框架

> 状态：**规划**（2026-08-18 设计讨论定稿，未实现）
> 本文档定义 tiu（tieui 开发框架）的模块化架构。核心原则：**完全模块化、
> 一切皆为模块、方便开发者扩展**。模块化 = 用现有语言机制重新组织，
> 非新语言特性。
>
> 关联：ui-framework.md（tieui 定位与总体架构）、tucore-arch.md（trm.ui
> 性能核心层）、port-model.md（接口模型）、package-model.md（包模型）、
> tieDB/persist/zd.tie（zd 序列化）。

## 1. 定位与设计原则

### 1.1 定位

tiu 是 tie 的 UI 开发框架，即 ui-framework.md 中"tieui 框架层"的正式命名。
纯 tie 编写，平台无关，只调用 trm.ui 的抽象接口（port Backend）。

### 1.2 设计原则（2026-08-18 定稿）

1. **一切皆为模块**：组件、布局、主题、平台后端、服务、行为、动效——
   全部是模块，统一"模块五件套"模型，无例外无特殊通道。
2. **模块化 = 现有机制重组**：模块化的语言地基（port/泛型/enum/闭包/
   移动语义/arena/包模型/zd）已全部就位，tiu 不做新语言特性。
3. **重建模型**：组件状态更新 = 事件处理返回新状态，树变换替换旧值。
   零借用检查依赖，天然适配 tie 的移动语义。
4. **组件树 = 所有权树**：父拥有子，窗口关闭整树析构（S1.5 已支持）。
5. **组合优于继承**：无继承，所有扩展走 port 实现 + 包装组合。
6. **默认安全**：unsafe 不泄漏到普通组件代码（组件更新不取指针）。
7. **通信介质统一 zd**：跨线程/跨进程/模块间通信一律 zd 序列化
   （ui-framework.md §1.4 决策的落地）。

### 1.3 语言地基清单（已就位）

| 语言能力 | 状态 | tiu 用途 |
| --- | --- | --- |
| port 接口（显式 impl + 双形态分发） | ✅ 已实现 | 模块接口面（Component/Layout/Theme/Backend/…） |
| 包模型（tieir 包 + MVS 版本选择） | ✅ 已实现 | 模块的物理分发单元 |
| 泛型单态化 | ✅ 已实现 | 泛型组件/约束（后置） |
| enum ADT + switch 穷尽匹配 | ✅ 已实现 | 事件/状态/指令建模 |
| 闭包（函数指针） | ✅ 已实现 | 回调、行为装饰链 |
| 移动语义 + arena | ✅ 已实现 | 组件树所有权、重建替换、每帧 arena |
| struct 数据/逻辑分离 | ✅ 已实现 | 组件数据 vs 组件逻辑 |
| zd 序列化（namespace zd） | ✅ 已实现 | 事件/消息跨边界传输 |
| 错误处理（Result + ?） | ✅ 已实现 | 模块 API 返回 Result |

## 2. 模块统一模型

### 2.1 定义

> **模块 = port 接口 + 实现 + 元数据 + 生命周期 + 依赖声明**

五件套齐备才算模块，缺一不可：

```
模块（Module）
├── 接口面（port）      → 对外契约，实现可替换
├── 实现（impl）        → 具体逻辑（组件行为/布局算法/绘制后端）
├── 元数据（声明）      → 名称、版本、属性清单、插槽清单、事件清单
├── 生命周期            → init → mount → run → unmount → shutdown
└── 依赖声明            → 依赖哪些 port（接口依赖，非具体实现）
```

### 2.2 "一切皆为模块"的验收标准

tiu 中不存在"非模块"的东西。**写新东西 = 实现一个模块**，统一心智模型。
开发者扩展的六个入口（新组件/新布局/新主题/新后端/新行为/新服务）走
**同一条链路**：实现 port → module 块声明 → tiec 编译成 tieir 包 →
tiu.app 清单注册 → 可用。没有例外、没有特殊通道。

## 3. 模块类型体系（七类）

| 模块类型 | 职责 | 接口（port） | 扩展方式 |
| --- | --- | --- | --- |
| **Component** | UI 元素（按钮/输入/列表/…） | `port Component` | 写新组件 |
| **Layout** | 布局算法（row/column/grid/stack/…） | `port Layout` | 写新布局器 |
| **Theme** | 视觉样式（颜色/字体/间距 token） | `port Theme` | 提供新主题 |
| **Backend** | 平台绘制（Win32/Canvas/帧缓冲） | `port Backend` | 移植新平台 |
| **Service** | 应用服务（路由/存储/数据绑定/…） | `port Service` | 注册服务 |
| **Behavior** | 交互扩展（拖拽/滚动/手势/…） | `port Behavior` | 行为装饰器 |
| **Effect** | 动效（动画/过渡/…） | `port Effect` | 动效插件 |

层级关系（从底到顶）：

```
Backend（平台，1 个活跃）
   └── Theme（视觉，1 个活跃）
         └── Layout（布局器集合）
               └── Component（组件树，海量）
                     ├── Behavior（挂在组件上的交互）
                     ├── Effect（挂在组件上的动效）
                     └── Service（应用级横切）
```

每个模块独立编译为 tieir 包，应用按需组合——完全模块化的物理形态。

## 4. 组件 = 模块的最小单元

### 4.1 完整定义（五件套）

```tie
// 1) 数据：纯 struct（数据/逻辑分离原则）
struct Button {
    label: string
    width: i64
    height: i64
}

// 2) 接口面 + 实现：impl port Component
impl Component for Button {
    pub func build(self, ctx) -> tree<Component>      // 子组件
    pub func layout(self, ctx, constraints) -> Size   // 布局（可用默认）
    pub func paint(self, ctx)                         // 绘制
    pub func on_event(self, ctx, ev) -> Component     // 事件处理 → 返回新状态
}

// 3) 元数据：module 声明（属性/插槽/事件清单）
module Button {
    name:    "tiu.controls.Button"
    version: "0.1.0"
    props:   [label: string, width: i64, height: i64]
    events:  [clicked(x: i64, y: i64)]
}

// 4) 生命周期：可选 impl
// 5) 依赖：本模块 depends 的 port（如 Theme、Layout）
```

### 4.2 组合（children 插槽，无继承）

```tie
var row = tiu.layout_row(
    tiu.button("OK", on_click),
    tiu.input("name"),
)
var page = tiu.container(row, padding = 16)
```

### 4.3 插槽与泛型：异构容器用 port 动态分发

```tie
struct Row {
    children: tree<Component>   // 异构：Button/Input/Container 混排
}
```

- 组件树是运行时异构集合，泛型单态化无法表达"任意组件" → **port 动态分发
  （vtable）是唯一正确形态**。
- 重建时 `on_event` 返回 `Component`（接口类型），`replace_child` 用接口值
  替换——**具体类型只在构造点存在，树内统一以接口形态流转**（组件树唯一规则）。

## 5. 状态管理：重建模型（B1，核心架构分叉点）

### 5.1 决策

tie 只有移动语义、无借用检查器；组件树是所有权树（父拥有子），子状态
变更需要可变访问——语言层面不存在。**解决方案：状态更新走"重建模型"**：

```tie
// 核心：事件处理返回新状态，新值替换旧值（移动语义天然支持）
struct Counter {
    n: i64 = 0
}

impl Component for Counter {
    pub func on_event(self, ctx, ev: Event) -> Counter {
        if ev is Clicked {
            self.n += 1        // 修改本地值，无借用问题
        }
        return self            // 新状态移动出去
    }
}
```

### 5.2 对组件树的意义

```
事件到达目标组件
  → 目标组件 on_event 产生新状态
  → 从目标向上，逐层重建父组件（父 build 用新子状态重新生成子树）
  → 旧树析构，新树就位（所有权自动管理，无泄漏无悬垂）
```

### 5.3 为什么适合 tie

| 项 | 说明 |
| --- | --- |
| 零借用检查依赖 | 所有更新都是"算新值换旧值"，编译器不需要可变引用分析 |
| 语义可迁移 | 与 React/Flutter 一致（状态提升 + 重建），心智成熟 |
| 内存友好 | 重建产物放 arena，帧末整体释放，零碎片 |
| 性能可控 | 脏路径剪枝（§6）只重建受影响分支 |

> 备选方案"句柄 + ptr 修改"（组件存 arena，事件处理 unsafe 取址改）会让
> unsafe 泄漏到普通组件代码，违背"默认安全"立场。**不采用**。

## 6. 生命周期：build / layout / paint + 脏路径剪枝

### 6.1 总体循环（与 trm.ui 主循环同构）

```
每帧：
  1. 事件处理（冒泡 + 重建）
  2. 脏子树 layout（只重排受影响分支）
  3. 脏矩形合并 → paint（只绘制变化区域）
```

### 6.2 阶段 1：事件 → 冒泡 → 重建

```
events = event_drain()
for ev in events:
    target  = hit_test(ev)                    // 坐标 → 目标组件
    for comp in bubble_path(target):          // 目标 → 根
        new = comp.on_event(ev)               // 重建模型：返回新状态
        if new 与 comp 不同:
            parent.replace_child(comp, new)   // 树变换（移动语义，旧节点析构）
            mark_dirty(new)                   // 只标记这条重建链
```

**剪枝要点**：
- **兄弟子树零触碰**：事件只重建冒泡路径上的组件，兄弟不动不查
- **结构不变快速跳过**：重建后若结构等价（diff 结果），不触发 layout 重排，
  只标 paint 脏
- **重建链 = 冒泡链**：一条路径同时完成事件处理与状态更新，机制单一

### 6.3 阶段 2：layout（约束向下，尺寸向上）

```
for subtree in dirty_subtrees:         // 只有重建链上的子树需要重排
    subtree.layout(constraints)       // 父给约束（min/max w/h, flex 权重）
    → 递归向下传递约束
    → 叶子返回固有尺寸
    → 自底向上汇总（子尺寸 → 父布局）
```

布局结果缓存：每个组件缓存自己的 bounds（含命中测试用），layout 不动则
缓存有效。

### 6.4 阶段 3：paint（脏矩形合并）

```
rects = collect_dirty_rects(dirty_subtrees)   // 各子树 bounds 交集合并
for rect in merge(rects):                     // 合并相邻/重叠矩形
    ctx.paint_begin_dirty(rect)               // trm.ui 脏矩形 API
    render_subtree(root, rect)                // 只绘制与 rect 相交的组件
    ctx.paint_end()
```

### 6.5 三阶段触发矩阵

| 触发 | build | layout | paint |
| --- | --- | --- | --- |
| 事件（冒泡重建） | 重建链 | 仅结构变化分支 | 脏矩形 |
| 窗口 resize | 不变 | 全树 | 全区域 |
| 主题切换 | 不变 | 尺寸可能变 | 全区域 |
| 定时器/动画 | 不变 | 不变 | 受影响区域 |

## 7. 深度 diff（D-b）

### 7.1 diff 语义：递归按槽比较

```
diff(old_root, new_root) -> ops
  ├── 类型不同        → [Replace(old, new)]        // 整节点替换
  ├── 类型相同:
  │     ├── 属性不等  → [UpdateProps] + 递归子槽
  │     └── 属性相等  → 仅递归子槽（结构比对）
  └── 每个插槽槽位逐一递归 → 子级 ops 合并
```

比较的是**结构等价性**而非逐字段拷贝：同类型 + 同属性 + 子结构递归相等
= 整棵子树等价，零重建（**子树快照跳过**）。

### 7.2 增量更新：diff 产物直接驱动三阶段

```
ops（插入/替换/更新/删除）
  ├── 插入/删除 → 该子树 mark_dirty（layout + paint）
  ├── 替换      → 新节点替换旧节点，mark_dirty
  └── UpdateProps（尺寸不变）→ 仅 paint 脏，layout 跳过
```

### 7.3 keyed 槽位

同构列表（List 渲染 N 行）需要 key 识别移动，否则中间插一行导致整列表
diff 失配：

```tie
module TodoItem {
    props: [key: string, text: string, done: bool]   // key 字段
}

tiu.list(items, key = |it| it.id)
```

- 无 key → 按位置 diff（基本形态）
- 有 key → 按 key 匹配（移动/插入/删除精确识别）
- key 走 module 元数据声明（A1 的 props 清单标 key 字段），编译器校验唯一性

## 8. 事件模型（C2：冒泡）

### 8.1 管线

```
系统事件（trm.ui 事件队列）
  → 命中测试（hit-test 坐标 → 目标组件）
  → 冒泡：目标 → 父 → 祖父 → … → 根
      每层 on_event(self, ev) -> EventResult
```

```tie
enum EventResult {
    Handled       // 消费了，继续冒泡（父还能看到）
    Unhandled     // 没消费，继续上送
    Stop          // 终止冒泡（遮罩层/模态拦截用）
}

impl Component for ModalOverlay {
    pub func on_event(self, ev: Event) -> EventResult {
        if ev is Clicked {
            self.close()
            return Stop           // 不穿透到下层
        }
        return Unhandled
    }
}
```

**与重建模型咬合**：冒泡路径上的每个组件 on_event 都返回新状态，冒泡即
重建路径——一条冒泡链 = 一条重建链，机制统一。

### 8.2 命中测试

- **利用布局缓存**：从根沿 bounds 包含 (x,y) 的分支下行，命中测试零计算
- **z-order**：绘制顺序 = 命中顺序（后绘制在上层）→ 逆序遍历 children；
  遮罩/弹窗天然上层，与冒泡 Stop 组合实现模态语义
- **透明穿透**：组件可声明 `hit_self = false`（纯容器），命中测试跳过自身
- **按下捕获**：按下时 hit_test 定目标，后续移动/抬起直达目标（不重新
  hit）——避免拖拽指针滑出丢事件，与 Web setPointerCapture 同理念

## 9. 布局协议

```tie
// 布局 = port（模块），约束传递是统一协议
port Layout {
    pub func layout(self, ctx: LayoutCtx, children: &tree, c: Constraints) -> Size
}

struct Constraints {
    min_w: i64, max_w: i64
    min_h: i64, max_h: i64
    flex:  i64          // 弹性权重
}
```

- **布局器也是模块**：row/column/grid 各是 impl Layout 的模块，可嵌套
  （`tiu.row([tiu.column([...]), tiu.expand(x)])`）
- 组件默认布局（无子/固定尺寸）走框架内置，需要自定义才实现 port Layout
  ——**懒实现，按需扩展**
- flex/expand 是布局协议的公共算子，所有布局器共享

## 10. 主题双轨（D1 + D2）

### 10.1 D1 数据主题（默认路径）——纯 token 表，换肤零代码

```tie
// themes/light.tiedata（tie:data 人读格式；二进制分发用 tie:zd）
theme {
    colors:  { primary: "#3366FF", bg: "#FFFFFF", text: "#111111" }
    fonts:   { base: "Microsoft YaHei 14", mono: "Consolas 13" }
    spacing: { xs: 4, sm: 8, md: 16, lg: 24 }
}
```

### 10.2 D2 代码主题（覆盖路径）——可编程解析

```tie
impl Theme for BrandTheme {
    // 优先级最高；未覆盖的 token 回落 D1 数据
    pub func resolve(self, token: string) -> Value {
        if token == "colors.primary" { return #7C4DFF }
        return Theme.base.resolve(token)    // 回落到数据主题
    }
}
```

### 10.3 解析优先级

**代码主题 → 数据 token → 框架默认值**。组件统一走 `ctx.theme("spacing.md")`
取样式，不感知来源——主题是模块，替换实现即换肤。

## 11. 模块元数据（A1）与静态注册（E1）

### 11.1 module 块（代码内声明 + 编译器校验 + 宏生成注册）

```tie
module Button {
    name:    "tiu.controls.Button"          // 全局唯一标识
    version: "0.1.0"                        // 语义化版本（MVS 复用）
    props:   [label: string, width: i64]    // 必须对应 struct 字段（编译器校验）
    slots:   [children]                     // 插槽清单
    events:  [clicked(x: i64, y: i64)]      // 事件清单
    depends: [Theme, Layout]                // 接口依赖（port 级，非具体实现）
}
```

**校验规则（编译期）**：
- `props` 逐项必须命中 struct 字段，类型一致，否则编译错误
- `events` 必须被 impl 实际发射（发射清单核对）
- `slots` 必须与 build 使用的插槽一致
- `depends` 必须是已存在的 port

### 11.2 静态注册

module 块由编译器/宏展开为**静态注册代码**（`tiu_register(&ButtonModule)`）
——声明即注册，零手工登记。module 元数据可由 tiec 导出（tieir 包携带清单），
喂给 VSCode 插件做属性提示/补全。

### 11.3 装配流程

```
tiu.app([Button, Input, RowLayout, Theme(light), Backend(win32)])

1. 依赖解析    模块 depends 声明的 port 逐个核对（编译期，MVS 选版本）
2. 注册表生成  module 块宏展开 → 静态注册表（类型 → 构造器 + 元数据）
3. Backend 装载 init → 加载平台实现（延迟绑定，trm.ui）
4. Theme 装载  数据主题（tie:data 解析进 token 表）→ 代码主题覆盖
5. root 装配   build(root_widget) → 组件树就位 → 主循环启动
```

### 11.4 动态加载（E2，后置）

运行时加载 tieir 包的插件热装载后置。接口面（port）不变，注册来源从
静态清单变运行时加载——架构不返工。

## 12. 三大横切模块：Behavior / Effect / Service

### 12.1 Behavior：包装组件 + 事件管道

**行为 = Component 的包装器**（不引入"挂到组件上"的新概念）：

```
Wrapper(Behavior)          ← 包装组件：实现 port Component
  └── target(Component)    ← 被装饰组件
```

```tie
struct Draggable {
    dragging: bool = false
    offset: (i64, i64) = (0, 0)
    target: Component
}

impl Behavior for Draggable {
    // 输入原始事件 → 更新自身状态 → 产出语义事件或转发
    pub func on_event(self, ev: Event) -> BehaviorOutcome {
        switch ev.kind {
            case MouseDown: self.dragging = true
            case MouseMove if self.dragging:
                return Outcome(emit: [Dragged(dx, dy)])   // 产语义事件
            case MouseUp: self.dragging = false
        }
        return Outcome(forward: true)    // 未消费 → 转发 target
    }
}

// 使用：包装
var drag_btn = tiu.with(tiu.button("OK"), Draggable())
```

要点：
- **行为自身状态也走重建**（on_event 返回新行为），机制与组件一致
- **行为链 = 多层包装**：`with(with(comp, A), B)` 嵌套，事件从外层向内层流
  （后包装的在外层先处理：B → A → comp）——组合优于继承
- **语义事件**：行为把原始事件流翻译成业务语义（Dragged/Scrolled/Swiped），
  组件只认语义事件——组件与输入设备解耦（桌面鼠标/触摸屏/浏览器均可）

### 12.2 Effect：时间的函数 + 两档脏

```tie
port Effect {
    pub func tick(self, dt: i64, t: i64) -> Effect   // 时间推进 → 返回新效果（重建）
    pub func value(self) -> Value                     // 当前插值值
    pub func done(self) -> bool                       // 完成 → 从活跃表移除
}

// 声明式挂载：返回包装组件（与 Behavior 同构）
var btn = tiu.button("OK")
    .animate(Opacity, from = 0.0, to = 1.0, duration = 300ms)

// 绘制时读取：组件不感知"这是不是动画"，只问当前值
impl Component for FadeIn {
    pub func paint(self, ctx) {
        var alpha = ctx.effect(Opacity).value()   // 0.0 → 1.0 插值
        ctx.paint_rect(..., alpha)
    }
}
```

要点：
- **活跃效果表**：主循环维护 `table<Effect>`，每帧 tick 全部活跃效果，
  done 移除——动画是主循环第三驱动源（事件/信号/动画）
- **驱动重绘**：活跃效果值变 → 请求重绘（paint 脏）。无事件帧但有动画
  → 每帧重绘（timer 信号路径）
- **两档脏**：paint-effect（透明度/颜色/边框）只脏 paint；layout-effect
  （尺寸/位置）脏 layout + paint。区分方式：effect 声明档位（module 元数据
  `dirty: paint | layout`），diff 引擎据此决定重排范围
- **缓动**：effect 内置 easing（linear/ease-in/out/spring），配置参数

**与重建模型的关系**：动画不触发组件树重建——effect 是独立于树的时间
状态机，值直接进绘制命令。树结构稳定时动画只是"同一棵树上的值在变"。
动画完成/中断需改树时（淡出后移除节点）走正常重建路径（effect 结束回调
发事件 → 冒泡 → 重建）。动画性能与重建成本解耦。

### 12.3 Service：上下文注入 + topic 广播（zd 载荷）

**安全规则（定死）**：服务永不持有组件引用；组件只在方法调用瞬间通过
ctx 取服务。

```tie
// ctx = 上下文：主题 + 布局约束 + 效果值 + 服务定位器
impl Component for Router {
    pub func on_event(self, ctx, ev) {
        var store = ctx.service(Store)          // 取服务（port 接口，实现可换）
        var page = store.current_page()
        ...
    }
}
```

- `ctx.service(Store)` 返回 port 接口，具体实现由注册表决定（正式实现 /
  mock / 换后端）——服务可替换性天然来自 port，零分配（静态注册表查表）

**服务状态变化 → 组件如何知道**（主路径）：

```
服务状态变化
  → 服务向事件队列 emit 业务事件（带 topic，如 "store.changed"）
  → 主循环分发 → 组件 on_event 响应 → 冒泡重建
  → 关心该 topic 的组件在 on_event 里重新读服务状态 → 更新视图
```

- **安全**：服务→组件方向只走事件队列，无引用、无订阅表、无悬垂可能
- **topic 过滤**（第一版就支持）：emit 带 topic，组件按 topic 过滤订阅
- **补充路径**：组件 build 时主动读服务状态（`ctx.service(Store).count`）
  ——用于"显示值"型组件（读不订阅）

**服务间依赖**：

```tie
module Router {
    name: "tiu.services.router"
    depends: [Store]        // 服务依赖服务：接口依赖，初始化拓扑排序
}

// app.run() 生命周期
// init: 按依赖拓扑序初始化（Store 先于 Router）
// run: 主循环
// shutdown: 反序释放（Router 先于 Store）
```

### 12.4 七类模块全景

| 模块 | 本质 | 状态管理 | 与树的关系 | 通信 |
| --- | --- | --- | --- | --- |
| Component | 树的节点 | 重建（on_event 返回新状态） | 树本身 | 冒泡事件 |
| Layout | 布局算法 | 无状态（纯函数式） | 作用于子树 | 约束传递 |
| Theme | token 解析 | 只读 | 横切 ctx | ctx.theme() |
| Backend | 平台绘制 | 系统句柄 | 树外 | Paint Commands |
| Behavior | 包装组件 | 重建（自己的状态） | 装饰 target | 事件管道转换 |
| Effect | 时间插值器 | tick 推进 | 树外活跃表 | ctx.effect() |
| Service | 应用级状态 | 事件源 + 状态持有 | 树外 | ctx.service() + topic 广播 |

**共同模式**：Behavior/Effect 都是"包装组件"（进树）；Service/Effect 都是
"树外活跃对象"（不进树，靠 ctx + 事件与树交互）。七类模块全部在既有的
"port + module 块 + 事件 + ctx"机制内成立，**无需打破重建模型或引入引用**。

## 13. 通信介质：zd（统一决策）

### 13.1 决策

tiu 的**跨边界通信一律 zd**（ui-framework.md §1.4 "机器间通信一律 zd"的
框架内落地）：

| 场景 | 介质 | 说明 |
| --- | --- | --- |
| 组件树内事件（进程内同线程） | 内存对象直接 move | 无需序列化（ui-framework §1.4 已定） |
| 服务 topic 广播载荷 | **zd** | 跨模块边界，统一编码 |
| 跨线程事件（协程 → 主线程） | **zd** | 事件队列条目 zd 编码 |
| 跨进程/IPC | **zd** | 多窗口/多进程形态 |
| webui 桥（wasm ↔ JS） | **zd** | 事件队列条目、消息载荷（webui-research §6.3 已定） |
| 主题 token 表分发/持久化 | tie:data（人读）/ tie:zd（紧凑） | 按需求二选一 |
| 模块元数据（tieir 包携带） | 二进制（自有格式） | 与 tieir 一致 |

**zd 现状基础**：已实现（tieDB/persist/zd.tie，namespace zd）：fixint/
varint/定宽/字符串/表/map/record 字段编码 + save/load 8 字节魔数
"TIEDBZD"+v1。已在 compiler/driver、std/db、prep 使用。

### 13.2 落地要点

- 事件类型 → zd record 字段编码（`{type: i64, payload: zd}`，与
  webui-research §6.3 相同载荷形态）
- 服务广播：`emit(topic, payload_zd)`——topic 是字符串（过滤键），
  payload 是 zd 字节
- 跨线程事件队列：SPSC 队列条目 = zd 字节块（生产者编码、消费者解码，
  线程间零共享结构体）
- JS 侧只做字节搬运、不解析 zd（webui-research §6.3 已定，最省）

## 14. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 模块定义 | 模块五件套：port + 实现 + 元数据 + 生命周期 + 依赖声明 | 仅"组件/文件"粒度 |
| 模块类型 | 七类：Component/Layout/Theme/Backend/Service/Behavior/Effect | 少于七类的简化集 |
| 元数据（A1） | module 块代码内声明 + 编译器校验 + 宏生成静态注册 | 独立清单文件（A2） |
| 状态管理（B1） | **重建模型**：on_event 返回新状态，树变换替换 | 句柄 + ptr 修改（unsafe 泄漏） |
| 事件（C2） | 冒泡 + Stop，冒泡链 = 重建链 | 仅本地分发（C1） |
| 主题（D） | tie:data 数据 + tie 代码双轨，代码优先 | 纯数据（D1）/ 纯代码（D2） |
| 注册（E1） | 编译期静态注册表 + tieir 包分发 | 运行时动态加载（E2，后置） |
| diff（D-b） | 递归槽位深度 diff + keyed 槽位 + 子树快照跳过 | 类型 + 属性浅比较（D-a） |
| 生命周期 | build → layout（约束/缓存）→ paint（脏矩形合并） | 每次事件全树重建 |
| 命中测试 | 布局缓存 + z-order + 按下捕获 | 每次事件重算 |
| Behavior | 包装组件 + 事件管道（外层先处理） | 事件挂载器独立概念 |
| Effect | 时间插值器 + 两档脏（paint/layout）+ 包装组件挂载 | 纯绘制参数不进树 |
| Service | ctx 注入 + topic 广播（第一版支持）+ 事件队列通信 | 订阅表（悬垂风险） |
| 通信介质 | **统一 zd**（跨线程/跨进程/模块间） | 每场景各自格式 |
| 布局 | port Layout 模块 + 约束传递 + flex/expand 公共算子 | 硬编码布局引擎 |

## 15. 待定与后置

1. **动画调度精度**：timer 信号驱动帧率（rAF 类比）的具体实现细节，
   与 trm.ui 定时器信号衔接——实现期定。
2. **泛型组件**：`port Component<T>` 形态后置（与泛型单态化同路径）。
3. **布局协议完整参数**：flex 权重 / aspect / 对齐 / 间距的完整参数集
   实现期细化。
4. **无障碍 / 国际化**：后置（接口面预留语义属性，具体机制后定）。
5. **动态加载（E2）**：插件热装载，依赖包模型运行时加载能力。
6. **主题热切换**：运行中换主题 → 全树重绘的触发路径（已定触发矩阵，
   具体流程实现期定）。
7. **module 元数据导出**：tieir 包携带清单 → VSCode 插件属性提示的具体
   格式，与 LSP 语义衔接时定。

## 16. 与既有规划的关系

| 既有规划 | tiu 的关系 |
| --- | --- |
| ui-framework.md | tiu 是其中"tieui 框架层"的正式命名；模块化是其内部组织原则；M2 里程碑实现本框架 |
| tucore-arch.md（trm.ui） | 模块的 Backend 类；tiu 框架层调用 trm.ui 抽象面（Window/Painter/EventSource） |
| port-model.md | 所有模块接口的语言基础（已实现 S2.4） |
| package-model.md | 模块的物理分发单元（已实现 S3.2，tieir 包 + MVS） |
| webui-research.md | webui 壳的 Canvas 桥即 Backend 的 Canvas 实现；事件桥载荷形态已定 zd |
| ext/tui | 终端 UI 可作 tiu 的一个特殊 Backend（或先行验证场） |
