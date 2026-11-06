//! 用哪一套 TeX：用户自己装的，还是随应用内置的 TeX Live。运行中可以换，不用重启。
//!
//! 不改本进程的环境变量：当前那套 TeX 的 `PATH`、`TEXMFHOME`… 记在这里，
//! 起子进程（[`crate::engine::command`]）、找程序（[`crate::engine::which`]）、读 TEXMF 变量时都问它。
//!
//! 默认（[`Choice::Auto`]）用户自己的 TeX 带齐 xelatex、pdflatex 且 LaTeX 内核不早于 [`FLOOR`] 就用它，
//! 否则用内置的。内核太旧时新模板、按需新装的宏包会报 `expl3 too old` 之类，用户自己又不好升级
//! （TeX Live 跨年要整个重装、Linux 发行版的版本跟着系统走）。

use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, OnceLock, RwLock},
    time::{Duration, Instant},
};

/// LaTeX 内核早于这一天就算太旧：TeX Live 2023 带的内核，Ubuntu 24.04 起的发行版也有
pub const FLOOR: &str = "2023-06-01";

/// 设置里选的 TeX
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    /// 自己的 TeX 能用、不太旧就用它，否则用内置的
    #[default]
    Auto,
    /// 用户自己装的
    Own,
    /// 随应用内置的
    Bundled,
}

impl Choice {
    pub fn parse(s: &str) -> Option<Choice> {
        match s {
            "auto" => Some(Choice::Auto),
            "own" => Some(Choice::Own),
            "bundled" => Some(Choice::Bundled),
            _ => None,
        }
    }
}

/// 用户自己的 TeX 是哪一种：决定版本对不上时给什么修法
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// 滚动更新，缺的宏包当场装最新版：内核不跟着更新就会新旧混装。更新一次即可
    Miktex,
    /// 官方安装器装的 TeX Live：按年份冻结，升级要装新一年的
    Texlive,
    /// Linux 发行版打包的 TeX Live：版本跟着系统走，tlmgr 也不能用
    Distro,
    Other,
}

/// 用户自己的 TeX（PATH 里找到的）
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Own {
    pub kind: Kind,
    /// LaTeX 内核日期 `2021-11-15`，读 `latex.ltx` 得来；读不到为 `None`
    pub kernel: Option<String>,
    /// 内核早于 [`FLOOR`]
    pub old: bool,
    /// xelatex 所在目录
    pub bin: String,
}

/// 随应用内置的 TeX Live
struct Bundled {
    year: Option<u32>,
    env: Env,
}

/// 启动时探测到的两套 TeX
struct Setup {
    /// 更新 MiKTeX 后内核变了，重读
    own: RwLock<Option<Own>>,
    bundled: Option<Bundled>,
    /// `SUBO_TEX=bundled`：在装了 TeX 的机器上测内置版
    forced: bool,
}

/// 一套 TeX 要给子进程的环境；`path` 为 `None` 用本进程的 PATH
#[derive(Clone, Debug, Default)]
struct Env {
    bundled: bool,
    path: Option<OsString>,
    vars: Vec<(&'static str, OsString)>,
}

static SETUP: OnceLock<Setup> = OnceLock::new();
static CURRENT: RwLock<Option<Arc<Env>>> = RwLock::new(None);

/// 桌面壳启动时调：`bundled_root` 是内置 TeX Live 的根目录（没打进包时为 `None`）。
/// 按设置选好一套，返回是否用了内置的
pub fn init(bundled_root: Option<PathBuf>) -> bool {
    let base = std::env::var_os("PATH").unwrap_or_default();
    let bundled = bundled_root.map(|root| {
        let year = release_year(&root);
        let layer = user_layer(&crate::library::subo_dir().join("texlive"), year);
        Bundled { year, env: bundled_env(&root, &layer, &base) }
    });
    let setup = Setup {
        own: RwLock::new(detect_own(&base)),
        bundled,
        forced: std::env::var("SUBO_TEX").is_ok_and(|v| v == "bundled"),
    };
    let choice = crate::settings::Settings::load(&crate::library::subo_dir().join("settings.json")).tex;
    let env = pick(&setup, choice);
    let bundled = env.bundled;
    *CURRENT.write().unwrap() = Some(Arc::new(env));
    let _ = SETUP.set(setup);
    bundled
}

/// 单独跑 `subo-server` 时：环境由调用方配好（`--bundled-tex` 时 PATH、TEXMFHOME 已指向内置版），不能换
pub fn init_external(bundled: bool) {
    let mut env = Env { bundled, ..Default::default() };
    if bundled {
        // 按需安装的引擎（LuaTeX、pTeX）的 bin 接在内置 bin 后面，字体缓存写进用户层
        let base = std::env::var_os("PATH").unwrap_or_default();
        let tlmgr = which_in(&base, "tlmgr").and_then(|t| dunce::canonicalize(t.parent()?).ok());
        let home = std::env::var_os("TEXMFHOME").map(PathBuf::from);
        if let (Some(bin), Some(home)) = (tlmgr, home) {
            if let (Some(platform), Some(layer)) = (bin.file_name(), home.parent()) {
                env.path = join_path(&[bin.clone(), layer.join("engine/bin").join(platform)], &base);
            }
        }
        if std::env::var_os("TEXMFCACHE").is_none() {
            if let Some(v) = std::env::var_os("TEXMFVAR") {
                env.vars.push(("TEXMFCACHE", v));
            }
        }
    }
    *CURRENT.write().unwrap() = Some(Arc::new(env));
}

fn pick(setup: &Setup, choice: Choice) -> Env {
    let own = || Env::default();
    let Some(b) = &setup.bundled else { return own() };
    let use_bundled = setup.forced
        || match choice {
            Choice::Bundled => true,
            Choice::Own => setup.own.read().unwrap().is_none(),
            Choice::Auto => setup.own.read().unwrap().as_ref().is_none_or(|o| o.old),
        };
    if use_bundled {
        b.env.clone()
    } else {
        own()
    }
}

/// 已经选过 TeX（桌面壳调过 [`init`]）
pub fn initialized() -> bool {
    CURRENT.read().unwrap().is_some()
}

fn current() -> Arc<Env> {
    CURRENT.read().unwrap().clone().unwrap_or_default()
}

/// 当前用的是内置 TeX Live：包内只读，补装宏包走 `tlmgr --usermode` 装进用户层
pub fn bundled() -> bool {
    current().bundled
}

/// 当前这套 TeX 的 PATH
pub fn path() -> Option<OsString> {
    current().path.clone().or_else(|| std::env::var_os("PATH"))
}

/// 当前这套 TeX 的环境变量（`TEXMFHOME` 之类）：内置版自己设的优先，其次本进程的
pub fn var(name: &str) -> Option<OsString> {
    let env = current();
    env.vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone()).or_else(|| std::env::var_os(name))
}

/// 给子进程装上当前这套 TeX 的环境
pub(crate) fn apply(cmd: &mut Command) {
    let env = current();
    if let Some(p) = &env.path {
        cmd.env("PATH", p);
    }
    for (k, v) in &env.vars {
        cmd.env(k, v);
    }
}

/// 设置页、出错卡片要的现状
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub choice: Choice,
    /// 正在用内置的
    pub bundled: bool,
    /// 两套都在、能在设置里换
    pub switchable: bool,
    /// 有内置版可用（换过去能编）
    pub has_bundled: bool,
    pub bundled_year: Option<u32>,
    pub own: Option<Own>,
    pub floor: &'static str,
}

pub fn status(choice: Choice) -> Status {
    let setup = SETUP.get();
    let has_bundled = setup.is_some_and(|s| s.bundled.is_some());
    let own = setup.and_then(|s| s.own.read().unwrap().clone());
    Status {
        choice,
        bundled: bundled(),
        switchable: has_bundled && own.is_some() && setup.is_some_and(|s| !s.forced),
        has_bundled,
        bundled_year: setup.and_then(|s| s.bundled.as_ref()?.year),
        own,
        floor: FLOOR,
    }
}

/// 按新设置换 TeX，立即生效。真换了时在后台重建跟 TeX 绑定的缓存（字体映射、引擎根）。
/// 返回是否换了
pub fn select(choice: Choice) -> bool {
    let changed = swap(choice);
    if changed {
        std::thread::spawn(after_switch);
    }
    changed
}

/// 只换环境，不重建缓存：命令行换完自己当场做 [`after_switch`]
pub(crate) fn swap(choice: Choice) -> bool {
    let Some(setup) = SETUP.get() else { return false };
    let env = pick(setup, choice);
    let changed = env.bundled != bundled();
    if changed {
        *CURRENT.write().unwrap() = Some(Arc::new(env));
        crate::case::forget_tex();
    }
    changed
}

/// 换到某套 TeX 后（含启动时）要做的：内置版重建引擎根的链接、补登记字体映射；字体映射重新扫
pub(crate) fn after_switch() {
    if bundled() {
        let stale = crate::engines::prepare();
        crate::engines::refresh_formats(&stale);
        if let Err(e) = crate::case::register_user_maps() {
            eprintln!("[case] 登记用户层字体映射失败：{e}");
        }
    }
    crate::fontmap::refresh();
}

/* ---------- 探测 ---------- */

/// TeX Live 的平台目录名（`bin/<平台>`）
const TL_PLATFORM: &str = if cfg!(target_os = "macos") {
    "universal-darwin"
} else if cfg!(windows) {
    "windows"
} else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
    "aarch64-linux"
} else {
    "x86_64-linux"
};

/// 内置的两个引擎。项目源码里探测不出特征时只会落到这两个上：Subo 建的项目用 xelatex，导入的用 pdflatex。
/// 用户自己的 TeX 要把它们带齐才算能用：只有 xelatex 时，导入的 arXiv 项目会被退回 xelatex 编，
/// 正是要避开的静默换字体
const BUILTIN_PROGRAMS: [&str; 2] = ["xelatex", "pdflatex"];

fn detect_own(path: &OsStr) -> Option<Own> {
    let found: Vec<PathBuf> = BUILTIN_PROGRAMS.iter().map(|p| which_in(path, p)).collect::<Option<_>>()?;
    let xelatex = &found[0];
    let bin = xelatex.parent().unwrap_or(Path::new("")).to_path_buf();
    let kernel = kernel_date(path);
    Some(Own {
        kind: kind_of(xelatex, path),
        old: kernel.as_deref().is_some_and(|k| k < FLOOR),
        kernel,
        bin: bin.to_string_lossy().into_owned(),
    })
}

fn kind_of(xelatex: &Path, path: &OsStr) -> Kind {
    let real = dunce::canonicalize(xelatex).unwrap_or_else(|_| xelatex.to_path_buf());
    let lower = real.to_string_lossy().to_lowercase();
    if lower.contains("miktex") || which_in(path, "initexmf").is_some() && which_in(path, "tlmgr").is_none() {
        Kind::Miktex
    } else if cfg!(target_os = "linux") && (lower.starts_with("/usr/bin/") || lower.starts_with("/usr/share/")) {
        Kind::Distro
    } else if which_in(path, "tlmgr").is_some() {
        Kind::Texlive
    } else {
        Kind::Other
    }
}

/// 这套 TeX 的 LaTeX 内核日期：`kpsewhich latex.ltx` 找到文件，读 `\fmtversion`
fn kernel_date(path: &OsStr) -> Option<String> {
    let kpsewhich = which_in(path, "kpsewhich")?;
    let mut cmd = crate::engine::bare_command(&kpsewhich);
    cmd.env("PATH", path).arg("latex.ltx");
    let ran = crate::engine::run(&mut cmd, Instant::now() + Duration::from_secs(10)).ok()?;
    let file = ran.log.lines().map(str::trim).find(|l| !l.is_empty())?;
    fmtversion(&fs::read_to_string(file).ok()?)
}

/// `latex.ltx` 里的 `\edef\fmtversion{2021-11-15}`（新版折成两行）；很旧的写成 `2017/04/15`
fn fmtversion(text: &str) -> Option<String> {
    let at = text.find("\\edef\\fmtversion").or_else(|| text.find("\\def\\fmtversion"))?;
    let rest = text[at..].split_once("\\fmtversion")?.1.trim_start();
    let date = rest.strip_prefix('{')?.split('}').next()?.trim().replace('/', "-");
    valid_date(&date).then_some(date)
}

fn valid_date(d: &str) -> bool {
    d.len() == 10 && d.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() })
}

/// `PATH` 里找程序，Windows 上按 `PATHEXT` 补后缀
pub(crate) fn which_in(path: &OsStr, bin: &str) -> Option<PathBuf> {
    std::env::split_paths(path).find_map(|dir| crate::engine::program_in(&dir, bin))
}

fn join_path(front: &[PathBuf], base: &OsStr) -> Option<OsString> {
    let mut dirs: Vec<PathBuf> = front.to_vec();
    dirs.extend(std::env::split_paths(base).filter(|d| !front.contains(d)));
    std::env::join_paths(dirs).ok()
}

/// 内置版的环境：bin 接在最前面，按需安装的引擎（LuaTeX、pTeX）的 bin 紧随其后；
/// 用户层（包内只读，补装的宏包、格式、字体缓存都写这里）
fn bundled_env(root: &Path, layer: &Path, base: &OsStr) -> Env {
    let mut vars = vec![
        ("TEXMFHOME", layer.join("texmf").into_os_string()),
        ("TEXMFVAR", layer.join("texmf-var").into_os_string()),
        ("TEXMFCONFIG", layer.join("texmf-config").into_os_string()),
    ];
    // texmf.cnf 的 TEXMFCACHE 是 `$TEXMFSYSVAR;$TEXMFVAR`，取第一个可写的：调试版的内置树可写，
    // luaotfload 的字体缓存会写进包里。固定到用户层
    if std::env::var_os("TEXMFCACHE").is_none() {
        vars.push(("TEXMFCACHE", layer.join("texmf-var").into_os_string()));
    }
    let bin = root.join("bin").join(TL_PLATFORM);
    let bin = dunce::canonicalize(&bin).unwrap_or(bin);
    let engines = layer.join("engine").join("bin").join(TL_PLATFORM);
    Env { bundled: true, path: join_path(&[bin, engines], base), vars }
}

/// 用户层按内置 TeX Live 的年份分开放：`<base>/2026/texmf`…。2026 年装进去的宏包可能依赖
/// 2026 的 LaTeX 内核，Subo 升级到 2027 后不再挂上旧年份的目录，用到时经缺字清单按需重装一次。
/// 读不出年份就用 `<base>` 本身。
fn user_layer(base: &Path, year: Option<u32>) -> PathBuf {
    let Some(year) = year else { return base.to_path_buf() };
    let layer = base.join(year.to_string());
    // 分年份之前（只有 2026 的版本）用户层直接放在 `<base>` 下，挪进 2026 的目录接着用
    if year == 2026 && !layer.exists() && base.join("texmf").is_dir() {
        let _ = fs::create_dir_all(&layer);
        for d in ["texmf", "texmf-var", "texmf-config"] {
            if base.join(d).exists() {
                let _ = fs::rename(base.join(d), layer.join(d));
            }
        }
    }
    layer
}

/// `release-texlive.txt` 第一行 `TeX Live (https://tug.org/texlive) version 2026` → `2026`
fn release_year(root: &Path) -> Option<u32> {
    let text = fs::read_to_string(root.join("release-texlive.txt")).ok()?;
    text.lines().next()?.rsplit(' ').next()?.get(..4)?.parse().ok()
}

/* ---------- 编译日志：内核比宏包旧 ---------- */

/// 编译失败是因为 LaTeX 内核比宏包、模板旧
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stale {
    /// 本机内核的日期（日志开头的 `LaTeX2e <2021-11-15>`）
    pub kernel: Option<String>,
    /// 文件要求的日期，日志里说了才有
    pub want: Option<String>,
}

/// 从失败的编译日志里认出「内核太旧」。TeX 按 79 列折行，先把行拼起来再找
pub fn stale(log: &str) -> Option<Stale> {
    let flat: String = log.split(['\r', '\n']).collect();
    let hit = flat.contains("l3-too-old")
        || (flat.contains("Support package") && flat.contains("too old"))
        || flat.contains("Mismatched LaTeX support files")
        || flat.contains("You have requested release");
    if !hit {
        return None;
    }
    let date = |s: &str| -> Option<String> {
        let d: String = s.chars().take(10).collect::<String>().replace('/', "-");
        valid_date(&d).then_some(d)
    };
    let after = |key: &str| flat.find(key).and_then(|i| date(&flat[i + key.len()..]));
    // `You have requested release `2026/06/01' of LaTeX, but only release `2021-11-15' is available`
    // `The L3 programming layer in the LaTeX format is dated 2021-11-15, but in your TeX tree the files require at least 2026-06-01`
    let want = after("You have requested release `").or_else(|| after("files require at least "));
    let kernel = after("but only release `")
        .or_else(|| after("LaTeX format is dated "))
        .or_else(|| after("LaTeX2e <"));
    Some(Stale { kernel, want })
}

/* ---------- MiKTeX 更新 ---------- */

/// MiKTeX 没更新成：`reason` 给界面挑文案和下一步，`log` 是命令的输出（只留末尾）
#[derive(Debug)]
pub struct MiktexFail {
    pub reason: &'static str,
    pub log: String,
}

/// 更新 MiKTeX：内核和宏包一起升到最新，再刷新文件库、重建格式。返回日志
///
/// MiKTeX 22 前后命令不一样：新版是 `miktex packages update`，旧版的 `miktex` 没有 `packages`
/// （报 `unknown topic`），要用 `mpm --update`；刷新文件库、重建格式同理退回 `initexmf`
pub fn update_miktex(deadline: Instant) -> Result<String, MiktexFail> {
    let log = update_miktex_in(&std::env::var_os("PATH").unwrap_or_default(), deadline)?;
    refresh_own();
    Ok(log)
}

fn update_miktex_in(path: &OsStr, deadline: Instant) -> Result<String, MiktexFail> {
    let miktex = which_in(&path, "miktex");
    let mpm = which_in(&path, "mpm");
    let initexmf = which_in(&path, "initexmf");
    let mut log = String::new();
    let mut run = |prog: &Path, args: &[&str]| -> Result<crate::engine::Ran, MiktexFail> {
        let mut cmd = crate::engine::bare_command(prog);
        cmd.env("PATH", path).args(args);
        log.push_str(&format!("> {} {}\n", prog.file_stem().unwrap_or_default().to_string_lossy(), args.join(" ")));
        let ran = crate::engine::run(&mut cmd, deadline)
            .map_err(|e| MiktexFail { reason: "failed", log: format!("{log}{e}") })?;
        log.push_str(ran.log.trim_end());
        log.push('\n');
        Ok(ran)
    };
    let fail = |ran: &crate::engine::Ran, log: &str| MiktexFail {
        reason: if ran.timed_out { "offline" } else { miktex_reason(&ran.log) },
        log: tail(log, 60),
    };

    // 新命令先试一下，这个版本不认就整套换旧命令
    let modern = match &miktex {
        Some(m) => !unknown_usage(&run(m, &["packages", "update-package-database"])?.log),
        None => false,
    };
    if modern {
        let m = miktex.as_deref().unwrap_or(Path::new("miktex"));
        let ran = run(m, &["packages", "update"])?;
        if !ran.ok {
            return Err(fail(&ran, &log));
        }
        run(m, &["fndb", "refresh"])?;
        run(m, &["formats", "build"])?;
    } else if let Some(mpm) = &mpm {
        run(mpm, &["--update-db"])?;
        let ran = run(mpm, &["--update"])?;
        if !ran.ok {
            return Err(fail(&ran, &log));
        }
        if let Some(i) = &initexmf {
            run(i, &["--update-fndb"])?;
            run(i, &["--dump"])?;
        }
    } else {
        return Err(MiktexFail { reason: "noTool", log: tail(&log, 60) });
    }
    Ok(tail(&log, 60))
}

/// 命令本身不认（版本太旧，没有这个子命令或选项），不是更新失败
fn unknown_usage(log: &str) -> bool {
    let l = log.to_ascii_lowercase();
    ["unknown topic", "bad usage", "unknown option", "unrecognized"].iter().any(|k| l.contains(k))
}

/// 看 MiKTeX 的报错猜原因：要管理员权限、版本太旧没法在线更新、连不上网
fn miktex_reason(log: &str) -> &'static str {
    let l = log.to_ascii_lowercase();
    let any = |keys: &[&str]| keys.iter().any(|k| l.contains(k));
    if any(&["administrator", "admin mode", "--admin", "access is denied", "access denied", "permission denied", "elevat"]) {
        "admin"
    } else if any(&["upgrade", "too old", "out of date", "out-of-date", "unsupported", "not supported", "incompatible", "newer version"]) {
        "tooOld"
    } else if crate::case::network_failure(log) || any(&["timed out", "timeout", "could not connect", "no internet", "proxy"]) {
        "offline"
    } else {
        "failed"
    }
}

fn tail(log: &str, n: usize) -> String {
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// 重读用户自己的 TeX（用户在 MiKTeX Console、官网安装包里更新过之后）：内核变了，自动档跟着重选
pub fn refresh_own() {
    if let Some(setup) = SETUP.get() {
        *setup.own.write().unwrap() = detect_own(&std::env::var_os("PATH").unwrap_or_default());
    }
    crate::case::forget_tex();
}

/// 打开 MiKTeX Console（它自己能申请管理员权限、升级 MiKTeX 本体）。不等它退出
pub fn open_miktex_console() -> Result<(), String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let console = which_in(&path, "miktex-console").ok_or("找不到 MiKTeX Console")?;
    crate::engine::bare_command(console).spawn().map(drop).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fmtversion() {
        assert_eq!(fmtversion("\\def\\fmtname{LaTeX2e}\n\\edef\\fmtversion\n   {2026-06-01}\n").as_deref(), Some("2026-06-01"));
        assert_eq!(fmtversion("\\edef\\fmtversion{2021-11-15}\n").as_deref(), Some("2021-11-15"));
        assert_eq!(fmtversion("\\def\\fmtversion{2017/04/15}").as_deref(), Some("2017-04-15"));
        assert_eq!(fmtversion("nothing"), None);
        assert!("2021-11-15" < FLOOR && "2024-11-01" > FLOOR);
    }

    #[test]
    fn stale_from_logs() {
        let ctex = "LaTeX2e <2021-11-15>\r\nE:\\MikTeX\\tex/latex/ctex\\ctexart.cls:43: Class ctexart Error: Support package `\r\nexpl3' too old.\r\nl.43 ...or:nnn { ctex } { l3-too-old } { expl3 } }\r\n";
        assert_eq!(stale(ctex), Some(Stale { kernel: Some("2021-11-15".into()), want: None }));
        let release = "LaTeX2e <2021-11-15> patch level 1\nLaTeX Warning: You have requested release `2026/06/01' of LaTeX, but only release `2021-11-15' is available.\n";
        assert_eq!(stale(release), Some(Stale { kernel: Some("2021-11-15".into()), want: Some("2026-06-01".into()) }));
        let l3 = "! LaTeX Error: Mismatched LaTeX support files detected.\nLoading 'expl3.ltx' aborted!\nThe L3 programming layer in the LaTeX format is dated 2022-02-24, but in your TeX tree the files require at least 2025-01-18.\n";
        assert_eq!(stale(l3), Some(Stale { kernel: Some("2022-02-24".into()), want: Some("2025-01-18".into()) }));
        assert_eq!(stale("! Undefined control sequence.\n"), None);
    }

    #[test]
    #[cfg(unix)]
    fn old_miktex_falls_back_to_mpm() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("subo-miktex-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let f = dir.join(name);
            fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
        };
        // MiKTeX 21：miktex 没有 packages，mpm 能更新
        script("miktex", "echo 'bad usage: unknown topic: packages' >&2; exit 1");
        script("mpm", "echo \"mpm $*\"");
        script("initexmf", "echo \"initexmf $*\"");
        let log = update_miktex_in(dir.as_os_str(), Instant::now() + Duration::from_secs(10)).unwrap();
        assert!(log.contains("mpm --update\n") && log.contains("initexmf --dump"), "{log}");
        assert!(!log.contains("miktex packages update\n"), "{log}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn miktex_reasons() {
        assert_eq!(miktex_reason("miktex: The operation requires administrator privileges. Run in --admin mode."), "admin");
        assert_eq!(miktex_reason("this MiKTeX version is too old; please upgrade"), "tooOld");
        assert_eq!(miktex_reason("could not resolve host: miktex.org"), "offline");
        assert_eq!(miktex_reason("something else"), "failed");
    }

    #[test]
    fn picks_tex() {
        let own = |old| Some(Own { kind: Kind::Texlive, kernel: None, old, bin: String::new() });
        let b = || Some(Bundled { year: Some(2026), env: Env { bundled: true, ..Default::default() } });
        let s = |own, bundled, forced| Setup { own: RwLock::new(own), bundled, forced };
        assert!(!pick(&s(own(false), b(), false), Choice::Auto).bundled);
        assert!(pick(&s(own(true), b(), false), Choice::Auto).bundled);
        assert!(pick(&s(None, b(), false), Choice::Auto).bundled);
        assert!(!pick(&s(own(true), b(), false), Choice::Own).bundled);
        assert!(pick(&s(own(false), b(), false), Choice::Bundled).bundled);
        assert!(pick(&s(None, b(), false), Choice::Own).bundled);
        assert!(pick(&s(own(false), b(), true), Choice::Own).bundled);
        // 没有内置版：只能用自己的，哪怕太旧
        assert!(!pick(&s(own(true), None, false), Choice::Bundled).bundled);
    }
}
