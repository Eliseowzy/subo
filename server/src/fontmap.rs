//! 让 TeX Live 自带的字体按名字也能找到。
//!
//! XeTeX 按名字找字体只问操作系统（macOS 走 Core Text，Linux 走 fontconfig），看不见 texmf 树：
//! `\setmainfont{Latin Modern Roman}`、fontawesome 宏包的 `\newfontfamily{\FA}{FontAwesome}`
//! 在没把字体装进系统的机器上一律报 cannot be found。内置 TeX Live 是这样，用户自己装的
//! MacTeX / TeX Live 也一样。
//!
//! fontspec 留了口子：按名字加载 `Foo Bar` 前会先读 `FooBar.fontspec`（去掉空格）。
//! 这里读当前 TeX Live 里每个 OTF / TTF 的 name 表和 OS/2 表，按族挑出正体 / 粗体 / 斜体 / 粗斜体，
//! 写成 `\defaultfontfeatures[Foo Bar]{Extension=..., UprightFont=..., ...}`，放在
//! `~/.subo/fontspec/tex/latex/subo-fontspec/`。编译时用 `TEXMFAUXTREES` 把这棵树挂到最前面，
//! 不改动任何 TeX Live 的文件。TeX Live 自己已经带了同名 `.fontspec` 的族（newtx、inconsolata）跳过。
//!
//! 树根由 engine::aux_trees 跟项目里的 texmf 树一起挂上。
//!
//! 第一次编译前生成一次（服务启动时就在后台开始），缺字清单装完宏包后重新生成。

use crate::engine;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// CSS / OS/2 的字重：Regular 400、Semibold 600、Bold 700
const REGULAR: u16 = 400;
const BOLD: u16 = 700;
const BOLD_MIN: u16 = 600;
/// 光学尺寸（Latin Modern 的 `10 Regular`）缺省按 10pt 正文挑
const TEXT_SIZE: f32 = 10.0;
const SUBDIR: &str = "tex/latex/subo-fontspec";
/// 树根下记着生成时的字体指纹，见 [`fingerprint`]
const STAMP: &str = ".stamp";
const KPSE_TIMEOUT: Duration = Duration::from_secs(20);

/// `None`：还没生成；`Some(None)`：生成不了（没有 kpsewhich 等），按没有映射编译
static TREE: Mutex<Option<Option<PathBuf>>> = Mutex::new(None);

/// 给 `TEXMFAUXTREES` 用的树根。第一次调用时生成，之后直接返回。
pub fn tree() -> Option<PathBuf> {
    let mut t = TREE.lock().unwrap();
    if t.is_none() {
        *t = Some(generate());
    }
    t.clone().flatten()
}

/// 字体变了（缺字清单装了宏包）后重新生成
pub fn refresh() {
    let fresh = generate();
    *TREE.lock().unwrap() = Some(fresh);
    *CATALOG.lock().unwrap() = None;
}


fn generate() -> Option<PathBuf> {
    engine::which("kpsewhich")?;
    let started = Instant::now();
    let paths: Vec<PathBuf> = font_dirs().iter().flat_map(|d| walk(d)).collect();
    let root = crate::library::subo_dir().join("fontspec");
    // 命令行每次都是新进程：字体文件一个没变就沿用上次生成的，不再逐个读 name 表
    let stamp = fingerprint(&paths);
    if root.join(SUBDIR).is_dir() && fs::read_to_string(root.join(STAMP)).ok().as_deref() == Some(stamp.as_str()) {
        return Some(root);
    }
    let faces: Vec<Face> = paths.iter().filter_map(|p| read_face(p)).collect();
    let stems: Vec<String> = families(&faces).keys().map(|f| stem(f)).collect();
    let upstream = upstream_fontspecs(&stems);
    let files = build(&faces, &upstream);

    // 先写到旁边再换上去：半写好的映射比没有更糟
    let tmp = root.with_extension(format!("tmp-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let dir = tmp.join(SUBDIR);
    fs::create_dir_all(&dir).ok()?;
    for (name, text) in &files {
        fs::write(dir.join(name), text).ok()?;
    }
    fs::write(tmp.join(STAMP), &stamp).ok()?;
    let _ = fs::remove_dir_all(&root);
    fs::rename(&tmp, &root).ok()?;
    eprintln!(
        "  字体映射: {} 个字体族（{} 个字体文件，{} ms）",
        files.len(),
        faces.len(),
        started.elapsed().as_millis()
    );
    Some(root)
}

/// 生成结果的指纹：全部字体文件的路径、大小、修改时间，加上映射格式的版本（改了生成逻辑就加一）
fn fingerprint(paths: &[PathBuf]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(format!("fontmap-v1 {}\n", env!("CARGO_PKG_VERSION")));
    for p in paths {
        let (len, mtime) = fs::metadata(p)
            .map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos())))
            .unwrap_or_default();
        h.update(format!("{}\t{len}\t{mtime}\n", p.display()));
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 当前 TeX Live 各棵树里放 OpenType / TrueType 的目录
fn font_dirs() -> Vec<PathBuf> {
    tex_font_dirs().into_iter().map(|(d, _)| d).collect()
}

/// 同上，带来源：`user` 是用户层（TEXMFHOME，补装的宏包在这里），其余算 `tex`
fn tex_font_dirs() -> Vec<(PathBuf, &'static str)> {
    let mut roots: Vec<(PathBuf, &'static str)> = Vec::new();
    for var in ["TEXMFHOME", "TEXMFLOCAL", "TEXMFDIST"] {
        let source = if var == "TEXMFHOME" { "user" } else { "tex" };
        let Some(val) = kpse(&[&format!("-var-value={var}")]) else { continue };
        for part in val.trim().trim_matches(|c| c == '{' || c == '}').split(',') {
            let part = part.trim().trim_start_matches("!!");
            let path = match part.strip_prefix("~/") {
                Some(rest) => match crate::library::home_dir() {
                    Some(h) => h.join(rest),
                    None => continue,
                },
                None => PathBuf::from(part),
            };
            if path.is_absolute() && !roots.iter().any(|(r, _)| r == &path) {
                roots.push((path, source));
            }
        }
    }
    roots
        .iter()
        .flat_map(|(r, src)| [(r.join("fonts/opentype"), *src), (r.join("fonts/truetype"), *src)])
        .filter(|(d, _)| d.is_dir())
        .collect()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    walk_ext(dir, &["otf", "ttf"])
}

fn walk_ext(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if ext(&p).is_some_and(|e| exts.contains(&e.as_str())) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn ext(p: &Path) -> Option<String> {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// TeX Live 自己带的 `<stem>.fontspec`（小写）。此时进程环境里没有挂我们的树，查到的都是上游的
fn upstream_fontspecs(stems: &[String]) -> HashSet<String> {
    let args: Vec<String> = stems.iter().map(|s| format!("{s}.fontspec")).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    kpse(&refs)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| Path::new(l.trim()).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()))
        .collect()
}

fn kpse(args: &[&str]) -> Option<String> {
    if args.is_empty() {
        return Some(String::new());
    }
    // 在临时目录里跑，项目目录里的同名文件不算
    let ran = engine::run(
        crate::engine::command("kpsewhich").args(args).current_dir(std::env::temp_dir()),
        Instant::now() + KPSE_TIMEOUT,
    )
    .ok()?;
    // 有一个没找到时退出码就非 0，找到的照样打印
    Some(ran.log)
}

#[derive(Clone, Debug)]
struct Face {
    /// 不带扩展名的文件名，fontspec 的 UprightFont 等就写它
    file: String,
    ext: String,
    families: Vec<String>,
    /// 最具体的样式名，如 `10 Bold Italic`
    style: String,
    weight: u16,
    italic: bool,
    size: f32,
}

impl Face {
    fn new(file: &str, ext: &str, families: Vec<String>, style: &str, weight: u16, italic_bit: bool, bold_bit: bool) -> Face {
        let words: Vec<String> = style.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
        let size = words.first().and_then(|w| w.parse::<f32>().ok()).unwrap_or(TEXT_SIZE);
        // 有的字体元数据不全：FandolSong-Bold 的字重标的是常规，只能看样式名
        let named_bold = bold_bit || words.iter().any(|w| w == "bold");
        let weight = if named_bold && weight < BOLD_MIN { BOLD } else { weight };
        let italic = italic_bit || words.iter().any(|w| w == "italic" || w == "oblique");
        Face { file: file.into(), ext: ext.into(), families, style: style.into(), weight, italic, size }
    }

    fn rank(&self, weight: u16) -> (u16, u32, bool, String) {
        let bare: Vec<String> = self
            .style
            .split_whitespace()
            .filter(|w| !w.starts_with(|c: char| c.is_ascii_digit()))
            .map(|w| w.to_ascii_lowercase())
            .collect();
        let plain = matches!(bare.join(" ").as_str(), "regular" | "bold" | "italic" | "bold italic");
        (self.weight.abs_diff(weight), ((self.size - TEXT_SIZE).abs() * 100.0) as u32, !plain, self.file.clone())
    }
}

fn families(faces: &[Face]) -> BTreeMap<String, Vec<&Face>> {
    let mut by: BTreeMap<String, Vec<&Face>> = BTreeMap::new();
    for f in faces {
        for fam in &f.families {
            by.entry(fam.clone()).or_default().push(f);
        }
    }
    by
}

fn stem(family: &str) -> String {
    family.chars().filter(|c| *c != ' ').collect()
}

fn name_ok(family: &str) -> bool {
    family.starts_with(|c: char| c.is_ascii_alphanumeric())
        && family.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-'))
}

/// 所有要写的 `(文件名, 内容)`
fn build(faces: &[Face], upstream: &HashSet<String>) -> Vec<(String, String)> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (fam, group) in families(faces) {
        let st = stem(&fam);
        let key = st.to_ascii_lowercase();
        // 跳过上游已有的；两个族去掉空格后撞名（大小写也算，macOS 文件系统不分）先到先得
        if !name_ok(&fam) || upstream.contains(&key) || !taken.insert(key) {
            continue;
        }
        let (ext, slots) = pick(&group);
        out.push((format!("{st}.fontspec"), render(&fam, &ext, &slots)));
    }
    out
}

/// 一族字体挑出四个槽位；正体必须有，其余没有就不写（fontspec 会退回正体并警告）
fn pick<'a>(faces: &[&'a Face]) -> (String, Vec<(&'static str, &'a Face)>) {
    let best = |pool: Vec<&'a Face>, w: u16| pool.into_iter().min_by_key(|f| f.rank(w));
    let sel = |italic: bool, bold: bool| -> Vec<&'a Face> {
        faces.iter().copied().filter(|f| f.italic == italic && (f.weight >= BOLD_MIN) == bold).collect()
    };
    let mut slots: Vec<(&'static str, &'a Face)> = Vec::new();
    match best(sel(false, false), REGULAR) {
        Some(up) => {
            slots.push(("UprightFont", up));
            for (key, italic, bold, w) in
                [("BoldFont", false, true, BOLD), ("ItalicFont", true, false, REGULAR), ("BoldItalicFont", true, true, BOLD)]
            {
                if let Some(f) = best(sel(italic, bold), w) {
                    slots.push((key, f));
                }
            }
        }
        // 只有斜体或只有粗体的族，拿最接近常规的那个当正体
        None => slots.push(("UprightFont", best(faces.to_vec(), REGULAR).expect("族里至少一个字体"))),
    }
    // 一族只能有一个 Extension：混了 .ttf / .otf 的只留跟正体相同的
    let ext = slots[0].1.ext.clone();
    slots.retain(|(_, f)| f.ext == ext);
    (ext, slots)
}

fn render(family: &str, ext: &str, slots: &[(&str, &Face)]) -> String {
    let st = stem(family);
    let names = if st == family { family.to_string() } else { format!("{family},{st}") };
    let body: String = slots.iter().map(|(k, f)| format!("  {k} = {} ,\n", f.file)).collect();
    format!(
        "% 由 Subo 生成（server/src/fontmap.rs）：让 fontspec 按名字也能找到 TeX Live 自带的字体\n\
         \\defaultfontfeatures[{names}]\n  {{\n  Extension = .{ext} ,\n{body}  }}\n"
    )
}

// ---- 读字体文件：只要 name 表和 OS/2 表的几个字段 ----

fn read_face(path: &Path) -> Option<Face> {
    // 集合字体（ttcf）要 FontIndex，按文件名映射说不清是哪一个，不管
    read_face_at(&mut fs::File::open(path).ok()?, path, 0)
}

/// 集合字体里的每一个也读出来；只给设置页的字体清单用
fn read_faces(path: &Path) -> Vec<Face> {
    let Ok(mut f) = fs::File::open(path) else { return Vec::new() };
    let Some(head) = read_at(&mut f, 0, 12) else { return Vec::new() };
    if &head[..4] != b"ttcf" {
        return read_face_at(&mut f, path, 0).into_iter().collect();
    }
    let n = u32be(&head, 8).unwrap_or(0).min(64) as usize;
    let Some(offs) = read_at(&mut f, 12, n * 4) else { return Vec::new() };
    (0..n).filter_map(|i| read_face_at(&mut f, path, u32be(&offs, i * 4)? as u64)).collect()
}

/// `start`：这个字体的表目录在文件里的位置；单个字体是 0。集合字体里各表的偏移也从文件头算起
fn read_face_at(f: &mut fs::File, path: &Path, start: u64) -> Option<Face> {
    let head = read_at(f, start, 12)?;
    if !matches!(&head[..4], [0, 1, 0, 0] | b"OTTO" | b"true") {
        return None;
    }
    let n = u16be(&head, 4)? as usize;
    let dir = read_at(f, start + 12, n * 16)?;
    let table = |tag: &[u8; 4]| {
        (0..n).find_map(|i| {
            let r = &dir[i * 16..i * 16 + 16];
            (&r[..4] == tag).then(|| (u32be(r, 8).unwrap() as u64, u32be(r, 12).unwrap() as usize))
        })
    };
    let (off, len) = table(b"name")?;
    let names = parse_names(&read_at(f, off, len.min(1 << 20))?);
    let (weight, italic_bit, bold_bit) = match table(b"OS/2") {
        Some((off, len)) if len >= 64 => {
            let os2 = read_at(f, off, 64)?;
            let sel = u16be(&os2, 62)?;
            (u16be(&os2, 4)?, sel & 1 != 0, sel & 0x20 != 0)
        }
        _ => (REGULAR, false, false),
    };

    let mut families: Vec<String> = Vec::new();
    for id in [16, 1] {
        if let Some(n) = names.get(&id) {
            if !families.contains(n) {
                families.push(n.clone());
            }
        }
    }
    if families.is_empty() {
        return None;
    }
    let style = names.get(&17).or_else(|| names.get(&2)).cloned().unwrap_or_else(|| "Regular".into());
    let file = path.file_stem()?.to_str()?;
    Some(Face::new(file, &ext(path)?, families, &style, weight, italic_bit, bold_bit))
}

fn read_at(f: &mut fs::File, off: u64, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0; len];
    f.seek(SeekFrom::Start(off)).ok()?;
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn u16be(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32be(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// name 表里 ID 1 / 2 / 16 / 17 的英文名。优先 Windows 平台（UTF-16BE，美式英语），其次 Mac Roman
fn parse_names(t: &[u8]) -> BTreeMap<u16, String> {
    let mut out: BTreeMap<u16, (u8, String)> = BTreeMap::new();
    let (Some(count), Some(base)) = (u16be(t, 2), u16be(t, 4)) else { return BTreeMap::new() };
    for i in 0..count as usize {
        let r = 6 + i * 12;
        let (Some(platform), Some(encoding), Some(lang), Some(id), Some(len), Some(off)) =
            (u16be(t, r), u16be(t, r + 2), u16be(t, r + 4), u16be(t, r + 6), u16be(t, r + 8), u16be(t, r + 10))
        else {
            break;
        };
        if !matches!(id, 1 | 2 | 16 | 17) {
            continue;
        }
        let start = base as usize + off as usize;
        let Some(raw) = t.get(start..start + len as usize) else { continue };
        let (rank, text) = match (platform, encoding, lang) {
            (3, 0 | 1 | 10, 0x409) => (0, utf16be(raw)),
            (3, 0 | 1 | 10, _) => (2, utf16be(raw)),
            (1, 0, 0) => (1, raw.iter().map(|&b| if b < 0x80 { b as char } else { '?' }).collect()),
            _ => continue,
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        if out.get(&id).is_none_or(|(r, _)| rank < *r) {
            out.insert(id, (rank, text));
        }
    }
    out.into_iter().map(|(k, (_, v))| (k, v)).collect()
}

fn utf16be(raw: &[u8]) -> String {
    let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

// ---- 设置页的字体清单 ----

/// 一个字体族
#[derive(Clone, Debug, Serialize)]
pub struct Family {
    pub name: String,
    /// 样式名，如 Regular、Bold Italic
    pub styles: Vec<String>,
    /// `tex`：TeX Live 自带；`user`：补装进用户层的；`system`：操作系统里装的（XeTeX 按名字就能找到）
    pub source: &'static str,
    /// 用户层字体所属的宏包：卸载字体就是卸载这些包。查不到（手放进去的）为空
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    /// 字体文件加起来的字节数；一个 .ttc 里装着几个族时，各族都算它一整份
    pub bytes: u64,
    /// 能给浏览器预览的样式（单个字体文件，不含 .ttc 集合：浏览器读不了）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub previews: Vec<String>,
    /// 与 `previews` 一一对应的文件，只在服务端用
    #[serde(skip)]
    pub files: Vec<PathBuf>,
}

static CATALOG: Mutex<Option<Vec<Family>>> = Mutex::new(None);

/// 所有能用的字体族。扫一遍要读上千个文件，结果记下来；`rescan` 或装了宏包后重扫
pub fn catalog(rescan: bool) -> Vec<Family> {
    if !rescan {
        if let Some(c) = CATALOG.lock().unwrap().as_ref() {
            return c.clone();
        }
    }
    let mut dirs = tex_font_dirs();
    dirs.extend(system_font_dirs().into_iter().map(|d| (d, "system")));
    let owners = if dirs.iter().any(|(_, s)| *s == "user") { crate::case::user_file_owners() } else { BTreeMap::new() };
    let home = crate::tex::var("TEXMFHOME").map(PathBuf::from);
    let homes: Vec<PathBuf> = home.iter().flat_map(|h| [h.clone(), dunce::canonicalize(&h).unwrap_or_else(|_| h.clone())]).collect();
    type Entry = (Vec<String>, Vec<String>, u64, Vec<(String, PathBuf)>);
    let mut by: BTreeMap<(String, &'static str), Entry> = BTreeMap::new();
    for (dir, source) in dirs {
        for path in walk_ext(&dir, &["otf", "ttf", "ttc", "otc"]) {
            let owner = (source == "user").then(|| owner_of(&owners, &homes, &path)).flatten();
            let size = std::fs::metadata(&path).map_or(0, |m| m.len());
            // 同一个文件里同一族的几个字面只算一次
            let mut counted: Vec<String> = Vec::new();
            let faces = read_faces(&path);
            let single = faces.len() == 1 && !matches!(ext(&path).as_deref(), Some("ttc" | "otc"));
            for face in faces {
                // 族名取首选的那个（ID 16 优先）；点开头的是 macOS 的界面私有字体，用户用不上
                let Some(name) = face.families.first().filter(|n| !n.starts_with('.')) else { continue };
                let (styles, pkgs, bytes, files) = by.entry((name.clone(), source)).or_default();
                if single && !files.iter().any(|(st, _)| *st == face.style) {
                    files.push((face.style.clone(), path.clone()));
                }
                if !counted.contains(name) {
                    counted.push(name.clone());
                    *bytes += size;
                }
                if !styles.contains(&face.style) {
                    styles.push(face.style.clone());
                }
                if let Some(o) = owner.filter(|o| !pkgs.contains(o)) {
                    pkgs.push(o.clone());
                }
            }
        }
    }
    let mut out: Vec<Family> = by
        .into_iter()
        .map(|((name, source), (styles, packages, bytes, files))| {
            let (previews, files) = files.into_iter().unzip();
            Family { name, styles, source, packages, bytes, previews, files }
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.source.cmp(b.source)));
    *CATALOG.lock().unwrap() = Some(out.clone());
    out
}

/// 预览用的字体文件：只认清单里有的族和样式，不按传进来的路径读文件
pub fn preview_file(name: &str, source: &str, style: &str) -> Option<PathBuf> {
    let list = catalog(false);
    let fam = list.iter().find(|f| f.name == name && f.source == source)?;
    let i = fam.previews.iter().position(|s| s == style)?;
    fam.files.get(i).cloned()
}

/// 用户层字体文件属于哪个包：按相对用户层根的路径查。kpsewhich 报的树根可能是解开链接的，两种都试
fn owner_of<'a>(owners: &'a BTreeMap<String, String>, homes: &[PathBuf], path: &Path) -> Option<&'a String> {
    let real = dunce::canonicalize(&path).unwrap_or_else(|_| path.to_path_buf());
    homes.iter().find_map(|h| {
        let rel = path.strip_prefix(h).or_else(|_| real.strip_prefix(h)).ok()?;
        owners.get(&rel.to_string_lossy().replace('\\', "/"))
    })
}

/// 操作系统的字体目录
fn system_font_dirs() -> Vec<PathBuf> {
    let home = crate::library::home_dir();
    let mut out: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec!["/System/Library/Fonts".into(), "/Library/Fonts".into()]
    } else if cfg!(windows) {
        let win = std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let mut v = vec![win.join("Fonts")];
        // 不用管理员权限、「为我安装」的字体在这里
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            v.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
        v
    } else {
        vec!["/usr/share/fonts".into(), "/usr/local/share/fonts".into()]
    };
    if let Some(h) = home {
        if cfg!(target_os = "macos") {
            out.push(h.join("Library/Fonts"));
        } else if !cfg!(windows) {
            out.push(h.join(".local/share/fonts"));
            out.push(h.join(".fonts"));
        }
    }
    out.retain(|d| d.is_dir());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(file: &str, fams: &[&str], style: &str, weight: u16, italic: bool) -> Face {
        Face::new(file, "otf", fams.iter().map(|s| s.to_string()).collect(), style, weight, italic, false)
    }

    fn files(faces: &[Face]) -> BTreeMap<String, String> {
        build(faces, &HashSet::new()).into_iter().collect()
    }

    #[test]
    fn latin_modern_picks_10pt_and_both_names() {
        let mut faces = Vec::new();
        for size in ["10", "12", "5"] {
            faces.push(face(&format!("lmroman{size}-regular"), &["Latin Modern Roman", &format!("LM Roman {size}")], &format!("{size} Regular"), 400, false));
            faces.push(face(&format!("lmroman{size}-bold"), &["Latin Modern Roman", &format!("LM Roman {size}")], &format!("{size} Bold"), 700, false));
        }
        faces.push(face("lmroman10-italic", &["Latin Modern Roman", "LM Roman 10"], "10 Italic", 400, true));
        faces.push(face("lmroman10-bolditalic", &["Latin Modern Roman", "LM Roman 10"], "10 Bold Italic", 700, true));
        let out = files(&faces);
        let lm = &out["LatinModernRoman.fontspec"];
        assert!(lm.contains("\\defaultfontfeatures[Latin Modern Roman,LatinModernRoman]"));
        assert!(lm.contains("Extension = .otf"));
        assert!(lm.contains("UprightFont = lmroman10-regular ,"));
        assert!(lm.contains("BoldFont = lmroman10-bold ,"));
        assert!(lm.contains("ItalicFont = lmroman10-italic ,"));
        assert!(lm.contains("BoldItalicFont = lmroman10-bolditalic ,"));
        // 按光学尺寸分的族也各有一份
        assert!(out["LMRoman12.fontspec"].contains("UprightFont = lmroman12-regular ,"));
    }

    #[test]
    fn bold_named_but_weight_mislabelled() {
        // 真实情况：FandolSong-Bold 的 usWeightClass 是 400
        let faces = [face("FandolSong-Regular", &["FandolSong"], "Regular", 400, false), face("FandolSong-Bold", &["FandolSong"], "Bold", 400, false)];
        let fs = &files(&faces)["FandolSong.fontspec"];
        assert!(fs.contains("UprightFont = FandolSong-Regular ,"));
        assert!(fs.contains("BoldFont = FandolSong-Bold ,"));
        assert!(fs.starts_with('%') && fs.contains("[FandolSong]"), "没空格的名字不重复写");
    }

    #[test]
    fn bold_beats_semibold_and_black() {
        let fam = &["Source Sans 3"];
        let faces = [
            face("SourceSans3-Regular", fam, "Regular", 400, false),
            face("SourceSans3-Light", &["Source Sans 3", "Source Sans 3 Light"], "Light", 300, false),
            face("SourceSans3-Semibold", &["Source Sans 3", "Source Sans 3 Semibold"], "Semibold", 600, false),
            face("SourceSans3-Bold", fam, "Bold", 700, false),
            face("SourceSans3-Black", &["Source Sans 3", "Source Sans 3 Black"], "Black", 900, false),
        ];
        let fs = &files(&faces)["SourceSans3.fontspec"];
        assert!(fs.contains("UprightFont = SourceSans3-Regular ,"));
        assert!(fs.contains("BoldFont = SourceSans3-Bold ,"));
        assert!(!fs.contains("ItalicFont"));
    }

    #[test]
    fn single_face_and_italic_only_family() {
        let faces = [face("FontAwesome", &["FontAwesome"], "Regular", 400, false), face("Foo-Italic", &["Foo Script"], "Italic", 400, true)];
        let out = files(&faces);
        assert_eq!(out["FontAwesome.fontspec"].matches("Font =").count(), 1);
        assert!(out["FooScript.fontspec"].contains("UprightFont = Foo-Italic ,"));
    }

    #[test]
    fn skips_upstream_odd_names_and_mixed_extensions() {
        let mut ttf = face("Mixed-Bold", &["Mixed"], "Bold", 700, false);
        ttf.ext = "ttf".into();
        let faces = [
            face("TeXGyreTermesX-Regular", &["TeX Gyre TermesX"], "Regular", 400, false),
            face("weird", &["Wéird Font"], "Regular", 400, false),
            face("Mixed-Regular", &["Mixed"], "Regular", 400, false),
            ttf,
        ];
        let upstream: HashSet<String> = ["texgyretermesx".to_string()].into();
        let out: BTreeMap<_, _> = build(&faces, &upstream).into_iter().collect();
        assert_eq!(out.keys().collect::<Vec<_>>(), vec!["Mixed.fontspec"]);
        assert!(!out["Mixed.fontspec"].contains("BoldFont"), "跟正体扩展名不同的不写");
    }

    /// 拼一个只有 name 和 OS/2 表的最小字体文件
    fn tiny_font(names: &[(u16, u16, &str)], weight: u16, fs_selection: u16) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut records = Vec::new();
        for &(platform, id, text) in names {
            let bytes: Vec<u8> = if platform == 3 {
                text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
            } else {
                text.bytes().collect()
            };
            let (enc, lang) = if platform == 3 { (1u16, 0x409u16) } else { (0, 0) };
            for v in [platform, enc, lang, id, bytes.len() as u16, strings.len() as u16] {
                records.extend(v.to_be_bytes());
            }
            strings.extend(bytes);
        }
        let mut name = Vec::new();
        for v in [0u16, names.len() as u16, (6 + records.len()) as u16] {
            name.extend(v.to_be_bytes());
        }
        name.extend(records);
        name.extend(strings);
        let mut os2 = vec![0u8; 78];
        os2[4..6].copy_from_slice(&weight.to_be_bytes());
        os2[62..64].copy_from_slice(&fs_selection.to_be_bytes());

        let mut out = b"OTTO".to_vec();
        out.extend([0u8, 2, 0, 0, 0, 0, 0, 0]);
        let name_off = 12 + 2 * 16;
        let os2_off = name_off + name.len();
        for (tag, off, len) in [(b"OS/2", os2_off, os2.len()), (b"name", name_off, name.len())] {
            out.extend(tag);
            out.extend([0u8; 4]);
            out.extend((off as u32).to_be_bytes());
            out.extend((len as u32).to_be_bytes());
        }
        out.extend(name);
        out.extend(os2);
        out
    }

    #[test]
    fn reads_names_weight_and_italic_from_file() {
        let dir = std::env::temp_dir().join(format!("subo-fontmap-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("lmroman10-bolditalic.otf");
        fs::write(
            &p,
            tiny_font(
                &[(1, 1, "LM Roman 10"), (3, 1, "LM Roman 10"), (3, 2, "Bold Italic"), (3, 16, "Latin Modern Roman"), (3, 17, "10 Bold Italic")],
                700,
                0x21,
            ),
        )
        .unwrap();
        let f = read_face(&p).unwrap();
        assert_eq!(f.file, "lmroman10-bolditalic");
        assert_eq!(f.ext, "otf");
        assert_eq!(f.families, vec!["Latin Modern Roman", "LM Roman 10"]);
        assert_eq!(f.style, "10 Bold Italic");
        assert_eq!((f.weight, f.italic, f.size), (700, true, 10.0));

        // 只有 Mac 平台名字、没有 OS/2 的老字体也能读
        let p2 = dir.join("Old.ttf");
        let mut bytes = tiny_font(&[(1, 1, "Old Face"), (1, 2, "Regular")], 400, 0);
        bytes[4..6].copy_from_slice(&1u16.to_be_bytes()); // 只留第一张表（OS/2）……
        bytes[12..16].copy_from_slice(b"name"); // ……改成 name，并指过去
        let name_off = (12 + 32) as u32;
        bytes[20..24].copy_from_slice(&name_off.to_be_bytes());
        let name_len = (bytes.len() - 78 - name_off as usize) as u32;
        bytes[24..28].copy_from_slice(&name_len.to_be_bytes());
        fs::write(&p2, bytes).unwrap();
        let g = read_face(&p2).unwrap();
        assert_eq!((g.families.clone(), g.style.as_str(), g.weight, g.italic), (vec!["Old Face".to_string()], "Regular", 400, false));

        // 集合字体和不是字体的文件不认
        fs::write(dir.join("c.ttf"), b"ttcf\0\x01\0\0\0\0\0\0").unwrap();
        assert!(read_face(&dir.join("c.ttf")).is_none());
        fs::write(dir.join("x.otf"), b"hello").unwrap();
        assert!(read_face(&dir.join("x.otf")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
