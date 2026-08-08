//! 编译缓存池：分片编译的中间产物缓存。
//!
//! Harbor M3 阶段二（协调统筹增强）：多线程分片编译时，每处理一步
//! （预处理 / 前端+IR），把每个切片的产物写入缓存池；**所有切片都写入后**
//! （屏障）才进入下一步。缓存池即为切片间的中转仓库。
//!
//! 存储技术（来自配置文件 `cache.storage`）：
//! - [Storage::Memory]：进程内 HashMap，读写快，进程退出即失；
//! - [Storage::File]：磁盘目录（[Cache::path]），跨进程/跨次编译可复用。
//!
//! 容量上限（[Cache::size_bytes]）：写入后若总字节超限，按 LRU 淘汰
//! （最久未访问的条目先删）。

use crate::config::{Cache, Storage};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// 缓存条目。
#[derive(Debug)]
struct Entry {
    /// 数据字节数
    size: u64,
    /// 最近访问时钟（越大越新）
    last_used: u64,
    /// memory 存储：数据本体；file 存储：空（内容在磁盘）
    data: Option<Vec<u8>>,
}

/// 编译缓存池（线程安全：内部 [Mutex] 串行化所有操作）。
pub struct CachePool {
    /// 存储技术（构造时固定）
    storage: Storage,
    /// file 存储的缓存目录（memory 存储为 None）
    dir: Option<PathBuf>,
    /// 容量上限（字节）
    capacity: u64,
    /// 条目表：键 → 条目
    entries: Mutex<HashMap<String, Entry>>,
    /// 单调时钟（LRU 访问序）
    clock: Mutex<u64>,
}

/// 计算 key 的安全文件名（file 存储：路径可含 `/`/`:` 等非法字符，需转义）。
fn safe_file_name(key: &str) -> String {
    // 简单替换：非字母数字下划线点 → `_`
    key.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

impl CachePool {
    /// 依据配置构造缓存池。
    ///
    /// file 存储：确保缓存目录存在（不存在则创建）。
    pub fn new(cache: &Cache) -> CachePool {
        let dir = match cache.storage {
            Storage::Memory => None,
            Storage::File => {
                let _ = fs::create_dir_all(&cache.path);
                Some(cache.path.clone())
            }
        };
        CachePool {
            storage: cache.storage,
            dir,
            capacity: cache.size_bytes,
            entries: Mutex::new(HashMap::new()),
            clock: Mutex::new(0),
        }
    }

    /// 当前时钟值并自增（LRU 访问序）。
    fn tick(&self) -> u64 {
        let mut c = self.clock.lock().unwrap();
        *c += 1;
        *c
    }

    /// 写入条目。若总字节数超限，反复淘汰最久未访问的条目直至达标。
    ///
    /// 单条即超限时也会被淘汰（容量是硬上限）。
    pub fn put(&self, key: String, bytes: Vec<u8>) {
        let size = bytes.len() as u64;
        let last_used = self.tick();
        // file 存储：先落盘
        if let Some(dir) = &self.dir {
            let path = dir.join(safe_file_name(&key));
            if let Err(e) = fs::write(&path, &bytes) {
                eprintln!("[tie] 缓存写入失败（忽略）: {e}");
                return;
            }
        }
        let mut entries = self.entries.lock().unwrap();
        entries.insert(
            key,
            Entry {
                size,
                last_used,
                data: if self.storage == Storage::Memory { Some(bytes) } else { None },
            },
        );
        drop(entries);
        self.evict_to_capacity();
    }

    /// 读取条目（LRU 刷新访问序）。不存在或读取失败 → None。
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let last_used = self.tick();
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.get_mut(key)?;
        entry.last_used = last_used;
        match &entry.data {
            Some(bytes) => Some(bytes.clone()),
            None => {
                // file 存储：从磁盘读
                let dir = self.dir.as_ref()?;
                let path = dir.join(safe_file_name(key));
                fs::read(path).ok()
            }
        }
    }

    /// 是否已缓存该键（不刷新访问序）。
    ///
    /// 公开查询 API：供诊断/测试与后续增量编译复用（当前流水线未调用）。
    #[allow(dead_code)]
    pub fn contains(&self, key: &str) -> bool {
        self.entries.lock().unwrap().contains_key(key)
    }

    /// 已缓存条目数（公开查询 API，测试与扩展用）。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// 当前总字节数（公开查询 API，测试与扩展用）。
    #[allow(dead_code)]
    pub fn total_bytes(&self) -> u64 {
        self.entries
            .lock()
            .unwrap()
            .values()
            .map(|e| e.size)
            .sum()
    }

    /// 清空缓存（file 存储同时删除磁盘文件）。
    ///
    /// `work_dir`：pipeline 的并行工作目录（无论哪种存储技术都创建于
    /// `cache.path/work/`），在此一并删除，避免残留空壳目录。
    /// pipeline 在阶段间流转完成后调用，释放中间产物。
    pub fn clear(&self, work_dir: Option<&std::path::Path>) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(dir) = &self.dir {
            for key in entries.keys() {
                let _ = fs::remove_file(dir.join(safe_file_name(key)));
            }
        }
        entries.clear();
        drop(entries);
        // 工作目录整体删除（可能不存在，忽略）
        if let Some(work) = work_dir {
            let _ = fs::remove_dir_all(work);
            // 父目录（cache.path）若已空则一并删除，不留空壳；
            // 非空（如 file 存储尚有其它缓存文件）时 remove_dir 失败被忽略。
            if let Some(parent) = work.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
    }

    /// 超限淘汰：总字节数 > 容量时，反复删除 last_used 最小的条目。
    fn evict_to_capacity(&self) {
        let mut entries = self.entries.lock().unwrap();
        loop {
            let total: u64 = entries.values().map(|e| e.size).sum();
            if total <= self.capacity {
                break;
            }
            // 找最久未访问（last_used 最小）的条目
            let victim = entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            let Some(key) = victim else { break };
            if let Some(dir) = &self.dir {
                let _ = fs::remove_file(dir.join(safe_file_name(&key)));
            }
            entries.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Cache;

    fn memory_cache(size: u64) -> CachePool {
        CachePool::new(&Cache {
            size_bytes: size,
            storage: Storage::Memory,
            path: PathBuf::from(".tie-cache"),
        })
    }

    #[test]
    fn put_get_roundtrip() {
        let pool = memory_cache(1024);
        pool.put("prep:main".to_string(), b"hello".to_vec());
        assert!(pool.contains("prep:main"));
        assert_eq!(pool.get("prep:main"), Some(b"hello".to_vec()));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn 超限按lru淘汰() {
        let pool = memory_cache(10); // 容量 10 字节
        pool.put("a".to_string(), vec![0; 6]);
        pool.put("b".to_string(), vec![0; 6]); // 超限 → 淘汰 a
        assert!(!pool.contains("a"), "最久未访问的 a 应被淘汰");
        assert!(pool.contains("b"));
        // 再次写入 c → 淘汰 b
        pool.put("c".to_string(), vec![0; 6]);
        assert!(!pool.contains("b"));
        assert!(pool.contains("c"));
        assert!(pool.total_bytes() <= 10);
    }

    #[test]
    fn 访问刷新lru顺序() {
        let pool = memory_cache(12);
        pool.put("a".to_string(), vec![0; 6]);
        pool.put("b".to_string(), vec![0; 6]);
        // 访问 a → a 变新，b 变最久
        let _ = pool.get("a");
        pool.put("c".to_string(), vec![0; 6]); // 超限 → 淘汰 b
        assert!(pool.contains("a"), "刚访问过的 a 不应被淘汰");
        assert!(!pool.contains("b"));
        assert!(pool.contains("c"));
    }

    #[test]
    fn file存储落盘与读回() {
        let dir = std::env::temp_dir().join(format!("tie-cache-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let cache = Cache {
            size_bytes: 1024,
            storage: Storage::File,
            path: dir.clone(),
        };
        let pool = CachePool::new(&cache);
        pool.put("ir:main".to_string(), b"llvm ir text".to_vec());
        assert_eq!(pool.get("ir:main"), Some(b"llvm ir text".to_vec()));
        // 磁盘文件存在
        let file = dir.join(safe_file_name("ir:main"));
        assert!(file.is_file(), "file 存储应落盘");
        pool.clear(None);
        assert!(!file.exists(), "clear 应删除磁盘文件");
        let _ = fs::remove_dir_all(&dir);
    }
}
