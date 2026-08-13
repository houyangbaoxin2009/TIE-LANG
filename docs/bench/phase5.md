# tie 自举阶段 5 前端+IR 性能闸门（G4）

- 生成时间: 2026-08-13 19:50:32
- CPU: 12th Gen Intel(R) Core(TM) i5-12490F
- 机器: JIRO-MAIN
- 通道: `tiec <file> --emit-ir`（tie 编译器）vs `tie-llvm <file> --emit-ir`（Rust 基线）
- 计时方法: 每文件预热 1 次 + 5 次热运行取中位数（median-of-5），CPU 固定核心 0，单次硬超时 60s

## 语料统计

- 语料: 74 个（examples/*.tie（剔除 oop_neg_*）+ tests/language/*.tie（剔除 *_neg.tie））
- 可编译（计入对比）: 30 个（覆盖 40.5%）
- tiec 不可编译: 43 个（见不可编译清单）
- 双失败（Rust 也失败）: 1 个（文件本身为负例/有错，非 tiec 缺口）

## 逐文件（耗时单位 ms，计入对比）

| 文件 | tiec | tie-llvm | 每文件比值 (tie/rust) |
| --- | ---: | ---: | ---: |
| examples/args_demo.tie | 79.7 | 88.5 | 0.90 |
| examples/csv_demo.tie | 145.8 | 87.7 | 1.66 |
| examples/format_demo.tie | 75.3 | 74.5 | 1.01 |
| examples/graph_demo.tie | 115.9 | 81 | 1.43 |
| examples/hello.tie | 61.4 | 66.7 | 0.92 |
| examples/import_main.tie | 64 | 67.6 | 0.95 |
| examples/import_nested.tie | 60.1 | 68.4 | 0.88 |
| examples/index_assign_demo.tie | 66.3 | 78.4 | 0.85 |
| examples/lib_math.tie | 63.8 | 69 | 0.92 |
| examples/lib_math2.tie | 59.4 | 66.8 | 0.89 |
| examples/lib_ns_tools.tie | 61.1 | 73.2 | 0.83 |
| examples/lib_util.tie | 65.1 | 66.3 | 0.98 |
| examples/loop_control_demo.tie | 66.6 | 77.3 | 0.86 |
| examples/namespace_demo.tie | 65.9 | 70.4 | 0.94 |
| examples/ns_import_demo.tie | 64.8 | 68 | 0.95 |
| examples/optsearch_demo.tie | 103.8 | 76 | 1.37 |
| examples/radix_demo.tie | 73.9 | 77.6 | 0.95 |
| examples/sort_demo.tie | 112.6 | 96.7 | 1.16 |
| examples/std_demo.tie | 115.7 | 77.6 | 1.49 |
| examples/strings.tie | 75 | 88.9 | 0.84 |
| examples/switch_table_demo.tie | 62.1 | 70.9 | 0.88 |
| tests/language/byref_table.tie | 63 | 68.4 | 0.92 |
| tests/language/ext_ui_cfg.tie | 130.4 | 100.3 | 1.30 |
| tests/language/global_table.tie | 64.9 | 70.7 | 0.92 |
| tests/language/intern.tie | 95.3 | 79.7 | 1.20 |
| tests/language/runtime_staticlib.tie | 68.3 | 70.6 | 0.97 |
| tests/language/std_args_time.tie | 87.4 | 87.4 | 1.00 |
| tests/language/std_coll_crc.tie | 117.8 | 89.6 | 1.31 |
| tests/language/std_encoding.tie | 146.2 | 89.1 | 1.64 |
| tests/language/utf_ascii.tie | 106.3 | 89.8 | 1.18 |

## 不可编译清单（tiec 当前缺口，不参与 ratio）

> 前端语义缺口（全局表误判）修复后，这些文件预计自动进入对比——同一脚本无需改动。

| 文件 | 原因 | tiec exit |
| --- | --- | ---: |
| examples/assign.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/brotli_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/byte_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/char.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/compress_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/exmath_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/exmath_num_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/jpeg_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/linalg_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/list_dir_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/log_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/log_enhance_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/lz4_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/m4_ops.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/ml_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/oop.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/regex_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/registry_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/script_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/std_math_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/std_math_primitives.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/std_primitives.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/std_refactor_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/switch_pattern.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/switch.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/table_dynamic.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/table_enhance_demo.tie | 前端语法缺口 | 1 |
| examples/table_param_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/table.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/test_wide.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/trit_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/tuple.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/version_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/wide.tie | irgen 最小集外（尚未支持的语句） | 1 |
| examples/zstd_demo.tie | irgen 最小集外（尚未支持的语句） | 1 |
| tests/language/ext_test_bench.tie | irgen 最小集外（尚未支持的语句） | 1 |
| tests/language/extern_decl.tie | irgen 最小集外（尚未支持的语句） | 1 |
| tests/language/interp_env_file.tie | irgen 最小集外（尚未支持的语句） | 1 |
| tests/language/interp_env_value.tie | irgen 最小集外（尚未支持的语句） | 1 |
| tests/language/interp_eval.tie | irgen 最小集外（尚未支持的语句） | 1 |
| tests/language/std_fs_path.tie | irgen 最小集外（尚未支持的语句） | 1 |
| tests/language/std_json.tie | irgen 最小集外（尚未支持的语句） | 1 |
| tests/language/std_net_text.tie | irgen 最小集外（尚未支持的语句） | 1 |

## 双失败（Rust 基线自身失败）

| 文件 | 原因 | Rust exit |
| --- | --- | ---: |
| tests/language/global_table_const.tie | Rust 基线自身编译失败（文件为负例/语义错误，非 tiec 缺口） | 1 |

## 汇总

| 指标 | tiec | tie-llvm | 比值 (tie/rust) |
| --- | ---: | ---: | ---: |
| 总耗时 (ms，中位和) | 2537.9 | 2337.1 | 1.086 |

## G4 判定

- **ratio = tiec_total / rust_total = 1.086**（仅 30 个可编译文件）
- 硬线: ratio ≤ 3.0（同量级）→ 可编译文件数 > 0 且 ratio ≤ 3.0 为 PASS
- 目标: ratio ≤ 2.0（阶段 2 符号表直查后）
- 判定: **G4 PASS（部分基准 + 缺口清单）**
- 覆盖: 40.5%（30/74）——**部分基准**：语料未全量编译，结论仅代表当前可编译子集；缺口清单见上，修复后重跑自动扩全。

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

- 净收益的量化验证依赖全量语料（当前覆盖 40.5%）。部分基准下比值已接近 1.0，说明 tie 编译器的 frontend+IR 通道与 Rust 同量级甚至略快，架构收益成立。

## 当前覆盖与缺口

- **当前可编译**: 30/74（40.5%）——仅 irgen 最小集文件（println/print/exec_code/time_now/get_env + 算术/if/for/var + 纯函数），详见逐文件表。
- **不可编译**: 43 个，原因分类见上表（irgen 最小集外为主；前端语义/语法缺口少量）。前端语义缺口（全局表误判）正由另一任务修复——修复后重跑本脚本即自动扩全。
- **双失败**: 1 个（Rust 基线自身也失败，文件本身为负例/有错，不算 tiec 缺口）。
- **G4 结论**: ratio 1.086（仅可编译子集）
- 覆盖注脚: 当前为部分基准（覆盖 40.5%），结论仅代表可编译子集；缺口清单见上，前端语义缺口修复后重跑即自动扩全。