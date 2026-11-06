//! 文件树右键菜单的文件操作：新建、重命名 / 移动、创建副本、删除（移到废纸篓）。
//! 路径一律是相对项目根的，经 [`project::resolve`] 挡掉 `..`、绝对路径和符号链接逃逸。

use crate::fail::Fail;
use crate::project;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 解析相对路径；空路径（项目根本身）不许动
fn target(root: &Path, rel: &str) -> Result<PathBuf, Fail> {
    let rel = rel.trim_matches(['/', '\\']);
    if rel.is_empty() {
        return Err(Fail::new("file.rootUntouchable", "不能对项目根目录做这个操作"));
    }
    if rel.split(['/', '\\']).any(|c| c.trim().is_empty() || c == ".") {
        return Err(Fail::new("file.badName", "文件名不合法").with("path", rel));
    }
    project::resolve(root, rel)
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn io_fail(code: &'static str, what: &str, e: std::io::Error) -> Fail {
    Fail::new(code, format!("{what}: {e}")).detail(e)
}

/// 新建空文件或文件夹；已存在就报错，不覆盖
pub fn create(root: &Path, rel: &str, dir: bool) -> Result<(), Fail> {
    let p = target(root, rel)?;
    if exists(&p) {
        return Err(Fail::new("file.exists", "已经有同名的文件或文件夹").with("path", rel));
    }
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| io_fail("file.createFailed", "新建失败", e))?;
    }
    let res = if dir {
        fs::create_dir(&p)
    } else {
        fs::OpenOptions::new().write(true).create_new(true).open(&p).map(|_| ())
    };
    res.map_err(|e| io_fail("file.createFailed", "新建失败", e))
}

/// 重命名或移动到别的目录；目标已存在就报错。只改大小写的改名在不分大小写的文件系统上也放行
pub fn rename(root: &Path, from: &str, to: &str) -> Result<(), Fail> {
    let src = target(root, from)?;
    let dst = target(root, to)?;
    if !exists(&src) {
        return Err(Fail::new("file.notFound", "文件不存在").with("path", from));
    }
    let same = src == dst;
    let case_only = !same && from.to_lowercase() == to.to_lowercase();
    if same {
        return Ok(());
    }
    if exists(&dst) && !case_only {
        return Err(Fail::new("file.exists", "已经有同名的文件或文件夹").with("path", to));
    }
    if dst.starts_with(&src) {
        return Err(Fail::new("file.intoItself", "不能把文件夹移到它自己里面").with("path", from));
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| io_fail("file.renameFailed", "重命名失败", e))?;
    }
    fs::rename(&src, &dst).map_err(|e| io_fail("file.renameFailed", "重命名失败", e))
}

/// 同目录下复制一份：`name 副本.ext`、`name 副本 2.ext`……返回新路径（相对项目根）
pub fn duplicate(root: &Path, rel: &str, suffix: &str) -> Result<String, Fail> {
    let src = target(root, rel)?;
    let meta = fs::symlink_metadata(&src).map_err(|_| Fail::new("file.notFound", "文件不存在").with("path", rel))?;
    let rel = rel.trim_matches(['/', '\\']);
    let (dir, name) = rel.rsplit_once('/').map(|(d, n)| (format!("{d}/"), n)).unwrap_or((String::new(), rel));
    // 文件夹和没有扩展名的、点开头的文件整个名字当主干
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !meta.is_dir() => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..1000 {
        let next = if n == 1 { format!("{dir}{stem} {suffix}{ext}") } else { format!("{dir}{stem} {suffix} {n}{ext}") };
        let dst = project::resolve(root, &next)?;
        if exists(&dst) {
            continue;
        }
        copy_all(&src, &dst).map_err(|e| {
            let _ = if dst.is_dir() { fs::remove_dir_all(&dst) } else { fs::remove_file(&dst) };
            io_fail("file.copyFailed", "复制失败", e)
        })?;
        return Ok(next);
    }
    Err(Fail::new("file.exists", "已经有同名的文件或文件夹").with("path", rel))
}

/// 递归复制；符号链接不跟进（和文件树一样防环、防逃逸）
fn copy_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() {
        fs::create_dir(dst)?;
        for e in fs::read_dir(src)?.flatten() {
            if e.file_type().map(|t| t.is_symlink()).unwrap_or(true) {
                continue;
            }
            copy_all(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else if meta.is_file() {
        fs::copy(src, dst).map(|_| ())
    } else {
        Ok(())
    }
}

/// 删除：移到系统的废纸篓 / 回收站，用户还能找回。系统那条路走不通时，
/// 退到项目里的 `.subo/trash/<时间>/` 下（文件树不列 `.subo`），也不真删
pub fn trash(root: &Path, rel: &str) -> Result<(), Fail> {
    let p = target(root, rel)?;
    if !exists(&p) {
        return Err(Fail::new("file.notFound", "文件不存在").with("path", rel));
    }
    if system_trash(&p) {
        return Ok(());
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let dst = root.join(".subo").join("trash").join(stamp.to_string()).join(rel.trim_matches(['/', '\\']));
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| io_fail("file.deleteFailed", "删除失败", e))?;
    }
    fs::rename(&p, &dst).map_err(|e| io_fail("file.deleteFailed", "删除失败", e))
}

#[cfg(target_os = "macos")]
fn system_trash(p: &Path) -> bool {
    // 直接挪进 ~/.Trash：不用让 Finder 代办，免得弹「想要控制访达」的授权框。
    // 跨卷（项目在移动硬盘上）挪不过去，交给调用方兜底
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return false };
    let bin = home.join(".Trash");
    let Some(name) = p.file_name().map(|n| n.to_string_lossy().to_string()) else { return false };
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !p.is_dir() => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    for n in 0..1000 {
        let dst = if n == 0 { bin.join(&name) } else { bin.join(format!("{stem} {n}{ext}")) };
        if exists(&dst) {
            continue;
        }
        return fs::rename(p, &dst).is_ok();
    }
    false
}

#[cfg(windows)]
fn system_trash(p: &Path) -> bool {
    use std::time::{Duration, Instant};
    // 路径走环境变量，不拼进脚本，免得引号、$ 之类被 PowerShell 当语法
    let kind = if p.is_dir() { "DeleteDirectory" } else { "DeleteFile" };
    let script = format!(
        "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::{kind}($env:SUBO_TRASH_PATH, 'OnlyErrorDialogs', 'SendToRecycleBin')"
    );
    let mut cmd = crate::engine::command("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]).env("SUBO_TRASH_PATH", p);
    crate::engine::run(&mut cmd, Instant::now() + Duration::from_secs(20)).is_ok_and(|r| r.ok) && !exists(p)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn system_trash(p: &Path) -> bool {
    use std::time::{Duration, Instant};
    let mut cmd = crate::engine::command("gio");
    cmd.arg("trash").arg("--").arg(p);
    crate::engine::run(&mut cmd, Instant::now() + Duration::from_secs(20)).is_ok_and(|r| r.ok) && !exists(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "subo-files-{tag}-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        dunce::canonicalize(&d).unwrap()
    }

    #[test]
    fn create_rename_duplicate() {
        let root = scratch("ops");
        create(&root, "a.tex", false).unwrap();
        assert_eq!(create(&root, "a.tex", false).unwrap_err().code, "file.exists");
        create(&root, "sec", true).unwrap();
        rename(&root, "a.tex", "sec/b.tex").unwrap();
        assert!(root.join("sec/b.tex").is_file());
        assert_eq!(rename(&root, "sec", "sec/inner").unwrap_err().code, "file.intoItself");
        assert_eq!(duplicate(&root, "sec/b.tex", "copy").unwrap(), "sec/b copy.tex");
        assert_eq!(duplicate(&root, "sec/b.tex", "copy").unwrap(), "sec/b copy 2.tex");
        assert_eq!(duplicate(&root, "sec", "copy").unwrap(), "sec copy");
        assert!(root.join("sec copy/b.tex").is_file());
        assert_eq!(target(&root, "").unwrap_err().code, "file.rootUntouchable");
        assert_eq!(target(&root, "../x").unwrap_err().code, "path.parent");
        let _ = fs::remove_dir_all(&root);
    }
}
