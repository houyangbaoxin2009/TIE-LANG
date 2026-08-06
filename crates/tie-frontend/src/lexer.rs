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
    Import,
    As,
    True,
    False,
    /// 类型关键字：i8..u64/f32/f64/bool/char/string/void/code/num/text/misc/table
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

    /// 数字字面量：十进制整数或浮点数（含小数点与指数）。
    fn scan_number(&mut self) -> Token {
        let (line, col) = (self.line, self.col);
        let mut text = String::new();
        let mut is_float = false;
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
        let kind = if is_float {
            TokenKind::Float(text.parse::<f64>().unwrap_or(0.0))
        } else {
            TokenKind::Int(text.parse::<i64>().unwrap_or(0))
        };
        Token::new(kind, line, col)
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
            "import" => TokenKind::Import,
            "as" => TokenKind::As,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
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

    /// 符号：单字符与双字符运算符。
    fn scan_symbol(&mut self) -> Result<Token, LexError> {
        let (line, col) = (self.line, self.col);
        let c = self.consume_char().unwrap();
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
