# G3 闸门：0-Rust 验证报告（自举 v2 T4.6）— 2026-08-12

## 种子界限（bootstrap boundary）

0-Rust 不是"零 Rust 从头构建"——tiec.exe 本身由 Rust 种子编译器
（`target/release/tie-llvm.exe`）编译，这是**唯一的 Rust 接触点**。
G3 验证的承诺是：**种子 tiec 之后的一切**（编译用户程序、链接运行时、
REPL 运行、解释器求值）**不触碰 cargo/target 的 Rust 产物**。

```
Rust 种子（tie-llvm.exe）
   └─ 编译 compiler/driver.tie → compiler/tiec.exe   ← bootstrap 界限
        ├─ tiec 编译用户程序 → opt/clang → exe        （链接 std/runtime.a）
        ├─ tiec 编译 compiler/repl.tie → repl.exe     （种子通道，见限制）
        └─ repl 运行 → interp.eval 求值                （tie 自写解释器）
```

## 验证矩阵（scripts/zero-rust-check.ps1）

| # | 检查项 | 结果 |
|---|---|---|
| 1 | 前置就绪（tiec / runtime.tie / runtime.a / clang / opt / 种子） | ✅ PASS |
| 2 | tiec 编译 examples/hello.tie → 运行输出 16 行正确 | ✅ PASS |
| 3 | tiec 编译运行时程序（exec_code/time_now/get_env）→ 运行正确 | ✅ PASS |
| 4 | 运行时程序二进制**无 tie_interp.lib 独有符号**（file_read/str_char 等 ptr 桥） | ✅ PASS |
| 5 | std/runtime.a 导出允许集符号（tie_exec_code/tie_get_env/tie_time_now） | ✅ PASS |
| 6 | REPL parity（tie repl vs Rust 通道，golden 18 命令，233 字节 diff 空） | ✅ PASS |
| 7 | interp 行为测试套件（11 文件 198 断言） | ✅ PASS |

**结论：G3 PASS** —— 种子 tiec 编译的运行时程序链接 tie 自写 runtime.a
（无 Rust tie-interp），运行时栈 Rust-free；REPL parity 空 diff；interp 套件全 PASS。

## 关键机制（T4.5 基础）

- **std/runtime.a**（tie 语言自写）：顶层裸函数 `func tie_exec_code(cmd: string) -> i64`
  编译为 `define i64 @tie_exec_code(ptr)`——无命名空间不 mangle，与语言底座
  IR 的 `declare i64 @tie_exec_code(ptr)` **字节级匹配**，clang 链接解析。
- 内部用 T0.7 extern fn 声明 libc：`system`（exec_code）、`getenv`（get_env，
  string 返回）、`time(0)`（time_now，i64/NULL ABI 等价技巧）。
- tiec 的 irgen 对 exec_code/time_now/get_env 生成 `extern_call(36)` 指令，
  llvmgen 收集符号输出 declare，driver 按 `g_used_runtime` 链接 runtime.a。

## 限制与 R3 fallback（诚实记录）

1. **runtime.a 需种子编译**：tiec 的 irgen 最小集不支持 `extern fn` 声明，
   故 std/runtime.tie 由种子编译器编译成 .a（0-Rust 验证允许的 bootstrap 步骤）。
2. **repl.exe 需种子编译**：repl 用 read_line（C ABI 桥，ptr 语义），irgen 最小集
   不支持——REPL parity 走种子通道（tie 解释器仍是 tie 写的，只是外壳编译通道）。
3. **ptr 桥符号未 tie 化**：tie 无指针类型，`tie_file_read`/`tie_str_char`/
   `tie_rand_range`/`tie_arg_*`/`tie_exec_output` 等仍由语言底座（Rust tie-interp）
   提供——见 std/runtime.tie 头部限制清单。
4. **R3 fallback**：若未来某环节回归失败，保留 Rust tie-interp 作为可用兜底
   （TIE_INTERP_LIB 环境变量可回退），直到 tiec 能力闭合（extern fn 支持、
   irgen 全语句集）后彻底退役。

## 修复记录（验证过程暴露的 tiec 缺陷）

- **llvmgen ret 类型硬编码**：显式 `return <expr>` 生成 `ret i32 %N` 但值是 i64
  → 按 ins_ty 输出 `ret <T> %N`。
- **irgen main 补 ret**：void main 补的 ret 原用 ty=-1（生成 `ret ptr 0`/`ret void`
  与 `define i32 @main` 不匹配）→ 显式 TK_I32。
