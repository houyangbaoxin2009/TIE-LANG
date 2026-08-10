# tie 编译器 Rust 基线基准（baseline-rust）

- 生成时间: 2026-08-10 19:46:21
- CPU: 12th Gen Intel(R) Core(TM) i5-12490F
- 机器: JIRO-MAIN
- 语料文件数: 101（pass 94 / fail 7）
- 计时方法: 每文件预热 1 次 + 5 次热运行取中位数，CPU 固定核心 0
- 前端通道: `tie-frontend <file> --check`
- 前端+IR 通道: `tie-llvm <file> --emit-ir`

## 汇总

| 指标 | 前端 --check | 前端+IR --emit-ir |
| --- | ---: | ---: |
| 总耗时 (ms) | 7287.6 | 13691 |
| 中位数 (ms) | 72.1 | 103 |
| 最大单文件 (ms) | 156.9 | 629.2 |
| 意外失败 (pass 文件 --emit-ir 退出码≠0) | 0 |
| --check 可成功样本 (import 无关) | 63 / 94 |

## 逐文件（耗时单位 ms）

| 文件 | 角色 | 前端 | 前端+IR | 退出码 |
| --- | --- | ---: | ---: | ---: |
| examples/args_demo.tie | pass | 79.5 | 287.5 | 0 |
| examples/assign.tie | pass | 152.1 | 104.7 | 0 |
| examples/brotli_demo.tie | pass | 104.3 | 101.2 | 1 ⚠ |
| examples/byte_demo.tie | pass | 67.9 | 103.7 | 0 |
| examples/char.tie | pass | 69.4 | 93.7 | 0 |
| examples/compress_demo.tie | pass | 78.3 | 97 | 1 ⚠ |
| examples/csv_demo.tie | pass | 86.8 | 114.5 | 1 ⚠ |
| examples/demo_pkg/.tie/deps/lib_colors/lib_colors.tie | pass | 79.8 | 123.7 | 0 |
| examples/demo_pkg/main.tie | pass | 74.6 | 101.1 | 0 |
| examples/exmath_demo.tie | pass | 86 | 135.1 | 1 ⚠ |
| examples/exmath_num_demo.tie | pass | 156.9 | 151.3 | 1 ⚠ |
| examples/format_demo.tie | pass | 118.4 | 105.1 | 1 ⚠ |
| examples/graph_demo.tie | pass | 110.6 | 98.1 | 1 ⚠ |
| examples/hello.tie | pass | 70.2 | 102.2 | 0 |
| examples/import_main.tie | pass | 91.7 | 110 | 1 ⚠ |
| examples/import_nested.tie | pass | 120.3 | 101.6 | 1 ⚠ |
| examples/index_assign_demo.tie | pass | 75.4 | 110.3 | 0 |
| examples/jpeg_demo.tie | pass | 122.9 | 158.1 | 1 ⚠ |
| examples/lib_colors/lib_colors.tie | pass | 63.9 | 76.3 | 0 |
| examples/lib_math.tie | pass | 59.8 | 91 | 0 |
| examples/lib_math2.tie | pass | 93.5 | 81.9 | 1 ⚠ |
| examples/lib_ns_tools.tie | pass | 70.8 | 77.6 | 0 |
| examples/lib_util.tie | pass | 77 | 103.8 | 0 |
| examples/linalg_demo.tie | pass | 87.6 | 125.2 | 1 ⚠ |
| examples/list_dir_demo.tie | pass | 86.7 | 334.1 | 0 |
| examples/log_demo.tie | pass | 122.9 | 121.5 | 1 ⚠ |
| examples/log_enhance_demo.tie | pass | 97 | 122.2 | 1 ⚠ |
| examples/loop_control_demo.tie | pass | 79.4 | 103 | 0 |
| examples/lz4_demo.tie | pass | 77 | 95.4 | 1 ⚠ |
| examples/m4_ops.tie | pass | 72 | 112.4 | 0 |
| examples/ml_demo.tie | pass | 83.9 | 99.1 | 1 ⚠ |
| examples/namespace_demo.tie | pass | 66.5 | 91.2 | 0 |
| examples/ns_import_demo.tie | pass | 84.9 | 86 | 1 ⚠ |
| examples/oop_neg_a.tie | fail | - | - | 1 |
| examples/oop_neg_b.tie | fail | - | - | 1 |
| examples/oop_neg_c.tie | fail | - | - | 1 |
| examples/oop_neg_d.tie | fail | - | - | 1 |
| examples/oop_neg_e.tie | fail | - | - | 1 |
| examples/oop.tie | pass | 65.5 | 97.2 | 0 |
| examples/optsearch_demo.tie | pass | 80.7 | 97.1 | 1 ⚠ |
| examples/radix_demo.tie | pass | 80.2 | 89.2 | 1 ⚠ |
| examples/regex_demo.tie | pass | 71.8 | 137 | 0 |
| examples/registry_demo.tie | pass | 108.1 | 138.5 | 1 ⚠ |
| examples/script_demo.tie | pass | 85.3 | 136.9 | 0 |
| examples/sort_demo.tie | pass | 80.6 | 144.9 | 1 ⚠ |
| examples/std_demo.tie | pass | 68.3 | 94.4 | 1 ⚠ |
| examples/std_math_demo.tie | pass | 75.9 | 108.4 | 1 ⚠ |
| examples/std_math_primitives.tie | pass | 68.8 | 105.4 | 0 |
| examples/std_primitives.tie | pass | 69.8 | 97.9 | 0 |
| examples/std_refactor_demo.tie | pass | 82.4 | 95.2 | 1 ⚠ |
| examples/strings.tie | pass | 75.2 | 89.4 | 0 |
| examples/switch_pattern.tie | pass | 62.3 | 80 | 0 |
| examples/switch_table_demo.tie | pass | 65.8 | 80.9 | 0 |
| examples/switch.tie | pass | 67.6 | 88.3 | 0 |
| examples/table_dynamic.tie | pass | 63.3 | 80.5 | 0 |
| examples/table_enhance_demo.tie | fail | - | - | 0 |
| examples/table_param_demo.tie | pass | 59.6 | 83.3 | 0 |
| examples/table.tie | pass | 64.1 | 85 | 0 |
| examples/test_wide.tie | pass | 66.7 | 82.1 | 0 |
| examples/trit_demo.tie | pass | 62.2 | 93.4 | 0 |
| examples/tuple.tie | pass | 71.2 | 91.2 | 0 |
| examples/version_demo.tie | pass | 77.7 | 99 | 1 ⚠ |
| examples/wide.tie | pass | 72.1 | 81.2 | 0 |
| examples/zstd_demo.tie | pass | 79.6 | 95.4 | 1 ⚠ |
| ext/cache.tie | pass | 71.8 | 85 | 0 |
| ext/codec/brotli.tie | pass | 71.2 | 188.5 | 0 |
| ext/codec/jpeg.tie | pass | 66.9 | 400.1 | 0 |
| ext/codec/lz4.tie | pass | 65.7 | 134.3 | 0 |
| ext/codec/zstd.tie | pass | 65.9 | 497.4 | 0 |
| ext/compress.tie | pass | 63.8 | 180.9 | 0 |
| ext/log.tie | pass | 74.9 | 133.4 | 1 ⚠ |
| ext/ml.tie | pass | 64 | 221.6 | 0 |
| ext/registry.tie | pass | 63.6 | 80.2 | 0 |
| pkg/deps.tie | pass | 69.6 | 629.2 | 1 ⚠ |
| pkg/fetch.tie | pass | 72.7 | 341.8 | 1 ⚠ |
| pkg/lock.tie | pass | 73.3 | 274 | 0 |
| pkg/main.tie | pass | 71.7 | 491.3 | 1 ⚠ |
| pkg/manifest.tie | pass | 73.5 | 228.5 | 0 |
| pkg/publish.tie | pass | 61.2 | 161 | 0 |
| pkg/search.tie | pass | 59.3 | 155.5 | 0 |
| prep/core.tie | pass | 67.4 | 178.8 | 0 |
| prep/indent.tie | pass | 65.9 | 86.6 | 0 |
| prep/rename_enl_to_ext_prog.tie | pass | 70.7 | 89.2 | 0 |
| prep/rename_enl_to_ext.tie | pass | 82.3 | 91.5 | 0 |
| prep/rename_tcmsg_to_log.tie | pass | 77.7 | 89.5 | 0 |
| prep/test_cn.tie | pass | 65.1 | 85.2 | 0 |
| prep/test_std_cn.tie | fail | - | - | 1 |
| prep/test_trim.tie | pass | 71.3 | 96.1 | 0 |
| repl/repl.tie | pass | 61.1 | 82.3 | 0 |
| std/assert.tie | pass | 72.3 | 86.4 | 0 |
| std/csv.tie | pass | 70.9 | 93.9 | 1 ⚠ |
| std/exmath.tie | pass | 79.7 | 615.9 | 0 |
| std/format.tie | pass | 68.7 | 100.9 | 0 |
| std/graph.tie | pass | 71.4 | 230.8 | 0 |
| std/linalg.tie | pass | 80.3 | 308.9 | 0 |
| std/math.tie | pass | 73.6 | 105.4 | 0 |
| std/optsearch.tie | pass | 65.4 | 199.3 | 0 |
| std/radix.tie | pass | 60.4 | 98.4 | 0 |
| std/sort.tie | pass | 66.3 | 165.2 | 0 |
| std/string.tie | pass | 75.4 | 176.7 | 0 |
| std/version.tie | pass | 59.8 | 115.9 | 0 |
