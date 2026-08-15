# 规划：LLVM 工具链升级（18.1.8 → 22.1.8）

> 状态：**已实现**（2026-08-15，S1.1 完成；commit 见 git log）
> 本文档定义 tie 的 LLVM 工具链升级计划：**18.1.8 → 22.1.8**。
> 结论：**升级到 22.1.8（最新稳定版），成本低收益明确；23 等稳定后再议。**
> 关联：toolchain.tie（opt/clang/llvm-ar/lld 驱动）、vendored LLVM（bin/llvm/）、
> wasm 后端规划（webui）、hw-accel.md（编译器性能）。

## 1. 版本现状

| 版本 | 发布 | 状态 |
| --- | --- | --- |
| **18.1.8** | tie 当前使用 | 基线 |
| 19.1.0 | 2024-09 | 过时 |
| 20.1.0 | 2025-03 | 过时 |
| 21.1.0 | 2025-08 | 过时 |
| **22.1.8** | 2026-06 | **最新稳定版（GitHub Latest）** |
| 23.1.0 | RC3（2026-08-12） | 未稳定，勿生产用 |

## 2. tie 的 LLVM 依赖面（升级成本评估的关键）

**依赖面极薄**——tie 只通过 6 个工具函数消费 LLVM，**全走命令行文本接口**：

| 工具函数 | 调用 | 说明 |
| --- | --- | --- |
| find_tool | TIE_LLVM_HOME/PATH/固定目录 | 工具发现 |
| opt | `opt -O{0..3} -S in.ll -o out.opt.ll` | 中间优化（文本 IR 进出） |
| clang | `clang in.opt.ll -o out.exe [--target]` | 链接可执行文件 |
| llvm-ar | `llvm-ar rcs out.a out.o` | 静态库归档 |
| link_exe / compile_object / archive | 组合上述 | 链接/编译/归档 |

**关键事实**：不依赖 LLVM C++ API/库——只有命令行文本接口。
这是升级风险最低的形态：命令行接口稳定，升级 = IR 语法适配 + 回归测试。

## 3. 各版本核心变化（对 tie 的影响面）

### 3.1 IR/语法破坏性变更（tie 生成文本 IR，需关注）

| 版本 | 变更 | tie 影响 |
| --- | --- | --- |
| 19 | 常量表达式 icmp/fcmp/shl 移除；debug intrinsics→records；intrinsic 改名 | 低（tie 不生成这些） |
| 20 | 递归类型禁止；x86_mmx 移除；NVVM intrinsics 移除 | 低（tie 不用） |
| 21 | 常量表达式 mul 移除；nocapture→captures(none)；inline asm label 参数移除 | 低 |
| **22** | **SwitchInst case 值不再作 operand**；ptrtoaddr；masked intrinsic 对齐参数变更 | **需验证**（见 §5） |
| 23(预览) | convert intrinsics 移除、denormal attrs 替换、BranchInst 拆分、wasm ref 类型重做 | 大——等稳定 |

### 3.2 opt pass（tie 零影响）

- 无 `-O0..-O3` 改名；新增 IRNormalizer(20)/AllocToken(22) 等
- tie 只用 `opt -O{0..3} -S`，零自定义 pass——**不受影响**

### 3.3 clang 默认行为

| 版本 | 变更 | tie 影响 |
| --- | --- | --- |
| 19 | triple 归一化；GCC_INSTALL_PREFIX 报错 | 低（tie 主要 Windows msvc） |
| 20 | pointer-TBAA/pointer overflow 默认 | 低（影响优化语义，需回归） |
| 22 | MSVC ABI 变更 | **Windows 相关，需回归验证** |

### 3.4 lld

| 版本 | 变更 | tie 影响 |
| --- | --- | --- |
| 21 | 可读可执行段默认合并 | 低 |
| **22** | **Wasm --stack-first 默认；wasm32-wasi→wasm32-wasip1** | wasm 目标启用时需适配 |

### 3.5 wasm（tie 未来 webui 后端）

- 19/21：无变化；20：bulk-memory/nontrapping/Lime1/EH 支持
- **22**：half soft-float；wasi 改名——wasm 支持成熟
- 23(预览)：wasm ref 类型表示法大改（target ext）——**升级 22 正好避开**

## 4. 升级决策

### 4.1 升级到 22.1.8（推荐）

1. **依赖面薄**：命令行接口 + 文本 IR，升级风险最低
2. **稳定**：22.1.8 发布 2 个月 + 8 个补丁；23 还在 RC3
3. **wasm 红利**：22 的 wasm 支持成熟；23 的 ref 类型大改会破坏——现在上车 22 正好
4. **性能与安全**：4 个版本的优化器改进 + 安全补丁

### 4.2 不升级 23（等稳定）

- 23 的 IR 破坏性变更明显更多（BranchInst 拆分、wasm ref 类型重做、convert intrinsics 移除）
- 等 23.1 稳定 + tie 升级 22 验证后再评估

## 5. 升级步骤

### 5.1 前置验证（回归重点）

- [ ] 跑 compiler/tests（词法/语法/语义回归）
- [ ] 跑行为等价回归（_driver_test，与 Rust 参考对比）
- [ ] **重点：IR 语法错误**（opt/clang 解析 22 的文本 IR）
- [ ] **SwitchInst 验证**：tie 当前用线性比较链生成 switch（非 LLVM switch 指令），
      22 的 SwitchInst operand 变更大概率无影响——实测确认
- [ ] clang 22 MSVC ABI 回归（Windows 链接行为）
- [ ] opt -O2 优化结果对比（数值/输出一致性）

### 5.2 升级操作

1. 替换 vendored LLVM 二进制（bin/llvm/ → 22.1.8）
2. 若 TIE_LLVM_HOME 指向外部安装：更新到 22.1.8
3. 回归测试全绿后提交
4. 更新 README 中的 LLVM 版本说明

### 5.2a 实现记录（2026-08-15，S1.1 完成）

- **二进制**：官方 GitHub releases `LLVM-22.1.8-win64.exe`（安装器需管理员）/
  `clang+llvm-22.1.8-x86_64-pc-windows-msvc.tar.xz`（归档包，解压即用，本机采用）
- **本机切换**：D:\LLVM 升级为 22.1.8；18.1.8 备份至 D:\LLVM18（便于回退）；
  PATH/TIE_LLVM_HOME 均无需改（路径不变）
- **关键适配**：clang 22 起 Windows 默认链接器从 link.exe 改为 lld-link（实测 -v 确认），
  lld-link 解析 Rust staticlib（tie_interp.lib）CRT 符号缺陷（printf undefined）导致
  interp 桥程序链接失败 → `compiler/backend/toolchain.tie` `link_exe` 非 vendored 场景
  显式 `-fuse-ld=link`；vendored（TIE_LLVM_HOME）场景保持 `-fuse-ld=lld`
- **回归结果**：interp 11/11 + _driver_test PASS + tests/language 24 PASS（零新增失败）+
  自举闭环 tiec2==tiec3 sha 一致 + G4 闸门 PASS（ratio 1.458）+ vendored hello/库编译链正常
- **SwitchInst**：tie switch 走 icmp 比较链（非 LLVM switch 指令）——22 变更零影响（实测确认）

### 5.3 wasm 目标适配（webui 启用时）

- 三元组：`wasm32-wasi` → **`wasm32-wasip1`**（22 改名）
- lld：关注 `--stack-first` 默认值变化
- 与 hw-accel/包模型（wasm 分发）衔接

## 6. 决策记录

| 决策点 | 结论 | 理由 |
| --- | --- | --- |
| 是否升级 | **升级到 22.1.8** | 依赖面薄 + 稳定 + wasm 红利 |
| 升级目标 | 22.1.8（最新稳定） | 23 未稳定（RC3） |
| 23 评估 | 等 23.1 稳定后再议 | IR 破坏性变更多 |
| 回归重点 | IR 语法 / SwitchInst / MSVC ABI | 22 的主要破坏点 |

## 7. 未决问题

1. **vendored 分发**：22.1.8 二进制从哪里获取（LLVM 官方预编译 vs 自编译——
   tie 发行版随包分发的形态，参考 tie-llvm-vendored-dist 先例）
2. **升级时机**：与里程碑的关系（M0 unsafe 扩展前还是后——建议独立小里程碑
   先做，隔离风险）
3. **行为等价基准**：升级后 G4 基准（ratio 1.09）是否重跑验证性能
