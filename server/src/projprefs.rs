//! 按项目存的设置（目前只有引擎），放在 `~/.subo/projects/<项目 id>.json`。
//!
//! 不写进项目目录：项目库是用户的源码，会进 git、会传回 arXiv，不往里塞我们的文件。
//! 读不出来（没设过、文件坏了）都当默认值，不拦打开项目。

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectPrefs {
    /// `pdflatex` / `xelatex`；没有表示自动探测
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
}

fn file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

pub fn load(dir: &Path, id: &str) -> ProjectPrefs {
    fs::read(file(dir, id)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// 全是默认值时删掉文件，不留空壳
pub fn save(dir: &Path, id: &str, prefs: &ProjectPrefs) -> io::Result<()> {
    let f = file(dir, id);
    if *prefs == ProjectPrefs::default() {
        return match fs::remove_file(&f) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    fs::create_dir_all(dir)?;
    let tmp = f.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(prefs).map_err(io::Error::other)?)?;
    fs::rename(&tmp, &f)
}

/// 项目改名后跟着挪；没设过就什么都不做
pub fn rename(dir: &Path, old: &str, new: &str) {
    let _ = fs::rename(file(dir, old), file(dir, new));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_rename_and_reset() {
        let d = std::env::temp_dir().join(format!("subo-projprefs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        assert_eq!(load(&d, "a"), ProjectPrefs::default());
        let p = ProjectPrefs { engine: Some("pdflatex".into()) };
        save(&d, "a", &p).unwrap();
        assert_eq!(load(&d, "a"), p);
        rename(&d, "a", "b");
        assert_eq!(load(&d, "a"), ProjectPrefs::default());
        assert_eq!(load(&d, "b"), p);
        save(&d, "b", &ProjectPrefs::default()).unwrap();
        assert!(!file(&d, "b").exists());
        let _ = fs::remove_dir_all(&d);
    }
}
