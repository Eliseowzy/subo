//! 已打开的本地文件夹：用户在磁盘上原有的 LaTeX 项目，原地编辑，不复制进项目库。
//!
//! 清单存在 `<subo 目录>/folders.json`。Subo 只改里面的文件内容，不改名、不删目录；
//! 「移出列表」只删清单里这一条。id 以 `@` 开头，项目库的目录名永远不会这样起，两边不撞。

use crate::fail::Fail;
use crate::library;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub path: PathBuf,
    /// 旧清单里没有这一项，读出来当没归档
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
}

pub struct Folders {
    file: PathBuf,
    list: Vec<Folder>,
}

pub fn is_folder_id(id: &str) -> bool {
    id.starts_with('@')
}

impl Folders {
    /// 读不出来（首次使用、文件坏了）就当空清单，不拦启动
    pub fn open(file: PathBuf) -> Self {
        let list = fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Folders { file, list }
    }

    pub fn all(&self) -> &[Folder] {
        &self.list
    }

    pub fn get(&self, id: &str) -> Option<&Folder> {
        self.list.iter().find(|f| f.id == id)
    }

    /// 登记一个文件夹，返回它的 id；已登记过的原样返回。
    pub fn add(&mut self, path: &Path) -> Result<String, Fail> {
        let path = dunce::canonicalize(&path).map_err(|_| Fail::new("folder.notFound", "找不到这个文件夹"))?;
        if !path.is_dir() {
            return Err(Fail::new("folder.notDir", "这不是一个文件夹"));
        }
        if let Some(f) = self.list.iter().find(|f| f.path == path) {
            return Ok(f.id.clone());
        }
        if !has_tex(&path, 3) {
            return Err(Fail::new("folder.noTex", "这个文件夹里没有 .tex 文件"));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let base = format!("@{}", library::slug(&name));
        let taken = |c: &str| self.list.iter().any(|f| f.id == c);
        let id = if !taken(&base) {
            base
        } else {
            (2..).map(|n| format!("{base}-{n}")).find(|c| !taken(c)).unwrap()
        };
        self.list.push(Folder { id: id.clone(), path, archived: false });
        self.save()?;
        Ok(id)
    }

    /// 返回 `false` 表示清单里没有这个 id
    pub fn set_archived(&mut self, id: &str, archived: bool) -> Result<bool, Fail> {
        let Some(f) = self.list.iter_mut().find(|f| f.id == id) else { return Ok(false) };
        if f.archived != archived {
            f.archived = archived;
            self.save()?;
        }
        Ok(true)
    }

    pub fn remove(&mut self, id: &str) -> Result<bool, Fail> {
        let before = self.list.len();
        self.list.retain(|f| f.id != id);
        if self.list.len() == before {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    /// 先写临时文件再改名，写到一半断电也不会留下半个 JSON
    fn save(&self) -> Result<(), Fail> {
        if let Some(dir) = self.file.parent() {
            fs::create_dir_all(dir).map_err(|e| Fail::new("folder.configDir", format!("无法创建配置目录: {e}")).detail(e))?;
        }
        let tmp = self.file.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(&self.list).unwrap_or_default();
        let failed = |e: std::io::Error| Fail::new("folder.saveList", format!("保存文件夹清单失败: {e}")).detail(e);
        fs::write(&tmp, body).map_err(failed)?;
        fs::rename(&tmp, &self.file).map_err(failed)
    }
}

/// 只往下看几层：用户误选了整个主目录时别把磁盘扫一遍
fn has_tex(dir: &Path, depth: u32) -> bool {
    let Ok(rd) = fs::read_dir(dir) else { return false };
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => subdirs.push(p),
            Ok(t) if t.is_file() && name.ends_with(".tex") => return true,
            _ => {}
        }
    }
    depth > 0 && subdirs.iter().any(|d| has_tex(d, depth - 1))
}

/// 弹系统的「选择文件夹」对话框。`Ok(None)` 表示用户取消；
/// 不支持的系统返回 `Err`，前端退回到手动粘贴路径。
pub fn pick() -> Result<Option<PathBuf>, Fail> {
    if !cfg!(target_os = "macos") {
        return Err(Fail::new("folder.pickUnsupported", "这个系统上还不能弹出选择框，请直接粘贴文件夹路径"));
    }
    let script = r#"POSIX path of (choose folder with prompt "选择一个 LaTeX 项目文件夹")"#;
    // 对话框等人点，给足时间
    let deadline = Instant::now() + Duration::from_secs(600);
    let ran = crate::engine::run(crate::engine::command("osascript").args(["-e", script]), deadline)
        .map_err(|e| Fail::new("folder.pickOpen", format!("无法打开选择框: {e}")).detail(e))?;
    if !ran.ok {
        // 取消时 osascript 报 -128
        return if ran.log.contains("-128") || ran.timed_out {
            Ok(None)
        } else {
            Err(Fail::new("folder.pickFailed", format!("选择框出错: {}", ran.log.trim())).detail(ran.log.trim()))
        };
    }
    let path = ran.log.trim();
    Ok((!path.is_empty()).then(|| PathBuf::from(path)))
}

/// 在 Finder（或系统文件管理器）里显示这个目录。
pub fn reveal(path: &Path) -> Result<(), Fail> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = crate::engine::command("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = crate::engine::command("explorer");
        c.arg(path);
        c
    } else {
        let mut c = crate::engine::command("xdg-open");
        c.arg(path);
        c
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    crate::engine::run(&mut cmd, deadline).map(|_| ()).map_err(|e| Fail::new("folder.revealFailed", format!("无法打开文件管理器: {e}")).detail(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "subo-folders-{tag}-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn add_dedups_and_persists() {
        let d = scratch("add");
        for name in ["a/paper", "b/paper", "empty"] {
            fs::create_dir_all(d.join(name)).unwrap();
        }
        fs::write(d.join("a/paper/main.tex"), "x").unwrap();
        fs::create_dir_all(d.join("b/paper/ch")).unwrap();
        fs::write(d.join("b/paper/ch/one.tex"), "x").unwrap();

        let file = d.join("cfg/folders.json");
        let mut f = Folders::open(file.clone());
        assert_eq!(f.add(&d.join("a/paper")).unwrap(), "@paper");
        assert_eq!(f.add(&d.join("a/paper/")).unwrap(), "@paper");
        assert_eq!(f.add(&d.join("b/paper")).unwrap(), "@paper-2");
        assert!(f.add(&d.join("empty")).is_err());
        assert!(f.add(&d.join("nope")).is_err());

        let again = Folders::open(file.clone());
        assert_eq!(again.all().len(), 2);
        let mut again = again;
        assert!(again.set_archived("@paper-2", true).unwrap());
        assert!(!again.set_archived("@nope", true).unwrap());
        assert!(Folders::open(file.clone()).get("@paper-2").unwrap().archived);
        assert!(again.remove("@paper").unwrap());
        assert!(!again.remove("@paper").unwrap());
        assert_eq!(Folders::open(file).all().len(), 1);
        // 移出列表不碰磁盘
        assert!(d.join("a/paper/main.tex").is_file());
        let _ = fs::remove_dir_all(&d);
    }
}
