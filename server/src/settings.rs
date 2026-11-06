//! 用户设置：`<subo 目录>/settings.json`，界面「设置」页读写。
//!
//! 只放用户会改、改了要记住的东西：项目库位置、安装源、缺包时是否自动安装、编译超时。
//! 外观、语言的选择和用户导入的包在 appearance.rs（`<subo 目录>/appearance/`）。
//!
//! 读不出来（首次使用、文件坏了）就用默认值，不拦启动；写入先写临时文件再改名。

use crate::{case, engine};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 编译超时允许的范围（秒）
pub const TIMEOUT_MIN: u64 = 10;
pub const TIMEOUT_MAX: u64 = 1800;

/// 编译内存上限允许的范围与默认值（MB）。正常文档几百 MB，LuaLaTeX 带大字体集也就一两 GB
pub const MEMORY_MIN: u64 = 512;
pub const MEMORY_MAX: u64 = 65536;
pub const MEMORY_DEFAULT: u64 = 4096;

/// 同时在跑的编译最多几份。一次 TeX 编译只吃一个核，上限是本机逻辑核数（machine.rs），默认不额外限制
pub const PARALLEL_MIN: u64 = 1;

/// 跟随 TeX Live 自己的 `tlmgr option repository`，不额外指定
pub const MIRROR_TLMGR: &str = "tlmgr";
pub const MIRROR_CUSTOM: &str = "custom";

/// 预置的 TeX Live 安装源。CTAN 官方地址按地理位置调度，中国大陆常被分到海外镜像或直接超时，
/// 所以把国内常用的几家 CTAN 镜像列出来，界面上可以一键测速挑最快的。
/// 名称（显示用）在前端语言文件 `settings.mirror.presets.<id>` 里。
pub const PRESETS: &[(&str, &str, Region)] = &[
    ("ctan", "https://mirror.ctan.org/systems/texlive/tlnet", Region::Global),
    ("tuna", "https://mirrors.tuna.tsinghua.edu.cn/CTAN/systems/texlive/tlnet", Region::Cn),
    ("ustc", "https://mirrors.ustc.edu.cn/CTAN/systems/texlive/tlnet", Region::Cn),
    ("aliyun", "https://mirrors.aliyun.com/CTAN/systems/texlive/tlnet", Region::Cn),
    ("huawei", "https://mirrors.huaweicloud.com/CTAN/systems/texlive/tlnet", Region::Cn),
    ("bfsu", "https://mirrors.bfsu.edu.cn/CTAN/systems/texlive/tlnet", Region::Cn),
    ("sjtu", "https://mirror.sjtu.edu.cn/CTAN/systems/texlive/tlnet", Region::Cn),
    ("nju", "https://mirrors.nju.edu.cn/CTAN/systems/texlive/tlnet", Region::Cn),
];

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    Global,
    Cn,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// 编译缺宏包时不再询问，直接装好重编。需要管理员授权的情况（系统 TeX Live 归 root）仍然询问
    pub auto_install: bool,
    /// `tlmgr`、预置源的 id，或 `custom`
    pub mirror: String,
    /// `mirror` 为 `custom` 时的地址：http(s) 网址，或本机上的 tlnet 目录（离线安装源）
    pub custom_mirror: String,
    /// 项目库位置；`None` 用默认的 `~/Documents/Subo`
    pub library: Option<PathBuf>,
    /// 单次编译超时（秒）；`None` 用启动参数给的值
    pub timeout_secs: Option<u64>,
    /// 编译进程的内存上限（MB）；`None` 用 [`MEMORY_DEFAULT`]
    pub memory_mb: Option<u64>,
    /// 同时编译的项目数上限；`None` 为本机逻辑核数
    pub parallel: Option<u64>,
    /// 用哪一套 TeX：自己装的还是内置的（见 tex.rs）
    pub tex: crate::tex::Choice,
    /// 软件更新：启动后和此后每天查一次有没有新版本（只取一份很小的版本清单，见 update.rs）
    pub update_check: bool,
    /// 查到新版本就在后台下好、装好，重启即生效；关着时只提示，用户点了才下载
    pub update_download: bool,
    /// 版本清单地址；空着用打包时写进去的默认地址
    pub update_source: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            auto_install: false,
            mirror: MIRROR_TLMGR.into(),
            custom_mirror: String::new(),
            library: None,
            timeout_secs: None,
            memory_mb: None,
            parallel: None,
            tex: crate::tex::Choice::Auto,
            update_check: true,
            update_download: false,
            update_source: String::new(),
        }
    }
}

impl Settings {
    pub fn load(file: &Path) -> Self {
        fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        fs::rename(&tmp, file)
    }

    /// 实际给 tlmgr 的仓库；`None` 表示不指定，用 tlmgr 自己的设置
    pub fn repository(&self) -> Option<String> {
        match self.mirror.as_str() {
            MIRROR_TLMGR => None,
            MIRROR_CUSTOM => Some(self.custom_mirror.trim().to_string()).filter(|u| !u.is_empty()),
            id => PRESETS.iter().find(|p| p.0 == id).map(|p| p.1.to_string()),
        }
    }

    /// 软件源跨到新年度版本后要不要自动改用归档源。自定义源是用户自己指的，不替用户换；
    /// 跟随 TeX Live、预置镜像都会随年度切走，自动换
    pub fn fallback(&self) -> case::Fallback {
        if self.mirror == MIRROR_CUSTOM {
            case::Fallback::Off
        } else {
            case::Fallback::Auto
        }
    }
}

/// 自定义的版本清单地址：空（用默认），或 https 网址。更新包本身靠签名把关，
/// 但清单走明文的话能被换成旧版本的清单，所以只收 https
pub fn check_update_source(url: &str) -> Result<(), &'static str> {
    if url.is_empty() {
        return Ok(());
    }
    let rest = url.strip_prefix("https://").ok_or("settings.badUpdateSource")?;
    if rest.is_empty() || url.len() > 512 || url.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
        return Err("settings.badUpdateSource");
    }
    Ok(())
}

/// 这个地址是不是国内的预置镜像：回退到归档源时先试国内的归档镜像
pub fn is_cn(url: &str) -> bool {
    let url = url.trim_end_matches('/');
    PRESETS.iter().any(|p| matches!(p.2, Region::Cn) && p.1 == url)
}

/// 校验安装源：预置 id、`tlmgr`，或合法的自定义地址
pub fn check_mirror(mirror: &str, custom: &str) -> Result<(), &'static str> {
    match mirror {
        MIRROR_TLMGR => Ok(()),
        MIRROR_CUSTOM => check_custom(custom.trim()),
        id if PRESETS.iter().any(|p| p.0 == id) => Ok(()),
        _ => Err("settings.badMirror"),
    }
}

/// 自定义源：http(s) 网址，或本机已存在、带 `tlpkg/texlive.tlpdb` 的目录。
/// 地址会作为一个独立参数交给 tlmgr / curl，不经过 shell；仍然拒绝空白和引号，免得出怪事
fn check_custom(url: &str) -> Result<(), &'static str> {
    if url.is_empty() || url.len() > 512 || url.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
        return Err("settings.badCustomMirror");
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Ok(());
    }
    let p = Path::new(url);
    if p.is_absolute() {
        return if p.join("tlpkg").join("texlive.tlpdb").is_file() { Ok(()) } else { Err("settings.localMirrorInvalid") };
    }
    Err("settings.badCustomMirror")
}

/// 测速：取仓库里很小的 `texlive.tlpdb.sha512` 的第一个字节，返回耗时（毫秒）；不通返回 `None`。
/// 本机目录直接看文件在不在。
pub fn probe(url: &str, timeout: Duration) -> Option<u64> {
    if !url.starts_with("http") {
        return Path::new(url).join("tlpkg").join("texlive.tlpdb").is_file().then_some(0);
    }
    let curl = engine::which("curl")?;
    let target = format!("{}/tlpkg/texlive.tlpdb.sha512", url.trim_end_matches('/'));
    let secs = format!("{}", timeout.as_secs().max(1));
    let started = Instant::now();
    let ran = engine::run(
        crate::engine::command(curl).args(["-fsSL", "-o", "/dev/null", "-r", "0-0", "--max-time", &secs, &target]),
        Instant::now() + timeout + Duration::from_secs(2),
    )
    .ok()?;
    ran.ok.then(|| started.elapsed().as_millis() as u64)
}

/// 测速时下载多少字节来估吞吐：`texlive.tlpdb` 有十几 MB，取开头这一段
const SPEED_BYTES: u64 = 512 * 1024;

/// 一个源的测速结果
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Speed {
    /// 首字节耗时（毫秒）；不通为 `None`
    pub ms: Option<u64>,
    /// 下载 [`SPEED_BYTES`] 那段的速度（KB/s）；本机目录不测，为 `None`
    pub kbps: Option<u64>,
}

/// 测速：下载 `texlive.tlpdb` 开头的一段，同时看首字节耗时和下载速度。
/// 只测延迟会挑出「连得快、下得慢」的源，装几十 MB 的包时反而更慢。本机目录只看在不在
pub fn probe_speed(url: &str, timeout: Duration) -> Speed {
    if !url.starts_with("http") {
        let ok = Path::new(url).join("tlpkg").join("texlive.tlpdb").is_file();
        return Speed { ms: ok.then_some(0), kbps: None };
    }
    let Some(curl) = engine::which("curl") else { return Speed::default() };
    let target = format!("{}/tlpkg/texlive.tlpdb", url.trim_end_matches('/'));
    let secs = format!("{}", timeout.as_secs().max(1));
    let range = format!("0-{}", SPEED_BYTES - 1);
    let Ok(ran) = engine::run(
        crate::engine::command(curl).args([
            "-sSL", "-o", "/dev/null", "-r", &range, "--max-time", &secs,
            "-w", "\nSUBO %{http_code} %{size_download} %{time_starttransfer} %{time_total}\n", &target,
        ]),
        Instant::now() + timeout + Duration::from_secs(2),
    ) else {
        return Speed::default();
    };
    // 超时（curl 28）也有 -w 的输出：下了一部分照样能算速度
    let Some(line) = ran.log.lines().rev().find_map(|l| l.strip_prefix("SUBO ")) else { return Speed::default() };
    let f: Vec<&str> = line.split_whitespace().collect();
    let (Some(code), Some(size), Some(first), Some(total)) = (
        f.first().and_then(|x| x.parse::<u32>().ok()),
        f.get(1).and_then(|x| x.parse::<f64>().ok()),
        f.get(2).and_then(|x| x.parse::<f64>().ok()),
        f.get(3).and_then(|x| x.parse::<f64>().ok()),
    ) else {
        return Speed::default();
    };
    if !(200..300).contains(&code) || size <= 0.0 {
        return Speed::default();
    }
    let kbps = (total > first).then(|| (size / 1024.0 / (total - first).max(0.001)) as u64);
    Speed { ms: Some((first * 1000.0) as u64), kbps }
}

/// 命令行给的源：预置 id、`tlmgr`，或地址（http(s) 网址、本机 tlnet 目录）。
/// 返回 (`mirror`, `custom_mirror`)，原样存进设置
pub fn parse_mirror(spec: &str) -> Result<(String, String), &'static str> {
    let spec = spec.trim();
    if spec == MIRROR_TLMGR || PRESETS.iter().any(|p| p.0 == spec) {
        return Ok((spec.to_string(), String::new()));
    }
    // 地址正好是某个预置源的，记成预置 id：回退到归档源时还能认出是国内镜像
    if let Some(p) = PRESETS.iter().find(|p| p.1 == spec.trim_end_matches('/')) {
        return Ok((p.0.to_string(), String::new()));
    }
    if !spec.contains('/') {
        return Err("settings.badMirror");
    }
    check_custom(spec).map(|_| (MIRROR_CUSTOM.to_string(), spec.to_string()))
}

/// 新项目库位置能不能用：绝对路径、不与旧库互相包含、能建出来
pub fn check_library(next: &Path, current: &Path) -> Result<PathBuf, &'static str> {
    if !next.is_absolute() {
        return Err("folder.notAbsolute");
    }
    fs::create_dir_all(next).map_err(|_| "settings.libraryCreateFailed")?;
    let next = dunce::canonicalize(&next).map_err(|_| "settings.libraryCreateFailed")?;
    if next == current {
        return Err("settings.librarySame");
    }
    if next.starts_with(current) || current.starts_with(&next) {
        return Err("settings.libraryNested");
    }
    Ok(next)
}

/// 把旧项目库里的项目（含模板目录 `.templates`）搬到新位置。只用 rename：
/// 同一磁盘上瞬间完成、不会半途丢文件；跨磁盘 rename 不了的、新位置已有同名的，原样留在旧库。
/// 返回 (搬过去的, 留下的名字)。
pub fn move_library(from: &Path, to: &Path) -> (usize, Vec<String>) {
    let mut moved = 0;
    let mut left = Vec::new();
    let Ok(rd) = fs::read_dir(from) else { return (0, left) };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // 隐藏目录里只有模板要带走；.DS_Store、编辑器缓存之类不管
        if name.starts_with('.') && name != crate::templates::DIR_NAME {
            continue;
        }
        if !e.path().is_dir() {
            continue;
        }
        let dest = to.join(&name);
        if dest.exists() || fs::rename(e.path(), &dest).is_err() {
            left.push(name);
        } else {
            moved += 1;
        }
    }
    (moved, left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_follows_choice() {
        let mut s = Settings::default();
        assert_eq!(s.repository(), None);
        s.mirror = "tuna".into();
        assert_eq!(s.repository().as_deref(), Some("https://mirrors.tuna.tsinghua.edu.cn/CTAN/systems/texlive/tlnet"));
        s.mirror = MIRROR_CUSTOM.into();
        s.custom_mirror = "  ".into();
        assert_eq!(s.repository(), None);
        s.custom_mirror = " https://example.org/tlnet ".into();
        assert_eq!(s.repository().as_deref(), Some("https://example.org/tlnet"));
    }

    #[test]
    fn custom_mirror_validated() {
        assert!(check_mirror("ustc", "").is_ok());
        assert!(check_mirror("nope", "").is_err());
        assert!(check_mirror(MIRROR_CUSTOM, "https://example.org/tlnet").is_ok());
        assert_eq!(check_mirror(MIRROR_CUSTOM, "ftp://x"), Err("settings.badCustomMirror"));
        assert_eq!(check_mirror(MIRROR_CUSTOM, "https://a b"), Err("settings.badCustomMirror"));
        assert_eq!(check_mirror(MIRROR_CUSTOM, "/nonexistent/tlnet"), Err("settings.localMirrorInvalid"));
    }

    #[test]
    fn mirror_spec_parsed() {
        assert_eq!(parse_mirror("tuna"), Ok(("tuna".into(), String::new())));
        assert_eq!(parse_mirror("tlmgr"), Ok(("tlmgr".into(), String::new())));
        assert_eq!(parse_mirror("https://mirrors.ustc.edu.cn/CTAN/systems/texlive/tlnet/"), Ok(("ustc".into(), String::new())));
        assert_eq!(parse_mirror("https://example.org/tlnet"), Ok((MIRROR_CUSTOM.into(), "https://example.org/tlnet".into())));
        assert_eq!(parse_mirror("tunaa"), Err("settings.badMirror"));
        assert_eq!(parse_mirror("/nonexistent/tlnet"), Err("settings.localMirrorInvalid"));
    }

    #[test]
    fn old_file_fields_default() {
        let s: Settings = serde_json::from_str(r#"{"autoInstall":true}"#).unwrap();
        assert!(s.auto_install);
        assert_eq!(s.mirror, MIRROR_TLMGR);
    }

    #[test]
    fn library_move_keeps_conflicts() {
        let base = std::env::temp_dir().join(format!("subo-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (a, b) = (base.join("a"), base.join("b"));
        for d in ["p1", "p2", ".templates/t1", ".cache"] {
            fs::create_dir_all(a.join(d)).unwrap();
        }
        fs::create_dir_all(b.join("p2")).unwrap();
        assert_eq!(check_library(&a.join("p1"), &dunce::canonicalize(&a).unwrap()), Err("settings.libraryNested"));
        let (moved, left) = move_library(&a, &b);
        assert_eq!(moved, 2);
        assert_eq!(left, vec!["p2".to_string()]);
        assert!(b.join("p1").is_dir() && b.join(".templates/t1").is_dir());
        assert!(a.join(".cache").is_dir());
        let _ = fs::remove_dir_all(&base);
    }
}
