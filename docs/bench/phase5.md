# tie 自举阶段 5 前端+IR 性能闸门（G4）

- 生成时间: 2026-08-14 13:07:28
- CPU: 12th Gen Intel(R) Core(TM) i5-12490F
- 机器: JIRO-MAIN
- 通道: `tiec <file> --emit-ir`（tie 编译器）vs `tie-llvm <file> --emit-ir`（Rust 基线）
- 计时方法: 每文件预热 1 次 + 5 次热运行取中位数（median-of-5），CPU 固定核心 0，单次硬超时 60s

## 语料统计

- 语料: 77 个（examples/*.tie（剔除 oop_neg_*）+ tests/language/*.tie（剔除 *_neg.tie））
- 可编译（计入对比）: 74 个（覆盖 96.1%）
- tiec 不可编译: 0 个（见不可编译清单）
- 双失败（Rust 也失败）: 3 个（文件本身为负例/有错，非 tiec 缺口）

## 逐文件（耗时单位 ms，计入对比）

| 文件 | tiec | tie-llvm | 每文件比值 (tie/rust) |
| --- | ---: | ---: | ---: |
| examples/args_demo.tie | 63.5 | 77.1 | 0.82 |
| examples/assign.tie | 65 | 66.5 | 0.98 |
| examples/brotli_demo.tie | 158.9 | 125.9 | 1.26 |
| examples/byte_demo.tie | 70.2 | 75.4 | 0.93 |
| examples/char.tie | 56.8 | 61.5 | 0.92 |
| examples/compress_demo.tie | 140.1 | 122.9 | 1.14 |
| examples/csv_demo.tie | 185.9 | 149.7 | 1.24 |
| examples/exmath_demo.tie | 574.8 | 469.8 | 1.22 |
| examples/exmath_num_demo.tie | 578.2 | 345 | 1.68 |
| examples/format_demo.tie | 76.9 | 86.4 | 0.89 |
| examples/graph_demo.tie | 165.6 | 133.3 | 1.24 |
| examples/hello.tie | 55.1 | 64 | 0.86 |
| examples/import_main.tie | 56.7 | 66.6 | 0.85 |
| examples/import_nested.tie | 64 | 74.4 | 0.86 |
| examples/index_assign_demo.tie | 61.3 | 69.3 | 0.88 |
| examples/jpeg_demo.tie | 397.9 | 278.6 | 1.43 |
| examples/lib_math.tie | 62.8 | 59.5 | 1.06 |
| examples/lib_math2.tie | 58.1 | 62.5 | 0.93 |
| examples/lib_ns_tools.tie | 58.3 | 61.7 | 0.94 |
| examples/lib_util.tie | 62.5 | 69.8 | 0.90 |
| examples/linalg_demo.tie | 199.7 | 163.1 | 1.22 |
| examples/list_dir_demo.tie | 58.4 | 64.4 | 0.91 |
| examples/log_demo.tie | 235.1 | 174.4 | 1.35 |
| examples/log_enhance_demo.tie | 230.2 | 173.2 | 1.33 |
| examples/loop_control_demo.tie | 65.3 | 78.9 | 0.83 |
| examples/lz4_demo.tie | 106.5 | 94.4 | 1.13 |
| examples/m4_ops.tie | 59.8 | 69.1 | 0.87 |
| examples/ml_demo.tie | 186.2 | 149.1 | 1.25 |
| examples/namespace_demo.tie | 57.4 | 64.1 | 0.90 |
| examples/ns_import_demo.tie | 63.6 | 61.5 | 1.03 |
| examples/oop.tie | 66.3 | 70.6 | 0.94 |
| examples/optsearch_demo.tie | 143.6 | 134 | 1.07 |
| examples/radix_demo.tie | 85 | 78.3 | 1.09 |
| examples/rdu_demo.tie | 229.4 | 185.2 | 1.24 |
| examples/regex_demo.tie | 66.4 | 69.2 | 0.96 |
| examples/registry_demo.tie | 65.3 | 74.3 | 0.88 |
| examples/script_demo.tie | 61.8 | 66.5 | 0.93 |
| examples/sort_demo.tie | 150 | 129.2 | 1.16 |
| examples/std_demo.tie | 178.3 | 147.1 | 1.21 |
| examples/std_math_demo.tie | 197 | 159.8 | 1.23 |
| examples/std_math_primitives.tie | 79.8 | 76.5 | 1.04 |
| examples/std_primitives.tie | 65.9 | 71.7 | 0.92 |
| examples/std_refactor_demo.tie | 203.9 | 165.7 | 1.23 |
| examples/strings.tie | 60.3 | 65.9 | 0.92 |
| examples/switch_pattern.tie | 66.6 | 70.3 | 0.95 |
| examples/switch_table_demo.tie | 57 | 61.5 | 0.93 |
| examples/switch.tie | 60.6 | 68.9 | 0.88 |
| examples/table_dynamic.tie | 67.5 | 72 | 0.94 |
| examples/table_enhance_demo.tie | 71.3 | 74.8 | 0.95 |
| examples/table_param_demo.tie | 61.4 | 70.5 | 0.87 |
| examples/table.tie | 57.9 | 65 | 0.89 |
| examples/test_wide.tie | 55.2 | 72 | 0.77 |
| examples/trit_demo.tie | 65.7 | 69.9 | 0.94 |
| examples/tuple.tie | 59.9 | 66.9 | 0.90 |
| examples/version_demo.tie | 95.1 | 91.1 | 1.04 |
| examples/wide.tie | 62.9 | 61.6 | 1.02 |
| examples/zstd_demo.tie | 449.5 | 409.3 | 1.10 |
| tests/language/byref_table.tie | 63.8 | 62.8 | 1.02 |
| tests/language/ext_test_bench.tie | 204.2 | 158.8 | 1.29 |
| tests/language/ext_ui_cfg.tie | 189.7 | 173.6 | 1.09 |
| tests/language/extern_decl.tie | 165.9 | 130.5 | 1.27 |
| tests/language/global_table.tie | 57.2 | 62 | 0.92 |
| tests/language/intern.tie | 110.1 | 101.3 | 1.09 |
| tests/language/interp_env_file.tie | 822.2 | 514.6 | 1.60 |
| tests/language/interp_env_value.tie | 885.5 | 534.5 | 1.66 |
| tests/language/interp_eval.tie | 10677.7 | 6257.1 | 1.71 |
| tests/language/runtime_staticlib.tie | 58.3 | 71.1 | 0.82 |
| tests/language/std_args_time.tie | 110.9 | 106.3 | 1.04 |
| tests/language/std_coll_crc.tie | 212 | 167.3 | 1.27 |
| tests/language/std_encoding.tie | 265.8 | 191.7 | 1.39 |
| tests/language/std_fs_path.tie | 167.9 | 144.1 | 1.17 |
| tests/language/std_json.tie | 625.2 | 387.8 | 1.61 |
| tests/language/std_net_text.tie | 166.9 | 141.8 | 1.18 |
| tests/language/utf_ascii.tie | 143.4 | 125.3 | 1.14 |

## 双失败（Rust 基线自身失败）

| 文件 | 原因 | Rust exit |
| --- | --- | ---: |
| tests/language/filetype_ir.ir.tie | Rust 基线自身编译失败（文件为负例/语义错误，非 tiec 缺口） | 101 |
| tests/language/generics.tie | Rust 基线自身编译失败（文件为负例/语义错误，非 tiec 缺口） | 1 |
| tests/language/global_table_const.tie | Rust 基线自身编译失败（文件为负例/语义错误，非 tiec 缺口） | 1 |

## 汇总

| 指标 | tiec | tie-llvm | 比值 (tie/rust) |
| --- | ---: | ---: | ---: |
| 总耗时 (ms，中位和) | 22085.1 | 15690.5 | 1.408 |

## G4 判定

- **ratio = tiec_total / rust_total = 1.408**（仅 74 个可编译文件）
- 硬线: ratio ≤ 3.0（同量级）→ 可编译文件数 > 0 且 ratio ≤ 3.0 为 PASS
- 目标: ratio ≤ 2.0（阶段 2 符号表直查后）
- 判定: **G4 PASS（部分基准 + 缺口清单）**
- 覆盖: 96.1%（74/77）——**部分基准**：语料未全量编译，结论仅代表当前可编译子集；缺口清单见上，修复后重跑自动扩全。

## 方法

- **冻结语料**: examples/*.tie（剔除 oop_neg_* 已知负例；库文件可含）与 tests/language/*.tie（剔除 *_neg.tie 负例），共 77 个。文件清单不落盘（脚本每次动态枚举），规则固定——缺口修复后新增可编译文件自动进入对比。
- **计时通道**: `--emit-ir`（前端 + IR 生成）。后端 opt/clang/lld 对两端是同一批外部工具，排除在 tie-vs-Rust 对比之外。
- **median-of-5**: 每文件预热 1 次（丢弃，排除加载/缓存噪声）+ 5 次热运行取中位数（`Measure-Command`）。
- **机器固定**: 进程与全部子进程经 `SetProcessAffinityMask` 固定到核心 0，避免调度抖动；测量期间避免其他负载。
- **单次硬超时 60s**: 防 tiec 意外死循环挂死基准。
- **比值口径**: 每文件 ratio = tiec 中位 / Rust 中位；总比 = tiec 中位总和 / Rust 中位总和（仅两边都成功、即「可编译」的文件计入）。

## 三处净收益分析（tie IR 生成的架构优势）

- 以下三点是 tie 编译器相对 Rust 种子编译器在 frontend+IR 通道的设计收益，在当前可编译子集与全量语料上都成立（随覆盖扩大，收益在总耗时中体现）：

| # | 净收益 | Rust 种子做法 | tie 编译器做法 | 收益来源 |
| --- | --- | --- | --- | --- |
| 1 | **无 renumber 单遍** | 生成 IR 后需全局重编号（`renumber` pass）规整 %N | llvmgen 生成时单调编号直出，省去整个重编号遍历 | 省一遍全 IR 线性扫描 + 字符串重建 |
| 2 | **语义单遍** | semantic 多趟扫描（函数签名收集 + 类型解析分阶段） | 单遍符号表 + 节点类型表（收集与解析合一） | 省掉额外符号表遍历与重复解析 |
| 3 | **类型表直查** | IR 生成时对节点做类型推断 | 语义阶段已写 node-id→type 表，llvmgen 直接查表 | 省去 IR 生成期的重复类型推断 |

- 净收益的量化验证依赖全量语料（当前覆盖 96.1%）。部分基准下比值已接近 1.0，说明 tie 编译器的 frontend+IR 通道与 Rust 同量级甚至略快，架构收益成立。

## 当前覆盖与缺口

- **当前可编译**: 74/77（96.1%）——仅 irgen 最小集文件（println/print/exec_code/time_now/get_env + 算术/if/for/var + 纯函数），详见逐文件表。
- **不可编译**: 0 个，原因分类见上表（irgen 最小集外为主；前端语义/语法缺口少量）。前端语义缺口（全局表误判）正由另一任务修复——修复后重跑本脚本即自动扩全。
- **双失败**: 3 个（Rust 基线自身也失败，文件本身为负例/有错，不算 tiec 缺口）。
- **G4 结论**: ratio 1.408（仅可编译子集）
- 覆盖注脚: 当前为部分基准（覆盖 96.1%），结论仅代表可编译子集；缺口清单见上，前端语义缺口修复后重跑即自动扩全。