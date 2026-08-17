# 规划：tie 构建配置模型（config.data.tie 统一配置文件 + 分层合并 + profile）

> 状态：**已实现**（2026-08-16，S3.1 落地，commit 见 CHANGELOG）
> 本文档定义 tie 的构建配置模型。决策汇总：
> **L2**（三层：CLI > 项目 config > 用户全局配置）+ **统一 config.data.tie 文件**
> （type tie<data> 角色，分节配置 tiec/prep/pkg 等子工具）
> + **D1-D7 全 7 域**（target/backend/opt/features/roles/link/modules）
> + **P3 分层合并 + profile**（dev/release，Cargo 风格）。
> 关联：包模型（backend 实现选择 P4b）、角色模型（roles 目录）、
> 宏模型（modules 预处理链）、库/包模型（tie.pkg 与 config 的关系）。
>
> 实现记录：
> - **配置模块** `compiler/config.tie`（type tie<class> 独立库）：解析（parse/
>   parse_append/parse_file）、访问器（get_str/get_int/get_bool/get_list/type_of）、
>   分层合并（load_merge：内置默认 < 用户 < 项目 < profile < CLI）、合并引擎
>   （merge/merge_value：标量覆盖/列表追加/`"="` 重置/独有键保留）、profile 激活
>   （顶层 `profile` 键或 CLI `--profile`）、apply_cli（CLI 显式覆盖，map_set
>   原地改槽位）、dump 摘要。
> - **driver 集成**（compiler/driver.tie）：`--config <f>` / `--profile <p>` /
>   `--backend <b>` 三个新参数；opt/target 优先级 CLI 显式 > 配置（含 profile
>   激活）> 默认；backend 实现选择（win32/LLVM 为当前唯一后端，其余 port 明确
>   报错）。
> - **验收**：tests/s31/config_smoke.tie 48 断言全绿；配置驱动 backend 选择
>   端到端验证（config 写 wasm → 报错，写 win32 → 正常编译）；自举回归全绿。
> - **关键修复**：全局扁平键值表布局纪律——所有构造路径先收集后统一登记，
>   杜绝嵌套子表交错 push 破坏父表键区间连续性的 bug。

## 1. 现状盘点

| 现有机制 | 内容 |
| --- | --- |
| tie.config | 协调统筹配置（advanced 多文件并行 + cache 缓存池），tie:data 格式 |
| tie.pkg | 包清单（name/version/description/role/main/author/license/dependencies） |
| CLI | -o / -O0-3 / --target / --emit-ir / --keep-ir / --prep-only / --config / --module |
| 环境变量 | TIE_LLVM_HOME（LLVM 工具发现）、TIE_REGISTRY（注册表基址） |

**新设计引入的配置需求**：backend 实现选择（P4b）、语言特性裁剪（embedded）、
test/bench 目录、预处理链、profile、用户级配置。

## 2. 配置文件（统一 config.data.tie）

### 2.1 单一配置文件，分节配置各子工具

```tie
// config.data.tie —— 统一构建配置文件（type tie<data> 角色，纯数据）
type tie<data>

[
    // ── 通用节：跨工具共享 ──
    "target": "win-x64",            // 目标平台
    "opt": 2,                       // 优化级别
    "debug": true,                  // 调试模式（断言/越界检查开）
    "profile": "dev",               // 当前 profile（P3）

    // ── tiec 节：编译器 ──
    "tiec": [
        "backend": "win32",         // port 实现选择（P4b）
        "features": ["async", "macro", "unsafe"],   // 语言特性开关
        "emit": "exe",              // exe / a / tieir / ll（产出形态）
        "link": ["user32", "gdi32"],  // 系统库链接
        "bounds_check": true,       // 越界检查
    ],

    // ── prep 节：预处理器 ──
    "prep": [
        "modules": ["migrate_str_v1.tie"],   // 预处理链（--module 机制）
        "strict_roles": true,       // 角色约束严格检查
    ],

    // ── pkg 节：包管理器 ──
    "pkg": [
        "registry": "https://reg.tie-lang.org",  // 注册表基址
        "cache_dir": ".tie/deps",
        "verify_signature": true,   // 签名校验（P5c）
    ],

    // ── roles 节：角色管线 ──
    "roles": [
        "test": ["tests/"],
        "bench": ["bench/"],
    ],

    // ── advanced/cache 节：现状保留 ──
    "advanced": [ "enabled": true, "threads": 0 ],
    "cache": [ "size": 268435456, "storage": "memory", "path": ".tie-cache" ],
]
```

- **单一文件**：`config.data.tie`（type tie<data> 角色，纯数据）
- **分节**：通用节 + tiec/prep/pkg 各子工具节 + roles + advanced/cache（现状兼容）
- 文件名：`config.data.tie`（与 xxx.<角色>.tie 命名约定一致，data 角色）
- 解析：复用 tie:data 表解析（现有 manifest 解析器同层）

### 2.2 与 tie.pkg 的分工

| 文件 | 职责 | 场景 |
| --- | --- | --- |
| tie.pkg | 包元数据（name/version/deps/role/main） | 发布/依赖解析 |
| config.data.tie | 构建行为（target/backend/opt/features/...） | 编译/构建 |

- **职责分离**：元数据 vs 行为——tie.pkg 管"是什么"，config 管"怎么构建"
- tie.pkg 可含 `config` 字段指向默认 config（包发布携带默认构建配置，
  消费者 `tie build` 时应用）

## 3. 配置层级（L2：三层）

```
CLI 参数          （最高优先：--target win-x64 覆盖一切）
  │
项目 config.data.tie （仓库内，团队共享）
  │
用户 ~/.config/tie/config.data.tie （个人偏好：默认优化、LLVM 路径）
  │
内置默认值         （最低：target=本机, opt=2, ...）
```

- 项目级：仓库根目录 `config.data.tie`
- 用户级：`~/.config/tie/config.data.tie`（Windows: `%USERPROFILE%\.config\tie\`）
- 环境变量保留特例：TIE_LLVM_HOME / TIE_REGISTRY（现状，不并入层级）

## 4. 内容域（D1-D7 全覆盖）

| 域 | 键 | 说明 |
| --- | --- | --- |
| D1 目标平台 | `target` | win-x64 / linux-arm64 / wasm32 / embedded / LLVM 三元组 |
| D2 后端选择 | `tiec.backend` | win32 / x11 / wasm / embedded（port 实现绑定，P4b） |
| D3 优化 | `opt` / `debug` | 0-3；debug 开断言/越界检查 |
| D4 特性开关 | `tiec.features` | async/macro/unsafe/owned/embedded 语言特性裁剪 |
| D5 角色管线 | `roles.test` / `roles.bench` | test/bench 发现目录 |
| D6 链接 | `tiec.link` | 系统库（user32/gdi32/m） |
| D7 宏/预处理 | `prep.modules` | 预处理链（--module 挂载） |

## 5. 合并与优先级（P3：分层合并 + profile）

### 5.1 profile（Cargo 风格，第一版即支持）

```tie
// config.data.tie 的 profiles 节
[
    "profiles": [
        "dev": [
            "debug": true,
            "opt": 0,
            "tiec": [ "bounds_check": true ],
        ],
        "release": [
            "debug": false,
            "opt": 2,
            "tiec": [ "bounds_check": false ],
        ],
    ],
]

// 使用：当前 profile 由顶层 "profile" 键或 CLI 指定
tie build --profile release      // CLI 覆盖 profile
```

- profile 定义在 config（profiles 节），激活经顶层 `profile` 键或 CLI
- 合并顺序：内置默认 < 用户 config < 项目 config < profile 覆盖 < CLI

### 5.2 合并规则

- 同键值合并：profile 未定义的键继承基础配置（非覆盖全节）
- 冲突：CLI 直接覆盖（不警告，显式意图）；层间冲突低层被高层覆盖
- 列表键（features/link/modules）：**追加合并**（父层列表 + 本层列表），
  除非显式 `"features": "="`（重置标记）
- 未知键：警告（防拼写错误，不阻塞）

## 6. 与包模型/角色模型/宏模型的咬合

| 机制 | 配置落点 |
| --- | --- |
| 包模型 P4b（backend 实现选择） | `tiec.backend`：win32/wasm/x11 绑定 port 实现 |
| 包模型 P5c（签名校验） | `pkg.verify_signature` |
| 角色模型（test/bench 发现） | `roles.test` / `roles.bench` 目录 |
| 角色模型（角色约束） | `prep.strict_roles` |
| 宏模型（预处理链） | `prep.modules`（--module 机制配置化） |
| 嵌入式裁剪 | `tiec.features` 禁用 spawn/线程（配合 tie:embedded） |
| 字符串迁移脚本 | `prep.modules`: ["migrate_str_v1.tie"] |

## 7. 编译器/工具链实现拆解

| 模块 | 改动 |
| --- | --- |
| config 解析 | config.data.tie 读取/解析（复用 tie:data 表解析） |
| 层级合并 | 三层 + profile 合并引擎（优先级/追加/重置规则） |
| CLI 覆盖 | CLI 参数 → 覆盖合并结果（最高优先） |
| 分发 | tiec/prep/pkg 各自读取自己节的配置（单一文件分节） |
| profile | profiles 节解析 + 激活（顶层键/CLI --profile） |

## 8. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 层级 | L2：CLI > 项目 config > 用户全局 config | 单层、四层（+环境变量） |
| 载体 | 统一 config.data.tie（type tie<data>，分节 tiec/prep/pkg） | tie.config 扩展、专用构建文件 |
| 内容域 | D1-D7 全覆盖（target/backend/opt/features/roles/link/modules） | 子集 |
| 优先级 | P3：分层合并 + profile（dev/release） | CLI 覆盖一切、冲突警告 |
| 与 tie.pkg | 职责分离（元数据 vs 行为）；tie.pkg 可携带默认 config 引用 | 合并进 tie.pkg |

## 9. 未决问题

1. **配置文件的 schema 校验**：tie:data 纯数据无类型检查——未知键警告够吗？
   需要 schema 声明（后续 profile 类型化）？
2. **profile 继承**：profile 间继承（`"release": { "extends": "dev" }`）？
   （第一版不做，扁平 profile）
3. **多项目共享配置**：monorepo 子项目继承根 config（`"extends": "../config.data.tie"`）？
4. **wasm 特化配置**：wasm 目标的特有选项（worker 数、内存上限）放 tiec 节
   还是独立 wasm 节
5. **config 的宏/条件**：配置能否用宏生成（复杂构建矩阵）——T3 eval 逃生舱？
