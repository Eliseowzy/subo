//! 字盘（case）第一片：编译失败时说清楚「缺什么、属于哪个宏包」。
//!
//! 三步：
//! 1. 从编译日志里认出缺失文件（`.sty`、`\input` 的文件、TFM 字体度量），
//!    以及参考文献样式（`.bst` / `.bbx` / `.cbx`）——bibtex 缺样式时编译照样成功，也要报
//! 2. 用 `tlmgr search --global --file` 把文件名映射到 TeX Live 包名，
//!    结果落盘缓存，离线时查过的文件照样能答
//! 3. 用户点了确认才 `tlmgr install`；TeX Live 装在系统目录时走系统的管理员授权框。
//!    内置 TeX 装进用户层（`--usermode`），tlmgr 不肯装的不可重定位包（pdftex 等）
//!    改为下载容器、只取宏文件
//!
//! 按名字找的字体（fontspec / xeCJK 报的 `The font "X" cannot be found`）另算：
//! 字体名不是文件名，tlmgr 按文件查不到，多半是没装的系统字体，只认出来告诉用户，不查不装。
//! 例外是 TeX Live 自带、只是 XeTeX 按名字找不到的字体，用 kpsewhich 认出来，见 [`fonts`]；
//! 完整版 TeX Live 有、本机还没装的，按随包索引换成缺文件去装，见 [`installable_fonts`]。
//!
//! 外部程序也一样只诊断：EPS / PS 插图要 Ghostscript 转成 PDF，Subo 不带它
//! （AGPL、体积），缺了就在清单里点名，告诉用户去装。

use crate::{engine, settings};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// TeX 把日志按 79 列硬折行（max_print_line），长文件名会被拦腰截断。
const MAX_PRINT_LINE: usize = 79;

/// 单次 tlmgr 查询的上限：它要拉远端 tlpdb，正常 2 s 左右。
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(30);

/// 一次 kpsewhich 的上限：只读本地文件索引，正常几十毫秒。
const KPSE_TIMEOUT: Duration = Duration::from_secs(10);

/// 一次安装的上限：大字体包下载加解包可能要几分钟。
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// 从日志里认出的缺失文件，按出现顺序去重。
pub fn missing_files(log: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: String| {
        if valid_name(&name) && !out.contains(&name) {
            out.push(name);
        }
    };
    // 最近一条 `No file MDAcmr.fd.`：NFSS 找不到字体定义文件时只记一笔，不报错
    let mut last_fd: Option<String> = None;
    for line in unwrap_lines(log) {
        if let Some(fd) = line.trim().strip_prefix("No file ").and_then(|r| r.strip_suffix('.')) {
            if fd.ends_with(".fd") {
                last_fd = Some(fd.to_string());
            }
        }
        // ! LaTeX Error: This NFSS system isn't set up properly.
        // 编码的默认替换字体也没有定义文件（arevmath 的 MDA 编码要 mathdesign 的 mdacmr.fd），
        // 罪魁是紧挨着的那条 `No file`。TeX Live 里的 .fd 一律小写
        if line.contains("This NFSS system isn't set up properly") {
            if let Some(fd) = last_fd.take() {
                push(fd.to_ascii_lowercase());
            }
        }
        // ! LaTeX Error: File `xltxtra.sty' not found.
        // ! Package xxx Error: File `foo.cfg' not found ...
        // epstopdf 转出来的 `fig-eps-converted-to.pdf` 不是 TeX Live 的文件，缺它是缺 Ghostscript，
        // 交给 missing_tools
        if let Some(name) = between(&line, "File `", "' not found") {
            if eps_source(name).is_none() {
                push(name.to_string());
            }
        }
        // Package fontenc Error: Encoding file `ly1enc.def' not found.
        // 用了某个编码（LY1、T2A……）而定义它的包没装；pdfLaTeX 文档里常见
        if let Some(name) = between(&line, "Encoding file `", "' not found") {
            push(name.to_string());
        }
        // ! I can't find file `foo'.  （plain TeX 的 \input，以及 mktextfm 里 mf 的回显）
        if let Some(name) = between(&line, "I can't find file `", "'") {
            push(name.to_string());
        }
        // !pdfTeX error: pdflatex (file tcrm1000): Font tcrm1000 at 602 not found
        // pdfTeX 有 TFM、但 map 里没登记字形文件（或登记的 .pfb 不在），又没有 mktexpk 现生成位图
        if let Some(name) = between(&line, "pdfTeX error: ", " not found")
            .and_then(|r| r.split_once("): Font ").map(|(_, f)| f))
            .and_then(|f| f.split_whitespace().next())
        {
            push(pdftex_glyphs(name));
        }
        // ./b.tex:3: Font T1/phv/m/n/10=phvr8t at 10.0pt not loadable: Metric (TFM) file ...
        // ! Font \x=phvr7t not loadable: Metric (TFM) file not found.
        if line.contains("not loadable: Metric (TFM) file") {
            if let Some(rest) = line.split_once("Font ").map(|(_, r)| r) {
                if let Some((_, after_eq)) = rest.split_once('=') {
                    // XeTeX 的 `\font\x=[foo.otf]`：方括号里是字体文件名
                    if let Some(file) = after_eq.strip_prefix('[').and_then(|r| r.split(']').next()) {
                        push(file.rsplit('/').next().unwrap_or(file).to_string());
                        continue;
                    }
                    // `\font\x="Name:feat"` 是按名字找系统字体，归 missing_fonts
                    if after_eq.starts_with('"') {
                        continue;
                    }
                    let name = after_eq
                        .split(|c: char| c.is_whitespace())
                        .next()
                        .unwrap_or("");
                    push(format!("{name}.tfm"));
                }
            }
        }
        // LuaTeX（luaotfload）按文件名找字体没找到；luatexja 的默认日文字体（原ノ味）没装时就是这样：
        // lltjdefs.sty:69: Font \JY3/mc/m/n/10=file:HaranoAjiMincho-Regular.otf:-kern;jfm=jlreq at 9.6pt not loadable: metric data not found or bad.
        // `name:` 按名字找系统字体，`[路径]` 是项目里的文件，都不归这里
        if line.contains("not loadable: metric data not found") {
            let file = line
                .split_once("Font ")
                .and_then(|(_, r)| r.split_once('='))
                .and_then(|(_, r)| r.trim_start_matches('"').strip_prefix("file:"))
                .and_then(|r| r.split([':', ' ', '"']).next())
                .filter(|f| f.contains('.'));
            if let Some(f) = file {
                push(f.to_string());
            }
        }
    }
    // mktextfm 回显的 `phvr8t` 与 TFM 报错的 `phvr8t.tfm` 是同一个文件，留带扩展名的
    let with_ext: Vec<String> = out
        .iter()
        .filter_map(|n| n.rsplit_once('.').map(|(stem, _)| stem.to_string()))
        .collect();
    out.retain(|n| n.contains('.') || !with_ext.contains(n));
    out
}

/// 参考文献样式缺失：bibtex 找不到 `.bst`，biblatex 找不到 `.bbx` / `.cbx`。按出现顺序去重。
///
/// bibtex 失败不让编译失败（xelatex 照样出 PDF，引用全是 [?]），所以不论编译成败都要看；
/// 它的报错在编译输出（`log`）里：`I couldn't open style file unsrturl.bst`。
/// pdfTeX 缺字形时要装的那个文件。EC / TS1 编码的 Computer Modern（`ecrm1000`、`tcrm1000`……）
/// 只有 TFM，字形在 cm-super 里、文件名还不一样（`sfrm1000.pfb`），按它的 map 找包；
/// 其余字体的 .pfb 一般与 TFM 同名
fn pdftex_glyphs(font: &str) -> String {
    let ec = |p: &str| {
        font.strip_prefix(p).is_some_and(|r| {
            r.len() >= 4 && r.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
    };
    if ec("ec") {
        "cm-super-t1.map".into()
    } else if ec("tc") {
        "cm-super-ts1.map".into()
    } else {
        format!("{font}.pfb")
    }
}

/// 用户层装了带 Type1 字体映射的包（times、cm-super、xcharter……）后登记进 `updmap-user`，
/// pdfTeX 和 xdvipdfmx 才找得到字形。登记哪些照用户层 tlpdb 里各包的 `execute addMap` /
/// `addMixedMap`（与 tlmgr 装进主树时跑的一样），只登记还没登记过的，一个都没有就不跑（updmap 要几秒）。
/// 只管内置 TeX 的用户层：用户自己的 TeX Live 装包时 tlmgr 会自己跑 updmap-sys。
/// 顺带把 pTeX 的日文字体从不嵌入切回嵌入原ノ味（装上了的话）
pub fn register_user_maps() -> Result<usize, String> {
    let home = crate::tex::var("TEXMFHOME").map(PathBuf::from).ok_or("没有设置 TEXMFHOME")?;
    let tlpdb = fs::read_to_string(home.join("tlpkg").join("texlive.tlpdb")).unwrap_or_default();
    // updmap-user 把登记写在 TEXMFHOME/web2c/updmap.cfg（实测，不在 TEXMFCONFIG）；两处都看，免得每次启动重跑
    let cfg: String = [Some(home.clone()), crate::tex::var("TEXMFCONFIG").map(PathBuf::from)]
        .into_iter()
        .flatten()
        .filter_map(|d| fs::read_to_string(d.join("web2c").join("updmap.cfg")).ok())
        .collect::<Vec<_>>()
        .join("\n");
    let fresh = unregistered_maps(&tlpdb, &cfg);
    let deadline = Instant::now() + Duration::from_secs(120);
    // 装 pTeX 引擎时还没有原ノ味，日文字体被设成了不嵌入（engines.rs 的 kanji_maps）；后来装上了就切回嵌入
    let embed = cfg.lines().any(|l| l.split_whitespace().eq(["jaEmbed", "noEmbed"]))
        && engine::which("kpsewhich").is_some_and(|k| {
            engine::run(crate::engine::command(k).arg("HaranoAjiMincho-Regular.otf"), deadline).is_ok_and(|r| r.ok)
        });
    if fresh.is_empty() && !embed {
        return Ok(0);
    }
    let updmap = engine::which("updmap-user").ok_or("找不到 updmap-user")?;
    if embed {
        let ran = engine::run(crate::engine::command(&updmap).args(["--nomkmap", "--setoption", "jaEmbed", "haranoaji"]), deadline)
            .map_err(|e| format!("无法运行 updmap-user：{e}"))?;
        if !ran.ok {
            return Err(format!("日文字体改回嵌入原ノ味失败：{}", tail(&ran.log, 10)));
        }
    }
    for (kind, map) in &fresh {
        let ran = engine::run(crate::engine::command(&updmap).args(["--nomkmap", "--enable"]).arg(format!("{kind}={map}")), deadline)
            .map_err(|e| format!("无法运行 updmap-user：{e}"))?;
        if !ran.ok {
            return Err(format!("登记 {map} 失败：{}", tail(&ran.log, 10)));
        }
    }
    let ran = engine::run(&mut crate::engine::command(&updmap), deadline).map_err(|e| format!("无法运行 updmap-user：{e}"))?;
    if !ran.ok {
        return Err(format!("生成字体映射失败：{}", tail(&ran.log, 10)));
    }
    Ok(fresh.len())
}

/// tlpdb 里要登记、`updmap.cfg` 里还没有的映射：`(Map 或 MixedMap, 文件名)`
fn unregistered_maps(tlpdb: &str, cfg: &str) -> Vec<(&'static str, String)> {
    let known: Vec<&str> = cfg.lines().filter_map(|l| l.split_whitespace().nth(1)).collect();
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for l in tlpdb.lines() {
        let Some(rest) = l.strip_prefix("execute add") else { continue };
        let (kind, map) = match rest.split_once(' ') {
            Some(("Map", m)) => ("Map", m.trim()),
            Some(("MixedMap", m)) => ("MixedMap", m.trim()),
            _ => continue,
        };
        if !known.contains(&map) && !out.iter().any(|(_, m)| m == map) {
            out.push((kind, map.to_string()));
        }
    }
    out
}

/// biblatex 缺样式会让编译失败，但报错只说 `Style 'ieee' not found`，分不清是 `.bbx` 还是 `.cbx`，
/// 带扩展名的那句是 Info，只写进 TeX 自己的 `.log`（`texlog`）：
/// `Package biblatex Info: ... file 'ieee.bbx' not found.`
pub fn missing_bib_styles(log: &str, texlog: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: String| {
        if valid_name(&name) && !out.contains(&name) {
            out.push(name);
        }
    };
    for line in unwrap_lines(log) {
        if let Some(name) = line.trim_end().strip_prefix("I couldn't open style file ") {
            let name = name.trim();
            push(if name.ends_with(".bst") { name.to_string() } else { format!("{name}.bst") });
        }
    }
    for line in unwrap_lines(texlog) {
        if line.starts_with("Package biblatex Info: ") {
            if let Some(name) = between(&line, "... file '", "' not found") {
                if name.ends_with(".bbx") || name.ends_with(".cbx") {
                    push(name.to_string());
                }
            }
        }
    }
    out
}

/// 从日志里认出的缺失系统字体（按名字找的），按出现顺序去重。
///
/// fontspec / xeCJK 的报错是 expl3 消息：按词折行，续行带 `(fontspec)` 前缀，
/// 长字体名还会再被 79 列硬折一次：
/// ```text
/// ./main.tex:4: Package fontspec Error:
/// (fontspec)                The font "FZShuSong-Z01" cannot be found;
/// ```
/// 所以先接 79 列折行，再去掉续行前缀、按空格拼成一段，最后找引号里的名字。
/// XeTeX 原语 `\font\x="Name:feat"` 找不到时报 `installed font not found`，也算。
pub fn missing_fonts(log: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: &str| {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if valid_font(&name) && !out.contains(&name) {
            out.push(name);
        }
    };
    let lines = unwrap_lines(log);
    let text = lines
        .iter()
        .map(|l| strip_msg_prefix(l))
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut rest = text.as_str();
    while let Some(i) = rest.find("The font \"") {
        rest = &rest[i + "The font \"".len()..];
        if let Some(j) = rest.find("\" cannot be found") {
            push(&rest[..j]);
            rest = &rest[j..];
        }
    }
    for line in &lines {
        // ./main.tex:1: Font \x="Nonexistent Sans:mapping=tex-text" at 10.0pt not loadable: Metric (TFM) file or installed font not found.
        if line.contains("not loadable:") && line.contains("installed font not found") {
            if let Some(spec) = between(line, "=\"", "\"") {
                // `Name/B:feat`：斜杠后是 XeTeX 的样式选项，冒号后是 OpenType 特性
                let name = spec.split([':', '/']).next().unwrap_or("");
                push(name);
            }
        }
    }
    out
}

/// 内置 TeX 的 rungs（scripts/rungs.sh）和 epstopdf（scripts/epstopdf.sh）找不到 gs 时打的话，
/// 后面跟着要转的文件
const RUNGS_NO_GS: &str = "subo-rungs: ghostscript not found: ";

/// epstopdf 宏包给转换结果起的名字：`figs/fig-eps-converted-to.pdf` → 原图 `figs/fig.eps`
fn eps_source(name: &str) -> Option<String> {
    ["eps", "ps"].iter().find_map(|ext| {
        let base = name.strip_suffix(&format!("-{ext}-converted-to.pdf"))?;
        (!base.is_empty()).then(|| format!("{base}.{ext}"))
    })
}

/// 认出「缺 Ghostscript」：EPS / PS 插图要它转成 PDF。
///
/// 内置 TeX 走 scripts/rungs.sh（xelatex → xdvipdfmx）或 scripts/epstopdf.sh（pdflatex → epstopdf 宏包
/// → repstopdf），找不到 gs 会打 [`RUNGS_NO_GS`] 一行，带文件名，最准。
/// 用户自己的 TeX Live 没有标记行，只能兜底，且本机确实找不到 gs 才算：
/// - xelatex：rungs.lua 缺 gs 时 xdvipdfmx 只在 `-q` 下留一句 `pdf_link_obj(): passed invalid object`，
///   还要 TeX 日志（`texlog`，即 `main.log`）里载入过 eps / ps 图；
/// - pdflatex：epstopdf 没转出来，pdftex.def 报 ``File `fig-eps-converted-to.pdf' not found``，图名从这里反推。
pub fn missing_tools(log: &str, texlog: &str, gs_found: bool) -> Vec<Missing> {
    let mut files: Vec<String> = Vec::new();
    let mut push = |f: &str| {
        let f = f.trim().trim_matches(|c| c == '\'' || c == '"');
        let f = f.strip_prefix("./").unwrap_or(f);
        if !f.is_empty() && f.chars().count() <= 256 && !f.chars().any(char::is_control) && !files.iter().any(|x| x == f) {
            files.push(f.to_string());
        }
    };
    let mut marked = false;
    for line in log.lines() {
        if let Some(i) = line.find(RUNGS_NO_GS) {
            marked = true;
            push(&line[i + RUNGS_NO_GS.len()..]);
        }
    }
    if !marked {
        if gs_found {
            return Vec::new();
        }
        for line in unwrap_lines(log) {
            if let Some(src) = between(&line, "File `", "' not found").and_then(eps_source) {
                marked = true;
                push(&src);
            }
        }
    }
    if !marked {
        if !log.contains("pdf_link_obj(): passed invalid object") {
            return Vec::new();
        }
        // File: fig.eps Graphic file (type eps)
        for line in texlog.lines() {
            if let Some(name) = line.strip_prefix("File: ") {
                for ty in [" Graphic file (type eps)", " Graphic file (type ps)"] {
                    if let Some(i) = name.find(ty) {
                        push(&name[..i]);
                    }
                }
            }
        }
        if files.is_empty() {
            return Vec::new();
        }
    }
    vec![Missing {
        kind: Kind::Tool,
        file: "ghostscript".into(),
        package: None,
        lookup: Lookup::Absent,
        needed_by: files,
        font_file: None,
        project_path: None,
    }]
}

/// 本机有没有 gs：PATH 里，或 Homebrew / MacPorts 的常见位置（从 Finder 启动时 PATH 里没有它们）。
/// 与 scripts/rungs.sh 找的位置一致。
pub fn ghostscript_found() -> bool {
    engine::which("gs").is_some()
        || ["/opt/homebrew/bin/gs", "/usr/local/bin/gs", "/opt/local/bin/gs"]
            .iter()
            .any(|p| Path::new(p).is_file())
}

/// `(fontspec)                The font …` → `The font …`；不是续行就原样返回。
fn strip_msg_prefix(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix('(') {
        if let Some((tag, after)) = rest.split_once(')') {
            let is_tag = !tag.is_empty()
                && tag.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
            if is_tag && (after.is_empty() || after.starts_with(' ')) {
                return after;
            }
        }
    }
    line
}

/// 字体名可以有空格、中文，但不能有引号、反斜杠、控制字符：要原样展示给人看。
fn valid_font(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 128
        && !name.chars().any(|c| c.is_control() || matches!(c, '"' | '\\' | '`'))
}

/// 把被 79 列折断的行接回去。
fn unwrap_lines(log: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for l in log.lines() {
        cur.push_str(l);
        if l.chars().count() != MAX_PRINT_LINE {
            lines.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let j = s[i..].find(end)?;
    Some(&s[i..i + j])
}

/// 只接受朴素文件名：既要拼进 tlmgr 的正则，也要当缓存键。
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
        && !name.starts_with('.')
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lookup {
    /// 还没查（第一条事件先把文件名推出去）
    Pending,
    /// TeX Live 里有，`package` 就是包名
    Found,
    /// TeX Live 里没有：多半是项目自己的文件，或者拼错了
    Absent,
    /// 查不了：tlmgr 不在、超时、离线且缓存里没有
    Unknown,
    /// 项目里就有，只是不在 TeX 的搜索路径上（放在子目录里），`project_path` 是它的位置。
    /// 不查 tlmgr
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// `.sty`、`\input` 的文件、TFM：按文件名问 tlmgr
    File,
    /// 按名字找的字体：`file` 字段放字体名，不查 tlmgr。TeX Live 里有同名字体文件时
    /// `lookup` 为 Found、`font_file` 放文件名，否则为 Absent
    Font,
    /// TeX Live 之外的程序：`file` 放程序代号（目前只有 `ghostscript`），
    /// `needed_by` 列出用到它的文件，`lookup` 恒为 Absent
    Tool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Missing {
    pub kind: Kind,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    pub lookup: Lookup,
    #[serde(rename = "neededBy", skip_serializing_if = "Vec::is_empty")]
    pub needed_by: Vec<String>,
    /// 只用于 Font：TeX Live 里同名的字体文件（如 `FontAwesome.otf`），这时 `lookup` 为 Found
    #[serde(rename = "fontFile", skip_serializing_if = "Option::is_none")]
    pub font_file: Option<String>,
    /// 只用于 `Lookup::Project`：项目里同名文件的相对路径（如 `style/foo.cls`）
    #[serde(rename = "projectPath", skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

/// 文件名 → 包名的磁盘缓存。`None` 表示 tlmgr 明确答复「没有」。
pub struct Index {
    path: PathBuf,
    map: BTreeMap<String, Option<String>>,
}

impl Index {
    pub fn open(path: PathBuf) -> Self {
        let map = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Index { path, map }
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        // 先写临时文件再改名：半写入的缓存比没缓存更糟
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string_pretty(&self.map) {
            if fs::write(&tmp, text).is_ok() {
                let _ = fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// 缓存默认放 `~/.subo/case/file-index-v1.json`（见架构文档 §5），`SUBO_HOME` 可改。
/// 判定规则变了就升版本号，旧缓存里的错答案自然作废。
pub fn default_index_path() -> PathBuf {
    let home = std::env::var_os("SUBO_HOME")
        .map(PathBuf::from)
        .or_else(|| crate::library::home_dir().map(|h| h.join(".subo")))
        .unwrap_or_else(|| PathBuf::from(".subo"));
    home.join("case").join("file-index-v1.json")
}

/// 先查缓存，再查随包带的完整版文件索引（[`FILE_INDEX`]），都没有才问 tlmgr。
/// 一次查询失败（通常是离线）后，剩下的文件不再挨个等超时，直接标 Unknown。
/// `repo`：设置里选的安装源，`None` 用 tlmgr 自己配的。
/// `fallback`：软件源跨到新年度版本时要不要自动改用归档源，见 [`Fallback`]。
pub fn resolve(index: &mut Index, files: &[String], repo: Option<&str>, fallback: Fallback) -> Vec<Missing> {
    let mut tlmgr_ok = engine::which("tlmgr").is_some();
    let mut dirty = false;
    let uncached: Vec<String> = files.iter().filter(|f| !index.map.contains_key(*f)).cloned().collect();
    let offline = file_index().and_then(|p| index_lookup(&p, &uncached));
    let out = files
        .iter()
        .map(|file| {
            if let Some(hit) = index.map.get(file) {
                return found_or_absent(file, hit.clone());
            }
            // 索引里有就是定论；没有不一定真没有（索引是打包时的快照，软件源后来可能加了），
            // 能联网就再问 tlmgr，问不成才按索引判没有，也不写进缓存
            let in_index = offline.as_ref().and_then(|f| f.get(file)).map(|(pkg, _)| pkg.clone());
            if in_index.is_some() {
                index.map.insert(file.clone(), in_index.clone());
                dirty = true;
                return found_or_absent(file, in_index);
            }
            if tlmgr_ok {
                match tlmgr_lookup(file, repo, fallback) {
                    Some(pkg) => {
                        index.map.insert(file.clone(), pkg.clone());
                        dirty = true;
                        return found_or_absent(file, pkg);
                    }
                    None => tlmgr_ok = false,
                }
            }
            if offline.is_some() {
                return found_or_absent(file, None);
            }
            Missing { kind: Kind::File, file: file.clone(), package: None, lookup: Lookup::Unknown, needed_by: Vec::new(), font_file: None, project_path: None }
        })
        .collect();
    if dirty {
        index.save();
    }
    out
}

/// 完整版 TeX Live 的「文件名 → 包名」索引，相对 TeX Live 根目录。裁剪内置 TeX Live 时由
/// `scripts/texlive-index.sh` 从软件源的 tlpdb 生成，每行 `包名<TAB>文件名 文件名 …`，
/// 只收 `tex/`、`fonts/`、`bibtex/`、`makeindex/` 下的运行文件。
///
/// 有了它，缺的文件属于哪个包离线也答得上，不必挨个联网问 tlmgr（一次约 2 s）；
/// [`loose_files`] 认出的大批候选也才查得起。用户自己的 TeX Live 没有这个文件，退回 tlmgr。
pub const FILE_INDEX: &str = "tlpkg/subo-file-index.txt";

fn file_index() -> Option<PathBuf> {
    let p = texlive_root()?.join(FILE_INDEX);
    p.is_file().then_some(p)
}

/// 在索引里查一批文件名：不分大小写，不带扩展名的也认 `.tex`。返回找到的
/// `文件名 → (包名, 索引里的写法)`。同名文件有好几个包时生成索引时已按 xelatex 的搜索顺序挑定，
/// 这里只剩大小写不同的重名，取先出现的。索引读不了返回 `None`。
/// 4 MB 的文本每次从头扫一遍（几毫秒），只在编译失败时用，不值得常驻内存。
fn index_lookup(path: &Path, files: &[String]) -> Option<BTreeMap<String, (String, String)>> {
    let text = fs::read_to_string(path).ok()?;
    let mut want: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for f in files {
        let key = f.to_ascii_lowercase();
        if !f.contains('.') {
            want.entry(format!("{key}.tex")).or_default().push(f);
        }
        want.entry(key).or_default().push(f);
    }
    let mut out: BTreeMap<String, (String, String)> = BTreeMap::new();
    for line in text.lines() {
        let Some((pkg, names)) = line.split_once('\t') else { continue };
        for name in names.split(' ') {
            if let Some(origs) = want.get(&name.to_ascii_lowercase()) {
                for f in origs {
                    // 先到先得，只有精确同名能顶掉补上 .tex 才对上的
                    let replace = match out.get(*f) {
                        None => true,
                        Some((_, had)) => name.eq_ignore_ascii_case(f) && !had.eq_ignore_ascii_case(f),
                    };
                    if replace {
                        out.insert((*f).clone(), (pkg.to_string(), name.to_string()));
                    }
                }
            }
        }
    }
    Some(out)
}

/// xdvipdfmx 报缺文件的句型：`(前缀, 结尾)`，结尾为空表示到行尾。
/// 字体文件缺了它多半只警告、PDF 照出（字缺了），所以编译成功时也要看
const DRIVER_MISSING: &[(&str, &str)] = &[
    // 映射表指向的 Type1 / OpenType 字体文件（mathdesign 的 mdbchr7m.pfb 之类）
    ("kpathsea library can find this font: ", ""),
    ("Could not open config file \"", "\""),
    ("Reading CMap file \"", "\" failed"),
    ("Could not open TFM/OFM file \"", "\""),
    ("Unable to find TFM file \"", "\""),
    // OpenType/TrueType font file "x" 与 OpenType/TrueType/dfont font file "x"
    ("font file \"", "\""),
    ("Could not open OpenType font: ", ""),
    ("Could not open TrueType font: ", ""),
    ("Could not open Type1 font: ", ""),
];

/// 宽进：日志里提到没找到、但不一定是它导致失败的文件。NFSS 缺字体定义文件时只在日志里记一句
/// `No file MDAcmr.fd.`，随后用替代字体或报一个看不出原因的错；biblatex 的
/// `file 'x.lbx' not found` 只是 Info；xdvipdfmx 缺字体文件只警告（[`DRIVER_MISSING`]）。
/// 这些多数是无害的回退（`No file main.aux.`），交给 [`confirm`] 严出。
/// `failed`：编译没成功。成功时 NFSS 的回退不算数，只看 xdvipdfmx 的。
/// `strict` 是 [`missing_files`] 已经认定的，不重复。
pub fn loose_files(log: &str, strict: &[String], failed: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in unwrap_lines(log) {
        let l = line.trim();
        let tex = || l.strip_prefix("No file ").and_then(|r| r.strip_suffix('.')).or_else(|| between(l, "file '", "' not found"));
        let driver = || {
            DRIVER_MISSING.iter().find_map(|(start, end)| {
                let i = l.find(start)? + start.len();
                let rest = &l[i..];
                let name = if end.is_empty() { rest.trim_end_matches('.') } else { &rest[..rest.find(end)?] };
                // 可能带目录；TFM 可能不带扩展名
                let base = name.rsplit('/').next().unwrap_or(name).trim();
                Some(if start.contains("TFM") && !base.contains('.') { format!("{base}.tfm") } else { base.to_string() })
            })
        };
        let name = driver().or_else(|| if failed { tex().map(str::to_string) } else { None });
        if let Some(n) = name.as_deref() {
            let dup = |v: &Vec<String>| v.iter().any(|s| s.eq_ignore_ascii_case(n));
            if valid_name(n) && !dup(&out) && !strict.iter().any(|s| s.eq_ignore_ascii_case(n)) {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// 严出：宽进的候选里只留完整版 TeX Live 有、本机没装的，文件名换成 TeX Live 里的写法。
/// 只查本地索引，不问 tlmgr——候选里大半是无害的，挨个联网太慢；没有索引就一个不报。
pub fn confirm(index: &mut Index, files: &[String]) -> Vec<Missing> {
    if files.is_empty() {
        return Vec::new();
    }
    let Some(found) = file_index().and_then(|p| index_lookup(&p, files)) else { return Vec::new() };
    let have = kpse_find(&found.values().map(|(_, n)| n.clone()).collect::<Vec<_>>());
    let mut out = Vec::new();
    for (pkg, name) in found.into_values() {
        if have.iter().any(|h| h.eq_ignore_ascii_case(&name)) || out.iter().any(|m: &Missing| m.file == name) {
            continue;
        }
        index.map.insert(name.clone(), Some(pkg.clone()));
        out.push(found_or_absent(&name, Some(pkg)));
    }
    if !out.is_empty() {
        index.save();
    }
    out
}

impl Index {
    /// 只装查出来过的包：安装接口不接受任意包名
    pub fn knows_package(&self, pkg: &str) -> bool {
        self.map.values().any(|v| v.as_deref() == Some(pkg))
    }

    /// 查出来属于这些包的文件名：装完顺着它们补依赖
    pub fn files_of(&self, pkgs: &[String]) -> Vec<String> {
        self.map
            .iter()
            .filter(|(_, v)| v.as_ref().is_some_and(|p| pkgs.contains(p)))
            .map(|(f, _)| f.clone())
            .collect()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub ok: bool,
    /// 用户在系统授权框里点了取消
    pub cancelled: bool,
    /// 走了管理员授权
    pub admin: bool,
    /// 失败看着是连不上软件源（离线、DNS、超时）：包记进待取队列，联网后补
    pub offline: bool,
    /// tlmgr 输出的末尾几十行，失败时给人看
    pub log: String,
    /// 主软件源已跨到新年度版本，这次改用了这个归档源
    pub archive: Option<String>,
    /// 软件源比本机 TeX Live 新、又没能回退时，软件源的年份：界面据此说明原因
    pub repo_year: Option<u32>,
    /// 装完顺着新文件的 `\RequirePackage` 补装的宏包
    pub also: Vec<String>,
    /// 本机 tlmgr 比软件源旧，装之前先替用户更新了它（`tlmgr update --self`）
    pub tlmgr_updated: bool,
}

/// 补装依赖最多追几层：正常一两层就齐，防的是索引和实际对不上时来回装
const FOLLOW_ROUNDS: usize = 3;

/// 安装宏包，装完再读要用的那几个新文件，看它们还要什么本机没有的，一并补装，
/// 免得重编一次又缺下一个。只顺着 `files`（缺字清单里的文件名）往下读，不读包里的
/// 其他文件：文档类、示例里的依赖用不上。走管理员授权的不补：每补一层都要再输一次密码。
/// 其余参数同 [`install_once`]。
pub fn install(
    pkgs: &[String],
    files: &[String],
    usermode: bool,
    repo: Option<&str>,
    fallback: Fallback,
) -> Result<Installed, String> {
    let mut done = install_once(pkgs, usermode, repo, fallback, false)?;
    let mut tried: Vec<String> = pkgs.to_vec();
    let mut last: Vec<String> = files.to_vec();
    for _ in 0..FOLLOW_ROUNDS {
        if !done.ok || done.admin {
            break;
        }
        let (next, next_files) = followups(&last, &tried);
        if next.is_empty() {
            break;
        }
        let repo = done.archive.as_deref().or(repo);
        track(|p| *p = Progress { followup: true, ..Progress::default() });
        match install_once(&next, usermode, repo, fallback, false) {
            // 补装失败、被取消都不算这次失败：用户要的已经装上了，缺的重编时照样会报出来
            Ok(more) => {
                done.log.push_str(&format!("\n{}", more.log));
                if !more.ok {
                    break;
                }
            }
            Err(e) => {
                done.log.push_str(&format!("\n{e}"));
                break;
            }
        }
        tried.extend(next.iter().cloned());
        done.also.extend(next);
        last = next_files;
    }
    Ok(done)
}

/// `files` 里声明要用、本机还没有的文件，按索引换成包名；`skip` 是已经装过的包，不再算。
/// 读到的文件属于刚装的包、本机已经有了（包自己拆出去的模块），接着往里读。
/// 返回 `(包名, 对应的文件名)`，文件名留给下一层接着读。没有索引就不补。
fn followups(files: &[String], skip: &[String]) -> (Vec<String>, Vec<String>) {
    let (mut pkgs, mut names): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let Some(index) = file_index() else { return (pkgs, names) };
    let mut read: Vec<String> = files.to_vec();
    let mut frontier: Vec<String> = files.to_vec();
    while !frontier.is_empty() && read.len() <= 100 {
        let wants = crate::prescan::requires_of(&kpse_paths(&frontier));
        let Some(found) = index_lookup(&index, &wants) else { break };
        let have = kpse_find(&found.values().map(|(_, n)| n.clone()).collect::<Vec<_>>());
        frontier.clear();
        for (pkg, name) in found.into_values() {
            let installed = have.iter().any(|h| h.eq_ignore_ascii_case(&name));
            if skip.contains(&pkg) {
                if installed && !read.contains(&name) {
                    read.push(name.clone());
                    frontier.push(name);
                }
            } else if !installed && valid_package(&pkg) {
                if !pkgs.contains(&pkg) {
                    pkgs.push(pkg);
                }
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    (pkgs, names)
}

/// 依次跑几条 tlmgr 子命令（如 `update --self`、`install …`），前一条失败就不往下跑。
/// `usermode` 或 TeX Live 目录可写（`direct`）就直接起进程，日志接在一起；
/// 否则在 macOS 上弹系统的管理员授权框，几条命令用 `&&` 连成一次授权。返回运行结果和走没走授权
fn run_tlmgr(
    tlmgr: &Path,
    steps: &[Vec<String>],
    usermode: bool,
    direct: bool,
    repo: Option<&str>,
    fallback: Fallback,
    deadline: Instant,
) -> Result<(std::io::Result<Remote>, bool), String> {
    if usermode || direct {
        let mut log = String::new();
        let mut last = None;
        for args in steps {
            let build = |r: Option<&str>| {
                let mut cmd = crate::engine::command(tlmgr);
                if usermode {
                    cmd.arg("--usermode");
                }
                // tlmgr 写管道时整块缓冲，逐包的进度行要等装完才出来；让 Perl 每行都刷
                cmd.env("PERL5OPT", perl_autoflush());
                if let Some(tmp) = progress_tmp() {
                    cmd.env("TMPDIR", &tmp).env("TMP", &tmp).env("TEMP", &tmp);
                }
                with_repo(&mut cmd, r).args(args);
                cmd
            };
            track(|p| p.cancellable = true);
            let mut done = match run_remote(build, repo, fallback, deadline, Some(&track_tlmgr)) {
                Ok(d) => d,
                Err(e) => return Ok((Err(e), false)),
            };
            if !log.is_empty() {
                done.ran.log = format!("{log}\n{}", done.ran.log);
            }
            log = done.ran.log.clone();
            let stop = !done.ran.ok;
            last = Some(done);
            if stop {
                break;
            }
        }
        let done = last.ok_or("没有要跑的 tlmgr 命令")?;
        Ok((Ok(done), false))
    } else if cfg!(target_os = "macos") {
        let path = tlmgr.to_string_lossy();
        if path.contains('\'') {
            return Err("tlmgr 路径里有引号，无法代为授权".into());
        }
        // 包名已校验过只含安全字符，路径、仓库地址用单引号包住（地址里的引号设置时就拒了）。
        // 回退到归档源时会再弹一次授权框，之后这个进程里直接用归档源
        let build = |r: Option<&str>| {
            let repo_arg = match r {
                Some(r) if !r.contains('\'') => format!(" --repository '{r}'"),
                _ => String::new(),
            };
            let sh = steps
                .iter()
                .map(|args| format!("'{path}'{repo_arg} {}", args.join(" ")))
                .collect::<Vec<_>>()
                .join(" && ");
            let script = format!(
                "do shell script \"{}\" with administrator privileges",
                sh.replace('\\', "\\\\").replace('"', "\\\"")
            );
            let mut cmd = crate::engine::command("osascript");
            cmd.args(["-e", &script]);
            cmd
        };
        Ok((run_remote(build, repo, fallback, deadline, None), true))
    } else {
        let line = steps.iter().map(|a| format!("tlmgr {}", a.join(" "))).collect::<Vec<_>>().join(" && ");
        let how = if cfg!(windows) { "以管理员身份打开命令提示符运行：" } else { "在终端运行：sudo " };
        Err(format!("TeX Live 目录需要管理员权限，请{how}{line}"))
    }
}

/// 本机 tlmgr 比软件源上的旧：两边 `texlive.infra` 的修订号
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TlmgrStale {
    pub local: u64,
    pub remote: u64,
}

/// 问软件源本机 tlmgr 旧不旧（`tlmgr update --self --list`）。只读，不要管理员权限；
/// 要下载软件源的包数据库，几秒钟。`Ok(None)` 是不旧；问不到（离线、软件源跨了年度）是 `Err`
pub fn tlmgr_stale(repo: Option<&str>) -> Result<Option<TlmgrStale>, String> {
    let tlmgr = engine::which("tlmgr").ok_or("找不到 tlmgr")?;
    let mut cmd = crate::engine::command(&tlmgr);
    cmd.arg("--machine-readable");
    with_repo(&mut cmd, repo).args(["update", "--self", "--list"]);
    let ran = engine::run(&mut cmd, Instant::now() + STALE_TIMEOUT).map_err(|e| format!("无法运行 tlmgr：{e}"))?;
    if !ran.ok {
        return Err(tail(&ran.log, 20));
    }
    Ok(parse_stale(&ran.log))
}

/// 问 tlmgr 旧不旧最多等这么久：要下载软件源的包数据库
const STALE_TIMEOUT: Duration = Duration::from_secs(60);

/// `--machine-readable` 的更新清单里 tlmgr 自己（`texlive.infra`，或带平台后缀的）那一行：
/// `texlive.infra\tu\t<本机>\t<软件源>\t…`
fn parse_stale(log: &str) -> Option<TlmgrStale> {
    log.lines().find_map(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 4 || f[1] != "u" || !f[0].starts_with("texlive.infra") {
            return None;
        }
        Some(TlmgrStale { local: f[2].parse().ok()?, remote: f[3].parse().ok()? })
    })
}

#[test]
fn stale_parsed() {
    let log = "location-url\thttps://x/tlnet\ntotal-bytes\t476936\nend-of-header\n\
        texlive.infra\tu\t80683\t80721\t476936\t-\t-\t-\t-\t-\nend-of-updates";
    assert_eq!(parse_stale(log), Some(TlmgrStale { local: 80683, remote: 80721 }));
    assert_eq!(parse_stale("end-of-header\nend-of-updates"), None);
}

/// 设置页的「更新 tlmgr」：`tlmgr update --self`。目录不可写的弹授权框；内置 TeX 用不着
pub fn update_tlmgr(repo: Option<&str>, fallback: Fallback) -> Result<Installed, String> {
    let tlmgr = engine::which("tlmgr").ok_or("找不到 tlmgr：本机 TeX 不是 TeX Live？")?;
    let direct = tl_root(&tlmgr).map(|r| writable(&r.join("tlpkg"))).unwrap_or(false);
    let deadline = Instant::now() + INSTALL_TIMEOUT;
    let (ran, admin) = run_tlmgr(&tlmgr, &[vec!["update".into(), "--self".into()]], false, direct, repo, fallback, deadline)?;
    let Remote { ran, archive, repo_year } = ran.map_err(|e| format!("无法运行 tlmgr：{e}"))?;
    let cancelled = admin && !ran.ok && ran.log.contains("(-128)");
    let offline = !ran.ok && !cancelled && repo_year.is_none() && (ran.timed_out || network_failure(&ran.log));
    Ok(Installed {
        ok: ran.ok,
        cancelled,
        admin,
        offline,
        log: tail(&ran.log, 40),
        archive,
        repo_year,
        also: Vec::new(),
        // 已是最新时 tlmgr 也报成功，只打一行「no self-updates」
        tlmgr_updated: ran.ok && !ran.log.contains("no self-updates for tlmgr available"),
    })
}

/// 安装宏包。TeX Live 目录可写就直接装；不可写（macOS 上 MacTeX 默认归 root）
/// 就弹系统的管理员授权框，密码由系统收，不经过我们。
///
/// `usermode`：TeX Live 随应用内置、包内只读，装进用户层（`TEXMFHOME`，桌面壳设在 `~/.subo/texlive/<年份>`）。
/// `repo`：设置里选的安装源，`None` 用 tlmgr 自己配的。
/// `fallback`：软件源跨到新年度版本时要不要自动改用归档源，见 [`Fallback`]。
/// `reinstall`：已经装了也重新下载覆盖（设置页的「重装」）。
fn install_once(pkgs: &[String], usermode: bool, repo: Option<&str>, fallback: Fallback, reinstall: bool) -> Result<Installed, String> {
    if pkgs.is_empty() || !pkgs.iter().all(|p| valid_package(p)) {
        return Err("宏包名不合法".into());
    }
    let tlmgr = engine::which("tlmgr").ok_or("找不到 tlmgr：本机 TeX 不是 TeX Live？")?;
    let deadline = Instant::now() + INSTALL_TIMEOUT;
    track_wanted(pkgs);
    let direct = tl_root(&tlmgr).map(|r| writable(&r.join("tlpkg"))).unwrap_or(false);

    if usermode {
        init_usertree(&tlmgr, deadline)?;
    }
    let mut install = vec!["install".to_string()];
    if reinstall {
        install.push("--reinstall".into());
    }
    install.extend(pkgs.iter().cloned());
    let self_update = vec!["update".to_string(), "--self".to_string()];
    let run = |steps: &[Vec<String>]| run_tlmgr(&tlmgr, steps, usermode, direct, repo, fallback, deadline);
    // 本机 tlmgr 比软件源上的旧时，tlmgr 一个包也不装，只叫人先 `tlmgr update --self`。
    // 要授权的先问一下旧不旧（不用密码），旧就把更新和安装放进同一次授权，免得输两次密码；
    // 目录可写的照常装，碰上了再更新重来，不多花这一趟。内置 TeX 包内只读、用户层不查这个，不走这条
    let admin_mode = !usermode && !direct;
    let stale_first = admin_mode && matches!(tlmgr_stale(repo), Ok(Some(_)));
    if stale_first {
        eprintln!("[case] tlmgr 比软件源旧，和安装放进同一次授权：tlmgr update --self");
    }
    let steps = if stale_first { vec![self_update.clone(), install.clone()] } else { vec![install.clone()] };
    let (mut ran, admin) = run(&steps)?;
    let mut tlmgr_updated = stale_first && matches!(&ran, Ok(r) if r.ran.ok);
    if !usermode && !stale_first && matches!(&ran, Ok(r) if !r.ran.ok && !r.ran.stopped && needs_self_update(&r.ran.log)) {
        eprintln!("[case] tlmgr 比软件源旧，先 tlmgr update --self");
        // 要授权的（问旧不旧时没问到）这里只能再弹一次：更新和重装放一起
        let steps = if admin_mode { vec![self_update, install.clone()] } else { vec![self_update] };
        match run(&steps)?.0 {
            Ok(up) if up.ran.ok && admin_mode => {
                tlmgr_updated = true;
                ran = Ok(up);
            }
            Ok(up) if up.ran.ok => {
                tlmgr_updated = true;
                ran = run(&[install])?.0;
            }
            Ok(mut up) => {
                up.ran.log = format!("tlmgr 比软件源上的旧，自动更新 tlmgr 没成功：\n{}", up.ran.log);
                ran = Ok(up);
            }
            Err(e) => ran = Err(e),
        }
    }
    let Remote { mut ran, archive, repo_year } = ran.map_err(|e| format!("无法运行 tlmgr：{e}"))?;
    if ran.stopped {
        // 只在下载完之前停得下来（见 request_cancel），用户层还没动过
        return Ok(Installed {
            ok: false,
            cancelled: true,
            admin,
            offline: false,
            log: format!("{}\n已取消安装", tail(&ran.log, 20)),
            archive,
            repo_year: None,
            also: Vec::new(),
            tlmgr_updated,
        });
    }
    // 不可重定位包另外下载容器，也要从同一个源拿
    let repo = archive.as_deref().or(repo);
    // tlmgr 跳过包时只打一行提示，退出码照样是 0：不当成功，否则前端重编一次还是同样的错
    let unknown = flagged(&ran.log, "unknown package: ", "");
    if !unknown.is_empty() {
        ran.ok = false;
    }
    if usermode && ran.ok {
        // 带二进制的包（luaotfload、minted…）不可重定位，tlmgr 不肯装进用户层。
        // 内置的引擎（xelatex、pdflatex）用不上它们的二进制，把容器里的宏文件解进 TEXMFHOME 就够了
        for pkg in flagged(&ran.log, "package ", " is not relocatable") {
            track(|p| {
                p.phase = "download";
                p.package = Some(pkg.clone());
                p.kb = None;
            });
            match install_runfiles(&tlmgr, &pkg, repo, deadline) {
                Ok(n) => ran.log.push_str(&format!("\n{pkg}：已把 {n} 个宏文件装进用户层")),
                Err(e) => {
                    ran.ok = false;
                    ran.log.push_str(&format!("\n{pkg}：{e}"));
                }
            }
        }
    }
    // osascript 在用户取消授权时报 -128
    let cancelled = admin && !ran.ok && ran.log.contains("(-128)");
    let offline = !ran.ok && !cancelled && repo_year.is_none() && (ran.timed_out || network_failure(&ran.log));
    let log = if ran.timed_out {
        format!("{}\n安装超时，已终止", tail(&ran.log, 40))
    } else {
        tail(&ran.log, 40)
    };
    Ok(Installed { ok: ran.ok, cancelled, admin, offline, log, archive, repo_year, also: Vec::new(), tlmgr_updated })
}

/// 宏包安装的实时进度，界面轮询它，免得用户对着转圈干等。
/// 同一时间只有一次安装（lib.rs 里有锁），放进程级就够了
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// `connect` 连软件源、读包数据库；`download` 下载（新版 tlmgr 先并行预取全部容器）；
    /// `install` 逐个解包安装；`finish` 刷新索引、登记字体
    pub phase: &'static str,
    /// 逐个安装时是第几个（从 1 起）；预取时为 0
    pub index: u32,
    /// 一共几个包（tlmgr 报了才有，含它自己补上的依赖）
    pub total: u32,
    pub package: Option<String>,
    /// 当前这个包的大小（KB）
    pub kb: Option<u64>,
    /// tlmgr 按已安装字节估的已用秒数、预计总秒数；第一个包时还估不出来
    pub spent: Option<u64>,
    pub estimate: Option<u64>,
    /// 用户要的已装好，正顺着新文件的依赖补装
    pub followup: bool,
    /// 从开始安装算起的秒数
    pub elapsed: u64,
    /// 这次已经下载的字节数（量 tlmgr 临时目录里的容器文件）；还没开始下为 null
    pub got: Option<u64>,
    /// 这次一共要下载的字节数（软件源包数据库里的容器大小）；查不到为 null
    pub want: Option<u64>,
    /// 从下载第一个字节算起过了几秒：界面据此算速度、剩余时间
    pub downloading: u64,
    /// 这次安装停得下来：tlmgr 是我们直接起的（走系统授权框的停不了）。解包开始后照样不能停，见 [`request_cancel`]
    pub cancellable: bool,
}

impl Default for Progress {
    fn default() -> Self {
        Progress {
            phase: "connect",
            index: 0,
            total: 0,
            package: None,
            kb: None,
            spent: None,
            estimate: None,
            followup: false,
            elapsed: 0,
            got: None,
            want: None,
            downloading: 0,
            cancellable: false,
        }
    }
}

struct Tracking {
    start: Instant,
    /// 最近一次 tlmgr 报进度的时刻
    seen: Instant,
    /// 给 tlmgr 的专属 `TMPDIR`：它把容器边下边写在这里，量文件大小就知道下了多少
    tmp: PathBuf,
    /// 要装的包：请求的，加上在临时目录里看到正在下的（依赖）
    wanted: Vec<String>,
    /// 每个容器文件见过的最大字节数：装完的容器会被删掉，按最大值累计才不会倒退
    files: BTreeMap<String, u64>,
    /// 软件源包数据库里的容器大小（字节），读到一次就留着
    sizes: Option<BTreeMap<String, u64>>,
    /// 第一次量到下载字节的时刻
    first_byte: Option<Instant>,
    p: Progress,
}

static PROGRESS: Mutex<Option<Tracking>> = Mutex::new(None);

/// 用户点了取消。只在下载完之前生效：tlmgr 开始解包、改用户层数据库之后再杀，
/// 会留下装了一半的包——半写入的字盘比没装更糟（docs/architecture.md §4.2「取字可取消、不污染字盘」）
static CANCEL: AtomicBool = AtomicBool::new(false);

/// 还在连软件源、下载：这时杀掉 tlmgr 不会动到用户层
fn cancellable(p: &Progress) -> bool {
    p.cancellable && matches!(p.phase, "connect" | "download")
}

/// 请求取消正在进行的安装。解包开始后不再接受，返回错误码
pub fn request_cancel() -> Result<(), &'static str> {
    let g = PROGRESS.lock().unwrap();
    let t = g.as_ref().ok_or("case.notInstalling")?;
    if !cancellable(&t.p) {
        return Err("case.cancelTooLate");
    }
    CANCEL.store(true, Ordering::SeqCst);
    Ok(())
}

/// tlmgr 运行中每 50 ms 问一次：要了取消、且还没开始解包
fn cancel_requested() -> bool {
    CANCEL.load(Ordering::SeqCst) && PROGRESS.lock().unwrap().as_ref().is_some_and(|t| cancellable(&t.p))
}

/// tlmgr 失败的输出看着像连不上软件源。tlmgr 自己没有统一的报错，按常见几种说法认
/// tlmgr 比软件源上的旧时拒绝干活，只打这句（`tlmgr itself needs to be updated`）
fn needs_self_update(log: &str) -> bool {
    log.contains("tlmgr itself needs to be updated")
}

#[test]
fn self_update_recognized() {
    let log = "tlmgr: package repository https://mirrors.tuna.tsinghua.edu.cn/CTAN/systems/texlive/tlnet (verified)\n\
        =====\ntlmgr itself needs to be updated.\nPlease do this via either\n  tlmgr update --self\n\
        tlmgr: Terminating; please see warning above!";
    assert!(needs_self_update(log));
    assert!(!needs_self_update("tlmgr: package repository x (verified)\n[1/1] install: listingsutf8"));
}

pub fn network_failure(log: &str) -> bool {
    let l = log.to_ascii_lowercase();
    [
        // 连不上主机、DNS 解析不了时 tlmgr 只说这一句
        "could not get texlive.tlpdb",
        "cannot contact",
        "unable to download",
        "failed to download",
        "could not download",
        "cannot download",
        "could not resolve",
        "couldn't resolve",
        "temporary failure in name resolution",
        "network is unreachable",
        "no route to host",
        "connection refused",
        "connection timed out",
        "operation timed out",
        "cannot load tex live database",
        "unable to load remote",
    ]
    .iter()
    .any(|k| l.contains(k))
}

#[cfg(test)]
#[test]
fn network_failures_recognized() {
    assert!(network_failure("tlmgr: TLPDB::from_file could not get texlive.tlpdb from: http://127.0.0.1:9/tlnet/tlpkg/texlive.tlpdb\nMaybe the repository setting should be changed."));
    assert!(network_failure("curl: (6) Could not resolve host: mirror.ctan.org"));
    assert!(!network_failure("tlmgr: package lettrine not present in repository.\nunknown package: lettrine"));
}

/// 开始一次安装：进度从「连接软件源」算起
pub fn progress_start(pkgs: &[String]) {
    CANCEL.store(false, Ordering::SeqCst);
    let now = Instant::now();
    let tmp = std::env::temp_dir().join(format!("subo-tlmgr-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    *PROGRESS.lock().unwrap() = Some(Tracking {
        start: now,
        seen: now,
        tmp,
        wanted: pkgs.to_vec(),
        files: BTreeMap::new(),
        sizes: None,
        first_byte: None,
        p: Progress::default(),
    });
}

/// 安装结束（成功失败都算），之后 [`progress`] 返回 `None`
pub fn progress_done() {
    if let Some(t) = PROGRESS.lock().unwrap().take() {
        let _ = fs::remove_dir_all(&t.tmp);
    }
}

/// 正在跟踪进度时给 tlmgr 用的临时目录（已建好）
fn progress_tmp() -> Option<PathBuf> {
    let tmp = PROGRESS.lock().unwrap().as_ref()?.tmp.clone();
    fs::create_dir_all(&tmp).ok()?;
    Some(tmp)
}

/// 当前安装进度；没有在装时为 `None`。tlmgr 只在每个包开始时报一次已用时间，
/// 中间按真实流逝往上加（不超过预计总时间），界面上的进度条和剩余时间才会走
pub fn progress() -> Option<Progress> {
    let (tmp, need_sizes) = {
        let g = PROGRESS.lock().unwrap();
        let t = g.as_ref()?;
        (t.tmp.clone(), t.sizes.is_none())
    };
    // 扫目录、读包数据库都在锁外做：包数据库二十来 MB，读一遍几十毫秒
    let scan = scan_tmp(&tmp);
    let sizes = if need_sizes { scan.tlpdb.as_deref().and_then(container_sizes) } else { None };

    let mut g = PROGRESS.lock().unwrap();
    let t = g.as_mut()?;
    if sizes.is_some() {
        t.sizes = sizes;
    }
    for (pkg, n) in scan.containers {
        let seen = t.files.entry(pkg.clone()).or_insert(0);
        *seen = (*seen).max(n);
        if !t.wanted.contains(&pkg) {
            t.wanted.push(pkg);
        }
    }
    let got: u64 = t.files.values().sum();
    if got > 0 && t.first_byte.is_none() {
        t.first_byte = Some(Instant::now());
    }
    let want = t.sizes.as_ref().and_then(|s| {
        let known: Vec<u64> = t.wanted.iter().filter_map(|p| s.get(p).copied()).collect();
        (!known.is_empty()).then(|| known.iter().sum())
    });
    let since = t.seen.elapsed().as_secs();
    let spent = t.p.spent.map(|s| t.p.estimate.map_or(s + since, |e| (s + since).min(e)));
    Some(Progress {
        elapsed: t.start.elapsed().as_secs(),
        spent,
        got: (got > 0).then_some(got),
        want,
        downloading: t.first_byte.map_or(0, |f| f.elapsed().as_secs()),
        ..t.p.clone()
    })
}

#[derive(Default)]
struct TmpScan {
    /// `包名 → 已写的字节数`：预取的 `.stageN/<包>.tar.xz`、缓存里的、自己下载的都算
    containers: Vec<(String, u64)>,
    /// tlmgr 下载的软件源包数据库（解压后的临时文件，名字是随机的）
    tlpdb: Option<PathBuf>,
}

/// 扫 tlmgr 的临时目录。File::Temp 会套几层随机目录，最多往下看四层
fn scan_tmp(tmp: &Path) -> TmpScan {
    fn walk(dir: &Path, depth: u8, out: &mut TmpScan) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            let path = e.path();
            if meta.is_dir() {
                if depth < 4 {
                    walk(&path, depth + 1, out);
                }
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            // 容器名是 `<包>.tar.xz`，源码、文档是 `<包>.source.tar.xz` / `<包>.doc.tar.xz`
            if let Some(stem) = name.strip_suffix(".tar.xz") {
                let pkg = stem.trim_end_matches(".doc").trim_end_matches(".source");
                match out.containers.iter_mut().find(|(p, _)| p == pkg) {
                    Some((_, n)) => *n += meta.len(),
                    None => out.containers.push((pkg.to_string(), meta.len())),
                }
            } else if out.tlpdb.is_none() && meta.len() > 1 << 20 && is_tlpdb(&path) {
                out.tlpdb = Some(path);
            }
        }
    }
    let mut out = TmpScan::default();
    walk(tmp, 0, &mut out);
    out
}

/// 包数据库的第一行是 `name 00texlive.config` 之类的 `name ` 开头
fn is_tlpdb(path: &Path) -> bool {
    let mut head = [0u8; 5];
    fs::File::open(path).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok() && &head == b"name "
}

/// 读包数据库里每个包要下载的容器大小：运行文件总要，文档、源码看本机的安装选项
fn container_sizes(path: &Path) -> Option<BTreeMap<String, u64>> {
    let (doc, src) = local_install_opts();
    let text = fs::read_to_string(path).ok()?;
    let mut out = BTreeMap::new();
    let mut name = "";
    for line in text.lines() {
        if let Some(n) = line.strip_prefix("name ") {
            name = n.trim();
            continue;
        }
        let v = line
            .strip_prefix("containersize ")
            .or_else(|| doc.then(|| line.strip_prefix("doccontainersize ")).flatten())
            .or_else(|| src.then(|| line.strip_prefix("srccontainersize ")).flatten());
        if let Some(v) = v.and_then(|v| v.trim().parse::<u64>().ok()) {
            *out.entry(name.to_string()).or_insert(0) += v;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// 本机 TeX Live 装不装文档、源码（`opt_install_docfiles` / `opt_install_srcfiles`），读不到当装文档、不装源码
fn local_install_opts() -> (bool, bool) {
    let db = engine::which("tlmgr").and_then(|t| tl_root(&t)).map(|r| r.join("tlpkg/texlive.tlpdb"));
    // 选项在开头的 00texlive.installation 段里，不用读整个文件
    let head: String = db
        .and_then(|p| fs::File::open(p).ok())
        .map(|f| {
            use std::io::BufRead;
            std::io::BufReader::new(f).lines().map_while(Result::ok).take(200).collect::<Vec<_>>().join("\n")
        })
        .unwrap_or_default();
    let opt = |k: &str, default: bool| {
        head.lines()
            .find_map(|l| l.strip_prefix(&format!("depend opt_{k}:")))
            .map_or(default, |v| v.trim() == "1")
    };
    (opt("install_docfiles", true), opt("install_srcfiles", false))
}

/// 改进度；没在装（比如单测里直接调 install）就什么也不做
pub(crate) fn track(f: impl FnOnce(&mut Progress)) {
    if let Some(t) = PROGRESS.lock().unwrap().as_mut() {
        f(&mut t.p);
        t.seen = Instant::now();
    }
}

/// 补装依赖时把这一轮的包加进要下载的总量
fn track_wanted(pkgs: &[String]) {
    if let Some(t) = PROGRESS.lock().unwrap().as_mut() {
        for p in pkgs {
            if !t.wanted.contains(p) {
                t.wanted.push(p.clone());
            }
        }
    }
}

/// 读 tlmgr 的一行输出，认出进度就记下
fn track_tlmgr(line: &str) {
    if let Some(s) = parse_step(line) {
        track(|p| {
            *p = Progress { phase: "install", followup: p.followup, ..s };
        });
    } else if let Some(n) = parse_prefetch(line) {
        track(|p| {
            p.phase = "download";
            p.total = n;
        });
    } else if line.starts_with("running ") || line.contains("regenerating ") {
        // 包都装完了：mktexlsr、fmtutil、updmap 这些收尾
        track(|p| p.phase = "finish");
    }
}

/// 解析 `tlmgr: prefetching 3 containers using 2 parallel downloads (curl)`（TLUtils::prefetch_start）
fn parse_prefetch(line: &str) -> Option<u32> {
    let rest = line.split_once("prefetching ")?.1;
    let (n, tail) = rest.split_once(' ')?;
    tail.starts_with("container").then(|| n.parse().ok()).flatten()
}

/// 解析 `[2/3, 00:12/00:40] install: erewhon-math [1234k]`（tlmgr.pl 的 action_install 打的）
fn parse_step(line: &str) -> Option<Progress> {
    let rest = line.trim_start().strip_prefix('[')?;
    let (head, tail) = rest.split_once("] ")?;
    let tail = tail.strip_prefix("install: ").or_else(|| tail.strip_prefix("reinstall: "))?;
    let (count, times) = head.split_once(", ")?;
    let (index, total) = count.split_once('/')?;
    let (spent, estimate) = times.split_once('/')?;
    let clock = |s: &str| s.split(':').try_fold(0u64, |acc, n| Some(acc * 60 + n.trim().parse::<u64>().ok()?));
    Some(Progress {
        phase: "install",
        index: index.trim().parse().ok()?,
        total: total.trim().parse().ok()?,
        package: Some(tail.split_whitespace().next()?.to_string()),
        kb: tail.rsplit_once('[').and_then(|(_, k)| k.trim_end().strip_suffix("k]")?.parse().ok()),
        spent: clock(spent),
        estimate: clock(estimate),
        ..Progress::default()
    })
}

/// 给 Perl 的 `PERL5OPT`：在原有的基础上加一句 STDOUT 自动刷新
fn perl_autoflush() -> String {
    let own = "-MIO::Handle;STDOUT->autoflush(1)";
    match std::env::var("PERL5OPT") {
        Ok(v) if !v.trim().is_empty() => format!("{v} {own}"),
        _ => own.to_string(),
    }
}

/// 用户层第一次用时要先建 `TEXMFHOME/tlpkg/texlive.tlpdb`，否则 `--usermode install` 直接报错。
/// 安装时总是带上实际要用的仓库，这里记下的仓库只在没有指定时起作用
fn init_usertree(tlmgr: &Path, deadline: Instant) -> Result<(), String> {
    let home = crate::tex::var("TEXMFHOME").map(PathBuf::from).ok_or("没有设置 TEXMFHOME，无法安装到用户目录")?;
    if home.join("tlpkg").join("texlive.tlpdb").is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(&home).map_err(|e| format!("无法创建 {}：{e}", home.display()))?;
    let ran = engine::run(crate::engine::command(tlmgr).args(["--usermode", "init-usertree"]), deadline)
        .map_err(|e| format!("无法运行 tlmgr：{e}"))?;
    if !ran.ok {
        return Err(format!("初始化用户层失败：{}", tail(&ran.log, 20)));
    }
    // 新建的用户层默认走 mirror.ctan.org 自动调度，实测会被分到海外镜像（装 123 KB 要 33 s）。
    // 沿用主树配好的镜像；读不到就保持默认
    if let Some(repo) = engine::run(crate::engine::command(tlmgr).args(["option", "repository"]), deadline)
        .ok()
        .and_then(|r| repository_url(&r.log))
    {
        let _ = engine::run(crate::engine::command(tlmgr).args(["--usermode", "option", "repository", &repo]), deadline);
    }
    Ok(())
}

/// tlmgr 的提示行里夹着的包名，比如 `tlmgr: package pdftex is not relocatable, …`。
/// `end` 为空时取到行尾。
fn flagged(log: &str, start: &str, end: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in log.lines() {
        let Some(i) = line.find(start) else { continue };
        let rest = &line[i + start.len()..];
        let name = if end.is_empty() { Some(rest.trim()) } else { rest.find(end).map(|j| &rest[..j]) };
        if let Some(n) = name.filter(|n| valid_package(n)) {
            if !out.iter().any(|o| o == n) {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// 不可重定位包的兜底：下载 `archive/<包>.tar.xz`，只取 `texmf-dist/` 下的宏文件放进 TEXMFHOME。
/// 返回装了几个文件。
fn install_runfiles(tlmgr: &Path, pkg: &str, repo: Option<&str>, deadline: Instant) -> Result<usize, String> {
    let home = crate::tex::var("TEXMFHOME").map(PathBuf::from).ok_or("没有设置 TEXMFHOME，无法安装到用户目录")?;
    let repo = repo.map(str::to_string).unwrap_or_else(|| default_repository(tlmgr, deadline));
    if !repo.starts_with("http://") && !repo.starts_with("https://") {
        return Err(format!("仓库 {repo} 不是网址，无法单独下载宏文件"));
    }
    let curl = engine::which("curl").ok_or("找不到 curl，无法下载")?;

    let tmp = std::env::temp_dir().join(format!("subo-tl-{}-{pkg}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let unpacked = tmp.join("x");
    fs::create_dir_all(&unpacked).map_err(|e| format!("无法创建临时目录：{e}"))?;
    let res = (|| {
        let archive = tmp.join(format!("{pkg}.tar.xz"));
        let url = format!("{}/archive/{pkg}.tar.xz", repo.trim_end_matches('/'));
        let got = engine::run(
            crate::engine::command(&curl).args(["-fsSL", "--retry", "2", "--connect-timeout", "20", "-o"]).arg(&archive).arg(&url),
            deadline,
        )
        .map_err(|e| format!("无法运行 curl：{e}"))?;
        if !got.ok {
            return Err(format!("下载 {url} 失败：{}", tail(&got.log, 5)));
        }
        unpack(tlmgr, &archive, &unpacked, deadline)?;
        let src = unpacked.join("texmf-dist");
        let mut n = 0;
        let mut fresh = Vec::new();
        copy_runfiles(&src, &src, &home, &mut n, &mut fresh).map_err(|e| format!("写入 {} 失败：{e}", home.display()))?;
        if n == 0 {
            return Err("容器里没有可用的宏文件".into());
        }
        // tlmgr 不知道这些文件，记一份清单，设置页才列得出、卸得掉。重装时原有文件不算新放的，并进旧清单
        let mut files = load_runfiles(&home, pkg).unwrap_or_default();
        for f in fresh.iter().filter_map(|f| f.strip_prefix(&home).ok()) {
            let rel = f.to_string_lossy().replace('\\', "/");
            if !files.contains(&rel) {
                files.push(rel);
            }
        }
        save_runfiles(&home, pkg, &files).map_err(|e| format!("写入文件清单失败：{e}"))?;
        Ok(n)
    })();
    let _ = fs::remove_dir_all(&tmp);
    let n = res?;
    // 用户层有 ls-R 就刷新一遍：kpathsea 先查 ls-R，旧索引会让新文件晚一步被找到
    if home.join("ls-R").is_file() {
        if let Some(mktexlsr) = engine::which("mktexlsr") {
            let _ = engine::run(crate::engine::command(mktexlsr).arg(&home), deadline);
        }
    }
    Ok(n)
}

/// 没指定安装源时下载容器用的仓库：tlmgr 自己配的，读不到就用 CTAN 的调度地址
pub(crate) fn default_repository(tlmgr: &Path, deadline: Instant) -> String {
    engine::run(crate::engine::command(tlmgr).args(["option", "repository"]), deadline)
        .ok()
        .and_then(|r| repository_url(&r.log))
        .map(|u| if u == "ctan" { DEFAULT_REPO.to_string() } else { u })
        .unwrap_or_else(|| DEFAULT_REPO.to_string())
}

/// 给 tlmgr 指定这一次用的仓库（全局选项，要放在子命令前面）。不改 tlmgr 自己的配置：
/// 设置里切回「跟随 TeX Live」时一切照旧
fn with_repo<'a>(cmd: &'a mut Command, repo: Option<&str>) -> &'a mut Command {
    if let Some(r) = repo {
        cmd.args(["--repository", r]);
    }
    cmd
}

/// 年度版本冻结后，官方在历史归档站保留最后一版软件源 `<年>/tlnet-final`。
/// 主软件源跨到新年度后，旧版 tlmgr 只能连这里。
/// 两处都实测可用；中国大陆连 utah 要 3 s 多，TUNA 0.2 s。
const ARCHIVE_UTAH: &str = "https://ftp.math.utah.edu/pub/tex/historic/systems/texlive";
const ARCHIVE_TUNA: &str = "https://mirrors.tuna.tsinghua.edu.cn/tex-historic-archive/systems/texlive";

/// 连软件源时怎么回退到归档源
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fallback {
    /// 用户自己填的源（网址或本机目录）：不替用户换，报 [`Installed::repo_year`] 让界面说明
    Off,
    /// 跟随 TeX Live、或选了预置镜像：自动换归档源。原来的源是国内镜像时先试国内的归档镜像
    Auto,
}

/// tlmgr 拒绝连软件源、因为软件源已是更新的年度版本时，返回软件源的年份。
/// 原文见 tlmgr.pl 的 `setup_one_remotetlpdb`，三种说法：
/// - 主源：`Local TeX Live (2026) is older than remote repository (2027).`
/// - 源只支持一个年份：`not compatible:` 之后 `local: 2026` / ` repository: 2027 (…)`
/// - 源支持一个年份范围：`supported by the repository … (2027--2028) do not include … (2026).`
/// 软件源比本机旧（镜像没同步）不算，换归档源也没用。
fn repo_newer_year(log: &str, local: u32) -> Option<u32> {
    let year = |s: &str| s.get(..4).and_then(|y| y.parse::<u32>().ok());
    let remote = if let Some(i) = log.find("is older than remote repository (") {
        year(&log[i + "is older than remote repository (".len()..])
    } else if log.contains("are not compatible:") {
        log.lines().find_map(|l| l.trim().strip_prefix("repository: ").and_then(year))
    } else if log.contains("do not include the version of the local installation") {
        // `(最早--最晚)`：本机比最早的还早才是源太新
        log.find("--").and_then(|i| year(log.get(i.checked_sub(4)?..)?))
    } else {
        None
    };
    remote.filter(|&r| r > local)
}

/// `year` 年的归档源，按要试的顺序。
/// `SUBO_TL_ARCHIVE` 可以换成别的地址（单位内网镜像、测试），其中的 `{year}` 换成年份
pub(crate) fn archive_repos(year: u32, cn: bool) -> Vec<String> {
    if let Some(t) = std::env::var("SUBO_TL_ARCHIVE").ok().filter(|t| !t.trim().is_empty()) {
        return vec![t.trim().replace("{year}", &year.to_string())];
    }
    let order = if cn { [ARCHIVE_TUNA, ARCHIVE_UTAH] } else { [ARCHIVE_UTAH, ARCHIVE_TUNA] };
    order.iter().map(|base| format!("{base}/{year}/tlnet-final")).collect()
}

/// 这个进程里已经确认要走的归档源：`(原来的源, 归档源)`。
/// 一旦主源跨了年度就不会再回来，记下来免得每次查询都先白跑一趟主源
static ARCHIVE: Mutex<Option<(Option<String>, String)>> = Mutex::new(None);

/// 这个进程里正在用的归档源（主软件源已跨到新年度版本时）：设置页据此告诉用户
pub fn archive_in_use() -> Option<String> {
    ARCHIVE.lock().unwrap().as_ref().map(|(_, a)| a.clone())
}

/// 本机 TeX Live 的年份；读不到就当没法判断，不回退。换了 TeX 后清掉重读
static LOCAL_YEAR: Mutex<Option<Option<u32>>> = Mutex::new(None);

pub(crate) fn local_year() -> Option<u32> {
    if let Some(y) = *LOCAL_YEAR.lock().unwrap() {
        return y;
    }
    let y = texlive_version().and_then(|v| v.get(..4)?.parse().ok());
    *LOCAL_YEAR.lock().unwrap() = Some(y);
    y
}

/// 换了一套 TeX（或更新了 MiKTeX）：跟年份绑定的记录作废
pub(crate) fn forget_tex() {
    *LOCAL_YEAR.lock().unwrap() = None;
    *ARCHIVE.lock().unwrap() = None;
}

/// 一条要连软件源的 tlmgr 命令跑完的结果
struct Remote {
    ran: engine::Ran,
    /// 实际用了归档源时是它的地址
    archive: Option<String>,
    /// 软件源比本机新、又没有回退（用户自己的源，或归档源也连不上）时，软件源的年份
    repo_year: Option<u32>,
}

/// 跑一条要连软件源的 tlmgr 命令：`build(repo)` 造出带仓库参数的命令。
/// 软件源已跨到新年度版本时，按 `fallback` 换成本机年份的 `tlnet-final` 重试
fn run_remote(
    build: impl Fn(Option<&str>) -> Command,
    repo: Option<&str>,
    fallback: Fallback,
    deadline: Instant,
    on_line: Option<&dyn Fn(&str)>,
) -> std::io::Result<Remote> {
    let run = |cmd: &mut Command| match on_line {
        Some(f) => engine::run_lines(cmd, deadline, f, &cancel_requested),
        None => engine::run(cmd, deadline),
    };
    if fallback == Fallback::Off {
        let ran = run(&mut build(repo))?;
        let repo_year = local_year().and_then(|y| repo_newer_year(&ran.log, y));
        return Ok(Remote { ran, archive: None, repo_year });
    }
    let known = ARCHIVE.lock().unwrap().as_ref().filter(|(r, _)| r.as_deref() == repo).map(|(_, a)| a.clone());
    if let Some(a) = known {
        let ran = run(&mut build(Some(&a)))?;
        return Ok(Remote { ran, archive: Some(a), repo_year: None });
    }
    let ran = run(&mut build(repo))?;
    if ran.stopped {
        return Ok(Remote { ran, archive: None, repo_year: None });
    }
    let Some((local, remote)) = local_year().and_then(|y| Some((y, repo_newer_year(&ran.log, y)?))) else {
        return Ok(Remote { ran, archive: None, repo_year: None });
    };
    let cn = repo.map(str::to_string).or_else(tlmgr_repository).is_some_and(|u| settings::is_cn(&u));
    let mut last = ran;
    let mut tried = None;
    for a in archive_repos(local, cn) {
        eprintln!("[case] 软件源已是 TeX Live {remote}，本机是 {local}：改用归档源 {a}");
        last = run(&mut build(Some(&a)))?;
        if last.ok {
            *ARCHIVE.lock().unwrap() = Some((repo.map(str::to_string), a.clone()));
            return Ok(Remote { ran: last, archive: Some(a), repo_year: None });
        }
        // 连不上（网络、这个镜像缺这一年）就换下一个；超时了、用户取消了不再耗
        if last.timed_out || last.stopped {
            break;
        }
        tried = Some(a);
    }
    // 归档源也没成：日志是归档源的，界面照样要说明主源已跨年度
    Ok(Remote { ran: last, archive: tried, repo_year: Some(remote) })
}

/// 装这些包要不要弹管理员授权框：系统 TeX Live 目录不可写、又不是内置 TeX 的用户层。
/// 自动安装遇到这种情况不装，留给用户点
pub fn needs_admin(usermode: bool) -> bool {
    if usermode {
        return false;
    }
    match engine::which("tlmgr").and_then(|t| tl_root(&t)) {
        Some(root) => !writable(&root.join("tlpkg")),
        None => false,
    }
}

/// tlmgr 自己配置的仓库（「跟随 TeX Live」时实际用的那个）；没有 tlmgr 时 `None`
pub fn tlmgr_repository() -> Option<String> {
    let tlmgr = engine::which("tlmgr")?;
    let ran = engine::run(crate::engine::command(tlmgr).args(["option", "repository"]), Instant::now() + KPSE_TIMEOUT).ok()?;
    repository_url(&ran.log).map(|u| if u == "ctan" { DEFAULT_REPO.to_string() } else { u })
}

/// TeX Live 的年份版本，如 `2026`
pub fn texlive_version() -> Option<String> {
    let tlmgr = engine::which("tlmgr")?;
    let ran = engine::run(crate::engine::command(tlmgr).arg("--version"), Instant::now() + KPSE_TIMEOUT).ok()?;
    ran.log
        .lines()
        .find_map(|l| l.split_once("TeX Live (https://tug.org/texlive) version ").map(|(_, v)| v.trim().to_string()))
}

/// TeX Live 的根目录（`TEXMFROOT`）
pub fn texlive_root() -> Option<PathBuf> {
    tl_root(&engine::which("tlmgr")?)
}

/// 已装的宏包，给设置页看
#[derive(Debug, Serialize, PartialEq)]
pub struct Package {
    pub name: String,
    /// 一句话说明（英文，来自 TeX Live 数据库）
    pub desc: String,
    pub version: String,
    /// `tex`：TeX Live 主树里的（内置或系统安装）；`user`：后来补装进用户层的
    pub source: &'static str,
    /// 占的磁盘空间（字节）：tlmgr 按本地数据库记的、实际装上的部分算，按文件装的直接量文件；量不到为 `None`
    pub bytes: Option<u64>,
}

/// 列出已装的宏包：主树一份，内置 TeX 时再加用户层。只读本地数据库，不联网。
/// 文档类集合（scheme / collection）和平台二进制（`xetex.universal-darwin`）不算。
/// `None`：没有 tlmgr（不是 TeX Live，或 mock 引擎）
pub fn installed_packages(usermode: bool) -> Option<Vec<Package>> {
    let tlmgr = engine::which("tlmgr")?;
    let list = |user: bool| -> Vec<Vec<String>> {
        let mut cmd = crate::engine::command(&tlmgr);
        if user {
            cmd.arg("--usermode");
        }
        cmd.args(["info", "--only-installed", "--data", "name,category,cat-version,shortdesc,size"]);
        engine::run(&mut cmd, Instant::now() + LOOKUP_TIMEOUT)
            .ok()
            .filter(|r| r.ok)
            .map(|r| r.log.lines().map(csv_fields).filter(|f| f.len() == 5).collect())
            .unwrap_or_default()
    };
    let keep = |f: &Vec<String>| !f[0].contains('.') && matches!(f[1].as_str(), "Package" | "TLCore");
    let mut out: Vec<Package> = list(false)
        .into_iter()
        .filter(keep)
        .map(|f| Package { name: f[0].clone(), version: f[2].clone(), desc: f[3].clone(), source: "tex", bytes: f[4].parse().ok() })
        .collect();
    if usermode {
        // 用户层的数据库会连带记下主树里已有的依赖，主树有的仍算主树的
        for f in list(true).into_iter().filter(keep) {
            if !out.iter().any(|p| p.name == f[0]) {
                out.push(Package { name: f[0].clone(), version: f[2].clone(), desc: f[3].clone(), source: "user", bytes: f[4].parse().ok() });
            }
        }
        // 不可重定位、按文件装进来的包 tlmgr 不知道，照清单补上
        if let Some(home) = user_home() {
            for name in runfiles_packages(&home) {
                if !out.iter().any(|p| p.name == name) {
                    let bytes = load_runfiles(&home, &name)
                        .map(|files| files.iter().filter_map(|f| fs::metadata(home.join(f)).ok()).map(|m| m.len()).sum());
                    out.push(Package { name, version: String::new(), desc: String::new(), source: "user", bytes });
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Some(out)
}

// ---- 用户层宏包的卸载、重装（设置页） ----
//
// 只管内置 TeX 的用户层（TEXMFHOME）：主树在应用包里只读，用户自己的 TeX Live 交给 tlmgr。
// 用户层里的包有两种来源：tlmgr --usermode 装的，记在用户层 tlpdb；不可重定位、
// 只取了宏文件的（见 install_runfiles），记在 `<用户层上级>/packages/<包>.json`。

pub(crate) fn user_home() -> Option<PathBuf> {
    crate::tex::var("TEXMFHOME").map(PathBuf::from)
}

fn runfiles_dir(home: &Path) -> PathBuf {
    home.parent().unwrap_or(home).join("packages")
}

fn load_runfiles(home: &Path, pkg: &str) -> Option<Vec<String>> {
    serde_json::from_slice(&fs::read(runfiles_dir(home).join(format!("{pkg}.json"))).ok()?).ok()
}

pub(crate) fn save_runfiles(home: &Path, pkg: &str, files: &[String]) -> std::io::Result<()> {
    let dir = runfiles_dir(home);
    fs::create_dir_all(&dir)?;
    let f = dir.join(format!("{pkg}.json"));
    let tmp = f.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(files).map_err(std::io::Error::other)?)?;
    fs::rename(&tmp, &f)
}

/// 有文件清单的包名
fn runfiles_packages(home: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(runfiles_dir(home)) else { return Vec::new() };
    let mut out: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(str::to_string))
        .filter(|n| valid_package(n))
        .collect();
    out.sort();
    out
}

/// 用户层 tlpdb 里一个包的块：`name` 行到下一个空行
struct TlpdbEntry {
    name: String,
    /// 相对用户层根的 runfiles
    runfiles: Vec<String>,
    /// `execute addMap` / `addMixedMap` 登记的映射文件名
    maps: Vec<String>,
}

fn user_tlpdb(home: &Path) -> Vec<TlpdbEntry> {
    let text = fs::read_to_string(home.join("tlpkg").join("texlive.tlpdb")).unwrap_or_default();
    parse_tlpdb(&text)
}

fn parse_tlpdb(text: &str) -> Vec<TlpdbEntry> {
    let mut out: Vec<TlpdbEntry> = Vec::new();
    let mut in_runfiles = false;
    for l in text.lines() {
        if let Some(name) = l.strip_prefix("name ") {
            out.push(TlpdbEntry { name: name.trim().to_string(), runfiles: Vec::new(), maps: Vec::new() });
            in_runfiles = false;
            continue;
        }
        let Some(e) = out.last_mut() else { continue };
        if l.starts_with("runfiles") {
            in_runfiles = true;
        } else if let Some(f) = l.strip_prefix(' ').filter(|_| in_runfiles) {
            // ` fonts/opentype/foo/Foo.otf details="…"`：路径后面可能跟属性
            if let Some(path) = f.split_whitespace().next() {
                e.runfiles.push(path.trim_start_matches("RELOC/").to_string());
            }
        } else {
            in_runfiles = false;
            if let Some(rest) = l.strip_prefix("execute add") {
                if let Some(("Map" | "MixedMap", m)) = rest.split_once(' ') {
                    e.maps.push(m.trim().to_string());
                }
            }
        }
    }
    out.retain(|e| !e.name.starts_with("00texlive"));
    out
}

/// 用户层里每个文件（相对用户层根，`/` 分隔）属于哪个包：设置页的字体据此找到能卸载的包
pub fn user_file_owners() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(home) = user_home() else { return out };
    for e in user_tlpdb(&home) {
        for f in e.runfiles {
            out.entry(f).or_insert_with(|| e.name.clone());
        }
    }
    for pkg in runfiles_packages(&home) {
        for f in load_runfiles(&home, &pkg).unwrap_or_default() {
            out.entry(f).or_insert_with(|| pkg.clone());
        }
    }
    out
}

/// 用户层里每个包有哪些文件（相对用户层根）。tlmgr 登记的、按文件装的都算
pub fn user_packages() -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Some(home) = user_home() else { return out };
    for e in user_tlpdb(&home) {
        out.entry(e.name).or_default().extend(e.runfiles);
    }
    for pkg in runfiles_packages(&home) {
        let files = load_runfiles(&home, &pkg).unwrap_or_default();
        out.entry(pkg).or_default().extend(files);
    }
    out
}

/// 要卸载、重装的包必须都在用户层里
fn check_user_packages(home: &Path, pkgs: &[String]) -> Result<(Vec<TlpdbEntry>, Vec<String>), String> {
    if pkgs.is_empty() || !pkgs.iter().all(|p| valid_package(p)) {
        return Err("宏包名不合法".into());
    }
    let db = user_tlpdb(home);
    let files = runfiles_packages(home);
    if let Some(p) = pkgs.iter().find(|p| !db.iter().any(|e| &e.name == *p) && !files.contains(p)) {
        return Err(format!("{p} 不是补装进用户层的宏包"));
    }
    Ok((db, files))
}

/// 卸载补装进用户层的宏包。tlmgr 登记过的交给 `tlmgr --usermode remove`，
/// 按文件装的照清单删；包登记过的 Type1 映射一并从 `updmap-user` 撤掉。
/// 返回 tlmgr 等的输出末尾，出错时给人看。
pub fn remove(pkgs: &[String]) -> Result<String, String> {
    let home = user_home().ok_or("没有设置 TEXMFHOME")?;
    let (db, files) = check_user_packages(&home, pkgs)?;
    let deadline = Instant::now() + INSTALL_TIMEOUT;
    let mut log = String::new();

    let listed: Vec<&String> = pkgs.iter().filter(|p| db.iter().any(|e| &e.name == *p)).collect();
    let maps: Vec<String> =
        db.iter().filter(|e| listed.contains(&&e.name)).flat_map(|e| e.maps.iter().cloned()).collect();
    if !listed.is_empty() {
        let tlmgr = engine::which("tlmgr").ok_or("找不到 tlmgr")?;
        let ran = engine::run(crate::engine::command(&tlmgr).args(["--usermode", "remove"]).args(&listed), deadline)
            .map_err(|e| format!("无法运行 tlmgr：{e}"))?;
        log.push_str(&ran.log);
        let left = user_tlpdb(&home);
        if !ran.ok || listed.iter().any(|p| left.iter().any(|e| &e.name == *p)) {
            return Err(format!("卸载失败：{}", tail(&ran.log, 20)));
        }
    }
    for pkg in pkgs.iter().filter(|p| files.contains(p)) {
        for rel in load_runfiles(&home, pkg).unwrap_or_default() {
            remove_user_file(&home, &rel);
        }
        let _ = fs::remove_file(runfiles_dir(&home).join(format!("{pkg}.json")));
    }
    refresh_user_lsr(&home, deadline);
    if let Err(e) = unregister_user_maps(&home, &maps, deadline) {
        log.push_str(&format!("\n{e}"));
    }
    Ok(tail(&log, 40))
}

/// 删用户层里的一个文件，顺手删掉变空的上级目录，到用户层根为止。
/// 清单是自己写的，仍只认用户层里的相对路径，`..` 一律不删
fn remove_user_file(home: &Path, rel: &str) {
    let rel = Path::new(rel);
    if rel.is_absolute() || rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return;
    }
    let p = home.join(rel);
    let _ = fs::remove_file(&p);
    let mut dir = p.parent();
    while let Some(d) = dir.filter(|d| d.starts_with(home) && *d != home) {
        if fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// 用户层有 ls-R 就刷新：kpathsea 先查 ls-R，旧索引里删掉的文件还会被找到
pub(crate) fn refresh_user_lsr(home: &Path, deadline: Instant) {
    if home.join("ls-R").is_file() {
        if let Some(mktexlsr) = engine::which("mktexlsr") {
            let _ = engine::run(crate::engine::command(mktexlsr).arg(home), deadline);
        }
    }
}

/// 卸掉的包登记过的映射，余下的包都不再用、`updmap.cfg` 里又登记着的，撤掉再重新生成
fn unregister_user_maps(home: &Path, maps: &[String], deadline: Instant) -> Result<(), String> {
    let still: Vec<String> = user_tlpdb(home).into_iter().flat_map(|e| e.maps).collect();
    let cfg: String = [Some(home.to_path_buf()), crate::tex::var("TEXMFCONFIG").map(PathBuf::from)]
        .into_iter()
        .flatten()
        .filter_map(|d| fs::read_to_string(d.join("web2c").join("updmap.cfg")).ok())
        .collect::<Vec<_>>()
        .join("\n");
    let known: Vec<&str> = cfg.lines().filter_map(|l| l.split_whitespace().nth(1)).collect();
    let gone: Vec<&String> = maps.iter().filter(|m| !still.contains(m) && known.contains(&m.as_str())).collect();
    if gone.is_empty() {
        return Ok(());
    }
    let updmap = engine::which("updmap-user").ok_or("找不到 updmap-user")?;
    for map in gone {
        let ran = engine::run(crate::engine::command(&updmap).args(["--nomkmap", "--disable"]).arg(map), deadline)
            .map_err(|e| format!("无法运行 updmap-user：{e}"))?;
        if !ran.ok {
            return Err(format!("撤销 {map} 失败：{}", tail(&ran.log, 10)));
        }
    }
    let ran = engine::run(&mut crate::engine::command(&updmap), deadline).map_err(|e| format!("无法运行 updmap-user：{e}"))?;
    if !ran.ok {
        return Err(format!("生成字体映射失败：{}", tail(&ran.log, 10)));
    }
    Ok(())
}

/// 重装补装进用户层的宏包：从安装源重新下载覆盖，文件坏了、被误删了用。
/// 不可重定位的包 tlmgr 照样报不可重定位，由 install_once 改走按文件安装
pub fn reinstall(pkgs: &[String], repo: Option<&str>, fallback: Fallback) -> Result<Installed, String> {
    let home = user_home().ok_or("没有设置 TEXMFHOME")?;
    check_user_packages(&home, pkgs)?;
    install_once(pkgs, true, repo, fallback, true)
}

/// tlmgr `--data` 的一行 CSV：带空格、逗号的字段加双引号，引号本身写成 `\"`
fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// tlmgr 自己的默认仓库，会被重定向到就近镜像
const DEFAULT_REPO: &str = "https://mirror.ctan.org/systems/texlive/tlnet";

/// 解 `.tar.xz`。macOS 的 bsdtar 自带 xz；GNU tar 要外部 xz，PATH 里没有就用 TeX Live 自带的那份。
pub(crate) fn unpack(tlmgr: &Path, archive: &Path, dest: &Path, deadline: Instant) -> Result<(), String> {
    let direct = engine::run(crate::engine::command("tar").arg("-xf").arg(archive).arg("-C").arg(dest), deadline);
    if direct.as_ref().is_ok_and(|r| r.ok) {
        return Ok(());
    }
    let xz = engine::which("xz").or_else(|| bundled_xz(tlmgr)).ok_or("解压失败：找不到 xz")?;
    let tar = archive.with_extension("");
    let ran = engine::run(crate::engine::command(&xz).args(["-dkf"]).arg(archive), deadline)
        .map_err(|e| format!("无法运行 xz：{e}"))?;
    if !ran.ok || !tar.is_file() {
        return Err(format!("解压失败：{}", tail(&ran.log, 5)));
    }
    let ran = engine::run(crate::engine::command("tar").arg("-xf").arg(&tar).arg("-C").arg(dest), deadline)
        .map_err(|e| format!("无法运行 tar：{e}"))?;
    if ran.ok { Ok(()) } else { Err(format!("解包失败：{}", tail(&ran.log, 5))) }
}

/// `tlpkg/installer/xz/xz.<平台>`
fn bundled_xz(tlmgr: &Path) -> Option<PathBuf> {
    let dir = tl_root(tlmgr)?.join("tlpkg").join("installer").join("xz");
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n == "xz" || n.starts_with("xz.")))
}

/// 把 `texmf-dist/` 下（可重定位包是容器根下）的文件按原相对路径拷进用户层；文档、源码不编译用不上，跳过。
/// 只跟普通文件和目录，不跟符号链接：容器内容不该指到别处。
/// `fresh` 收下原先没有、这次新放进去的文件（卸载引擎时只删这些，见 engines.rs）。
pub(crate) fn copy_runfiles(root: &Path, dir: &Path, home: &Path, n: &mut usize, fresh: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for e in fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let rel = p.strip_prefix(root).unwrap_or(&p);
        // 可重定位包的容器没有 texmf-dist 这层，顶层还带着 tlpkg/（包的元数据），也不要
        if dir == root && matches!(rel.to_str(), Some("doc" | "source" | "tlpkg")) {
            continue;
        }
        let ft = e.file_type()?;
        if ft.is_dir() {
            copy_runfiles(root, &p, home, n, fresh)?;
        } else if ft.is_file() {
            let to = home.join(rel);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            if !to.exists() {
                fresh.push(to.clone());
            }
            fs::copy(&p, &to)?;
            *n += 1;
        }
    }
    Ok(())
}

/// `Default package repository (repository): https://…` → URL
fn repository_url(out: &str) -> Option<String> {
    out.lines()
        .find_map(|l| l.split_once("(repository): ").map(|(_, u)| u.trim().to_string()))
        .filter(|u| !u.is_empty())
}

pub(crate) fn valid_package(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 64
        && !p.starts_with('-')
        && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `…/bin/<arch>/tlmgr` 往上两级是 TEXMFROOT。`texbin` 这类软链先解开。
fn tl_root(tlmgr: &Path) -> Option<PathBuf> {
    let bin = dunce::canonicalize(&tlmgr.parent()?).ok()?;
    Some(bin.parent()?.parent()?.to_path_buf())
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".subo-probe-{}", std::process::id()));
    let ok = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

pub(crate) fn tail(log: &str, n: usize) -> String {
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// 缺的文件里，项目子目录中就有同名文件的挑出来，从 `files` 里拿掉，不再去问 tlmgr。
///
/// xelatex 在项目根目录里跑，TeX 只找当前目录和 texmf 树，不会钻进子目录：
/// 文档类放在 `style/` 里、`\documentclass{foo}` 就找不到。项目里名字带 texmf 的树
/// 编译时已经挂上了（见 engine::project_trees），走到这里的是没放对地方的。
pub fn in_project(root: &Path, files: &mut Vec<String>) -> Vec<Missing> {
    let paths: Vec<String> = crate::project::tree(root).into_iter().filter(|e| !e.dir).map(|e| e.path).collect();
    let mut out = Vec::new();
    files.retain(|file| match locate(&paths, file) {
        Some(p) => {
            out.push(Missing {
                kind: Kind::File,
                file: file.clone(),
                package: None,
                lookup: Lookup::Project,
                needed_by: Vec::new(),
                font_file: None,
                project_path: Some(p),
            });
            false
        }
        None => true,
    });
    out
}

/// 项目文件里末段是 `file` 的（不带扩展名时也认 `.tex`），有几个取层级最浅的
pub(crate) fn locate(paths: &[String], file: &str) -> Option<String> {
    let tex = format!("{file}.tex");
    paths
        .iter()
        .filter(|p| {
            let base = p.rsplit(['/', '\\']).next().unwrap_or("");
            base == file || (!file.contains('.') && base == tex)
        })
        .min_by_key(|p| (p.matches(['/', '\\']).count(), p.len()))
        .map(|p| p.replace('\\', "/"))
}

pub fn pending(files: &[String]) -> Vec<Missing> {
    files
        .iter()
        .map(|f| Missing { kind: Kind::File, file: f.clone(), package: None, lookup: Lookup::Pending, needed_by: Vec::new(), font_file: None, project_path: None })
        .collect()
}

fn found_or_absent(file: &str, pkg: Option<String>) -> Missing {
    let lookup = if pkg.is_some() { Lookup::Found } else { Lookup::Absent };
    Missing { kind: Kind::File, file: file.to_string(), package: pkg, lookup, needed_by: Vec::new(), font_file: None, project_path: None }
}

/// 按名字找不到的字体，XeTeX 还会当 TFM 再找一遍；带 Metafont 的 TeX Live 会为它跑 mktextfm，
/// 回显 ``! I can't find file `FontAwesome'``。这不是缺文件，从文件清单里去掉，免得重复报一条
/// 「TeX Live 中没有」。
pub fn drop_font_echoes(files: &mut Vec<String>, fonts: &[String]) {
    files.retain(|f| !fonts.iter().any(|n| f == n || f.strip_suffix(".tfm") == Some(n.as_str())));
}

/// 缺的字体：直接给定论，不必等查询。
///
/// 多数是系统字体，TeX Live 管不了。例外是 TeX Live 自带了这个字体、只是按名字找不到：
/// XeTeX 按名字只问操作系统（macOS 走 Core Text，Linux 走 fontconfig），看不见 texmf 树。
/// 编译时已为 TeX Live 自带字体挂上 `.fontspec` 映射（见 fontmap.rs），
/// 走到这里的是映射没覆盖到的（集合字体、名字带非 ASCII 字符的族、映射还没生成好）。
/// 用一次 kpsewhich 按常见的文件命名去找，找到就标 Found，告诉用户改用文件名加载。
pub fn fonts(names: &[String]) -> Vec<Missing> {
    let mut cands: Vec<String> = Vec::new();
    for c in names.iter().flat_map(|n| font_file_candidates(n)) {
        if !cands.contains(&c) {
            cands.push(c);
        }
    }
    let found = kpse_find(&cands);
    names.iter().map(|n| font_missing(n, &found)).collect()
}

/// `found` 是 kpsewhich 找到的文件名
fn font_missing(name: &str, found: &[String]) -> Missing {
    let font_file = match_font_file(name, found);
    let lookup = if font_file.is_some() { Lookup::Found } else { Lookup::Absent };
    Missing { kind: Kind::Font, file: name.to_string(), package: None, lookup, needed_by: Vec::new(), font_file, project_path: None }
}

/// 本机没有、但完整版 TeX Live 带着的字体：按常见命名在随包索引（[`FILE_INDEX`]）里找文件，
/// 返回 `(字体名, 索引里的文件名)`。xetexko 模板写 `\setmainhangulfont{UnBatang.ttf}`，没装
/// unfonts-core 时 fontspec 报 `The font "UnBatang" cannot be found`；换成缺文件 `UnBatang.ttf`，
/// 就能跟缺 `.sty` 一样查出包、一键装上。只看 [`fonts`] 判为 Absent 的，没有索引就一个不报。
pub fn installable_fonts(fonts: &[Missing]) -> Vec<(String, String)> {
    let names: Vec<&str> = fonts.iter().filter(|m| m.kind == Kind::Font && m.lookup == Lookup::Absent).map(|m| m.file.as_str()).collect();
    if names.is_empty() {
        return Vec::new();
    }
    file_index().map(|p| fonts_in_index(&p, &names)).unwrap_or_default()
}

fn fonts_in_index(index: &Path, names: &[&str]) -> Vec<(String, String)> {
    let cands: Vec<String> = names.iter().flat_map(|n| font_file_candidates(n)).collect();
    let Some(found) = index_lookup(index, &cands) else { return Vec::new() };
    names
        .iter()
        .filter_map(|n| font_file_candidates(n).iter().find_map(|c| found.get(c)).map(|(_, file)| (n.to_string(), file.clone())))
        .collect()
}

/// 字体名可能对应的文件名，按优先级排：`TeX Gyre Pagella` → `TeXGyrePagella.otf`、
/// `TeXGyrePagella-Regular.otf`……`texgyrepagella-regular.otf`。名字里有文件名不该有的字符就不猜。
fn font_file_candidates(name: &str) -> Vec<String> {
    let stem: String = name.chars().filter(|c| *c != ' ').collect();
    if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return Vec::new();
    }
    let lower = stem.to_ascii_lowercase();
    let mut out = Vec::new();
    for base in [stem.clone(), format!("{stem}-Regular"), lower.clone(), format!("{lower}-regular")] {
        for ext in [".otf", ".ttf"] {
            let c = format!("{base}{ext}");
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out
}

/// 按候选顺序取第一个找到的文件名。kpsewhich 在大小写不敏感的文件系统上
/// 会把 `fontawesome.otf` 答成 `FontAwesome.otf`，所以不分大小写比。
fn match_font_file(name: &str, found: &[String]) -> Option<String> {
    font_file_candidates(name)
        .iter()
        .find_map(|c| found.iter().find(|f| f.eq_ignore_ascii_case(c)).cloned())
}

/// 一次 kpsewhich 查一批文件，返回找到的文件名（不带目录）。
/// 在临时目录里跑：项目目录里的同名文件不算「TeX Live 自带」。
fn kpse_find(files: &[String]) -> Vec<String> {
    kpse_paths(files).iter().filter_map(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())).collect()
}

/// kpsewhich 找到的绝对路径
fn kpse_paths(files: &[String]) -> Vec<PathBuf> {
    if files.is_empty() || engine::which("kpsewhich").is_none() {
        return Vec::new();
    }
    let Ok(ran) = engine::run(
        crate::engine::command("kpsewhich").args(files).current_dir(std::env::temp_dir()),
        Instant::now() + KPSE_TIMEOUT,
    ) else {
        return Vec::new();
    };
    // 有一个没找到时退出码就非 0，找到的照样打印，所以不看 ok
    ran.log.lines().map(PathBuf::from).filter(|p| p.is_absolute()).collect()
}

/// `Some(Some(pkg))` 找到；`Some(None)` 远端明确没有；`None` 没查成。
fn tlmgr_lookup(file: &str, repo: Option<&str>, fallback: Fallback) -> Option<Option<String>> {
    let pattern = file_pattern(file);
    let build = |r: Option<&str>| {
        let mut cmd = crate::engine::command(engine::which("tlmgr").unwrap_or_else(|| "tlmgr".into()));
        with_repo(&mut cmd, r).args(["search", "--global", "--file", &pattern]);
        cmd
    };
    let ran = run_remote(build, repo, fallback, Instant::now() + LOOKUP_TIMEOUT, None).ok()?.ran;
    if !ran.ok {
        return None;
    }
    Some(pick_package(&ran.log, file))
}

/// tlmgr 的 `--file` 是 Perl 正则，按「路径末段恰好是这个文件名」来锚定，
/// 否则 `xltxtra.sty` 会顺带匹配 `lwarp-xltxtra.sty`。
fn file_pattern(file: &str) -> String {
    let esc = file.replace('.', r"\.").replace('+', r"\+");
    if file.contains('.') {
        format!("/{esc}$")
    } else {
        // `\input foo` 不带扩展名时 TeX 会补 .tex
        format!(r"/{esc}(\.tex)?$")
    }
}

/// 解析 tlmgr 的输出：
/// ```text
/// helvetic:
/// 	texmf-dist/fonts/tfm/adobe/helvetic/phvr8t.tfm
/// ```
/// 取第一个含有末段完全相同文件的包。文档与源码目录不算：
/// 引擎根本不会去那里找，`chap1.tex` 不该被说成 hepthesis 示例里的那份。
fn pick_package(out: &str, file: &str) -> Option<String> {
    let mut pkg: Option<&str> = None;
    for line in out.lines() {
        if let Some(path) = line.strip_prefix('\t') {
            let path = path.trim();
            if path.starts_with("texmf-dist/doc/") || path.starts_with("texmf-dist/source/") {
                continue;
            }
            let base = path.rsplit('/').next().unwrap_or("");
            let hit = base == file || (!file.contains('.') && base == format!("{file}.tex"));
            if let (true, Some(p)) = (hit, pkg) {
                return Some(p.to_string());
            }
        } else if let Some(name) = line.strip_suffix(':') {
            // 「tlmgr: package repository …」这类提示行不以冒号结尾，不会被当成包名
            if !name.contains(' ') {
                pkg = Some(name);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_encoding_file() {
        // 原样是在 79 列处折断的两行
        let line = "./gstarticle.cls:6: Package fontenc Error: Encoding file `ly1enc.def' not found.";
        let wrapped = format!("{}\n{}\n", &line[..MAX_PRINT_LINE], &line[MAX_PRINT_LINE..]);
        assert_eq!(missing_files(&wrapped), vec!["ly1enc.def"]);
    }

    #[test]
    fn pdftex_missing_glyphs() {
        let ec = "!pdfTeX error: pdflatex (file tcrm1000): Font tcrm1000 at 602 not found\n ==> Fatal error occurred, no output PDF file produced!\n";
        assert_eq!(missing_files(ec), vec!["cm-super-ts1.map"]);
        let t1 = "!pdfTeX error: pdflatex (file ecrm1095): Font ecrm1095 at 600 not found\n";
        assert_eq!(missing_files(t1), vec!["cm-super-t1.map"]);
        let other = "!pdfTeX error: pdflatex (file XCharterMathMI): Font XCharterMathMI at 337 not found\n";
        assert_eq!(missing_files(other), vec!["XCharterMathMI.pfb"]);
        // tcx 这类短名不当 EC 字体
        assert_eq!(pdftex_glyphs("tcx"), "tcx.pfb");
    }

    #[test]
    fn user_maps_to_register() {
        let tlpdb = "name cm-super\nexecute addMixedMap cm-super-t1.map\nexecute addMixedMap cm-super-ts1.map\n\
                     name xcharter\nexecute addMap XCharter.map\nexecute AddFormat name=x\nname times\nexecute addMap utm.map\n";
        let cfg = "# Generated\nMixedMap cm-super-t1.map\nMap utm.map\n";
        assert_eq!(
            unregistered_maps(tlpdb, cfg),
            vec![("MixedMap", "cm-super-ts1.map".to_string()), ("Map", "XCharter.map".to_string())]
        );
        assert!(unregistered_maps("", cfg).is_empty());
    }

    #[test]
    fn repository_from_tlmgr_option() {
        let out = "Default package repository (repository): https://mirrors.tuna.tsinghua.edu.cn/CTAN/systems/texlive/tlnet\n";
        assert_eq!(
            repository_url(out).as_deref(),
            Some("https://mirrors.tuna.tsinghua.edu.cn/CTAN/systems/texlive/tlnet")
        );
        assert_eq!(repository_url("tlmgr: something else"), None);
    }

    /// tlmgr.pl `setup_one_remotetlpdb` 的三种原文（TL 2026 的 tlmgr），前面带 tlmgr 的前缀行
    #[test]
    fn newer_repo_recognized() {
        let main = "tlmgr: Local TeX Live (2026) is older than remote repository (2027).\n\
                    Cross release updates are only supported with\n  update-tlmgr-latest(.sh/.exe) -- --upgrade\n\
                    See https://tug.org/texlive/upgrade.html for details.\n\
                    tlmgr: Cannot load TeX Live database from https://mirrors.tuna.tsinghua.edu.cn/CTAN/systems/texlive/tlnet\n";
        assert_eq!(repo_newer_year(main, 2026), Some(2027));
        let single = "The TeX Live versions of the local installation\nand the repository are not compatible:\n      \
                      local: 2026\n repository: 2027 (https://example.org/tlnet)\nPerhaps that particular CTAN mirror is outdated?\n";
        assert_eq!(repo_newer_year(single, 2026), Some(2027));
        let range = "The TeX Live versions supported by the repository\nhttps://example.org/tlnet\n  (2027--2028)\n\
                     do not include the version of the local installation\n  (2026).\n";
        assert_eq!(repo_newer_year(range, 2026), Some(2027));
    }

    #[test]
    fn older_or_unrelated_repo_not_recognized() {
        // 镜像没同步、比本机还旧：换归档源没用
        let stale = "The TeX Live versions of the local installation\nand the repository are not compatible:\n      \
                     local: 2026\n repository: 2025 (https://example.org/tlnet)\n";
        assert_eq!(repo_newer_year(stale, 2026), None);
        let range = "The TeX Live versions supported by the repository\nx\n  (2016--2025)\n\
                     do not include the version of the local installation\n  (2026).\n";
        assert_eq!(repo_newer_year(range, 2026), None);
        assert_eq!(repo_newer_year("tlmgr: Cannot load TeX Live database from https://x/tlnet\n", 2026), None);
        assert_eq!(repo_newer_year("tlmgr install: unknown package: nosuchpkg\n", 2026), None);
    }

    #[test]
    fn archive_follows_region() {
        assert_eq!(
            archive_repos(2026, false),
            vec![
                "https://ftp.math.utah.edu/pub/tex/historic/systems/texlive/2026/tlnet-final".to_string(),
                "https://mirrors.tuna.tsinghua.edu.cn/tex-historic-archive/systems/texlive/2026/tlnet-final".to_string(),
            ]
        );
        assert!(archive_repos(2026, true)[0].contains("tuna"));
        assert!(settings::is_cn("https://mirrors.ustc.edu.cn/CTAN/systems/texlive/tlnet/"));
        assert!(!settings::is_cn("https://mirror.ctan.org/systems/texlive/tlnet"));
    }

    /// 用一个假 tlmgr 走一遍 run_remote：主源报「本机比软件源旧」，换归档源后成功并记住
    #[cfg(unix)]
    #[test]
    fn falls_back_to_archive() {
        let dir = std::env::temp_dir().join(format!("subo-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("tlmgr");
        fs::write(
            &fake,
            "#!/bin/sh\ncase \"$2\" in *tlnet-final) echo \"ok $2\"; exit 0;; esac\n\
             echo 'tlmgr: Local TeX Live (2026) is older than remote repository (2027).' >&2; exit 1\n",
        )
        .unwrap();
        fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        *LOCAL_YEAR.lock().unwrap() = Some(Some(2026));
        let build = |r: Option<&str>| {
            let mut cmd = crate::engine::command(&fake);
            with_repo(&mut cmd, r).arg("install");
            cmd
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let main = "https://mirror.ctan.org/systems/texlive/tlnet";

        // 用户自己的源：不换，只报软件源的年份
        let r = run_remote(&build, Some(main), Fallback::Off, deadline, None).unwrap();
        assert!(!r.ran.ok);
        assert_eq!((r.archive, r.repo_year), (None, Some(2027)));

        let r = run_remote(&build, Some(main), Fallback::Auto, deadline, None).unwrap();
        assert!(r.ran.ok, "{}", r.ran.log);
        let used = r.archive.unwrap();
        assert!(used.ends_with("/2026/tlnet-final") && r.ran.log.contains(&used));
        assert_eq!(r.repo_year, None);
        // 记住了：下一次直接用归档源
        assert_eq!(ARCHIVE.lock().unwrap().as_ref().map(|(_, a)| a.clone()), Some(used));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_tlmgr_progress_lines() {
        let p = parse_step("[1/3, ??:??/??:??] install: cm-super [65288k]").unwrap();
        assert_eq!((p.index, p.total, p.package.as_deref(), p.kb), (1, 3, Some("cm-super"), Some(65288)));
        assert_eq!((p.spent, p.estimate), (None, None));
        let p = parse_step("[2/3, 00:12/01:02:05] install: erewhon-math @1 [1234k]").unwrap();
        assert_eq!((p.index, p.package.as_deref(), p.kb), (2, Some("erewhon-math"), Some(1234)));
        assert_eq!((p.spent, p.estimate), (Some(12), Some(3725)));
        assert!(parse_step("[1/2, ??:??/??:??] remove: foo").is_none());
        assert!(parse_step("tlmgr: package repository https://x (verified)").is_none());
        assert_eq!(parse_prefetch("tlmgr: prefetching 3 containers using 2 parallel downloads (curl)"), Some(3));
        assert_eq!(parse_prefetch("tlmgr: prefetching 1 container using 1 parallel download (curl)"), Some(1));
        assert_eq!(parse_prefetch("tlmgr install: package already present: foo"), None);
    }

    /// 实测 `tlmgr --usermode install pdftex` 的输出：退出码是 0，什么也没装
    #[test]
    fn csv_line_parsed() {
        assert_eq!(
            csv_fields(r#"acmart,Package,2.20,"Class for typesetting publications of ACM""#),
            vec!["acmart", "Package", "2.20", "Class for typesetting publications of ACM"]
        );
        assert_eq!(
            csv_fields(r#"x,Package,,"A \"quoted\", comma""#),
            vec!["x", "Package", "", r#"A "quoted", comma"#]
        );
    }

    #[test]
    fn not_relocatable_is_flagged() {
        let out = "tlmgr: package repository https://mirrors.tuna.tsinghua.edu.cn/CTAN/systems/texlive/tlnet (not verified: gpg unavailable)\n\
                   tlmgr: prefetching 1 containers using 1 parallel downloads (curl)\n\
                   tlmgr: package pdftex is not relocatable, cannot install it in user mode!\n\
                   tlmgr: package luaotfload is not relocatable, cannot install it in user mode!\n\
                   tlmgr: package pdftex is not relocatable, cannot install it in user mode!\n";
        assert_eq!(flagged(out, "package ", " is not relocatable"), vec!["pdftex", "luaotfload"]);
        assert!(flagged(out, "unknown package: ", "").is_empty());
    }

    #[test]
    fn unknown_package_is_flagged() {
        let out = "tlmgr install: unknown package: nosuchpkg\ntlmgr: package already present: marvosym\n";
        assert_eq!(flagged(out, "unknown package: ", ""), vec!["nosuchpkg"]);
        assert!(flagged(out, "package ", " is not relocatable").is_empty());
    }

    #[test]
    fn runfiles_skip_doc_and_source() {
        let tmp = std::env::temp_dir().join(format!("subo-runfiles-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("x/texmf-dist");
        for f in ["tex/generic/pdftex/glyphtounicode.tex", "doc/pdftex/manual.pdf", "source/pdftex/x.dtx"] {
            let p = src.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
        }
        let home = tmp.join("home");
        let mut n = 0;
        let mut fresh = Vec::new();
        copy_runfiles(&src, &src, &home, &mut n, &mut fresh).unwrap();
        assert_eq!(n, 1);
        assert_eq!(fresh, vec![home.join("tex/generic/pdftex/glyphtounicode.tex")]);
        assert!(home.join("tex/generic/pdftex/glyphtounicode.tex").is_file());
        assert!(!home.join("doc").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    const NO_GS: &str = include_str!("../testdata/missing-ghostscript.txt");
    const NO_GS_PLAIN: &str = include_str!("../testdata/missing-ghostscript-plain.txt");
    const EPS_TEXLOG: &str = include_str!("../testdata/eps-texlog.txt");

    /// 内置 TeX：rungs.sh 的标记行接在 TeX 输出后面，不另起一行
    #[test]
    fn ghostscript_from_rungs_marker() {
        let t = missing_tools(NO_GS, "", true);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, Kind::Tool);
        assert_eq!(t[0].file, "ghostscript");
        assert_eq!(t[0].lookup, Lookup::Absent);
        assert_eq!(t[0].needed_by, vec!["fig.eps"]);
        // 不是缺宏包，也不是缺字体
        assert!(missing_files(NO_GS).is_empty());
        assert!(missing_fonts(NO_GS).is_empty());
    }

    #[test]
    fn ghostscript_marker_dedup_and_paths() {
        let log = "subo-rungs: ghostscript not found: ./figs/a b.eps\n\
                   subo-rungs: ghostscript not found: ./figs/a b.eps\n\
                   (./x.aux)subo-rungs: ghostscript not found: c.ps\n";
        assert_eq!(missing_tools(log, "", false)[0].needed_by, vec!["figs/a b.eps", "c.ps"]);
    }

    /// 用户自己的 TeX Live：没有标记行，靠 xdvipdfmx 的报错加 main.log 里的图
    #[test]
    fn ghostscript_fallback_needs_all_three() {
        let t = missing_tools(NO_GS_PLAIN, EPS_TEXLOG, false);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].needed_by, vec!["fig.eps"]);
        // 本机有 gs：那是别的毛病（EPS 本身坏了等），不点名 gs
        assert!(missing_tools(NO_GS_PLAIN, EPS_TEXLOG, true).is_empty());
        // 没载入 eps：同一句报错也可能是别的原因
        assert!(missing_tools(NO_GS_PLAIN, "File: fig.pdf Graphic file (type pdf)", false).is_empty());
        // 编译日志里没有 xdvipdfmx 的报错
        assert!(missing_tools("! Undefined control sequence.", EPS_TEXLOG, false).is_empty());
    }

    const NO_GS_PDF: &str = include_str!("../testdata/missing-ghostscript-pdflatex.txt");
    const NO_GS_PDF_PLAIN: &str = include_str!("../testdata/missing-ghostscript-pdflatex-plain.txt");

    /// 内置 TeX 的 pdflatex：epstopdf.sh 的标记行在 stderr，接在 TeX 输出后面
    #[test]
    fn ghostscript_from_epstopdf_marker() {
        let t = missing_tools(NO_GS_PDF, "", true);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].file, "ghostscript");
        assert_eq!(t[0].needed_by, vec!["fig.eps"]);
        // pdftex.def 报的 fig-eps-converted-to.pdf 不当缺文件去查 tlmgr
        assert!(missing_files(NO_GS_PDF).is_empty());
    }

    /// 用户自己的 TeX Live：epstopdf.pl 没有标记行，从 pdftex.def 的报错反推原图，本机有 gs 时不点名
    #[test]
    fn ghostscript_fallback_pdflatex() {
        let t = missing_tools(NO_GS_PDF_PLAIN, "", false);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].needed_by, vec!["fig.eps"]);
        assert!(missing_files(NO_GS_PDF_PLAIN).is_empty());
        assert!(missing_tools(NO_GS_PDF_PLAIN, "", true).is_empty());
        // 报错折行、子目录、.ps
        let line = "./main.tex:9: Package pdftex.def Error: File `figs/very/long/name/of/a/figure/plot-ps-converted-to.pdf' not found: using draft setting.";
        let log = format!("{}\n{}\n", &line[..MAX_PRINT_LINE], &line[MAX_PRINT_LINE..]);
        assert_eq!(missing_tools(&log, "", false)[0].needed_by, vec!["figs/very/long/name/of/a/figure/plot.ps"]);
    }

    #[test]
    fn eps_source_names() {
        assert_eq!(eps_source("fig-eps-converted-to.pdf").as_deref(), Some("fig.eps"));
        assert_eq!(eps_source("a/b-ps-converted-to.pdf").as_deref(), Some("a/b.ps"));
        assert_eq!(eps_source("-eps-converted-to.pdf"), None);
        assert_eq!(eps_source("fig.pdf"), None);
    }

    #[test]
    fn tool_serializes_needed_by() {
        let v = serde_json::to_value(&missing_tools(NO_GS, "", true)[0]).unwrap();
        assert_eq!(v["kind"], "tool");
        assert_eq!(v["neededBy"][0], "fig.eps");
        let f = serde_json::to_value(&fonts(&["X".into()])[0]).unwrap();
        assert!(f.get("neededBy").is_none());
    }

    const STY: &str = include_str!("../testdata/missing-sty.txt");
    const TFM: &str = include_str!("../testdata/missing-tfm.txt");
    const INPUT: &str = include_str!("../testdata/missing-input.txt");

    #[test]
    fn latex_error_sty() {
        assert_eq!(missing_files(STY), vec!["xltxtra.sty"]);
    }

    #[test]
    fn tfm_wrapped_and_mf_echo_merged() {
        // 报错行在 79 列处折断；mktextfm 里还有一句 `I can't find file `phvr8t'`
        assert_eq!(missing_files(TFM), vec!["phvr8t.tfm"]);
    }

    #[test]
    fn project_input() {
        assert_eq!(missing_files(INPUT), vec!["chap1.tex"]);
    }

    #[test]
    fn luaotfload_font_file() {
        // jlreq + luatexja 在没装原ノ味时的报错，79 列处折断
        let log = "atches/lltjdefs.sty:69: Font \\JY3/mc/m/n/10=file:HaranoAjiMincho-Regular.otf:-k\n\
                   ern;jfm=jlreq at 9.62219pt not loadable: metric data not found or bad.\n";
        assert_eq!(missing_files(log), vec!["HaranoAjiMincho-Regular.otf"]);
        // 按名字找的系统字体不归这里
        let by_name = "! Font \\x=name:Hiragino Mincho ProN:+kern at 10pt not loadable: metric data not found or bad.\n";
        assert!(missing_files(by_name).is_empty());
    }

    #[test]
    fn tfm_without_file_line_error() {
        let log = "! Font \\XeTeXLink@font=phvr7t not loadable: Metric (TFM) file not found.\n";
        assert_eq!(missing_files(log), vec!["phvr7t.tfm"]);
    }

    #[test]
    fn long_name_split_across_lines() {
        let head = "! LaTeX Error: File `";
        let pad = MAX_PRINT_LINE - head.len();
        let name = format!("{}{}", "x".repeat(pad), "yz.sty");
        let log = format!("{head}{}\nyz.sty' not found.\n", &name[..pad]);
        assert_eq!(missing_files(&log), vec![name]);
    }

    #[test]
    fn dedup_and_reject_odd_names() {
        let log = "! LaTeX Error: File `a.sty' not found.\n\
                   ! LaTeX Error: File `a.sty' not found.\n\
                   ! LaTeX Error: File `../etc/passwd' not found.\n\
                   ! LaTeX Error: File `b c.sty' not found.\n";
        assert_eq!(missing_files(log), vec!["a.sty"]);
    }

    #[test]
    fn clean_log_has_nothing() {
        assert!(missing_files("Output written on a.pdf (1 page).\n").is_empty());
        assert!(missing_fonts("Output written on a.pdf (1 page).\n").is_empty());
    }

    const BST: &str = include_str!("../testdata/missing-bst.txt");
    const BBX_TEXLOG: &str = include_str!("../testdata/missing-bbx-texlog.txt");

    #[test]
    fn bibtex_style_file() {
        assert_eq!(missing_bib_styles(BST, ""), vec!["unsrturl.bst"]);
        // 不归 missing_files 管：编译成功时 run_job 不跑它
        assert!(missing_files(BST).is_empty());
    }

    #[test]
    fn biblatex_style_from_texlog() {
        assert_eq!(missing_bib_styles("", BBX_TEXLOG), vec!["ieee.bbx"]);
        let texlog = "Package biblatex Info: Trying to load citation style 'foo'...\n\
                      Package biblatex Info: ... file 'foo.cbx' not found.\n\
                      Package biblatex Info: ... file 'foo.cbx' not found.\n";
        assert_eq!(missing_bib_styles("", texlog), vec!["foo.cbx"]);
    }

    #[test]
    fn biblatex_found_and_lbx_ignored() {
        // 找到了的、语言模块（缺了只是警告，回落到默认）都不算
        let texlog = "Package biblatex Info: ... file 'numeric.bbx' found.\n\
                      Package biblatex Info: ... file 'english.lbx' not found.\n\
                      Package biblatex Info: ... file 'main.bbl' not found.\n";
        assert!(missing_bib_styles("", texlog).is_empty());
    }

    #[test]
    fn bibtex_style_without_extension_and_odd_names() {
        let log = "I couldn't open style file mystyle\n\
                   I couldn't open style file ../x.bst\n";
        assert_eq!(missing_bib_styles(log, ""), vec!["mystyle.bst"]);
        assert!(missing_bib_styles("Output written on a.pdf (1 page).\n", "").is_empty());
    }

    const CJKFONT: &str = include_str!("../testdata/missing-cjkfont.txt");
    const FONT_MKTEXTFM: &str = include_str!("../testdata/missing-font-mktextfm.txt");
    const FONT_WRAPPED: &str = include_str!("../testdata/missing-font-wrapped.txt");

    #[test]
    fn fontspec_font() {
        let log = "./main.tex:4: Package fontspec Error: \n\
                   (fontspec)                The font \"Nonexistent Font Pro\" cannot be found;\n\
                   (fontspec)                this may be but usually is not a fontspec bug.\n";
        assert_eq!(missing_fonts(log), vec!["Nonexistent Font Pro"]);
        // 字体报错不该混进文件清单
        assert!(missing_files(log).is_empty());
    }

    #[test]
    fn xecjk_font_deduped() {
        // 真实日志：\setCJKmainfont 找不到时 xeCJK 借 fontspec 报错，同一句会出现多次
        assert_eq!(missing_fonts(CJKFONT), vec!["FZShuSong-Z01 Not Here"]);
        assert!(missing_files(CJKFONT).is_empty());
    }

    #[test]
    fn xecjk_own_prefix_and_chinese_name() {
        let log = "! Package xeCJK Error: \n\
                   (xeCJK)                The font \"方正书宋不存在\" cannot be found; this may be\n\
                   (xeCJK)                but usually is not a fontspec bug.\n";
        assert_eq!(missing_fonts(log), vec!["方正书宋不存在"]);
    }

    #[test]
    fn font_name_wrapped_at_79() {
        // 真实日志：expl3 按词折行后，超长的那个词又被 79 列硬折了一刀
        let long = format!("Acme {}Sans Text", "Long".repeat(25));
        assert_eq!(missing_fonts(FONT_WRAPPED), vec![long]);
    }

    #[test]
    fn font_name_split_by_expl3_wrap() {
        let log = "(fontspec)                The font \"Source Han\n\
                   (fontspec)                Serif SC\" cannot be found; this may be\n";
        assert_eq!(missing_fonts(log), vec!["Source Han Serif SC"]);
    }

    #[test]
    fn xetex_primitive_font() {
        // 真实日志，79 列折在 `not loadabl|e`
        let log = "./main.tex:1: Font \\x=\"Nonexistent Sans:mapping=tex-text\" at 10.0pt not loadabl\n\
                   e: Metric (TFM) file or installed font not found.\n\
                   ./main.tex:2: Font \\y=[nofile.otf] at 10.0pt not loadable: Metric (TFM) file or\n \
                   installed font not found.\n";
        assert_eq!(missing_fonts(log), vec!["Nonexistent Sans"]);
        // 方括号里是文件名，照常问 tlmgr
        assert_eq!(missing_files(log), vec!["nofile.otf"]);
    }

    #[test]
    fn tlpdb_entries_runfiles_and_maps() {
        let db = "name 00texlive.installation\ncategory TLCore\n\n\
                  name lato\ncategory Package\nexecute addMap lato.map\nrunfiles size=3\n \
                  RELOC/fonts/truetype/typoland/lato/Lato-Regular.ttf\n \
                  fonts/map/dvips/lato/lato.map details=\"x\"\ncatalogue-version 3.3\n\n\
                  name times\nexecute addMixedMap utm.map\nrunfiles size=1\n tex/latex/times/times.sty\n";
        let e = parse_tlpdb(db);
        assert_eq!(e.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["lato", "times"]);
        assert_eq!(e[0].runfiles, ["fonts/truetype/typoland/lato/Lato-Regular.ttf", "fonts/map/dvips/lato/lato.map"]);
        assert_eq!(e[0].maps, ["lato.map"]);
        assert_eq!(e[1].maps, ["utm.map"]);
    }

    #[test]
    fn remove_user_file_stays_inside_home() {
        let d = std::env::temp_dir().join(format!("subo-rm-{}", std::process::id()));
        let home = d.join("home");
        fs::create_dir_all(home.join("tex/latex/foo")).unwrap();
        fs::write(home.join("tex/latex/foo/foo.sty"), "").unwrap();
        fs::write(d.join("outside"), "").unwrap();
        remove_user_file(&home, "../outside");
        assert!(d.join("outside").exists());
        remove_user_file(&home, "tex/latex/foo/foo.sty");
        assert!(!home.join("tex").exists() && home.is_dir());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn fonts_are_settled_absent() {
        let got = font_missing("Foo", &[]);
        assert_eq!(got.kind, Kind::Font);
        assert_eq!(got.lookup, Lookup::Absent);
        let json = serde_json::to_value(&got).unwrap();
        assert_eq!(json["kind"], "font");
        assert_eq!(json["file"], "Foo");
        assert!(json.get("fontFile").is_none());
    }

    #[test]
    fn mktextfm_echo_of_font_name_is_not_a_missing_file() {
        // 真实日志：系统 TeX Live 里 fontawesome 按名字加载失败，mktextfm 跟着回显了一句
        let fonts = missing_fonts(FONT_MKTEXTFM);
        assert_eq!(fonts, vec!["FontAwesome"]);
        let mut files = missing_files(FONT_MKTEXTFM);
        assert!(!files.is_empty(), "前提：不过滤时会被当成缺文件");
        files.push("FontAwesome.tfm".into());
        files.push("phvr8t.tfm".into());
        drop_font_echoes(&mut files, &fonts);
        assert_eq!(files, vec!["phvr8t.tfm"]);
    }

    #[test]
    fn font_file_candidates_cover_common_naming() {
        let c = font_file_candidates("TeX Gyre Pagella");
        assert_eq!(c[0], "TeXGyrePagella.otf");
        assert!(c.contains(&"TeXGyrePagella-Regular.otf".to_string()));
        assert!(c.contains(&"texgyrepagella-regular.otf".to_string()));
        assert!(c.contains(&"TeXGyrePagella.ttf".to_string()));
        // 中文名、带奇怪字符的不猜文件名
        assert!(font_file_candidates("方正书宋").is_empty());
        assert!(font_file_candidates("Foo/../Bar").is_empty());
    }

    #[test]
    fn font_shipped_by_texlive_is_found() {
        // 真实场景：fontawesome.sty 按名字加载 FontAwesome，TeX Live 带的是 FontAwesome.otf
        let got = font_missing("FontAwesome", &["FontAwesome.otf".into()]);
        assert_eq!(got.lookup, Lookup::Found);
        assert_eq!(got.font_file.as_deref(), Some("FontAwesome.otf"));
        assert_eq!(got.package, None, "不能让前端拿去 tlmgr install");
        let json = serde_json::to_value(&got).unwrap();
        assert_eq!(json["fontFile"], "FontAwesome.otf");
        assert_eq!(json["lookup"], "found");

        // 文件名大小写跟字体名不一致也认；kpsewhich 给的大小写原样保留
        let got = font_missing("TeX Gyre Pagella", &["texgyrepagella-regular.otf".into()]);
        assert_eq!(got.font_file.as_deref(), Some("texgyrepagella-regular.otf"));
        // 完全同名的文件优先于 `-Regular`
        let found = vec!["LibertinusSerif-Regular.otf".into(), "LibertinusSerif.otf".into()];
        assert_eq!(match_font_file("Libertinus Serif", &found).as_deref(), Some("LibertinusSerif.otf"));
        // 不相干的文件不算
        assert_eq!(font_missing("Lato", &["FontAwesome.otf".into()]).lookup, Lookup::Absent);
    }

    #[test]
    fn index_lookup_case_and_tex_suffix() {
        let d = std::env::temp_dir().join(format!("subo-case-idx-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let p = d.join("idx.txt");
        fs::write(&p, "arev\tams-mdbch.sty uzavm.fd\nfoo\tintro.tex\nmathdesign\tmdacmr.fd mdamdbch.fd\nother\tmdacmr.fd intro\n").unwrap();
        let files: Vec<String> = ["MDAcmr.fd", "intro", "main.aux"].iter().map(|s| s.to_string()).collect();
        let got = index_lookup(&p, &files).unwrap();
        assert_eq!(got["MDAcmr.fd"], ("mathdesign".to_string(), "mdacmr.fd".to_string()), "不分大小写，先出现的包");
        assert_eq!(got["intro"], ("other".to_string(), "intro".to_string()), "精确同名优先于补 .tex");
        assert!(!got.contains_key("main.aux"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn fonts_shipped_by_full_texlive_become_files() {
        let d = std::env::temp_dir().join(format!("subo-case-fontidx-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let p = d.join("idx.txt");
        fs::write(&p, "unfonts-core\tUnDotum.ttf UnBatang.ttf UnBatangBold.ttf\nlibertinus-otf\tLibertinusSerif-Regular.otf\n").unwrap();
        let got = fonts_in_index(&p, &["UnBatang", "Libertinus Serif", "Apple Myungjo", "나눔명조"]);
        assert_eq!(
            got,
            vec![
                ("UnBatang".to_string(), "UnBatang.ttf".to_string()),
                ("Libertinus Serif".to_string(), "LibertinusSerif-Regular.otf".to_string()),
            ],
            "系统字体和非 ASCII 名字不算"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn loose_candidates() {
        let log = "No file poster.aux.\nNo file MDAcmr.fd.\nPackage biblatex Info: ... file 'german.lbx' not found.\nNo file mdacmr.fd.\n\
                   ! LaTeX Error: File `foo.sty' not found.\n";
        assert_eq!(loose_files(log, &["foo.sty".to_string()], true), vec!["poster.aux", "MDAcmr.fd", "german.lbx"]);
        assert!(loose_files(log, &[], false).is_empty(), "编译成功时 NFSS 的回退不算");
    }

    #[test]
    fn driver_missing_files() {
        // 真实输出：映射到 mathdesign 的 Type1 字体而 mathdesign 没装
        let log = "xdvipdfmx:warning: Could not locate a virtual/physical font for TFM \"cmr10\".\n\
                   xdvipdfmx:warning: >> This font is mapped to a physical font \"mdbchr7m.pfb\".\n\
                   xdvipdfmx:warning: >> Please check if kpathsea library can find this font: mdbchr7m.pfb\n\
                   xdvipdfmx:fatal: Cannot proceed without .vf or \"physical\" font for PDF output...\n\
                   xdvipdfmx:warning: Could not open config file \"dvipdfmx.cfg\".\n\
                   xdvipdfmx:warning: Reading CMap file \"UniGB-UCS2-H\" failed.\n\
                   xdvipdfmx:warning: Unable to find TFM file \"ptmr8r\".\n\
                   xdvipdfmx:warning: Could not open OpenType/TrueType font file \"/x/y/FiraMath-Regular.otf\"\n";
        assert_eq!(
            loose_files(log, &[], false),
            vec!["mdbchr7m.pfb", "dvipdfmx.cfg", "UniGB-UCS2-H", "ptmr8r.tfm", "FiraMath-Regular.otf"]
        );
    }

    #[test]
    fn nfss_default_without_fd() {
        let log = "No file TUqhv.fd.\nLaTeX Font Info:    Trying to load font information for MDA+cmr on input line 9\n\
                   4.\nNo file MDAcmr.fd.\n\n./poster.tex:94: LaTeX Error: This NFSS system isn't set up properly.\n";
        assert_eq!(missing_files(log), vec!["mdacmr.fd"]);
        // 只有 No file、没有 NFSS 报错的是无害的回退，不算缺文件
        assert!(missing_files("No file TUqhv.fd.\n").is_empty());
    }

    #[test]
    fn located_in_project_subdir() {
        let paths: Vec<String> =
            ["main.tex", "a/b/foo.cls", "style/foo.cls", "style/myfoo.cls", "ch/intro.tex"].iter().map(|s| s.to_string()).collect();
        assert_eq!(locate(&paths, "foo.cls").as_deref(), Some("style/foo.cls"));
        assert_eq!(locate(&paths, "intro").as_deref(), Some("ch/intro.tex"));
        assert_eq!(locate(&paths, "oo.cls"), None);

        let d = std::env::temp_dir().join(format!("subo-case-proj-{}", std::process::id()));
        fs::create_dir_all(d.join("style")).unwrap();
        fs::write(d.join("style/tudelft.cls"), "").unwrap();
        let mut files = vec!["tudelft.cls".to_string(), "xltxtra.sty".to_string()];
        let hit = in_project(&d, &mut files);
        assert_eq!(files, vec!["xltxtra.sty"]);
        assert_eq!((hit[0].lookup.clone(), hit[0].project_path.as_deref()), (Lookup::Project, Some("style/tudelft.cls")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn pick_exact_basename() {
        // 真实输出：`xltxtra.sty` 子串还命中了 lwarp
        let out = "tlmgr: package repository https://mirrors.example/tlnet (not verified: gpg unavailable)\n\
                   lwarp:\n\ttexmf-dist/tex/latex/lwarp/lwarp-xltxtra.sty\n\
                   xltxtra:\n\ttexmf-dist/tex/xelatex/xltxtra/xltxtra.sty\n";
        assert_eq!(pick_package(out, "xltxtra.sty").as_deref(), Some("xltxtra"));
    }

    #[test]
    fn pick_tfm_and_none() {
        let out = "helvetic:\n\ttexmf-dist/fonts/tfm/adobe/helvetic/phvr8t.tfm\n\
                   \ttexmf-dist/fonts/vf/adobe/helvetic/phvr8t.vf\n\
                   tex4ht:\n\ttexmf-dist/tex4ht/ht-fonts/alias/adobe/helvetic/phvr8t.htf\n";
        assert_eq!(pick_package(out, "phvr8t.tfm").as_deref(), Some("helvetic"));
        assert_eq!(pick_package("", "nosuch.sty"), None);
    }

    #[test]
    fn doc_examples_do_not_count() {
        // 真实输出：项目自己的 chap1.tex 在 TeX Live 里只有示例文档同名
        let out = "hepthesis:\n\ttexmf-dist/doc/latex/hepthesis/example/chap1.tex\n\
                   pkuthss:\n\ttexmf-dist/doc/latex/pkuthss/example/chap/chap1.tex\n\
                   foo:\n\ttexmf-dist/source/latex/foo/chap1.tex\n";
        assert_eq!(pick_package(out, "chap1.tex"), None);
    }

    #[test]
    fn pattern_is_anchored() {
        assert_eq!(file_pattern("xltxtra.sty"), r"/xltxtra\.sty$");
        assert_eq!(file_pattern("chap1"), r"/chap1(\.tex)?$");
    }

    #[test]
    fn package_names() {
        assert!(valid_package("sourcesans"));
        assert!(valid_package("collection-fontsrecommended"));
        assert!(!valid_package("--all"));
        assert!(!valid_package("a; rm -rf ~"));
        assert!(!valid_package("a'b"));
    }

    #[test]
    fn cache_answers_offline() {
        let dir = std::env::temp_dir().join(format!("subo-case-{}", std::process::id()));
        let path = dir.join("file-index.json");
        let mut idx = Index::open(path.clone());
        idx.map.insert("xltxtra.sty".into(), Some("xltxtra".into()));
        idx.map.insert("chap1.tex".into(), None);
        idx.save();

        // 重新打开，只靠缓存作答，不碰 tlmgr
        let mut idx = Index::open(path);
        let got = resolve(&mut idx, &["xltxtra.sty".into(), "chap1.tex".into()], None, Fallback::Off);
        assert_eq!(got[0].package.as_deref(), Some("xltxtra"));
        assert_eq!(got[0].lookup, Lookup::Found);
        assert_eq!(got[1].lookup, Lookup::Absent);
        let _ = fs::remove_dir_all(dir);
    }
}
