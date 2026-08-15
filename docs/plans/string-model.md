# 规划：tie 字符串模型（学习 Rust：UTF-8 + {ptr,len} + 迭代器引导）

> 状态：**规划**（2026-08-15 设计讨论定稿，未实现）
> 本文档定义 tie 的字符串模型。决策汇总：
> **R1**（UTF-8 表示，现状保持）+ **O2**（移动语义 `{ptr, len}` 结构，二进制安全）
> + **M1 不可变 + M2 StringBuilder** + **V2 拷贝切片 + SSO** + **F1 边界自动 NUL**
> + **B1 bytes 独立** + **L1+L2 字面量静态 + intern 可选**。
> **码点访问：学习 Rust**——不提供 O(1) 随机码点访问，用 API 设计引导
> （迭代器 + 字节路径优先 + 显式 to_chars 转换）。
> **迁移策略：不考虑**——目前无用户，未来有用户时发布附迁移预处理脚本
> （tie-prep --module 机制，已有先例 prep/rename_tcmsg_to_log.tie）。
> 关联：内存模型（移动语义+arena）、unsafe 模型（ptr/slice）、闭包模型、
> 角色模型（迁移脚本机制）。

## 1. 现状盘点

- 字符串 = **UTF-8，NUL 结尾 char***（LLVM ptr），`str_*` 原语（Rust 侧隐藏堆）
- char = UTF-32 码点（i32）
- 双轨 API：码点级 `str_len`/`str_char`（O(n)）+ 字节级 `byte_len`/`utf8_bytes`
- std/string.tie：trim/slice/contains/find/starts_with/ends_with/replace/split/
  to_upper/to_lower/join/repeat
- std/utf.tie：codepoint/from_code/utf8_bytes/byte_len/hex_escape/is_ascii/
  is_letter/to_char

**核心痛点**：`str_char(i)` 随机码点访问 O(n)；字符串不可见底层（隐藏堆）；
NUL 依赖（非二进制安全）；与移动语义/arena 模型脱节。

## 2. 内部表示（R1 UTF-8 + O2 {ptr,len}）

### 2.1 结构升级：带长度，二进制安全

```tie
// 内部表示：{ ptr: ptr<u8>, len: i64 }
// 分配 len+1 字节，末尾预留 \0（供 FFI 边界零拷贝传递，见 §6）
var s: string = "hello"
// s = { ptr → 'h','e','l','l','o','\0', len = 5 }
```

- **带长度**：不再依赖 NUL 终止扫描（O(1) 长度、可含 \0、二进制安全）
- **字节长度 O(1)**：`len(s)` 直接读字段（现状 str_len 是码点计数 O(n)）
- 分配：arena（区域释放）或静态（字面量 .rodata）
- NUL 预留位：分配 len+1，末尾 \0——传 extern 零拷贝

### 2.2 编码：UTF-8（R1，现状保持）

- 与 C/系统 API 互操作直接（char* 语义）；内存紧凑；嵌入式友好
- 排除 UTF-16/UTF-32（互操作转换成本 + 内存膨胀）

## 3. 所有权（O2：移动语义，与内存模型咬合）

```tie
var a = "hello"
var b = a           // move：a 失效，b 拥有底层缓冲（零拷贝）
var c = clone(a)    // 显式复制
```

- 字符串是**所有权值**（非引用）：move 零拷贝、arena 区域释放
- 与内存模型（移动语义 + arena）完全一致
- 不可变（M1）：共享读安全（多引用读同一字符串——只读不冲突）

## 4. 构建（M2：StringBuilder）

```tie
var sb = string_builder()     // 可变构建器（唯一可变字符串形态）
sb.append("a")
sb.append(42)                  // 自动 to_string
var s = sb.build()             // 一次性产出 string（move）
```

- 不可变 string + 可变 builder：安全与性能兼得
- builder 原地追加（摊销 O(1)），循环拼接不再 O(n²)
- 替代现状的 `s = s + "x"` 循环模式

## 5. 切片（V2：拷贝 + SSO）

```tie
var sub = s.slice(0, 5)       // 短切片内联在栈上（≤23 字节零堆分配）
var big = s.slice(0, 100)     // 长切片堆分配（arena）
```

- **拷贝语义 + 小字符串优化（SSO）**：短切片零分配（高频场景）
- 无生命周期问题（无借用视图）——绕开借用检查（既定决策）
- **视图（&str 零拷贝）后置**：需要借用检查或 unsafe，第二版再评估

## 6. FFI 互操作（F1：边界自动 NUL）

```tie
// 传给 extern（C API 要 char*）：自动 NUL（内部已预留 \0 位，零拷贝）
unsafe { system(cmd) }

// 接收 extern 返回（char*）：自动扫描 NUL → {ptr,len}（拷贝进 arena）
unsafe extern fn getenv(name: string) -> string;
```

- 内部 {ptr,len} 纯二进制安全；边界自动转换（系统 API 友好）
- 预留 \0 位实现：分配 len+1，末尾 \0——传 extern 零拷贝
- 接收方向：扫描 NUL 定长 + 拷贝进 arena（一次拷贝，安全）

## 7. 码点访问（学习 Rust：设计引导，不解决 O(1)）

### 7.1 Rust 的做法（借鉴）

1. **拒绝随机索引语法**：Rust `s[i]` 是编译错误——消灭"按位置取字符"期望
2. **迭代器一次扫描**：`s.chars()` 线性迭代，摊销 O(1)/字符
3. **字节路径优先**：`len()` O(1)、字节切片 O(1)、ASCII 快速路径
4. **显式转换**：`Vec<char>` O(n) 一次，之后 O(1) 索引
5. **不缓存偏移表**：字符串不可变 + 线性访问为主，缓存不值

### 7.2 tie 的 API 设计

```tie
// 迭代器（语言级 for-in 支持）：一次扫描
for c in s.chars() { ... }                 // 码点迭代
for (off, c) in s.char_indices() { ... }   // 字节偏移 + 码点

// 字节路径（O(1)，首选）
var n = len(s)              // 字节长度 O(1)
var b = s.bytes()           // 零拷贝字节视图（slice<u8>）
var sub = s.slice_bytes(0, 3)  // 字节切片 O(1)——要求边界在字符边界（panic 否则）

// 显式转换（O(n) 一次，之后 O(1)）
var cs = to_chars(s)        // → table<char>（码点表）
var c = cs[i]               // O(1) 随机码点访问

// 现有 str_char(i) 保留：明确标注 O(n)（文档 + 建议用 to_chars 替代）

// ASCII 快速路径
if s.is_ascii() {           // 全 ASCII：可安全按字节处理（常见：解析/关键词）
    ...
}
```

- **码点计数**：`utf.codepoint_count(s)` O(n)（迭代计数）——罕见场景
- **`str_len` 语义调整**：现状 = 码点计数（O(n)），新模型 `len(s)` = 字节（O(1)）；
  码点计数显式 `utf.codepoint_count(s)`

### 7.3 设计原则

- **零额外内存**：不缓存偏移表
- **零复杂度**：无数据结构魔法，API 引导正确用法
- **复杂度语义明确**：每个字符串 API 文档标注 O(1)/O(n)

## 8. 字符串与字节（B1：独立）

```tie
var b: bytes = utf8_bytes(s)      // string → bytes（拷贝）
var s2: string = from_bytes(b)    // bytes → string（拷贝，非法 UTF-8 报错）
```

- bytes 独立类型（std/bytes.tie 现状）；string = "合法 UTF-8 的 bytes"
- 二进制数据走 bytes；文本走 string——清晰分离

## 9. 字面量与驻留（L1+L2）

- **字面量静态分配**：编译期字符串放 .rodata（现状），零运行时分配
- **intern 可选**：`std/intern.tie` 显式驻留（O(1) 比较场景：符号表/关键词）
- 驻留表全局状态 → 并发场景需注意（第一版文档化，不加锁）

## 10. 迁移策略（不考虑，脚本兜底）

- **现状**：无用户——字符串表示变更（NUL char* → {ptr,len}）直接破坏
- **未来**：发布时附迁移预处理脚本（tie-prep --module 机制，
  先例 prep/rename_tcmsg_to_log.tie：读源码文本 → 输出迁移后文本）
- 迁移脚本形态：`prep/migrate_str_v1.tie`（顶层 `process(src)->string`），
  发布说明附 `tie-prep --module migrate_str_v1` 用法

## 11. 编译器实现拆解（tiec 自举）

| 模块 | 改动 |
| --- | --- |
| 表示层 | 字符串 {ptr,len} 结构（替代 NUL 依赖）、分配 len+1 预留 \0 |
| 原语层 | str_* 原语适配（len 直读/迭代器/切片字节）、新原语（chars/char_indices/bytes） |
| 语法 | for-in 迭代器（chars/bytes）、string_builder 类型 |
| semantic | 移动语义字符串（与内存模型同）、slice 边界检查 |
| FFI | 边界自动 NUL（传）/ 扫描拷贝（收） |
| std | string_builder、to_chars、codepoint_count、复杂度标注更新 |

## 12. 决策记录（讨论产物）

| 决策点 | 结论 | 备选（未选） |
| --- | --- | --- |
| 编码 | R1 UTF-8（现状保持） | UTF-16、UTF-32 |
| 表示 | O2：{ptr,len} 结构（二进制安全、O(1) 长度） | NUL 依赖（现状） |
| 可变性 | M1 不可变 + M2 StringBuilder | 可变 string |
| 所有权 | O2 移动语义（与内存模型咬合） | 隐藏堆（现状）、引用计数 |
| 切片 | V2 拷贝 + SSO（短切片零分配） | 零拷贝视图 &str（后置） |
| FFI | F1 边界自动 NUL（预留 \0 位零拷贝） | 显式 c_str 转换 |
| 码点访问 | 学习 Rust：迭代器引导 + 字节路径优先 + to_chars 显式转换 | O(1) 随机访问（缓存表） |
| 字节 | B1 bytes 独立类型 | string = bytes 特例 |
| 驻留 | L1 字面量静态 + L2 intern 可选 | 全 intern |
| 迁移 | 不考虑（无用户）；未来迁移预处理脚本（--module 机制） | 渐变路径/兼容层 |

## 13. 未决问题

1. **迭代器语法**：for-in 遍历 chars 的语法形态（`for c in s.chars()` 与现有
   for-in table 的关系——语言需要迭代器协议）
2. **SSO 阈值**：23 字节（Rust 同款）还是按目标平台调整（嵌入式更小）
3. **字符串池与并发**：intern 驻留表的并发访问策略（第一版单线程文档化）
4. **非法 UTF-8 处理**：from_bytes 遇到非法序列——panic（R2 内外分明：内部错误）
5. **代码点迭代的性能**：UTF-8 解码逐字节 vs 查表（第一版逐字节简单实现）
