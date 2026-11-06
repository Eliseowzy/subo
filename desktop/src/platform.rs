//! 平台差异：macOS、Linux、Windows。

use std::path::{Path, PathBuf};

/// 补全 PATH：从 Finder、Dock、桌面环境启动的程序拿不到用户 shell 里配的 PATH，
/// 引擎探测（xelatex / tectonic）、tlmgr、unzip 都靠 PATH 找。
/// 先取登录 shell 的 PATH，再追加 TeX 发行版的常见安装位置。
pub fn fix_path() {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let mut add = |d: PathBuf| {
        if d.is_dir() && !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    for d in login_shell_path() {
        add(d);
    }
    for d in tex_dirs() {
        add(d);
    }
    if let Ok(joined) = std::env::join_paths(&dirs) {
        // 还没有起任何线程，改环境变量是安全的
        std::env::set_var("PATH", joined);
    }
}

#[cfg(unix)]
fn login_shell_path() -> Vec<PathBuf> {
    use std::{ffi::OsString, process::Command, time::Duration};
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"));
    // 用标记包住，避开 shell 启动脚本自己往 stdout 打的东西
    let mut child = match Command::new(shell)
        .args(["-l", "-c", "printf '\\n__SUBO__%s__SUBO__' \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    // 启动脚本卡住（比如等网络）不能拖住整个应用
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        use std::io::Read;
        let _ = so.read_to_string(&mut out);
    }
    out.split("__SUBO__")
        .nth(1)
        .map(|p| std::env::split_paths(&OsString::from(p)).collect())
        .unwrap_or_default()
}

#[cfg(windows)]
fn login_shell_path() -> Vec<PathBuf> {
    // Windows 的 GUI 程序继承注册表里的 PATH，不需要这一步
    Vec::new()
}

/// TeX 发行版的常见位置。都不存在时由服务端退回 mock 引擎，界面上会提示。
fn tex_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        // MacTeX / BasicTeX 的稳定软链，以及 Homebrew 的两个前缀
        for d in ["/Library/TeX/texbin", "/opt/homebrew/bin", "/usr/local/bin"] {
            out.push(PathBuf::from(d));
        }
    } else if cfg!(target_os = "linux") {
        // 官方安装器装在 /usr/local/texlive/<年份>/bin/<架构>，取最新一年
        out.extend(newest_texlive(Path::new("/usr/local/texlive")));
        if let Some(home) = subo_server::home_dir() {
            out.extend(newest_texlive(&home.join("texlive")));
        }
        out.push(PathBuf::from("/usr/local/bin"));
    } else if cfg!(windows) {
        // TeX Live 官方安装器默认装在 C:\texlive\<年份>\bin\windows；装好后写的是注册表里的 PATH，
        // 装完没重新登录时本进程看不到，这里补上
        let drive = std::env::var_os("SystemDrive").map_or_else(|| PathBuf::from("C:\\"), |d| PathBuf::from(d).join("\\"));
        out.extend(newest_texlive(&drive.join("texlive")));
        // MiKTeX：「只为我安装」在 %LOCALAPPDATA%，「为所有用户安装」在 Program Files
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            out.push(PathBuf::from(local).join("Programs\\MiKTeX\\miktex\\bin\\x64"));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            out.push(PathBuf::from(pf).join("MiKTeX\\miktex\\bin\\x64"));
        }
    }
    out
}

fn newest_texlive(root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut years: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.len() == 4 && n.chars().all(|c| c.is_ascii_digit())))
        .collect();
    years.sort();
    let Some(year) = years.pop() else { return Vec::new() };
    std::fs::read_dir(year.join("bin"))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

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

/// 选 TeX：交给服务端（tex.rs），按设置和本机 TeX 的版本在自己的和内置的之间选，运行中可以换。
/// 返回是否用了内置版。必须在起任何线程之前调用
pub fn choose_tex() -> bool {
    subo_server::tex::init(bundled_texlive())
}

/// 内置 TeX Live 的根目录，由 scripts/desktop.sh bundle 在 Tauri 出包后拷进去：
/// macOS 在 `Subo.app/Contents/Resources/texlive`；Linux 在 `/usr/lib/subo/texlive`
/// （deb，程序是 `/usr/bin/subo`），AppImage 里是挂载点下的 `usr/lib/subo/texlive`；
/// Windows 在安装目录下的 `texlive\`（和 `subo.exe` 同级）。
/// 调试版用 `desktop/texlive`（由 scripts/texlive-trim.sh 生成）。
pub fn bundled_texlive() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        if cfg!(target_os = "macos") {
            // Contents/MacOS/subo → Contents/Resources/texlive
            candidates.push(dir.join("../Resources/texlive"));
        } else if cfg!(target_os = "linux") {
            // usr/bin/subo → usr/lib/subo/texlive，deb 与 AppImage 的布局相同
            candidates.push(dir.join("../lib/subo/texlive"));
        } else if cfg!(windows) {
            candidates.push(dir.join("texlive"));
        }
    }
    if cfg!(target_os = "linux") {
        candidates.push(PathBuf::from("/usr/lib/subo/texlive"));
    }
    if cfg!(debug_assertions) {
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("texlive"));
    }
    candidates
        .into_iter()
        .find(|r| subo_server::program_in(&r.join("bin").join(TL_PLATFORM), "xelatex").is_some())
        .and_then(|r| dunce::canonicalize(&r).ok())
}

/// `下载/main.pdf` 已存在就用 `main (2).pdf`，不覆盖用户的文件。
pub fn unique_path(want: &Path) -> PathBuf {
    if !want.exists() {
        return want.to_path_buf();
    }
    let stem = want.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = want.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = want.parent().unwrap_or(Path::new("."));
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("总能找到空位")
}
