# 规划：tie 文件角色扩展（R2 多角色叠加 + R3 角色参数化）

> 状态：**规划**（2026-08-15 设计讨论定稿，未实现）
> 本文档定义 tie 文件角色体系的扩展。决策汇总：
> **R2**（多角色叠加：基础角色唯一 + 修饰角色可叠加）+ **R3**（角色参数化）。
> 覆盖：unsafe/owned/embedded 修饰角色、tieir 分发角色、test/bench 领域角色、
> **db:vector 向量数据库角色**、ui/port 落地、角色语法子集约束。
> 关联：unsafe 模型（文件级 unsafe U3）、内存模型（owned 模式 P3）、
> 并发模型（embedded 子集）、包模型（tieir 分发）、UI 框架（ui/port 实现）。

## 1. 现状（10 角色，3 个挂接点未实现）

| 角色 | 状态 | 使用量 |
| --- | --- | --- |
| class / logic | ✅ 实现 | 213 / 186 |
| data / script / db / ir / zd | ✅ 实现 | 7 / 4 / 2 / 2 / 2 |
| **ui / port** | 🟡 **挂接点未实现**（提示） | 0 / 0 |
| type | ✅ 泛型入口 | 1 |

讨论中已隐含 4 个新角色需求：unsafe（文件级）、owned（所有权模式）、
embedded（嵌入式）、tieir（IR 分发）。

## 2. 角色机制（R2 多角色叠加 + R3 参数化）

### 2.1 语法

```tie
type tie<基础角色[:参数], 修饰角色...>
```

```tie
type tie<ui:window>              // 基础角色 + 参数（R3）
type tie<class, unsafe>          // 基础 + 修饰（R2）
type tie<ui:window, unsafe>      // 组合：参数化基础 + 修饰
type tie<db:schema, owned>       // db schema + 所有权模式
type tie<port, embedded>         // 嵌入式接口文件
```

### 2.2 角色分类（正交模型）

```
基础角色（唯一，表达"用途"）：
  logic / script / class / data / db / ui / port / ir / tieir / zd / type / test / bench

修饰角色（可叠加，表达"模式"）：
  unsafe / owned / embedded
```

- **基础角色唯一**：一个文件只能选一个（表达文件做什么）
- **修饰角色可叠加**：一个文件可多个（表达怎么解释）
- 参数（R3）：给基础角色细分形态（`ui:window` / `db:schema`）

### 2.3 合法性规则

| 组合 | 合法 | 说明 |
| --- | --- | --- |
| `class, unsafe` | ✅ | unsafe 类库（tieuicore 内部） |
| `logic, owned` | ✅ | 所有权模式可执行 |
| `ui:window, embedded` | ✅ | 嵌入式窗口 UI（M7） |
| `ui:window, unsafe` | ✅ | 窗口 UI + unsafe（系统 API 直连） |
| `port, embedded` | ✅ | 嵌入式接口文件 |
| `data, unsafe` | ⚠️ | data 只允许数据定义——unsafe 无意义但允许（约束由基础角色管） |
| `class, class` | ❌ | 基础角色重复 = 错误 |
| `unsafe, unsafe` | ❌ | 修饰重复 = 错误 |
| 无基础角色 | ❌ | 必须有一个基础角色（默认 logic） |

- 检查在 prep（头部解析）+ semantic（约束）

## 3. 模式类修饰角色

| 修饰角色 | 语义 | 来源设计 |
| --- | --- | --- |
| `unsafe` | 文件级 unsafe：整文件可触底（内部零标记） | unsafe 模型 U3 逃生舱 |
| `owned` | 所有权模式：默认移动语义（live/moved 检查启用） | 内存模型 P3 |
| `embedded` | 嵌入式约束：协作式协程/静态池/禁用 spawn/线程原语 | 并发模型 tie:embedded |

### 3.1 语义叠加

- `unsafe` 只放大能力边界（允许触底），不改变语法子集
- `owned` 改变语义（默认移动），需要 semantic 检查启用
- `embedded` 收紧能力（禁用 OS 特性），违规 = 编译错误

```tie
// 文件级 unsafe + 所有权模式 + 嵌入式
type tie<class, unsafe, owned, embedded>
```

## 4. 领域类角色

### 4.1 分发类：tieir

```tie
type tie<tieir>     // 预编译为 tie-IR 分发（包模型 L2 决策）
```

- 管线：前端 + IR 生成 → tieir 文本序列化（不生成机器码）
- 与 `ir`（生成 LLVM IR .ll）区分：tieir 是 tie-IR 中间表示（平台无关），
  ir 是 LLVM IR（单平台）
- 发布到注册表/git 的形态（包模型 §2.2）

### 4.2 测试/基准：test / bench

```tie
type tie<test>      // 测试文件（tie test 扫描入口）
type tie<bench>     // 基准文件（tie bench 扫描入口）
```

- 复用 ext/test.tie（断言收集+统计）与 ext/bench.tie（基准计时）
- `tie test` / `tie bench` 按角色发现文件并运行
- 约束：只允许 func test_* / func bench_* 定义

### 4.3 参数化基础角色（R3）

| 角色:参数 | 语义 | 管线 |
| --- | --- | --- |
| `ui:window` | 桌面窗口 UI | tieui 编译（M2 后） |
| `ui:web` | Web UI | webui 编译（M5 后） |
| `ui:embedded` | 嵌入式 UI | tieui 嵌入式（M7） |
| `db:schema` | 数据库结构定义 | tieDB schema 生成 |
| `db:seed` | 种子数据 | tieDB 导入 |
| `db:vector` | **向量数据库集合定义**（维度/度量/索引） | tieDB 向量集合生成 |
| `data:config` | 配置数据文件 | 配置解析（ext/config） |
| `data:asset` | 资源数据（图标/文本） | 资源打包 |

- 参数不强制：`ui` 无参数 = 通用 UI（默认窗口）
- 未知参数 = 编译错误（参数表可扩展）

## 5. 角色语法子集约束

每个基础角色定义"允许什么"（semantic 层检查）：

| 角色 | 允许 | 禁止 |
| --- | --- | --- |
| data | record/数据定义/const | func/var 逻辑 |
| db:schema | record + 索引声明 | 逻辑 |
| db:vector | 向量集合定义（见 §5.1） | 逻辑 |
| port | port 声明 + 默认实现 | main、struct 实现体 |
| ui | view/layout/事件处理/组件树 | main（可无） |
| test | func test_* | main |
| bench | func bench_* | main |
| class | 库函数/struct/port/impl | main |
| tieir | 任意库内容（编译为 IR） | main |
| logic | 任意 + main | — |

- 违反约束 = 编译错误（"角色 data 不允许 func 定义"）
- 修饰角色不改变语法子集（unsafe 放大能力、owned 改语义、embedded 收紧能力）

### 5.1 向量数据库角色（db:vector）

```tie
type tie<db:vector>

// 向量集合定义：维度 + 度量方式 + 索引类型
vector_collection embeddings {
    dim: 128                  // 向量维度
    metric: cosine            // 度量：cosine / l2 / dot
    index: flat               // 索引：flat（精确）/ hnsw（近似，后置）
    payload: {                // 附加负载字段（元数据）
        id: i64
        text: string
    }
}
```

- 语义：声明式向量集合 schema（类似 db:schema 对关系表的声明）
- 生成：tieDB 向量集合（tiedb.connect/collection）+ ext/vecsearch Flat 索引
  （现有能力：L2/余弦距离 + 展平存储 add/remove/get/search top-k）
- 度量：cosine / l2 / dot（与 ext/vecsearch 对齐）
- 索引：第一版 flat（精确）；hnsw 等近似索引后置（新索引 = 新参数值）
- 管线：`db:vector` 文件 → 生成 tieDB 向量集合初始化代码 / 数据校验

## 6. 文件名声明系统（F1 连字符格式 + R3 强制一致）

### 6.1 命名格式

```
xxx.<基础角色>[-<参数>]-[<修饰角色>...].tie
```

```tie
app.class.tie              // class
app.class-unsafe.tie       // class + unsafe 修饰
db-vector.tie              // db:vector（参数用连字符，头部用冒号）
db-vector-embedded.tie     // db:vector + embedded
app.ui-window.tie          // ui:window
```

- **基础角色**：唯一，文件名第一段
- **参数**：连字符跟在基础角色后（`db-vector` = 头部 `db:vector`）
- **修饰角色**：追加连字符段（可多个）
- **Windows 约束**：文件名不允许冒号 → 参数在文件名中用连字符表达
  （压缩表示）；头部是规范表示（`type tie<db:vector>`）
- 无角色段 = 普通文件（默认 logic，或按头部声明）

### 6.2 与头部的关系（R3：强制一致）

| 场景 | 处理 |
| --- | --- |
| 文件名有角色，头部无声明 | 采用文件名（文件名 = 默认角色） |
| 文件名无角色，头部有声明 | 采用头部（头部优先） |
| 两边都有，**一致** | 正常 |
| 两边都有，**不一致** | **编译错误**（强制单一真相，无歧义） |

```tie
// 文件 db-vector.tie 头部写：
type tie<db:vector>     // ✅ 一致
type tie<db:schema>     // ❌ 编译错误：文件名声明 db:vector，头部声明 db:schema

// 文件 app.class-unsafe.tie 头部写：
type tie<class, unsafe> // ✅ 一致（顺序无要求）
type tie<class>         // ❌ 编译错误：缺少 unsafe 修饰
```

- **单一真相原则**：文件名与头部都是角色声明源，不一致 = 错误
  （升级自现状的"警告"，消除双源歧义）
- 顺序无关：`class-unsafe` 与头部 `type tie<class, unsafe>` /
  `type tie<unsafe, class>` 均一致（集合比较，非顺序比较）

### 6.3 解析规则（固定顺序）

1. 取 `.tie` 前的全部点分段，最后一段做角色解析
2. 第一段（基础角色）必须在基础角色白名单，否则：
   - 是已知文件名（如 `schema.db.tie` 的 db？）——按现状约定多段解析
   - 未知 → 按普通文件名处理（默认角色）
3. 后续连字符段：命中修饰角色白名单 = 修饰；否则报错（未知角色段）

### 6.4 与现有约定的兼容

- `xxx.<角色>.tie`（单角色，现状）：完全兼容（`app.script.tie`、`schema.db.tie`）
- `xxx.zd.tie`（压缩数据）：保持（基础角色 zd，无参数无修饰）
- **不考虑存量迁移**（2026-08-15 决策）：现状"警告"直接升级为"错误"；
  未来用户迁移用预处理脚本（--module 机制）

## 7. 与工具链管线映射

| 角色 | 管线 | 里程碑 |
| --- | --- | --- |
| `ui` | tieui 编译：组件树 → 目标平台 | M2（框架）/ M7（嵌入式） |
| `port` | 接口检查 + tieir 导出 | port-model 落地 |
| `tieir` | 前端+IR → tieir 序列化 | 包模型 tieir 分发 |
| `test` | `tie test` 发现运行 | 短里程碑 |
| `bench` | `tie bench` 发现运行 | 短里程碑 |
| `db:schema` | tieDB schema 生成 | tieDB 扩展 |
| `db:vector` | tieDB 向量集合生成（vecsearch Flat 索引） | tieDB 扩展（现有 vecsearch 能力） |

## 8. 编译器实现拆解（tiec 自举）

| 模块 | 改动 |
| --- | --- |
| prep（头部解析） | 多角色解析（逗号分隔 + 冒号参数）、合法性检查（基础唯一/修饰可叠） |
| prep（文件名解析） | 文件名角色段解析（连字符格式）、与头部一致性检查（R3 错误） |
| semantic | 角色语法子集约束、修饰角色语义（unsafe 边界/owned 检查/embedded 禁用） |
| 管线分派 | 按基础角色分派（新增 tieir/test/bench 管线） |
| CLI | `tie test` / `tie bench` 子命令（角色发现） |

## 9. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 角色机制 | R2 多角色叠加（基础唯一 + 修饰可叠）+ R3 参数化 | 单角色、纯修饰 |
| 修饰角色 | unsafe / owned / embedded（均来自已定设计） | — |
| 分发角色 | tieir（与 ir 区分：tie-IR vs LLVM IR） | — |
| 领域角色 | test / bench（复用 ext 已有框架） | 后置 |
| 参数化 | ui:window/web/embedded、db:schema/seed/**vector**、data:config/asset | 无参数 |
| 约束 | 角色语法子集 semantic 检查 | 无约束 |
| 文件名格式 | F1：`xxx.<基础>[-参数]-[修饰...].tie`（参数连字符，头部冒号） | 点号多段、仅基础角色 |
| 文件名一致性 | R3：不一致 = 编译错误（升级自现状警告） | 保持警告、文件名唯一真相 |

## 10. 未决问题

1. **参数表扩展机制**：`ui:xxx` / `db:xxx` 参数表由谁维护（编译器内建 vs 包注册）
2. **修饰角色与基础角色的深层交互**：`owned` 对 data 文件的意义（数据定义
   是否需要移动语义——可能无意义，允许但文档化）
3. ~~**角色与文件名约定的交互**~~ **已定案**：F1 连字符格式
   （`xxx.class-unsafe.tie`），参数连字符（`db-vector`），R3 不一致 = 错误
4. **`tie test` 的发现规则**：递归目录？test 目录约定？（第一版：当前目录
   直接子文件）
5. **角色版本化**：新角色随语言版本演进，旧编译器遇到未知角色怎么办
   （报错 vs 警告跳过）
6. ~~**R3 升级路径**~~ **已定案（2026-08-15）**：不考虑迁移——无用户，直接
   不一致 = 编译错误；未来用户迁移用预处理脚本（--module 机制）
