//! 多线程分片编译流水线：将编译项目切片、并行处理、产物入缓存池。
//!
//! Harbor M3 阶段二（协调统筹增强）：当配置 `advanced.enabled = true` 时，
//! `tie` 把输入的多个文件（或目录下的 `.tie` 文件）视为一个**编译项目**，
//! 按文件**切片**，用多线程流水线处理：
//!
//! ```text
//! 阶段 1（预处理）    阶段 2（前端 + IR）   阶段 3（后端）
//! 每个切片 preprocess  每个切片 词法→语法→   每个切片 写 .ll →
//!     ↓ 产物入池        语义→IR 文本           opt → 链接/归档
//!     ↓                 ↓ 产物入池             ↓ 最终产物
//! ── 屏障 ────────    ── 屏障 ────────    ── 完成 ──
//! ```
//!
//! 关键语义（用户需求）：**每处理一步，把产物释放到缓存池；所有切片都
//! 释放到缓存池后（屏障），才进入下一步**。阶段间通过 [std::thread::scope]
//! 自然形成屏障——所有 worker join 后才继续下一阶段。
//!
//! 中间文件冲突处理：并行时每个切片写自己的 `.ll`/`.opt.ll`，放在独立的
//! 工作子目录（`<cache.path>/work/<切片名>/`），互不干扰，编译后清理。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tie_llvm::driver::{CompileOptions, CompileOutcome, compile_from_ir};
use tie_llvm::optimizer::OptLevel;
use tie_prep::preprocess::{FileRole, PreprocessResult, preprocess};

use crate::cache::CachePool;
use crate::config::Config;

/// 一个编译切片（= 项目里的一个源文件）。
struct Slice {
    /// 切片名（缓存键与工作目录名的基础；取文件路径的相对字符串）
    name: String,
    /// 源文件路径
    input: PathBuf,
}

/// 分片编译流水线。
pub struct Pipeline<'a> {
    /// 协调配置
    config: &'a Config,
    /// 切片列表
    slices: Vec<Slice>,
    /// 缓存池（阶段间中转仓库）
    cache: CachePool,
    /// 并行线程数
    threads: usize,
    // ---- CLI 透传选项 ----
    /// 输出路径（单切片时生效；多切片各自默认命名）
    output: Option<PathBuf>,
    /// 优化级别
    opt_level: Option<OptLevel>,
    /// 只生成 IR（阶段 2 后即停）
    emit_ir_only: bool,
    /// 保留中间 IR 文件
    keep_ir: bool,
    /// 交叉编译目标
    target: Option<String>,
}

impl<'a> Pipeline<'a> {
    /// 构造流水线：输入文件列表 + 配置 + CLI 透传选项。
    ///
    /// 目录输入会被展开为目录下全部 `.tie` 文件（每个文件一个切片）。
    pub fn new(
        config: &'a Config,
        inputs: &[PathBuf],
        output: Option<PathBuf>,
        opt_level: Option<OptLevel>,
        emit_ir_only: bool,
        keep_ir: bool,
        target: Option<String>,
    ) -> Result<Pipeline<'a>, String> {
        // 展开输入：文件直接入列；目录收集全部 *.tie 文件
        let mut files: Vec<PathBuf> = Vec::new();
        for input in inputs {
            if input.is_dir() {
                let mut found = 0usize;
                for entry in fs::read_dir(input)
                    .map_err(|e| format!("读取目录 {} 失败: {e}", input.display()))?
                {
                    let entry = entry.map_err(|e| e.to_string())?;
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "tie") {
                        files.push(path);
                        found += 1;
                    }
                }
                if found == 0 {
                    return Err(format!("目录 {} 中没有 .tie 文件", input.display()));
                }
            } else {
                files.push(input.clone());
            }
        }

        if files.is_empty() {
            return Err("没有可编译的输入文件".into());
        }

        let slices = files
            .into_iter()
            .map(|input| Slice {
                // 切片名 = 去扩展名后的路径字符串（保留相对路径，避免跨目录重名）
                name: input.with_extension("").display().to_string().replace('\\', "/"),
                input,
            })
            .collect::<Vec<_>>();

        // 线程数：配置 0 = 自动（按 CPU 核数）；不超过切片数
        let threads = if config.advanced.threads == 0 {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
        } else {
            config.advanced.threads
        };
        let threads = threads.min(slices.len()).max(1);

        Ok(Pipeline {
            config,
            slices,
            cache: CachePool::new(&config.cache),
            threads,
            output,
            opt_level,
            emit_ir_only,
            keep_ir,
            target,
        })
    }

    /// 运行流水线：三阶段（预处理 → 前端+IR → 后端），每阶段并行 + 屏障。
    ///
    /// 返回各切片的编译产物消息。
    pub fn run(&self) -> Result<Vec<CompileOutcome>, String> {
        // ---- 阶段 1：预处理（并行）----
        // 每个切片 preprocess → 产物（正文 + 角色）写入缓存池 `prep:<名>`
        let preps: Vec<PreprocessResult> = self.parallel(&self.slices, |slice| {
            let source = fs::read_to_string(&slice.input)
                .map_err(|e| format!("读取 {} 失败: {e}", slice.input.display()))?;
            let pre = preprocess(&source);
            // 文件名默认角色与头部声明一致性检查：头部声明优先，不一致时警告。
            // `xxx.<角色>.tie` 形式的文件名声明只是默认值，头部 `type tie<X>`
            // 声明是权威——不一致仅提示、不报错，采用头部声明。
            let name_role = slice
                .input
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(FileRole::from_filename);
            if let Some(r) = name_role
                && r != pre.role
            {
                eprintln!(
                    "警告: 文件 {} 名称声明为 {}，与头部声明 {} 不一致，已采用头部声明",
                    slice.input.display(),
                    r,
                    pre.role
                );
            }
            // 释放到缓存池：正文（后续阶段从此取）
            self.cache.put(
                format!("prep:{}", slice.name),
                pre.cleaned_source.clone().into_bytes(),
            );
            Ok(pre)
        })?;

        // ---- 阶段 2：前端 + IR 生成（并行）----
        // 每个切片：从缓存池取预处理正文 → 词法/语法/import/语义 → IR 文本入池 `ir:<名>`
        let ir_used: Vec<Vec<String>> = self.parallel(&self.slices, |slice| {
            let idx = self.slices.iter().position(|s| s.name == slice.name).unwrap();
            // 非编译角色（data/ui/db/port）跳过：不参与后端
            if !matches!(
                preps[idx].role,
                FileRole::Logic | FileRole::Script | FileRole::Class | FileRole::Type
            ) {
                return Ok(Vec::new());
            }
            let source = self
                .cache
                .get(&format!("prep:{}", slice.name))
                .ok_or_else(|| format!("缓存缺失: prep:{}", slice.name))?;
            let source = String::from_utf8(source).map_err(|e| format!("缓存正文非法 UTF-8: {e}"))?;

            // 词法（含 ASI）
            let tokens = tie_frontend::lexer::tokenize(&source)
                .map_err(|e| format!("{}: {e}", slice.input.display()))?;
            // 语法
            let program = tie_frontend::parser::parse_program(&tokens)
                .map_err(|e| format!("{}: {e}", slice.input.display()))?;
            // import 展开（相对切片所在目录）
            let base_dir = slice
                .input
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let program = tie_frontend::imports::expand_imports(
                program,
                &base_dir,
                &tie_prep::clean_source,
            )
            .map_err(|e| format!("{}: {e}", slice.input.display()))?;
            // 语义分析
            let sem = tie_frontend::semantic::analyze(&program)
                .map_err(|e| format!("{}: {e}", slice.input.display()))?;
            // 入口检查：logic/script 必须有 main（class/type 库角色不需要）
            if !self.emit_ir_only
                && matches!(preps[idx].role, FileRole::Logic | FileRole::Script)
                && !sem.funcs.contains_key("main")
            {
                return Err(format!(
                    "{}: 文件角色为 logic/script，必须定义入口函数 main",
                    slice.input.display()
                ));
            }
            // IR 生成 → 入池
            let ir_out = tie_llvm::ir::gen_ir(&program, &sem)
                .map_err(|e| format!("{}: {e}", slice.input.display()))?;
            let used = ir_out.used_externs.clone();
            self.cache.put(format!("ir:{}", slice.name), ir_out.ir.into_bytes());
            Ok(used)
        })?;

        // emit_ir_only：IR 已入池，写回输入同名 .ll 后结束
        if self.emit_ir_only {
            let mut outcomes = Vec::new();
            for (idx, slice) in self.slices.iter().enumerate() {
                if !matches!(
                    preps[idx].role,
                    FileRole::Logic | FileRole::Script | FileRole::Class | FileRole::Type
                ) {
                    continue;
                }
                let ir = self
                    .cache
                    .get(&format!("ir:{}", slice.name))
                    .ok_or_else(|| format!("缓存缺失: ir:{}", slice.name))?;
                let ir_path = slice.input.with_extension("ll");
                fs::write(&ir_path, ir).map_err(|e| format!("写入 {} 失败: {e}", ir_path.display()))?;
                outcomes.push(CompileOutcome {
                    message: format!("已生成 LLVM IR: {}", ir_path.display()),
                    artifact: Some(ir_path),
                });
            }
            return Ok(outcomes);
        }

        // ---- 阶段 3：后端（并行）----
        // 每个切片：从缓存池取 IR 文本 → 写独立工作目录 .ll → opt → 链接/归档
        let outcomes = self.parallel(&self.slices, |slice| {
            let idx = self.slices.iter().position(|s| s.name == slice.name).unwrap();
            // 非编译角色跳过
            if !matches!(
                preps[idx].role,
                FileRole::Logic | FileRole::Script | FileRole::Class | FileRole::Type
            ) {
                return Ok(CompileOutcome {
                    message: format!(
                        "[tie] {}: 角色 {}，已跳过编译（该工具链后续版本实现）",
                        slice.input.display(),
                        preps[idx].role
                    ),
                    artifact: None,
                });
            }
            let ir = self
                .cache
                .get(&format!("ir:{}", slice.name))
                .ok_or_else(|| format!("缓存缺失: ir:{}", slice.name))?;

            // 独立工作目录：cache.path/work/<切片名>/，避免并行写同名 .ll 冲突
            let work_dir = self.config.cache.path.join("work").join(safe_dir_name(&slice.name));
            fs::create_dir_all(&work_dir)
                .map_err(|e| format!("创建工作目录 {} 失败: {e}", work_dir.display()))?;
            let ir_path = work_dir.join(format!("{}.ll", safe_file_stem(&slice.name)));
            fs::write(&ir_path, ir).map_err(|e| format!("写入 {} 失败: {e}", ir_path.display()))?;

            // 复用 tie-llvm 后端（opt + 链接/归档），输入指向工作目录的 .ll
            let opts = CompileOptions {
                input: slice.input.clone(),
                output: if self.slices.len() == 1 { self.output.clone() } else { None },
                opt_level: self.opt_level,
                emit_ir_only: false,
                keep_intermediate: self.keep_ir,
                target: self.target.clone(),
            };
            // 用切片角色重建 PreprocessResult（后端按角色分派产物；
            // 头部信息已随旧 // tie: 指令系统移除，compile_from_ir 不再读头部）
            let pre = PreprocessResult {
                cleaned_source: String::new(),
                role: preps[idx].role,
            };
            let ir_meta = tie_llvm::ir::IrOutput {
                ir: String::new(),
                used_externs: ir_used[idx].clone(),
            };
            let outcome = compile_from_ir(&ir_path, &ir_meta, &pre, &opts)
                .map_err(|e| format!("{}: {e}", slice.input.display()))?;
            // 未要求保留时清理工作目录
            if !self.keep_ir {
                let _ = fs::remove_dir_all(&work_dir);
            } else {
                // 保留：把消息里的路径指到真实保留位置
                return Ok(CompileOutcome {
                    message: format!("{}（中间 IR 保留于 {}）", outcome.message, ir_path.display()),
                    artifact: outcome.artifact,
                });
            }
            Ok(outcome)
        })?;

        // 阶段间流转完成：清空缓存池（释放中间产物内存/磁盘缓存）并删除工作目录
        let work_base = self.config.cache.path.join("work");
        self.cache.clear(Some(&work_base));

        Ok(outcomes)
    }

    /// 并行处理所有切片（阶段内并行，join 即阶段屏障）。
    ///
    /// `threads` 个 worker 各领一部分切片并行执行；所有 worker 结束后
    /// （[std::thread::scope] 自动 join）统一收集结果——这就是「所有切片
    /// 都释放到缓存池后，进行下一步」的屏障实现。
    fn parallel<T, F>(&self, slices: &[Slice], f: F) -> Result<Vec<T>, String>
    where
        F: Fn(&Slice) -> Result<T, String> + Sync + Send,
        T: Send,
    {
        let n = slices.len();
        // 各 worker 写入独立槽位（按全局索引），join 后按序读取
        let slots: Vec<Mutex<Option<Result<T, String>>>> = (0..n).map(|_| Mutex::new(None)).collect();
        // 借引用捕获：&F 是 Fn 且 F: Sync 时 &F: Send，可在多线程间共享
        let f = &f;
        std::thread::scope(|scope| {
            let chunk = (n + self.threads - 1) / self.threads;
            for (chunk_start, chunk_slices) in slices.chunks(chunk).enumerate() {
                let slots = &slots;
                scope.spawn(move || {
                    for (i, slice) in chunk_slices.iter().enumerate() {
                        let global_i = chunk_start * chunk + i;
                        let r = f(slice);
                        *slots[global_i].lock().unwrap() = Some(r);
                    }
                });
            }
        });
        // 收集结果（所有槽位应已填充；take 移出值，避免 T 需要 Clone）
        let mut results = Vec::with_capacity(n);
        for slot in &slots {
            results.push(
                slot.lock()
                    .unwrap()
                    .take()
                    .expect("并行阶段槽位应被填充"),
            );
        }
        // 阶段失败：返回第一个错误（不继续下一阶段）
        for r in &results {
            if let Err(e) = r {
                return Err(e.clone());
            }
        }
        // 全部成功：解包
        Ok(results.into_iter().map(|r| r.unwrap()).collect())
    }
}

/// 工作目录安全名：把路径分隔符替换为下划线，去掉盘符冒号。
fn safe_dir_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '_',
            other => other,
        })
        .collect()
}

/// 文件茎安全名（与输入同名去扩展名，供 .ll 命名）。
fn safe_file_stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("slice")
        .to_string()
}
