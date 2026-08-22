# 规划：irgen（AST → tie-IR）按 LLVM 风格多层重构

> 状态：规划中
> 所属：编译器解耦/模块化（对比 LLVM 的三段式后端：前端→中端→后端）
> 目标文件：`compiler/backend/irgen.tie`（主文件当前约 7700 行，平铺）

## 1. 背景与动机

irgen 是把「前端 AST（sstate 列式表）→ tie-IR（ir 列式表）」的后端生成器。
与 LLVM 后端分层（SelectionDAG → ISel → CodeGen → MC）同理，irgen 内部
也可以、也应该按**职责分层**，而非一个超大的平铺文件。

当前 irgen 主文件（约 7700 行）所有生成函数平铺在一个 `namespace irgen`
内，职责没有横向分界。已拆出三个**纵向切片**（按特性而非按层）：
`irgen_str`（s21 原语）、`irgen_vtable`（port）、`irgen_closure`（闭包）。

本次重构补齐**横向分层**：把平铺的生成函数按「LLVM 风格的分层」重编为
多个文件，每层一个文件、职责单一、依赖方向清晰（上层调用下层）。

## 2. LLVM 分层对应

| LLVM 后端 | tie-IR 相应阶段 | 职责 |
| --- | --- | --- |
| TargetMachine | gen_fn_def / gen_program | 目标/模块级驱动 |
| IRBuilder（Builder） | scope_*/loop_*/ir.new_inst 封装 | 指令构造原子 |
| Constant / Type | gc？ gen_*_lit / 类型工具 | 字面量与类型编码 |
| Lowering / DAG | gen_binary / gen_*_bin / gen_checked | 算术/比较/溢出 |
| ISel / visitExpr | gen_expr 调度器 + 各生成函数 | 表达式→tie-IR |
| visitStmt / Block | gen_stmt 调度器 + 各语句生成 | 语句/控制流 → tie-IR |
| 聚合类型处理 | gen_agg_* / gen_struct_* / gen_table_* | 元组/struct/enum/table |
| BOLT/ABI | extern 声明 / gen_user_call / 方法分派 | 调用约定与符号 |

## 3. 目标分层（本重构要建立的层）

| 层 | 职责 | 建议文件 | 主要成员 |
| --- | --- | --- | --- |
| L0 Builder 层 | 作用域/循环栈/指令构造原子 | `irgen_builder.tie` | scope_put/get/push/pop、loop_*、string_reg、gen_var_ref、gen_block_stmts |
| L1 字面量层 | 常量与类型编码 | `irgen_lit.tie` | gen_int/str/float/char/trit/bool_lit、str_unescape、gen_ternary |
| L2 算术层 | 二元/一元运算、比较、checked | `irgen_arith.tie` | gen_binary、gen_binary_float、gen_trit_bin、gen_widen/coerce/narrow、gen_checked、gen_icmp_c 族 |
| L3 表达式层 | 表达式生成（主调度） | `irgen_expr.tie` | gen_expr、builtin_expr、is_builtin_name、gen_runtime_call、gen_index_read |
| L4 语句/控制流 | 语句生成、控制流、打印 | `irgen_stmt.tie` | gen_stmt、gen_switch、gen_for_*、gen_index_assign、gen_println、gen_block 分支 |
| L5 聚合结构 | 元组/struct/enum/table/map 字面量与字段 | `irgen_agg.tie` | gen_tuple_lit、gen_struct_construct、gen_field_access/assign、gen_agg_*、gen_table_lit、gen_enum_construct |
| L6 调用分派/ABI | 用户调用、方法调用、extern 桥、atomic | `irgen_call.tie` | gen_user_call(_from)、gen_method_call、gen_ext_call*、gen_struct_method_call、gen_atomic_method |
| L7 顶层入口 | 模块/函数/全局驱动 | 主文件 `irgen.tie` | gen_ast、gen_src、gen_fn_def、gen_ns、gen_global_var、gen_program、set_target |

辅助：check函数层、杂项工具（ig_slice/ig_find_char/ig_ns_sym/find_var_refs）归入使用处或 L0。

> 注：L1-L7 与已有三个纵向切片（str/vtable/closure）正交，互不冲突。

## 4. 环的处理（关键设计决策）

传统分层会问「gen_expr 调 gen_stmt，gen_stmt 又调 gen_expr，如何无环？」
本重构的答案：**tie 的 import 是文本内联，且同 namespace 内「先调用后定义」
合法（先收集后生成语义）**。即：

- 各层文件声明**同名 `namespace irgen`**，主文件 import 各层文件，文本内联
  后全部函数进入同一编译单元；
- 因此「层间互相调用」在物理上**不存在闭环**——所有层函数最终在同一文本内，
  无 import 循环；
- 分层体现的是**逻辑职责**（谁做什么），而非「谁调谁的编译期约束」。

由此「严格无环」落地为：**保持现有调用关系不变（零逻辑改动），只按职责
把函数归到对应层文件**。分层完成后，层间依赖是清晰的方向（表达式层调用
算术层/字面量层，语句层调用表达式层），便于阅读与后续演进，但**不**强制
重写任何调用为接口/回调。

## 5. 实施步骤（每步一提交）

按依赖方向从底向上拆，每步独立验证（自举闭环 + 回归）再提交：

1. **S1** 拆 L0 Builder 层 → `irgen_builder.tie`
2. **S2** 拆 L1 字面量层 → `irgen_lit.tie`
3. **S3** 拆 L2 算术层 → `irgen_arith.tie`
4. **S4** 拆 L3 表达式层主体（builtin/runtime/index） → `irgen_expr.tie`
5. **S5** 拆 L4 语句/控制流层 → `irgen_stmt.tie`
6. **S6** 拆 L5 聚合结构层 → `irgen_agg.tie`
7. **S7** 拆 L6 调用分派/ABI 层 → `irgen_call.tie`
8. **S8** 收束主文件，确认仅剩顶层入口驱动
9. **验收**：自举闭合 hash 一致 + regress-s21 全绿 + 编译零错误

每个 S 步的完成标准：
- `tiec driver.tie -o …` 一阶编译成功（exit 0）；
- 该步后的 tiec 再编译 driver.tie 二阶成功；
- irgen 主文件行数下降、层文件职责单一；
- 零逻辑改动（纯搬运，可 git diff 核对）。

## 6. 验收标准（全任务）

- [ ] irgen 主文件从约 7700 行降到「仅顶层驱动」规模（< 1000 行量级）；
- [ ] 新增 8 个层文件，每层职责单一、命名语义清晰；
- [ ] 一阶 + 二阶自举闭环 hash 一致；
- [ ] `regress-s21.ps1` 全量 PASS=79（FAIL=2 为既有基线）零新增回归；
- [ ] 编译零错误；
- [ ] 每步一个独立提交，信息含该层说明 + 验证结果。

## 7. 相关文件

| 文件 | 作用 |
| --- | --- |
| `compiler/backend/irgen.tie` | 目标重构对象（主文件，最终仅顶层驱动） |
| `compiler/backend/irgen_str/vtable/closure.tie` | 既有纵向切片（保持不变） |
| `compiler/middle/ir.tie` | tie-IR 表访问（ir.* 命名空间） |
| `compiler/backend/llvmgen.tie(+str/inst)` | IR→LLVM 的下游消费者（不动） |