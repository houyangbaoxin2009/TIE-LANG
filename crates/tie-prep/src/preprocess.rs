//! 预处理核心逻辑：清理代码 + 识别文件类型 + 角色判定。
//!
//! Harbor M3 自举：核心逻辑（头部提取/角色判定/正文重建）由 tie 语言编写
//! （prep/core.tie，include_str! 内嵌），本文件是解释执行壳——
//! 1. 字节规范化（去 BOM、CRLF→LF）仍留在壳层（tie 字符串字面量无法表达
//!    BOM 字符，且字节规范化属壳层职责）；
//! 2. 通过 tie-interp 的 eval/eval_call 注册并调用 `prep::process`；
//! 3. 解析模块返回的协议文本，还原 [PreprocessResult]。
//!
//! 协议文本（与 prep/core.tie 约定）：
//! ```text
//! ROLE:logic
//! HEADERS:2
//! H:opt=2
//! H:target=win
//! BODY:12
//! <正文恰好 12 字节>
//! ```
//! 头部区逐行固定，正文按 BODY 声明的字节数精确截取——正文可含任意内容
//! （含换行），不会破坏协议。

use std::fmt;

/// prep/core.tie 模块源码（编译期内嵌，发布无需额外文件）。
const PREP_MODULE: &str = include_str!("../../../prep/core.tie");

/// 模块入口函数全名（命名空间 prep 下的 process）。
const PREP_ENTRY: &str = "prep::process";

/// 头部指令（文件角色声明）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// 指令文本：`// tie:logic` 的 `logic`；`// tie:target=win-x64` 的 `target=win-x64`
    pub raw: String,
}

impl Header {
    /// 头部角色关键字（第一个词，如 `logic` / `ui` / `db` / `data`）。
    pub fn kind(&self) -> &str {
        self.raw
            .split(['=', ' '])
            .next()
            .unwrap_or("")
            .trim()
    }

    /// 是否为 `key=value` 形式的选项，返回 (key, value)。
    pub fn as_option(&self) -> Option<(&str, &str)> {
        let mut it = self.raw.splitn(2, '=');
        let key = it.next()?.trim();
        let val = it.next()?.trim();
        if key.is_empty() || val.is_empty() { None } else { Some((key, val)) }
    }
}

/// 文件角色：对应 `// tie:` 指令声明，决定转交哪个工具链。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRole {
    /// 逻辑代码（默认，编译为可执行文件，需 main）
    Logic,
    /// 界面文件（转交 UI 工具链，后续版本）
    Ui,
    /// 数据库文件（转交数据库工具链，后续版本）
    Db,
    /// 数据交换文件（转交数据解析工具，数据交换格式）
    Data,
    /// 库文件（转交编译器，编译为库，不要求 main）
    Library,
}

impl FileRole {
    /// 角色名（用于消息展示）。
    pub fn as_str(self) -> &'static str {
        match self {
            FileRole::Logic => "logic",
            FileRole::Ui => "ui",
            FileRole::Db => "db",
            FileRole::Data => "data",
            FileRole::Library => "library",
        }
    }
}

impl fmt::Display for FileRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 预处理结果。
#[derive(Debug, Clone)]
pub struct PreprocessResult {
    /// 清理后的正文源码（无 BOM、统一 `\n` 换行、已剥离头部行）
    pub cleaned_source: String,
    /// 解析出的头部指令（按出现顺序）
    pub headers: Vec<Header>,
    /// 文件角色
    pub role: FileRole,
}

/// 角色名 → 枚举（协议文本解析用）。
fn role_from_str(s: &str) -> FileRole {
    match s.trim() {
        "ui" => FileRole::Ui,
        "db" => FileRole::Db,
        "data" => FileRole::Data,
        "library" => FileRole::Library,
        _ => FileRole::Logic,
    }
}

/// 预处理入口：原始源码 → 清理后的正文 + 头部信息 + 角色。
///
/// 自举实现：字节规范化（去 BOM、CRLF→LF）后，解释执行 prep/core.tie 的
/// `prep::process`（头部提取/角色判定/正文重建全在 tie 模块内），
/// 再解析模块返回的协议文本。模块注册与调用失败 → panic 并给出可读信息
/// （模块内嵌于二进制，失败属于自举链破损，应尽早暴露）。
pub fn preprocess(source: &str) -> PreprocessResult {
    // 1. 字节规范化（壳层职责）：去 BOM、CRLF 归一
    let source = source.trim_start_matches('\u{FEFF}');
    let source = source.replace("\r\n", "\n");

    // 2. 解释执行 tie 模块：注册 prep::process 并传入源码
    let text = run_module(PREP_MODULE, PREP_ENTRY, &source)
        .unwrap_or_else(|e| panic!("预处理模块执行失败: {e}"));

    // 3. 解析协议文本（ROLE / HEADERS:n / H:raw * n / BODY:m / 正文 m 字节）
    parse_protocol(&text)
}

/// 解释执行任意 tie 模块（Harbor M3 可扩展性证明）。
///
/// 加载模块源码（eval 注册顶层/命名空间函数），以字符串值直传源码调用
/// 其入口函数（约定 `func process(src: string) -> string`），返回结果文本。
/// 语义与预处理自举一致：模块内嵌逻辑完全用 tie 语言编写，Rust 侧零逻辑。
///
/// 扩展方式：新增转换器/处理器 = 新增一个 tie 模块文件 + 调用本函数，
/// 无需修改 Rust（`tie-prep --module <file.tie>` 即命令行挂载入口）。
pub fn run_module(module_source: &str, entry: &str, source: &str) -> Result<String, String> {
    let mut session = tie_interp::Session::new();
    session.eval(module_source)?;
    session.eval_call(entry, source)
}

/// 解析协议文本（与 prep/core.tie 的 process 输出约定一致）。
///
/// 头部区固定 3 + n 行：ROLE:x / HEADERS:n / H:raw * n / BODY:m，
/// 随后紧跟恰好 m 字节正文。正文按字节数精确截取（tie 的 len 语义是字节数），
/// 可含任意内容（含换行、任意行首文本），不会破坏协议。
fn parse_protocol(text: &str) -> PreprocessResult {
    let mut headers = Vec::new();
    let mut role = FileRole::Logic;
    let mut body_len = 0usize;
    let mut body_start = None; // 正文起始字节偏移（BODY 行之后）
    let mut offset = 0usize;

    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if content.starts_with("ROLE:") {
            role = role_from_str(&content[5..]);
        } else if content.starts_with("HEADERS:") {
            let n: usize = content[8..].trim().parse().unwrap_or(0);
            // 预分配：n 条 H 行
            headers = Vec::with_capacity(n);
        } else if content.starts_with("H:") {
            headers.push(Header { raw: content[2..].to_string() });
        } else if content.starts_with("BODY:") {
            body_len = content[5..].trim().parse().unwrap_or(0);
            // BODY 行之后的位置 = 当前行结尾偏移（含换行）
            body_start = Some(offset + line.len());
        } else if body_start.is_none() {
            // 协议损坏（既不是头部行也不是 BODY）→ 空结果（防御）
            return PreprocessResult {
                cleaned_source: String::new(),
                headers: Vec::new(),
                role: FileRole::Logic,
            };
        }
        offset += line.len();
    }

    // 正文：从 BODY 行后取恰好 body_len 字节（超出按模块输出截断，缺失补空）
    let cleaned_source = match body_start {
        Some(start) => {
            let rest = &text[start..];
            rest.chars().take(body_len).collect()
        }
        None => String::new(),
    };
    PreprocessResult { cleaned_source, headers, role }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 无头文件默认为逻辑角色() {
        let r = preprocess("fn main() {}");
        assert_eq!(r.role, FileRole::Logic);
        assert!(r.headers.is_empty());
        assert!(r.cleaned_source.contains("fn main"));
    }

    #[test]
    fn 识别数据角色() {
        let r = preprocess("// tie:data\n[\"a\":1]\n");
        assert_eq!(r.role, FileRole::Data);
        assert_eq!(r.headers.len(), 1);
        assert!(!r.cleaned_source.contains("tie:data"));
    }

    #[test]
    fn 头部与内容分离() {
        let r = preprocess("// tie:logic\n// tie:opt=2\n\nfn main() {}\n");
        assert_eq!(r.role, FileRole::Logic);
        assert_eq!(r.headers.len(), 2);
        assert_eq!(r.headers[1].as_option(), Some(("opt", "2")));
        assert!(r.cleaned_source.starts_with("fn main"));
    }

    #[test]
    fn 内容区的tie注释是普通注释() {
        let r = preprocess("fn main() {\n    // tie:data\n}\n");
        assert_eq!(r.role, FileRole::Logic);
        assert_eq!(r.headers.len(), 0);
        assert!(r.cleaned_source.contains("tie:data"));
    }

    /// 扩展性验证：run_module 能加载任意 tie 转换器模块（Harbor M3）。
    ///
    /// 用 prep/indent.tie（制表符 → 4 空格转换器）证明：新增转换器只需
    /// 写 tie 模块 + 调用 run_module，Rust 侧零逻辑改动。
    #[test]
    fn 模块扩展性_挂载转换器模块() {
        let module = include_str!("../../../prep/indent.tie");
        // 含制表符缩进的输入（\t 转义在 Rust 字符串里直接写 tab）
        let src = "func main() {\n\tprintln(\"hi\")\n}\n";
        let out = run_module(module, "process", src).expect("转换器模块执行成功");
        assert!(
            out.contains("    println"),
            "制表符应转换为 4 空格，实际输出: {out:?}"
        );
        assert!(!out.contains('\t'), "输出不应残留制表符: {out:?}");
    }

    /// 扩展性验证：模块执行失败时返回 Err（错误信息可读），不 panic。
    #[test]
    fn 模块扩展性_缺失入口报错() {
        // 模块没有顶层 process 函数 → eval_call 报"未定义的函数"
        let err = run_module("func other() {}", "process", "x").unwrap_err();
        assert!(err.contains("process"), "错误应提及入口函数: {err}");
    }
}
