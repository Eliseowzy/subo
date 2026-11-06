//! 待取队列：离线时缺的宏包、查不到属于哪个包的文件先记下，联网后一键补齐（docs/architecture.md §4.2）。
//!
//! 进队列的两条路：编译后查包名时连不上（缺字清单里的 `unknown`），或者安装时连不上软件源。
//! 装上了（安装、导入字盘快照）就出队列；用户也可以手动移掉。
//!
//! 存在 `~/.subo/case/queue.json`，写入先写临时文件再改名。读不出来按空队列算。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 队列最多记这么多项（包和文件分别算），防的是日志解析出错时无限增长
const MAX_ITEMS: usize = 500;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Queue {
    /// 包名 → 要它的项目 id
    pub packages: BTreeMap<String, Vec<String>>,
    /// 还不知道属于哪个包的文件名 → 要它的项目 id；联网后先查包名
    pub files: BTreeMap<String, Vec<String>>,
    /// 最近一次变动（Unix 秒）
    pub updated: u64,
}

impl Queue {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.files.is_empty()
    }
}

pub struct Store {
    path: PathBuf,
    q: Queue,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn add(map: &mut BTreeMap<String, Vec<String>>, names: &[String], project: Option<&str>) -> bool {
    let mut changed = false;
    for n in names {
        if !map.contains_key(n) && map.len() >= MAX_ITEMS {
            break;
        }
        let who = map.entry(n.clone()).or_insert_with(|| {
            changed = true;
            Vec::new()
        });
        if let Some(p) = project.filter(|p| !who.iter().any(|w| w == p)) {
            who.push(p.to_string());
            changed = true;
        }
    }
    changed
}

fn remove(map: &mut BTreeMap<String, Vec<String>>, names: &[String]) -> bool {
    let before = map.len();
    map.retain(|k, _| !names.contains(k));
    map.len() != before
}

impl Store {
    pub fn open(path: PathBuf) -> Self {
        let q = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Store { path, q }
    }

    pub fn get(&self) -> Queue {
        self.q.clone()
    }

    pub fn add_packages(&mut self, pkgs: &[String], project: Option<&str>) {
        if add(&mut self.q.packages, pkgs, project) {
            self.touch();
        }
    }

    pub fn add_files(&mut self, files: &[String], project: Option<&str>) {
        if add(&mut self.q.files, files, project) {
            self.touch();
        }
    }

    /// 装上了、用户移掉了
    pub fn remove(&mut self, pkgs: &[String], files: &[String]) {
        let a = remove(&mut self.q.packages, pkgs);
        let b = remove(&mut self.q.files, files);
        if a || b {
            self.touch();
        }
    }

    /// 查到了包名的文件挪进包里，要它的项目跟着走；TeX Live 里没有的文件直接丢掉
    pub fn settle(&mut self, found: &[(String, Option<String>)]) {
        let mut changed = false;
        for (file, pkg) in found {
            let Some(who) = self.q.files.remove(file) else { continue };
            changed = true;
            if let Some(pkg) = pkg {
                let slot = self.q.packages.entry(pkg.clone()).or_default();
                for w in who {
                    if !slot.contains(&w) {
                        slot.push(w);
                    }
                }
            }
        }
        if changed {
            self.touch();
        }
    }

    fn touch(&mut self) {
        self.q.updated = now();
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_vec_pretty(&self.q) {
            if fs::write(&tmp, text).is_ok() {
                let _ = fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// 队列文件的位置：与缺字索引放在一起
pub fn default_path() -> PathBuf {
    crate::case::default_index_path().with_file_name("queue.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let d = std::env::temp_dir().join(format!(
            "subo-queue-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        Store::open(d.join("queue.json"))
    }

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn add_settle_remove_round_trip() {
        let mut s = store();
        s.add_packages(&v(&["xltxtra"]), Some("paper"));
        s.add_packages(&v(&["xltxtra"]), Some("thesis"));
        s.add_files(&v(&["phvr8t.tfm", "chap1.tex"]), Some("paper"));
        assert_eq!(s.get().packages["xltxtra"], v(&["paper", "thesis"]));

        // 查到包名的挪过去，TeX Live 没有的丢掉
        s.settle(&[("phvr8t.tfm".into(), Some("helvetic".into())), ("chap1.tex".into(), None)]);
        let q = s.get();
        assert!(q.files.is_empty());
        assert_eq!(q.packages["helvetic"], v(&["paper"]));

        // 落盘了，重新打开还在
        let again = Store::open(s.path.clone());
        assert_eq!(again.get().packages.len(), 2);

        s.remove(&v(&["xltxtra", "helvetic"]), &[]);
        assert!(s.get().is_empty());
    }
}
