//! 词法分析器（Lexer）。
//!
//! 职责：把 tie 源码字符串转换为 token 流，并在换行处按 ASI 规则
//! 自动补充分号 token（Semi），使解析器无需关心行尾分隔符。
//!
//! # ASI（自动分号补全）规则
//!
//! 单行一条语句时行尾不需要写分号，编译器自动补全：
//! - 换行时若行内已有 token、括号深度为 0、且行尾 token 属于
//!   「可结束语句」集合 → 插入 `Semi`。
//! - 行尾是二元运算符/逗号/冒号/开括号/点/`else`/`in` 等 → 语句未结束，不补。
//! - 行尾 token 是 `{` / `}` / `;` 本身 → 不补（块边界自有语义）。

use std::fmt;

/// token 在源码中的位置（用于报错定位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// 起始行（从 1 开始）
    pub line: u32,
    /// 起始列（从 1 开始）
    pub col: u32,
}

/// 词法错误：携带位置与信息，供 driver 汇总报告。
#[derive(Debug, Clone)]
pub struct LexError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "词法错误 @{}:{}: {}", self.span.line, self.span.col, self.message)
    }
}

/// token 种类。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ---- 字面量 ----
    /// 整数（i64）
    Int(i64),
    /// 浮点数（f64）
    Float(f64),
    /// 字符串字面量（已解码转义）
    Str(String),
    /// 字符字面量（已解码转义，存单字符的 UTF-32 值）
    CharLit(char),
    /// 标识符
    Ident(String),

    // ---- 关键字 ----
    /// 函数定义 `func`
    Func,
    /// 可变变量声明 `var`
    Var,
    /// 不可变变量声明 `const`
    Const,
    If,
    Else,
    While,
    For,
    In,
    Return,
    /// 多分支选择 `switch`
    Switch,
    /// switch 分支 `case`
    Case,
    /// switch 默认分支 `default`
    Default,
    /// switch 守卫条件 `when`（模式匹配增强：`case 8 when flag:`）
    When,
    Import,
    As,
    /// struct 定义 `struct`（纯数据，M2.1.8；取代 class）
    Struct,
    /// 继承 `extends`
    Extends,
    /// 命名空间声明 `namespace tcmsg { }`
    Namespace,
    /// 公有可见性标记（M2.1.7 单文件命名空间）：`pub func`——命名空间内函数
    /// 默认私有（仅同命名空间可见），加 pub 后跨命名空间/跨文件可调
    Pub,
    /// using 引入语句（M2.1.7）：`using fmt2;` 把已导入命名空间的公有函数
    /// 引入当前文件，之后可裸调用
    Using,
    True,
    False,
    /// 平衡三进制 trit 的零值字面量（M4 补齐）：`zero`——trit 三值 true(+1)/zero(0)/false(-1)
    Zero,
    /// 类型关键字：i8..u64/f32/f64/bool/trit/char/string/void/code/num/text/misc/table
    TypeKw(TyKw),

    // ---- 符号 ----
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    /// 命名空间路径分隔符 `::`（C#/Rust 风格，如 `tcmsg::error`）
    DoubleColon,
    /// 分号（显式写出或 ASI 自动补全）
    Semi,
    Dot,
    DotDot,   // ..
    Arrow,    // ->
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,       // =
    EqEq,     // ==
    NotEq,    // !=
    Lt,
    Gt,
    Le,
    Ge,
    AndAnd,   // &&
    OrOr,     // ||
    Bang,     // !

    // ---- M4 运算符扩展：复合赋值 / 位运算 / 自增自减 / 三目 ----
    PlusEq,     // +=
    MinusEq,    // -=
    StarEq,     // *=
    SlashEq,    // /=
    PercentEq,  // %=
    AmpEq,      // &=
    PipeEq,     // |=
    CaretEq,    // ^=
    ShlEq,      // <<=
    ShrEq,      // >>=
    Amp,        // &
    Pipe,       // |
    Caret,      // ^
    Shl,        // <<
    Shr,        // >>
    Inc,        // ++
    Dec,        // --
    Question,   // ?

    // ---- 特殊 ----
    Eof,
}

/// 类型关键字枚举（Rust 风格：i8/i16/i32/i64/u8/u16/u32/u64/f32/f64/bool/char/string/void/code，
/// 外加宽类型 num/text/misc 与表类型 table）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TyKw {
    /// 有符号整数
    I8,
    I16,
    I32,
    I64,
    /// 无符号整数
    U8,
    U16,
    U32,
    U64,
    /// 浮点数
    F32,
    F64,
    /// 布尔
    Bool,
    /// 平衡三进制 trit（三值逻辑：-1/0/+1，数论常用；M4 补齐）
    Trit,
    /// 字符
    Char,
    /// 字符串
    Str,
    /// 空类型（函数返回）
    Void,
    /// 代码数据（code 是代码类型的关键词）
    Code,
    /// 宽类型：数（覆盖全部整数与浮点）
    Num,
    /// 宽类型：文（覆盖字符串与字符）
    Text,
    /// 宽类型：其他（覆盖其余全部类型）
    Misc,
    /// 表类型：数组与高级数组（表）
    Table,
}

impl TyKw {
    /// 关键字文本。
    pub fn as_str(self) -> &'static str {
        match self {
            TyKw::I8 => "i8",
            TyKw::I16 => "i16",
            TyKw::I32 => "i32",
            TyKw::I64 => "i64",
            TyKw::U8 => "u8",
            TyKw::U16 => "u16",
            TyKw::U32 => "u32",
            TyKw::U64 => "u64",
            TyKw::F32 => "f32",
            TyKw::F64 => "f64",
            TyKw::Bool => "bool",
            TyKw::Trit => "trit",
            TyKw::Char => "char",
            TyKw::Str => "string",
            TyKw::Void => "void",
            TyKw::Code => "code",
            TyKw::Num => "num",
            TyKw::Text => "text",
            TyKw::Misc => "misc",
            TyKw::Table => "table",
        }
    }

    /// 是否为整数类型（含无符号）。
    pub fn is_int(self) -> bool {
        matches!(
            self,
            TyKw::I8 | TyKw::I16 | TyKw::I32 | TyKw::I64 | TyKw::U8 | TyKw::U16 | TyKw::U32 | TyKw::U64
        )
    }

    /// 是否为浮点类型。
    pub fn is_float(self) -> bool {
        matches!(self, TyKw::F32 | TyKw::F64)
    }

    /// 是否为宽类型（num/text/misc，接受范围在语义分析时展开）。
    pub fn is_wide(self) -> bool {
        matches!(self, TyKw::Num | TyKw::Text | TyKw::Misc)
    }
}

impl TokenKind {
    /// 是否为二元运算符（用于 ASI 判断：行尾是运算符则语句未结束）。
    ///
    /// M4 说明：复合赋值/位运算/移位/三目问号都算「运算符」→ 行尾不补分号；
    /// 自增自减 `++`/`--` 特意**不加入**——后缀 `i++\n` 应结束语句（补分号）。
    fn is_bin_op(&self) -> bool {
        matches!(
            self,
            TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::EqEq
                | TokenKind::NotEq
                | TokenKind::Lt
                | TokenKind::Gt
                | TokenKind::Le
                | TokenKind::Ge
                | TokenKind::AndAnd
                | TokenKind::OrOr
                | TokenKind::Eq
                | TokenKind::PlusEq
                | TokenKind::MinusEq
                | TokenKind::StarEq
                | TokenKind::SlashEq
                | TokenKind::PercentEq
                | TokenKind::AmpEq
                | TokenKind::PipeEq
                | TokenKind::CaretEq
                | TokenKind::ShlEq
                | TokenKind::ShrEq
                | TokenKind::Amp
                | TokenKind::Pipe
                | TokenKind::Caret
                | TokenKind::Shl
                | TokenKind::Shr
                | TokenKind::Question
        )
    }
}

/// 单个 token：种类 + 源码位置。
#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    /// 构造带位置的 token。
    pub fn new(kind: TokenKind, line: u32, col: u32) -> Self {
        Self { kind, span: Span { line, col } }
    }
}

/// 词法分析器。
pub struct Lexer<'a> {
    /// 源码字符序列
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    /// 当前行号（从 1 开始）
    line: u32,
    /// 当前列号（从 1 开始）
    col: u32,
    /// 当前行内是否已产生过 token（决定换行时是否补分号）
    line_has_token: bool,
    /// 行尾最后一个 token 的种类（ASI 判定用）
    last_line_token: Option<TokenKind>,
    /// 括号深度（`(`/`[`/`{` 内换行不补分号）
    paren_depth: u32,
    /// 已扫描到的 token（含 ASI 补全）
    tokens: Vec<Token>,
}

/// 词法分析入口：源码 → token 流（已做 ASI 补全）。
pub fn tokenize(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).run()
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().peekable(),
            line: 1,
            col: 1,
            line_has_token: false,
            last_line_token: None,
            paren_depth: 0,
            tokens: Vec::new(),
        }
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        while let Some(c) = self.chars.peek().copied() {
            match c {
                // ---- 空白：行尾触发 ASI 判定 ----
                '\n' => {
                    self.consume_char();
                    self.finish_line();
                }
                ' ' | '\t' | '\r' => {
                    self.consume_char();
                }
                // ---- 注释 ----
                '/' if self.peek_next() == Some('/') => {
                    self.scan_line_comment()?;
                }
                '/' if self.peek_next() == Some('*') => {
                    self.scan_block_comment()?;
                }
                // ---- 字符串 ----
                '"' => {
                    let tok = self.scan_string()?;
                    self.push(tok);
                }
                // ---- 字符字面量 ----
                '\'' => {
                    let tok = self.scan_char()?;
                    self.push(tok);
                }
                // ---- 数字 ----
                c if c.is_ascii_digit() => {
                    let tok = self.scan_number();
                    self.push(tok);
                }
                // ---- 标识符 / 关键字 ----
                c if c.is_alphabetic() || c == '_' => {
                    let tok = self.scan_ident();
                    self.push(tok);
                }
                // ---- 符号 ----
                _ => {
                    let tok = self.scan_symbol()?;
                    self.push(tok);
                }
            }
        }
        // 文件末尾补一个 Eof
        self.tokens.push(Token::new(TokenKind::Eof, self.line, self.col));
        Ok(self.tokens)
    }

    // ---------- 基础工具 ----------

    /// 消费一个字符并推进行列号。
    fn consume_char(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    /// 预读当前字符之后的那个字符（不消费）。
    ///
    /// 注意：`Peekable::clone()` 会保留已 peek 的字符，因此先 `next()` 丢弃
    /// 当前字符，再 `next()` 才是真正的下一个字符。
    fn peek_next(&mut self) -> Option<char> {
        let mut it = self.chars.clone();
        it.next(); // 丢弃当前已 peek 的字符（若有）
        it.next() // 返回真正的下一个字符
    }

    /// 记录一个 token 到流中，并更新 ASI 追踪状态。
    fn push(&mut self, tok: Token) {
        self.line_has_token = true;
        // 括号深度只统计 `(` 与 `[`：
        // - `{`/`}` 是块边界，天然分隔语句，不参与换行补分号判定
        // - `(`/`[` 内换行表示表达式续行，不补分号
        match &tok.kind {
            TokenKind::LParen | TokenKind::LBracket => self.paren_depth += 1,
            TokenKind::RParen | TokenKind::RBracket => {
                self.paren_depth = self.paren_depth.saturating_sub(1)
            }
            _ => {}
        }
        self.last_line_token = Some(tok.kind.clone());
        self.tokens.push(tok);
    }

    /// 处理行尾：按 ASI 规则决定是否补分号，并重置行内状态。
    fn finish_line(&mut self) {
        if self.line_has_token && self.paren_depth == 0 {
            let should_insert = match &self.last_line_token {
                None => false,
                Some(k) => {
                    // 行尾 token 属「可结束语句」集合才补分号；
                    // `}`/`;`/块边界与续行符不补
                    !k.is_bin_op()
                        && !matches!(
                            k,
                            TokenKind::Comma
                                | TokenKind::Colon
                                | TokenKind::DoubleColon
                                | TokenKind::Dot
                                | TokenKind::DotDot
                                | TokenKind::LParen
                                | TokenKind::LBracket
                                | TokenKind::LBrace
                                | TokenKind::RBrace
                                | TokenKind::Semi
                                | TokenKind::Else
                                | TokenKind::In
                        )
                }
            };
            if should_insert {
                let tok = Token::new(TokenKind::Semi, self.line, self.col);
                self.tokens.push(tok);
            }
        }
        self.line_has_token = false;
        self.last_line_token = None;
    }

    // ---------- 各 token 扫描 ----------

    /// 行注释 `// ...`（不产生 token；头部 `// tie:` 已由 tie-prep 提取）。
    fn scan_line_comment(&mut self) -> Result<(), LexError> {
        // 消费 "//"
        self.consume_char();
        self.consume_char();
        while let Some(&c) = self.chars.peek() {
            if c == '\n' {
                break;
            }
            self.consume_char();
        }
        Ok(())
    }

    /// 块注释 `/* ... */`（不产生 token）。
    fn scan_block_comment(&mut self) -> Result<(), LexError> {
        let (line, col) = (self.line, self.col);
        self.consume_char(); // '/'
        self.consume_char(); // '*'
        loop {
            match self.chars.peek().copied() {
                None => {
                    return Err(LexError {
                        span: Span { line, col },
                        message: "块注释未闭合".into(),
                    })
                }
                Some('*') if self.peek_next() == Some('/') => {
                    self.consume_char();
                    self.consume_char();
                    return Ok(());
                }
                Some(c) => {
                    self.consume_char();
                    let _ = c;
                }
            }
        }
    }

    /// 字符串字面量（支持 `\n` `\t` `\\` `\"` 转义）。
    fn scan_string(&mut self) -> Result<Token, LexError> {
        let (line, col) = (self.line, self.col);
        self.consume_char(); // 开引号
        let mut s = String::new();
        loop {
            match self.chars.peek().copied() {
                None => {
                    return Err(LexError {
                        span: Span { line, col },
                        message: "字符串未闭合".into(),
                    })
                }
                Some('"') => {
                    self.consume_char();
                    return Ok(Token::new(TokenKind::Str(s), line, col));
                }
                Some('\\') => {
                    self.consume_char();
                    let esc = self
                        .consume_char()
                        .ok_or_else(|| LexError {
                            span: Span { line, col },
                            message: "转义符后缺少字符".into(),
                        })?;
                    match esc {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        '\'' => s.push('\''),
                        '0' => s.push('\0'),
                        other => {
                            return Err(LexError {
                                span: Span { line, col },
                                message: format!("未知转义序列 \\{other}"),
                            })
                        }
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.consume_char();
                }
            }
        }
    }

    /// 字符字面量 `'a'` / `'\n'`（单引号包裹，恰好一个字符，支持转义）。
    fn scan_char(&mut self) -> Result<Token, LexError> {
        let (line, col) = (self.line, self.col);
        self.consume_char(); // 开引号
        let ch = match self.chars.peek().copied() {
            None => {
                return Err(LexError {
                    span: Span { line, col },
                    message: "字符字面量未闭合".into(),
                })
            }
            Some('\\') => {
                self.consume_char();
                let esc = self
                    .consume_char()
                    .ok_or_else(|| LexError {
                        span: Span { line, col },
                        message: "转义符后缺少字符".into(),
                    })?;
                match esc {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    '\\' => '\\',
                    '"' => '"',
                    '\'' => '\'',
                    '0' => '\0',
                    other => {
                        return Err(LexError {
                            span: Span { line, col },
                            message: format!("未知转义序列 \\{other}"),
                        })
                    }
                }
            }
            Some(c) => {
                self.consume_char();
                c
            }
        };
        // 闭合引号
        match self.chars.peek().copied() {
            Some('\'') => {
                self.consume_char();
                Ok(Token::new(TokenKind::CharLit(ch), line, col))
            }
            // 多字符字面量（如 'ab'）不合法
            Some(_) => Err(LexError {
                span: Span { line, col },
                message: "字符字面量只能包含一个字符".into(),
            }),
            None => Err(LexError {
                span: Span { line, col },
                message: "字符字面量未闭合".into(),
            }),
        }
    }

    /// 数字字面量：整数（十进制/十六进制/二进制/八进制/三进制）或浮点数（含小数点与指数）。
    ///
    /// 进制前缀（M4 补齐，C 风格）：
    /// - `0x` / `0X` → 十六进制（`0xFF` = 255，数字 0-9 与字母 a-f/A-F）；
    /// - `0b` / `0B` → 二进制（`0b1010` = 10，仅 0/1）；
    /// - `0o` / `0O` → 八进制（`0o17` = 15，仅 0-7）；
    /// - `0t` / `0T` → 三进制（`0t210` = 21，仅 0-2；t = ternary，数论常用）；
    /// - 无前缀 → 十进制整数或浮点数（`42` / `3.14` / `1e5`，行为与历史一致）。
    /// 进制字面量恒为整数（不支持小数）；解析失败（如 `0x` 后无数字）回退为 0。
    fn scan_number(&mut self) -> Token {
        let (line, col) = (self.line, self.col);
        let mut text = String::new();
        let mut is_float = false;
        // 进制前缀检测：以 0 开头且下一字符是 x/b/o/t（大小写均可）
        let mut radix: i64 = 10;
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_digit() {
                text.push(c);
                self.consume_char();
            } else if c == '.' && !is_float {
                // 仅当下一个字符也是数字时视作小数（避免 `1..10` 中的 `..`）
                let is_decimal = matches!(self.peek_next(), Some(n) if n.is_ascii_digit());
                if is_decimal {
                    is_float = true;
                    text.push(c);
                    self.consume_char();
                    continue;
                }
                break;
            } else if matches!(c, 'e' | 'E') && !text.contains(['e', 'E']) {
                // 指数：1e5 / 1.5e-3
                let mut ahead = self.chars.clone();
                ahead.next(); // 'e'
                let sign = matches!(ahead.peek(), Some('-') | Some('+'));
                if sign {
                    ahead.next();
                }
                if matches!(ahead.peek(), Some(c) if c.is_ascii_digit()) {
                    is_float = true;
                    text.push(c);
                    self.consume_char();
                    if sign {
                        text.push(self.consume_char().unwrap());
                    }
                    while let Some(&d) = self.chars.peek() {
                        if d.is_ascii_digit() {
                            text.push(d);
                            self.consume_char();
                        } else {
                            break;
                        }
                    }
                }
                break;
            } else {
                break;
            }
        }
        // 进制前缀处理（在文本已收齐后判断）：0x/0b/0o/0t 前缀 → 按对应进制解析。
        // 注意：必须在主循环结束后处理——主循环只收数字，`0x` 的 `x` 会在
        // else 分支停下，此时 text 为 "0"；这里补扫进制的数字与字母。
        if text == "0" {
            // 窥探下一字符决定进制（不消费——由下面分支统一消费）
            match self.chars.peek().copied() {
                Some('x') | Some('X') => {
                    self.consume_char(); // 吃掉 'x'
                    radix = 16;
                    // 十六进制数字：0-9 + a-f + A-F
                    while let Some(&d) = self.chars.peek() {
                        if d.is_ascii_hexdigit() {
                            text.push(d);
                            self.consume_char();
                        } else {
                            break;
                        }
                    }
                }
                Some('b') | Some('B') => {
                    self.consume_char(); // 吃掉 'b'
                    radix = 2;
                    while let Some(&d) = self.chars.peek() {
                        if matches!(d, '0' | '1') {
                            text.push(d);
                            self.consume_char();
                        } else {
                            break;
                        }
                    }
                }
                Some('o') | Some('O') => {
                    self.consume_char(); // 吃掉 'o'
                    radix = 8;
                    while let Some(&d) = self.chars.peek() {
                        if matches!(d, '0'..='7') {
                            text.push(d);
                            self.consume_char();
                        } else {
                            break;
                        }
                    }
                }
                Some('t') | Some('T') => {
                    self.consume_char(); // 吃掉 't'
                    radix = 3;
                    while let Some(&d) = self.chars.peek() {
                        if matches!(d, '0'..='2') {
                            text.push(d);
                            self.consume_char();
                        } else {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        let kind = if is_float {
            TokenKind::Float(text.parse::<f64>().unwrap_or(0.0))
        } else if radix != 10 {
            // 进制整数：按对应进制解析（i64 溢出回绕为 0——与十进制 parse 一致防御）
            TokenKind::Int(Self::parse_radix(&text, radix))
        } else {
            TokenKind::Int(text.parse::<i64>().unwrap_or(0))
        };
        Token::new(kind, line, col)
    }

    /// 按指定进制（2/3/8/16）解析整数字面量文本（含 a-f/A-F 字母；非法输入回退 0）。
    ///
    /// 与 Rust `i64::from_str_radix` 语义一致但**不报错**——词法阶段对非法/溢出
    /// 输入回退 0（与十进制 `text.parse::<i64>().unwrap_or(0)` 的防御约定一致）。
    fn parse_radix(text: &str, radix: i64) -> i64 {
        let mut result: i64 = 0;
        for c in text.chars() {
            // 数字 0-9 与字母 a-f/A-F → 值 0..15（进制数字超过 9 的部分由调用方保证合法）
            let digit = match c {
                '0'..='9' => c as i64 - '0' as i64,
                'a'..='f' => c as i64 - 'a' as i64 + 10,
                'A'..='F' => c as i64 - 'A' as i64 + 10,
                _ => return 0, // 非法字符 → 整体回退 0
            };
            if digit >= radix {
                return 0; // 超出该进制数字范围 → 回退 0
            }
            // 溢出防护：进位前检查——若 result 已超过 (i64::MAX - digit) / radix，
            // 下一次进位必溢出（debug 构建乘法溢出会 panic，必须显式拦截）→ 回退 0
            if result > (i64::MAX - digit) / radix {
                return 0;
            }
            result = result * radix + digit;
        }
        result
    }

    /// 标识符 / 关键字。
    fn scan_ident(&mut self) -> Token {
        let (line, col) = (self.line, self.col);
        let mut text = String::new();
        while let Some(&c) = self.chars.peek() {
            if c.is_alphanumeric() || c == '_' {
                text.push(c);
                self.consume_char();
            } else {
                break;
            }
        }
        let kind = match text.as_str() {
            "func" => TokenKind::Func,
            "var" => TokenKind::Var,
            "const" => TokenKind::Const,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "return" => TokenKind::Return,
            "switch" => TokenKind::Switch,
            "case" => TokenKind::Case,
            "default" => TokenKind::Default,
            // switch 模式匹配增强：守卫条件 `case 8 when flag:`
            "when" => TokenKind::When,
            "import" => TokenKind::Import,
            "as" => TokenKind::As,
            // 面向对象（M2.1.8）：struct 纯数据（class/this/static 废弃，逻辑走命名空间函数）
            "struct" => TokenKind::Struct,
            "extends" => TokenKind::Extends,
            // 命名空间（命名空间语法，C# 风格块式声明）
            "namespace" => TokenKind::Namespace,
            // M2.1.7 单文件命名空间：pub 可见性标记 + using 引入语句
            "pub" => TokenKind::Pub,
            "using" => TokenKind::Using,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            // 平衡三进制 trit 零值（M4 补齐）：zero 是保留字
            "zero" => TokenKind::Zero,
            "i8" => TokenKind::TypeKw(TyKw::I8),
            "i16" => TokenKind::TypeKw(TyKw::I16),
            "i32" => TokenKind::TypeKw(TyKw::I32),
            "i64" => TokenKind::TypeKw(TyKw::I64),
            "u8" => TokenKind::TypeKw(TyKw::U8),
            "u16" => TokenKind::TypeKw(TyKw::U16),
            "u32" => TokenKind::TypeKw(TyKw::U32),
            "u64" => TokenKind::TypeKw(TyKw::U64),
            "f32" => TokenKind::TypeKw(TyKw::F32),
            "f64" => TokenKind::TypeKw(TyKw::F64),
            "bool" => TokenKind::TypeKw(TyKw::Bool),
            "trit" => TokenKind::TypeKw(TyKw::Trit),
            "char" => TokenKind::TypeKw(TyKw::Char),
            "string" => TokenKind::TypeKw(TyKw::Str),
            "void" => TokenKind::TypeKw(TyKw::Void),
            "code" => TokenKind::TypeKw(TyKw::Code),
            // 宽类型（类别框）：num/text/misc 加快编译推导
            "num" => TokenKind::TypeKw(TyKw::Num),
            "text" => TokenKind::TypeKw(TyKw::Text),
            "misc" => TokenKind::TypeKw(TyKw::Misc),
            // 表类型（数组与高级数组）
            "table" => TokenKind::TypeKw(TyKw::Table),
            _ => TokenKind::Ident(text),
        };
        Token::new(kind, line, col)
    }

    /// 符号：单字符 / 双字符 / 三字符（`<<=` `>>=`）运算符。
    fn scan_symbol(&mut self) -> Result<Token, LexError> {
        let (line, col) = (self.line, self.col);
        let c = self.consume_char().unwrap();
        // 三字符移位复合赋值 `<<=` / `>>=`：与双字符移位 `<<`/`>>` 拆开单独处理
        //（否则 pair 表先命中 Shl/Shr 就返回，第三个 `=` 会被误认成下一个 token）。
        if let Some(next) = self.chars.peek().copied() {
            if (c == '<' && next == '<') || (c == '>' && next == '>') {
                self.consume_char(); // 消费第二个 '<' / '>'
                // 第三个字符是 '=' → 复合移位赋值 `<<=` / `>>=`
                if self.chars.peek() == Some(&'=') {
                    self.consume_char();
                    let kind = if c == '<' { TokenKind::ShlEq } else { TokenKind::ShrEq };
                    return Ok(Token::new(kind, line, col));
                }
                let kind = if c == '<' { TokenKind::Shl } else { TokenKind::Shr };
                return Ok(Token::new(kind, line, col));
            }
        }
        // 双字符符号
        if let Some(next) = self.chars.peek().copied() {
            let pair = match (c, next) {
                ('=', '=') => Some(TokenKind::EqEq),
                ('!', '=') => Some(TokenKind::NotEq),
                ('<', '=') => Some(TokenKind::Le),
                ('>', '=') => Some(TokenKind::Ge),
                ('&', '&') => Some(TokenKind::AndAnd),
                ('|', '|') => Some(TokenKind::OrOr),
                ('.', '.') => Some(TokenKind::DotDot),
                ('-', '>') => Some(TokenKind::Arrow),
                // 命名空间路径分隔符 `::`（双冒号；单个 `:` 仍是 Colon）
                (':', ':') => Some(TokenKind::DoubleColon),
                // M4 复合赋值：`op =`
                ('+', '=') => Some(TokenKind::PlusEq),
                ('-', '=') => Some(TokenKind::MinusEq),
                ('*', '=') => Some(TokenKind::StarEq),
                ('/', '=') => Some(TokenKind::SlashEq),
                ('%', '=') => Some(TokenKind::PercentEq),
                ('&', '=') => Some(TokenKind::AmpEq),
                ('|', '=') => Some(TokenKind::PipeEq),
                ('^', '=') => Some(TokenKind::CaretEq),
                // M4 自增自减：`++` / `--`
                ('+', '+') => Some(TokenKind::Inc),
                ('-', '-') => Some(TokenKind::Dec),
                _ => None,
            };
            if let Some(kind) = pair {
                self.consume_char();
                return Ok(Token::new(kind, line, col));
            }
        }
        // 单字符符号
        let kind = match c {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ':' => TokenKind::Colon,
            ';' => TokenKind::Semi,
            '.' => TokenKind::Dot,
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '=' => TokenKind::Eq,
            '<' => TokenKind::Lt,
            '>' => TokenKind::Gt,
            '!' => TokenKind::Bang,
            // M4 位运算与三目
            '&' => TokenKind::Amp,
            '|' => TokenKind::Pipe,
            '^' => TokenKind::Caret,
            '?' => TokenKind::Question,
            other => {
                return Err(LexError {
                    span: Span { line, col },
                    message: format!("无法识别的字符 '{other}'"),
                })
            }
        };
        Ok(Token::new(kind, line, col))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- ASI 自动分号补全 ----------

    /// 行尾是标识符/整数时换行补分号；补出的分号位于下一行行首
    /// （finish_line 在消费换行后执行，故 span 是下一行 (line+1, 1)）。
    #[test]
    fn 行尾标识符自动补分号() {
        let toks = tokenize("var x = 1\nvar y").expect("不应报错");
        let semis: Vec<_> = toks.iter().filter(|t| t.kind == TokenKind::Semi).collect();
        assert_eq!(semis.len(), 1, "仅第一行行尾应补一个分号");
        assert_eq!((semis[0].span.line, semis[0].span.col), (2, 1), "补出的分号在下一行行首");
    }

    /// 行尾是整数/字符串时换行补分号，每个语句行各补一个。
    #[test]
    fn 行尾整数与字符串自动补分号() {
        let toks = tokenize("x = 1\ns = \"hi\"\n").expect("不应报错");
        let semis: Vec<_> = toks.iter().filter(|t| t.kind == TokenKind::Semi).collect();
        assert_eq!(semis.len(), 2, "两行语句各补一个分号");
        // 第一个分号紧跟整数 1（证明是整数行尾补出来的，而不是字符串行）
        let int_idx = toks.iter().position(|t| t.kind == TokenKind::Int(1)).expect("有整数 1");
        assert!(matches!(toks[int_idx + 1].kind, TokenKind::Semi), "整数后应紧跟补出的分号");
    }

    /// 行尾是二元运算符（+ - * / % == != < > <= >= && || =）→ 语句未结束，不补分号。
    #[test]
    fn 行尾二元运算符不补分号() {
        for src in [
            "a +\nb", "a -\nb", "a *\nb", "a /\nb", "a %\nb", "a ==\nb", "a !=\nb", "a <\nb",
            "a >\nb", "a <=\nb", "a >=\nb", "a &&\nb", "a ||\nb", "a =\nb",
        ] {
            let toks = tokenize(src).expect("不应报错");
            assert!(
                !toks.iter().any(|t| t.kind == TokenKind::Semi),
                "源码 {src:?} 行尾是二元运算符，不应补分号"
            );
        }
    }

    /// 行尾是 M4 运算符（复合赋值/位运算/移位/三目问号）→ 语句未结束，不补分号；
    /// 但 `++`/`--` 不算二元运算符，后缀 `i++\n` 行尾应补分号结束语句。
    #[test]
    fn 行尾m4运算符不补分号() {
        for src in [
            "x +=\n1", "x -=\n1", "x *=\n1", "x /=\n1", "x %=\n1", "x &=\n1", "x |=\n1",
            "x ^=\n1", "x <<=\n1", "x >>=\n1", "a &\nb", "a |\nb", "a ^\nb", "a <<\nb",
            "a >>\nb", "a ?\nb : c",
        ] {
            let toks = tokenize(src).expect("不应报错");
            assert!(
                !toks.iter().any(|t| t.kind == TokenKind::Semi),
                "源码 {src:?} 行尾是 M4 运算符，不应补分号"
            );
        }
        // 对照组：后缀 `i++` / `i--` 行尾应补分号（自增自减不加入 is_bin_op）
        for src in ["i++\n", "i--\n"] {
            let toks = tokenize(src).expect("不应报错");
            assert!(
                toks.iter().any(|t| t.kind == TokenKind::Semi),
                "源码 {src:?} 行尾是自增/自减，应补分号结束语句"
            );
        }
    }

    /// `(` / `[` 内换行表示表达式续行，不补分号；闭合括号后的行尾才补。
    #[test]
    fn 括号内换行不补分号() {
        for src in ["f(1,\n2)\n", "[1,\n2]\n"] {
            let toks = tokenize(src).expect("不应报错");
            let semis: Vec<_> = toks.iter().filter(|t| t.kind == TokenKind::Semi).collect();
            assert_eq!(semis.len(), 1, "源码 {src:?} 应只在右括号行尾补一个分号");
            let semi_idx = toks.iter().position(|t| t.kind == TokenKind::Semi).expect("有分号");
            assert!(
                matches!(toks[semi_idx - 1].kind, TokenKind::RParen | TokenKind::RBracket),
                "分号应跟在右括号之后"
            );
        }
    }

    /// 行尾是 `else` / `in` → 语句未结束，不补分号。
    #[test]
    fn 行尾else与in不补分号() {
        // else 行尾：else 后应紧跟左大括号，中间无分号
        let toks = tokenize("if x\nelse {").expect("不应报错");
        let else_idx = toks.iter().position(|t| t.kind == TokenKind::Else).expect("有 else");
        assert!(matches!(toks[else_idx + 1].kind, TokenKind::LBrace), "else 后应紧跟左大括号");
        // in 行尾：整个流中不应出现分号
        let toks = tokenize("for x in\nlist").expect("不应报错");
        assert!(!toks.iter().any(|t| t.kind == TokenKind::Semi), "in 行尾不补分号");
    }

    /// 行尾是 `{` / `}` → 块边界，不补分号（finish_line 排除列表含 LBrace/RBrace）。
    #[test]
    fn 行尾大括号不补分号() {
        let toks = tokenize("if x\n{\n}").expect("不应报错");
        let semis: Vec<_> = toks.iter().filter(|t| t.kind == TokenKind::Semi).collect();
        assert_eq!(semis.len(), 1, "只有 if 条件行行尾补分号，{{ 与 }} 行尾不补");
        let brace_idx = toks.iter().position(|t| t.kind == TokenKind::LBrace).expect("有左大括号");
        assert!(matches!(toks[brace_idx + 1].kind, TokenKind::RBrace), "块内不应插入分号");
    }

    /// 显式写出的分号不会被再次补全（补全只发生在行尾无分号时）。
    #[test]
    fn 显式分号不重复补全() {
        let toks = tokenize("x = 1;\ny = 2\n").expect("不应报错");
        let n = toks.iter().filter(|t| t.kind == TokenKind::Semi).count();
        assert_eq!(n, 2, "第一行显式 1 个，第二行补全 1 个，共 2 个");
    }

    // ---------- M4 补齐：trit 关键字 + 多进制字面量 ----------

    /// trit 类型关键字与 zero 字面量：`trit` → TypeKw(Trit)，`zero` → Zero。
    /// `zero1`/`tritx` 是普通标识符（保留字只精确匹配整词）。
    #[test]
    fn trit与zero关键字() {
        let toks = tokenize("var t: trit = zero\n").expect("不应报错");
        let ty = toks.iter().find(|t| matches!(t.kind, TokenKind::TypeKw(TyKw::Trit)));
        assert!(ty.is_some(), "trit 应识别为类型关键字");
        let z = toks.iter().find(|t| t.kind == TokenKind::Zero);
        assert!(z.is_some(), "zero 应识别为保留字");
        // 整词边界：zero1 是标识符不是 Zero
        let toks2 = tokenize("zero1\n").expect("不应报错");
        assert!(matches!(&toks2[0].kind, TokenKind::Ident(s) if s == "zero1"));
        // tritx 是标识符不是类型
        let toks3 = tokenize("tritx\n").expect("不应报错");
        assert!(matches!(&toks3[0].kind, TokenKind::Ident(s) if s == "tritx"));
        // trit 的 as_str 正确
        assert_eq!(TyKw::Trit.as_str(), "trit");
        // trit 不是整数/浮点/宽类型
        assert!(!TyKw::Trit.is_int());
        assert!(!TyKw::Trit.is_float());
        assert!(!TyKw::Trit.is_wide());
    }

    /// 多进制整数字面量（M4 补齐）：0x 十六进制 / 0b 二进制 / 0o 八进制 / 0t 三进制。
    #[test]
    fn 多进制整数字面量() {
        // 十六进制：0xFF = 255（大小写前缀均可）
        let toks = tokenize("0xFF\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(255)));
        let toks = tokenize("0x2a\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(42)));
        let toks = tokenize("0X1F\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(31)));
        // 二进制：0b1010 = 10
        let toks = tokenize("0b1010\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(10)));
        let toks = tokenize("0B11111111\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(255)));
        // 八进制：0o17 = 15
        let toks = tokenize("0o17\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(15)));
        let toks = tokenize("0O10\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(8)));
        // 三进制：0t210 = 2*9 + 1*3 + 0 = 21（数论常用进制）
        let toks = tokenize("0t210\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(21)));
        let toks = tokenize("0T12\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(5)));
        // 十进制行为不变：0 / 42 / 0.5（0 后跟非进制字母）
        let toks = tokenize("0\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(0)));
        let toks = tokenize("42\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(42)));
        let toks = tokenize("0.5\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Float(0.5)));
        // 0 后跟其他字母：0a → 整数 0 + 标识符 a（不是进制前缀）
        let toks = tokenize("0a\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(0)));
    }

    /// 多进制字面量非法输入防御：空进制（0x 后无数字）/ 越进制数字（0b2）/ 非法字符。
    /// 全部回退为 0（与十进制 parse 失败 unwrap_or(0) 的防御约定一致）。
    #[test]
    fn 多进制字面量非法回退零() {
        // 0x 后无数字 → 0
        let toks = tokenize("0x\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(0)));
        // 二进制中出现 2 → 整体回退 0（0b2 解析失败）
        let toks = tokenize("0b2\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(0)));
        // 八进制中出现 8 → 回退 0
        let toks = tokenize("0o8\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(0)));
        // 三进制中出现 3 → 回退 0
        let toks = tokenize("0t3\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(0)));
        // 十六进制中出现 g（不在 a-f）→ 回退 0
        let toks = tokenize("0xg\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(0)));
        // 溢出：0xFFFFFFFFFFFFFFFF（> i64::MAX）→ 回退 0（防御）
        let toks = tokenize("0xffffffffffffffff\n").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(0)));
    }

    // ---------- 注释 ----------

    /// 行注释 `//` 不产生任何 token。
    #[test]
    fn 行注释不产生token() {
        let toks = tokenize("var x = 1 // 注释\nvar y").expect("不应报错");
        assert_eq!(toks.len(), 8, "注释不应产生 token（8 = var x = 1 分号 var y Eof）");
        assert!(!toks.iter().any(|t| matches!(t.kind, TokenKind::Str(_))), "注释文本不应成为字符串");
        assert!(toks.iter().any(|t| t.kind == TokenKind::Semi), "注释前的语句行尾仍补分号");
    }

    /// 块注释 `/* */`（可跨行）不产生任何 token。
    #[test]
    fn 块注释不产生token() {
        let toks = tokenize("/* 块\n注释 */ x\n").expect("不应报错");
        assert_eq!(toks.len(), 3, "只有标识符 x、补出的分号和 Eof");
        assert!(matches!(&toks[0].kind, TokenKind::Ident(s) if s == "x"));
        assert!(matches!(toks[1].kind, TokenKind::Semi));
    }

    /// 块注释未闭合 → 报错，消息含「未闭合」。
    #[test]
    fn 块注释未闭合报错() {
        let err = tokenize("var x /* 未闭合").expect_err("块注释未闭合应报错");
        assert!(err.message.contains("未闭合"), "错误消息：{}", err.message);
    }

    // ---------- 字符串 ----------

    /// 字符串转义 `\n \t \r \\ \" \' \0` 全部正确解码。
    #[test]
    fn 字符串转义正确解码() {
        let toks = tokenize("\"a\\nb\\tc\\\\d\\\"e\\'f\\0g\\r\"").expect("不应报错");
        assert!(
            matches!(&toks[0].kind, TokenKind::Str(s) if s == "a\nb\tc\\d\"e'f\0g\r"),
            "转义解码结果：{:?}",
            toks[0].kind
        );
    }

    /// 未知转义序列（如 `\q`）→ 报错，消息含「未知转义」。
    #[test]
    fn 字符串未知转义报错() {
        let err = tokenize("\"\\q\"").expect_err("未知转义应报错");
        assert!(err.message.contains("未知转义"), "错误消息：{}", err.message);
    }

    /// 字符串未闭合 → 报错，消息含「未闭合」。
    #[test]
    fn 字符串未闭合报错() {
        let err = tokenize("\"abc").expect_err("未闭合字符串应报错");
        assert!(err.message.contains("未闭合"), "错误消息：{}", err.message);
    }

    // ---------- 字符字面量 ----------

    /// 字符字面量 `'a'` / `'\n'` / `'\''`（转义）识别为 CharLit。
    #[test]
    fn 字符字面量识别() {
        let toks = tokenize("'a' '\\n' '\\''").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::CharLit('a')));
        assert!(matches!(toks[1].kind, TokenKind::CharLit('\n')));
        assert!(matches!(toks[2].kind, TokenKind::CharLit('\'')));
    }

    /// 多字符字面量 `'ab'` → 报错，消息含「只能包含一个字符」。
    #[test]
    fn 字符字面量多字符报错() {
        let err = tokenize("'ab'").expect_err("多字符字面量应报错");
        assert!(err.message.contains("只能包含一个字符"), "错误消息：{}", err.message);
    }

    // ---------- 数字 ----------

    /// 整数与含小数点的浮点（`2.5`）。
    #[test]
    fn 整数与浮点识别() {
        let toks = tokenize("1 2.5 100 0").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(1)));
        assert!(matches!(toks[1].kind, TokenKind::Float(f) if f == 2.5));
        assert!(matches!(toks[2].kind, TokenKind::Int(100)));
        assert!(matches!(toks[3].kind, TokenKind::Int(0)));
    }

    /// 指数形式浮点：1e5 / 1.5e-3 / 2E3。
    #[test]
    fn 指数浮点识别() {
        for (src, expected) in [("1e5", 100000.0), ("1.5e-3", 0.0015), ("2E3", 2000.0)] {
            let toks = tokenize(src).expect("不应报错");
            let got = match toks[0].kind {
                TokenKind::Float(f) => f,
                _ => panic!("源码 {src:?} 应为浮点"),
            };
            assert!((got - expected).abs() < 1e-9, "源码 {src:?} 解析为 {got}，期望 {expected}");
        }
    }

    /// `1..10` 中 `..` 是范围运算符，不被误认为小数点（scan_number 只在
    /// 小数点后跟数字时才按浮点处理）。
    #[test]
    fn 范围运算符不被误认为小数点() {
        let toks = tokenize("1..10").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::Int(1)));
        assert!(matches!(toks[1].kind, TokenKind::DotDot));
        assert!(matches!(toks[2].kind, TokenKind::Int(10)));
        // 成员访问 `a.b` 与范围 `1..2` 共存，各自正确
        let toks = tokenize("a.b 1..2").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::Dot));
        assert!(matches!(toks[3].kind, TokenKind::Int(1)));
        assert!(matches!(toks[4].kind, TokenKind::DotDot));
    }

    // ---------- 位置与 Eof ----------

    /// Span 的 line/col 从 1 开始；补出的分号位于下一行行首。
    #[test]
    fn 位置信息从1开始且正确() {
        let toks = tokenize("var x\n  if y").expect("不应报错");
        assert_eq!((toks[0].span.line, toks[0].span.col), (1, 1), "var");
        assert_eq!((toks[1].span.line, toks[1].span.col), (1, 5), "x");
        assert_eq!((toks[2].span.line, toks[2].span.col), (2, 1), "补出的分号");
        assert_eq!((toks[3].span.line, toks[3].span.col), (2, 3), "if");
        assert_eq!((toks[4].span.line, toks[4].span.col), (2, 6), "y");
        assert_eq!((toks[5].span.line, toks[5].span.col), (2, 7), "Eof 在文件末尾");
    }

    /// 空源码只产生一个 Eof；任何 token 流末尾恒有 Eof。
    #[test]
    fn 空源码仅含eof且末尾恒有eof() {
        let toks = tokenize("").expect("空源码不应报错");
        assert_eq!(toks.len(), 1, "空源码只有 Eof");
        assert!(matches!(toks[0].kind, TokenKind::Eof));
        assert_eq!((toks[0].span.line, toks[0].span.col), (1, 1));
        let toks = tokenize("x = 1").expect("不应报错");
        assert!(matches!(toks.last().expect("非空流").kind, TokenKind::Eof), "Eof 恒在末尾");
    }

    // ---------- 关键字与标识符 ----------

    /// 控制流/OOP/字面量关键字逐一识别（func..false 共 20 个；class/this/static
    /// 已废弃为普通标识符，M2.1.8 struct 取代 class）。
    #[test]
    fn 关键字全部识别() {
        let src = "func var const if else while for in return switch case default when import as struct extends namespace pub using true false";
        let toks = tokenize(src).expect("不应报错");
        let expected = [
            TokenKind::Func,
            TokenKind::Var,
            TokenKind::Const,
            TokenKind::If,
            TokenKind::Else,
            TokenKind::While,
            TokenKind::For,
            TokenKind::In,
            TokenKind::Return,
            TokenKind::Switch,
            TokenKind::Case,
            TokenKind::Default,
            TokenKind::When,
            TokenKind::Import,
            TokenKind::As,
            TokenKind::Struct,
            TokenKind::Extends,
            TokenKind::Namespace,
            TokenKind::Pub,
            TokenKind::Using,
            TokenKind::True,
            TokenKind::False,
        ];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(toks[i].kind, *want, "第 {i} 个关键字");
        }
        assert!(matches!(toks[expected.len()].kind, TokenKind::Eof));
    }

    /// 类型关键字 i8..table 逐一识别为 TypeKw(TyKw::*)（共 19 个）。
    #[test]
    fn 类型关键字识别() {
        let src = "i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 bool char string void code num text misc table";
        let toks = tokenize(src).expect("不应报错");
        let expected = [
            TyKw::I8,
            TyKw::I16,
            TyKw::I32,
            TyKw::I64,
            TyKw::U8,
            TyKw::U16,
            TyKw::U32,
            TyKw::U64,
            TyKw::F32,
            TyKw::F64,
            TyKw::Bool,
            TyKw::Char,
            TyKw::Str,
            TyKw::Void,
            TyKw::Code,
            TyKw::Num,
            TyKw::Text,
            TyKw::Misc,
            TyKw::Table,
        ];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(toks[i].kind, TokenKind::TypeKw(*want), "第 {i} 个类型关键字");
        }
        assert!(matches!(toks[expected.len()].kind, TokenKind::Eof));
    }

    /// 含关键字前缀/后缀的标识符仍是标识符；完整拼写才是关键字。
    #[test]
    fn 标识符与关键字区分() {
        let toks = tokenize("variable ifx _x i32x x1").expect("不应报错");
        for t in &toks[..5] {
            assert!(matches!(t.kind, TokenKind::Ident(_)), "应识别为标识符：{t:?}");
        }
        assert!(matches!(&toks[0].kind, TokenKind::Ident(s) if s == "variable"));
        // 对照：完整拼写仍是关键字 / 类型关键字
        let toks = tokenize("if i32").expect("不应报错");
        assert!(matches!(toks[0].kind, TokenKind::If));
        assert!(matches!(toks[1].kind, TokenKind::TypeKw(TyKw::I32)));
    }

    // ---------- 符号 ----------

    /// 双字符符号（== != <= >= && || .. ->）与单字符符号逐一定位。
    #[test]
    fn 运算符与符号识别() {
        let toks = tokenize("== != <= >= && || .. ->").expect("不应报错");
        let want2 = [
            TokenKind::EqEq,
            TokenKind::NotEq,
            TokenKind::Le,
            TokenKind::Ge,
            TokenKind::AndAnd,
            TokenKind::OrOr,
            TokenKind::DotDot,
            TokenKind::Arrow,
        ];
        for (i, w) in want2.iter().enumerate() {
            assert_eq!(toks[i].kind, *w, "第 {i} 个双字符符号");
        }
        assert!(matches!(toks[want2.len()].kind, TokenKind::Eof));
        // 单字符符号（含显式分号）。
        // 注：`%` 与 `=` 之间加空格隔开——M4 起 `%=` 是复合赋值 PercentEq 单 token，
        // 本测试只验证「单字符符号」的逐个识别，故避免 `%=` 相邻被吞并。
        let toks = tokenize("(){}[],:;.+*-/% =<>!").expect("不应报错");
        let want1 = [
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::LBrace,
            TokenKind::RBrace,
            TokenKind::LBracket,
            TokenKind::RBracket,
            TokenKind::Comma,
            TokenKind::Colon,
            TokenKind::Semi,
            TokenKind::Dot,
            TokenKind::Plus,
            TokenKind::Star,
            TokenKind::Minus,
            TokenKind::Slash,
            TokenKind::Percent,
            TokenKind::Eq,
            TokenKind::Lt,
            TokenKind::Gt,
            TokenKind::Bang,
        ];
        for (i, w) in want1.iter().enumerate() {
            assert_eq!(toks[i].kind, *w, "第 {i} 个单字符符号");
        }
        assert!(matches!(toks[want1.len()].kind, TokenKind::Eof));
    }

    /// M4 运算符扩展识别：复合赋值（含三字符 `<<=`/`>>=`）、位运算、移位、自增自减、三目问号。
    #[test]
    fn m4运算符扩展识别() {
        // 复合赋值：`+=` 等（`<<=`/`>>=` 是三字符，验证先吃 `<<` 再特判 `=`）
        let toks = tokenize("x += 1").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::PlusEq), "复合加等应识别为 PlusEq");
        let toks = tokenize("x <<= 2").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::ShlEq), "三字符 <<= 应识别为 ShlEq");
        let toks = tokenize("x >>= 2").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::ShrEq), "三字符 >>= 应识别为 ShrEq");
        let toks = tokenize("a -= b *= c /= d %= e").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::MinusEq));
        assert!(matches!(toks[3].kind, TokenKind::StarEq));
        assert!(matches!(toks[5].kind, TokenKind::SlashEq));
        assert!(matches!(toks[7].kind, TokenKind::PercentEq));
        let toks = tokenize("a &= b |= c ^= d").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::AmpEq));
        assert!(matches!(toks[3].kind, TokenKind::PipeEq));
        assert!(matches!(toks[5].kind, TokenKind::CaretEq));
        // 纯移位：`<<` / `>>`（不是复合赋值）
        let toks = tokenize("a << b >> c").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::Shl));
        assert!(matches!(toks[3].kind, TokenKind::Shr));
        // 位运算单字符：`&` / `|` / `^`
        let toks = tokenize("a & b | c ^ d").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::Amp));
        assert!(matches!(toks[3].kind, TokenKind::Pipe));
        assert!(matches!(toks[5].kind, TokenKind::Caret));
        // 三目：`?` 与 `:`（`:` 仍是 Colon）
        let toks = tokenize("a ? b : c").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::Question));
        assert!(matches!(toks[3].kind, TokenKind::Colon));
        // 自增自减：`++` / `--`
        let toks = tokenize("i++ j--").expect("不应报错");
        assert!(matches!(toks[1].kind, TokenKind::Inc));
        assert!(matches!(toks[3].kind, TokenKind::Dec));
    }

    /// 无法识别的字符（如 `@`）→ 报错，消息含「无法识别」。
    #[test]
    fn 无法识别字符报错() {
        let err = tokenize("@x").expect_err("无法识别的字符应报错");
        assert!(err.message.contains("无法识别"), "错误消息：{}", err.message);
    }
}
