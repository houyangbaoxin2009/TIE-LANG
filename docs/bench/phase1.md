# tie 自举阶段 1 前端性能闸门（G1）

- 生成时间: 2026-08-11 11:21:37
- CPU: 12th Gen Intel(R) Core(TM) i5-12490F
- 机器: JIRO-MAIN
- 通道: `tiec-proto <file> --check`（tie 前端）vs `tie-frontend <file> --check`（Rust 前端）
- 计时方法: 每文件预热 1 次 + 5 次热运行取中位数，CPU 固定核心 0，单次硬超时 60s

## 语料统计

- 语料文件总数: 101（pass 94 / fail 7）
- 计入对比（两边都 exit 0）: 62 个
- 排除: 32 个
- 排除原因: tiec-proto 语义层不展开 import（import 文件预期 tiec exit 1），故仅对两边都 exit 0 的文件做公平对比

## 逐文件（耗时单位 ms，计入对比）

| 文件 | tiec-proto | tie-frontend | 每文件比值 (tie/rust) |
| --- | ---: | ---: | ---: |
| examples/args_demo.tie | 67.7 | 56.3 | 1.20 |
| examples/assign.tie | 65.3 | 60.9 | 1.07 |
| examples/byte_demo.tie | 249.6 | 55.1 | 4.53 |
| examples/char.tie | 63 | 51.5 | 1.22 |
| examples/demo_pkg/.tie/deps/lib_colors/lib_colors.tie | 57.4 | 55.8 | 1.03 |
| examples/demo_pkg/main.tie | 63.7 | 49.4 | 1.29 |
| examples/hello.tie | 56 | 47.5 | 1.18 |
| examples/index_assign_demo.tie | 91.2 | 47.9 | 1.90 |
| examples/lib_colors/lib_colors.tie | 55.2 | 46.9 | 1.18 |
| examples/lib_math.tie | 58 | 47.8 | 1.21 |
| examples/lib_ns_tools.tie | 53 | 46.7 | 1.13 |
| examples/lib_util.tie | 52.1 | 46.1 | 1.13 |
| examples/list_dir_demo.tie | 55.5 | 47.9 | 1.16 |
| examples/loop_control_demo.tie | 69.2 | 45.1 | 1.53 |
| examples/m4_ops.tie | 68 | 45.7 | 1.49 |
| examples/namespace_demo.tie | 55.4 | 47.8 | 1.16 |
| examples/oop.tie | 77.4 | 48.8 | 1.59 |
| examples/regex_demo.tie | 70 | 47.4 | 1.48 |
| examples/script_demo.tie | 59.7 | 47.3 | 1.26 |
| examples/std_math_primitives.tie | 104.6 | 45.9 | 2.28 |
| examples/std_primitives.tie | 81.1 | 47.5 | 1.71 |
| examples/strings.tie | 68.7 | 47.4 | 1.45 |
| examples/switch_pattern.tie | 61.8 | 47.1 | 1.31 |
| examples/switch_table_demo.tie | 55.4 | 45.5 | 1.22 |
| examples/switch.tie | 61 | 48.8 | 1.25 |
| examples/table_dynamic.tie | 65.2 | 46.6 | 1.40 |
| examples/table_param_demo.tie | 61.1 | 48.7 | 1.25 |
| examples/table.tie | 54.9 | 47.9 | 1.15 |
| examples/test_wide.tie | 52.2 | 48.1 | 1.09 |
| examples/trit_demo.tie | 81.8 | 47.7 | 1.71 |
| examples/tuple.tie | 68.2 | 48.1 | 1.42 |
| examples/wide.tie | 56.9 | 50.5 | 1.13 |
| ext/cache.tie | 55.3 | 48.2 | 1.15 |
| ext/codec/brotli.tie | 352.2 | 47.5 | 7.41 |
| ext/codec/jpeg.tie | 915.3 | 49.3 | 18.57 |
| ext/codec/lz4.tie | 187.3 | 47.4 | 3.95 |
| ext/codec/zstd.tie | 1884.5 | 48.3 | 39.02 |
| ext/compress.tie | 287.4 | 47.3 | 6.08 |
| ext/ml.tie | 462 | 47.1 | 9.81 |
| ext/registry.tie | 53.5 | 46.9 | 1.14 |
| pkg/lock.tie | 531.1 | 49.5 | 10.73 |
| pkg/manifest.tie | 350.1 | 47.1 | 7.43 |
| pkg/search.tie | 263.2 | 45.1 | 5.84 |
| prep/core.tie | 212.9 | 47.4 | 4.49 |
| prep/indent.tie | 60 | 52 | 1.15 |
| prep/rename_enl_to_ext_prog.tie | 58.8 | 47.9 | 1.23 |
| prep/rename_enl_to_ext.tie | 54.6 | 48.5 | 1.13 |
| prep/rename_tcmsg_to_log.tie | 61.4 | 48.4 | 1.27 |
| prep/test_cn.tie | 53.4 | 49.4 | 1.08 |
| prep/test_trim.tie | 78.9 | 49.6 | 1.59 |
| repl/repl.tie | 56.4 | 47.9 | 1.18 |
| std/assert.tie | 62.4 | 47.4 | 1.32 |
| std/exmath.tie | 1892.9 | 48.3 | 39.19 |
| std/format.tie | 74.3 | 49.6 | 1.50 |
| std/graph.tie | 539.3 | 48.5 | 11.12 |
| std/linalg.tie | 1084.2 | 50.8 | 21.34 |
| std/math.tie | 107.1 | 46.8 | 2.29 |
| std/optsearch.tie | 551.8 | 48.5 | 11.38 |
| std/radix.tie | 80.8 | 46.8 | 1.73 |
| std/sort.tie | 273.5 | 49.3 | 5.55 |
| std/string.tie | 322.7 | 47.8 | 6.75 |
| std/version.tie | 125.5 | 44.9 | 2.80 |

## 排除文件

| 文件 | 原因 | tiec exit | rust exit |
| --- | --- | ---: | ---: |
| examples/brotli_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/compress_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/csv_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/exmath_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/exmath_num_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/format_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/graph_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/import_main.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/import_nested.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/jpeg_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/lib_math2.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/linalg_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/log_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/log_enhance_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/lz4_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/ml_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/ns_import_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/optsearch_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/radix_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/registry_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/sort_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/std_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/std_math_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/std_refactor_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/version_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| examples/zstd_demo.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| ext/log.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| pkg/deps.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| pkg/fetch.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| pkg/main.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |
| pkg/publish.tie | tiec-proto 语义层不展开 import（exit 1） | 1 | 0 |
| std/csv.tie | 两边均退出非 0（tiec=1 / rust=1） | 1 | 1 |

## 汇总

| 指标 | tiec-proto | tie-frontend | 比值 (tie/rust) |
| --- | ---: | ---: | ---: |
| 总耗时 (ms) | 13353.1 | 3005.2 | 4.443 |
| 单文件中位数 (ms) | 68.2 | 47.9 | - |
| 单文件最大 (ms) | 1892.9 | 60.9 | - |

## G1 判定

- **ratio = tiec_proto_total / tie_frontend_total = 4.443**
- 硬闸门: ratio < 1.0（tie 前端总耗时 < Rust 前端总耗时）→ **G1 FAIL**
- 目标: 0.5–0.83（tie 前端 1.2–2× 快于 Rust）
- 结论: tie 前端未快于 Rust，G1 未过；热点分析见下，优化留待后续阶段（不改产品代码）。

## 热点分析（G1 FAIL，仅分析不改产品代码）

- 计入的 62 个文件中，**无任何文件 tiec-proto 快于 tie-frontend**（tiec_ms ≤ rust_ms 的文件数 = 0）。
- 每文件比值分布: <2× 有 42 个、2–5× 有 6 个、5–10× 有 7 个、≥10× 有 7 个——大文件（符号多的库文件）急剧恶化。

### tiec-proto 最慢 Top8

| 文件 | tiec-proto (ms) | tie-frontend (ms) | 每文件比值 |
| --- | ---: | ---: | ---: |
| std/exmath.tie | 1892.9 | 48.3 | 39.19 |
| ext/codec/zstd.tie | 1884.5 | 48.3 | 39.02 |
| std/linalg.tie | 1084.2 | 50.8 | 21.34 |
| ext/codec/jpeg.tie | 915.3 | 49.3 | 18.57 |
| std/optsearch.tie | 551.8 | 48.5 | 11.38 |
| std/graph.tie | 539.3 | 48.5 | 11.12 |
| pkg/lock.tie | 531.1 | 49.5 | 10.73 |
| ext/ml.tie | 462 | 47.1 | 9.81 |

### 可能原因（依据 compiler/proto 实现）

1. **符号表构建是 O(n²)**：`semantic.tie` 的 `sorted_insert`（约 662 行）每插入一个符号先二分定位，再 `table_push` + `while i > pos` 整体后移表——每个新符号都位移整张表。大符号表文件（zstd/exmath/linalg/jpeg 等，符号数百个）构成主要热点，比值达 10–38×。
2. **表访问经 C ABI 间接调用**：tie 语言 `keys[mid]` / `keys[i] = keys[i-1]` 每次读写 `table<i64>` 都落到运行时表操作（下标/边界/长度维护），比 Rust 原生 Vec/数组访问慢一个数量级，放大 O(n²) 常数。
3. **字符串池 intern 开销**：每个名字/字面量走 `intern.intern`（二分 + 比较 + 分配），符号名频繁 id 化；`out = out + ...` 字符串拼接每次产生新分配（如 `semantic.tie` 464/695 行附近）。

### 记录

- **G1 未过，如实记录，优化留待后续阶段**（阶段 2 模块化重写时按 LLVM 3 层重做，届时符号表/列式表按 T2.x 任务重构；不改当前产品代码）。
- 被排除的 32 个文件：31 个两边都 exit 1（Rust --check 同样不展开 import，属 import 文件）；1 个（pkg/publish.tie）tiec-proto 单独失败——无 import 但报 `未定义的函数 'path_dirname'`，为 tiec-proto 语义层对命名空间内函数解析的差异，非 import 所致，已记录。