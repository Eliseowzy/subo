//! 软件更新的壳侧实现：用 Tauri 的 updater 插件查、下、装。
//! 设置、状态和定时检查在本地服务里（server/src/update.rs），这里只做这三件事。
//!
//! 版本清单为 latest.json；验签公钥在 tauri.conf.json。

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use subo_server::{
    fail::Fail,
    update::{BoxFut, Progress, Release, Shell},
};
use tauri::{AppHandle, Url};
use tauri_plugin_updater::{Error, Update, UpdaterExt};

/// 打包写进去的默认版本清单地址。设置里可以换成别的（内网镜像等）
const DEFAULT_SOURCE: &str = "https://dl.subo.ink/latest.json";

/// 只取一份几百字节的 JSON，连不上就别让用户干等
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Updater {
    app: AppHandle,
    /// 上次查到的版本，下载、安装时用
    found: Arc<Mutex<Option<Update>>>,
}

pub fn hook(app: AppHandle) -> subo_server::update::Hook {
    Arc::new(Updater { app, found: Default::default() })
}

/// Windows 上下好的安装程序先放这里，用户点了重启再运行（安装程序会关掉应用）
#[cfg_attr(not(windows), allow(dead_code))]
fn staged_path() -> PathBuf {
    std::env::temp_dir().join("subo-update-setup.exe")
}

impl Shell for Updater {
    fn default_source(&self) -> String {
        DEFAULT_SOURCE.into()
    }

    fn check(&self, source: String) -> BoxFut<Option<Release>> {
        let (app, found) = (self.app.clone(), self.found.clone());
        let source = if source.is_empty() { DEFAULT_SOURCE.to_string() } else { source };
        Box::pin(async move {
            let url: Url = source
                .parse()
                .map_err(|e| Fail::new("update.badSource", "版本清单地址不对").with("url", source.clone()).detail(e))?;
            #[cfg_attr(not(windows), allow(unused_mut))]
            let mut builder = app.updater_builder();
            // Windows 轻量版（不带 TeX）只换成轻量版，免得一升级多出两百来 MB 的 TeX。
            #[cfg(windows)]
            if crate::platform::bundled_texlive().is_none() {
                builder = builder.target("windows-x86_64-lite");
            }
            let updater = builder.endpoints(vec![url]).and_then(|b| b.build()).map_err(fail)?;
            let update = tokio::time::timeout(CHECK_TIMEOUT, updater.check())
                .await
                .map_err(|_| Fail::new("update.timeout", "连接更新服务器超时"))?
                .map_err(fail)?;
            let release = update.as_ref().map(|u| Release {
                version: u.version.clone(),
                notes: u.body.clone().filter(|b| !b.trim().is_empty()),
                date: u.raw_json.get("pub_date").and_then(|d| d.as_str()).map(Into::into),
            });
            *found.lock().unwrap() = update;
            Ok(release)
        })
    }

    fn download(&self, progress: Progress) -> BoxFut<()> {
        let found = self.found.clone();
        Box::pin(async move {
            let update = found
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| Fail::new("update.nothingToDownload", "还没查到新版本，先检查更新"))?;
            let mut got = 0u64;
            let bytes = update
                .download(
                    |n, total| {
                        got += n as u64;
                        progress(got, total);
                    },
                    || {},
                )
                .await
                .map_err(fail)?;
            // macOS / Linux 直接装：换掉应用文件，正在跑的这个不受影响，重启后就是新版本。
            // 装在要管理员权限的位置时插件会弹系统授权框，所以放到 blocking 线程
            #[cfg(not(windows))]
            tauri::async_runtime::spawn_blocking(move || update.install(bytes))
                .await
                .map_err(|e| Fail::new("update.installFailed", "安装新版本失败").detail(e))?
                .map_err(fail)?;
            // Windows 的 install 会运行安装程序并退出应用，等用户点了再做；两百来 MB 不留在内存里
            #[cfg(windows)]
            std::fs::write(staged_path(), &bytes)
                .map_err(|e| Fail::new("update.installFailed", "保存安装程序失败").detail(e))?;
            Ok(())
        })
    }

    fn apply(&self) -> Result<(), Fail> {
        #[cfg(windows)]
        {
            let update = self.found.lock().unwrap().clone().ok_or_else(|| Fail::new("update.notReady", "新版本还没下载好"))?;
            let bytes = std::fs::read(staged_path()).map_err(|e| Fail::new("update.notReady", "新版本还没下载好").detail(e))?;
            update.install(bytes).map_err(fail)?;
        }
        self.app.restart()
    }
}

/// 插件的错误换成界面认得的错误码（文案在语言文件 `server.update.*`）
fn fail(e: Error) -> Fail {
    let (code, msg) = match &e {
        Error::Reqwest(_) | Error::Network(_) | Error::Http(_) => ("update.network", "连不上更新服务器"),
        Error::ReleaseNotFound | Error::Serialization(_) | Error::Semver(_) => ("update.badManifest", "版本清单读不懂"),
        Error::TargetNotFound(_) | Error::TargetsNotFound(_) | Error::UnsupportedArch | Error::UnsupportedOs => {
            ("update.noPackage", "新版本没有适合这台电脑的安装包")
        }
        Error::Minisign(_) | Error::Base64(_) | Error::SignatureUtf8(_) | Error::SignedVersionMismatch { .. } | Error::MissingSignedVersion => {
            ("update.badSignature", "更新包的签名对不上，已丢弃")
        }
        Error::AuthenticationFailed => ("update.denied", "没有授权替换应用"),
        Error::InsecureTransportProtocol => ("update.badSource", "版本清单地址要是 https 网址"),
        _ => ("update.installFailed", "安装新版本失败"),
    };
    Fail::new(code, msg).detail(e)
}
