# NEW — 发行版新鲜事

> 这里记录 tie 语言**当前发行版**的新功能与特色（面向读者：想快速知道"这个版本
> 有什么新东西"的人）。完整变更流水账见 [CHANGELOG.md](CHANGELOG.md)，
> 工程全貌与用法见 [README.md](README.md)。

**内部代号**：Harbor 港湾（2026.1 正式版代号，首个正式版 = 工具链第一次靠岸停泊）
**本版**：Harbor-2026.1-preview.3
**对比基线**：Harbor-2026.1-preview.2

> 本版相对 preview.2 的增量 = 阶段 1 语言地基（S1.x）→ 阶段 3 宏整合（S3.3）
> 共 25 个提交：语言系统能力从「可运行」全面补齐到「可系统编程」。

---

## 一句话

tie 是一门**通用编程语言**：用一门语言写逻辑、写界面、写数据库、当数据交换格式。
这个发行版最重要的一件事：**编译器已经完全用 tie 自己写出来了（0-Rust 完全自举）**，
并且一口气补齐了现代语言的整套系统能力——宏、闭包、接口、错误处理、枚举、泛型、
unsafe、窄整数、包管理，全部到位。

## 亮点速览

| 亮点                    | 一句话                                            | 来源           |
| --------------------- | ---------------------------------------------- | ------------ |
| 🔥 **0-Rust 完全自举**    | 编译器 `tiec` 纯 tie 自写，自举链 IR 逐字节一致，Rust 工具链已全部归档 | preview.2 延续 |
| 🧩 **宏/元编程（S3.3）**    | 编译期 AST→AST 函数：准引用 + 插值 + gensym 词法卫生（本版最新）    | **本版全新**     |
| 🧊 **闭包/一等函数值（S2.2）** | `func` 字面量 + `fn(A)->R` 类型 + 高阶函数 + 间接调用       | **本版全新**     |
| 🧭 **接口 port（S2.4）**  | `port` 声明 + `impl` 显式实现 + 静态/动态双形态分发           | **本版全新**     |
| 🛟 **错误处理（S2.3）**     | `Result`/`Option` + `?` 解包 + `panic`           | **本版全新**     |
| 📦 **库/包模型（S3.2）**    | tieir 二进制序列化 + 多文件包 + MVS + 签名校验               | **本版全新**     |
| ⚙️ **构建配置（S3.1）**     | `config.data.tie` 分层合并 + profile + backend 选择  | **本版全新**     |
| 🧱 **语言地基（S1）**       | unsafe / 窄整数 / 角色扩展 / 移动语义 四大系统能力              | **本版全新**     |
| 🎯 **enum 枚举 + 用户泛型** | Rust 风格 ADT + 编译期单态化                           | preview.2 延续 |
| 📀 **vendored LLVM**  | 发行版内置精简 LLVM，解压即用免安装（本版随包升 LLVM 22.1.8）        | preview.1 延续 |

---

## 语言特性（本版新增）

### 宏 / 元编程（S3.3，最新）

编译期 AST→AST 函数，表达式宏全链路验证：

```tie
// 宏定义：参数与返回值都是 code（代码片段）
macro double(x: code) -> code {
    return `( $(x) * 2 )        // 准引用 + 插值
}

func main() {
    print(double(3 + 4))        // 编译期展开为 (3+4)*2 → 输出 14
}
```

- `macro name(x: code) -> code { 体 }` 定义；准引用字面量 `` `(expr) ``（表达式
  形式，立即解析）与 `` `{ stmts } ``（块形式，延迟解析回填）；
- 插值 `$x` / `$(expr)`；`gensym("前缀")` 内置实现**词法卫生**（H2 改名 +
  H3 唯一符号，展开不与用户代码冲突）；
- 编译期由 interp 执行（函数式宏展开 pass `mexpand`，轮次上限 64 防死循环）；
- 已声明遗留：语句级宏 / 跨文件宏 / 过程宏（后续里程碑）。

### 一等函数值 / 闭包（S2.2）

```tie
var add10 = func(x: i64) -> i64 { return x + 10 }   // func 字面量（闭包）
var f: fn(i64) -> i64 = add10                        // fn 类型作变量
print(apply(f, 32))                                  // 高阶函数 → 42
```

- `func` 字面量 = 闭包：捕获变量 **move 进环境**（`{env, entry}` 闭包值）；
- `fn(A) -> R` 函数类型：作参数 / 返回 / 变量；命名函数提升（函数名直接作函数值）；
- 后端 `call_indirect`(70) 间接调用 + 多闭包返回槽位修复，探针 1-5 全过。

### 接口 port（S2.4）

tie 的 interface，**静态 + 动态双形态分发**：

```tie
port Drawable { pub func draw(self, ctx: ptr) -> i64 }   // 方法签名集合
impl Drawable for Button { ... }                          // 显式实现

func render_all<T: Drawable>(xs) { ... }   // 泛型约束：静态分发，单态化零开销
var d: Drawable = button                   // 提升（须 unsafe）：动态分发
```

- `impl` 漏方法 = 编译错误（「impl 'P for S' 缺少方法 'M'」）；
- 动态分发：编译器隐式生成 **vtable 全局常量** + 间接调用，支持
  `table<Drawable>` 异构容器；
- 提升（struct → port）归 unsafe（借用语义，安全边界显式化）。

### 错误处理（S2.3）

```tie
import std/result                       // 预置 enum Result<T,E> / Option<T>
func div(a: i64, b: i64) -> Result<i64, string> {
    if b == 0 { return Err("除零") }
    return Ok(a / b)
}
func main() {
    var v = div(10, 2) ?                // ? 解包：Err 提前返回，Ok 解出 payload
}
```

- `?` 解包后缀：Err/None 提前 return、Ok/Some 解包 payload（仅限返回
  Result/Option 的函数内）；`panic("msg")` 致命错误（printf + exit(1)）。

### enum 枚举（preview.2）

Rust 风格 ADT，tiec 全链路实现：

```tie
enum Shape { Circle(i64) Rect(i64, i64) }   // 带 payload 变体
enum Option<T> { Some(T) None }             // 泛型变体
```

- 构造：`Color.Red`（常量）/ `Shape.Circle(5)`；匹配：switch 对 tag 整数比较链；
- LLVM 布局：静态结构体 `{ i64 tag, i64×K 槽 }`——零虚表、零堆分配；
- 一期 payload 白名单：整数族 / bool / char / trit。

### 用户泛型（早前，本版持续增强）

```tie
func max<T>(a: T, b: T) -> T { return if a > b then a else b }
struct Box<T> { var value: T }
```

编译期**单态化** + 调用点类型推断，与 enum 泛型变体 / 泛型 port 约束协同。

### 字符串模型（S2.1）：{ptr,len} 二进制安全

- 字符串内部表示升级为 **{ptr, len} 长度头布局**（8 字节长度头 + UTF-8 数据 +
  边界自动 NUL），FFI 传方向**零拷贝**，旧 API 行为不变；
- `len(s)` 读长度头 O(1)（不再 strlen 扫描）；新原语 `str_byte(s,i)`（二进制
  安全取字节，可携带 `\0`）、`utf8_seq_len`/`utf8_char_at`（码点迭代）；
- **StringBuilder**：`string_builder()` + `sb_append`/`sb_append_byte` +
  `sb_build`，原地追加 + 容量倍增（高频字符串拼接用）；
- tie 桥返回字符串自动补头，所有字符串值统一"有头"不变量。

### 语言地基（S1，阶段 1 三件套 + 移动语义）

- **unsafe 模型（S1.2）**：`unsafe fn` / `unsafe { }` 块；`ptr<T>` / `slice<T>` /
  `atomic<T>`（load/store/fetch_*/compare_exchange，内存序 Relaxed..SeqCst）；
  `repr(C)` struct；extern 强制 unsafe；`asm!` 内联汇编（`{N}` 占位符自动转 LLVM
  `$N`）；`alloc`/`free`/`memcpy`/`memset`；
- **窄整数（S1.3）**：`i8`/`i16`/`i32`/`u8`/`u16`/`u32`/`u64`/`f32`——后缀字面量
  `42i32`、`as_*` 转换族、`checked_*` 溢出检测族（返回 (值, 溢出标志) 二元组）、
  明确移位语义（无符号 lshr / 有符号 ashr，移过量≥位宽有定义）；
- **角色扩展（S1.4）**：多角色叠加 `type tie<db:vector, unsafe>`、角色参数化
  （`ui:window/web/embedded`、`db:schema/seed/vector`、`data:config/asset`）、
  文件名与头部角色不一致 = 编译错误；
- **移动语义（S1.5）**：`smove` 独立 pass + `TIE_MOVE_CHECK=1`，自举
  compiler/ 全链零移动错误。

---

## 编译器与工具链

### 完全自举（0-Rust）

- `tiec`（`compiler/`，tie 语言自写）是**主编译器**：前端（lexer/parser/semantic）
  + 中端（tie-IR 列式表）+ 后端（irgen + llvmgen + toolchain）全部 tie 实现；
- 自举链稳定：tiec 编译 tiec，二阶产物 **--emit-ir 逐字节一致**（SHA256 同）；
- Rust 工具链（crates/，-31915 行）已全部归档至独立仓库
  [tiec_rust](https://github.com/tie-lang/tiec_rust)，主仓库 **0-Rust**；
- REPL / 包管理器同样用 tie 语言自写（`repl/repl.tie`、`pkg/`）。

### 构建配置系统（S3.1）

- 统一 `config.data.tie` 配置文件（type tie<data> 角色），分节配置
  tiec/prep/pkg 等子工具；
- **L2 三层分层合并**：CLI > 项目 config > 用户 `~/.config/tie/config.data.tie` >
  内置默认；P3 **profile**（dev/release，Cargo 风格）；
- **D1-D7 全 7 域**（target/backend/opt/features/roles/link/modules）+ 
  `--backend` 后端实现选择（win32/LLVM 工具链）。

### 库 / 包模型（S3.2）

- **tieir 二进制序列化**：IR 分发单元（魔数 TIEIR + 7 段定宽编码，内容哈希
  FNV-1a 防篡改）；CLI：`--tieir-out <f>` / `--dump-irt <f>`；
- **多文件包 L1c**：`tie pack` / `tie verify`——包 = 目录 + tie.pkg，入口声明
  导出面，包内模块私有；
- **MVS 最小版本选择**（P2c）：约束冲突取最低满足版本，可复现、幂等；
- **签名校验（P5c）**：pack 生成 signature 内容哈希，install/verify 自动校验。

### 包管理器（Harbor M6，tie 语言自写）

`tie init / add / remove / install / update / build / run / publish / search / info`

- **三源依赖**：path（本地）/ git（`git+https://...`）/ registry（
  `log@^1.2` 约束，`TIE_REGISTRY` 指定基址）；
- `tie.lock` 锁文件：install 幂等恢复；`tie publish` 打包 tar.gz + git tag + push。

### vendored LLVM（preview.1 起，本版持续）

- 发行版 zip 内置精简 LLVM 工具链（`bin/llvm/`：clang / opt / llvm-ar /
  lld-link + clang 头文件 + 许可文本），**解压即用免安装**；
- `TIE_LLVM_HOME` 环境变量指向即可开箱即用；随包 lld 让无 MSVC/VS 机器也能链接。

### REPL / LSP

- 无参数 `tiec` → 进入 REPL（`repl/repl.tie` 自举产物）；`tie --lsp` 语言服务器
  模式（LSP over stdio，供编辑器接入）；
- 编辑器扩展 `editor/vscode-tie`（语法高亮 + LSP 诊断）随发行版分发。

---

## 标准库与生态

- **std/fs 完整文件系统库**：读取/写入/元数据/目录遍历/复制移动/归档（untar_gz/
  unzip），Rust std::fs 风格 API；底层 7 个 **UTF-8 安全桥**（Windows 宽字符
  API），**中文/Unicode 路径问题根治**；
- **rdu 嵌入式基础层**：第三层内置库（独立于 std/ext），**无堆 / 无 OS / 无
  libc** 的 freestanding 可用——零原语调用、零动态内存、无递归、无全局可变
  状态，6 模块（bits/math/ascii/crc/fixed/rnd）；
- **标准库全景**：文本编码（string/utf/ascii/encoding/regex/json）、数据结构与
  算法（sort/collection/crypto/graph/linalg/math/radix）、IO 系统
  （fs/path/http/net/time/process/args）+ tieDB / 扩展库 ext / 示例库 lib_colors；
- **trit 平衡三进制**（独门特色）：三值逻辑（true/unknown/false）保持 Kleene
  语义，多进制字面量 `0x`/`0b`/`0o`/`0t`，数论与逻辑计算常用。

---

## 快速上手

```bash
# 编译并运行示例（tiec 自举编译器）
compiler\tiec.exe examples\hello.tie
examples\hello.exe

# 无参数 → 进入 REPL
compiler\tiec.exe repl\repl.tie

# 包管理器
tie init myapp && tie add log@^1.2 && tie run
```

## 获取发行版

发行版 zip 内置于 GitHub Release（tag：Harbor-2026.1-preview.*），解压即用：
`bin/`（tiec/repl/pkg + 内置 LLVM 工具链）+ `std/` + `ext/` + `rdu/` +
文档 + VSCode 扩展。

## 下一步（路线图摘要）

- 宏扩展：语句级宏 / 跨文件宏 / 过程宏（M4）；
- 动态库编译（M5）、LSP 重写（tsp）、运行时套件（trm）、硬件加速（hw-accel）；
- 正式发行版 2026.1「Harbor」——工具链合集打包、双平台 Release。
