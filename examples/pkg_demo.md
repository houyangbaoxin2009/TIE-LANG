# tie 包管理器演示（M6 E1/E2 + E3/E4）

本目录（`examples/demo_pkg/`）是 `tie init` 生成的示例项目，演示包管理器的
完整能力：**init → add → install → build/run**，以及 E3/E4 的 **git/registry 源、
tie.lock 锁文件、发布/搜索**。

包管理器 CLI 全部由 tie 语言自写（`pkg/` 目录），Rust 侧（`crates/tie`）只做
「子命令识别 + exec pkg.exe」转发。

## 前置：构建包管理器（自举）

```bash
cargo build --release -p tie-interp          # 产出 tie_interp.lib（含 http_get_file 修复）
target/release/tie-llvm.exe pkg/main.tie -o pkg/pkg.exe   # 编译 pkg.exe
```

之后 `tie init|add|remove|install|update|build|run|publish|search|info|help`
即可用（子命令识别自动转发）。

## 端到端演示（path 源）

```bash
# 1. 初始化项目（生成 tie.pkg + main.tie 模板）
tie init demo_pkg
cd demo_pkg

# 2. 添加依赖
#    path 源：本地目录，install 时复制到 .tie/deps/<包名>/
tie add path:../lib_colors
#    registry 源：版本约束（精确 / 区间 / 最新）
tie add log@1.0.0
tie add csv@^0.2
#    git 源：https/ssh 仓库（#tag 后缀指定版本）
tie add demo@git+https://host/repo.git#v1.0.0

# 3. 安装依赖（解析 + 拉取 + 生成/校验 tie.lock）
tie install
# 输出:
#   [pkg] 安装依赖到 .tie/deps/ ...
#   [pkg]  已安装 lib_colors ← ../lib_colors
#   [pkg]  下载 log@1.0.0 ← http://<registry>/packages/log/1.0.0.tar.gz
#   [pkg] 已生成 tie.lock
#   [pkg] 全部依赖安装完成
# 再次 install：tie.lock 有效 → 按锁文件幂等恢复（缓存命中不重新拉取）

# 4. 更新依赖（重新解析 + 更新 tie.lock）
tie update

# 5. 构建 / 运行
tie build      # 调用 tie 编译器编译 main.tie
tie run        # 编译并执行，输出 Hello, tie!

# 6. 帮助 / 移除
tie help
tie remove lib_colors
```

## 三源端到端验收（E3/E4）

### registry 源（本地 HTTP 注册表）

```bash
# 注册表 = 任意 HTTP 静态目录，约定结构：
#   index.tie                        行格式：包名|版本|描述
#   packages/<name>/<version>.tar.gz 包文件（首层为项目文件）
python -m http.server 8124 --directory <registry-root> &
export TIE_REGISTRY=http://127.0.0.1:8124   # Windows: set TIE_REGISTRY=...

tie init demo2
cd demo2
tie add demo2lib@1.0.0        # 精确版本
tie install                    # → .tie/deps/demo2lib/ 生成 + tie.lock
tie add demo2lib@^1.0          # 区间约束：index 筛最高满足（如 1.1.0）
tie install
```

### git 源（本地裸仓库模拟）

```bash
git init --bare /tmp/gitdemo.git    # 模拟远程仓库（打 tag v1.0.0）
tie init demo4 && cd demo4
tie add gitdemo@git+file:///tmp/gitdemo.git#v1.0.0
tie install                          # git clone --depth 1 --branch v1.0.0
# → .tie/deps/gitdemo/（不含 .git 元数据）；锁恢复时缓存命中不再克隆
```

### tie.lock 幂等

```text
// tie.lock（tie:data 文本；记录解析后的精确版本与来源）
[
    "name": "demo2",
    "version": "0.1.0",
    "deps": [
        "demo2lib": {
            "version": "1.0.0",
            "source": "http://127.0.0.1:8124/packages/demo2lib/1.0.0.tar.gz",
            "spec": "1.0.0",
        },
    ],
]
```

install 时若 tie.lock 存在且与当前 tie.pkg 直接依赖一致 → 直接按锁文件恢复
（缓存 `.tie/cache/` 命中不重复拉取）；否则重新解析并覆盖锁文件。`tie update`
强制重新解析。

### 发布与搜索（E4）

```bash
# 发布：校验 tie.pkg → 打包 .tie/dist/<name>-<version>.tar.gz → git tag/push
tie publish

# 搜索 / 信息（查询 TIE_REGISTRY 指向的 index.tie）
tie search demo        # 包名包含 demo 的全部版本
tie info demo2lib      # 该包的最高版本与描述
```

## 生成的项目结构

```text
demo_pkg/
├── tie.pkg                项目清单（tie:data 格式；依赖声明在这里）
├── tie.lock               锁文件（install 生成；记录解析版本/来源，幂等恢复）
├── main.tie               入口源码模板
├── main.exe               tie run 的编译产物
└── .tie/
    ├── deps/              已安装依赖（import 路径可指向这里）
    │   └── lib_colors/    path 源复制结果（lib_colors.tie + README.md）
    └── cache/             拉取缓存（git 克隆 / registry 解压），锁恢复复用
```

## tie.pkg 清单格式

```tie
// tie.pkg
// tie:data
[
    "name": "demo_pkg",
    "version": "0.1.0",
    "description": "tie 项目",
    "role": "logic",
    "main": "main.tie",
    "author": "",
    "license": "MIT",
    "dependencies": [
        "lib_colors": "path:../lib_colors",              // path 源：本地目录
        "log": "1.0.0",                                  // registry：精确版本
        "csv": "^0.2",                                   // registry：区间约束
        "demo": "git+https://host/repo.git#v1.0.0",      // git 源（#tag 指定版本）
    ],
]
```

依赖项：键 = 包名，值 = spec。`path:<路径>` 本地目录；`git+<url>[#tag]` /
`git@host:repo.git` / 裸 https 含 `.git` 为 git 源；`x.y.z` / `^x.y` / `*`
为 registry 版本约束（TIE_REGISTRY 可覆盖默认基址 `https://pkg.tie-lang.org`）。

## 实现说明

- **模块分工**（全部 tie 语言自写）：
  - `pkg/main.tie` —— CLI 入口：子命令分派（init/add/remove/install/update/
    build/run/publish/search/info/help）+ install 锁文件流程；
  - `pkg/manifest.tie` —— tie.pkg 清单解析（字段提取、依赖增删、逗号分隔序列传参）；
  - `pkg/deps.tie` —— 三源安装 + 递归依赖解析（resolve：BFS + 去重 + 冲突检测，
    深度限制 3）+ 锁文件落地（install_from_lock）；
  - `pkg/fetch.tie` —— git 源识别/浅克隆 + registry 基址/URL/版本选择/下载解压；
  - `pkg/lock.tie` —— tie.lock 生成/解析/校验；
  - `pkg/publish.tie` / `pkg/search.tie` —— 打包发布 / 注册表搜索与信息查询；
- **Rust 侧**：`crates/tie/src/main.rs` 识别 11 个子命令（且非 `.tie` 文件）→
  查找并 exec `pkg.exe`（查找顺序：`TIE_PKG_EXE` → tie.exe 同目录 → 当前目录 →
  `pkg/` 目录 → tie.exe 向上回溯 workspace）。
- **tie 约束**：跨模块传参用逗号分隔字符串序列（表参数元素类型静态未知）；
  `&&` 非短路（嵌套 if）；无 break/continue/递归（迭代 + 队头指针）；
  **import 展开陷阱**：同一命名空间被多模块 import 会重复内联报错，且被别名导入
  的文件其**自身命名空间必须写在 import 语句之前**（collect_ns_paths 按语句顺序
  收集，别名映射取首个命中；文件末尾的 import 需显式 `;`）；
  **编译路径 file_exists 对目录恒 false**（fopen 打不开目录）——目录存在性用
  目录内文件探测（如 `.tie/cache/git/<name>/tie.pkg`），删除目录一律无条件
  `remove_dir_all`。
