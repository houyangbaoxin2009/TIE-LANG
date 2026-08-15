# 规划：tie 实施路线图（多会话并行执行）

> 状态：**规划**（2026-08-15 定稿，待执行）
> 本文档整合全部 19 份设计文档为可执行实施路线图。
> 执行模式：**多会话并行**（主控只规划与验收，不亲自写代码；
> 每个工作单元 = 独立会话的可交付物）。
> 决策确认：
> 1. S1.1 LLVM 升级先行独立推进
> 2. 多会话并行实现（主控验收）
> 3. **M3 移动语义提前到阶段 1**（避免后续特性建立在旧语义上）
> 4. tieconsole + 嵌入式纳入本年度目标
> 5. **时间约束：全部工作 1 星期内完成**（2026-08-15 决策）——
>    高并行度执行，多会话同时推进，主控密集协调验收
> 6. **时间观念修正（2026-08-15）**：AI 写代码比人工快 100 倍——
>    1 周是保守上限（缓冲），实际冲刺按"最快可达"调度，
>    每波次不设固定 Day 门槛，依赖验收通过即启动下一波
> 关联：ui-framework（里程碑 M0-M8）、全部专项模型文档（unsafe/int/error/
> closure/port/package/role/string/macro/tieir/trm/tucore/tieconsole/tsp/
> build-config/hw-accel/llvm-upgrade）。

## 0. 执行模式（多会话并行，1 周冲刺）

```
主控（本会话）：规划 + 任务拆解 + 验收 + 密集协调
  │
  ├── 并行波 1：会话 A(S1.1) + B(S1.2-1.4) + C(S1.5) + F(webui 调研)
  ├── 并行波 2：会话 D(S2.1) + E(S2.2) + G(S2.3+S2.4) + H(S3.1)
  ├── 并行波 3：S3.2 + S3.3 + S3.4 + S4.1 + S4.2
  ├── 并行波 4：S4.3 + S4.4 + S4.5 + S4.6 + S4.7
  └── 每个单元完成后：验收（编译零错误 + 回归测试）→ 提交推送双远端
```

- **1 周时间盒（保守上限）**：AI 写代码比人工快 100 倍——全部 17 个单元
  按"最快可达"调度，1 周是缓冲而非目标
- **高并行度**：同一时间 4-6 个会话并行推进（远超常规串行）
- 会话间依赖通过"验收门槛"衔接；主控全天候协调
- 风险控制：单元粒度已拆到最小（每单元独立可交付），
  依赖尽量消除（webui 调研提前、S1.1 独立）
- 主控不做实现，只做：任务拆解、依赖协调、验收、提交推送

## 阶段 1：地基加固 + 移动语义

> 目标：语言核心互操作能力 + 内存模型底座。**并行波 1**。

### S1.1 LLVM 升级（独立，先行）✅ 已完成（2026-08-15）

- **内容**：LLVM 18.1.8 → 22.1.8（见 [llvm-upgrade.md](llvm-upgrade.md)）
- **交付**：vendored LLVM 22.1.8 替换 + 回归全绿
- **验收**：compiler/tests + _driver_test 行为等价通过；IR 语法无错；MSVC ABI 回归
- **并行**：与其他会话完全独立
- **实现记录**：D:\LLVM 切换 22.1.8（18 备份 D:\LLVM18）；toolchain.tie 适配 clang 22
  默认 lld-link（非 vendored 显式 -fuse-ld=link）；回归全绿（详见 llvm-upgrade.md §5.2a）

### S1.2 M0：unsafe 语法（语言地基）

- **内容**：unsafe（U3）+ ptr<T>/slice<T>（T2/T4/O3）+ repr(C)（R1）+
  extern 强制 unsafe（E3）+ atomic<T>（A1）+ asm!（I1）+ alloc/free（M1）
  （见 [unsafe-model.md](unsafe-model.md)）
- **交付**：编译器支持 unsafe 全能力；示例：unsafe fn 调 Win32 API
- **验收**：编译零错误；unsafe 边界检查生效；repr(C) 布局精确
- **状态**：✅ 完成（2026-08-15，S1.2 落地，tiec 自举 0-Rust；
  详见 unsafe-model.md 实现记录）

### S1.3 窄整数（互操作前置）

- **内容**：i8/i16/i32/u8/u16/u32/u64/f32（L2+L3 字面量/C2+C3 转换/
  O3 溢出/A1 算术/B2 移位）（见 [int-model.md](int-model.md)）
- **交付**：窄整数完整语义 + checked_* + as_* 转换族
- **验收**：repr(C) 结构体字段宽度精确匹配 C ABI
- **状态**：✅ 完成（2026-08-15，S1.3 落地，tiec 自举 0-Rust；
  详见 int-model.md 实现记录）

### S1.4 角色系统扩展

- **内容**：R2 多角色 + R3 参数化 + 文件名声明（F1/R3）
  + db:vector 向量数据库角色（见 [role-model.md](role-model.md)）
- **交付**：`type tie<db:vector, owned>` 解析 + `xxx.db-vector.tie` 文件名一致检查
- **验收**：不一致 = 编译错误；角色语法子集约束生效
- **状态**：✅ 完成（2026-08-15，S1.4 落地，prep+driver 双端；
  详见 role-model.md 实现记录）

### S1.5 M3：移动语义 + arena（提前到阶段 1）

- **内容**：live/moved 状态跟踪 + 所有权析构 + arena 块 + 逃逸检查
  + **std/compiler 一次性迁移**（无渐进，见 [ui-framework.md](ui-framework.md) §4.4）
- **交付**：默认移动语义 + arena 区域 + std/compiler 全量迁移
- **验收**：move 示例正确；arena 释放正确；编译器自身迁移后自举成功
- **并行**：与 S1.2-1.4 可部分并行（不同语言面），但影响 std/compiler
  需与后续单元衔接

## 阶段 2：语义升级

> 目标：字符串/闭包/错误/接口——语言表达力完整。**并行波 2**。

### S2.1 字符串模型

- **内容**：{ptr,len} 二进制安全 + 迭代器（chars/char_indices）+
  StringBuilder + SSO + 边界自动 NUL（见 [string-model.md](string-model.md)）
- **依赖**：S1.5（移动语义）
- **验收**：字符串含 \0 + len O(1) + 迭代器 + FFI 零拷贝

### S2.2 闭包

- **内容**：func 字面量（A3）+ move 捕获（B1）+ 函数指针（C2）+
  递归/闭包内 await（见 [closure-model.md](closure-model.md)）
- **依赖**：S1.5（移动语义/所有权）
- **验收**：闭包示例 + 递归闭包 + 与协程咬合（spawn 闭包）

### S2.3 错误处理

- **内容**：Result/Option（E2）+ ? 传播（P1）+ 可配置 panic（F3）
  + 内外分明（R2）（见 [error-model.md](error-model.md)）
- **依赖**：S1.5 + S2.2（闭包）
- **验收**：? 链式传播 + panic 三端行为 + Result 跨 channel

### S2.4 接口 port

- **内容**：显式 impl（P1）+ 双形态分发（D3）+ 隐式 vtable（I1+I2）
  + 借用归 unsafe（见 [port-model.md](port-model.md)）
- **依赖**：S2.2（函数指针做 vtable）
- **验收**：静态/动态分发 + 手写 vtable（unsafe）+ 异构容器

## 阶段 3：工具链完备

> 目标：构建/库包/宏/LSP——开发者体验完整。**并行波 2-3**。

### S3.1 构建配置

- **内容**：config.data.tie（L2 分层）+ 分节（tiec/prep/pkg）+ profile（P3）
  （见 [build-config.md](build-config.md)）
- **依赖**：阶段 1（角色系统）
- **验收**：`--backend=wasm` 实现选择 + profile dev/release

### S3.2 库/包模型

- **内容**：多文件包（L1c）+ tieir 序列化（S3）+ MVS（P2c）+ 签名（P5c）
  + 接口依赖（P4b）（见 [package-model.md](package-model.md) + [tieir-format.md](tieir-format.md)）
- **依赖**：S2.4（port）+ S3.1（构建配置）
- **验收**：发布 tieir 包 + 消费方链接 + 签名校验

### S3.3 宏/元编程

- **内容**：code 三形态（C1+C2+C3）+ 函数式宏（M3）+ 过程宏（M4 后置）
  + 卫生（H2+H3）（见 [macro-model.md](macro-model.md)）
- **依赖**：S2.2（闭包）
- **验收**：宏展开 + gensym 隔离 + code 类型三形态

### S3.4 LSP 重写（tsp）

- **内容**：tsp 全量 16 项能力 + 增量分析 + 语义高亮 + tieconsole 复用
  （见 [tsp-lsp.md](tsp-lsp.md)）
- **依赖**：S2.1（符号表稳定）
- **验收**：VSCode 补全/引用/重命名 + 增量编辑流畅

## 阶段 4：运行时与 UI + Web（1 周冲刺目标）

> 目标：trm/tieui/tieconsole/嵌入式/webui/硬件加速——产品形态落地。
> **并行波 3-4**。webui 提前 + 定义更新（网页 + tie 索引）；
> 硬件加速提前（2026-08-15 决策）。

### S4.1 trm

- **内容**：动态库（延迟绑定）+ system 域（terminal/process/fs/env/
  session/clock/net/data）+ 直接编译保留（opt-in）（见 [trm-arch.md](trm-arch.md)）
- **依赖**：S1.2（unsafe）+ S3.2（动态库/包）
- **验收**：动态加载 + 域粒度裁剪 + 直接编译零依赖保持

### S4.2 trm.ui（原 tucore）

- **内容**：窗口/绘制/事件信号/字体（A4/H2/E3/D2/F3）+ 组合式开发
  （见 [tucore-arch.md](tucore-arch.md)）
- **依赖**：S4.1
- **验收**：Win32 窗口显示 + 命令列表绘制 + 事件/信号混合

### S4.3 tieui 框架

- **内容**：组件树/布局/事件分发 + 组合式开发（children 插槽/行为装饰）
  （见 [ui-framework.md](ui-framework.md) + tucore-arch §9）
- **依赖**：S4.2
- **验收**：组合组件示例 + 三端复用抽象面

### S4.4 tieconsole（本年度目标）

- **内容**：对象化 shell：`->` 管道 + cmdlet 预定义 + 格式系统（F1+F2+F3）
  + 交互（I1+I2+I3）+ 会话（A1+A2）（见 [tieconsole.md](tieconsole.md)）
- **依赖**：S4.1（terminal/process 域）+ S3.4（LSP 补全复用）
- **验收**：PowerShell 对标交互 + 对象管道 + 跨平台一致

### S4.5 嵌入式（本年度目标）

- **内容**：tie:embedded 子集 + trm-embedded + 协作式协程 + 静态池
  （见 [ui-framework.md](ui-framework.md) §6 + [embedded-rdu.md](embedded-rdu.md)）
- **依赖**：阶段 2（移动语义/闭包）+ S4.2（帧缓冲）
- **验收**：MCU 帧缓冲输出 + 编译期裁剪（禁用 spawn = 编译错误）

### S4.6 webui（提前，定义更新：网页 + tie 索引）

- **内容**：**webui = 网页 + tie 索引**——tie 程序编译 wasm 跑浏览器 +
  tieDB/vecsearch 检索能力作为 Web 服务（索引在服务端，网页端查询）
  （见 [ui-framework.md](ui-framework.md) §2 + [tiedb 规划](tiedb.md)）
- **依赖**：阶段 2 + LLVM 22（wasm 支持成熟）+ tieDB（索引基础）
- **验收**：tie 程序浏览器运行 + 网页端检索 tie 索引（向量/文本搜索）

### S4.7 硬件加速（提前）

- **内容**：SIMD（P1 立即）→ GPU 检索（P2）→ Intel NPU（P3）
  ——**GPU/NPU 直接服务 webui 的 tie 索引检索**（embedding + 检索全链路）
  （见 [hw-accel.md](hw-accel.md)）
- **依赖**：S4.6（webui 索引场景）+ tieDB（vecsearch）
- **验收**：vecsearch GPU 10-50x + OpenVINO embedding + 网页端低延迟检索

## 阶段 5：远景（明年）

> 目标：更广泛平台与生态。**原 webui/硬件加速已提前至阶段 4**。

### S5.1 生态扩展

- **内容**：更多平台（macOS/Wayland 后端完善）、tieir 生态、更多组件库
- **依赖**：阶段 4 完成
- **验收**：三端全平台 + 生态包增长

## 依赖图总览

```
S1.1 LLVM升级（独立）──────────────┐
                                   ├─→ 阶段2（S2.1→S2.2→S2.3→S2.4）
S1.2-S1.4 语言地基 ────────────────┤         │
                                   │         ├─→ 阶段3（S3.1→S3.2 / S3.3→S3.4）
S1.5 移动语义（提前）──────────────┘         │
                                             ├─→ 阶段4（S4.1→S4.2→S4.3→S4.4→S4.5）
                                             │      └─→ S4.6 webui → S4.7 硬件加速
                                             │
                                             └─→ 阶段5（S5.1 生态扩展，明年）
```

> 说明：S4.6 webui 依赖 tieDB（索引基础）与阶段 2；S4.7 硬件加速直接服务
> S4.6 的索引检索——三者联动，webui 落地后硬件加速立即跟进（GPU/NPU 加速
> tie 索引检索）。

## 会话分配建议（1 周冲刺）

| 会话 | 工作单元 | 波次 |
| --- | --- | --- |
| 会话 A | S1.1 LLVM 升级 | 波 1 |
| 会话 B | S1.2-S1.4 语言地基 | 波 1 |
| 会话 C | S1.5 移动语义 | 波 1 |
| 会话 F | S4.6 webui 前置调研（wasm 可行性） | 波 1（提前） |
| 会话 D | S2.1 字符串 | 波 2 |
| 会话 E | S2.2 闭包 | 波 2 |
| 会话 G | S2.3 错误处理 + S2.4 接口 | 波 2 |
| 会话 H | S3.1 构建配置 + S3.2 库包 | 波 2-3 |
| 会话 I | S3.3 宏 + S3.4 tsp | 波 3 |
| 会话 J | S4.1 trm + S4.2 trm.ui | 波 3 |
| 会话 K | S4.3 tieui + S4.4 tieconsole | 波 4 |
| 会话 L | S4.5 嵌入式 + S4.7 硬件加速 | 波 4 |
| 主控 | 全程：验收 + 依赖协调 + 双远端提交 | 全程 |

> 备注：波次为建议调度，实际以依赖验收为准（前一单元验收通过才启动
> 依赖它的单元）；无依赖的单元可随时提前。

## 验收总则（每单元）

1. 编译零错误（用户核心关注）
2. 单元回归测试通过（tests/ + _driver_test 行为等价）
3. 遵循设计文档决策（无偏离）
4. 提交推送双远端（franj2 + GitHub）
5. 更新 README/CHANGELOG（用户文档维护要求）
