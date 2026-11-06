//! 项目库：`~/Documents/Subo/` 下每个子目录是一个项目（quire）。
//! 负责列出、新建、改名、从压缩包（zip / arXiv 的 tar.gz）导入。单个项目内部的事归 `project`。

use crate::fail::Fail;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 解压后总大小上限。论文项目带图也很少过百 MB，超了多半是误传或 zip 炸弹。
const UNZIP_MAX_BYTES: u64 = 500 << 20;
const UNZIP_MAX_FILES: u64 = 10_000;

#[derive(Serialize)]
pub struct Summary {
    pub id: String,
    /// 显示名：项目库里就是 id，本地文件夹是它的目录名
    pub name: String,
    pub path: String,
    pub entry: Option<String>,
    /// 最近修改时间（Unix 秒），用于列表排序
    pub modified: u64,
    /// 用户磁盘上原有的文件夹，原地编辑，不在项目库里
    pub folder: bool,
    /// 文件夹不见了（被移走、移动硬盘没插）
    pub missing: bool,
    /// 已归档：首页默认不列，在「已归档」里能找回，照样能打开
    pub archived: bool,
}

/// 项目库里的项目靠目录里这个空文件记归档：改名、换库位置都跟着目录走，
/// 不用另记一份 id 清单。点开头，文件树、模板复制都会跳过它。本地文件夹不往里写，记在清单里。
pub const ARCHIVED_MARK: &str = ".subo-archived";

pub fn is_archived(root: &Path) -> bool {
    root.join(ARCHIVED_MARK).is_file()
}

/// 给项目库里的项目归档或取消归档。
pub fn set_archived(home: &Path, id: &str, archived: bool) -> Result<(), Fail> {
    let dir = home.join(id);
    if !valid_id(id) || crate::folders::is_folder_id(id) || !dir.is_dir() {
        return Err(Fail::new("project.notFound", "没有这个项目"));
    }
    let mark = dir.join(ARCHIVED_MARK);
    let res = if archived {
        fs::write(&mark, b"")
    } else {
        match fs::remove_file(&mark) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    };
    res.map_err(|e| Fail::new("library.archiveFailed", format!("归档状态保存失败: {e}")).detail(e))
}

/// Subo 自己的配置与缓存：`SUBO_HOME`，否则 `~/.subo`。用户不需要来这里找东西。
pub fn subo_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SUBO_HOME") {
        return PathBuf::from(dir);
    }
    let Some(home) = home_dir() else { return PathBuf::from(".subo") };
    static RENAMED: std::sync::Once = std::sync::Once::new();
    RENAMED.call_once(|| migrate_old_name(&home));
    home.join(".subo")
}

/// 产品改名前叫 Stet：配置与缓存在 `~/.stet`，项目库在 `~/Documents/Stet`。
/// 新目录还没有时整个改名过去；新目录已存在就不动，免得两边混在一起。
/// 放在 `subo_dir` 第一次调用时做：桌面壳在起服务之前就要用它配 TeX 用户层。
fn migrate_old_name(home: &Path) {
    let docs = home.join("Documents");
    for (old, new) in [(home.join(".stet"), home.join(".subo")), (docs.join("Stet"), docs.join("Subo"))] {
        if old.is_dir() && !new.exists() && fs::rename(&old, &new).is_ok() {
            eprintln!("  已从旧名目录 {} 搬到 {}", old.display(), new.display());
        }
    }
}

/// 项目库放在用户看得见的地方：`~/Documents/Subo`。
/// 设了 `SUBO_HOME` 时跟着它走（测试、多实例隔离）。
pub fn default_home() -> PathBuf {
    if std::env::var_os("SUBO_HOME").is_some() {
        return subo_dir().join("quires");
    }
    documents_dir().map(|d| d.join("Subo")).unwrap_or_else(|| subo_dir().join("quires"))
}

/// 用户主目录：Unix 读 `HOME`，Windows 读 `USERPROFILE`
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// 「文档」目录。Windows 上它可能被重定向到 OneDrive 或域策略指定的位置，向系统要真实路径
fn documents_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(d) = known_documents() {
        return Some(d);
    }
    home_dir().map(|h| h.join("Documents"))
}

#[cfg(windows)]
fn known_documents() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    // SAFETY: 成功时系统分配一段以 0 结尾的宽字符串，读完用 CoTaskMemFree 释放；失败时也要释放
    unsafe {
        let mut p: *mut u16 = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT as u32, std::ptr::null_mut(), &mut p);
        let out = (hr >= 0 && !p.is_null()).then(|| {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(p, len)))
        });
        CoTaskMemFree(p as *const _);
        out
    }
}

/// 早期版本把项目库放在隐藏的 `~/.subo/quires`。新位置还没有时整个搬过去，
/// 返回搬动的旧路径；新位置已存在就不动，免得两边混在一起。
pub fn migrate_legacy(home: &Path) -> Option<PathBuf> {
    let old = subo_dir().join("quires");
    if old == home || !old.is_dir() || home.exists() {
        return None;
    }
    if let Some(parent) = home.parent() {
        fs::create_dir_all(parent).ok()?;
    }
    fs::rename(&old, home).ok().map(|_| old)
}

/// 项目名最多几个字。中文一字 3 字节，40 字约 120 字节，离文件名 255 字节的上限还远，
/// 列表里也放得下。前端输入框用同一个数。
pub const NAME_MAX_CHARS: usize = 40;

/// 项目 id 就是目录名：不能含路径分隔符，不能以点开头（点开头留给临时目录）。
/// `@` 开头的是本地文件夹（见 `folders`），在项目库里不认。
/// 长度只按文件系统上限卡（255 字节）：这里卡严了，磁盘上已有的长名目录会从列表里消失，
/// 用户连改回来的入口都没有。名字长短由 `slug` 在起名时管。
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && !id.starts_with('.')
        && !id.contains(['/', '\\', '\0'])
}

pub fn list(home: &Path) -> Vec<Summary> {
    let Ok(rd) = fs::read_dir(home) else { return Vec::new() };
    let mut out: Vec<Summary> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().to_string();
            (valid_id(&id) && !crate::folders::is_folder_id(&id))
                .then(|| summary(&id, &id, &e.path(), false))
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

pub fn summary(id: &str, name: &str, root: &Path, folder: bool) -> Summary {
    let missing = !root.is_dir();
    Summary {
        id: id.to_string(),
        name: name.to_string(),
        path: root.to_string_lossy().to_string(),
        entry: if missing { None } else { crate::project::entry(root) },
        modified: if missing { 0 } else { modified(root) },
        folder,
        missing,
        // 本地文件夹的归档记在清单里，由调用方填
        archived: !folder && !missing && is_archived(root),
    }
}

/// 目录本身的 mtime 不反映子文件改动，取顶层 .tex 里最新的一个。
fn modified(root: &Path) -> u64 {
    let secs = |p: &Path| {
        fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };
    let mut best = secs(root);
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with(".tex") {
                best = best.max(secs(&e.path()));
            }
        }
    }
    best
}

/// 新建、改名时检查用户输入的名字。导入 zip、打开文件夹用的是现成的文件名，不走这里，
/// 由 `slug` 截断。
pub fn check_name(name: &str) -> Result<(), Fail> {
    let n = name.trim().chars().count();
    if n > NAME_MAX_CHARS {
        return Err(Fail::new("name.tooLong", format!("名称太长（{n} 字），最多 {NAME_MAX_CHARS} 字"))
            .with("count", n)
            .with("max", NAME_MAX_CHARS));
    }
    Ok(())
}

/// 用户起的名字 → 目录名。保留中文等 Unicode 字母数字，空白变 `-`，其余丢掉，
/// 最多 `NAME_MAX_CHARS` 字。
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
            s.push(c);
        } else if c.is_whitespace() && !s.ends_with('-') {
            s.push('-');
        }
    }
    let s: String = s.trim_matches(['-', '.']).chars().take(NAME_MAX_CHARS).collect();
    let s = s.trim_end_matches(['-', '.']);
    if s.is_empty() {
        "untitled".into()
    } else {
        s.to_string()
    }
}

/// 同名就加 `-2`、`-3`……
pub(crate) fn unique(home: &Path, base: &str) -> String {
    if !home.join(base).exists() {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !home.join(c).exists())
        .unwrap()
}

pub(crate) fn template(title: &str) -> String {
    let title = title.replace(['\\', '{', '}', '%', '#', '$', '&', '_', '^', '~'], "");
    format!(
        "\\documentclass[UTF8]{{ctexart}}\n\
         \\usepackage{{amsmath}}\n\
         \n\
         \\title{{{title}}}\n\
         \\author{{}}\n\
         \\date{{\\today}}\n\
         \n\
         \\begin{{document}}\n\
         \\maketitle\n\
         \n\
         从这里开始写。\n\
         \n\
         \\end{{document}}\n"
    )
}

pub fn create(home: &Path, name: &str) -> Result<String, Fail> {
    check_name(name)?;
    fs::create_dir_all(home).map_err(|e| Fail::new("library.createHome", format!("无法创建项目库: {e}")).detail(e))?;
    let id = unique(home, &slug(name));
    let dir = home.join(&id);
    fs::create_dir(&dir).map_err(|e| Fail::new("library.createDir", format!("无法创建项目目录: {e}")).detail(e))?;
    fs::write(dir.join("main.tex"), template(name.trim()))
        .map_err(|e| Fail::new("library.writeTemplate", format!("写入 main.tex 失败: {e}")).detail(e))?;
    Ok(id)
}

/// 改名就是给目录改名，新 id 同样走 slug 与去重。名字落到同一个 slug 时原样返回；
/// 只差大小写时（macOS 默认文件系统不分大小写）目标「已存在」的就是自己，直接改。
pub fn rename(home: &Path, id: &str, name: &str) -> Result<String, Fail> {
    let src = home.join(id);
    if !valid_id(id) || !src.is_dir() {
        return Err(Fail::new("project.notFound", "没有这个项目"));
    }
    check_name(name)?;
    let base = slug(name);
    if base == id {
        return Ok(base);
    }
    let id = if same_dir(&src, &home.join(&base)) { base } else { unique(home, &base) };
    fs::rename(&src, home.join(&id)).map_err(|e| Fail::new("library.renameFailed", format!("改名失败: {e}")).detail(e))?;
    Ok(id)
}

#[cfg(unix)]
fn same_dir(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_dir(a: &Path, b: &Path) -> bool {
    matches!((dunce::canonicalize(&a), dunce::canonicalize(&b)), (Ok(x), Ok(y)) if x == y)
}

/// 把上传的压缩包解成一个新项目：Overleaf 导出的 zip，或 arXiv 下载的源码包（tar.gz）。
/// 全程在项目库下的点开头临时目录里做，成功才改名上架，失败整个删掉——半解压的项目比没有更糟。
pub fn import_archive(home: &Path, name: &str, bytes: &[u8]) -> Result<String, Fail> {
    fs::create_dir_all(home).map_err(|e| Fail::new("library.createHome", format!("无法创建项目库: {e}")).detail(e))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = home.join(format!(".incoming-{nanos}"));

    let res = (|| {
        fs::create_dir(&tmp).map_err(|e| Fail::new("import.tempDir", format!("创建临时目录失败: {e}")).detail(e))?;
        unpack(bytes, &tmp)?;
        let root = strip_single_top(&tmp);
        // 从别人那儿拿来的包，进库时不该是归档状态
        let _ = fs::remove_file(root.join(ARCHIVED_MARK));
        if !has_tex(&root) {
            return Err(Fail::new("import.noTex", "压缩包里没有 .tex 文件"));
        }
        let id = unique(home, &slug(name));
        fs::rename(&root, home.join(&id)).map_err(|e| Fail::new("import.store", format!("落盘失败: {e}")).detail(e))?;
        Ok(id)
    })();

    let _ = fs::remove_dir_all(&tmp);
    res
}

/// 按内容认格式解到 `dest`（已存在的空目录），再清掉链接和打包垃圾。
/// 不看扩展名：arXiv 下载的源码包常常没有扩展名。项目导入和模板上传共用。
pub(crate) fn unpack(bytes: &[u8], dest: &Path) -> Result<(), Fail> {
    if bytes.starts_with(b"PK") {
        unzip(bytes, dest)?;
    } else {
        crate::tarball::extract(bytes, dest, UNZIP_MAX_BYTES, UNZIP_MAX_FILES)?;
    }
    sanitize(dest).map_err(|e| Fail::new("import.sanitize", format!("整理解压结果失败: {e}")).detail(e))
}

/// 解 zip。自己解，不调系统 `unzip`：Windows 上没有它。
/// 先看清单里的总大小、文件数挡住炸弹，解的时候再按实际字节数封顶（清单里的大小可以造假）；
/// 带 `..`、绝对路径的项和符号链接一律跳过。
fn unzip(bytes: &[u8], dest: &Path) -> Result<(), Fail> {
    let failed = |e: &dyn std::fmt::Display| Fail::new("import.unzipFailed", format!("解压失败: {e}")).with("detail", e.to_string());
    let mut zip = zip::ZipArchive::new(io::Cursor::new(bytes)).map_err(|_| Fail::new("import.notZip", "不是有效的 zip 文件"))?;
    let files = zip.len() as u64;
    if files > UNZIP_MAX_FILES {
        return Err(Fail::new("import.tooMany", format!("{files} 个文件，超过上限 {UNZIP_MAX_FILES}"))
            .with("count", files)
            .with("max", UNZIP_MAX_FILES));
    }
    let too_big = |bytes: u64| {
        let (mb, max) = (bytes >> 20, UNZIP_MAX_BYTES >> 20);
        Fail::new("import.tooBig", format!("解压后 {mb} MB，超过上限 {max} MB")).with("mb", mb).with("max", max)
    };
    let declared: u64 = (0..zip.len()).filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.size())).sum();
    if declared > UNZIP_MAX_BYTES {
        return Err(too_big(declared));
    }
    let mut written = 0u64;
    let mut names = EntryNames::default();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| failed(&e))?;
        let Some(rel) = f.enclosed_name() else { continue };
        if f.is_symlink() {
            continue;
        }
        let target = dest.join(&rel);
        if f.is_dir() {
            fs::create_dir_all(&target).map_err(|e| failed(&e))?;
            continue;
        }
        names.check(&rel)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| failed(&e))?;
        }
        let mut out = fs::File::create(&target).map_err(|e| failed(&e))?;
        let room = UNZIP_MAX_BYTES - written;
        let n = io::copy(&mut io::Read::take(&mut f, room + 1), &mut out).map_err(|e| failed(&e))?;
        written += n;
        if n > room {
            return Err(too_big(written));
        }
    }
    Ok(())
}

/// 压缩包里的文件名，解之前先过一遍：
/// - 只差大小写的两项（`Fig.png` 和 `fig.png`）在 Windows、macOS 默认的文件系统上是同一个文件，
///   后一个会悄悄盖掉前一个，所以直接报错，各平台一致；同一个名字出现两次照旧以后一个为准
/// - Windows 上不能用的设备名（`aux.tex`、`con`）建不出来，报清楚是哪个名字
#[derive(Default)]
pub(crate) struct EntryNames(std::collections::HashMap<String, String>);

impl EntryNames {
    /// `rel` 是相对解压目录的路径
    pub(crate) fn check(&mut self, rel: &Path) -> Result<(), Fail> {
        if cfg!(windows) {
            if let Some(name) = rel.iter().filter_map(|c| c.to_str()).find(|c| crate::project::reserved_on_windows(c)) {
                return Err(Fail::new("import.reserved", format!("压缩包里有 {name}：它是 Windows 的保留名，不能用作文件名")).with("name", name));
            }
        }
        let shown = rel.iter().map(|c| c.to_string_lossy()).collect::<Vec<_>>().join("/");
        match self.0.entry(shown.to_lowercase()) {
            std::collections::hash_map::Entry::Occupied(e) if *e.get() != shown => {
                let first = e.get().clone();
                Err(Fail::new("import.caseClash", format!("压缩包里 {first} 和 {shown} 只差大小写，解出来会互相覆盖"))
                    .with("a", first)
                    .with("b", shown))
            }
            std::collections::hash_map::Entry::Occupied(_) => Ok(()),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(shown);
                Ok(())
            }
        }
    }
}

/// 删掉符号链接与 macOS 打包垃圾。项目里不该有指向外面的东西。
fn sanitize(dir: &Path) -> io::Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let path = e.path();
        let name = e.file_name();
        let ft = e.file_type()?;
        if ft.is_symlink() || name == ".DS_Store" {
            fs::remove_file(&path)?;
        } else if ft.is_dir() {
            if name == "__MACOSX" {
                fs::remove_dir_all(&path)?;
            } else {
                sanitize(&path)?;
            }
        }
    }
    Ok(())
}

/// GitHub 下载、手工压缩的 zip 常常整体包了一层目录；只有一层时剥掉。
pub(crate) fn strip_single_top(dir: &Path) -> PathBuf {
    let entries: Vec<_> = fs::read_dir(dir).map(|d| d.flatten().collect()).unwrap_or_default();
    match entries.as_slice() {
        [only] if only.file_type().map(|t| t.is_dir()).unwrap_or(false) => only.path(),
        _ => dir.to_path_buf(),
    }
}

fn has_tex(dir: &Path) -> bool {
    let Ok(rd) = fs::read_dir(dir) else { return false };
    rd.flatten().any(|e| {
        let p = e.path();
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            has_tex(&p)
        } else {
            p.extension().is_some_and(|x| x == "tex")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "subo-lib-{tag}-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            io::Write::write_all(&mut w, data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn unzip_keeps_tree_and_skips_escapes() {
        let d = scratch("unzip");
        let bytes = zip_of(&[("main.tex", b"hi"), ("ch/one.tex", b"1"), ("../evil.tex", b"x"), ("/abs.tex", b"x")]);
        unpack(&bytes, &d).unwrap();
        assert_eq!(fs::read(d.join("main.tex")).unwrap(), b"hi");
        assert_eq!(fs::read(d.join("ch/one.tex")).unwrap(), b"1");
        assert!(!d.parent().unwrap().join("evil.tex").exists());
        // 绝对路径去掉开头的 `/` 落在项目里，和 tar 的处理一致
        assert!(d.join("abs.tex").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unzip_rejects_case_clash() {
        let d = scratch("unzip-case");
        let bytes = zip_of(&[("main.tex", b"x"), ("fig/Plot.png", b"1"), ("Fig/plot.png", b"2")]);
        let err = unpack(&bytes, &d).unwrap_err();
        assert_eq!(err.code, "import.caseClash");
        assert_eq!(err.params["a"], "fig/Plot.png");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unzip_rejects_garbage() {
        let d = scratch("unzip-bad");
        let err = unpack(b"PK\x03\x04not really a zip", &d).unwrap_err();
        assert_eq!(err.code, "import.notZip");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn slug_keeps_cjk_and_drops_separators() {
        assert_eq!(slug("  我的 论文 / v2  "), "我的-论文-v2");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("..."), "untitled");
        assert_eq!(slug(""), "untitled");
        assert_eq!(slug(&"长".repeat(100)).chars().count(), NAME_MAX_CHARS);
        // 截断处正好落在分隔符上，别留个尾巴
        assert_eq!(slug(&format!("{} b", "a".repeat(NAME_MAX_CHARS - 1))), "a".repeat(NAME_MAX_CHARS - 1));
    }

    #[test]
    fn long_names_rejected_but_old_long_dirs_still_listed() {
        let home = scratch("long");
        let long = "王".repeat(NAME_MAX_CHARS + 1);
        assert!(create(&home, &long).is_err());
        create(&home, "a").unwrap();
        assert!(rename(&home, "a", &long).is_err());
        assert!(home.join("a").is_dir());
        // 早期版本能起出 60 个汉字（180 字节）的目录，列表里要看得见、改得回来
        let old = "王".repeat(60);
        fs::create_dir(home.join(&old)).unwrap();
        assert!(list(&home).iter().any(|p| p.id == old));
        assert_eq!(rename(&home, &old, "简历").unwrap(), "简历");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn ids_reject_paths_and_hidden() {
        assert!(valid_id("论文"));
        assert!(!valid_id(".incoming-1"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(""));
    }

    #[test]
    fn create_dedups_names() {
        let home = scratch("create");
        assert_eq!(create(&home, "paper").unwrap(), "paper");
        assert_eq!(create(&home, "paper").unwrap(), "paper-2");
        let tex = fs::read_to_string(home.join("paper/main.tex")).unwrap();
        assert!(tex.contains("\\title{paper}"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn rename_slugs_and_dedups() {
        let home = scratch("rename");
        create(&home, "a").unwrap();
        create(&home, "b").unwrap();
        assert_eq!(rename(&home, "a", "a").unwrap(), "a");
        assert_eq!(rename(&home, "a", "我的 论文").unwrap(), "我的-论文");
        assert!(home.join("我的-论文/main.tex").is_file());
        assert!(!home.join("a").exists());
        assert_eq!(rename(&home, "b", "我的论文/").unwrap(), "我的论文");
        assert_eq!(rename(&home, "我的论文", "我的 论文").unwrap(), "我的-论文-2");
        assert!(rename(&home, "nope", "x").is_err());
        assert!(rename(&home, "../x", "x").is_err());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn archive_mark_follows_rename() {
        let home = scratch("archive");
        create(&home, "a").unwrap();
        assert!(!list(&home)[0].archived);
        set_archived(&home, "a", true).unwrap();
        set_archived(&home, "a", true).unwrap();
        assert!(list(&home)[0].archived);
        let id = rename(&home, "a", "b").unwrap();
        assert!(list(&home)[0].archived);
        set_archived(&home, &id, false).unwrap();
        set_archived(&home, &id, false).unwrap();
        assert!(!list(&home)[0].archived);
        assert!(set_archived(&home, "nope", true).is_err());
        assert!(set_archived(&home, "../x", true).is_err());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn imports_arxiv_tarball() {
        let home = scratch("arxiv");
        let src = scratch("arxiv-src");
        fs::write(src.join("ms.tex"), "\\documentclass{article}\n").unwrap();
        fs::write(src.join("appendix.tex"), "\\documentclass{article}\n").unwrap();
        fs::write(src.join("00README.json"), r#"{"sources":[{"filename":"ms.tex","usage":"toplevel"}]}"#).unwrap();
        let tgz = src.with_extension("tar.gz");
        let ok = crate::engine::command("tar").arg("czf").arg(&tgz).arg("-C").arg(&src).arg(".").status().unwrap();
        assert!(ok.success());
        let id = import_archive(&home, "2401.00001", &fs::read(&tgz).unwrap()).unwrap();
        assert_eq!(crate::project::entry(&home.join(&id)).as_deref(), Some("ms.tex"));
        // 临时目录不留
        assert!(fs::read_dir(&home).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.')));
        assert_eq!(import_archive(&home, "x", b"not an archive").unwrap_err().code, "import.notArchive");
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_file(&tgz);
    }

    /// 空白项目同理（见 presets 的 `presets_detect_as_xelatex_without_pin`）：没记 pin 的老项目靠 ctexart 认作 xelatex
    #[test]
    fn blank_project_detects_as_xelatex() {
        use crate::engine::program::{detect_program, Program};
        let home = scratch("detect");
        let id = create(&home, "老项目").unwrap();
        assert_eq!(detect_program(&home.join(&id), "main.tex").map(|d| d.program), Some(Program::Xelatex));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn template_strips_tex_specials() {
        assert!(template("a_b{c}\\d").contains("\\title{abcd}"));
    }
}
