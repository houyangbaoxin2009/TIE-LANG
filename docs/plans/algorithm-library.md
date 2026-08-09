# 规划（M4 补齐）：算法库分类——std 与 enl 分层

> 状态：**规划**（分类已定，按序实现）
> 所属：Harbor（2026.1）架构 M4 补齐（语言能力分层扩展的一部分）
> 背景：用户提出用 tie 语言实现一批经典算法，需按「改进 std 的进 std，改进 enl 的进 enl」
> 原则分类。本文件给出 38 个算法的完整分类 + 可行性评估 + 实现优先级。

## 0. tie 语言能力边界（决定可行性）

| 能力 | 状态 | 对算法的影响 |
| --- | --- | --- |
| struct 字段读写 | ✅ | 图节点/矩阵元素/树节点可用 struct 表达 |
| 表（只读 + 追加） | ✅ | 容器可用「并行表」或「struct 数组表」模拟 |
| **表下标赋值 `t[i] = v`** | 🔧 **规划中（阶段 0 前置）** | 落地后矩阵/图/DP 算法回归自然形态；此前用「重建表」过渡 |
| 表 pop | ❌ | 栈操作用「尾部游标」模拟（huffman_build 已用） |
| 字符串 id 表 `["a":1]` | ❌ | 哈希表不可用 → 顺序扫描代替 |
| 递归函数 | ✅（M3 起） | 分治/回溯/树遍历可用 |
| 位运算（&、\|、^、<<、>>） | ✅（M4 起） | 位级算法（压缩/编码）可做 |
| 字节流/二进制 IO | 🔧 **规划中（阶段 0b：byte_read/byte_write 原语）** | 落地后多媒体编解码（JPEG/MP3/H.264 等）可用 tie 实现；此前不可行 |
| 变参函数 | ❌ | 参数固定，多用表传参（sprintf 范式） |

**结论**：数据/数值/图/搜索/组合类算法全部可用 tie 实现；**多媒体编解码**
（JPEG/MP3）与现代压缩（LZ4/Zstd/Brotli）需先补「字节流/位操作」底座原语
（阶段 0b），算法本体仍 100% tie 实现——**能造轮子就不用 Rust 底座**。
H.264/H.265/AVC 视频编解码已排除（用户决策）。

## 1. 分类总览

| 类 | 归属 | 算法 |
| --- | --- | --- |
| 数值分析 | **std/exmath.tie**（扩展现有） | Monte-Carlo、数值微分、数值积分、欧拉法、龙格-库塔法 |
| 数据拟合/参数估计 | **std/exmath.tie** | 最小二乘拟合（线性/多项式）、参数估计（矩/极大似然） |
| 插值 | **std/exmath.tie** | 线性插值、拉格朗日插值、牛顿插值、样条插值 |
| 线性代数 | **std/linalg.tie**（新） | 高斯消元、LU 分解、矩阵乘法/转置/行列式/逆、特征值（幂法） |
| 图算法 | **std/graph.tie**（新） | Dijkstra、Floyd、Prim、Bellman-Ford、最大流、二分匹配 |
| 搜索/规划 | **std/optsearch.tie**（新） | 回溯搜索、分治算法、分支限界、常见规划算法（贪心/动态规划） |
| 数据压缩 | **enl/compress.tie**（新） | 霍夫曼（已在 exmath）、LZ77/LZSS、LZW、算术编码 |
| 机器学习 | **enl/ml.tie**（新） | 决策树、支持向量机（SVM） |
| 现代压缩/多媒体 | **enl/codec/**（tie 实现，前置字节流原语） | Zstandard、Brotli、LZ4、JPEG、MP3 |

## 2. 分层原则（为什么这样分）

### 进 std/ 的标准：无状态纯函数，输入输出确定性，无全局可变状态
- 数值/拟合/插值/线性代数/图/搜索——输入数据 + 参数 → 输出结果，不依赖外部环境；
- 同一命名空间函数可相互调用（如 exmath.is_prime 供 sieve 内部使用）；
- 与现有 std 风格一致（assert/string/math/csv/format/exmath）。

### 进 enl/ 的标准：有状态 / 应用级 / 依赖 std，贴近具体应用场景
- **压缩**：需要维护编码表/字典等运行状态，且压缩率依赖上下文 → 应用级；
- **机器学习**：训练过程有模型状态（树结构/支持向量），推理依赖训练产物 → 有状态；
- 与现有 enl/log（控制台信息库，有状态 i18n）定位一致。

### 留在 Rust 底座原语的标准：tie 语言表达不了
- 逐字节位流解析（JPEG 的熵编码、压缩算法的位流打包）——**仅指字节/位级系统原语**，
  算法本体仍用 tie 实现；
- 需要二进制文件 IO（tie 只有文本 file_read/write）——补 byte_read/byte_write 原语后解决；
- 超大规模实时数值运算（实时视频编解码的帧率要求）。

> 注：H.264 / H.265 / AVC 已从路线图移除（用户决策）——不实现视频编解码。

## 3. 逐算法详细分类

### 3.1 std/exmath.tie（数值分析 + 拟合 + 插值，扩展现有命名空间）

| 算法 | 函数签名 | 说明 |
| --- | --- | --- |
| Monte-Carlo | `monte_carlo_pi(n: i64) -> f64` | 随机撒点估 π（rand_range 或新增原语） |
| 数值微分 | `diff(f: 表, x: f64, h: f64) -> f64` | 中心差分 `(f(x+h)-f(x-h))/2h`；f 用「采样表」传参（tie 无函数指针） |
| 数值积分 | `integrate(f: 表, a: f64, b: f64, n: i64) -> f64` | 梯形法/辛普森法（采样表） |
| 欧拉法 | `euler(f: 表, y0: f64, x0: f64, x1: f64, h: f64) -> f64` | 一阶 ODE 数值解 |
| 龙格-库塔 | `rk4(f: 表, y0: f64, x0: f64, x1: f64, h: f64) -> f64` | 四阶 RK，比欧拉更准 |
| 线性拟合 | `fit_line(xs: 表, ys: 表) -> (a: f64, b: f64)` | 最小二乘 y=ax+b |
| 多项式拟合 | `fit_poly(xs: 表, ys: 表, deg: i64) -> 表` | 最小二乘多项式（正规方程→高斯消元） |
| 参数估计 | `mean(xs: 表) -> f64` / `variance(xs: 表) -> f64` | 矩估计（均值/方差） |
| 线性插值 | `lerp(x0: f64, y0: f64, x1: f64, y1: f64, x: f64) -> f64` | 两点线性插值 |
| 拉格朗日插值 | `lagrange(xs: 表, ys: 表, x: f64) -> f64` | 经典多项式插值 |
| 牛顿插值 | `newton_interp(xs: 表, ys: 表, x: f64) -> f64` | 差商表 |
| 三次样条 | `spline(xs: 表, ys: 表, x: f64) -> f64` | 自然样条（三对角矩阵→追赶法） |

> 函数指针限制：tie 无函数类型，数值方法（微分/积分/ODE）的「被积函数」用
> **采样表**（x 与 y 的并行表）传参。这符合 tie 现有风格（无高阶函数）。

### 3.2 std/linalg.tie（新：线性代数，命名空间 linalg）

| 算法 | 函数签名 | 说明 |
| --- | --- | --- |
| 矩阵乘法 | `mat_mul(a: 表, b: 表, n: i64) -> 表` | 表存矩阵（按行拼接 + 尺寸参数） |
| 矩阵转置 | `mat_trans(a: 表, n: i64) -> 表` | |
| 行列式 | `det(a: 表, n: i64) -> f64` | 高斯消元（选主元） |
| 矩阵求逆 | `mat_inv(a: 表, n: i64) -> 表` | 高斯-约当 |
| 高斯消元 | `gauss(a: 表, b: 表, n: i64) -> 表` | 解线性方程组（列主元） |
| LU 分解 | `lu_decompose(a: 表, n: i64) -> (l: 表, u: 表)` | Doolittle 法 |
| 特征值 | `eigen_power(a: 表, n: i64, iter: i64) -> f64` | 幂法求最大特征值（主特征值） |

> 矩阵表示：tie 表元素须同类型，矩阵用**一维 i64/f64 表按行拼接 + n 尺寸参数**表达
>（`mat_at(mat, i, j, n) = mat[i*n+j]`）。行向量、增广矩阵同样处理。
> 消元过程中的行变换：阶段 0（表下标赋值）落地前用「重建表」实现（正确但 O(n³) 变
> O(n⁴)）；落地后直接用 `mat[i*n+j] = v` 原地更新，回归自然 O(n³)。

### 3.3 std/graph.tie（新：图算法，命名空间 graph）

| 算法 | 函数签名 | 说明 |
| --- | --- | --- |
| Dijkstra | `dijkstra(adj: 表, n: i64, src: i64) -> 表` | 单源最短路（O(V²) 未优化） |
| Floyd | `floyd(dist: 表, n: i64) -> 表` | 全源最短路（动态规划） |
| Prim | `prim(adj: 表, n: i64) -> 表` | 最小生成树（O(V²)） |
| Bellman-Ford | `bellman_ford(edges: 表, n: i64, src: i64) -> 表` | 负权最短路 + 负环检测 |
| 最大流 | `max_flow(cap: 表, n: i64, s: i64, t: i64) -> i64` | Ford-Fulkerson（DFS 增广） |
| 二分匹配 | `bipartite_match(adj: 表, n: i64, m: i64) -> i64` | 匈牙利算法（DFS 增广） |

> 图表示：邻接矩阵用一维表（同 linalg）；邻接表用「struct 表」（GraphEdge{to, w}）
> 或「并行表」。dist/visited 等数组的更新：阶段 0 前用重建表，落地后用下标赋值。

### 3.4 std/optsearch.tie（新：搜索与规划，命名空间 opt）

| 算法 | 函数签名 | 说明 |
| --- | --- | --- |
| 回溯搜索 | `n_queens(n: i64) -> i64` / `subset_sum(xs: 表, target: i64) -> 表` | 八皇后/子集和（递归回溯） |
| 分治算法 | `merge_sort(xs: 表) -> 表` / `quick_sort(xs: 表) -> 表` / `max_subarray(xs: 表) -> i64` | 归并/快排/最大子段和 |
| 分支限界 | `knapsack(items: 表, cap: i64) -> i64` | 0-1 背包（上界剪枝） |
| 规划（贪心） | `activity_select(starts: 表, ends: 表) -> i64` / `huffman`（已在 exmath） | 活动选择/哈夫曼贪心 |
| 规划（DP） | `knapsack_dp(items: 表, cap: i64) -> i64` / `lis(xs: 表) -> i64` | 背包 DP/最长递增子序列 |

### 3.5 enl/compress.tie（新：数据压缩，命名空间 compress）

| 算法 | 函数签名 | 说明 |
| --- | --- | --- |
| LZ77/LZSS | `lz77_compress(s: string) -> 表` / `lz77_decompress(表) -> string` | 滑动窗口匹配（字典在窗口内） |
| LZW | `lzw_compress(s: string) -> 表` / `lzw_decompress(表) -> string` | 动态字典（字符串 id 表不可用 → 并行表模拟） |
| 算术编码 | `arith_encode(s: string) -> 表` / `arith_decode(表) -> string` | 区间划分（用 i64 定点避免浮点精度） |
| 霍夫曼 | `huffman_build/encode/decode`（已在 std/exmath） | 已实现，compress 可引用 |

> 为何 enl：压缩需维护编码表/字典/窗口等运行状态，属应用级；且这些算法输出
> 是「压缩数据表」（自定义格式），贴近具体应用而非纯数学工具。

### 3.6 enl/ml.tie（新：机器学习，命名空间 ml）

| 算法 | 函数签名 | 说明 |
| --- | --- | --- |
| 决策树 | `tree_train(xs: 表, ys: 表) -> 表` / `tree_predict(tree: 表, x: 表) -> i64` | ID3（信息增益）；树用 struct/并行表存储 |
| 支持向量机 | `svm_train(xs: 表, ys: 表) -> (w: 表, b: f64)` / `svm_predict(w: 表, b: f64, x: 表) -> i64` | 线性 SVM（感知机/梯度下降；核函数留后续） |

> 为何 enl：训练产生模型状态（树结构/权重向量），推理依赖该状态 → 有状态应用。
> 首版做**线性可分**场景（感知机式 SVM）；核方法/软间隔留后续。

### 3.7 现代压缩与多媒体（tie 实现，前置字节流底座原语）

**原则（用户决策）**：能造轮子就不用 Rust 底座——算法**全部用 tie 实现**，
Rust 只补「语言底座原语」中**语言层无法自举**的系统能力（字节流读写、位流打包），
算法本体（熵编码、DCT、变换、预测）不依赖任何外部 crate。

| 算法 | 前置语言能力 | 说明 |
| --- | --- | --- |
| LZ4 | 字节流 IO + 位流打包 | LZ 类滑窗/字典匹配，结构最简单——编解码器首个里程碑 |
| Zstandard / Brotli | 字节流 IO + 位流打包 | Zstd 用 FSE/Huffman 熵编码后端，Brotli 用 LZ77+静态 Huffman——工作量最大的一档，拆多个 .tie 模块 |
| JPEG | 字节流 IO + 位流 + 余弦变换 | tie 实现 DCT（或整数近似）、量化、ZigZag、霍夫曼/算术熵编码 |
| MP3 | 字节流 IO + 位流 | tie 实现 MDCT 变换、心理声学模型简化版、哈夫曼熵编码 |

> 处置：**算法本体全部用 tie 语言编写**，放入 enl/（有状态编解码器）或独立
> `enl/codec/` 子目录。Rust 侧仅新增语言底座原语：
> `byte_read(file) / byte_write(file, bytes) / bit_stream_new / bit_read / bit_write` 等
> （系统级字节/位操作，语言层表达不了的才是原语）。
> 阶段划分：先补字节流原语 → 实现 LZ4（相对简单，验证完整链路）→ JPEG →
> MP3 → Zstd/Brotli（熵编码后端最复杂，最后攻坚）。
> 已排除：**H.264 / H.265 / AVC（用户决策）**——视频编解码不实现。

### 3.8 阶段 0b（前置）：字节流 / 位操作底座原语（Rust，算法轮子的地基）

> 与 3.7 配套：这些是「语言层无法自举」的系统能力（文件字节级 IO、位打包），
> 属语言底座原语；**算法本体仍 100% tie 实现**。

| 原语 | 签名 | 用途 |
| --- | --- | --- |
| 字节读 | `byte_read(path) -> table` | 读文件为字节表（i64 0..255） |
| 字节写 | `byte_write(path, bytes: table) -> bool` | 字节表写文件 |
| 位读 | `bit_read(bits: table, pos: i64) -> i64` | 读位流第 pos 位 |
| 位写 | `bit_write(...)` | 位流打包（熵编码后端） |
| 字节拼接 | `byte_concat(a: table, b: table) -> table` | 字节表合并 |

> 说明：这些原语给 tie 提供「字节/位」的最小系统地基；LZ77/LZW/霍夫曼/算术编码
> 等算法逻辑全在 tie 里写。与「表下标赋值」（阶段 0）配合，编解码器可自然实现。

## 4. 实施顺序（依赖关系）

### 阶段 0（前置）：表下标赋值能力（M4 补齐语言特性）

> **为什么先做**：全部数学算法（线性代数/图/搜索）都需要「原地更新容器元素」——
> Dijkstra 更新 dist[i]、高斯消元改矩阵行、DP 填表。当前 tie 表**只读 + 追加**
> （`t[i] = v` 报「赋值目标必须是变量或对象字段」），迫使算法用「重建表」表达，
> 高斯消元 O(n³) 退化为 O(n⁴)、图算法 dist 更新每次全表拷贝——正确但不可用。
> 补上表下标赋值后，算法实现回归自然形态，且能力本身是 M6 包管理器
> （依赖表/解析表操作）的底座。

**设计（四层同步，沿用 M2.1.8 FieldAssign 的「新增变体零触碰现有路径」模式）**：

| 层 | 改动 |
| --- | --- |
| AST（tie-frontend/ast.rs） | 新增 `Stmt::IndexAssign(IndexAssignStmt)`：`base: Box<Expr>`（限变量/下标链）、`index: Box<Expr>`、`op: Option<BinaryOp>`、`value: Expr`、`span`。与 [AssignStmt]（target 是 String 快路径）分开，零触碰既有路径 |
| Parser（parser.rs） | `parse_expr_or_assign`：解析完整表达式后，若后跟赋值运算符且表达式是 `Index` 链 → `IndexAssign`（仿 FieldAssign 分支：`t[i] = v`、`t[i][j] = v`、`t[i] += v` 复合赋值） |
| Semantic（semantic.rs） | `Stmt::IndexAssign` 分析：base 必须是表变量（含下标链）且**可寻址**；index 类型 i64；元素类型匹配校验（`t[i] = v` 的 v 与表元素类型一致，与 table_push 同规则）；const 表不可改 |
| Interp（tie-interp/lib.rs） | `Value::Table(Vec)` 下标写入：越界 → 报错（文本与 table_at 越界一致「下标越界」）；复合赋值先读后写（复用 eval_compound_assign 语义） |
| IR（tie-llvm/ir.rs） | 表存储是 C ABI 桥 DynTable 指针：新增 `tie_table_set_*` 系列桥（i64/f64/string/bool）或「读旧值 + 写回」复合操作；定长表（字面量表）用 LLVM 数组 GEP store |

**验收**：`t[0] = 9` / `t[i] += 1` / 二维 `t[i][j] = v` 编译运行正确；越界/类型错误有测试；
workspace 测试全绿。**此阶段完成后，阶段一至四的算法全部用自然形态实现**（不再重建表）。

### 第一批（std/exmath 扩展，无外部依赖）
  lerp / lagrange / mean / variance / diff / integrate / monte_carlo_pi
  → 拟合（fit_line → fit_poly 依赖 gauss）→ euler / rk4

### 第二批（std/linalg，exmath 的 fit_poly 依赖它）
  mat_mul / mat_trans / det / gauss / mat_inv / lu_decompose / eigen_power

### 第三批（std/graph，独立）
  dijkstra / floyd / prim / bellman_ford / max_flow / bipartite_match

### 第四批（std/optsearch，递归能力）
  merge_sort / quick_sort / max_subarray / n_queens / subset_sum / knapsack / lis

### 第五批（enl/compress，字节位运算 M4 已备）
  lz77 / lzss / lzw / arith

### 第六批（enl/ml，依赖 linalg）
  决策树（ID3）→ 线性 SVM

### 第七批（enl/codec/，前置阶段 0b 字节流原语）
  字节流原语落地后：LZ4（编解码首个里程碑，验证完整链路）→ JPEG（DCT+霍夫曼）→
  MP3（MDCT+心理声学简化版）→ Zstd/Brotli（FSE/静态霍夫曼熵编码后端，最后攻坚）。
  全部 tie 实现，不调任何 Rust 编解码 crate。

## 5. 验收标准

- 每个算法有独立 demo（examples/alg_*.tie）验证正确性（已知输入 → 已知输出）；
- std 库算法纯函数（同输入同输出，无全局状态）；
- enl 库算法有状态但接口清晰（训练/推理分离）；
- 编解码器端到端：tie 压缩 → tie 解压 → 内容一致（roundtrip 验收）；
- 全部 workspace 测试全绿、编译零错误；
- 分类文档（本文件）与 README 标准库/扩展库章节同步更新。

## 6. 不做（明确排除）

- **H.264 / H.265 / AVC（用户决策）**——视频编解码不实现；
- 编解码性能对标原生库（tie 版以正确性/可读性为目标，性能优化待「表下标赋值」等能力成熟）；
- 大规模线性代数性能优化（表只读约束下 O(n³)→O(n⁴) 可接受，优化待「表下标赋值」能力）；
- SVM 核方法/软间隔、决策树剪枝——首版线性/基础版；
- 完整 MP3 心理声学模型（首版简化版，验证编码链路即可）。

## 7. 相关文件

| 文件 | 作用 |
| --- | --- |
| std/exmath.tie | 数值/拟合/插值（已有霍夫曼，扩展中） |
| std/linalg.tie | 线性代数（新建） |
| std/graph.tie | 图算法（新建） |
| std/optsearch.tie | 搜索与规划（新建） |
| enl/compress.tie | 数据压缩（新建） |
| enl/ml.tie | 机器学习（新建） |
| examples/alg_*.tie | 每算法一个演示 |
| docs/plans/package-manager.md | M4 补齐阶段零引用本文件 |
| README.md / CHANGELOG.md | 标准库/扩展库清单更新 |
