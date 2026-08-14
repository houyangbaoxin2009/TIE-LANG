//! tie 编译协调配置：tie 数据文件格式配置解析。
//!
//! Harbor M3 阶段二（协调统筹增强）：`tie` 支持通过配置文件开启
//! 「多线程分片编译 + 缓存池」。配置文件是 tie 语言的数据交换文件
//! （以 `type tie<data>` 声明角色，新文件类型声明系统），正文是一个
//! 表字面量，键为字符串：
//!
//! ```tie
//! type tie<data>
//! // 协调统筹配置（默认全关闭；`advanced.enabled = true` 开启分片编译）
//! [
//!   "advanced": [
//!     "enabled": false,   // 多线程分片编译总开关
//!     "threads": 0,       // 并行线程数（0 = 按 CPU 核数自动）
//!   ],
//!   "cache": [
//!     "size": 268435456,  // 缓存池容量上限（字节；默认 256MB）
//!     "storage": "memory",// 存储技术：memory（进程内）/ file（磁盘目录）
//!     "path": ".tie-cache", // file 存储时的缓存目录
//!   ],
//! ]
//! ```
//!
//! 声明行 `type tie<data>` 是**真实语法 token**（不是注释）：解析时先跳过
//! 开头的文件类型声明 token 序列（`Ident("type") Ident("tie") [Lt Ident(子类型)
//! Gt] [Semi?]`，守护式——仅当开头恰好是 `type tie` 才消费），再解析正文表。
//! 无声明行的普通表配置（直接 `[ ... ]` 开头）原样解析，完全兼容。
//!
//! 为什么自写解析器而非复用 [tie_frontend::parser::parse_program]：
//! 1. parser 顶层只允许函数/import/类/命名空间声明，表字面量是表达式，
//!    无法直接作为顶层语句；
//! 2. 语义层（analyze）目前拒绝字符串 id 表（「留待 M3」）——配置恰好
//!    就是字符串键表。
//!
//! 因此这里复用 [tie_frontend::lexer::tokenize]（词法完全一致，注释自动
//! 跳过），对 token 流做表字面量的递归下降解析，绕过语义层。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tie_frontend::lexer::{tokenize, Token, TokenKind};

/// 解析出的协调统筹配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// 多线程分片编译配置
    pub advanced: Advanced,
    /// 缓存池配置
    pub cache: Cache,
}

/// 多线程分片编译配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advanced {
    /// 总开关（默认关闭；开启后按文件分片并行编译）
    pub enabled: bool,
    /// 并行线程数（0 = 自动，按 CPU 核数）
    pub threads: usize,
}

/// 缓存池配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    /// 容量上限（字节；默认 256MB = 268435456）
    pub size_bytes: u64,
    /// 存储技术
    pub storage: Storage,
    /// file 存储时的缓存目录（memory 存储也用于中间文件临时目录）
    pub path: PathBuf,
}

/// 缓存池存储技术。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// 进程内内存（HashMap，快；跨进程不共享）
    Memory,
    /// 磁盘目录（跨进程可复用，缓存在 [Cache::path] 下）
    File,
}

impl Default for Config {
    /// 默认配置：功能全部关闭，保证 `tie` 原有单文件行为不变。
    fn default() -> Self {
        Config {
            advanced: Advanced {
                enabled: false,
                threads: 0, // 0 = 自动
            },
            cache: Cache {
                size_bytes: 256 * 1024 * 1024, // 256MB
                storage: Storage::Memory,
                path: PathBuf::from(".tie-cache"),
            },
        }
    }
}

/// 配置文件默认名（当前目录）。
pub const DEFAULT_CONFIG_FILE: &str = "tie.config";

/// 从默认位置加载配置；文件不存在时返回默认配置（不报错）。
///
/// 查找顺序：显式指定 > 当前目录 `tie.config` > 默认（全关闭）。
pub fn load(explicit: Option<&Path>) -> Result<Config, String> {
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => {
            let p = PathBuf::from(DEFAULT_CONFIG_FILE);
            if !p.is_file() {
                return Ok(Config::default());
            }
            p
        }
    };
    let source = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取配置文件 {} 失败: {e}", path.display()))?;
    parse(&source).map_err(|e| format!("配置文件 {}: {e}", path.display()))
}

/// 解析 tie 数据文件配置文本。
///
/// 只关心表字面量：顶层 `[ ... ]`，元素为 `"key": value`，值可为
/// 整数/浮点/字符串/布尔/嵌套表。其余 token（注释等）由 lexer 自动跳过。
///
/// 若文本以 `type tie` / `type tie<data>` 开头（新文件类型声明系统，
/// 真实 token 而非注释），先跳过声明 token 序列再解析表；无声明的普通
/// 表配置不受影响。
pub fn parse(source: &str) -> Result<Config, String> {
    let tokens = tokenize(source).map_err(|e| format!("词法错误: {e}"))?;
    // 空输入（无有效 token，仅 EOF 哨兵）：返回全默认配置（等价于没有配置文件）
    if tokens.is_empty() || matches!(tokens[0].kind, TokenKind::Eof) {
        return Ok(Config::default());
    }
    let mut cursor = Cursor { tokens: &tokens, pos: 0 };
    // 跳过可选的文件类型声明（`type tie` / `type tie<data>`）
    skip_type_declaration(&mut cursor);
    let root = parse_table(&mut cursor)?;
    // 顶层表按 "advanced" / "cache" 键取值
    let mut cfg = Config::default();
    if let Some(Value::Table(adv)) = root.get("advanced") {
        if let Some(Value::Bool(b)) = adv.get("enabled") {
            cfg.advanced.enabled = *b;
        }
        if let Some(Value::Int(n)) = adv.get("threads") {
            if *n < 0 {
                return Err("advanced.threads 不能为负数".into());
            }
            cfg.advanced.threads = *n as usize;
        }
    }
    if let Some(Value::Table(cache)) = root.get("cache") {
        if let Some(Value::Int(n)) = cache.get("size") {
            if *n < 0 {
                return Err("cache.size 不能为负数".into());
            }
            cfg.cache.size_bytes = *n as u64;
        }
        if let Some(Value::Str(s)) = cache.get("storage") {
            cfg.cache.storage = match s.as_str() {
                "memory" => Storage::Memory,
                "file" => Storage::File,
                other => return Err(format!("cache.storage 仅支持 memory/file，实际: {other}")),
            };
        }
        if let Some(Value::Str(p)) = cache.get("path") {
            cfg.cache.path = PathBuf::from(p);
        }
    }
    Ok(cfg)
}

/// token 游标（递归下降解析用）。
struct Cursor<'a> {
    tokens: &'a [Token],
    pos: usize,
}

/// 配置值：表字面量元素的标量或嵌套表。
enum Value {
    Int(i64),
    Str(String),
    Bool(bool),
    Table(HashMap<String, Value>),
}

/// 解析一个表字面量：`[ "k": v, "k2": [ ... ] ]`。
///
/// 表元素间以逗号（列）或分号（行）分隔，允许尾随分隔符。
fn parse_table(cursor: &mut Cursor) -> Result<HashMap<String, Value>, String> {
    expect(cursor, TokenKind::LBracket, "期望 '[' 开始表字面量".to_string())?;
    let mut map = HashMap::new();
    loop {
        skip_separators(cursor);
        // 空表或表结束
        if peek(cursor).is_some_and(|t| t == TokenKind::RBracket) {
            cursor.pos += 1;
            break;
        }
        // 键：字符串（TableId::Str）；也兼容裸标识符
        let key = match next(cursor).ok_or("表元素缺少键")? {
            TokenKind::Str(s) => s,
            TokenKind::Ident(s) => s,
            other => return Err(format!("表键应为字符串，实际: {other:?}")),
        };
        expect(cursor, TokenKind::Colon, format!("键 '{key}' 后缺少 ':'"))?;
        let value = parse_value(cursor)?;
        map.insert(key, value);
    }
    Ok(map)
}

/// 解析一个值：标量字面量或嵌套表。
fn parse_value(cursor: &mut Cursor) -> Result<Value, String> {
    let tok = next(cursor).ok_or("值缺失")?;
    match tok {
        TokenKind::Int(n) => Ok(Value::Int(n)),
        TokenKind::Str(s) => Ok(Value::Str(s)),
        TokenKind::True => Ok(Value::Bool(true)),
        TokenKind::False => Ok(Value::Bool(false)),
        // 负数：词法器把 `-1` 拆成 Minus + Int(1)，此处拼回负整数
        TokenKind::Minus => {
            let n = next(cursor).ok_or("负号后缺少数字")?;
            match n {
                TokenKind::Int(n) => Ok(Value::Int(-n)),
                other => Err(format!("负号后应为整数，实际: {other:?}")),
            }
        }
        TokenKind::LBracket => {
            cursor.pos -= 1; // 回退，交给 parse_table 消费 '['
            Ok(Value::Table(parse_table(cursor)?))
        }
        other => Err(format!("配置值仅支持字面量/嵌套表，实际: {other:?}")),
    }
}

/// 跳过逗号与分号（表元素分隔符），容忍尾随分隔符。
fn skip_separators(cursor: &mut Cursor) {
    while let Some(t) = peek(cursor) {
        if matches!(t, TokenKind::Comma | TokenKind::Semi) {
            cursor.pos += 1;
        } else {
            break;
        }
    }
}

/// 跳过开头的文件类型声明 token 序列（`type tie` / `type tie<data>`）。
///
/// 新文件类型声明系统（tie-prep）：文件以 `type tie` / `type tie<X>` 声明
/// 角色（X ∈ {type, script, data, ui, class, logic, port, db}），这是真实
/// 语法 token（旧 `// tie:data` 注释指令已完全移除）。配置文件作为数据文件
/// 通常以 `type tie<data>` 开头，此处识别并整体跳过：
/// `Ident("type") Ident("tie") [Lt Ident(子类型) Gt] [Semi?]`
///
/// **守护式**：仅当 token 流开头恰好是 `Ident("type")` 且紧跟 `Ident("tie")`
/// 时才消费；否则不消费（普通表配置直接 `[ ... ]` 开头，原样交给表解析）。
/// `Semi` 可选（ASI 自动补全或显式写出），子类型 `Lt Ident Gt` 可选。
fn skip_type_declaration(cursor: &mut Cursor) {
    // 守护：开头必须是 Ident("type") 紧跟 Ident("tie")
    let is_decl = matches!(peek(cursor), Some(TokenKind::Ident(ref s)) if s == "type")
        && matches!(peek_at(cursor, 1), Some(TokenKind::Ident(ref s)) if s == "tie");
    if !is_decl {
        return;
    }
    // 消费 Ident("type") Ident("tie")
    cursor.pos += 2;
    // 可选子类型：`Lt Ident(任意) Gt`（如 `type tie<data>`）
    if matches!(peek(cursor), Some(TokenKind::Lt))
        && matches!(peek_at(cursor, 1), Some(TokenKind::Ident(_)))
        && matches!(peek_at(cursor, 2), Some(TokenKind::Gt))
    {
        cursor.pos += 3;
    }
    // 可选分号（ASI 自动补全或显式写出）
    if matches!(peek(cursor), Some(TokenKind::Semi)) {
        cursor.pos += 1;
    }
}

/// 取当前 token 种类（不前进）。
fn peek(cursor: &Cursor) -> Option<TokenKind> {
    cursor.tokens.get(cursor.pos).map(|t| t.kind.clone())
}

/// 取距当前位置偏移 n 的 token 种类（不前进）。
fn peek_at(cursor: &Cursor, n: usize) -> Option<TokenKind> {
    cursor.tokens.get(cursor.pos + n).map(|t| t.kind.clone())
}

/// 取当前 token 种类并前进。
fn next(cursor: &mut Cursor) -> Option<TokenKind> {
    let t = cursor.tokens.get(cursor.pos).map(|t| t.kind.clone());
    if t.is_some() {
        cursor.pos += 1;
    }
    t
}

/// 断言当前 token 匹配期望，匹配则前进。
fn expect(cursor: &mut Cursor, want: TokenKind, msg: String) -> Result<(), String> {
    let got = next(cursor).ok_or_else(|| format!("{msg}（输入已结束）"))?;
    if got == want {
        Ok(())
    } else {
        Err(format!("{msg}，实际: {got:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空配置返回默认() {
        let cfg = parse("").unwrap();
        assert!(!cfg.advanced.enabled);
        assert_eq!(cfg.advanced.threads, 0);
        assert_eq!(cfg.cache.size_bytes, 256 * 1024 * 1024);
        assert_eq!(cfg.cache.storage, Storage::Memory);
    }

    #[test]
    fn 完整配置解析() {
        let src = r#"
type tie<data>
// 协调统筹配置
[
  "advanced": [
    "enabled": true,
    "threads": 4,
  ],
  "cache": [
    "size": 1048576,
    "storage": "file",
    "path": ".cache",
  ],
]
"#;
        let cfg = parse(src).unwrap();
        assert!(cfg.advanced.enabled);
        assert_eq!(cfg.advanced.threads, 4);
        assert_eq!(cfg.cache.size_bytes, 1048576);
        assert_eq!(cfg.cache.storage, Storage::File);
        assert_eq!(cfg.cache.path, PathBuf::from(".cache"));
    }

    #[test]
    fn 带类型声明行解析配置() {
        // `type tie<data>` 声明行 + 表 == 无声明的同名表（声明被跳过）
        let with_decl = r#"
type tie<data>
[ "advanced": [ "enabled": true ] ]
"#;
        let without_decl = r#"
[ "advanced": [ "enabled": true ] ]
"#;
        assert_eq!(parse(with_decl).unwrap(), parse(without_decl).unwrap());
        assert!(parse(with_decl).unwrap().advanced.enabled);
    }

    #[test]
    fn 类型声明不带子类型解析配置() {
        // `type tie`（无 `<子类型>`）同样应跳过，正文表正常解析
        let src = "type tie\n[ \"cache\": [ \"size\": 1024 ] ]";
        let cfg = parse(src).unwrap();
        assert_eq!(cfg.cache.size_bytes, 1024);
        assert_eq!(cfg.cache.storage, Storage::Memory);
    }

    #[test]
    fn 无类型声明的普通配置不变() {
        // 无声明行：守护式跳过不消费任何 token，普通表配置原样解析
        let src = "[ \"cache\": [ \"size\": 2048 ] ]";
        let cfg = parse(src).unwrap();
        assert_eq!(cfg.cache.size_bytes, 2048);
    }

    #[test]
    fn 缺省键走默认() {
        let cfg = parse(r#"[ "advanced": [ "enabled": true ] ]"#).unwrap();
        assert!(cfg.advanced.enabled);
        // 未写的字段保持默认
        assert_eq!(cfg.cache.storage, Storage::Memory);
        assert_eq!(cfg.cache.size_bytes, 256 * 1024 * 1024);
    }

    #[test]
    fn 非法存储技术报错() {
        let err = parse(r#"[ "cache": [ "storage": "disk" ] ]"#).unwrap_err();
        assert!(err.contains("仅支持 memory/file"), "错误应提示合法值: {err}");
    }

    #[test]
    fn 负线程数报错() {
        let err = parse(r#"[ "advanced": [ "threads": -1 ] ]"#).unwrap_err();
        assert!(err.contains("不能为负数"), "错误应提示负数: {err}");
    }

    #[test]
    fn 注释被词法器跳过() {
        // // 与 /* */ 注释不应干扰解析
        let src = "/* 头注释 */ [ // 行注释\n \"cache\": [ \"size\": 1 ] /* 尾注释 */ ]";
        let cfg = parse(src).unwrap();
        assert_eq!(cfg.cache.size_bytes, 1);
    }
}
