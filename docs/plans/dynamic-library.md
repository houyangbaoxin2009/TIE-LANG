# 规划（Harbor M5）：动态库编译（DLL / .so）

> 状态：**规划**（已排期，待实现）
> 所属：Harbor（2026.1）架构 M5
> 依赖：Harbor M4（标准库重构）已完成；动态库是库编译（M3 已完成 `.a` 静态库）的延伸

## 1. 背景与现状

tie 已有库编译能力（M3 完成）：

- `// tie:library` 角色 → 静态库 `.a`（LLVM IR → opt → clang -c → llvm-ar rcs）；
- tie 程序消费库的方式是 **import 源码展开**（跨文件内联），不链接 `.a`；
- `.a` 可作为 LLVM 目标码被 C 工具链链接（无导出符号规范、无头文件）。

**缺口**：不能编译为**动态库**（Windows `.dll` / Linux `.so`）——
C/其他语言无法在运行期加载 tie 编写的库（如插件体系、跨语言模块）。

## 2. 目标

`tie lib_xxx.tie`（`// tie:library`）可输出动态库：

- Windows：`lib_xxx.dll`（+ 可选 `.lib` 导入库）；
- Linux：`lib_xxx.so`（`-fPIC -shared`）；
- 导出符号：库的**公有函数**（顶层函数 / `pub func` 命名空间函数）带 `dllexport`；
- 跨语言调用约定：标量（i64/f64/bool/char）直接传；字符串按 C 字符串指针 +
  `tie_free_result` 释放约定（复用 tie-interp C ABI 桥范式）；表/struct 暂不跨边界
  （内部堆结构，文档明示限制）。

## 3. 设计

### 3.1 CLI

```
tie lib_math.tie -o lib_math.dll        # 显式 .dll/.so 扩展 → 动态库模式
tie lib_math.tie --shared               # 或显式开关
```

- 输出扩展名 `.dll`（win）/ `.so`（linux）识别为动态库模式；
- 角色仍为 `// tie:library`（无 main）。

### 3.2 IR 层

- 库入口函数（顶层函数与命名空间 `pub func`，符号 `ns_symbol` 全名转 `$`）
  定义标记 `dllexport`：`define dllexport i64 @mathlib$add(...)`；
- 内部辅助函数（私有函数）不导出；
- 字符串返回：保持 tie 内部堆串表示，跨语言调用方用 C ABI 桥释放约定
  （或导出 tie_free_result 供调用方释放）。

### 3.3 driver 层

- 动态库分支：`clang -shared`（win：`lld-link /DLL` 或 clang 驱动自动）+
  `.def`/dllexport 符号；Linux：`-fPIC -shared`；
- 复用现有 opt/clang/lld 调用链，仅后端标志差异；
- `--target` 交叉动态库（win-x64 → .dll 等）。

### 3.4 示例与测试

- `examples/lib_math_dyn` 演示：tie 写库 → C 程序（`LoadLibrary`/`dlopen`）加载调用；
- 端到端测试：编译 .dll → 符号导出检查（dumpbin/llvm-nm）→ C 侧调用冒烟；
- workspace 测试全绿。

## 4. 不做（明确排除）

- 表/struct 跨 DLL 边界传递（内部堆布局，留待 C ABI 规范设计）；
- 动态库的热加载/插件 ABI 版本管理；
- 生成 C 头文件（调用方手写声明，文档给示例）。

## 5. 验收标准

- `tie xxx.tie -o xxx.dll`（win）产物可被 C 程序 LoadLibrary + GetProcAddress 调用；
- 标量/字符串参数与返回的导出函数跨语言行为正确；
- Linux `.so` 同流程通过（CI 或文档注明未测平台）；
- workspace 编译零错误、测试全绿。

## 6. 影响范围

| 组件 | 影响 |
| --- | --- |
| tie-llvm（driver.rs） | 动态库后端分支（-shared/导出） |
| tie-llvm（ir.rs） | 库函数 dllexport 标记 |
| tie（CLI） | -o 扩展名识别 / --shared 开关 |
| docs/ | CLI 用法、库编译章节、示例 |
| scripts/package.ps1 | 发行版收录示例 |
