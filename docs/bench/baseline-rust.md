# tie 编译器 Rust 基线基准（baseline-rust）

- 生成时间: 2026-08-10 15:47:22
- CPU: 12th Gen Intel(R) Core(TM) i5-12490F
- 机器: JIRO-MAIN
- 语料文件数: 101（pass 94 / fail 7）
- 计时方法: 每文件预热 1 次 + 5 次热运行取中位数，CPU 固定核心 0
- 前端通道: `tie-frontend <file> --check`
- 前端+IR 通道: `tie-llvm <file> --emit-ir`

## 汇总

| 指标 | 前端 --check | 前端+IR --emit-ir |
| --- | ---: | ---: |
| 总耗时 (ms) | 5446.4 | 11114.1 |
| 中位数 (ms) | 57.3 | 76.3 |
| 最大单文件 (ms) | 95.2 | 694.2 |
| 意外失败 (pass 文件 --emit-ir 退出码≠0) | 0 |
| --check 可成功样本 (import 无关) | 62 / 94 |

## 逐文件（耗时单位 ms）

| 文件 | 角色 | 前端 | 前端+IR | 退出码 |
| --- | --- | ---: | ---: | ---: |
| examples/args_demo.tie | pass | 61.2 | 87.2 | 0 |
| examples/assign.tie | pass | 54.1 | 150.8 | 0 |
| examples/brotli_demo.tie | pass | 65.9 | 74.7 | 1 ⚠ |
| examples/byte_demo.tie | pass | 49.7 | 73.5 | 0 |
| examples/char.tie | pass | 49.4 | 70.2 | 0 |
| examples/compress_demo.tie | pass | 63.4 | 68 | 1 ⚠ |
| examples/csv_demo.tie | pass | 61.5 | 75.6 | 1 ⚠ |
| examples/demo_pkg/.tie/deps/lib_colors/lib_colors.tie | pass | 49.2 | 64.9 | 0 |
| examples/demo_pkg/main.tie | pass | 50.1 | 82.2 | 0 |
| examples/exmath_demo.tie | pass | 60.2 | 75.5 | 1 ⚠ |
| examples/exmath_num_demo.tie | pass | 63.5 | 89.1 | 1 ⚠ |
| examples/format_demo.tie | pass | 69.6 | 70.9 | 1 ⚠ |
| examples/graph_demo.tie | pass | 58.1 | 77.2 | 1 ⚠ |
| examples/hello.tie | pass | 50.9 | 62.8 | 0 |
| examples/import_main.tie | pass | 59.8 | 64.9 | 1 ⚠ |
| examples/import_nested.tie | pass | 73.8 | 87 | 1 ⚠ |
| examples/index_assign_demo.tie | pass | 52.4 | 74.2 | 0 |
| examples/jpeg_demo.tie | pass | 59.9 | 109 | 1 ⚠ |
| examples/lib_colors/lib_colors.tie | pass | 52.9 | 64.6 | 0 |
| examples/lib_math.tie | pass | 51.7 | 66.2 | 0 |
| examples/lib_math2.tie | pass | 71.9 | 98.6 | 1 ⚠ |
| examples/lib_ns_tools.tie | pass | 53.7 | 105.1 | 0 |
| examples/lib_util.tie | pass | 95.2 | 90.9 | 0 |
| examples/linalg_demo.tie | pass | 62.2 | 72.9 | 1 ⚠ |
| examples/list_dir_demo.tie | pass | 62.5 | 63.4 | 0 |
| examples/log_demo.tie | pass | 59.3 | 73.2 | 1 ⚠ |
| examples/log_enhance_demo.tie | pass | 61.1 | 70 | 1 ⚠ |
| examples/loop_control_demo.tie | pass | 51.1 | 67.1 | 0 |
| examples/lz4_demo.tie | pass | 58 | 67.7 | 1 ⚠ |
| examples/m4_ops.tie | pass | 50.5 | 72.2 | 0 |
| examples/ml_demo.tie | pass | 61.4 | 69.5 | 1 ⚠ |
| examples/namespace_demo.tie | pass | 58.1 | 62.1 | 0 |
| examples/ns_import_demo.tie | pass | 59.6 | 65.3 | 1 ⚠ |
| examples/oop_neg_a.tie | fail | - | - | 1 |
| examples/oop_neg_b.tie | fail | - | - | 1 |
| examples/oop_neg_c.tie | fail | - | - | 1 |
| examples/oop_neg_d.tie | fail | - | - | 1 |
| examples/oop_neg_e.tie | fail | - | - | 1 |
| examples/oop.tie | pass | 54.2 | 76.3 | 0 |
| examples/optsearch_demo.tie | pass | 62.3 | 73.6 | 1 ⚠ |
| examples/radix_demo.tie | pass | 60.6 | 73.2 | 1 ⚠ |
| examples/regex_demo.tie | pass | 55.6 | 70.9 | 0 |
| examples/registry_demo.tie | pass | 61.6 | 74.6 | 1 ⚠ |
| examples/script_demo.tie | pass | 53 | 77.4 | 0 |
| examples/sort_demo.tie | pass | 64.5 | 91.2 | 1 ⚠ |
| examples/std_demo.tie | pass | 67.3 | 78.7 | 1 ⚠ |
| examples/std_math_demo.tie | pass | 63.6 | 89.1 | 1 ⚠ |
| examples/std_math_primitives.tie | pass | 57.8 | 86.9 | 0 |
| examples/std_primitives.tie | pass | 62.3 | 71.9 | 0 |
| examples/std_refactor_demo.tie | pass | 61.2 | 100.1 | 1 ⚠ |
| examples/strings.tie | pass | 93.2 | 87.1 | 0 |
| examples/switch_pattern.tie | pass | 59.1 | 85.6 | 0 |
| examples/switch_table_demo.tie | pass | 56.1 | 66.4 | 0 |
| examples/switch.tie | pass | 60.7 | 68.1 | 0 |
| examples/table_dynamic.tie | pass | 53.9 | 73.3 | 0 |
| examples/table_enhance_demo.tie | fail | - | - | 1 |
| examples/table_param_demo.tie | pass | 59.2 | 68 | 1 ⚠ |
| examples/table.tie | pass | 57.5 | 61.2 | 0 |
| examples/test_wide.tie | pass | 55.7 | 61.1 | 0 |
| examples/trit_demo.tie | pass | 53.7 | 75.1 | 0 |
| examples/tuple.tie | pass | 57.3 | 73.3 | 0 |
| examples/version_demo.tie | pass | 60.6 | 76.6 | 1 ⚠ |
| examples/wide.tie | pass | 61.2 | 74.8 | 0 |
| examples/zstd_demo.tie | pass | 60.6 | 71.9 | 1 ⚠ |
| ext/cache.tie | pass | 59.4 | 78.4 | 0 |
| ext/codec/brotli.tie | pass | 52 | 213.1 | 0 |
| ext/codec/jpeg.tie | pass | 58.4 | 416 | 0 |
| ext/codec/lz4.tie | pass | 57.4 | 116.1 | 0 |
| ext/codec/zstd.tie | pass | 57.2 | 694.2 | 0 |
| ext/compress.tie | pass | 54.6 | 239.4 | 0 |
| ext/log.tie | pass | 63.6 | 115.9 | 1 ⚠ |
| ext/ml.tie | pass | 53.4 | 205.8 | 0 |
| ext/registry.tie | pass | 54.3 | 71.7 | 0 |
| pkg/deps.tie | pass | 64.7 | 649.2 | 1 ⚠ |
| pkg/fetch.tie | pass | 68.5 | 323.1 | 1 ⚠ |
| pkg/lock.tie | pass | 52.8 | 214.3 | 0 |
| pkg/main.tie | pass | 61.7 | 409.1 | 1 ⚠ |
| pkg/manifest.tie | pass | 51.6 | 193.1 | 0 |
| pkg/publish.tie | pass | 51.2 | 128.1 | 0 |
| pkg/search.tie | pass | 51.3 | 129 | 0 |
| prep/core.tie | pass | 52.2 | 143.5 | 0 |
| prep/indent.tie | pass | 49.4 | 72.3 | 0 |
| prep/rename_enl_to_ext_prog.tie | pass | 49.7 | 70.5 | 0 |
| prep/rename_enl_to_ext.tie | pass | 51.8 | 64.4 | 0 |
| prep/rename_tcmsg_to_log.tie | pass | 52.9 | 67.3 | 0 |
| prep/test_cn.tie | pass | 52.4 | 61.4 | 0 |
| prep/test_std_cn.tie | fail | - | - | 1 |
| prep/test_trim.tie | pass | 52.2 | 72 | 0 |
| repl/repl.tie | pass | 51.7 | 67.1 | 0 |
| std/assert.tie | pass | 52.3 | 68 | 0 |
| std/csv.tie | pass | 63.7 | 78.3 | 1 ⚠ |
| std/exmath.tie | pass | 51.8 | 516.3 | 0 |
| std/format.tie | pass | 53.8 | 76.4 | 0 |
| std/graph.tie | pass | 54.5 | 164.9 | 0 |
| std/linalg.tie | pass | 53.2 | 213.1 | 0 |
| std/math.tie | pass | 51.9 | 85.3 | 0 |
| std/optsearch.tie | pass | 53.1 | 170.9 | 0 |
| std/radix.tie | pass | 55.4 | 81.9 | 0 |
| std/sort.tie | pass | 52.9 | 125.7 | 0 |
| std/string.tie | pass | 51.3 | 150.8 | 0 |
| std/version.tie | pass | 53.4 | 94.6 | 0 |
