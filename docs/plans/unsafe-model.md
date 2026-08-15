# 规划：tie unsafe 模型（指针/切片/repr(C)/extern/原子/汇编/手动内存）

> 状态：**规划**（2026-08-15 设计讨论定稿，未实现）
> 本文档定义 tie 的 unsafe 完整模型——所有底层能力的总闸门。
> 决策汇总：
> **U3**（unsafe 语法：块/函数为主 + 文件级逃生舱）
> **指针模型**（T2+T4+O3+S1+U1 一组方案）：
>   T2 类型化指针 `ptr<T>` + T4 切片 `slice<T>` + O3 全量操作集
>   + S1 指针全归 unsafe + U1 ref 形参 = 指针安全语法糖
> **R1**（显式 repr(C) struct）+ **E3**（extern 声明标注 unsafe，调用必须 unsafe）
> + **A1**（语言级 `atomic<T>`）+ **I1**（语言级 `asm!` 内联汇编）
> + **M1**（语言级 alloc/free）。
> 关联：闭包模型（函数指针 C2）、接口模型（vtable 手写 I2）、内存模型
> （移动语义+arena）、并发模型（协程/无锁）、UI 框架。

## 1. 目标与原则

### 1.1 原则（安全模型的灵魂）

1. **默认安全，显式解锁**：普通代码永远碰不到指针/地址/内存布局/系统调用；
   只有显式 unsafe 区域可以
2. **一致性**：所有触底操作（指针/系统调用/原子/汇编/手动内存）统一归 unsafe，
   没有"隐式 unsafe"通道
3. **编译器边界检查**：unsafe 边界由编译器强制（安全代码调用 unsafe 能力 =
   编译错误）

### 1.2 unsafe 能力清单（7 类）

| # | 能力 | 决策 | 依赖 |
|---|---|---|---|
| 1 | 指针/切片（ptr/slice 类型、算术、deref） | T2+T4+O3+S1 | 内存模型 |
| 2 | repr(C) 结构体（显式 C 内存布局） | R1 | 系统 API |
| 3 | extern 调用（系统 API 直连） | E3（强制 unsafe） | 现状 T0.7 扩展 |
| 4 | 原子/无锁（atomic + memory_order） | A1（语言级） | 并发模型 |
| 5 | 内联汇编（asm!） | I1（语言级） | 协程/嵌入式 |
| 6 | 手动内存（alloc/free） | M1（语言级） | 内存模型 |
| 7 | vtable 手写/函数指针构造 | I2（port 模型） | 接口模型 |

## 2. 语法（U3：块/函数为主 + 文件级逃生舱）

### 2.1 unsafe 函数与块（主形态）

```tie
// unsafe 函数：整函数可触底
unsafe fn create_window(title: string, w: i64, h: i64) -> i64 {
    ...
}

// unsafe 块：局部逃逸
unsafe {
    var p: ptr<i64> = addr_of(x)
    ...
}
```

### 2.2 文件级声明（逃生舱）

```tie
type tie<unsafe>   // 整文件 unsafe（仅纯系统编程文件使用，如 tieuicore 内部）
```

- 文件级是逃生舱不是主路：安全代码与 unsafe 代码同文件共存时用块/函数隔离
- 纯系统编程文件（tieuicore 适配层）可整文件声明，内部零标记

### 2.3 安全边界（编译器强制）

- 安全代码调用 unsafe fn / 使用 unsafe 能力 = 编译错误
- 安全函数体内不能出现指针类型、extern 调用、原子操作、asm!、alloc/free
- unsafe 块内的代码可以调用安全代码（向外是安全的）

## 3. 指针模型（T2 类型化 + T4 切片 + O3 全量操作 + S1 全归 unsafe + U1 ref 统一）

### 3.1 类型化指针 `ptr<T>`

```tie
unsafe {
    var p: ptr<i64> = addr_of(x)      // 创建（unsafe）
    var v: i64 = deref(p)             // 读（unsafe），类型已知
    deref_write(p, 42)                // 写（unsafe）
    p = p + 1                         // 算术按元素步长（i64 → +8 字节），unsafe
    var d: i64 = q - p                // 指针距离（unsafe）
    var is_n: bool = is_null(p)       // 空检查（unsafe）
}
```

- `ptr<T>` 是泛型类型（与单态化咬合），deref 类型安全
- 算术单位 = 元素（自动按类型步长）；`ptr<u8>` 即字节指针

### 3.2 切片 `slice<T>`（指针 + 长度）

```tie
unsafe {
    var s: slice<i64> = slice_of(arr, 0, 10)   // 指针 + 长度
    var v: i64 = s[3]                          // 越界检查（unsafe 内可开/关）
    var n: i64 = len(s)                        // 长度
}
```

- 对应系统 API 的 buffer+len 模式
- 越界检查：默认开（unsafe 内仍检查，防低级错误）；`--no-bounds-check` 关闭
- 与现有定长表/动态表的关系：slice 是它们的"原始视图"

### 3.3 全量操作集（O3）

**标准集**：
- `addr_of(x)` 取地址 / `addr_of_field(s, s.f)` 字段地址
- `deref(p)` 读 / `deref_write(p, v)` 写
- `p + i64`（元素步长）/ `p - q`（距离）/ `is_null(p)` / 与 i64 互转
- `memcpy(dst, src, n)` / `memset(p, val, n)` 原语
- 比较（==/!=/<）

**扩展集（O3 全量）**：
- **cast**：`ptr<T>` ↔ `ptr<U>`（类型重解释，unsafe，对齐自证）
- **volatile 读写**：`volatile_load(p)` / `volatile_store(p, v)`（硬件寄存器/MMIO）
- **对齐控制**：`align_of<T>()` / `align_up(p, n)`（嵌入式对齐需求）

### 3.4 S1：指针全归 unsafe

- 指针的**一切**操作（创建/读写/算术/比较/互转/持有）都在 unsafe 内
- 安全代码永远见不到指针类型（变量声明、参数、字段都不行）
- 生命周期责任全在程序员（自证）：不引入借用检查（既定决策）

### 3.5 U1：ref 形参 = 指针的安全语法糖

```tie
// 安全通道：ref 按引用传参（编译器保证写回，无需 unsafe）
func fill(x: ref table<i64>) { ... }     // 现状 T0.3，升级为通用 ref T

// unsafe 通道：显式指针（需 unsafe）
unsafe {
    var p: ptr<i64> = addr_of(x)
}
```

- `ref table<T>` 升级为 **`ref T` 通用按引用传参**（任意类型）
- IR 层 = 指针传递（现状已如此），但**安全代码可用 ref**（编译器保证语义）
- 分界：安全世界用 ref（编译器管理写回），unsafe 世界用 ptr（程序员管理一切）
- 同一底层（指针），两个通道（安全/unsafe）

## 4. repr(C) 结构体（R1：显式标注）

```tie
// 显式声明 C ABI 内存布局（字段顺序、对齐、无 padding 优化）
repr(C) struct WndClassW {
    var style: u32
    var lpfnWndProc: ptr
    var cbSize: i32
    ...
}
```

- 只有标注 repr(C) 的 struct 才是 C 布局；其余 struct 编译器自由布局（可优化）
- 布局规则：字段按声明顺序、自然对齐、符合 C ABI
- 需要标量整数类型 u8/u16/u32/i8/i16/i32 支持（现只有 i64/f64/bool）——
  **前置：窄整数类型**（unsafe 互操作必需）
- repr(C) struct 可用于 extern 参数（按引用传递，见 §5）

## 5. extern 与 unsafe（E3：声明标注 + 调用必须 unsafe）

```tie
// 声明处标注 unsafe
unsafe extern fn system(cmd: string) -> i32;

// 调用处必须 unsafe 上下文
unsafe {
    var code = system(cmd)
}
// system(cmd)  ← 安全代码直接调用 = 编译错误
```

- **E3 = 强制**：extern 声明必须带 unsafe 标注，调用必须在 unsafe 上下文内
- **不考虑存量兼容**（2026-08-15 决策）：无用户，直接强制；std/process.tie
  等现有 extern 声明加 unsafe 标注、封装函数标记 unsafe fn 或内部 unsafe 块
  ——一次性改造；未来用户迁移用预处理脚本（--module 机制）
- 扩展：extern 支持 ptr 参数（i64 地址透传）+ repr(C) 结构体按引用传递
  （解决"指针/结构体不能传"的 T0.7 限制清单）

## 6. 原子操作（A1：语言级 atomic<T>）

```tie
unsafe {
    var c: atomic<i64> = 0
    c.fetch_add(1, Relaxed)                    // 读-改-写
    var old = c.compare_exchange(expect, new, AcqRel)  // CAS
    var v = c.load(Acquire)
    c.store(42, Release)
}
```

- `atomic<T>` 语言内建，第一版只支持 **i64 / f64 / bool**（够 channel 底层）
- memory_order 枚举：Relaxed / Acquire / Release / AcqRel / SeqCst
- 原子类型不可复制/移动（防撕裂）
- 编译器发射 LLVM 原子指令（atomicrmw/cmpxchg/load/store + ordering）
- wasm 目标：原子操作 → wasm atomics（SharedArrayBuffer 场景）或编译错误
  （无共享内存时，见并发模型 §5.9）

## 7. 内联汇编（I1：语言级 asm! 宏）

```tie
unsafe {
    // 协程上下文切换（栈式协程核心，~50 行汇编）
    asm!("
        mov {0}, rsp
        mov rsp, {1}
        ...
    ", out(reg) saved_sp, in(reg) new_sp)

    // 嵌入式寄存器操作
    asm!("msr {0}, {1}", in(reg) reg_id, in(reg) value)
}
```

- 语法：`asm!(模板, 操作数...)`（Rust 风格）
- 操作数：`in(reg)` / `out(reg)` / `inout(reg)` / `out("memory")` 等
- 编译器发射 LLVM inline asm（llvm.inlineasm / inline asm 语法）
- 用途：栈式协程上下文切换（tieuicore 的 switch_context 可直接在 tie 写）、
  嵌入式硬件操作（寄存器/MMIO）、性能关键路径
- 平台相关：asm 模板按 target 条件编译（`#[target(arch)]` 或 cfg 机制）

## 8. 手动内存（M1：语言级 alloc/free）

```tie
unsafe {
    var p: ptr<u8> = alloc(1024)      // 分配（unsafe）
    memcpy(p, src, 1024)
    free(p)                            // 释放（unsafe）
}
```

- `alloc(n: i64) -> ptr<u8>` / `free(p: ptr<u8>)`
- 底层实现：对接 libc malloc（桌面）或自有分配器（嵌入式静态池）
- 泄漏/悬垂/双重释放：unsafe 内程序员自证（编译器不检查）
- 与 arena 的关系：arena 是安全通道（区域释放），alloc/free 是 unsafe 通道
  （手动管理）——分层内存全景（ui-framework.md §4.3）的"手动堆"层

## 9. 安全边界总结

```
┌─ 安全代码（默认）─────────────────────────────┐
│  i64/f64/bool/string/table/struct/enum         │
│  ref 按引用传参（编译器保证）                    │
│  arena 区域分配（区域释放）                     │
│  闭包/接口（编译器管理）                        │
├─ unsafe 边界（编译器强制检查）─────────────────┤
│  指针/切片创建、读写、算术、cast                 │
│  repr(C) struct、extern 调用、atomic、asm!     │
│  alloc/free                                    │
└────────────────────────────────────────────────┘
```

**规则**：安全代码调用任何 unsafe 能力 = 编译错误；
unsafe 代码可以调用安全代码（向下兼容）；
unsafe 边界清晰、无隐式通道。

## 10. 编译器实现拆解（tiec 自举）

| 模块 | 改动 |
| --- | --- |
| lexer/parser | `unsafe` 关键字（fn/块）、`type tie<unsafe>` 角色、`repr(C)` 标注、`asm!` 语法、`atomic<T>` 类型语法 |
| 类型系统 | `ptr<T>`、`slice<T>`、`atomic<T>`、窄整数（u8/u16/u32/i8/i16/i32） |
| semantic | unsafe 边界检查（安全代码触底报错）、ref T 通用化、repr(C) 校验（字段类型限制） |
| irgen | 指针操作指令（addr_of/deref/算术/cast）、原子指令、inline asm 发射、alloc/free 对接 |
| llvmgen | LLVM 指针类型/原子指令/inline asm/内存原语发射 |
| wasm 后端 | 指针 → 线性内存地址；原子 → wasm atomics 或报错；asm! → 目标不支持报错 |
| 嵌入式 | alloc/free → 静态池；asm! → 目标架构支持（ARM/riscv 内联汇编） |

## 11. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| unsafe 语法 | U3：块/函数为主 + 文件级逃生舱（type tie\<unsafe\>） | 纯块、纯文件级 |
| 指针类型 | T2 类型化 `ptr<T>` + T4 切片 `slice<T>` | 裸 ptr、引用 &T |
| 操作集 | O3 全量：标准集 + cast + volatile + 对齐 | 最小集、标准集 |
| 指针安全 | S1：指针一切操作归 unsafe | 分级（创建 unsafe/读安全） |
| ref 统一 | U1：`ref T` 通用按引用 = 指针安全语法糖 | 保持 ref table 专用 |
| 结构体布局 | R1：显式 repr(C)（其余编译器自由布局） | 全 C 布局 |
| extern 安全 | E3：声明标注 unsafe + 调用必须 unsafe | 保持现状（可直接调） |
| 原子 | A1：语言级 `atomic<T>`（i64/f64/bool 起步） | 原语级（tieuicore 提供） |
| 内联汇编 | I1：语言级 `asm!` 宏（LLVM inline asm） | 外部 .s、tieuicore 封装 |
| 手动内存 | M1：语言级 alloc/free（unsafe 面） | 仅 arena + extern malloc |

## 12. 未决问题

1. ~~**窄整数类型**~~ **已定案**（2026-08-15）：i8/i16/i32/u8/u16/u32/u64/f32 完整
   落地（L2+L3 字面量 + C2+C3 转换 + O3 溢出 + A1 算术 + B2 移位），
   详见 [int-model.md](int-model.md)。
2. **unsafe fn 调用安全代码**：规则确认（允许，向外安全）——细化边界语义
3. **文件级 unsafe 的混合**：type tie\<unsafe\> 文件能否包含安全函数（export 出去）
4. **asm! 的条件编译**：平台相关模板的声明机制（`#[target(arch)]` 或 cfg）
5. **alloc 失败处理**：返回 null？panic？断言？（嵌入式无异常，需定策略）
6. **volatile 与内存模型**：volatile 读写与编译器优化的交互（禁止重排/合并）
7. **ptr 的 Send/Sync 语义**：并发模型下指针能否跨协程传递（第一版建议禁止，
   只传所有权值）
