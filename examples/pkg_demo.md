# tie 包管理器演示（M6 骨架 E1/E2）

本目录（`examples/demo_pkg/`）是 `tie init` 生成的示例项目，演示包管理器的
最小可行闭环：**init → add → install → build/run**。

包管理器 CLI 全部由 tie 语言自写（`pkg/` 目录），Rust 侧（`crates/tie`）只做
「子命令识别 + exec pkg.exe」转发。

## 前置：构建包管理器（自举）

```bash
cargo build --release -p tie-interp          # 产出 tie_interp.lib
target/release/tie-llvm.exe pkg/main.tie -o pkg/pkg.exe   # 编译 pkg.exe
```

之后 `tie init|add|remove|install|build|run|help` 即可用（子命令识别自动转发）。

## 端到端演示

```bash
# 1. 初始化项目（生成 tie.pkg + main.tie 模板）
tie init demo_pkg
cd demo_pkg

# 2. 添加依赖
#    path 源：本地目录，install 时复制到 .tie/deps/<包名>/
tie add path:../lib_colors
#    版本约束（registry/git 源后续阶段，install 时提示跳过）
tie add log@1.0.0

# 3. 安装依赖
tie install
# 输出:
#   [pkg] 安装依赖到 .tie/deps/ ...
#   [pkg]  已安装 lib_colors ← ../lib_colors
#   [pkg]  跳过 log: 源 "1.0.0" 暂不支持（仅 path 源）
#   [pkg] 全部依赖安装完成

# 4. 构建 / 运行
tie build      # 调用 tie 编译器编译 main.tie
tie run        # 编译并执行，输出 Hello, tie!

# 5. 帮助 / 移除
tie help
tie remove lib_colors
```

## 生成的项目结构

```text
demo_pkg/
├── tie.pkg                项目清单（tie:data 格式；依赖声明在这里）
├── main.tie               入口源码模板
├── main.exe               tie run 的编译产物
└── .tie/
    └── deps/              已安装依赖（import 路径可指向这里）
        └── lib_colors/     path 源复制结果（lib_colors.tie + README.md）
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
        "lib_colors": "path:../lib_colors",   // path 源：本地目录
        "log": "1.0.0",                       // 版本约束：registry 源（占位）
    ],
]
```

依赖项：键 = 包名，值 = spec。`path:<路径>` 为本地目录源（install 复制目录）；
`x.y.z` 为版本约束（registry/git 源为后续阶段）。

## 实现说明

- **模块分工**（全部 tie 语言自写）：
  - `pkg/main.tie` —— CLI 入口：`arg_string(i)` 解析子命令，分派
    init/add/remove/install/build/run/help；
  - `pkg/manifest.tie` —— tie.pkg 清单解析（字段提取、依赖增删、逗号分隔
    序列传参）；
  - `pkg/deps.tie` —— path 源安装（`copy_dir` 递归复制到 `.tie/deps/`）；
  - `std/version.tie` —— 复用 semver 版本比较（add 时校验版本格式）。
- **Rust 侧**：`crates/tie/src/main.rs` 识别首个参数为子命令（且非 `.tie`
  文件）→ 查找并 exec `pkg.exe`（查找顺序：`TIE_PKG_EXE` → tie.exe 同目录
  → 当前目录 → `pkg/` 目录 → tie.exe 向上回溯 workspace）。
- **tie 约束**：跨模块传参用逗号分隔字符串序列（表参数元素类型静态未知）；
  `&&` 非短路（嵌套 if）；无 break/continue（迭代 + return）。
