# S3.2 探针（tests/s32_probe/）

S3.2 库/包模型验收探针。运行前置：`compiler/tiec.exe` 须为 S3.2 自举版
（含 `--tieir-out` / `--dump-irt`），`pkg/pkg.exe` 为 S3.2 编译版（含
`pack` / `verify` 子命令）。

## probe1_tieir_roundtrip.tie

tieir 序列化 roundtrip：构造 IR（函数/块/指令/span/符号/导出/模块头/依赖）→
`tieir.serialize()` 二进制 → `tieir.deserialize()` 重建 → 逐项比对结构一致；
文件写读往返；内容哈希（FNV-1a 32）可复现；魔数/版本损坏拒绝。

```bash
compiler/tiec.exe tests/s32_probe/probe1_tieir_roundtrip.tie -o <tmp>/p1.exe
<tmp>/p1.exe        # → 23 项 OK，exit 0
```

## probe_pkg_lib / probe_pkg_app（多文件包 L1c 全链路）

- `probe_pkg_lib/`：多文件包（L1c）——`tie.pkg`（main=src/main.tie，
  exports 声明导出面）+ `src/main.tie`（入口，对外 API `s32math.add/calc`）
  + `src/calc.tie`（包内私有模块，经入口间接可达）
- `probe_pkg_app/`：消费方——依赖 `s32_math`（path 源），import 包入口
  编译链接运行

全链路（pack → install → verify → 消费编译运行）：

```bash
# 1) 打包分发单元（tieir + signature + tar.gz；P5c 签名者走 TIE_SIGNER）
cd tests/s32_probe/probe_pkg_lib
TIE_SIGNER=s32-package pkg/pack.exe 或 tie pack
#   → s32_math-1.0.0.tieir + signature + .tie/dist/s32_math-1.0.0.tar.gz

# 2) 消费方安装（path 源 → .tie/deps/s32_math/；install 自动签名校验 P5c）
cd ../probe_pkg_app
tie install     # → tie.lock + 安装 + 「校验通过: s32_math（哈希匹配）」
tie verify s32_math   # 显式校验

# 3) 消费方编译链接运行（import 包入口）
compiler/tiec.exe main.tie -o main.exe
./main.exe      # → add(2,3)=5 / calc(5)=11
```
