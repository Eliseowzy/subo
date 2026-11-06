//! 按需安装的 TeX 引擎：XeTeX、pdfTeX 随包内置，LuaHBTeX、pTeX 等在设置里装和卸。
//!
//! 只管内置 TeX（包内只读）。引擎装进用户层旁边的「引擎根」`~/.subo/texlive/<年>/engine/`：
//! - `bin/<平台>/` 放二进制和链接（`lualatex → luahbtex`）；PATH 里排在内置 bin 之后
//! - 其余顶层条目（`texmf-dist`、`texmf.cnf`、`texmf-var`……）是指回内置树的符号链接。
//!   kpathsea 按二进制所在位置推出 SELFAUTOPARENT = 引擎根，经链接找到内置的配置和宏包
//! - 引擎自己的 runfiles 装进 TEXMFHOME，格式由 `fmtutil-user` 生成到 TEXMFVAR
//!
//! .app 挪了位置（换目录、更新）链接就悬空，所以每次启动按当前内置树重建（[`prepare`]）。
//! 每个装好的引擎在引擎根里留一份清单 `<id>.json`，卸载照它删，不碰别的东西。
//!
//! 引擎二进制必须与内置 TeX Live 同一年：2025 的 pdftex 读 2026 的格式会报
//! `Fatal format file error; I'm stymied`。下载前按软件源 tlpdb 的 `release/<年>` 挑源（跨年度后走归档源），
//! 装之前再看一眼二进制自报的 `(TeX Live <年>)`，离线导入的包也照此把关。

use crate::{case, engine, settings};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// 下载、解压、生成格式加起来的上限
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
/// 读软件源 tlpdb 开头几 KB 看年份
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Spec {
    pub id: &'static str,
    /// 显示名，专名不翻译
    pub name: &'static str,
    /// 这个引擎提供的 LaTeX 程序（工具栏里能选的）
    pub programs: &'static [&'static str],
    pub builtin: bool,
    /// 二进制容器 `<包>.<平台>.tar.xz` 和要从里面取的文件
    bins: &'static [(&'static str, &'static [&'static str])],
    /// `bin/<平台>/` 下的链接：(链接名, 指向)
    links: &'static [(&'static str, &'static str)],
    /// runfiles 容器 `<包>.tar.xz`，`texmf-dist/` 下的文件装进 TEXMFHOME
    runfiles: &'static [&'static str],
    /// 要生成的格式：(格式名, 引擎)，生成到 `TEXMFVAR/web2c/<引擎>/<格式>.fmt`
    formats: &'static [(&'static str, &'static str)],
    /// dvipdfmx 的日文字体映射，装完用 `updmap-user` 登记（tlpdb 里的 `execute addKanjiMap`）
    kanji_maps: &'static [&'static str],
    /// 下载体积（字节，容器 xz 之和，TL 2026 实量）：macOS、x86_64 Linux、aarch64 Linux
    download: [u64; 3],
}

pub const CATALOG: &[Spec] = &[
    Spec {
        id: "xetex",
        name: "XeTeX",
        programs: &["xelatex"],
        builtin: true,
        bins: &[],
        links: &[],
        runfiles: &[],
        formats: &[],
        kanji_maps: &[],
        download: [0; 3],
    },
    Spec {
        id: "pdftex",
        name: "pdfTeX",
        programs: &["pdflatex"],
        builtin: true,
        bins: &[],
        links: &[],
        runfiles: &[],
        formats: &[],
        kanji_maps: &[],
        download: [0; 3],
    },
    Spec {
        id: "luahbtex",
        name: "LuaHBTeX",
        programs: &["lualatex"],
        builtin: false,
        // luatex.<平台> 里的 luatex / texlua 只给 luaotfload-tool 之类的命令行工具用，编译用不上
        bins: &[("luahbtex", &["luahbtex"])],
        links: &[("lualatex", "luahbtex")],
        // luatex：luatex-unicode-letters.tex（格式要读）；luaotfload：fontspec 在 LuaTeX 下的字体加载器
        runfiles: &["luahbtex", "luatex", "luaotfload"],
        formats: &[("lualatex", "luahbtex")],
        kanji_maps: &[],
        download: [4_678_604 + 633_876, 2_799_272 + 633_876, 2_674_348 + 633_876],
    },
    Spec {
        id: "ptex",
        name: "pTeX / upTeX",
        programs: &["platex", "uplatex"],
        builtin: false,
        // fmtutil.cnf 里 platex、uplatex 都由 e-upTeX 跑
        bins: &[("uptex", &["euptex"])],
        links: &[("platex", "euptex"), ("uplatex", "euptex")],
        // ptex-fontmaps：dvipdfmx 把日文 TFM 对到实际字体的映射；adobemapping：映射要用的 CMap（UniJIS2004-UTF16-H……）
        runfiles: &[
            "ptex", "uptex", "ptex-base", "uptex-base", "ptex-fonts", "uptex-fonts", "platex", "uplatex", "ptex-fontmaps",
            "adobemapping",
        ],
        formats: &[("platex", "euptex"), ("uplatex", "euptex")],
        kanji_maps: &["ptex-@jaEmbed@@jaVariant@.map", "uptex-@jaEmbed@@jaVariant@.map"],
        download: [2_734_056 + 2_482_000, 1_416_956 + 2_482_000, 1_308_144 + 2_482_000],
    },
];

pub fn spec(id: &str) -> Option<&'static Spec> {
    CATALOG.iter().find(|s| s.id == id)
}

/// 提供这个 LaTeX 程序的引擎
pub fn for_program(program: &str) -> Option<&'static Spec> {
    CATALOG.iter().find(|s| s.programs.contains(&program))
}

/// 这个程序现在能不能跑：PATH 里有它（内置 bin、引擎根，或用户自己的 TeX）
pub fn available(program: &str) -> bool {
    engine::which(program).is_some()
}

impl Spec {
    /// 这台机器上要的容器文件名，离线导入时照这个清单找
    pub fn containers(&self, platform: &str) -> Vec<String> {
        let mut out: Vec<String> = self.bins.iter().map(|(p, _)| format!("{p}.{platform}.tar.xz")).collect();
        out.extend(self.runfiles.iter().map(|p| format!("{p}.tar.xz")));
        out
    }

    fn download_size(&self, platform: &str) -> u64 {
        match platform {
            p if p.contains("darwin") => self.download[0],
            "aarch64-linux" => self.download[2],
            _ => self.download[1],
        }
    }
}

/// 内置 TeX 的位置和用户层：都从环境里推，桌面壳（platform.rs）和 `--bundled-tex` 已经配好
struct Ctx {
    /// 内置 TeX Live 的根
    root: PathBuf,
    /// `universal-darwin`、`x86_64-linux`……
    platform: String,
    tlmgr: PathBuf,
    /// `~/.subo/texlive/<年>/engine`
    dir: PathBuf,
    home: PathBuf,
    var: PathBuf,
    year: u32,
}

impl Ctx {
    fn get() -> Result<Ctx, String> {
        let tlmgr = engine::which("tlmgr").ok_or("找不到 tlmgr")?;
        let bin = tlmgr.parent().and_then(|b| dunce::canonicalize(&b).ok()).ok_or("找不到 TeX Live 的 bin 目录")?;
        let platform = bin.file_name().ok_or("找不到 TeX Live 的平台目录")?.to_string_lossy().into_owned();
        let root = bin.parent().and_then(Path::parent).ok_or("找不到 TeX Live 的根目录")?.to_path_buf();
        let home = crate::tex::var("TEXMFHOME").map(PathBuf::from).ok_or("没有设置 TEXMFHOME")?;
        let var = crate::tex::var("TEXMFVAR").map(PathBuf::from).ok_or("没有设置 TEXMFVAR")?;
        let dir = home.parent().ok_or("TEXMFHOME 没有上级目录")?.join("engine");
        let year = release_year(&root).ok_or("读不出内置 TeX Live 的年份")?;
        Ok(Ctx { root, platform, tlmgr, dir, home, var, year })
    }

    fn bin(&self) -> PathBuf {
        self.dir.join("bin").join(&self.platform)
    }

    fn manifest(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// 离线导入的容器先放这儿，凑齐了再装
    fn staging(&self) -> PathBuf {
        self.dir.join("import")
    }
}

/// `release-texlive.txt` 第一行 `TeX Live (https://tug.org/texlive) version 2026`
fn release_year(root: &Path) -> Option<u32> {
    let text = fs::read_to_string(root.join("release-texlive.txt")).ok()?;
    text.lines().next()?.rsplit(' ').next()?.get(..4)?.parse().ok()
}

/// 装好的引擎留下的清单
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    year: u32,
    /// 引擎根 bin 下的二进制和链接（文件名）
    bins: Vec<String>,
    /// 新放进 TEXMFHOME 的 runfiles（绝对路径）；原先就有的不记，卸载时不删
    runfiles: Vec<PathBuf>,
    /// 生成的格式文件（绝对路径）
    formats: Vec<PathBuf>,
    /// 装好后占的空间（字节）
    bytes: u64,
    /// 生成格式时内置 xelatex.fmt 的戳：内置树换了（Subo 更新）格式要跟着重做
    base: String,
    /// 登记过日文字体映射，卸载时要撤掉
    #[serde(default)]
    kanji: bool,
}

fn load_manifest(ctx: &Ctx, id: &str) -> Option<Manifest> {
    serde_json::from_slice(&fs::read(ctx.manifest(id)).ok()?).ok()
}

fn installed(ctx: &Ctx, spec: &Spec) -> Option<Manifest> {
    let m = load_manifest(ctx, spec.id)?;
    (m.year == ctx.year && m.bins.iter().all(|b| ctx.bin().join(b).exists())).then_some(m)
}

/// 内置格式的戳：大小 + 修改时间
fn base_stamp(root: &Path) -> String {
    fs::metadata(root.join("texmf-var/web2c/xetex/xelatex.fmt"))
        .map(|m| {
            let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            format!("{}-{t}", m.len())
        })
        .unwrap_or_default()
}

/// 换到内置 TeX 时调用（含启动时）：重建引擎根的链接。引擎根的 bin 已由 [`crate::tex`] 接在内置 bin 后面。
/// 返回内置树换过、格式要重做的引擎，交给 [`refresh_formats`] 在后台做。
pub fn prepare() -> Vec<&'static str> {
    let ctx = match Ctx::get() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[engines] 跳过引擎根：{e}");
            return Vec::new();
        }
    };
    if let Err(e) = relink(&ctx) {
        eprintln!("[engines] 重建引擎根的链接失败：{e}");
    }
    let base = base_stamp(&ctx.root);
    CATALOG
        .iter()
        .filter(|s| installed(&ctx, s).is_some_and(|m| m.base != base))
        .map(|s| s.id)
        .collect()
}

/// 引擎根顶层除 `bin` 外都链回内置树。已有的链接先删掉重建：内置树可能换了位置
fn relink(ctx: &Ctx) -> std::io::Result<()> {
    fs::create_dir_all(ctx.bin())?;
    for e in fs::read_dir(&ctx.dir)?.flatten() {
        if e.file_type().is_ok_and(|t| t.is_symlink()) {
            fs::remove_file(e.path())?;
        }
    }
    for e in fs::read_dir(&ctx.root)?.flatten() {
        let name = e.file_name();
        if name == "bin" || ctx.dir.join(&name).exists() {
            continue;
        }
        symlink(&e.path(), &ctx.dir.join(&name))?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("这个系统上还没有接入引擎根"))
}

/// 内置树换过之后重新生成已装引擎的格式
pub fn refresh_formats(ids: &[&str]) {
    let Ok(ctx) = Ctx::get() else { return };
    let deadline = Instant::now() + INSTALL_TIMEOUT;
    for id in ids {
        let (Some(spec), Some(mut m)) = (spec(id), load_manifest(&ctx, id)) else { continue };
        match make_formats(&ctx, spec, deadline) {
            Ok(fmts) => {
                m.formats = fmts;
                m.base = base_stamp(&ctx.root);
                let _ = save_manifest(&ctx, spec.id, &m);
                eprintln!("[engines] 内置 TeX 换过，已重新生成 {} 的格式", spec.name);
            }
            Err(e) => eprintln!("[engines] 重新生成 {} 的格式失败：{}", spec.name, e.detail),
        }
    }
}

/// 设置页的一行
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: &'static str,
    pub name: &'static str,
    pub programs: &'static [&'static str],
    pub builtin: bool,
    pub installed: bool,
    /// 装好的占用；没装时是要下载的体积。内置的为 0
    pub bytes: u64,
    /// 离线导入要的容器文件名
    pub containers: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    /// 内置 TeX：Subo 管引擎；用户自己的 TeX：只报哪些程序在，装卸由 tlmgr 管
    pub managed: bool,
    /// TeX Live 的年份：内置的读 release-texlive.txt，用户自己的问 tlmgr
    pub year: Option<u32>,
    /// 这次运行中主软件源已跨年、改用的归档源：说明这一年的版本确实冻结了
    pub archive: Option<String>,
    pub platform: Option<String>,
    pub engines: Vec<Status>,
    /// 已经放进导入区的容器
    pub staged: Vec<String>,
}

pub fn overview(managed: bool) -> Overview {
    let ctx = if managed { Ctx::get().ok() } else { None };
    let engines = CATALOG
        .iter()
        .map(|s| {
            let (installed, bytes) = match &ctx {
                _ if s.builtin && managed => (true, 0),
                Some(c) => match installed(c, s) {
                    Some(m) => (true, m.bytes),
                    None => (false, s.download_size(&c.platform)),
                },
                None => (s.programs.iter().all(|p| available(p)), 0),
            };
            Status {
                id: s.id,
                name: s.name,
                programs: s.programs,
                builtin: s.builtin,
                installed,
                bytes,
                containers: ctx.as_ref().map(|c| s.containers(&c.platform)).unwrap_or_default(),
            }
        })
        .collect();
    let staged = ctx
        .as_ref()
        .and_then(|c| fs::read_dir(c.staging()).ok())
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>())
        .map(|mut v| {
            v.sort();
            v
        })
        .unwrap_or_default();
    Overview {
        managed,
        year: ctx.as_ref().map(|c| c.year).or_else(crate::case::local_year),
        archive: crate::case::archive_in_use(),
        platform: ctx.as_ref().map(|c| c.platform.clone()),
        engines,
        staged,
    }
}

/// 容器从哪儿来
pub enum Source<'a> {
    /// 设置里的安装源（`None` 用 tlmgr 自己配的）
    Repo { repo: Option<&'a str>, fallback: case::Fallback },
    /// 离线导入：之前放进导入区的文件
    Import,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub id: &'static str,
    pub bytes: u64,
    /// 主软件源已跨年度，这次改用了这个归档源
    pub archive: Option<String>,
}

/// 安装失败：`code` 对应前端的 `server.<code>`，`params` 填进文案，`detail` 是给人看的日志
#[derive(Debug)]
pub struct Failed {
    pub code: &'static str,
    pub params: Vec<(&'static str, String)>,
    pub detail: String,
}

impl Failed {
    fn new(code: &'static str, detail: impl Into<String>) -> Failed {
        Failed { code, params: Vec::new(), detail: detail.into() }
    }

    fn with(mut self, k: &'static str, v: impl ToString) -> Failed {
        self.params.push((k, v.to_string()));
        self
    }
}

impl From<String> for Failed {
    fn from(detail: String) -> Failed {
        Failed::new("engines.installFailed", detail)
    }
}

impl From<&str> for Failed {
    fn from(detail: &str) -> Failed {
        Failed::new("engines.installFailed", detail)
    }
}

pub fn install(id: &str, source: Source) -> Result<Installed, Failed> {
    let spec = spec(id).filter(|s| !s.builtin).ok_or_else(|| Failed::new("engines.unknown", id).with("engine", id))?;
    let ctx = Ctx::get()?;
    let deadline = Instant::now() + INSTALL_TIMEOUT;
    let tmp = std::env::temp_dir().join(format!("subo-engine-{}-{}", std::process::id(), spec.id));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("x")).map_err(|e| format!("无法创建临时目录：{e}"))?;
    let res = (|| {
        let names = spec.containers(&ctx.platform);
        let archive = match source {
            Source::Repo { repo, fallback } => {
                let (base, archive) = pick_repo(&ctx, repo, fallback)?;
                for n in &names {
                    fetch(&base, n, &tmp.join(n), deadline)?;
                }
                archive
            }
            Source::Import => {
                let missing: Vec<&String> = names.iter().filter(|n| !ctx.staging().join(n).is_file()).collect();
                if !missing.is_empty() {
                    let list = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
                    return Err(Failed::new("engines.importMissing", list.clone()).with("files", list));
                }
                for n in &names {
                    fs::copy(ctx.staging().join(n), tmp.join(n)).map_err(|e| format!("读取 {n} 失败：{e}"))?;
                }
                None
            }
        };
        // 每个容器解进自己的目录：可重定位的包（ptex-fonts……）容器里没有 `texmf-dist/` 这层，
        // 要分开才认得出根在哪儿
        for n in &names {
            let dir = tmp.join("x").join(n.trim_end_matches(".tar.xz"));
            fs::create_dir_all(&dir).map_err(|e| format!("无法创建临时目录：{e}"))?;
            case::unpack(&ctx.tlmgr, &tmp.join(n), &dir, deadline)?;
        }
        check_year(&ctx, spec, &tmp.join("x"), deadline)?;
        let mut m = Manifest { year: ctx.year, base: base_stamp(&ctx.root), ..Default::default() };
        if let Err(e) = place(&ctx, spec, &tmp.join("x"), &mut m, deadline) {
            remove(&ctx, &m);
            return Err(e);
        }
        save_manifest(&ctx, spec.id, &m).map_err(|e| format!("写入清单失败：{e}"))?;
        Ok(Installed { id: spec.id, bytes: m.bytes, archive })
    })();
    let _ = fs::remove_dir_all(&tmp);
    if res.is_ok() {
        if let Source::Import = source {
            let _ = fs::remove_dir_all(ctx.staging());
        }
    }
    res
}

/// 解好的容器（`x/<容器名>/`）放到位：runfiles 进 TEXMFHOME，二进制进引擎根，再生成格式。
/// 一边放一边记进 `m`，中途失败时调用方照它回滚
fn place(ctx: &Ctx, spec: &Spec, x: &Path, m: &mut Manifest, deadline: Instant) -> Result<(), Failed> {
    let mut n = 0;
    for pkg in spec.runfiles {
        let dir = x.join(pkg);
        let src = if dir.join("texmf-dist").is_dir() { dir.join("texmf-dist") } else { dir };
        case::copy_runfiles(&src, &src, &ctx.home, &mut n, &mut m.runfiles)
            .map_err(|e| format!("写入 {} 失败：{e}", ctx.home.display()))?;
    }
    relink(ctx).map_err(|e| format!("建引擎根失败：{e}"))?;
    let bin = ctx.bin();
    for (pkg, files) in spec.bins {
        for f in *files {
            let from = x.join(format!("{pkg}.{}", ctx.platform)).join("bin").join(&ctx.platform).join(f);
            if !from.is_file() {
                return Err(format!("{pkg}.{}.tar.xz 里没有 {f}", ctx.platform).into());
            }
            let to = bin.join(f);
            let _ = fs::remove_file(&to);
            m.bins.push(f.to_string());
            thin(&from, &to).map_err(|e| format!("放置 {f} 失败：{e}"))?;
        }
    }
    for (link, target) in spec.links {
        let to = bin.join(link);
        let _ = fs::remove_file(&to);
        m.bins.push(link.to_string());
        symlink(Path::new(target), &to).map_err(|e| format!("建链接 {link} 失败：{e}"))?;
    }
    refresh_lsr(&ctx.home, deadline);
    m.formats = make_formats(ctx, spec, deadline)?;
    if !spec.kanji_maps.is_empty() {
        m.kanji = true;
        kanji_maps(spec, true, deadline)?;
    }
    m.bytes = m.bins.iter().map(|b| bin.join(b)).chain(m.runfiles.iter().cloned()).chain(m.formats.iter().cloned()).map(|p| file_len(&p)).sum();
    Ok(())
}

fn file_len(p: &Path) -> u64 {
    fs::symlink_metadata(p).map(|m| if m.is_file() { m.len() } else { 0 }).unwrap_or(0)
}

/// macOS 的二进制是 universal 的，`lipo -thin` 只留本机架构，体积减半；TL 的签名仍然有效（实测）。
/// 没有 lipo（没装命令行工具时 /usr/bin/lipo 只是个壳）就原样拷
fn thin(from: &Path, to: &Path) -> std::io::Result<()> {
    if cfg!(target_os = "macos") {
        let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
        let ok = crate::engine::command("lipo")
            .arg(from)
            .args(["-thin", arch, "-output"])
            .arg(to)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok && to.is_file() {
            return set_exec(to);
        }
        let _ = fs::remove_file(to);
    }
    fs::copy(from, to)?;
    set_exec(to)
}

#[cfg(unix)]
fn set_exec(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_exec(_p: &Path) -> std::io::Result<()> {
    Ok(())
}

/// 用户层有 ls-R 就刷新：kpathsea 先查 ls-R，旧索引会让新文件找不到
fn refresh_lsr(home: &Path, deadline: Instant) {
    if home.join("ls-R").is_file() {
        if let Some(mktexlsr) = engine::which("mktexlsr") {
            let _ = engine::run(crate::engine::command(mktexlsr).arg(home), deadline);
        }
    }
}

/// `fmtutil-user --byfmt <格式>`，返回生成的格式文件
fn make_formats(ctx: &Ctx, spec: &Spec, deadline: Instant) -> Result<Vec<PathBuf>, Failed> {
    let fmtutil = engine::which("fmtutil-user").ok_or("找不到 fmtutil-user")?;
    let mut out = Vec::new();
    for (fmt, eng) in spec.formats {
        let ran = engine::run(crate::engine::command(&fmtutil).args(["--byfmt", fmt]).current_dir(std::env::temp_dir()), deadline)
            .map_err(|e| format!("无法运行 fmtutil-user：{e}"))?;
        let file = ctx.var.join("web2c").join(eng).join(format!("{fmt}.fmt"));
        if !ran.ok || !file.is_file() {
            return Err(Failed::new("engines.formatFailed", case::tail(&ran.log, 30)).with("format", fmt));
        }
        out.push(file);
    }
    Ok(out)
}

/// 登记 / 撤掉 dvipdfmx 的日文字体映射。默认的 jaEmbed 是原ノ味（haranoaji，26 MB，没内置）：
/// 用户层也没有时改成不嵌入（noEmbed），PDF 引用阅读器自带的明朝、黑体，先能出 PDF；
/// 以后装上原ノ味，[`case::register_user_maps`] 会切回嵌入
fn kanji_maps(spec: &Spec, enable: bool, deadline: Instant) -> Result<(), Failed> {
    let updmap = engine::which("updmap-user").ok_or("找不到 updmap-user")?;
    let run = |args: &[&str]| -> Result<(), Failed> {
        let ran = engine::run(crate::engine::command(&updmap).args(args), deadline).map_err(|e| format!("无法运行 updmap-user：{e}"))?;
        if ran.ok { Ok(()) } else { Err(format!("updmap-user {} 失败：{}", args.join(" "), case::tail(&ran.log, 10)).into()) }
    };
    for map in spec.kanji_maps {
        if enable {
            run(&["--nomkmap", "--enable", &format!("KanjiMap={map}")])?;
        } else {
            run(&["--nomkmap", "--disable", map])?;
        }
    }
    if enable && engine::which("kpsewhich").is_some_and(|k| {
        !engine::run(crate::engine::command(k).arg("HaranoAjiMincho-Regular.otf"), deadline).is_ok_and(|r| r.ok)
    }) {
        run(&["--nomkmap", "--setoption", "jaEmbed", "noEmbed"])?;
    }
    run(&[])
}

/// 年份对不上就不装：先看二进制自报的 `(TeX Live 2026)`
fn check_year(ctx: &Ctx, spec: &Spec, x: &Path, deadline: Instant) -> Result<(), Failed> {
    for (pkg, files) in spec.bins {
        let Some(f) = files.first() else { continue };
        let exe = x.join(format!("{pkg}.{}", ctx.platform)).join("bin").join(&ctx.platform).join(f);
        let ran = engine::run(crate::engine::command(&exe).arg("--version"), deadline).map_err(|e| format!("无法运行 {f}：{e}"))?;
        match banner_year(&ran.log) {
            Some(y) if y == ctx.year => {}
            Some(y) => {
                return Err(Failed::new("engines.yearMismatch", format!("{f}: TeX Live {y}"))
                    .with("got", y)
                    .with("want", ctx.year))
            }
            None => return Err(format!("{f} 不认 --version：{}", case::tail(&ran.log, 5)).into()),
        }
    }
    Ok(())
}

/// `This is LuaHBTeX, Version 1.24.0 (TeX Live 2026)` → 2026
fn banner_year(out: &str) -> Option<u32> {
    let i = out.find("(TeX Live ")?;
    out[i + "(TeX Live ".len()..].get(..4)?.parse().ok()
}

/// 挑下载容器用的源，返回 `(源, 实际用了的归档源)`。软件源的年份比内置的新就换本年的 tlnet-final
/// （用户自己填的源不替他换），比内置的旧（镜像没同步）直接报错；读不出年份照用，装前还会再查二进制
fn pick_repo(ctx: &Ctx, repo: Option<&str>, fallback: case::Fallback) -> Result<(String, Option<String>), Failed> {
    let base = repo
        .map(str::to_string)
        .unwrap_or_else(|| case::default_repository(&ctx.tlmgr, Instant::now() + PROBE_TIMEOUT));
    let base = base.trim_end_matches('/').to_string();
    let year = repo_year(&base);
    match year {
        None => Ok((base, None)),
        Some(y) if y == ctx.year => Ok((base, None)),
        Some(y) if y > ctx.year && fallback == case::Fallback::Auto => {
            for a in case::archive_repos(ctx.year, settings::is_cn(&base)) {
                if repo_year(&a) == Some(ctx.year) {
                    eprintln!("[engines] 软件源已是 TeX Live {y}，本机是 {}：改用归档源 {a}", ctx.year);
                    return Ok((a.clone(), Some(a)));
                }
            }
            Err(Failed::new("engines.archiveFailed", base).with("year", y).with("want", ctx.year))
        }
        Some(y) => Err(Failed::new("engines.repoYear", base).with("year", y).with("want", ctx.year)),
    }
}

/// 软件源 tlpdb 第一条 `00texlive.config` 里的 `depend release/2026`。只取开头 8 KB
fn repo_year(base: &str) -> Option<u32> {
    let head = if base.starts_with("http://") || base.starts_with("https://") {
        let curl = engine::which("curl")?;
        let url = format!("{base}/tlpkg/texlive.tlpdb");
        let ran = engine::run(
            crate::engine::command(curl).args(["-fsSL", "--connect-timeout", "10", "-r", "0-8191"]).arg(&url),
            Instant::now() + PROBE_TIMEOUT,
        )
        .ok()?;
        ran.log
    } else {
        let mut buf = vec![0u8; 8192];
        let n = std::io::Read::read(&mut fs::File::open(Path::new(base).join("tlpkg/texlive.tlpdb")).ok()?, &mut buf).ok()?;
        String::from_utf8_lossy(&buf[..n]).into_owned()
    };
    tlpdb_release(&head)
}

fn tlpdb_release(head: &str) -> Option<u32> {
    head.lines().find_map(|l| l.trim().strip_prefix("depend release/")?.get(..4)?.parse().ok())
}

/// 下载（或从本机的 tlnet 目录拷）`archive/<name>`
fn fetch(base: &str, name: &str, to: &Path, deadline: Instant) -> Result<(), Failed> {
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        let from = Path::new(base).join("archive").join(name);
        return fs::copy(&from, to).map(|_| ()).map_err(|e| format!("读取 {} 失败：{e}", from.display()).into());
    }
    let curl = engine::which("curl").ok_or("找不到 curl，无法下载")?;
    let url = format!("{base}/archive/{name}");
    let got = engine::run(
        crate::engine::command(curl).args(["-fsSL", "--retry", "2", "--connect-timeout", "20", "-o"]).arg(to).arg(&url),
        deadline,
    )
    .map_err(|e| format!("无法运行 curl：{e}"))?;
    if !got.ok || !to.is_file() {
        return Err(Failed::new("engines.downloadFailed", case::tail(&got.log, 5)).with("url", url));
    }
    Ok(())
}

fn save_manifest(ctx: &Ctx, id: &str, m: &Manifest) -> std::io::Result<()> {
    let f = ctx.manifest(id);
    let tmp = f.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(m).map_err(std::io::Error::other)?)?;
    fs::rename(&tmp, &f)
}

/// 删清单里记的东西：二进制、链接、新放的 runfiles、格式
fn remove(ctx: &Ctx, m: &Manifest) {
    for b in &m.bins {
        let _ = fs::remove_file(ctx.bin().join(b));
    }
    for p in m.runfiles.iter().chain(m.formats.iter()) {
        let _ = fs::remove_file(p);
        // 顺手删掉变空的上级目录，到用户层根为止
        let mut dir = p.parent();
        while let Some(d) = dir.filter(|d| d.starts_with(&ctx.home) || d.starts_with(&ctx.var)) {
            if d == ctx.home || d == ctx.var || fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
}

pub fn uninstall(id: &str) -> Result<(), Failed> {
    let spec = spec(id).ok_or_else(|| Failed::new("engines.unknown", id).with("engine", id))?;
    if spec.builtin {
        return Err(Failed::new("engines.builtin", id).with("engine", spec.name));
    }
    let ctx = Ctx::get()?;
    let Some(m) = load_manifest(&ctx, id) else { return Ok(()) };
    if m.kanji {
        // 撤不掉也接着删：映射指向的 TFM 没了，dvipdfmx 只是用不上这几行
        if let Err(e) = kanji_maps(spec, false, Instant::now() + INSTALL_TIMEOUT) {
            eprintln!("[engines] 撤销 {} 的日文字体映射失败：{}", spec.name, e.detail);
        }
    }
    remove(&ctx, &m);
    let _ = fs::remove_file(ctx.manifest(id));
    refresh_lsr(&ctx.home, Instant::now() + PROBE_TIMEOUT);
    Ok(())
}

/// 离线导入：把一个容器放进导入区。文件名必须是某个引擎要的容器，免得什么都往里塞
pub fn stage(name: &str, body: &[u8]) -> Result<Vec<&'static str>, Failed> {
    let ctx = Ctx::get()?;
    let wanted: Vec<&'static Spec> =
        CATALOG.iter().filter(|s| s.containers(&ctx.platform).iter().any(|c| c == name)).collect();
    if wanted.is_empty() {
        return Err(Failed::new("engines.importUnknown", name).with("file", name));
    }
    fs::create_dir_all(ctx.staging()).map_err(|e| format!("无法创建导入目录：{e}"))?;
    fs::write(ctx.staging().join(name), body).map_err(|e| format!("保存 {name} 失败：{e}"))?;
    Ok(wanted.iter().map(|s| s.id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_from_banner_and_tlpdb() {
        assert_eq!(banner_year("This is LuaHBTeX, Version 1.24.0 (TeX Live 2026)\n"), Some(2026));
        assert_eq!(banner_year("e-upTeX 3.141592653-p4.1.2-u2.02-250303 (utf8.uptex) (TeX Live 2025)"), Some(2025));
        assert_eq!(banner_year("nothing"), None);
        let head = "name 00texlive.config\nlongdesc release/YYYY specifies\ndepend frozen/0\ndepend minrelease/2016\ndepend release/2026\n";
        assert_eq!(tlpdb_release(head), Some(2026));
        assert_eq!(tlpdb_release("name x\n"), None);
    }

    #[test]
    fn containers_per_platform() {
        let lua = spec("luahbtex").unwrap();
        assert_eq!(
            lua.containers("universal-darwin"),
            vec!["luahbtex.universal-darwin.tar.xz", "luahbtex.tar.xz", "luatex.tar.xz", "luaotfload.tar.xz"]
        );
        assert_eq!(for_program("lualatex").map(|s| s.id), Some("luahbtex"));
        assert_eq!(for_program("uplatex").map(|s| s.id), Some("ptex"));
        assert!(for_program("pdflatex").unwrap().builtin);
        assert!(lua.download_size("x86_64-linux") < lua.download_size("universal-darwin"));
    }

    #[test]
    fn relink_and_remove() {
        let tmp = std::env::temp_dir().join(format!("subo-engines-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let root = tmp.join("tl");
        for d in ["bin/universal-darwin", "texmf-dist/tex", "texmf-var"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(root.join("texmf.cnf"), "x").unwrap();
        let ctx = Ctx {
            root: root.clone(),
            platform: "universal-darwin".into(),
            tlmgr: root.join("bin/universal-darwin/tlmgr"),
            dir: tmp.join("layer/engine"),
            home: tmp.join("layer/texmf"),
            var: tmp.join("layer/texmf-var"),
            year: 2026,
        };
        fs::create_dir_all(&ctx.dir).unwrap();
        // 上次启动时的内置树已经不在了：悬空的链接要换掉
        symlink(&tmp.join("gone/texmf-dist"), &ctx.dir.join("texmf-dist")).unwrap();
        relink(&ctx).unwrap();
        assert!(ctx.bin().is_dir());
        assert_eq!(fs::read_link(ctx.dir.join("texmf-dist")).unwrap(), root.join("texmf-dist"));
        assert!(ctx.dir.join("texmf.cnf").is_file());
        assert!(!ctx.dir.join("bin").is_symlink());

        let f = ctx.home.join("tex/luatex/luaotfload/x.lua");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, "x").unwrap();
        fs::write(ctx.bin().join("luahbtex"), "x").unwrap();
        let m = Manifest { year: 2026, bins: vec!["luahbtex".into()], runfiles: vec![f.clone()], ..Default::default() };
        remove(&ctx, &m);
        assert!(!f.exists() && !ctx.home.join("tex").exists());
        assert!(ctx.home.is_dir());
        assert!(!ctx.bin().join("luahbtex").exists());
        let _ = fs::remove_dir_all(&tmp);
    }
}
