# compiler/ 协议规范——tie 语言自举编译器模块间通信协议

> 状态：**制定中**（2026-08-10，阶段 2 起步）
> 所属：自举里程碑——「用 tie 语言重写 tie 编译器自身」（docs/plans/self-hosting.md 阶段 2）
> 一句话：编译器各阶段（词法→语法→语义→IR）均为独立 tie 模块，模块间用
> **协议文本**（字符串进、字符串出）通信——与 tie:script 的「字符串为边界」
> 哲学一致，模块完全解耦、可单独测试、可跨进程（Rust 壳 ↔ compiler）。

## 1. 设计原则

1. **模块化**：每个阶段一个文件、一个命名空间（`lexer` / `parser` /
   `semantic` / `ir`），只暴露入口函数；内部实现自由演进，接口只变协议。
2. **协议文本**：模块间传递统一为文本（`TOKENS:` / `AST:` / `IR:` 前缀协议），
   便于调试、缓存、跨语言边界（Rust 壳 eval_call 只能传字符串）。
3. **长度前缀承载字符串**：字符串值（标识符/字面量内容）可能含空格、制表符、
   换行等任意字符，协议内以「码点数前缀 + 原样内容」承载（与 tie:script
   `BODY:<m>` 码点方案同源），不用分隔符，杜绝歧义。
4. **错误约定**：模块出错返回 `ERR:<line> <col> <message>` 单行文本
   （无前缀段落）；成功返回 `段落前缀` 开头的协议文本。调用方按首段判定。
5. **双路径一致**：协议文本使解释路径（interp eval 注册）与编译路径
   （tie-llvm 编译）行为一致；token 编号表是唯一权威（见 §2）。

## 2. token 种类编号表（词法输出，权威）

编号与 Rust 版 `crates/tie-frontend/src/lexer.rs` 的 `TokenKind` 一一对应，
**已分配编号永不复用**。tie 侧常量表见 `compiler/lexer.tie` 顶部。

### 2.1 字面量（0–4）

| 编号 | token | 值字段 |
| --- | --- | --- |
| 0 | Int | num = 整数值 |
| 1 | Float | float = 浮点值（协议文本用 `to_string(f64)`） |
| 2 | Str | str = 已解码转义的字符串内容 |
| 3 | CharLit | num = 字符 Unicode 标量（i64） |
| 4 | Ident | str = 标识符文本 |

### 2.2 关键字（5–29）

| 编号 | token | 编号 | token |
| --- | --- | --- | --- |
| 5 | Func | 18 | Default |
| 6 | Var | 19 | When |
| 7 | Const | 20 | Import |
| 8 | If | 21 | As |
| 9 | Else | 22 | Struct |
| 10 | While | 23 | Extends |
| 11 | For | 24 | Namespace |
| 12 | In | 25 | Pub |
| 13 | Return | 26 | Using |
| 14 | Break | 27 | True |
| 15 | Continue | 28 | False |
| 16 | Switch | 29 | Zero |
| 17 | Case | | |

### 2.3 类型关键字（30–51）

| 编号 | 类型 | 编号 | 类型 |
| --- | --- | --- | --- |
| 30 | i8 | 41 | trit |
| 31 | i16 | 42 | char |
| 32 | i32 | 43 | string |
| 33 | i64 | 44 | void |
| 34 | u8 | 45 | code |
| 35 | u16 | 46 | num |
| 36 | u32 | 47 | text |
| 37 | u64 | 48 | misc |
| 38 | f32 | 49 | table |
| 39 | f64 | 50 | map |
| 40 | bool | 51 | （预留） |

### 2.4 符号（52–98，编号与 Rust `TokenKind` 枚举顺序一一对应）

| 编号 | 符号 | 编号 | 符号 |
| --- | --- | --- | --- |
| 52 | `(` LParen | 66 | `-` Minus |
| 53 | `)` RParen | 67 | `*` Star |
| 54 | `{` LBrace | 68 | `/` Slash |
| 55 | `}` RBrace | 69 | `%` Percent |
| 56 | `[` LBracket | 70 | `=` Eq |
| 57 | `]` RBracket | 71 | `==` EqEq |
| 58 | `,` Comma | 72 | `!=` NotEq |
| 59 | `:` Colon | 73 | `<` Lt |
| 60 | `::` DoubleColon | 74 | `>` Gt |
| 61 | `;` Semi | 75 | `<=` Le |
| 62 | `.` Dot | 76 | `>=` Ge |
| 63 | `..` DotDot | 77 | `&&` AndAnd |
| 64 | `->` Arrow | 78 | `\|\|` OrOr |
| 65 | `+` Plus | 79 | `!` Bang |

| 编号 | 符号 | 编号 | 符号 |
| --- | --- | --- | --- |
| 80 | `+=` PlusEq | 90 | `&` Amp |
| 81 | `-=` MinusEq | 91 | `\|` Pipe |
| 82 | `*=` StarEq | 92 | `^` Caret |
| 83 | `/=` SlashEq | 93 | `<<` Shl |
| 84 | `%=` PercentEq | 94 | `>>` Shr |
| 85 | `&=` AmpEq | 95 | `++` Inc |
| 86 | `\|=` PipeEq | 96 | `--` Dec |
| 87 | `^=` CaretEq | 97 | `?` Question |
| 88 | `<<=` ShlEq | 98 | Eof |
| 89 | `>>=` ShrEq | | |

## 3. 词法协议（lexer 输出 / parser 输入）

```
TOKENS:<n>                        ← 第 1 行：token 总数（含 Eof）
<kind> <line> <col> <num> <float> <slen> <str>   ← 之后 n 行，每行一个 token
```

- `<kind>`：§2 编号（十进制 i64）；
- `<line>` / `<col>`：起始行列（从 1 开始）；
- `<num>`：整数值字段（Int 值 / Char 标量 / 无值 0）；
- `<float>`：浮点值字段（`to_string(f64)`；无值 0）；
- `<slen>`：字符串值码点数（str_len 语义；无值 0）；
- `<str>`：恰好 `<slen>` 个码点的**转义表示**（长度前缀保证可含任意字符；
  解析方按码点数精确截取，不按分隔符）。转义规则（与 `lexer.escape_str` 对应，
  parser 用 `unescape_str` 还原）：

  | 原字符 | 转义文本 | 说明 |
  | --- | --- | --- |
  | `\n` | `\\n` | 换行 |
  | `\t` | `\\t` | 制表符 |
  | `\r` | `\\r` | 回车 |
  | `\\` | `\\\\` | 反斜杠 |
  | `\"` | `\\\"` | 双引号 |
  | `\'` | `\\'` | 单引号 |
  | `\0` | `\\0` | 空字符（NUL） |
  | 其余 | 原样 | 含中文等多字节字符 |

  > 为何转义（自举阶段 2 清扫的 C 边界障碍）：协议文本经 eval_call 的 C ABI 桥
  > （`tie_eval_call` → `CString`）返回，字符串含真实 `\0`（源码 `"\0"` 字面量
  > 解码后）会被 CString 截断 → 调用方拿到空串。转义后协议文本纯可打印，
  > 跨 C 边界安全、可调试。

字段间以**单个空格**分隔；`<str>` 是行尾字段，其后无内容。
示例（`var x = 1` 的词法输出，4 个 token + Eof）：

```
TOKENS:5
6 1 1 0 0 0
4 1 5 0 0 1 x
70 1 7 0 0 0
0 1 9 1 0 0
96 1 10 0 0 0
```

## 4. 错误协议（所有模块统一）

```
ERR:<line> <col> <message>
```

- 出错即返回此行（无其他段落）；`<message>` 为错误描述文本（可含空格）。

## 5. 模块清单与接口（阶段 2 全貌）

| 文件 | 命名空间 | 入口函数 | 输入 → 输出 |
| --- | --- | --- | --- |
| compiler/util.tie | util | （工具集，无入口） | 字符串/进制解析/表工具，供各模块调用 |
| compiler/lexer.tie | lexer | `tokenize(src) -> string` | 源码 → `TOKENS:` 协议文本 |
| compiler/parser.tie | parser | `parse(tokens_text) -> string` | `TOKENS:` → `AST:` 协议文本 |
| compiler/semantic.tie | semantic | `check(ast_text) -> string` | `AST:` → 校验结果（`OK:` / 错误表） |
| compiler/ir.tie | ir | `gen(ast_text) -> string` | `AST:` → LLVM IR 文本 |
| compiler/main.tie | compiler | `compile(src) -> string` | 源码 → `.ll` 文本（Rust 壳入口） |

> parser/semantic/ir 的 `AST:` 协议与 B1 tag 表编码（self-hosting.md §3.5
> 列式并行表）在对应模块实现时再细化，本文件先定 token 层。

## 6. 与 Rust 版行为的对齐基准

- token 编号表 ↔ `crates/tie-frontend/src/lexer.rs` `TokenKind`；
- ASI 补分号规则 ↔ lexer.rs `finish_line`（行尾可结束集合、括号深度）；
- 数字扫描 ↔ `scan_number`（进制前缀 0x/0b/0o/0t、指数、`1..10` 不误判）；
- 字符串/字符转义 ↔ `scan_string` / `scan_char`（`\n \t \r \\ \" \' \0`）；
- 错误消息文本与位置 ↔ 各扫描函数（`块注释未闭合` / `字符串未闭合` /
  `字符字面量只能包含一个字符` / `未知转义序列 \X` / `无法识别的字符 'X'`）。
