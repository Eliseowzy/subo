//! 本地历史：没有协作、没有云端，「回到十分钟前」只能靠自己记。
//!
//! 保存文件时，磁盘上的旧内容距离上一份快照超过 [`INTERVAL`] 就先存一份；恢复、载入外部修改、
//! 自动修复这类整篇替换的操作前无条件存一份。和上一份一模一样的不重复存。
//!
//! 放在 `~/.subo/history/<项目路径哈希>/<文件路径哈希>/<毫秒时间戳>.txt`，同目录的 `path`
//! 记着文件的相对路径。不往项目里写东西（与编译产物同理）。每个文件最多留 [`KEEP`] 份，
//! 超出的删最旧的；超过 [`MAX_BYTES`] 的文件不记（多半是数据，不是手写的源码）。

use serde::Serialize;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 自动快照的最小间隔：防抖保存每 600 ms 一次，不能次次都存
pub const INTERVAL: Duration = Duration::from_secs(5 * 60);
/// 每个文件最多留这么多份
const KEEP: usize = 100;
/// 单份上限
const MAX_BYTES: usize = 4 << 20;

#[derive(Debug, Serialize, PartialEq)]
pub struct Version {
    /// 毫秒时间戳，也是版本的 id
    pub id: u64,
    pub size: u64,
}

pub struct History {
    dir: PathBuf,
}

fn hash_of(x: impl Hash) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    x.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl History {
    /// `base` 是 `~/.subo/history`，`root` 是项目的真实路径
    pub fn new(base: &Path, root: &Path) -> Self {
        History { dir: base.join(hash_of(root)) }
    }

    fn file_dir(&self, rel: &str) -> PathBuf {
        self.dir.join(hash_of(rel))
    }

    /// 由新到旧
    pub fn list(&self, rel: &str) -> Vec<Version> {
        let Ok(rd) = fs::read_dir(self.file_dir(rel)) else { return Vec::new() };
        let mut out: Vec<Version> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name();
                let id = name.to_str()?.strip_suffix(".txt")?.parse().ok()?;
                Some(Version { id, size: e.metadata().ok()?.len() })
            })
            .collect();
        out.sort_by(|a, b| b.id.cmp(&a.id));
        out
    }

    pub fn read(&self, rel: &str, id: u64) -> Option<String> {
        fs::read_to_string(self.file_dir(rel).join(format!("{id}.txt"))).ok()
    }

    /// 存一份。`force` 为 false 时离上一份不到 [`INTERVAL`] 就不存；内容和上一份相同永远不存。
    /// 返回存下的版本
    pub fn keep(&self, rel: &str, content: &str, force: bool) -> Option<u64> {
        if content.len() > MAX_BYTES {
            return None;
        }
        let versions = self.list(rel);
        if let Some(last) = versions.first() {
            if !force && now_ms().saturating_sub(last.id) < INTERVAL.as_millis() as u64 {
                return None;
            }
            if self.read(rel, last.id).as_deref() == Some(content) {
                return None;
            }
        }
        let dir = self.file_dir(rel);
        fs::create_dir_all(&dir).ok()?;
        let _ = fs::write(dir.join("path"), rel);
        // 同一毫秒里存两份（强制存紧跟自动存）时往后挪一毫秒，不覆盖
        let mut id = now_ms().max(versions.first().map_or(0, |v| v.id + 1));
        while dir.join(format!("{id}.txt")).exists() {
            id += 1;
        }
        let tmp = dir.join(format!("{id}.tmp"));
        fs::write(&tmp, content).ok()?;
        fs::rename(&tmp, dir.join(format!("{id}.txt"))).ok()?;
        for old in versions.iter().skip(KEEP - 1) {
            let _ = fs::remove_file(dir.join(format!("{}.txt", old.id)));
        }
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PathBuf {
        std::env::temp_dir().join(format!(
            "subo-history-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    #[test]
    fn keeps_spaced_distinct_versions() {
        let h = History::new(&base(), Path::new("/p/paper"));
        assert!(h.keep("main.tex", "v1", false).is_some());
        // 间隔不够：不存
        assert!(h.keep("main.tex", "v2", false).is_none());
        // 强制：存；和上一份相同：不存
        let v2 = h.keep("main.tex", "v2", true).unwrap();
        assert!(h.keep("main.tex", "v2", true).is_none());
        let list = h.list("main.tex");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, v2);
        assert_eq!(h.read("main.tex", v2).as_deref(), Some("v2"));
        // 别的文件、别的项目互不相干
        assert!(h.list("intro.tex").is_empty());
        assert!(History::new(&h.dir.parent().unwrap().to_path_buf(), Path::new("/p/other")).list("main.tex").is_empty());
    }

    #[test]
    fn prunes_oldest_beyond_cap() {
        let h = History::new(&base(), Path::new("/p/x"));
        for i in 0..(KEEP + 5) {
            h.keep("a.tex", &format!("v{i}"), true).unwrap();
        }
        let list = h.list("a.tex");
        assert_eq!(list.len(), KEEP);
        assert_eq!(h.read("a.tex", list[0].id).as_deref(), Some(format!("v{}", KEEP + 4).as_str()));
    }
}
