//! 项目模板：项目库下的 `.templates/<id>/`，每个模板一个目录：
//!
//! ```text
//! .templates/
//! └── 学位论文/
//!     ├── template.json   # 显示名、入口、来源、体积、依赖线索
//!     └── files/          # 模板内容，原样复制成新项目
//! ```
//!
//! 放在项目库里而不是 `~/.subo`：模板是用户的东西，删 `~/.subo` 只该损失缓存。
//! 点开头的目录不会出现在项目列表里。
//!
//! **入库只看形状，不编译。** 缺字体、缺宏包是「用的时候」的事：新项目打开即编译，
//! 缺什么走字盘的缺字清单补。入库时把 `\usepackage`、`\setmainfont` 之类扫出来
//! 记成线索给用户看，不去核对本机有没有。
//!
//! 形状校验挡的是「这根本不是一个 LaTeX 项目」：
//! 1. 顶层要有主文件——含 `\documentclass` 和 `\begin{document}` 的 UTF-8 `.tex`；
//! 2. 文件类型白名单：源码、样式、参考文献、图片、字体之类；可执行文件、Office 文档、
//!    嵌套压缩包等一律拒收，并列出是哪些；
//! 3. 内容要和扩展名对得上：改名成 `.png` 的东西不收；
//! 4. 编译产物（`.aux`、`.log`、`.synctex.gz`、与主文件同名的 PDF……）和系统杂项直接丢掉；
//! 5. 体积、文件数比项目导入卡得更紧：模板是起点，不是归档。
//!
//! **分类。** 每个模板恰好属于 [`CATEGORIES`] 里的一类。入库时按文档类猜一个
//! （beamer → 幻灯片、ctexbook → 学位论文……），猜不出就归「其他」，用户随时可改。
//! 早先存的模板没有分类字段，列出来时现猜，不回写。
//!
//! 预置模板见 [`crate::presets`]：编进二进制、只读，列表里和用户模板排在一起。

use crate::fail::Fail;
use crate::library;
use crate::presets;
use crate::project;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 带 CJK 字体的模板一款字体就十几 MB，留够余量
const MAX_BYTES: u64 = 100 << 20;
const MAX_FILES: u64 = 1000;
const MAX_DEPTH: usize = 8;
/// 拒收清单、丢弃清单最多列几项，再多就只给个数
const LIST_MAX: usize = 5;
/// 依赖线索最多记几条
const HINTS_MAX: usize = 60;

const META: &str = "template.json";
const FILES: &str = "files";

/// 用户编辑的源码，必须是 UTF-8：编辑器只认 UTF-8
const SOURCE_EXT: &[&str] = &["tex", "ltx", "sty", "cls", "bib"];
/// 其余文本类：不查编码，只查不是二进制（无 NUL）
const TEXT_EXT: &[&str] = &[
    "clo", "cfg", "def", "fd", "bst", "bbx", "cbx", "lbx", "dbx", "ist", "bbl", "ins", "dtx", "tikz",
    "pgf", "txt", "md", "csv", "tsv", "dat", "lua", "mp", "map", "enc", "afm", "gitignore", "latexmkrc",
];
/// 没有扩展名、但项目里常见的文本文件
const TEXT_NAMES: &[&str] = &["README", "LICENSE", "LICENCE", "COPYING", "Makefile", "latexmkrc", ".latexmkrc", ".gitignore"];
/// 二进制但没有可靠文件头的 TeX 字体文件，只认扩展名
const OPAQUE_EXT: &[&str] = &["tfm", "vf", "pfm"];
/// 编译中间产物：丢掉，不算拒收
const BUILD_EXT: &[&str] = &[
    "aux", "log", "out", "toc", "lof", "lot", "lol", "fls", "fdb_latexmk", "synctex", "bcf", "blg",
    "idx", "ilg", "ind", "nav", "snm", "vrb", "xdv", "dvi", "glo", "gls", "glg", "acn", "acr", "alg",
    "thm", "brf", "ptc", "swp", "bak",
];
const JUNK_NAMES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];
const JUNK_DIRS: &[&str] = &["__MACOSX", "node_modules"];

/// 模板分类，一个模板只属于一类。键给前端查文案，顺序即界面上的顺序
pub const CATEGORIES: &[&str] = &["paper", "thesis", "report", "resume", "slides", "poster", "other"];

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub name: String,
    /// [`CATEGORIES`] 之一；早先存的模板没有这个字段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub entry: String,
    /// Unix 秒
    pub created: u64,
    /// `project` / `zip` / `builtin`
    pub source: String,
    pub files: u64,
    pub bytes: u64,
    /// 源码里引用、模板自己没带的宏包与文档类，只是线索，没核对本机有没有
    pub packages: Vec<String>,
    /// 源码里点名的字体（`\setmainfont` 等），模板自带的字体文件不算
    pub fonts: Vec<String>,
}

#[derive(Serialize)]
pub struct Summary {
    pub id: String,
    /// 预置模板：只读，不能删、不能改分类
    pub builtin: bool,
    #[serde(flatten)]
    pub meta: Meta,
}

/// 入库结果：新模板 id、最终显示名和被丢掉的杂项（相对路径，最多 `LIST_MAX` 项）
#[derive(Serialize)]
pub struct Stored {
    pub id: String,
    pub name: String,
    pub dropped: Vec<String>,
    #[serde(rename = "droppedCount")]
    pub dropped_count: usize,
}

/// 模板目录在项目库下的名字；换项目库位置时跟着项目一起搬
pub const DIR_NAME: &str = ".templates";

pub fn dir(home: &Path) -> PathBuf {
    home.join(DIR_NAME)
}

/// 用户模板（新存的在前）+ 预置模板（固定顺序）
pub fn list(dir: &Path) -> Vec<Summary> {
    let mut out = list_user(dir);
    out.extend(presets::ALL.iter().map(preset_summary));
    out
}

fn list_user(dir: &Path) -> Vec<Summary> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Summary> = rd
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().to_string();
            if !library::valid_id(&id) {
                return None;
            }
            let mut meta = read_meta(&e.path())?;
            let files = e.path().join(FILES);
            if !files.is_dir() {
                return None;
            }
            if !meta.category.as_deref().is_some_and(|c| CATEGORIES.contains(&c)) {
                meta.category = Some(guess_category(&files, &meta.entry, &meta.name).into());
            }
            Some(Summary { id, builtin: false, meta })
        })
        .collect();
    out.sort_by(|a, b| b.meta.created.cmp(&a.meta.created));
    out
}

fn preset_summary(p: &presets::Preset) -> Summary {
    let sources = p
        .files
        .iter()
        .filter(|(f, _)| f.ends_with(".tex"))
        .map(|(_, c)| c.to_string())
        .collect();
    let local = p.files.iter().filter_map(|(f, _)| f.rsplit('/').next()).map(str::to_ascii_lowercase).collect();
    let (packages, fonts) = scan_hints(sources, &local);
    Summary {
        id: p.full_id(),
        builtin: true,
        meta: Meta {
            name: p.name.into(),
            category: Some(p.category.into()),
            entry: p.entry.into(),
            created: 0,
            source: "builtin".into(),
            files: p.files.len() as u64,
            bytes: p.bytes(),
            packages,
            fonts,
        },
    }
}

/// 名字里的关键词比文档类更可靠：简历、报告常用 article / ctexart，光看文档类分不出来
const NAME_HINTS: &[(&str, &[&str])] = &[
    ("resume", &["简历", "履历", "履歷", "resume", "résumé", "cv"]),
    ("slides", &["幻灯", "演示", "简报", "簡報", "slides", "beamer", "presentation"]),
    ("poster", &["海报", "海報", "poster"]),
    ("thesis", &["学位论文", "毕业论文", "學位論文", "thesis", "dissertation"]),
    ("report", &["报告", "報告", "周报", "週報", "report"]),
];

/// 猜分类：先看模板名与主文件名里的关键词，再看主文件的文档类。只是个起点，用户可以改
fn guess_category(files: &Path, entry: &str, name: &str) -> &'static str {
    // 驼峰拆开再比：ModernFancyCV → modern fancy cv
    let mut names = String::new();
    let mut prev_lower = false;
    for c in format!("{name} {entry}").chars() {
        if c.is_uppercase() && prev_lower {
            names.push(' ');
        }
        prev_lower = c.is_lowercase();
        names.extend(c.to_lowercase());
    }
    // cv 这类短词只认完整的词，免得 rcvd 之类误中
    let words: Vec<&str> = names.split(|c: char| !c.is_alphanumeric()).collect();
    let hit = |k: &str| if k.is_ascii() && k.len() <= 3 { words.contains(&k) } else { names.contains(k) };
    if let Some((cat, _)) = NAME_HINTS.iter().find(|(_, ks)| ks.iter().any(|k| hit(k))) {
        return cat;
    }
    let Ok(src) = fs::read_to_string(files.join(entry)) else { return "other" };
    let code: String = src.lines().map(strip_comment).collect::<Vec<_>>().join("\n");
    let cmds = commands(&code);
    let class = cmds
        .iter()
        .find(|(c, _)| *c == "documentclass")
        .and_then(|(_, a)| a.first())
        .map(|c| c.to_ascii_lowercase())
        .unwrap_or_default();
    let uses = |pkg: &str| {
        cmds.iter().any(|(c, a)| *c == "usepackage" && a.first().is_some_and(|x| x.split(',').any(|p| p.trim() == pkg)))
    };
    let has = |words: &[&str]| words.iter().any(|w| class.contains(w));
    if has(&["poster"]) || ["beamerposter", "tikzposter", "a0poster", "baposter"].iter().any(|p| uses(p)) {
        "poster"
    } else if has(&["beamer", "slide", "powerdot", "prosper"]) {
        "slides"
    } else if has(&["cv", "resume", "résumé"]) {
        "resume"
    } else if has(&["thesis", "dissertation", "book", "memoir"]) {
        "thesis"
    } else if has(&["report", "ctexrep", "scrreprt"]) {
        "report"
    } else if has(&["art", "paper", "ieeetran", "acmart", "llncs", "revtex", "elsarticle", "aastex", "jss"]) {
        "paper"
    } else {
        "other"
    }
}

/// 改分类。预置模板只读
pub fn set_category(dir: &Path, id: &str, category: &str) -> Result<(), Fail> {
    if presets::get(id).is_some() {
        return Err(Fail::new("template.builtin", "预置模板不能修改"));
    }
    if !CATEGORIES.contains(&category) {
        return Err(Fail::new("template.badCategory", format!("没有这个分类：{category}")).with("category", category));
    }
    let p = dir.join(id);
    let mut meta = match library::valid_id(id).then(|| read_meta(&p)).flatten() {
        Some(m) => m,
        None => return Err(Fail::new("template.notFound", "没有这个模板")),
    };
    meta.category = Some(category.into());
    let json = serde_json::to_vec_pretty(&meta).unwrap_or_default();
    fs::write(p.join(META), json).map_err(|e| Fail::new("template.store", format!("保存模板失败: {e}")).detail(e))
}

fn read_meta(tpl: &Path) -> Option<Meta> {
    serde_json::from_slice(&fs::read(tpl.join(META)).ok()?).ok()
}

/// 把项目存成模板。先复制到临时目录（跳过隐藏目录、符号链接，边复制边卡体积），
/// 再走和 zip 一样的校验。
pub fn from_project(dir: &Path, root: &Path, name: &str) -> Result<Stored, Fail> {
    library::check_name(name)?;
    stage(dir, name, "project", |files| {
        let mut budget = Budget::default();
        copy_tree(root, files, &mut budget, 0)
    })
}

/// 上传的压缩包（zip / tar.gz）存成模板。解压与防炸弹沿用项目导入的那一套。
pub fn from_zip(dir: &Path, name: &str, bytes: &[u8]) -> Result<Stored, Fail> {
    stage(dir, name, "zip", |files| {
        let unpack = files.with_extension("unpack");
        let res = (|| {
            fs::create_dir(&unpack).map_err(|e| Fail::new("import.tempDir", format!("创建临时目录失败: {e}")).detail(e))?;
            library::unpack(bytes, &unpack)?;
            let top = library::strip_single_top(&unpack);
            fs::remove_dir(files).ok();
            fs::rename(&top, files).map_err(|e| Fail::new("template.store", format!("保存模板失败: {e}")).detail(e))
        })();
        let _ = fs::remove_dir_all(&unpack);
        res
    })
}

/// 在 `.templates/.incoming-N/` 里备好 `files/`，校验、扫线索、写元数据，成功才改名上架；
/// 失败整个删掉。
fn stage(
    dir: &Path,
    name: &str,
    source: &str,
    fill: impl FnOnce(&Path) -> Result<(), Fail>,
) -> Result<Stored, Fail> {
    fs::create_dir_all(dir).map_err(|e| Fail::new("template.store", format!("保存模板失败: {e}")).detail(e))?;
    let tmp = dir.join(format!(".incoming-{}", now_nanos()));
    let res = (|| {
        let files = tmp.join(FILES);
        fs::create_dir_all(&files).map_err(|e| Fail::new("import.tempDir", format!("创建临时目录失败: {e}")).detail(e))?;
        fill(&files)?;
        let admitted = admit(&files)?;
        let (packages, fonts) = hints(&files);
        // 显示名也去重：同一个项目存两次，列表里得分得清
        let taken: Vec<String> = list_user(dir).into_iter().map(|t| t.meta.name).collect();
        let name = unique_name(name.trim(), &taken);
        let category = guess_category(&files, &admitted.entry, &name);
        let meta = Meta {
            name: name.clone(),
            category: Some(category.into()),
            entry: admitted.entry,
            created: now_nanos() as u64 / 1_000_000_000,
            source: source.into(),
            files: admitted.files,
            bytes: admitted.bytes,
            packages,
            fonts,
        };
        let json = serde_json::to_vec_pretty(&meta).unwrap_or_default();
        fs::write(tmp.join(META), json).map_err(|e| Fail::new("template.store", format!("保存模板失败: {e}")).detail(e))?;
        let id = library::unique(dir, &library::slug(&name));
        fs::rename(&tmp, dir.join(&id)).map_err(|e| Fail::new("template.store", format!("保存模板失败: {e}")).detail(e))?;
        let dropped_count = admitted.dropped.len();
        Ok(Stored { id, name, dropped: admitted.dropped.into_iter().take(LIST_MAX).collect(), dropped_count })
    })();
    let _ = fs::remove_dir_all(&tmp);
    res
}

/// 用模板新建项目：把 `files/` 复制进项目库，起名、去重同新建项目。
/// 预置模板直接把编进来的源码写出去。
pub fn instantiate(dir: &Path, id: &str, home: &Path, name: &str) -> Result<String, Fail> {
    exists(dir, id)?;
    library::check_name(name)?;
    fs::create_dir_all(home).map_err(|e| Fail::new("library.createHome", format!("无法创建项目库: {e}")).detail(e))?;
    let tmp = home.join(format!(".incoming-{}", now_nanos()));
    let res = (|| {
        fs::create_dir(&tmp).map_err(|e| Fail::new("import.tempDir", format!("创建临时目录失败: {e}")).detail(e))?;
        write_files(dir, id, &tmp)?;
        let pid = library::unique(home, &library::slug(name));
        fs::rename(&tmp, home.join(&pid)).map_err(|e| Fail::new("import.store", format!("保存项目失败: {e}")).detail(e))?;
        Ok(pid)
    })();
    let _ = fs::remove_dir_all(&tmp);
    res
}

fn exists(dir: &Path, id: &str) -> Result<(), Fail> {
    if presets::get(id).is_some() || (library::valid_id(id) && dir.join(id).join(FILES).is_dir()) {
        Ok(())
    } else {
        Err(Fail::new("template.notFound", "没有这个模板"))
    }
}

/// 把模板内容写进已存在的空目录 `dest`；预置模板写编进来的源码
fn write_files(dir: &Path, id: &str, dest: &Path) -> Result<(), Fail> {
    match presets::get(id) {
        Some(p) => p.write_to(dest),
        None => copy_tree(&dir.join(id).join(FILES), dest, &mut Budget::default(), 0),
    }
}

/// 预览模板：在 `dest`（Subo 自己的缓存目录，不在项目库里）展开一份副本，
/// 只供只读浏览和编译，不会出现在项目列表里。已有的旧副本整个换掉。
pub fn materialize(dir: &Path, id: &str, dest: &Path) -> Result<(), Fail> {
    exists(dir, id)?;
    let _ = fs::remove_dir_all(dest);
    fs::create_dir_all(dest).map_err(|e| Fail::new("import.tempDir", format!("创建临时目录失败: {e}")).detail(e))?;
    write_files(dir, id, dest).inspect_err(|_| {
        let _ = fs::remove_dir_all(dest);
    })
}

pub fn remove(dir: &Path, id: &str) -> Result<(), Fail> {
    if presets::get(id).is_some() {
        return Err(Fail::new("template.builtin", "预置模板不能删除"));
    }
    let p = dir.join(id);
    if !library::valid_id(id) || !p.join(META).is_file() {
        return Err(Fail::new("template.notFound", "没有这个模板"));
    }
    fs::remove_dir_all(&p).map_err(|e| Fail::new("template.removeFailed", format!("删除模板失败: {e}")).detail(e))
}

/// 同名就加 `-2`、`-3`……，与项目去重一个规矩
fn unique_name(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| !taken.contains(c)).unwrap()
}

fn now_nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

#[derive(Default)]
struct Budget {
    files: u64,
    bytes: u64,
}

impl Budget {
    fn take(&mut self, bytes: u64) -> Result<(), Fail> {
        self.files += 1;
        self.bytes += bytes;
        if self.files > MAX_FILES {
            return Err(Fail::new("template.tooMany", format!("文件超过 {MAX_FILES} 个，不像是模板")).with("max", MAX_FILES));
        }
        if self.bytes > MAX_BYTES {
            let max = MAX_BYTES >> 20;
            return Err(Fail::new("template.tooBig", format!("超过 {max} MB，不像是模板")).with("max", max));
        }
        Ok(())
    }
}

/// 复制目录树：不跟符号链接，跳过隐藏目录与 `node_modules` 之类（`.git` 动辄几百 MB）。
fn copy_tree(src: &Path, dst: &Path, budget: &mut Budget, depth: usize) -> Result<(), Fail> {
    let io = |e: std::io::Error| Fail::new("template.copy", format!("复制文件失败: {e}")).detail(e);
    if depth > MAX_DEPTH {
        return Err(Fail::new("template.tooDeep", format!("目录超过 {MAX_DEPTH} 层")).with("max", MAX_DEPTH));
    }
    fs::create_dir_all(dst).map_err(io)?;
    for e in fs::read_dir(src).map_err(io)? {
        let e = e.map_err(io)?;
        let name = e.file_name();
        let n = name.to_string_lossy();
        let ft = e.file_type().map_err(io)?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if n.starts_with('.') || JUNK_DIRS.contains(&n.as_ref()) {
                continue;
            }
            copy_tree(&e.path(), &dst.join(&name), budget, depth + 1)?;
        } else if ft.is_file() {
            budget.take(e.metadata().map_err(io)?.len())?;
            fs::copy(e.path(), dst.join(&name)).map_err(io)?;
        }
    }
    Ok(())
}

struct Admitted {
    entry: String,
    files: u64,
    bytes: u64,
    dropped: Vec<String>,
}

enum Verdict {
    Keep,
    Drop,
    /// 拒收，附原因码：`type` 类型不收、`mismatch` 内容与扩展名不符、`encoding` 不是 UTF-8
    Reject(&'static str),
}

/// 形状校验。原地删掉杂项与编译产物；有拒收的文件就整体报错，把前几个列给用户。
fn admit(root: &Path) -> Result<Admitted, Fail> {
    let entry = project::entry(root);
    // 与主文件同名的 PDF 是编译产物（导出过一次就会有），不是模板的一部分
    let entry_pdf = entry.as_deref().and_then(|e| e.strip_suffix(".tex")).map(|s| format!("{s}.pdf"));

    let mut files = Vec::new();
    collect(root, root, &mut files, 0)?;

    let mut dropped = Vec::new();
    let mut rejected: Vec<(String, &'static str)> = Vec::new();
    let mut budget = Budget::default();
    for (rel, path, is_dir) in files {
        if is_dir {
            fs::remove_dir_all(&path).ok();
            dropped.push(format!("{rel}/"));
            continue;
        }
        let verdict = if Some(&rel) == entry_pdf.as_ref() { Verdict::Drop } else { judge(&path) };
        match verdict {
            Verdict::Keep => budget.take(fs::metadata(&path).map(|m| m.len()).unwrap_or(0))?,
            Verdict::Drop => {
                fs::remove_file(&path).ok();
                dropped.push(rel);
            }
            Verdict::Reject(why) => rejected.push((rel, why)),
        }
    }

    if !rejected.is_empty() {
        // 先报最要紧的一类：内容造假 > 编码 > 类型
        let why = ["mismatch", "encoding", "type"]
            .into_iter()
            .find(|w| rejected.iter().any(|(_, r)| r == w))
            .unwrap_or("type");
        let names: Vec<&str> = rejected.iter().filter(|(_, r)| *r == why).map(|(n, _)| n.as_str()).collect();
        let count = names.len();
        let list = names.iter().take(LIST_MAX).copied().collect::<Vec<_>>().join(", ");
        let (code, msg) = match why {
            "mismatch" => ("template.mismatch", format!("文件内容与扩展名不符：{list}")),
            "encoding" => ("template.encoding", format!("源码不是 UTF-8 编码：{list}")),
            _ => ("template.badFiles", format!("含有模板里不该有的文件：{list}")),
        };
        return Err(Fail::new(code, msg).with("files", list).with("count", count));
    }

    let Some(entry) = entry.filter(|e| is_root_doc(&root.join(e))) else {
        return Err(Fail::new(
            "template.noEntry",
            "找不到主文件：顶层需要一个含 \\documentclass 和 \\begin{document} 的 .tex",
        ));
    };
    dropped.sort();
    Ok(Admitted { entry, files: budget.files, bytes: budget.bytes, dropped })
}

/// 收集所有文件，相对路径用 `/`。隐藏目录与杂项目录作为整项返回，交给 admit 删。
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf, bool)>, depth: usize) -> Result<(), Fail> {
    if depth > MAX_DEPTH {
        return Err(Fail::new("template.tooDeep", format!("目录超过 {MAX_DEPTH} 层")).with("max", MAX_DEPTH));
    }
    let io = |e: std::io::Error| Fail::new("template.copy", format!("复制文件失败: {e}")).detail(e);
    for e in fs::read_dir(dir).map_err(io)? {
        let e = e.map_err(io)?;
        let path = e.path();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let name = e.file_name().to_string_lossy().to_string();
        let ft = e.file_type().map_err(io)?;
        if ft.is_symlink() {
            // sanitize 已清过；保险起见，链接一律丢
            fs::remove_file(&path).ok();
            continue;
        }
        if ft.is_dir() {
            if name.starts_with('.') || JUNK_DIRS.contains(&name.as_str()) {
                out.push((rel, path, true));
            } else {
                collect(root, &path, out, depth + 1)?;
            }
        } else {
            out.push((rel, path, false));
        }
    }
    Ok(())
}

fn judge(path: &Path) -> Verdict {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");

    if JUNK_NAMES.contains(&name.as_str())
        || name.ends_with('~')
        || lower.ends_with(".synctex.gz")
        || lower.ends_with(".run.xml")
        || BUILD_EXT.contains(&ext)
    {
        return Verdict::Drop;
    }
    if TEXT_NAMES.contains(&name.as_str()) || (!lower.contains('.') && name.chars().all(|c| c.is_ascii_uppercase())) {
        return text(path, false);
    }
    if SOURCE_EXT.contains(&ext) {
        return text(path, true);
    }
    if TEXT_EXT.contains(&ext) {
        return text(path, false);
    }
    if OPAQUE_EXT.contains(&ext) {
        return Verdict::Keep;
    }
    let magic: &[&[u8]] = match ext {
        "png" => &[b"\x89PNG"],
        "jpg" | "jpeg" => &[b"\xFF\xD8\xFF"],
        "gif" => &[b"GIF8"],
        "pdf" => &[b"%PDF"],
        "eps" | "ps" => &[b"%!PS", b"\xC5\xD0\xD3\xC6"],
        "otf" | "ttf" => &[b"OTTO", b"\x00\x01\x00\x00", b"true"],
        "ttc" => &[b"ttcf"],
        "woff" => &[b"wOFF"],
        "woff2" => &[b"wOF2"],
        "pfb" => &[b"\x80\x01"],
        "svg" => return svg(path),
        // hidden files（.editorconfig 之类）无害，丢掉即可，不值得拒收
        _ if name.starts_with('.') => return Verdict::Drop,
        _ => return Verdict::Reject("type"),
    };
    let head = head(path, 1024);
    // PDF 头前面允许有少量垃圾字节，阅读器都这么容忍
    let hit = if ext == "pdf" {
        head.windows(4).any(|w| w == b"%PDF")
    } else {
        magic.iter().any(|m| head.starts_with(m))
    };
    if hit {
        Verdict::Keep
    } else {
        Verdict::Reject("mismatch")
    }
}

fn head(path: &Path, n: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(n);
    if let Ok(f) = fs::File::open(path) {
        let _ = f.take(n as u64).read_to_end(&mut buf);
    }
    buf
}

/// 文本文件：不能有 NUL（伪装成文本的二进制）；源码还得是 UTF-8。
fn text(path: &Path, utf8: bool) -> Verdict {
    let Ok(bytes) = fs::read(path) else { return Verdict::Reject("type") };
    if bytes.contains(&0) {
        return Verdict::Reject("mismatch");
    }
    if utf8 && std::str::from_utf8(&bytes).is_err() {
        return Verdict::Reject("encoding");
    }
    Verdict::Keep
}

fn svg(path: &Path) -> Verdict {
    let h = head(path, 4096);
    if !h.contains(&0) && String::from_utf8_lossy(&h).contains("<svg") {
        Verdict::Keep
    } else {
        Verdict::Reject("mismatch")
    }
}

fn is_root_doc(path: &Path) -> bool {
    let Ok(s) = fs::read_to_string(path) else { return false };
    let code: String = s.lines().map(strip_comment).collect::<Vec<_>>().join("\n");
    code.contains("\\documentclass") && code.contains("\\begin{document}")
}

/// 去掉行内注释：没被反斜杠转义的 `%` 之后都不算
fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'%' => return &line[..i],
            _ => i += 1,
        }
    }
    line
}

/// 从源码里扫依赖线索。只是给人看的提示：不展开宏、不跟 `\input`，扫到什么记什么，
/// 模板自带的 `.sty`/`.cls`/字体文件剔掉。
fn hints(root: &Path) -> (Vec<String>, Vec<String>) {
    let mut paths = Vec::new();
    let mut local = std::collections::HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            local.insert(name.to_ascii_lowercase());
            if matches!(p.extension().and_then(|x| x.to_str()), Some("tex" | "sty" | "cls" | "ltx")) {
                paths.push(p);
            }
        }
    }
    paths.sort();
    let sources = paths.iter().filter_map(|p| fs::read_to_string(p).ok()).collect();
    scan_hints(sources, &local)
}

/// `local` 是模板自带文件的小写文件名，自带的 `.sty`/`.cls`/字体不算依赖
fn scan_hints(sources: Vec<String>, local: &std::collections::HashSet<String>) -> (Vec<String>, Vec<String>) {
    let mut packages: Vec<String> = Vec::new();
    let mut fonts: Vec<String> = Vec::new();
    let push = |list: &mut Vec<String>, v: String| {
        if !v.is_empty() && list.len() < HINTS_MAX && !list.contains(&v) {
            list.push(v);
        }
    };
    for s in sources {
        let code: String = s.lines().map(strip_comment).collect::<Vec<_>>().join("\n");
        for (cmd, args) in commands(&code) {
            match cmd {
                "documentclass" | "LoadClass" => {
                    if let Some(c) = args.first() {
                        if !local.contains(&format!("{}.cls", c.to_ascii_lowercase())) {
                            push(&mut packages, format!("{c}.cls"));
                        }
                    }
                }
                "usepackage" | "RequirePackage" => {
                    for pkg in args.first().map(|a| a.split(',')).into_iter().flatten() {
                        let pkg = pkg.trim();
                        if is_plain_name(pkg) && !local.contains(&format!("{}.sty", pkg.to_ascii_lowercase())) {
                            push(&mut packages, pkg.to_string());
                        }
                    }
                }
                _ => {
                    // 字体命令的「字体名」参数位置不同：\setCJKfamilyfont{族}{字体}
                    let font = if cmd == "setCJKfamilyfont" { args.get(1) } else { args.first() };
                    if let Some(f) = font.map(|f| f.trim()) {
                        let file = f.rsplit('/').next().unwrap_or(f).to_ascii_lowercase();
                        if !f.is_empty() && !f.contains('\\') && !local.contains(&file) {
                            push(&mut fonts, f.to_string());
                        }
                    }
                }
            }
        }
    }
    (packages, fonts)
}

const FONT_CMDS: &[&str] = &[
    "setmainfont", "setsansfont", "setmonofont", "setmathfont", "fontspec", "newfontfamily",
    "setCJKmainfont", "setCJKsansfont", "setCJKmonofont", "setCJKfamilyfont", "newCJKfontfamily",
];

/// 认出关心的命令及其 `{}` 参数（`[]` 选项跳过；`\newfontfamily\foo` 的控制序列跳过）。
fn commands(code: &str) -> Vec<(&'static str, Vec<String>)> {
    let wanted = ["documentclass", "LoadClass", "usepackage", "RequirePackage"]
        .iter()
        .chain(FONT_CMDS)
        .copied();
    let wanted: Vec<&'static str> = wanted.collect();
    let mut out = Vec::new();
    let b = code.as_bytes();
    let mut i = 0;
    while let Some(off) = code[i..].find('\\') {
        let start = i + off + 1;
        let end = start + code[start..].find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(code.len() - start);
        i = end.max(start);
        let Some(&cmd) = wanted.iter().find(|w| **w == &code[start..end]) else { continue };
        // 只关心 \documentclass{}、\newfontfamily\x{} 这类：参数要几个就收几个
        let need = if cmd == "setCJKfamilyfont" { 2 } else { 1 };
        let mut args = Vec::new();
        let mut j = end;
        loop {
            // 只跳 ASCII 空白：UTF-8 续字节按 Latin-1 看可能像空白，跳进去会切坏字符
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= b.len() || args.len() == need {
                break;
            }
            match b[j] {
                b'[' => match code[j..].find(']') {
                    Some(k) => j += k + 1,
                    None => break,
                },
                b'\\' if (cmd == "newfontfamily" || cmd == "newCJKfontfamily") && args.is_empty() => {
                    j += 1;
                    while j < b.len() && b[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                }
                b'{' => match code[j..].find('}') {
                    Some(k) => {
                        args.push(code[j + 1..j + k].trim().to_string());
                        j += k + 1;
                    }
                    None => break,
                },
                _ => break,
            }
        }
        if !args.is_empty() {
            out.push((cmd, args));
        }
        i = j.max(i);
    }
    out
}

/// 宏包名里有 `\`、`#` 之类的是宏展开出来的，不记
fn is_plain_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("subo-tpl-{tag}-{}", now_nanos()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    const DOC: &str = "\\documentclass[UTF8]{ctexart}\n\\usepackage{amsmath, mystyle}\n% \\usepackage{commented}\n\\setCJKmainfont{Source Han Serif SC}[Bold=*]\n\\newfontfamily\\code[Scale=0.9]{JetBrains Mono}\n\\setmainfont{fonts/Local.otf}\n\\begin{document}\nx\n\\end{document}\n";

    fn project(tag: &str) -> PathBuf {
        let p = scratch(tag);
        fs::write(p.join("main.tex"), DOC).unwrap();
        fs::write(p.join("mystyle.sty"), "\\RequirePackage{xcolor}\n").unwrap();
        fs::create_dir(p.join("fonts")).unwrap();
        fs::write(p.join("fonts/Local.otf"), b"OTTO....").unwrap();
        fs::write(p.join("logo.png"), b"\x89PNG\r\n").unwrap();
        fs::write(p.join("main.aux"), "junk").unwrap();
        fs::write(p.join("main.pdf"), "%PDF-1.5").unwrap();
        fs::create_dir(p.join(".git")).unwrap();
        fs::write(p.join(".git/HEAD"), "ref").unwrap();
        p
    }

    #[test]
    fn save_project_then_use() {
        let (src, home) = (project("src"), scratch("home"));
        let dir = dir(&home);
        let stored = from_project(&dir, &src, "我的 模板").unwrap();
        assert_eq!(stored.id, "我的-模板");
        // .git 在复制时就跳过了；编译产物、同名 PDF 被丢
        assert_eq!(stored.dropped, vec!["main.aux", "main.pdf"]);
        let files = dir.join("我的-模板/files");
        assert!(files.join("logo.png").is_file());
        assert!(!files.join(".git").exists());

        let all = list_user(&dir);
        assert_eq!(all.len(), 1);
        let m = &all[0].meta;
        assert_eq!((m.name.as_str(), m.entry.as_str(), m.files), ("我的 模板", "main.tex", 4));
        assert_eq!(m.packages, vec!["ctexart.cls", "amsmath", "xcolor"]);
        assert_eq!(m.fonts, vec!["Source Han Serif SC", "JetBrains Mono"]);

        // 再存一次：id 和显示名都加后缀
        let again = from_project(&dir, &src, "我的 模板").unwrap();
        assert_eq!((again.id.as_str(), again.name.as_str()), ("我的-模板-2", "我的 模板-2"));
        remove(&dir, &again.id).unwrap();

        let pid = instantiate(&dir, "我的-模板", &home, "论文").unwrap();
        assert_eq!(pid, "论文");
        assert!(home.join("论文/main.tex").is_file());
        assert_eq!(instantiate(&dir, "我的-模板", &home, "论文").unwrap(), "论文-2");
        // 模板目录是隐藏的，不会混进项目列表
        assert!(library::list(&home).iter().all(|p| !p.id.starts_with('.')));

        remove(&dir, "我的-模板").unwrap();
        assert!(list_user(&dir).is_empty());
        assert!(remove(&dir, "../论文").is_err());
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn rejects_foreign_and_disguised_files() {
        let home = scratch("bad");
        let dir = dir(&home);

        let p = project("exe");
        fs::write(p.join("setup.exe"), "MZ").unwrap();
        fs::write(p.join("notes.docx"), "PK").unwrap();
        let e = from_project(&dir, &p, "x").err().unwrap();
        assert_eq!(e.code, "template.badFiles");
        assert_eq!(e.params["count"], 2);

        let p = project("fake");
        fs::write(p.join("photo.png"), "MZ not a png").unwrap();
        assert_eq!(from_project(&dir, &p, "x").err().unwrap().code, "template.mismatch");

        let p = project("gbk");
        fs::write(p.join("ch1.tex"), b"\xd6\xd0\xce\xc4").unwrap();
        assert_eq!(from_project(&dir, &p, "x").err().unwrap().code, "template.encoding");

        // 失败不留半成品
        assert!(list_user(&dir).is_empty());
        assert!(fs::read_dir(&dir).unwrap().next().is_none());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn needs_a_real_entry() {
        let home = scratch("entry");
        let p = scratch("noentry");
        fs::write(p.join("chapter.tex"), "\\section{x}\n").unwrap();
        assert_eq!(from_project(&dir(&home), &p, "x").err().unwrap().code, "template.noEntry");
        // \begin{document} 被注释掉也不算
        fs::write(p.join("main.tex"), "\\documentclass{article}\n%\\begin{document}\n").unwrap();
        assert_eq!(from_project(&dir(&home), &p, "x").err().unwrap().code, "template.noEntry");
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn categories_guessed_then_editable() {
        let (src, home) = (project("cat"), scratch("cat-home"));
        let dir = dir(&home);
        // ctexart → 论文
        let stored = from_project(&dir, &src, "文章").unwrap();
        assert_eq!(list_user(&dir)[0].meta.category.as_deref(), Some("paper"));

        set_category(&dir, &stored.id, "report").unwrap();
        assert_eq!(list_user(&dir)[0].meta.category.as_deref(), Some("report"));
        assert_eq!(set_category(&dir, &stored.id, "nope").err().unwrap().code, "template.badCategory");
        assert_eq!(set_category(&dir, "没有", "paper").err().unwrap().code, "template.notFound");

        // 早先存的模板没有分类字段：列出来时现猜
        let meta = dir.join(&stored.id).join(META);
        let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("category");
        fs::write(&meta, v.to_string()).unwrap();
        fs::write(dir.join(&stored.id).join("files/main.tex"), "\\documentclass{ctexbeamer}\n\\begin{document}\n\\end{document}\n").unwrap();
        assert_eq!(list_user(&dir)[0].meta.category.as_deref(), Some("slides"));
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn guesses_by_document_class() {
        let d = scratch("guess");
        let case = |doc: &str| {
            fs::write(d.join("main.tex"), doc).unwrap();
            guess_category(&d, "main.tex", "x")
        };
        assert_eq!(case("\\documentclass[t]{tudelft-beamerposter}"), "poster");
        assert_eq!(case("\\documentclass{article}\n\\usepackage{tikzposter}"), "poster");
        assert_eq!(case("\\documentclass[aspectratio=169]{beamer}"), "slides");
        assert_eq!(case("\\documentclass{friggeri-cv}"), "resume");
        assert_eq!(case("\\documentclass{zjuthesis}"), "thesis");
        assert_eq!(case("\\documentclass{ctexbook}"), "thesis");
        assert_eq!(case("\\documentclass[conference]{IEEEtran}"), "paper");
        assert_eq!(case("\\documentclass{gstarticle}"), "paper");
        assert_eq!(case("% \\documentclass{beamer}\n\\documentclass{letter}"), "other");
        // 名字里的关键词优先：ctexart 写的简历、报告
        fs::write(d.join("main.tex"), "\\documentclass{ctexart}").unwrap();
        assert_eq!(guess_category(&d, "main.tex", "我的简历底稿"), "resume");
        assert_eq!(guess_category(&d, "main.tex", "ModernFancyCV"), "resume");
        assert_eq!(guess_category(&d, "main.tex", "Q3 周报"), "report");
        assert_eq!(guess_category(&d, "main.tex", "rcvd notes"), "paper");
        fs::write(d.join("resume.tex"), "\\documentclass{article}").unwrap();
        assert_eq!(guess_category(&d, "resume.tex", "x"), "resume");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn presets_listed_used_and_read_only() {
        let home = scratch("preset");
        let dir = dir(&home);
        let all = list(&dir);
        assert_eq!(all.len(), presets::ALL.len());
        assert!(all.iter().all(|t| t.builtin && t.meta.source == "builtin"));
        // 预置模板只用内置宏包与字体，但线索照样扫：前端据此提示
        let thesis = all.iter().find(|t| t.id == "builtin:thesis-zh").unwrap();
        assert!(thesis.meta.packages.contains(&"ctexbook.cls".to_string()));
        assert_eq!(thesis.meta.files, 10);

        let pid = instantiate(&dir, "builtin:thesis-zh", &home, "我的论文").unwrap();
        assert!(home.join(&pid).join("chapters/intro.tex").is_file());
        assert_eq!(project::entry(&home.join(&pid)).as_deref(), Some("main.tex"));

        assert_eq!(remove(&dir, "builtin:poster").err().unwrap().code, "template.builtin");
        assert_eq!(set_category(&dir, "builtin:poster", "other").err().unwrap().code, "template.builtin");
        assert_eq!(instantiate(&dir, "builtin:nope", &home, "x").err().unwrap().code, "template.notFound");
        let _ = fs::remove_dir_all(&home);
    }

    /// presets/ 下的每个文件都要登记在 presets::ALL 里，漏登的不会被编进去
    #[test]
    fn every_preset_file_is_embedded() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("presets");
        let mut on_disk = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if !e.file_name().to_string_lossy().starts_with('.') {
                    on_disk.push(p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut embedded: Vec<String> =
            presets::ALL.iter().flat_map(|p| p.files.iter().map(move |(f, _)| format!("{}/{f}", p.id))).collect();
        on_disk.sort();
        embedded.sort();
        assert_eq!(on_disk, embedded);
    }

    #[test]
    fn comments_respect_escaped_percent() {
        assert_eq!(strip_comment("50\\% done % note"), "50\\% done ");
        assert_eq!(strip_comment("% all"), "");
    }
}
