//! 字盘快照（docs/architecture.md §4.3）：把用户层里补装的宏包打成一个 `.subocase` 文件，
//! 带到离线的机器上导入即可编——不是「碰巧缓存过」，而是可以确定地搬运。
//!
//! 格式是 gzip 过的 tar：`subo-case.json`（清单）+ `texmf/<相对用户层根的路径>`。
//! 清单记 TeX Live 年份、每个包的文件和 SHA-256。导入时年份不同拒收（不同年份的宏包和
//! 格式文件不保证兼容），逐个核对大小和哈希，全部对上才往用户层写；写每个文件都先写临时名再改名。
//!
//! 只管内置 TeX 的用户层（`TEXMFHOME`）。导入的包按文件登记（与不可重定位包同一套清单），
//! 设置页照样能看到、能卸载。

use crate::case;
use crate::fail::Fail;
use crate::tarball;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const EXT: &str = "subocase";
const MANIFEST: &str = "subo-case.json";
const FORMAT: &str = "subo-case";
const VERSION: u32 = 1;
/// tar 里用户层文件的前缀
const TREE: &str = "texmf";
/// 导入时解出来的总量与文件数上限：完整的 cm-super 也就七十来 MB，四 GB 远够
pub const IMPORT_MAX_BYTES: u64 = 4 << 30;
const IMPORT_MAX_FILES: u64 = 100_000;
/// 用户层里这些不是宏包文件：配置、数据库、索引。快照里出现就拒收，免得改掉用户层的配置
const RESERVED: &[&str] = &["tlpkg", "web2c", "ls-R"];

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    /// 导出时本机 TeX Live 的年份；查不到为 None（导入时不比）
    pub texlive: Option<u32>,
    /// Unix 秒
    pub created: u64,
    /// 从哪个项目导出的（显示名）；导出全部时为 None
    pub project: Option<String>,
    pub packages: Vec<Package>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileEntry {
    /// 相对用户层根，`/` 分隔
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub packages: Vec<String>,
    pub files: usize,
    /// 快照文件大小（压缩后）
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    /// 新装进用户层的包
    pub packages: Vec<String>,
    /// 用户层里已经有的包，没动
    pub skipped: Vec<String>,
    pub files: usize,
    /// 用户层里已有同名、内容不同的文件，保留原样没覆盖
    pub kept: Vec<String>,
    /// 快照来自哪个项目
    pub project: Option<String>,
}

fn unmanaged() -> Fail {
    Fail::new("case.unmanaged", "用的是本机自己的 TeX，宏包请用 tlmgr 管理")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// 相对路径只能是普通的几段，开头不能是用户层的配置目录
fn valid_rel(rel: &str) -> bool {
    let p = Path::new(rel);
    let mut parts = p.components();
    let ok = p.components().all(|c| matches!(c, Component::Normal(_))) && !rel.is_empty();
    ok && parts.next().and_then(|c| c.as_os_str().to_str()).is_some_and(|first| !RESERVED.contains(&first))
}

/// 项目最近一次编译读过的、属于用户层里补装宏包的文件，换成包名。
/// `fls` 是 TeX 的 `-recorder` 记录；`home` 是用户层根。只认绝对路径（用户层里的文件都是）
pub fn packages_used(fls: &str, home: &Path, owners: &BTreeMap<String, String>) -> Vec<String> {
    let home = dunce::canonicalize(&home).unwrap_or_else(|_| home.to_path_buf());
    let mut out = BTreeSet::new();
    for line in fls.lines() {
        let Some(path) = line.strip_prefix("INPUT ") else { continue };
        let p = Path::new(path.trim());
        if !p.is_absolute() {
            continue;
        }
        let p = dunce::canonicalize(&p).unwrap_or_else(|_| p.to_path_buf());
        let Ok(rel) = p.strip_prefix(&home) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if let Some(pkg) = owners.get(&rel) {
            out.insert(pkg.clone());
        }
    }
    out.into_iter().collect()
}

/// 把这些用户层宏包（`None` 为全部）打成快照写到 `out`。先写临时文件，写完再改名
pub fn export(pkgs: Option<&[String]>, project: Option<&str>, out: &Path) -> Result<Exported, Fail> {
    let home = case::user_home().ok_or_else(unmanaged)?;
    let all = case::user_packages();
    let chosen: Vec<(&String, &Vec<String>)> = all.iter().filter(|(name, _)| pkgs.is_none_or(|p| p.contains(name))).collect();
    if chosen.is_empty() {
        return Err(Fail::new("case.snapshotEmpty", "没有要导出的宏包：用到的都是内置的"));
    }

    let failed = |e: std::io::Error| Fail::new("case.snapshotWriteFailed", format!("写快照失败：{e}")).detail(e);
    let mut manifest = Manifest {
        format: FORMAT.into(),
        version: VERSION,
        texlive: case::local_year(),
        created: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        project: project.map(str::to_string),
        packages: Vec::new(),
    };
    // 先读一遍算哈希、写清单；清单要放在包的最前面，导入时先看清单
    let mut data: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, files) in chosen {
        let mut entry = Package { name: name.clone(), files: Vec::new() };
        for rel in files.iter().filter(|r| valid_rel(r)) {
            // 清单里有、磁盘上没有的（用户手动删过）跳过：导出的是现状
            let Ok(bytes) = fs::read(home.join(rel)) else { continue };
            entry.files.push(FileEntry { path: rel.clone(), size: bytes.len() as u64, sha256: sha256(&bytes) });
            data.push((format!("{TREE}/{rel}"), bytes));
        }
        if !entry.files.is_empty() {
            manifest.packages.push(entry);
        }
    }
    if manifest.packages.is_empty() {
        return Err(Fail::new("case.snapshotEmpty", "没有要导出的宏包：用到的都是内置的"));
    }

    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(failed)?;
    }
    let tmp = out.with_extension(format!("{EXT}.tmp-{}", std::process::id()));
    let write = || -> std::io::Result<u64> {
        let mut w = tarball::Writer::new(BufWriter::new(fs::File::create(&tmp)?));
        w.add(MANIFEST, &serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?)?;
        for (name, bytes) in &data {
            w.add(name, bytes)?;
        }
        let mut f = w.finish()?;
        f.flush()?;
        drop(f);
        fs::rename(&tmp, out)?;
        Ok(fs::metadata(out)?.len())
    };
    let bytes = write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        failed(e)
    })?;
    Ok(Exported {
        packages: manifest.packages.iter().map(|p| p.name.clone()).collect(),
        files: data.len(),
        bytes,
    })
}

/// 导入快照。`scratch` 是可以随便用的临时目录（会被清掉）。
/// 先全部解到临时目录、核对完清单再往用户层写；用户层里已有的包不动
pub fn import(bytes: &[u8], scratch: &Path) -> Result<Imported, Fail> {
    let home = case::user_home().ok_or_else(unmanaged)?;
    let _ = fs::remove_dir_all(scratch);
    fs::create_dir_all(scratch).map_err(|e| Fail::new("case.snapshotWriteFailed", format!("建临时目录失败：{e}")).detail(e))?;
    let res = import_into(bytes, scratch, &home);
    let _ = fs::remove_dir_all(scratch);
    res
}

fn bad() -> Fail {
    Fail::new("case.snapshotBad", "不是 Subo 字盘快照")
}

fn import_into(bytes: &[u8], scratch: &Path, home: &Path) -> Result<Imported, Fail> {
    tarball::extract(bytes, scratch, IMPORT_MAX_BYTES, IMPORT_MAX_FILES).map_err(|e| match e.code {
        "import.notArchive" | "import.pdfOnly" => bad(),
        _ => e,
    })?;
    let manifest: Manifest = fs::read(scratch.join(MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|m: &Manifest| m.format == FORMAT)
        .ok_or_else(bad)?;
    if manifest.version > VERSION {
        return Err(Fail::new("case.snapshotVersion", "快照是更新版本的 Subo 导出的，请先升级"));
    }
    if let (Some(theirs), Some(ours)) = (manifest.texlive, case::local_year()) {
        if theirs != ours {
            return Err(Fail::new("case.snapshotYear", format!("快照是 TeX Live {theirs} 的，本机是 {ours}"))
                .with("year", theirs)
                .with("local", ours));
        }
    }

    // 全部核对完再动用户层：一个文件不对就整包拒收
    let installed = case::user_packages();
    let mut fresh: Vec<&Package> = Vec::new();
    let mut skipped = Vec::new();
    for p in &manifest.packages {
        if !case::valid_package(&p.name) || p.files.is_empty() {
            return Err(bad());
        }
        for f in &p.files {
            if !valid_rel(&f.path) {
                return Err(Fail::new("case.snapshotBadPath", format!("快照里有不该放进用户层的文件：{}", f.path)).with("file", f.path.as_str()));
            }
            let data = fs::read(scratch.join(TREE).join(&f.path)).ok();
            if data.as_ref().is_none_or(|d| d.len() as u64 != f.size || sha256(d) != f.sha256) {
                return Err(Fail::new("case.snapshotCorrupt", format!("快照已损坏：{} 对不上", f.path)).with("file", f.path.as_str()));
            }
        }
        if installed.contains_key(&p.name) {
            skipped.push(p.name.clone());
        } else {
            fresh.push(p);
        }
    }

    let owners: BTreeMap<&str, &str> = installed.iter().flat_map(|(pkg, files)| files.iter().map(move |f| (f.as_str(), pkg.as_str()))).collect();
    let failed = |e: std::io::Error| Fail::new("case.snapshotWriteFailed", format!("写入用户层失败：{e}")).detail(e);
    let (mut count, mut kept, mut done) = (0usize, Vec::new(), Vec::new());
    for p in fresh {
        let mut mine = Vec::new();
        for f in &p.files {
            let to = home.join(&f.path);
            if let Ok(existing) = fs::read(&to) {
                // 已经有一模一样的：别的包没占着就记到这个包名下；内容不同的不覆盖
                if sha256(&existing) == f.sha256 {
                    if !owners.contains_key(f.path.as_str()) {
                        mine.push(f.path.clone());
                    }
                } else {
                    kept.push(f.path.clone());
                }
                continue;
            }
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir).map_err(failed)?;
            }
            let tmp = to.with_file_name(format!(".{}.subo-import", to.file_name().and_then(|n| n.to_str()).unwrap_or("f")));
            fs::copy(scratch.join(TREE).join(&f.path), &tmp).map_err(failed)?;
            fs::rename(&tmp, &to).map_err(failed)?;
            mine.push(f.path.clone());
            count += 1;
        }
        case::save_runfiles(home, &p.name, &mine).map_err(failed)?;
        done.push(p.name.clone());
    }
    if !done.is_empty() {
        case::refresh_user_lsr(home, Instant::now() + Duration::from_secs(120));
    }
    Ok(Imported { packages: done, skipped, files: count, kept, project: manifest.project })
}

/// 项目最近一次编译的 `.fls`（`outdir` 是那次编译的产物目录，`entry` 是入口文件）
pub fn recorder_file(outdir: &Path, entry: &str) -> PathBuf {
    let stem = Path::new(entry).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "main".into());
    outdir.join(format!("{stem}.fls"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "subo-snap-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rel_paths_stay_inside_and_skip_config() {
        assert!(valid_rel("tex/latex/foo/foo.sty"));
        assert!(valid_rel("fonts/tfm/public/x/x.tfm"));
        assert!(!valid_rel("../etc/passwd"));
        assert!(!valid_rel("/abs/x.sty"));
        assert!(!valid_rel("web2c/texmf.cnf"));
        assert!(!valid_rel("tlpkg/texlive.tlpdb"));
        assert!(!valid_rel("ls-R"));
        assert!(!valid_rel(""));
    }

    #[test]
    fn recorder_maps_user_files_to_packages() {
        let home = scratch("home");
        fs::create_dir_all(home.join("tex/latex/xltxtra")).unwrap();
        fs::write(home.join("tex/latex/xltxtra/xltxtra.sty"), "x").unwrap();
        let owners = BTreeMap::from([("tex/latex/xltxtra/xltxtra.sty".to_string(), "xltxtra".to_string())]);
        let fls = format!(
            "PWD /tmp/p\nINPUT main.tex\nINPUT /usr/local/texlive/2026/texmf-dist/tex/latex/base/article.cls\nINPUT {}/tex/latex/xltxtra/xltxtra.sty\nOUTPUT main.pdf\n",
            home.display()
        );
        assert_eq!(packages_used(&fls, &home, &owners), vec!["xltxtra".to_string()]);
        let _ = fs::remove_dir_all(&home);
    }

    /// 造一个快照包：清单 + 文件；`tamper` 为 true 时把文件内容改掉、清单不改
    fn pack(year: Option<u32>, files: &[(&str, &[u8])], tamper: bool) -> Vec<u8> {
        let manifest = Manifest {
            format: FORMAT.into(),
            version: VERSION,
            texlive: year,
            created: 0,
            project: Some("paper".into()),
            packages: vec![Package {
                name: "demo".into(),
                files: files.iter().map(|(p, d)| FileEntry { path: p.to_string(), size: d.len() as u64, sha256: sha256(d) }).collect(),
            }],
        };
        let mut w = tarball::Writer::new(Vec::new());
        w.add(MANIFEST, &serde_json::to_vec(&manifest).unwrap()).unwrap();
        for (p, d) in files {
            let d: &[u8] = if tamper { b"evil" } else { d };
            w.add(&format!("{TREE}/{p}"), d).unwrap();
        }
        w.finish().unwrap()
    }

    #[test]
    fn import_checks_hashes_and_paths_before_writing() {
        let home = scratch("ihome");
        let files: &[(&str, &[u8])] = &[("tex/latex/demo/demo.sty", b"\\ProvidesPackage{demo}")];

        // 内容被改过：拒收，用户层一个文件都不写
        let err = import_into(&pack(None, files, true), &scratch("t1"), &home).unwrap_err();
        assert_eq!(err.code, "case.snapshotCorrupt");
        assert!(!home.join("tex").exists());

        // 想往配置目录里写：拒收
        let err = import_into(&pack(None, &[("web2c/texmf.cnf", b"x")], false), &scratch("t2"), &home).unwrap_err();
        assert_eq!(err.code, "case.snapshotBadPath");

        // 不是快照
        let mut w = tarball::Writer::new(Vec::new());
        w.add("main.tex", b"x").unwrap();
        assert_eq!(import_into(&w.finish().unwrap(), &scratch("t3"), &home).unwrap_err().code, "case.snapshotBad");
        assert_eq!(import_into(b"junk", &scratch("t4"), &home).unwrap_err().code, "case.snapshotBad");
    }
}
