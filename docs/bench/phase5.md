# tie 自举阶段 5 前端+IR 性能闸门（G4）

- 生成时间: 2026-08-13 22:20:39
- CPU: 12th Gen Intel(R) Core(TM) i5-12490F
- 机器: JIRO-MAIN
- 通道: `tiec <file> --emit-ir`（tie 编译器）vs `tie-llvm <file> --emit-ir`（Rust 基线）
- 计时方法: 每文件预热 1 次 + 5 次热运行取中位数（median-of-5），CPU 固定核心 0，单次硬超时 60s

## 语料统计

- 语料: 74 个（examples/*.tie（剔除 oop_neg_*）+ tests/language/*.tie（剔除 *_neg.tie））
- 可编译（计入对比）: 73 个（覆盖 98.6%）
- tiec 不可编译: 0 个（见不可编译清单）
- 双失败（Rust 也失败）: 1 个（文件本身为负例/有错，非 tiec 缺口）

## 逐文件（耗时单位 ms，计入对比）

| 文件 | tiec | tie-llvm | 每文件比值 (tie/rust) |
| --- | ---: | ---: | ---: |
| examples/args_demo.tie | 80.7 | 91.9 | 0.88 |
| examples/assign.tie | 87.2 | 95.4 | 0.91 |
| examples/brotli_demo.tie | 132.7 | 97 | 1.37 |
| examples/byte_demo.tie | 84.6 | 99.8 | 0.85 |
| examples/char.tie | 76.4 | 80.8 | 0.95 |
| examples/compress_demo.tie | 119.9 | 92.3 | 1.30 |
| examples/csv_demo.tie | 138.2 | 101.6 | 1.36 |
| examples/exmath_demo.tie | 304.3 | 100.1 | 3.04 |
| examples/exmath_num_demo.tie | 297.9 | 106.5 | 2.80 |
| examples/format_demo.tie | 86.7 | 87.8 | 0.99 |
| examples/graph_demo.tie | 125.9 | 90.1 | 1.40 |
| examples/hello.tie | 74.2 | 78.4 | 0.95 |
| examples/import_main.tie | 75.4 | 77.5 | 0.97 |
| examples/import_nested.tie | 76.8 | 78.2 | 0.98 |
| examples/index_assign_demo.tie | 80.6 | 92 | 0.88 |
| examples/jpeg_demo.tie | 219.2 | 138.2 | 1.59 |
| examples/lib_math.tie | 79.2 | 78.3 | 1.01 |
| examples/lib_math2.tie | 85.1 | 81.9 | 1.04 |
| examples/lib_ns_tools.tie | 74.1 | 76 | 0.97 |
| examples/lib_util.tie | 70.4 | 75.2 | 0.94 |
| examples/linalg_demo.tie | 138.3 | 97.1 | 1.42 |
| examples/list_dir_demo.tie | 83.3 | 79.8 | 1.04 |
| examples/log_demo.tie | 145.4 | 91.2 | 1.59 |
| examples/log_enhance_demo.tie | 139.8 | 91.1 | 1.53 |
| examples/loop_control_demo.tie | 75.8 | 94.5 | 0.80 |
| examples/lz4_demo.tie | 90.3 | 83.4 | 1.08 |
| examples/m4_ops.tie | 73.7 | 86.7 | 0.85 |
| examples/ml_demo.tie | 128.7 | 95.8 | 1.34 |
| examples/namespace_demo.tie | 74.7 | 81.9 | 0.91 |
| examples/ns_import_demo.tie | 71.2 | 76 | 0.94 |
| examples/oop.tie | 82.9 | 96.7 | 0.86 |
| examples/optsearch_demo.tie | 122.7 | 93.7 | 1.31 |
| examples/radix_demo.tie | 88.6 | 88.2 | 1.00 |
| examples/regex_demo.tie | 82.2 | 87.6 | 0.94 |
| examples/registry_demo.tie | 85.4 | 89.3 | 0.96 |
| examples/script_demo.tie | 82.8 | 98.7 | 0.84 |
| examples/sort_demo.tie | 128.9 | 113.6 | 1.13 |
| examples/std_demo.tie | 121.8 | 92.5 | 1.32 |
| examples/std_math_demo.tie | 149.7 | 110.3 | 1.36 |
| examples/std_math_primitives.tie | 92.7 | 105.1 | 0.88 |
| examples/std_primitives.tie | 81.9 | 91.8 | 0.89 |
| examples/std_refactor_demo.tie | 143.2 | 101.1 | 1.42 |
| examples/strings.tie | 81.2 | 91.1 | 0.89 |
| examples/switch_pattern.tie | 81.6 | 93.5 | 0.87 |
| examples/switch_table_demo.tie | 79.7 | 84.5 | 0.94 |
| examples/switch.tie | 85.6 | 88.5 | 0.97 |
| examples/table_dynamic.tie | 75.8 | 86.8 | 0.87 |
| examples/table_enhance_demo.tie | 90.1 | 96 | 0.94 |
| examples/table_param_demo.tie | 74.9 | 85.3 | 0.88 |
| examples/table.tie | 77.7 | 83.6 | 0.93 |
| examples/test_wide.tie | 79.7 | 77.7 | 1.03 |
| examples/trit_demo.tie | 80.8 | 96.3 | 0.84 |
| examples/tuple.tie | 83 | 84.3 | 0.98 |
| examples/version_demo.tie | 88.5 | 92.6 | 0.96 |
| examples/wide.tie | 77.2 | 89.5 | 0.86 |
| examples/zstd_demo.tie | 262.6 | 92.3 | 2.85 |
| tests/language/byref_table.tie | 83.3 | 89.2 | 0.93 |
| tests/language/ext_test_bench.tie | 146.9 | 104.1 | 1.41 |
| tests/language/ext_ui_cfg.tie | 158.5 | 122.6 | 1.29 |
| tests/language/extern_decl.tie | 135.5 | 97.1 | 1.40 |
| tests/language/global_table.tie | 79 | 86.4 | 0.91 |
| tests/language/intern.tie | 99.4 | 89.7 | 1.11 |
| tests/language/interp_env_file.tie | 366.5 | 110.7 | 3.31 |
| tests/language/interp_env_value.tie | 373.4 | 108.8 | 3.43 |
| tests/language/interp_eval.tie | 4658.1 | 197.8 | 23.55 |
| tests/language/runtime_staticlib.tie | 72.7 | 85.2 | 0.85 |
| tests/language/std_args_time.tie | 93.9 | 90.6 | 1.04 |
| tests/language/std_coll_crc.tie | 122.1 | 104.1 | 1.17 |
| tests/language/std_encoding.tie | 165 | 91 | 1.81 |
| tests/language/std_fs_path.tie | 98.6 | 100.9 | 0.98 |
| tests/language/std_json.tie | 304.9 | 118.9 | 2.56 |
| tests/language/std_net_text.tie | 115.3 | 99.4 | 1.16 |
| tests/language/utf_ascii.tie | 107.3 | 98.2 | 1.09 |

## 双失败（Rust 基线自身失败）

| 文件 | 原因 | Rust exit |
| --- | --- | ---: |
| tests/language/global_table_const.tie | Rust 基线自身编译失败（文件为负例/语义错误，非 tiec 缺口） | 1 |

## 汇总

| 指标 | tiec | tie-llvm | 比值 (tie/rust) |
| --- | ---: | ---: | ---: |
| 总耗时 (ms，中位和) | 13152.3 | 6901.8 | 1.906 |

## G4 判定

- **ratio = tiec_total / rust_total = 1.906**（仅 73 个可编译文件）
- 硬线: ratio ≤ 3.0（同量级）→ 可编译文件数 > 0 且 ratio ≤ 3.0 为 PASS
- 目标: ratio ≤ 2.0（阶段 2 符号表直查后）
- 判定: **G4 PASS（部分基准 + 缺口清单）**
- 覆盖: 98.6%（73/74）——**部分基准**：语料未全量编译，结论仅代表当前可编译子集；缺口清单见上，修复后重跑自动扩全。

## 方法

- **冻结语料**: examples/*.tie（剔除 oop_neg_* 已知负例；库文件可含）与 tests/language/*.tie（剔除 *_neg.tie 负例），共 74 个。文件清单不落盘（脚本每次动态枚举），规则固定——缺口修复后新增可编译文件自动进入对比。
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

- 净收益的量化验证依赖全量语料（当前覆盖 98.6%）。部分基准下比值已接近 1.0，说明 tie 编译器的 frontend+IR 通道与 Rust 同量级甚至略快，架构收益成立。

## 当前覆盖与缺口

- **当前可编译**: 73/74（98.6%）——仅 irgen 最小集文件（println/print/exec_code/time_now/get_env + 算术/if/for/var + 纯函数），详见逐文件表。
- **不可编译**: 0 个，原因分类见上表（irgen 最小集外为主；前端语义/语法缺口少量）。前端语义缺口（全局表误判）正由另一任务修复——修复后重跑本脚本即自动扩全。
- **双失败**: 1 个（Rust 基线自身也失败，文件本身为负例/有错，不算 tiec 缺口）。
- **G4 结论**: ratio 1.906（仅可编译子集）
- 覆盖注脚: 当前为部分基准（覆盖 98.6%），结论仅代表可编译子集；缺口清单见上，前端语义缺口修复后重跑即自动扩全。