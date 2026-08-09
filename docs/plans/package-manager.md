# 规划（Harbor M6）：包管理器——用 tie 语言自己写

> 状态：**规划**（已排期，待实现）
> 所属：Harbor（2026.1）架构 M6
> 依赖：Harbor M5（动态库编译）已完成规划；包管理器是「用 tie 语言扩展 tie 工具链」
> 自举路线的最高形态——**整个包管理器（含 CLI 子命令解析）全部用 tie 语言编写**。
> 前置：**M4 补齐**（语言能力扩展，见 §3.3）——M4 尚未完全收官，M6 依赖的语言能力
> 归入 M4 补齐阶段先行完成。

## 1. 背景与现状

### 1.1 tie 工具链已打通的自举路径

| 里程碑 | 自举内容 | 模式 |
| --- | --- | --- |
| M2.2/M3 | `prep/core.tie` 预处理器核心 tie 语言化，Rust 壳解释执行（tie:script 协议） | tie 写核心逻辑 + Rust 薄壳 |
| M3 | REPL 外壳 `repl/repl.tie` 用 tie 语言自写，编译链接 interp 静态库生成 `repl.exe` | tie 写完整程序 + 编译执行 |
| M4 | 标准库/扩展库全 tie 写（`std/` + `ext/`），语言自身成为自举工具库 | tie 写库 |

### 1.2 缺口：包管理能力为零

- 项目**依赖**目前只能靠手写 `import` 相对路径（`../std/string.tie`），无版本、无解析、无锁文件；
- tie 语言**底座原语不足**：无网络（HTTP 下载）、无进程执行（调 git/编译器）、无解压（zip/tar.gz）、
  无递归目录操作、无路径工具、无环境变量——这些恰是包管理器的基本操作；
- `tie` 命令是单形态入口（编译/REPL/LSP），没有子命令体系（`tie init` / `tie install` / `tie build`）。

### 1.3 M5 的铺垫

M5 动态库编译完成后，库可以 `.dll`/`.so` 形态被 C 等其他语言消费；M6 包管理器负责
把 tie 编写的库/程序**组织、分发、复用**起来——两者共同构成 tie 的生态基础。

## 2. 目标

`tie` 成为「万能命令」——一个入口覆盖全部日常操作，包管理器是其子命令体系：

```
tie init [name]             初始化项目（生成 tie.pkg 清单 + 主文件模板）
tie add <pkg>[@version]     声明依赖（path / git / registry 三种源）
tie remove <pkg>            移除依赖
tie install [--dev]         解析 + 拉取全部依赖，生成 tie.lock 锁文件
tie update [pkg]            更新依赖到允许的最新版本
tie build [--release]       构建项目（复用 tie 编译工具链）
tie run [args...]           构建并运行
tie test                    运行项目测试（扩展）
tie publish                 打包发布到注册表（git tag + push / HTTP 上传）
tie search <query>          搜索注册表
tie info <pkg>              查询包信息
tie clean                   清理构建产物
tie cache [clean]           包缓存管理（扩展）
```

**核心约束（用户决策）**：

1. **包管理器 100% 用 tie 语言实现**——依赖解析、版本比较、清单解析、CLI 子命令解析、
   帮助文本、安装/构建/发布流程编排，全部是 `.tie` 源码；
2. Rust 侧只做两件事：**补齐语言底座原语**（网络/进程/解压/目录/路径/环境）+
   **最薄加载壳**（识别子命令 → 执行 tie 写的包管理器程序）；
3. **包源混合**：本地路径（`file://`/相对路径）+ git 仓库（https **与 ssh** `git@host:...`）+
   HTTP(S) 注册表三种源；git 源经系统 git（ssh 由用户密钥认证，tie 不碰凭据）。

## 3. 总体架构

### 3.1 执行形态：编译执行（pkg.exe），沿用 REPL 自举模式

包管理器是一个**多文件 tie 程序**（需 import/命名空间/多模块），因此**不能**走
tie:script 单文件自包含约束（`eval` 不支持 import）。采用与 `repl.exe` 相同的自举路径：

```text
pkg/main.tie（入口，含 CLI 子命令解析）
   ├── import pkg/cli.tie       参数解析 / 帮助文本 / 子命令分派
   ├── import pkg/manifest.tie   tie.pkg 清单解析（tie:data → 依赖表）
   ├── import pkg/version.tie    semver 解析/比较/区间匹配
   ├── import pkg/deps.tie       依赖图解析/去重/冲突检测
   ├── import pkg/fetch.tie      包源拉取（path 复制 / git clone / http 下载解压）
   ├── import pkg/install.tie    安装到 .tie/deps 与全局缓存
   ├── import pkg/build.tie      构建编排（调用 tie 编译器）
   ├── import pkg/run.tie        运行编排
   ├── import pkg/publish.tie    打包 tar.gz / git tag push / http 上传
   └── import pkg/search.tie     注册表索引查询
        │
        ▼ （tie-llvm 编译，链接 interp 静态库）
     pkg.exe
```

- 构建步骤：`cargo build --release -p tie-interp` → `tie-llvm pkg/main.tie` → `pkg/pkg.exe`；
- `tie` 入口发现第一个参数是子命令名（`init`/`add`/`install`/…）→ 查找并执行 `pkg.exe`
  （查找顺序同 `repl.exe`：`TIE_PKG_EXE` → tie.exe 同目录 → 当前目录 → `pkg/` 目录）；
- 找不到时给出构建提示（与 REPL 未找到 repl.exe 一致的体验）。

> 备选：interp `eval` 支持 import 后，pkg 也可解释执行。但编译执行更符合
> 「tie 写完整工具」的自举精神，且性能好、可独立分发。规划以编译执行为主。

### 3.2 新 crate：`crates/tie-pkg`（Rust 薄壳）

| 职责 | 内容 |
| --- | --- |
| 子命令识别 | 判断首个参数是否为包管理命令，是则转 pkg.exe 执行（不透传编译参数） |
| pkg.exe 查找 | 环境变量 → exe 同目录 → 当前目录 → workspace `pkg/`（同 repl.exe 模式） |
| 注册表索引 | 供 `search`/`info` 用（下载索引 tie:data/JSON → 文本交回 tie 程序处理）——首版可无 |
| 构建自举 | 提供「编译 pkg/main.tie → pkg.exe」的命令（`tie pkg` 无子命令时提示） |

### 3.3 底座能力扩展（M4 补齐阶段：分层原则）

M6 包管理器需要的新能力**按分层补齐**，不全部堆成 Rust 原语：

| 层 | 归处 | 判定标准 | 内容 |
| --- | --- | --- | --- |
| 语言底座原语 | interp eval + C ABI + IR + semantic | 必须碰系统底层（进程/网络/文件系统深度操作/解压） | 网络、进程、解压、目录递归、路径、环境变量 |
| 标准库 | `std/`（tie 写） | 无状态纯逻辑工具，用现有原语可表达 | semver 版本比较、路径字符串规范化、清单文本处理等 |
| 扩展库 | `ext/`（tie 写） | 有状态/应用级，依赖 std 或原语组合 | 注册表客户端封装、包缓存管理等 |

**分层顺序**：先补语言底座原语（Rust）→ 再用 tie 把可表达的工具写进 `std/` →
有状态应用封装进 `ext/`。凡是「改进 std 的进 std，改进 ext 的进 ext」，tie 能写的不进 Rust。

#### 语言底座原语（Rust，M4 补齐阶段一）

| 类别 | 原语 | 用途 |
| --- | --- | --- |
| 网络 | `http_get(url) -> string` | 拉注册表索引/清单 |
| 网络 | `http_get_file(url, path) -> bool` | 下载包文件（tar.gz/zip）到本地 |
| 进程 | `exec_code(cmd) -> i64` | 执行命令取退出码（git clone/push 透传终端） |
| 进程 | `exec_output(cmd) -> string` | 执行命令捕获 stdout（git ls-remote / 版本查询） |
| 解压 | `untar_gz(file, dest) -> bool` | 解压包（tar.gz，注册表默认格式） |
| 解压 | `unzip(file, dest) -> bool` | 解压包（zip，兼容形态） |
| 目录 | `mkdir_all(path) -> bool` | 建多级目录 |
| 目录 | `remove_dir_all(path) -> bool` | 递归删目录 |
| 目录 | `copy_dir(src, dest) -> bool` | 递归复制目录 |
| 目录 | `walk_dir(path) -> table` | 递归列出全部文件路径 |
| 路径 | `path_join(a, b) -> string` | 拼接路径（平台分隔符） |
| 路径 | `path_basename(p) -> string` / `path_dirname(p) -> string` | 路径分解 |
| 路径 | `path_abs(p) -> string` / `path_normalize(p) -> string` | 绝对化/规范化 |
| 路径 | `cwd() -> string` | 当前工作目录 |
| 环境 | `get_env(name) -> string` | 读环境变量（HOME/ssh 等） |
| 环境 | `set_env(name, val) -> void` | 写环境变量（传递子进程） |
| 文件 | `file_copy(src, dest) -> bool` / `file_move(src, dest) -> bool` | 文件复制/移动 |

Rust 侧实现：进程走 `std::process::Command`；网络首版用 Rust std TcpStream 手写
最小 HTTP/1.1 GET（零新依赖，够用；后续可换 reqwest）；解压引入 `flate2` + `zip`
两个 crate；目录/路径用 `std::fs` + `std::path`。原语全部可被 tie 语言调用
（编译路径经 interp 静态库，与 file_read 等同一模式）。

### 3.4 清单格式：tie.pkg（tie:data 原生格式）

tie 语言天然的数据交换格式就是清单格式——**tie:data**，无需发明新语法：

```tie
// tie.pkg
// tie:data
[
    "name": "myproj",
    "version": "0.1.0",
    "description": "示例项目",
    "role": "logic",            // logic / library（复用预处理角色）
    "main": "main.tie",         // 入口文件
    "author": "you@example.com",
    "license": "MIT",
    "dependencies": [           // 运行时依赖：名 -> 版本约束
        "log": "1.0.0",
        "csv": "0.2.0",
    ],
    "devDependencies": [
        "assert": "1.0.0",
    ],
    "sources": [                // 源配置（可按包名覆盖）
        "registry": "https://pkg.tie-lang.org",
        "git": "https://git.franj2.top/tie",
        "ssh": "git@git.franj2.top:tie",
    ],
]
```

锁文件 `tie.lock`（tie:data）：记录**解析后的精确版本 + 来源 + 校验和**，保证可复现构建。

### 3.5 版本约束（首版范围）

- 版本号 `x.y.z`（semver 主.次.修）；
- 约束语法（`tie add pkg@1.0.0` / `pkg@^1.2`）：
  - 精确 `1.0.0`；区间 `^1.2`（>=1.2.0 <2.0.0）；`>=1.0 <2.0`；`*` 最新；
- 版本比较器在 `pkg/version.tie` 用 tie 写（split "." + 逐段数值比较，正则验证格式）。

### 3.6 缓存与目录布局

```text
~/.tie/               全局缓存
├── registry-index/   注册表索引副本（search/info 用）
├── packages/         已下载包文件（name/version/ 校验和复用）
├── git/              git 仓库克隆缓存（按 url hash）
└── tmp/              解压临时区

<project>/.tie/       项目本地
├── deps/             已安装依赖（import 路径指向这里，含 std/ext 语义）
└── bin/              构建产物（tie build 输出）

<project>/tie.pkg     项目清单
<project>/tie.lock    锁文件
```

### 3.7 依赖解析流程（tie 写，pkg/deps.tie）

1. 读 `tie.pkg` → 收集 dependencies + devDependencies；
2. 对每个依赖：查全局缓存 → 命中复用；未命中按源拉取
   （registry 下载解压 / git clone --depth 1 --branch <tag> / path 直接复制）；
3. **递归解析**依赖的依赖（`tie.pkg` 里的 dependencies），构建依赖图；
4. **去重与冲突检测**：同名不同版本 → 取满足所有约束的最高版本；无满足版本 → 报错；
5. 解析结果写入 `tie.lock`（精确版本 + 来源 + 校验和）；
6. `tie install` 幂等：`tie.lock` 存在且未变 → 直接按锁文件恢复。

### 3.8 ssh 支持（用户明确要求）

- git 源支持 https（`https://host/org/repo.git`）**与 ssh**（`git@host:org/repo.git`
  或 `ssh://git@host/org/repo.git`）两种形式；
- tie 只负责把 URL 原样交给 `git clone`（`exec_*` 原语），认证走用户 ssh 密钥
  （`~/.ssh`），tie 不存储/传递任何凭据；
- `tie add pkg@git+ssh://...` 或 sources 里配置 ssh 基址；http_get 类原语不涉 ssh。

### 3.9 注册表（registry，首版形态）

- 注册表 = 一个 HTTP 静态站点，约定目录结构：
  - `index.tie`（或 `index.json`）：全部包名 + 最新版本 + 描述（search/info 用）；
  - `packages/<name>/<version>.tar.gz`：包文件；
  - 包文件 = `tie.pkg` + 全部源码 + 可选 `README`/`LICENSE`（tar.gz，首层目录为包名/版本）；
- `publish` 首版：**git 仓库承载**（打 tag + push 到 git 源）为默认；HTTP 注册表上传留接口；
- 本地 HTTP 服务器可开箱即用（`python -m http.server` 或任意静态托管），
  M6 验收即用本地目录 + 本地 HTTP 服务验证全流程。

## 4. 实施阶段

### 阶段零：M4 补齐——语言能力分层扩展（先行，M4 未收官部分）

> 用户决策：M6 缺的语言能力**归入 M4 补齐**，分层处理——
> 系统底层 → 语言底座原语（Rust）；纯逻辑工具 → `std/`（tie 写）；
> 有状态应用 → `ext/`（tie 写）。tie 能写的不进 Rust。

- **4.0.1 底座原语（Rust）**：interp + llvm + frontend + semantic 新增 §3.3 全部原语
  （网络 http_get/http_get_file、进程 exec_code/exec_output、解压 untar_gz/unzip、
  目录 mkdir_all/remove_dir_all/copy_dir/walk_dir、路径 path_*/cwd、
  环境 get_env/set_env、文件 file_copy/file_move）；
  引入 `flate2` + `zip` 依赖；两路径一致 + 测试；
- **4.0.1b 表下标赋值（语言特性，M4 补齐）**：新增 `Stmt::IndexAssign`（`t[i] = v` /
  `t[i] += v` / 二维 `t[i][j] = v`），四层同步（AST/parser/semantic/interp/IR +
  DynTable 写入桥）。**前置依据**：全部数学算法（docs/plans/algorithm-library.md
  阶段 0）与包管理器依赖表/清单操作都需要原地更新表元素，当前「只读+追加」约束
  使算法退化；此能力是算法库与包管理器的共同底座；
- **4.0.2 std/ 扩展（tie 写）**：semver 版本解析/比较/区间匹配（`std/version.tie`，
  `version.parse` / `version.compare` / `version.satisfies`）、
  路径字符串工具（`str.basename` / `str.dirname` 文本版，若原语已覆盖则省略）、
  清单文本处理（`str.lines` / `str.split_lines` 等）；全部 `pub func` 导出；
  高级数学算法库 `std/exmath.tie` 已新增（霍夫曼编码/解码 + 素数筛/快速幂/斐波那契/
  阶乘/组合数），完整算法路线图见 docs/plans/algorithm-library.md；
- **4.0.3 ext/ 扩展（tie 写）**：注册表客户端封装（`ext/registry.tie`——HTTP 索引拉取、
  包 URL 拼接、缓存路径计算，有状态：本地缓存目录 + 已下载集合）；
  包缓存管理（`ext/cache.tie`——校验/复用/清理）；
- **4.0.4 验收**：原语两路径行为一致（interp 测试 + 编译 demo）；
  std/ext 新库有 demo 示例（examples/）与测试；workspace 全绿。

### 阶段一：pkg 骨架 + 本地 path 源
- `pkg/main.tie` + `pkg/cli.tie`（子命令解析/帮助，纯 tie）；
- `pkg/manifest.tie`（tie:data 清单解析）、`pkg/version.tie`（semver）；
- `tie init` / `tie add path:...` / `tie remove` / `tie install`（本地目录复制）/ `tie build` / `tie run`；
- 用 tie 语言写的示例项目端到端跑通（init → add → install → build → run）。

### 阶段二：git 源（https + ssh）+ 锁文件
- `pkg/fetch.tie` 支持 git clone（含 ssh 地址）、tag 版本；
- `pkg/deps.tie` 依赖图递归解析 + 冲突检测；
- `tie.lock` 生成与幂等恢复；`tie update`；
- git 源端到端验收（本地裸仓库模拟 git 源）。

### 阶段三：HTTP 注册表 + 发布 + 扩展命令
- `http_get`/`http_get_file` 打通 registry 下载与 index 查询；
- `tie search` / `tie info` / `tie publish`（打 tar.gz → git tag push；HTTP 上传接口）；
- `tie clean` / `tie test` / `tie cache`；
- 本地 HTTP 注册表端到端验收（发布 → 另一项目 add/install）。

## 5. 不做（明确排除）

- 包签名与完整性校验的加密级实现（仅记录校验和，SHA-256 后续）；
- 依赖审计/漏洞扫描；
- 多架构产物（包源码为主，构建时本地编译）；
- 全局可执行命令安装（`tie install -g` 留待后续）；
- 私有注册表鉴权（首版假设公开/内网静态注册表）。

## 6. 验收标准

- **自举**：包管理器源码 100% 是 `.tie` 文件（pkg/ 目录），Rust 仅底座原语 + 加载壳；
- **子命令**：`tie init/add/remove/install/update/build/run/test/publish/search/info/clean/cache`
  全部可用；`tie --help` 与 `tie help <cmd>` 显示 tie 写的帮助文本；
- **三源拉取**：path / git（https + ssh）/ HTTP registry 均端到端跑通；
- **锁文件**：tie.lock 生成正确、幂等恢复、可复现构建；
- **依赖解析**：传递依赖/去重/冲突检测正确（有测试用例）；
- **示例生态**：用 tie 写的库发布到注册表，另一项目 add/install 后正常 import 使用；
- workspace 编译零错误、测试全绿。

## 7. 影响范围

| 组件 | 影响 |
| --- | --- |
| crates/tie-interp | 新增 §3.3 原语（eval 分支 + C ABI 导出） |
| crates/tie-llvm | 新原语 IR declare/调用生成 |
| crates/tie-frontend | 新原语 semantic 签名校验 |
| crates/tie-pkg（新） | 子命令识别 + pkg.exe 查找/执行 + 构建自举 |
| crates/tie | 入口增加子命令分派（识别包管理命令 → tie-pkg） |
| pkg/（新，tie 源码） | 包管理器全部逻辑（main/cli/manifest/version/deps/fetch/install/build/run/publish/search） |
| Cargo.toml | workspace 增 crates/tie-pkg；根依赖增 flate2/zip |
| std/（可选） | 若 tie.pkg 解析抽象为通用 tie:data 解析函数，可入 std |
| docs/ | README CLI/路线图、docs/plans/package-manager.md（本文件）、docs/ai-guide、docs/prompt-pack、CHANGELOG |
| scripts/package.ps1 | 发行版收录 pkg.exe 与 std/ext |

## 8. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 手写 HTTP 客户端不稳 | 首版仅 GET 静态文件（无重定向复杂化）；按需演进 reqwest |
| tie 语言能力不足（如无正则细节差异） | 已有 regex 全族原语；字符串工具 std/str 已就绪 |
| 单文件 pkg 逻辑过重 | 多文件 import + 命名空间组织（M2.1.7 真模块边界已就绪） |
| Windows/Linux 路径差异 | path_* 原语封装平台分隔符；测试覆盖两平台可运行路径 |
| 与现有 `tie <file>` 语义冲突 | 子命令识别优先级：首个参数是已知子命令名且非 .tie 文件 → 包管理器 |
