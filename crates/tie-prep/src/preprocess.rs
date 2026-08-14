//! 预处理核心逻辑：清理代码 + 识别文件类型 + 角色判定。
//!
//! Harbor M3 自举：核心逻辑（声明行提取/角色判定/正文重建）由 tie 语言编写
//! （prep/core.tie，include_str! 内嵌），本文件是解释执行壳——
//! 1. 字节规范化（去 BOM、CRLF→LF）仍留在壳层（tie 字符串字面量无法表达
//!    BOM 字符，且字节规范化属壳层职责）；
//! 2. 通过 tie-interp 的 eval/eval_call 注册并调用 `prep::process`；
//! 3. 解析模块返回的协议文本，还原 [PreprocessResult]。
//!
//! # 新文件类型声明系统
//!
//! 文件在头部区（文件最前面的连续前导行，允许其间空行）用真正的语法行声明
//! 类型，形如：
//! ```text
//! type tie          # 泛型入口类型（FileRole::Type）
//! type tie<data>    # 数据文件（FileRole::Data）
//! type tie<db>      # 数据库文件（FileRole::Db）
//! type tie<ir>      # IR 文件（FileRole::Ir，直接生成 LLVM IR .ll）
//! ```
//! `type tie<X>` 的 X ∈ {script, data, ui, class, logic, port, db, ir, zd}。
//! 无声明时默认角色为 logic。多次声明首个生效，其余同样剥离。
//! 旧 `// tie:xxx` 注释指令系统已完全移除——该类注释不再被提取/剥离，
//! 作为普通注释留在正文中（词法阶段自然忽略）。
//!
//! 协议文本（与 prep/core.tie 约定）：
//! ```text
//! ROLE:data
//! BODY:12
//! <正文恰好 12 码点>
//! ```
//! 声明错误时模块首行输出 `ERROR:<message>`，Rust 侧检测后 panic 报出
//! （preprocess 无 Result 签名、调用方一律期望 [PreprocessResult]，以
//! panic 携带声明错误文本是最简单一致的暴露方式）。
//! 协议头逐行固定，正文按 BODY 声明的码点数精确截取——正文可含任意内容
//! （含换行），不会破坏协议。

use std::fmt;

/// prep/core.tie 模块源码（编译期内嵌，发布无需额外文件）。
const PREP_MODULE: &str = include_str!("../../../prep/core.tie");

/// 模块入口函数全名（命名空间 prep 下的 process）。
const PREP_ENTRY: &str = "prep::process";

/// prep/clean.tie 清理脚本源码（编译期内嵌，发布无需额外文件）。
const CLEAN_MODULE: &str = include_str!("../../../prep/clean.tie");

/// 清理脚本入口函数全名（命名空间 prep_clean 下的 process）。
const CLEAN_ENTRY: &str = "prep_clean::process";

/// 行级源码清理：去 BOM、CRLF 归一（壳层职责——tie 字符串字面量无法表达
/// BOM 字符），再解释执行 tie 语言自写的清理脚本 prep/clean.tie 剥离头部区
/// 文件类型声明行（`type tie` / `type tie<X>`，声明行空行占位，行号不变）。
///
/// 供 import 展开 / 解释器 / LSP 等需要**行号对齐**的路径复用（编译路径的
/// 正文重建走 [preprocess]）。脚本剥离失败 → panic 并给出可读信息
/// （脚本内嵌于二进制，失败属于自举链破损，应尽早暴露）。
pub fn clean_source(source: &str) -> String {
    let source = source.trim_start_matches('\u{FEFF}');
    let source = source.replace("\r\n", "\n");
    run_module(CLEAN_MODULE, CLEAN_ENTRY, &source)
        .unwrap_or_else(|e| panic!("清理脚本执行失败: {e}"))
}

/// 文件角色：由 `type tie` / `type tie<X>` 声明（或文件名 `<名>.<角色>.tie`）
/// 决定，表示文件在四段式工具链中的类型，决定转交哪个工具链。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRole {
    /// 泛型入口类型（`type tie` 声明 / xxx.type.tie 文件名默认）
    Type,
    /// 脚本（编译可执行）
    Script,
    /// 数据文件（存数据/数据交换）
    Data,
    /// 界面文件
    Ui,
    /// 类/库文件（编译静态库 .a）
    Class,
    /// 逻辑代码（编译可执行，需 main）——无声明时的默认角色
    Logic,
    /// 端口/对外接口文件
    Port,
    /// 数据库文件
    Db,
    /// IR 文件（`type tie<ir>` 声明 / xxx.ir.tie 文件名默认）——
    /// 检测到即直接生成 LLVM IR（.ll），不继续 opt/clang 链接
    Ir,
    /// 压缩的 tie:data（`type tie<zd>` 声明 / xxx.zd.tie 文件名默认）——
    /// 二进制、人类不可读，主要用文件名 `xxx.zd.tie` 声明；
    /// 必须按文件名预判并在任何文本读取之前短接
    Zd,
}

impl FileRole {
    /// 角色名（用于消息展示 / 协议文本 / 文件名识别）。
    pub fn as_str(self) -> &'static str {
        match self {
            FileRole::Type => "type",
            FileRole::Script => "script",
            FileRole::Data => "data",
            FileRole::Ui => "ui",
            FileRole::Class => "class",
            FileRole::Logic => "logic",
            FileRole::Port => "port",
            FileRole::Db => "db",
            FileRole::Ir => "ir",
            FileRole::Zd => "zd",
        }
    }

    /// 角色名 → 枚举（精确匹配 10 个角色名，未知返回 None）。
    ///
    /// 与旧 `role_from_str`（未知回退 logic）不同：未知角色来自声明系统
    /// 之外（如 `type tie<library>`），必须在 [preprocess] 阶段报错而非
    /// 静默回退，故返回 Option 交由调用方决定处理方式。
    pub fn from_str(s: &str) -> Option<FileRole> {
        match s.trim() {
            "type" => Some(FileRole::Type),
            "script" => Some(FileRole::Script),
            "data" => Some(FileRole::Data),
            "ui" => Some(FileRole::Ui),
            "class" => Some(FileRole::Class),
            "logic" => Some(FileRole::Logic),
            "port" => Some(FileRole::Port),
            "db" => Some(FileRole::Db),
            "ir" => Some(FileRole::Ir),
            "zd" => Some(FileRole::Zd),
            _ => None,
        }
    }

    /// 从文件名推断默认角色：`xxx.<角色>.tie` 形式（文件名默认角色，
    /// 头部声明优先于文件名声明）。
    ///
    /// 仅当文件名以 `.tie` 结尾且倒数第二段（`<名>.<角色>` 中的 `<角色>`）
    /// 恰好等于 9 个角色名之一时识别：
    /// - `"main.type.tie"` → [FileRole::Type]
    /// - `"schema.db.tie"` → [FileRole::Db]
    /// - `"app.script.tie"` → [FileRole::Script]
    /// - `"code.ir.tie"` → [FileRole::Ir]
    /// - `"main.tie"` → None（无角色段）
    /// - `"foo.logic2.tie"` → None（中间段不是合法角色名）
    /// - `"main.data.txt"` → None（非 `.tie` 后缀）
    pub fn from_filename(name: &str) -> Option<FileRole> {
        // 去掉 ".tie" 后缀；非 .tie 文件无角色概念
        let stem = name.strip_suffix(".tie")?;
        // 取最后一段作为角色段（"main.type" → "type"）
        let dot = stem.rfind('.')?;
        let role_seg = &stem[dot + 1..];
        Self::from_str(role_seg)
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
    /// 清理后的正文源码（无 BOM、统一 `\n` 换行、已剥离声明行）
    pub cleaned_source: String,
    /// 文件角色
    pub role: FileRole,
}

/// 从文件名推断默认角色（[FileRole::from_filename] 的壳层转发）。
///
/// 供 crates/tie、crates/tie-llvm 等调用方在读取文件前按文件名预判角色；
/// 注意文件名只是默认，头部声明优先于文件名声明。
pub fn role_from_filename(name: &str) -> Option<FileRole> {
    FileRole::from_filename(name)
}

/// 预处理入口：原始源码 → 清理后的正文 + 角色。
///
/// 自举实现：字节规范化（去 BOM、CRLF→LF）后，解释执行 prep/core.tie 的
/// `prep::process`（声明行提取/角色判定/正文重建全在 tie 模块内），
/// 再解析模块返回的协议文本。模块注册与调用失败 → panic 并给出可读信息
/// （模块内嵌于二进制，失败属于自举链破损，应尽早暴露）。
///
/// 文件声明错误（如 `type tie<library>` 未知子类型、`type tie<data> extra`
/// 多余尾随内容）同样 panic：preprocess 无 Result 签名、所有调用方期望
/// [PreprocessResult]，以 panic 携带声明错误文本是最简单一致的暴露方式
/// （错误消息含声明行原文，用户可直接定位修复）。
pub fn preprocess(source: &str) -> PreprocessResult {
    // 1. 字节规范化（壳层职责）：去 BOM、CRLF 归一
    let source = source.trim_start_matches('\u{FEFF}');
    let source = source.replace("\r\n", "\n");

    // 2. 解释执行 tie 模块：注册 prep::process 并传入源码
    let text = run_module(PREP_MODULE, PREP_ENTRY, &source)
        .unwrap_or_else(|e| panic!("预处理模块执行失败: {e}"));

    // 3. 解析协议文本（ROLE / BODY / 可选 ERROR）；声明错误 → panic 携带原文
    parse_protocol(&text).unwrap_or_else(|e| panic!("文件声明错误: {e}"))
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
/// 协议固定 1 + 2 + 正文 段：ROLE:x / BODY:m / 随后恰好 m 个码点的正文。
/// 声明错误时首行为 ERROR:<message>，此处以 Err 返回（由 [preprocess]
/// panic 携带文本暴露）。正文按码点数精确截取（BODY 声明的是 str_len
/// 码点数，与 str.chars().take(m) 对齐），可含任意内容（含换行、任意
/// 行首文本），不会破坏协议。
fn parse_protocol(text: &str) -> Result<PreprocessResult, String> {
    let mut role = FileRole::Logic;
    let mut body_len = 0usize;
    let mut body_start = None; // 正文起始字节偏移（BODY 行之后）
    let mut error = None;      // 声明错误消息（ERROR: 行，首个生效）
    let mut offset = 0usize;

    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if content.starts_with("ERROR:") {
            // 声明错误：记录消息，之后其余协议行不再解析
            error = Some(content[6..].to_string());
        } else if content.starts_with("ROLE:") {
            // 角色名精确匹配；未知角色防御性回退默认 logic（正常协议
            // 中角色名必合法，未知只可能来自协议损坏/模块 bug）
            role = FileRole::from_str(&content[5..]).unwrap_or(FileRole::Logic);
        } else if content.starts_with("BODY:") {
            body_len = content[5..].trim().parse().unwrap_or(0);
            // BODY 行之后的位置 = 当前行结尾偏移（含换行）
            body_start = Some(offset + line.len());
        } else if body_start.is_none() && error.is_none() {
            // 协议损坏（既不是 ERROR/ROLE/BODY 也不是正文）→ 空结果（防御）
            return Ok(PreprocessResult {
                cleaned_source: String::new(),
                role: FileRole::Logic,
            });
        }
        offset += line.len();
    }

    if let Some(msg) = error {
        return Err(msg);
    }

    // 正文：从 BODY 行后取恰好 body_len 个字符（码点；超出按模块输出截断，缺失补空）
    let cleaned_source = match body_start {
        Some(start) => {
            let rest = &text[start..];
            rest.chars().take(body_len).collect()
        }
        None => String::new(),
    };
    Ok(PreprocessResult { cleaned_source, role })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 新文件类型声明系统（type tie / type tie<X>） ----------

    #[test]
    fn 无声明默认为逻辑角色() {
        let r = preprocess("fn main() {}\n");
        assert_eq!(r.role, FileRole::Logic);
        assert!(r.cleaned_source.contains("fn main"));
    }

    #[test]
    fn 声明type_tie为泛型入口角色() {
        let r = preprocess("type tie\nfn main() {}\n");
        assert_eq!(r.role, FileRole::Type);
        assert!(!r.cleaned_source.contains("type tie"), "声明行应被剥离");
        assert!(r.cleaned_source.contains("fn main"));
    }

    #[test]
    fn 声明type_tie_data为数据角色() {
        let r = preprocess("type tie<data>\n[\"a\":1]\n");
        assert_eq!(r.role, FileRole::Data);
        assert!(!r.cleaned_source.contains("type tie"), "声明行应被剥离");
        assert!(r.cleaned_source.contains("\"a\":1"));
    }

    #[test]
    fn 声明type_tie_db为数据库角色() {
        let r = preprocess("type tie<db>\n[\"schema\":{}]\n");
        assert_eq!(r.role, FileRole::Db);
        assert!(r.cleaned_source.contains("\"schema\""));
    }

    #[test]
    fn 声明type_tie_class为库角色() {
        let r = preprocess("type tie<class>\nfunc add(a: i64, b: i64) -> i64 { return a + b }\n");
        assert_eq!(r.role, FileRole::Class);
    }

    #[test]
    fn 声明type_tie_ir为IR角色() {
        // ir 角色：直接生成 LLVM IR（.ll），不继续 opt/clang 链接
        let r = preprocess("type tie<ir>\nfunc main() { println(\"ir\") }\n");
        assert_eq!(r.role, FileRole::Ir);
        assert!(!r.cleaned_source.contains("type tie"), "声明行应被剥离");
        assert!(r.cleaned_source.contains("println"));
    }

    #[test]
    #[should_panic(expected = "未知子类型")]
    fn 未知子类型声明报错() {
        // library 已随旧 // tie: 指令系统移除，属于未知子类型 → 声明错误
        let _ = preprocess("type tie<library>\nfn main() {}\n");
    }

    #[test]
    #[should_panic(expected = "声明格式错误")]
    fn 声明行多余尾随内容报错() {
        // `type tie<data> extra` 不是合法声明（trim 后仍有尾随内容）→ 错误
        let _ = preprocess("type tie<data> extra\n[1]\n");
    }

    #[test]
    fn 声明行从正文剥离() {
        // 声明行与其后（声明区内的）空行都不进入正文
        let r = preprocess("type tie<data>\n\n[\"a\":1]\n");
        assert_eq!(r.role, FileRole::Data);
        assert!(!r.cleaned_source.contains("type tie"), "声明行应被剥离");
        assert_eq!(r.cleaned_source, "[\"a\":1]", "声明区空行也应剥离");
    }

    #[test]
    fn 旧式注释不再剥离且角色默认逻辑() {
        // 旧 // tie:xxx 指令系统已移除：头部区的 // tie:logic 是普通注释，
        // 不剥离、不影响角色（默认 logic）
        let r = preprocess("// tie:logic\nfn main() {}\n");
        assert_eq!(r.role, FileRole::Logic);
        assert!(
            r.cleaned_source.contains("// tie:logic"),
            "// tie: 注释应保留在正文，实际: {:?}",
            r.cleaned_source
        );
        assert!(r.cleaned_source.contains("fn main"));
    }

    #[test]
    fn 正文内的声明行不是声明() {
        // 声明行出现在正文中（首个内容行之后）→ 只是普通内容，角色保持默认
        let r = preprocess("fn main() {}\ntype tie<data>\n");
        assert_eq!(r.role, FileRole::Logic);
        assert!(
            r.cleaned_source.contains("type tie<data>"),
            "正文内的声明行应原样保留"
        );
    }

    #[test]
    fn 多次声明首个生效() {
        let r = preprocess("type tie<data>\ntype tie<db>\n[1]\n");
        assert_eq!(r.role, FileRole::Data, "首个声明应生效");
        assert!(!r.cleaned_source.contains("type tie"), "后续声明行也应剥离");
        assert_eq!(r.cleaned_source, "[1]");
    }

    #[test]
    fn 声明行尾随空白被trim() {
        // 声明行尾随空格不影响识别（头部区扫描先 trim）
        let r = preprocess("type tie<data>  \n[1]\n");
        assert_eq!(r.role, FileRole::Data);
    }

    // ---------- 文件名默认角色（xxx.<角色>.tie） ----------

    #[test]
    fn 从文件名识别角色() {
        assert_eq!(FileRole::from_filename("main.type.tie"), Some(FileRole::Type));
        assert_eq!(FileRole::from_filename("schema.db.tie"), Some(FileRole::Db));
        assert_eq!(FileRole::from_filename("app.script.tie"), Some(FileRole::Script));
        assert_eq!(FileRole::from_filename("lib.class.tie"), Some(FileRole::Class));
        assert_eq!(FileRole::from_filename("ui_main.ui.tie"), Some(FileRole::Ui));
        assert_eq!(FileRole::from_filename("log.logic.tie"), Some(FileRole::Logic));
        assert_eq!(FileRole::from_filename("svc.port.tie"), Some(FileRole::Port));
        assert_eq!(FileRole::from_filename("code.ir.tie"), Some(FileRole::Ir));
        assert_eq!(FileRole::from_filename("app.zd.tie"), Some(FileRole::Zd));
        // 非 `<名>.<角色>.tie` 形式 → None
        assert_eq!(FileRole::from_filename("main.tie"), None);
        assert_eq!(FileRole::from_filename("foo.logic2.tie"), None);
        assert_eq!(FileRole::from_filename("main.data.txt"), None);
        assert_eq!(FileRole::from_filename("main.type.tie.extra"), None);
        assert_eq!(FileRole::from_filename("main.Type.tie"), None, "角色名区分大小写");
    }

    #[test]
    fn role_from_filename壳层转发() {
        assert_eq!(role_from_filename("schema.db.tie"), Some(FileRole::Db));
        assert_eq!(role_from_filename("main.tie"), None);
    }

    #[test]
    fn 角色名双向映射() {
        let all = [
            FileRole::Type,
            FileRole::Script,
            FileRole::Data,
            FileRole::Ui,
            FileRole::Class,
            FileRole::Logic,
            FileRole::Port,
            FileRole::Db,
            FileRole::Ir,
            FileRole::Zd,
        ];
        for r in all {
            assert_eq!(FileRole::from_str(r.as_str()), Some(r), "角色 {}", r.as_str());
        }
        // 未知/空/大小写不符/多余后缀 → None
        assert_eq!(FileRole::from_str("library"), None);
        assert_eq!(FileRole::from_str(""), None);
        assert_eq!(FileRole::from_str("LOGIC"), None);
        assert_eq!(FileRole::from_str("logic2"), None);
        assert_eq!(FileRole::from_str(" data "), Some(FileRole::Data), "允许首尾空白");
    }

    // ---------- 扩展性验证（Harbor M3：run_module 挂载任意 tie 模块） ----------

    /// 扩展性验证：run_module 能加载任意 tie 转换器模块。
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

    // ---------- 中文编码缺陷回归（M 修复：len 字节数 vs str_char 码点索引） ----------
    //
    // 缺陷根因：len() 返回 UTF-8 字节数，str_char() 按 Unicode 码点索引。对中文
    // （一个汉字 3 字节）混用导致 trim 尾随空白无法去除、slice 边界错位。修复后
    // prep/core.tie 的字符串边界改用 str_len（码点数），以下用例验证中文完整保留。

    #[test]
    fn 中文正文完整保留() {
        let src = "type tie\nfunc main() {\n    println(\"你好，世界！\")\n}\n";
        let r = preprocess(src);
        assert!(
            r.cleaned_source.contains("你好，世界！"),
            "中文正文应完整保留，实际: {:?}",
            r.cleaned_source
        );
        assert_eq!(r.role, FileRole::Type);
    }

    #[test]
    fn 中文注释行后正文正确重建() {
        // "// 中文注释头部行" 不是 type tie 声明 → 属于正文（保留）
        let src = "// tie:logic\n// 中文注释头部行\nfunc main() {}\n";
        let r = preprocess(src);
        assert!(r.cleaned_source.contains("中文注释头部行"));
        assert_eq!(r.role, FileRole::Logic);
    }

    #[test]
    fn 中文数据文件角色判定() {
        let src = "type tie<data>\n[\"键\":\"值\"]\n";
        let r = preprocess(src);
        assert_eq!(r.role, FileRole::Data);
        assert!(
            r.cleaned_source.contains("键"),
            "中文键应保留: {:?}",
            r.cleaned_source
        );
    }

    /// str_len 语义验证：码点数（chars().count）≠ len 字节数（s.len()），
    /// 中文"你好"码点 2、字节 6；ASCII 两者一致。
    #[test]
    fn str_len与len的码点字节语义区分() {
        let module = include_str!("../../../prep/test_trim.tie");
        // test_trim.tie 的 T4 行输出 str_len 与 len 对比
        let out = run_module(module, "process", "").expect("模块执行成功");
        assert!(out.contains("str_len=2"), "中文 str_len 应为 2（码点），实际: {out:?}");
        assert!(out.contains("len=6"), "中文 len 应为 6（字节），实际: {out:?}");
    }

    /// 转换器模块对含中文源码逐字符遍历不丢字（str_len 码点边界）。
    #[test]
    fn 中文模块转换完整保留() {
        let module = include_str!("../../../prep/indent.tie");
        let src = "func main() {\n\tprintln(\"中文测试\")\n}\n";
        let out = run_module(module, "process", src).expect("转换器模块执行成功");
        assert!(
            out.contains("中文测试"),
            "中文应完整保留，实际: {out:?}"
        );
        assert!(!out.contains('\t'), "制表符应转换，实际: {out:?}");
    }
}
