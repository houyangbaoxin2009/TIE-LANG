# 编译器解耦任务——移交与衔接计划

> 状态：irgen 分层（S1–S8）+ 宏子系统解耦（宏 S1–S3）已全部完成，均已交付并推送 origin/main
> 日期：2026-08-22 / 2026-08-23（宏解耦）
> 交接对象：接手继续实施 `compiler/` 解耦/分层重构的人
> 位置：`f:\Projects\tie\compiler\`

---

## 1. 一句话总览

把 tie 自写编译器 `compiler/` 从"巨型平铺单文件"重构为「按职责分层、每层一个
文件、同 namespace 跨文件」的模块化结构，最终目标是 **irgen 按 LLVM 风格
分层 + 全局 `tig_` 命名统一**。所有改动遵循「零逻辑改动、纯搬移/重命名、
每步独立验证并提交」的纪律。

已确立并反复验证的核心语言机制：

> **tie 的 import 是文本内联**：被 import 的文件并入同一编译单元，一切符号
> 全局可见；**同名 `namespace` 可横跨多个文件闭合并集**；同 namespace 内
> 函数可互相裸名调用、可访问主文件顶层全局 var、「先调用后定义」合法。
>
> 因此：把大文件的函数按职责搬到多个子文件（每个声明同名 namespace），
> 主文件 import 它们即可，**零逻辑改动**。

---

## 2. 已完成的全部提交（已推送 origin/main）

| 提交 | 内容 |
| --- | --- |
| `1173f9b` | llvmgen(2721→1036) 拆3文件：主+llvmgen_str+llvmgen_inst |
| `eb2daf7` | irgen 内联原语区 → irgen_str（字符串/表/字典/数字转串，1867行） |
| `9bdad56` | irgen vtable/port 后端 → irgen_vtable（并纠正 gen_fn_def 等归属） |
| `8b9c176` | irgen 闭包后端 → irgen_closure |
| `42d727f` | interp 二元运算 → interp_bin |
| `512e0ae` | interp 函数调用 → interp_call |
| `e757842` | sinfer ?解包区 → sinfer_ret |
| `75e2328` | sgen 模板实例化 → sgen_inst |
| `b24ec87` | scollect 顶层收集实现 → scollect_port |
| `b945bc9` | smove 语句检查 → smove_stmt |
| `d274edf` | semantic 泛型预扫描 → semantic_gen |
| `555e8a0` | **irgen 分层 S1**：L0 Builder 层 → irgen_builder |
| `8f6f7e6` | **irgen 分层 S2**：L1 字面量层 → irgen_lit |
| `2720cd5` | **irgen 分层 S3**：L2 算术/比较层 → irgen_arith |
| `cdeb613` | **irgen 分层 S4**：L3 表达式层 → irgen_expr |
| `128e29a` | **irgen 分层 S5**：L4 语句/控制流层 → irgen_stmt |
| `7d34252` | **irgen 命名统一**：91 个 gen_* → tig_*（tie-ir gen），新增本移交文档 |
| `f8db886` | **irgen 分层 S6**：L5 聚合结构层 → irgen_agg |
| `1b8a1a5` | **irgen 分层 S7**：L6 调用分派/ABI 层 → irgen_call |
| `b47a08e` | **irgen 分层 S8**：收束主文件（runtime 片 → irgen_rt），主文件 7700→1010 行 |
| `0c1ef29` | **宏解耦 S1**：宏执行入口（eval_proto/eval_macro/eval_macro_proc/code 执行）→ interp_macro |
| `d938cda` | **宏解耦 S2**：code 值构造（gen_code_lit/卫生/插值/协议拼接）→ interp_code |
| `f19ec93` | **宏解耦 S3**：M4 token 流工具（expr_from_wrap/转义/parse_tokens/deparse）→ interp_macro |

每步验证：一阶自举 `tiec driver.tie` + 二阶自举 + `regress-s21.ps1` 全量
PASS=79（其中 2 个 FAIL 为历史基线 try_probe/shift_neg_free，SKIP 2 个为
既定基线），零新增回归。

---

## 3. 当前工作区状态（接手重点）

> 已解决（2026-08-22）：本交接点对应的 `tig_` 精确重命名已提交为 `7d34252`
> 并推送 origin/main；随后 S6–S8 分层亦已完成（见 §2），交接闭环。

**原交接点未提交的改动 = `tig_` 精确重命名（已完成、已验证、已提交 7d34252）**。

执行内容：把 **irgen 命名空间**的 91 个 `gen_*` 函数（含 pub `gen_ast`/
`gen_src`）改名为 `tig_*`（tie-ir gen）。基于 91 名**白名单** + 词边界精确替换，
涉及文件与次数：

| 文件 | 改名数 | 文件 | 改名数 |
| --- | --- | --- | --- |
| irgen.tie | 165 | irgen_stmt.tie | 103 |
| irgen_arith.tie | 136 | irgen_str.tie | 291 |
| irgen_builder.tie | 0 | irgen_vtable.tie | 31 |
| irgen_closure.tie | 27 | llvmgen.tie(注释) | 3 |
| irgen_expr.tie | 241 | llvmgen_inst.tie | 1 |
| irgen_lit.tie | 27 | _driver_test.tie | 5 |
| driver.tie | 7 | driver-lite.tie | 2 |
| compiler/README.tie | 3 | | |

**禁止误伤项（本次刻意保留）**：
- `llvmgen` 自身函数 `gen_func` / `gen_inst`（属 LLVM 生成器，非 irgen，保持 `gen_`）
- `interp` 命名空间的 `gen_expr`/`gen_call` 等（不属于 irgen，不在白名单）

已验证：一阶+二阶自举均 exit 0。**该重命名已提交并推送（commit `7d34252`）。**

> 注意：工作区的 `assets/make_social_preview.py`、`assets/social-preview.png`、
> 一堆 `tests/*/*.exe` 是回归噪音/无关改动，不要混入本次提交（只 `git add`
> 上表列出的 compiler 下文件）。

---

## 4. irgen 分层完成情况（按 docs/plans/irgen-llvm-layers.md）

分层 S1–S8 **全部完成**（对应提交见 §2）。最终职责分布：

| 层 | 文件 | 内容 | 状态 |
| --- | --- | --- | --- |
| L0 Builder | `irgen_builder.tie` | 作用域/循环栈/指令构造原子 | 已交（S1）|
| L1 字面量 | `irgen_lit.tie` | 常量与类型编码 | 已交（S2）|
| L2 算术 | `irgen_arith.tie` | 二元/比较/checked | 已交（S3）|
| L3 表达式 | `irgen_expr.tie` | 表达式生成主调度 | 已交（S4）|
| L4 语句/控制流 | `irgen_stmt.tie` | 语句/switch/for/打印 | 已交（S5）|
| L5 聚合结构 | `irgen_agg.tie` | 元组/struct/enum/table 字面量与字段访问 | 已交（S6）|
| L6 调用分派/ABI | `irgen_call.tie` | user_call/method_call/ext_call*/atomic | 已交（S7）|
| L7 顶层入口 | `irgen.tie` | 顶层驱动 + 全局状态 + 共享辅助 | 已交（S8）|
| 运行时控制 | `irgen_rt.tie` | try/panic/catch/cb_ptr/default_val | 已交（S8）|
| 纵向切片 | `irgen_str/vtable/closure` | 字符串/表/字典/port/闭包 | 既存 |

> **没有 S9**：规划到 S8 + 验收（第 9 步）为止。验收已满足——一/二/三阶自举
> exit 0 且二/三阶二进制 hash 一致（CLOSURE=STABLE）+ `regress-s21` 全量
> PASS=79（FAIL=2/SKIP=2 均为既定基线）+ 编译零错误。

> 命名约定：新拆函数一律沿用 `tig_*`（tie-ir gen）前缀，保持与 `gen_*`→`tig_*`
> 统一命名一致。分层文档 `docs/plans/irgen-llvm-layers.md` 中的函数名仍是 `gen_*` 旧名。

用户原始需求（可选后续）：
- **子任务A（已完成）**：宏↔解释器回环解耦前置步——宏子系统从 `interp`
  主文件拆分为独立文件 `interp_macro.tie`（执行入口 + M4 token 工具）+
  `interp_code.tie`（code 值构造/卫生/插值拼接），M4 token/转义工具从
  interp_call 归拢。零逻辑改动，宏子系统函数有清晰归属。结构回环
  `mexpand → interp → parser` 的源码可执行依赖仍存（interp 需 parser 供
  REPL eval + eval_code/eval_expr/tokenize 内置做源码解析），彻底斩断需
  eval/token 内置的源码解析入口改走协议文本或独立注入，属更深重构，另行
  立项。
- **剩余巨型文件**（>600 行）：scheck(1483)/sstate(1381)/semantic(主)/config(1414)/
  tieir_ser(753)/语法族(pstmt_top 1232/pexpr 1205/putil 860/pstmt_flow 660)。
  其中语法族、tieir_ser、sbuiltin 无清晰内部分区，不宜再拆；sstate 是全局状态
  池、config 是配置数据，拆分需谨慎/边际价值低。

---

## 5. 交接时必须知道的验证命令

```powershell
cd f:\Projects\tie
# 1) 一阶自举（验证改动可编译编译器自身）
& .\compiler\tiec.exe .\compiler\driver.tie -o .\_dec.exe          # 期望 exit 0
# 2) 二阶自举 + 闭合一致性（tiec 自编译自身 hash 相同）
& .\_dec.exe .\compiler\driver.tie -o .\_dec2.exe
# 3) 完整回归
& .\scripts\regress-s21.ps1 .\_dec.exe     # 期望 PASS=79 FAIL=2(既有基线) SKIP=2
Remove-Item .\_dec.exe,._dec2.exe -Force
```

`tiec.exe` 的编译大约 14 秒一次，可作快速迭代环路。替换泵（tigas）若中途
失败：读完整错误消息，通常是某个调用点没同步改名或 import 顺序/namespace
闭合问题，逐一修正即可，不要回归降级范围。

---

## 6. 关键纪律（防止再"疯"）

1. **一次只做一件事**：命名重命名或分层拆分，分开发吉与提交。
2. **每步动手前先 Read** 对应文件，确认边界（函数起始注释 / namespace 闭合），
   不臆测。
3. **零逻辑改动**：纯搬运/重命名，函数体一字不改；可用 `git diff` 核对。
4. **白名单替换**：凡改名，用明确名单 + 词边界，宁可漏改一次（编译会报）
   不可误伤（gen_func/gen_inst 就是教训）。
5. **每步独立验证 + 单独提交 + 推送**，提交信息含：这一步做什么、验证结果。
6. 大改前先并行运行子代理做**静态依赖分析**（谁调谁、有没有环），再设计。

---

## 7. 相关文档

| 文件 | 说明 |
| --- | --- |
| `docs/plans/irgen-llvm-layers.md` | irgen 7 层架构规划（含改名对照旧名 gen_*） |
| `docs/plans/self-hosting*.md` | 编译器自举背景 |
| `compiler/README.tie` | 编译器目录结构与当前进度 |