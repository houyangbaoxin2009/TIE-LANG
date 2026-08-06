//! 预处理核心逻辑：清理代码 + 识别文件类型 + 角色判定。
//!
//! 纯函数、无 IO，便于库调用与单元测试。

use std::fmt;

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

/// 预处理入口：原始源码 → 清理后的正文 + 头部信息 + 角色。
pub fn preprocess(source: &str) -> PreprocessResult {
    // 1. 清理：去 BOM、规范化换行
    let source = source.trim_start_matches('\u{FEFF}');
    let source = source.replace("\r\n", "\n");

    // 2. 识别文件类型：提取文件最前面的连续 `// tie:` 头指令
    let (headers, body) = extract_headers(&source);

    // 3. 判定角色
    let role = detect_role(&headers);

    PreprocessResult { cleaned_source: body, headers, role }
}

/// 提取头部：文件最前面连续出现的 `// tie:` 行。
///
/// 规则：
/// - 允许头部行之间的空行；
/// - 遇到第一个非头部内容行（非空、非 `// tie:`）即停止；
/// - 返回（头部指令列表, 清理后的正文）。
fn extract_headers(source: &str) -> (Vec<Header>, String) {
    let mut headers = Vec::new();
    let mut body_start = 0; // 正文起始行索引（第一个非头部内容行）
    let mut in_header_zone = true;

    for (idx, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if in_header_zone {
            if trimmed.is_empty() {
                // 头部区域内的空行：跳过（仍算头部区域）
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("// tie:") {
                let raw = rest.trim().to_string();
                headers.push(Header { raw });
                continue;
            }
            // 第一个非头部内容行：正文从这里开始
            body_start = idx;
            in_header_zone = false;
        }
    }

    // 重建正文：从第一个非头部内容行开始（不含头部行及其后的空行）
    let body: String = source.lines().skip(body_start).collect::<Vec<_>>().join("\n");
    (headers, body)
}

/// 从头部指令推断文件角色（无头 → Logic）。
fn detect_role(headers: &[Header]) -> FileRole {
    for h in headers {
        match h.kind() {
            "ui" => return FileRole::Ui,
            "db" => return FileRole::Db,
            "data" => return FileRole::Data,
            "library" => return FileRole::Library,
            _ => {}
        }
    }
    FileRole::Logic
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
}
