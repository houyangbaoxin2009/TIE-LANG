# 调研：webui 模式技术调研（wasm 壳 + 桥）

> 状态：**调研报告**（2026-08-15，为 ui-framework.md 的 M5 webui 里程碑提供决策依据）
> 范围：wasm 编译链路、wasm 并发（JSPI/协程）、终端模拟壳、Canvas 桥、JS↔wasm 通信、
> 参考项目。所有实验性结论均在本机（LLVM 22.1.8 + node）实测验证。
> 关联：[ui-framework.md](ui-framework.md)（webui 定位：存量 tie 应用 Web 迁移通道，壳 + 桥）

## 1. 结论速览（对设计的影响）

| # | 结论 | 对 ui-framework.md 的影响 |
| --- | --- | --- |
| 1 | tie IR → wasm 编译链路**实测可行**（clang wasm32 + wasm-ld），真实障碍是 **libc 依赖**（printf/malloc/fopen 等），需自建桩层 | M5.1 工具链改造点明确：`backend/toolchain.tie` 的 link_exe 加 wasm 分支 |
| 2 | wasm-ld 默认 GC：`--no-entry` 时未导出函数被清除（实测 230 字节空模块） | webui 必须**显式 `--export` 入口函数**，由 JS 壳调用 |
| 3 | **JSPI 已标准化**（phase 4）：Chrome 137+/Firefox 139+，wasm 内可同步风格调用异步 JS API | **修正 5.9 的"async/await 编译为状态机"假设**——JSPI 包装 import 即可，编译器零改动；状态机变换降级为 fallback |
| 4 | stack switching 提案 2026-05 仍在设计中，**浏览器未落地** | wasm 上用户级多协程（M:N）暂不可行；webui 第一版单线程顺序执行 + JSPI 足够 |
| 5 | 多线程（Worker + SharedArrayBuffer）需 COOP/COEP 跨源隔离，浏览器仍非默认 | 确认 5.9 决策：wasm 共享内存/原子路径禁用，spawn → Worker 消息传递 |
| 6 | `@xterm/xterm` 6.0.0（2025-12，旧包 `xterm` 5.3 已弃用），WebGL 渲染、CJK/IME 成熟 | 终端模拟壳第一版直接用 xterm，不搞自研终端 |
| 7 | tie 的 `str_from_code` 原语已能构造任意码点（含 ESC 27）→ **字节输出通道已具备** | ext/tui 的"无 ANSI"限制是历史问题，webui 终端壳可输出 ANSI 序列 |
| 8 | WASI 路线（wasm32-wasip1）：本机 LLVM 无 sysroot（缺 crt1.o/libc.a），需下载 wasi-sdk；且浏览器端还需 WASI shim，wasip1→wasip2 生态在迁移 | **不推荐 WASI 路线**，走 wasm32-unknown-unknown + 自研 libc 桩（符合全 tie 哲学） |
| 9 | tieDB 的 zd 持久化在浏览器端映射 **OPFS**（Origin Private File System），SQLite wasm 同款方案 | M5 后期 tieDB web 化路径明确 |

## 2. wasm 编译链路（本地实验验证）

### 2.1 实验记录

本机环境：D:\LLVM（clang 22.1.8，与 tie 的 S1.1 升级版本一致，含 wasm-ld.exe）。

| 实验 | 内容 | 结果 |
| --- | --- | --- |
| E1 | C 最小模块 → `clang --target=wasm32-unknown-unknown -nostdlib` + wasm-ld | ✅ 生成 440B wasm，node 中调用 `add(2,3)=5` PASS |
| E2 | `tiec examples/hello.tie --emit-ir` → clang 编译 .ll → wasm-ld | ✅ IR 编译通过（仅 target triple 覆盖警告） |
| E3 | 同 E2 但 `--no-entry` 无导出 | ⚠️ 链接"成功"但产物 230B 空模块（仅导出 memory）——**函数被 GC** |
| E4 | 同 E3 加 `--export=main` | ❌ 报未定义符号：puts ×4、printf ×14（hello.tie 依赖的 libc 全部缺失） |
| E5 | `--target=wasm32-wasip1` 编译 + 链接 | 编译 ✅；链接 ❌ 缺 `crt1.o`、`libc.a`、`libclang_rt.builtins.a`（无 wasi-sdk sysroot） |

### 2.2 真实障碍清单（tie → wasm）

tie 生成的 LLVM IR **直接调用 libc**（hello.tie 就依赖 17 个符号）：
printf/puts/strlen/strcmp/malloc/fopen/fwrite/fclose/fflush/exit/remove/sqrt/sin/cos/tan/exp/log/pow/floor/ceil/round。
wasm32-unknown-unknown 是 freestanding，无 libc → 需要**libc 替代层（桩）**。

**桩层范围**（webui 壳自带，tie 写）：
- `printf/puts` → 写 stdout 字节队列（JS 轮询取走喂给 xterm）——**终端模拟的核心**
- `malloc/free` → wasm 线性内存分配器（dlmalloc 思路 / 自研 bump+free-list；
  与 tie 内存模型"arena + 移动语义"天然契合，可直接 arena 化）
- `fopen/fwrite/fclose/remove` → 内存虚拟文件 / OPFS 持久化桩
- `sqrt/sin/cos/...` → 数学函数 wasm 实现（或直接用 LLVM 内建 `llvm.sqrt` 等，
  大部分可映射内建指令，**零实现成本**）
- `exit` → 抛异常回 JS 壳（trap 语义）

### 2.3 工具链改造点（tiec）

`compiler/backend/toolchain.tie` 的 `link_exe()`（clang 链接）加 wasm 分支：

```
--target=wasm32-unknown-unknown 时：
  1. clang -c in.opt.ll --target=wasm32-unknown-unknown -nostdlib → .o
  2. wasm-ld --no-entry --export=main --export=__tie_poll_events
             [--export=...显式导出清单] -o out.wasm
       + 链接 libc 桩层（runtime 的 wasm 版，tie 编译产物）
       + --stack-size N（wasm-ld 默认栈，注意 --stack-first 让栈在数据段内）
```

要点：
- **显式导出清单**（E3 教训）：wasm-ld GC 默认清空未导出函数。webui 需要
  导出：入口 `main`（或 `__tie_start`）、事件轮询 `poll_event`、内存
  （JS 需访问线性内存）。
- **栈**：wasm 无原生栈，wasm-ld 在线性内存里分配固定栈（默认 64KB，可
  `-z stack-size=N` 调整）。与 tie 协程"固定栈 + 大小参数"模型同构。
- **链接顺序**：`runtime.a`（原生目标）不能用于 wasm——wasm 版运行时由
  tiec 用 `--target=wasm32` 重新编译生成（自举链本身支持交叉编译）。

### 2.4 WASI 路线评估（结论：不采用）

- wasm32-wasip1 需要 wasi-sdk sysroot（约 100MB，含 libc.a），且 wasi-libc
  是 C 实现——违背"全 tie 技术栈"。
- 浏览器端还要 WASI shim（fd_write 等 40+ 函数，大部分可桩化，但引入了
  WASI 版本包袱：wasip1 已冻结、wasip2（component model）迁移中，生态动荡）。
- 自研桩（wasm32-unknown-unknown）范围与 wasi-libc 桩化后所需工作几乎相同，
  但完全自控、无外部依赖。**推荐自研桩**。

## 3. wasm 并发与协程

### 3.1 JSPI（JavaScript Promise Integration）—— 重要发现

**状态**：W3C Wasm WG **phase 4（已标准化）**；Chrome 137+（2025-04）、
Firefox 139+（2025-07）。Safari 支持状态未确认（需查 WebKit status）。
参考：v8.dev/blog/jspi（2024-07，含性能：单次挂起/恢复 ~1μs 级开销）。

**机制**：wasm 调用被 JSPI 包装的 JS import（返回 Promise）时，**自动挂起
整个 wasm 调用栈**，Promise resolve 后恢复。wasm 侧代码保持同步风格。

**对 tie 的意义**（修正 ui-framework.md §5.9）：
- 原假设"tiec 的 wasm 后端把 async/await 编译为状态机变换"——**不必要**。
- tie 的阻塞式 IO（http_get 等 extern 调用）在 webui 壳中把对应 JS import
  用 JSPI 包装（`WebAssembly.Suspending` 包裹 import），tie 代码零改动、
  编译器零改动。
- 编译器零改动 = M5 工作量大幅下降。状态机变换仅作为非 JSPI 浏览器
  （Safari 未支持时）的 fallback，且可后置。

**限制**（如实说）：
- 挂起的是"整个调用栈"：主线程同一时刻只有一个 JSPI 挂起点有效——多协程
  并发调度（M:N 用户态协程）无法直接用 JSPI 表达。
- 每次挂起都要回到 JS 事件循环（rAF/setTimeout 轮询驱动的 UI 主循环
  不受影响）。
- 不改变 JS 侧语义：JS 不能被 wasm 挂起。

### 3.2 stack switching 提案（未来的协程原语）

**状态**（GitHub WebAssembly/stack-switching，2026-05 最新提交）：仍在
**设计讨论阶段**——2025-10 改 reduction semantics、2026-05 讨论
"stackable fibers"。**没有任何浏览器落地**，实现遥遥无期。

**结论**：wasm 上用户级多协程（tie 的 stackful 协程 M:N 调度）目前只有
三条路：
1. **单线程顺序 + JSPI**（webui 第一版）——CLI 迁移应用天然顺序执行，足够
2. **多协程 → 编译为状态机**（Rust async 式，编译器大手术）——后置
3. **等 stack switching 落地**（2-3 年尺度）——届时 tiec wasm 后端补
   `cont.new/resume` 指令即可
4. 附：Asyncify（Emscripten 二进制变换）可做通用挂起，但体积/性能代价
   大、集成复杂度高，不推荐

### 3.3 多线程：Web Worker + SharedArrayBuffer

**现状**（MDN 2026-02 更新）：
- `Atomics` 无条件可用；wasm 原子指令（threads 提案）同样无条件允许。
- `SharedArrayBuffer` 构造器**仍被隐藏**，除非页面满足：
  secure context + **cross-origin isolated**（COOP/COEP 响应头）。
- `WebAssembly.Memory shared` 共享内存同样受限。
- COOP/COEP 部署要求服务端配 header，第三方资源（CDN 等）需 CORP 配合，
  部署成本高，浏览器厂商表示"希望未来移除限制"但尚无时间表。

**对 tie 的意义**（确认 5.9 决策）：
- wasm 共享内存/原子路径在 webui **禁用**（编译错误）——现状正确。
- `spawn` → Web Worker 映射：postMessage 传递消息 = channel 语义，**可行且
  是唯一多线程通道**；Worker 内可再实例化一个 wasm 实例（独立线性内存，
  无共享）。tieDB 的 web 版大计算（向量搜索）可放 Worker。
- 注意：Worker 内的 wasm 不能用 SAB 与主线程共享 → 跨线程传大数据要
  复制（可接受，第一版）。

## 4. 终端模拟壳（webui 第一版形态）

### 4.1 xterm.js 选型

| 项 | 结论 |
| --- | --- |
| 包 | **`@xterm/xterm` 6.0.0**（2025-12-15 发布；旧包 `xterm` 5.3.0 已弃用，npm 明确提示迁移） |
| 渲染 | DOM/Canvas/WebGL 三级，WebGL 渲染大输出流流畅 |
| CJK/IME | 中文渲染与输入法支持成熟（VSCode 内置终端同源） |
| 体积 | unpacked ~5.9MB，gzip 后 ~1.5MB 级（ESM 构建，可 tree-shake） |
| 附加件 | @xterm/addon-fit（自适应）、@xterm/addon-web-links 等按需 |
| 协议 | 完整 VT100/ANSI + 扩展序列，`term.write()` 收字节、`term.onData` 出输入 |

### 4.2 对接模型（与 trm 事件轮询同构）

```
tie 程序（wasm）
  ├─ printf 桩 → stdout 字节队列（wasm 内存）
  │     └─ JS rAF/定时轮询 → term.write(new Uint8Array)   ← 输出
  ├─ poll_event()（导出）← JS 把键盘输入写进事件队列      ← 输入
  └─ main()（导出，JS 壳启动时调用）
```

- **输入**：term.onData → JS 编码 UTF-8 → 写入 wasm 事件队列 →
  tie 侧 `poll_event()` 读取（与 trm 桌面事件轮询模型完全一致）。
- **输出**：tie 的 print/println → printf/puts 桩 → 队列；JS 侧轮询
  取走 → `term.write()`。**不需要字节级 FFI 改动**——桩在 libc 层截获。
- **ANSI 能力**：tie 的 `str_from_code` 底座原语（2026-08-14 完成）可构造
  任意 Unicode 码点（含 ESC 27）→ tie 代码**已能生成 ANSI 序列**；
  ext/tui 的"无 ANSI"限制（语言无 \xHH 转义）是历史问题，webui 下可用
  str_from_code + 字符串拼接输出转义序列，或后续给语言加转义支持。
- **终端尺寸变化**：fit addon 的 resize 事件 → wasm 侧 `set_term_size(w,h)`
  import（CLI 程序可用 ioctl 语义桩，tie 侧 get_term_size()）。

### 4.3 自研终端 vs xterm（结论：用 xterm）

自研最小 ANSI 渲染器（Canvas，支持子集）约 300-800 行，但 CJK 宽度、
IME、选择/复制、滚动缓冲区、超链接等全要自己踩坑。xterm 成熟且 MIT，
tie 项目专注自举工具链，终端壳用 xterm 是标准做法（VSCode/Jupyter 同款）。

## 5. Canvas 2D 桥（tieui 应用上 wasm，M5 后期）

### 5.1 映射关系（trm.ui 绘制面 ↔ Canvas 2D）

| trm.ui（桌面 GDI/Direct2D） | webui（Canvas 2D） | 备注 |
| --- | --- | --- |
| fill_rect(x,y,w,h,color) | fillRect | 1:1 |
| stroke_rect / draw_line | strokeRect / lineTo+stroke | 1:1 |
| draw_text(s,x,y,font,color) | fillText（系统字体） | 见 5.2 字体 |
| clip / save / restore | save/restore+clip | 1:1 |
| draw_image | drawImage | 1:1 |

设计已定"无软件光栅化、指令流由平台执行"——Canvas 2D 是这套 Paint
Commands 的浏览器执行器，**与桌面后端共享同一指令抽象**，无需改 UI 核心层。

### 5.2 字体（重要差异点）

- 桌面：GDI/Direct2D 用**系统字体**渲染中文。
- 浏览器：Canvas fillText 同样用**系统字体**（含中文字体栈
  `"PingFang SC","Microsoft YaHei",sans-serif`）。
- 语义一致（都是系统字体），视觉差异可接受。**不需要在 wasm 内嵌字体**
  （省去 CJK 子集打包的巨量工作）。位图字体（嵌入式用）仅嵌入式需要。

### 5.3 性能与重绘

- 每帧指令数 ≤ 几百（UI 规模）时 Canvas 2D 直接 1:1 执行即可（60fps 无压力）。
- 重绘策略沿用 tieui 的**脏矩形**（Paint Commands 只发增量）。
- 大场景（图像处理/复杂动画）后置：WebGPU 或 OffscreenCanvas + Worker。
- 像素级操作（tieui 若未来有）→ putImageData 直传，天然支持。

### 5.4 事件

- DOM 事件（click/mousemove/keydown/wheel）→ JS 归一化 → 写入 wasm
  事件队列 → tie 侧 poll_event()。与桌面消息队列语义对齐（坐标、按键码、
  修饰键、滚轮增量）。

## 6. JS ↔ wasm 桥通信

### 6.1 互操作机制（语言面零改动）

- tie 的 extern 声明 → LLVM IR `declare` → wasm 里即 **import**；JS 壳
  在实例化时提供 import 对象即可。**现有 extern 机制直接复用**。
- 导出：wasm-ld 显式导出清单（见 2.3），JS 调 `instance.exports.main()`。

### 6.2 字符串跨界

- tie 字符串 = NUL 结尾 UTF-8 `char*`。JS 读：`TextDecoder().decode(
  memory.buffer, ptr, len)`——注意**含内嵌 NUL 的字符串**需加长度参数
  （zd 序列化天然带长度，不受影响）。
- JS → tie：JS 写 UTF-8 字节到 wasm 内存（malloc 桩分配）+ 传指针。
- 高频路径（终端输出）按块传输避免逐行 alloc。

### 6.3 桥消息格式：zd（已定决策的落地）

ui-framework.md §1.4 已定"webui 桥/wasm 通信默认 zd"。zd 是纯内存
序列化（varint/表/map，tie 自写，tieDB/persist/zd.tie）——在 wasm 上
**零系统依赖、直接可用**。事件队列条目、Worker 消息载荷都用 zd 编码：
`{type: i64, payload: zd}`。JS 侧解码 zd 需实现一个小解码器
（~200 行 JS，或直接不透传、只做字节搬运——事件由 wasm 侧解释，JS 只
搬运，**JS 不解析 zd**，最省）。

### 6.4 事件轮询

- 与 trm 桌面 `poll_event()` 模型一致：JS 把 DOM 事件写入 wasm 内存中的
  事件队列（简单 SPSC：JS 写 + wasm 读，单线程内无竞争），wasm 侧轮询。
- 驱动节奏：tie 主循环空闲时轮询；JS 侧不主动调用 wasm（避免重入），
  由 wasm 通过 `requestAnimationFrame` import 自驱动渲染（Canvas 模式），
  或 JS 定时泵（终端模式）。

## 7. 参考项目（同类实践）

| 项目 | 模式 | 对 tie 的借鉴 |
| --- | --- | --- |
| **Go wasm**（wasm_exec.js） | Go 程序 → wasm + JS 桥壳，主线程跑，`syscall/js` 桥 | tie webui 壳的直接范本：单实例 + 导出入口 + 事件循环 |
| **SQLite wasm**（sqlite.org） | Worker 线程跑 wasm + **OPFS** 持久化 + 可选 SAB 共享 | tieDB web 版：Worker + OPFS + zd 文件 |
| **pyodide** | Python 全栈 wasm（含 CPython），主线程/Worker 双模式 | 大型运行时移植路径参考（tie 运行时小得多，不需借鉴其复杂度） |
| **xterm.js 生态**（VSCode web 终端等） | 终端壳 + 后端字节流 | 4.2 对接模型 |
| **Emscripten JSPI/Asyncify** | 同步代码挂起方案 | JSPI 直接参考实现；Asyncify 不采用 |
| **JSLinux / v86** | 纯 wasm 模拟器 + 终端输出 | 证明"CLI 应用进浏览器"路线成熟 |

## 8. 对 ui-framework.md 的修订建议

### 8.1 §5.9 修正（wasm 兼容策略）

原：
> wasm 无原生协程：tiec 的 wasm 后端把 async/await 编译为状态机变换

改：
> - **首选 JSPI**（Chrome 137+/Firefox 139+，phase 4 标准化）：阻塞式 extern
>   IO 的 JS import 用 `WebAssembly.Suspending` 包装，tie 代码与编译器**零
>   改动**获得同步式异步 IO。Safari 未确认支持 → 壳层探测 `WebAssembly.Suspending`
>   是否存在，不支持时 IO 降级为回调模式（tie 侧 poll 结果）。
> - **多协程（M:N）wasm 化后置**：stack switching 提案未落地，第一版单线程
>   顺序执行；后续可选状态机变换或等 stack switching。

### 8.2 §8 里程碑 M5 拆解

| 阶段 | 内容 | 依赖 |
| --- | --- | --- |
| M5.1 | 工具链：tiec `--target=wasm32`（link_exe 分支 + wasm-ld + 显式导出 + wasm 版运行时重编译） | M0 |
| M5.2 | libc 桩层（tie 写）：printf/puts→stdout 队列、malloc→arena 分配器、文件→内存桩、数学→LLVM 内建 | M5.1 |
| M5.3 | webui 壳 v1：HTML + JS 桥 + `@xterm/xterm` 6 终端模拟 + 事件队列 + JSPI 包装 | M5.2 |
| M5.4 | 页面化：Canvas 2D 桥 + trm.ui 绘制面 wasm 化（tieui 应用迁移） | M5.3 + M2 |
| M5.5 | tieDB web 化：Worker + OPFS + zd 持久化 | M5.4 |

### 8.3 §9 待定决策更新

- 第 4 条"wasm 后端的 async 状态机变换工作量需专项评估" → **取消**（JSPI 替代）；
  改为"JSPI 不可用浏览器的 IO 降级模式设计"（小工作量）。

## 9. 风险与未决点

1. **Safari JSPI**：未确认支持状态。若缺失，Safari 用户走回调降级模式
   （功能可用、代码稍繁）。需在实现前查 WebKit status 一次。
2. **libc 桩覆盖范围**：tie IR 依赖的 libc 符号清单需按实际使用面扫描
   （stdio/string/math/文件），桩层按需生长。hello.tie 实测 17 个符号。
3. **wasm 体积**：tie 运行时 + 程序 wasm 化后体积未测（预计几十~几百 KB 级，
   无 libc 所以远小于 Emscripten 产物）；M5.1 后实测，必要时 -Oz + strip。
4. **内存布局**：wasm-ld 固定栈 + 数据段 + 堆的布局（--stack-first vs
   堆后置）需在 M5.1 实验确定，影响 malloc 桩与 JS 侧内存视图。
5. **OPFS 兼容性**：Safari OPFS 已支持；私有模式/旧浏览器降级为内存文件
   （不持久化）。
6. **COOP/COEP**：若未来要共享内存（tieDB 大表跨线程），需服务端配 header；
   第一版不依赖，保持"部署零配置"。

## 10. 关键参考链接

- JSPI：https://v8.dev/blog/jspi 、WebAssembly/js-promise-integration 提案
- stack switching：https://github.com/WebAssembly/stack-switching
- SAB/COOP/COEP：MDN SharedArrayBuffer（2026-02 更新）
- xterm：https://github.com/xtermjs/xterm.js （@xterm/xterm 6.0.0）
- SQLite wasm：https://sqlite.org/wasm/doc/trunk/index.md
- wasm-ld：LLVM lld wasm 文档（--export / --no-entry / -z stack-size）
