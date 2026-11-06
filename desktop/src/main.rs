//! Subo 桌面壳。
//!
//! 基本不走 Tauri 的 IPC（只有 macOS 上拖动窗口要用，见 capabilities/window.json）：
//! 在本进程里起 `subo-server`（随机端口，只听 127.0.0.1），
//! 窗口直接打开它托管的前端。前端和浏览器里调试时完全是同一套代码。
//! 壳只补浏览器做不到的事：原生选文件夹、外链交给系统浏览器、下载落到「下载」目录、
//! macOS 菜单栏（`menu.rs`）、软件更新（`update.rs`）。
//!
//! 平台差异集中在 `platform.rs`；加 Windows 时只需补那里的 `cfg(windows)` 分支和打包配置。

// 发布版在 Windows 上不弹控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
mod menu;
mod platform;
mod update;

use std::{
    collections::HashMap,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use subo_server::fail::Fail;
use tauri::{
    webview::{DownloadEvent, NewWindowResponse},
    AppHandle, Manager, PhysicalSize, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_dialog::DialogExt;

const MAIN: &str = "main";

/// 告诉前端红绿灯叠在页面上了，顶栏左边要让出位置（styles.css 里的 data-shell="mac"）
#[cfg(target_os = "macos")]
const MAC_SHELL_SCRIPT: &str = r#"document.documentElement.dataset.shell = "mac";"#;

fn main() {
    // 从 Finder / 桌面环境启动时 PATH 很短，xelatex、tlmgr、unzip 会找不到
    platform::fix_path();
    let bundled_tex = platform::choose_tex();

    // `subo build ./paper` 这类一次性命令：用同一套 TeX（多半是内置的），不开窗口。见 docs/cli.md
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| subo_server::cli::wants(a)) {
        std::process::exit(subo_server::cli::run("subo", &args, bundled_tex));
    }

    let builder = tauri::Builder::default();
    // 菜单栏只在 macOS 上做；Linux 上 Tauri 会把菜单画进窗口里，和界面不搭
    #[cfg(target_os = "macos")]
    let builder = builder.menu(|app| menu::empty(app)).on_menu_event(menu::on_event);
    let app = builder
        // 必须最先注册：第二次启动只把已有窗口叫到前面，不再起第二个服务
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window(MAIN) {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let addr = start_server(app.handle(), bundled_tex)?;
            open_window(app.handle(), addr)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Subo 启动失败");
    app.run(|_app, _event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event {
            menu::reopen(_app);
        }
    });
}

/// 先同步绑好端口再起服务：窗口打开时端口已在听，请求会排队等服务就绪，不会连不上。
fn start_server(app: &AppHandle, bundled_tex: bool) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let std_listener = TcpListener::bind(("127.0.0.1", 0))?;
    std_listener.set_nonblocking(true)?;
    let addr = std_listener.local_addr()?;

    let opts = subo_server::Options {
        dist: Some(frontend_dir(app)?),
        picker: Some(folder_picker(app.clone())),
        #[cfg(target_os = "macos")]
        menu: Some(menu::hook(app.clone())),
        bundled_tex,
        updater: updater(app),
        ..Default::default()
    };
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let res = match tokio::net::TcpListener::from_std(std_listener) {
            Ok(listener) => subo_server::serve(opts, listener).await,
            Err(e) => Err(e),
        };
        if let Err(e) = res {
            eprintln!("本地服务退出: {e}");
            handle.exit(1);
        }
    });
    Ok(addr)
}

/// 软件更新只给打包出来的应用：调试版是 target/ 下的裸程序，没有可替换的 .app。
/// `SUBO_UPDATE_TEST=1` 强行打开，用来对着本机起的清单试流程
fn updater(app: &AppHandle) -> Option<subo_server::update::Hook> {
    let on = !cfg!(debug_assertions) || std::env::var_os("SUBO_UPDATE_TEST").is_some();
    on.then(|| update::hook(app.clone()))
}

/// 前端构建产物：打包后在资源目录的 `dist/`；`SUBO_DIST` 可指到别处（开发时指向 app/dist）。
fn frontend_dir(app: &AppHandle) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(dir) = std::env::var_os("SUBO_DIST") {
        return Ok(PathBuf::from(dir));
    }
    // 调试版直接用仓库里的 app/dist：target/debug 下的资源拷贝只在 cargo 重编时刷新，会是旧前端
    if cfg!(debug_assertions) {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../app/dist");
        if repo.join("index.html").is_file() {
            return Ok(repo);
        }
    }
    let bundled = app.path().resource_dir()?.join("dist");
    if bundled.join("index.html").is_file() {
        return Ok(bundled);
    }
    Err("找不到前端构建产物：先在 app/ 下运行 npm run build".into())
}

/// 原生「选择文件夹」框，替代服务端只在 macOS 上可用的 osascript。
fn folder_picker(app: AppHandle) -> subo_server::Picker {
    Arc::new(move || {
        let mut dialog = app.dialog().file();
        if let Some(w) = app.get_webview_window(MAIN) {
            dialog = dialog.set_parent(&w);
        }
        match dialog.blocking_pick_folder() {
            None => Ok(None),
            Some(p) => p
                .into_path()
                .map(Some)
                .map_err(|e| Fail::new("folder.pickFailed", format!("选择框出错: {e}")).detail(e)),
        }
    })
}

fn open_window(app: &AppHandle, addr: SocketAddr) -> tauri::Result<()> {
    let home: Url = format!("http://{addr}/").parse().expect("本地地址");
    let origin = home.origin();
    let nav_origin = origin.clone();
    // macOS 的下载完成事件不带路径，自己记下每个下载落在哪
    let saved: Arc<Mutex<HashMap<Url, PathBuf>>> = Default::default();
    let downloads = app.path().download_dir().ok();

    let builder = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(home))
        .title("Subo")
        .inner_size(1280.0, 820.0)
        .min_inner_size(880.0, 560.0)
        .center()
        // 先藏着，量过屏幕再显示，免得用户看到窗口跳一下
        .visible(false)
        // 本地服务之外的地址一律交给系统浏览器，窗口里只跑 Subo 自己
        .on_navigation(move |url| {
            if url.origin() == nav_origin || url.scheme() == "about" {
                return true;
            }
            open_external(url);
            false
        })
        .on_new_window(move |url, _features| {
            if url.origin() != origin {
                open_external(&url);
            }
            NewWindowResponse::Deny
        })
        .on_download(move |_webview, event| {
            match event {
                DownloadEvent::Requested { url, destination } => {
                    let Some(dir) = downloads.as_ref() else { return false };
                    let name = destination
                        .file_name()
                        .map(|n| n.to_owned())
                        .or_else(|| url.path_segments()?.last().map(Into::into))
                        .unwrap_or_else(|| "download".into());
                    *destination = platform::unique_path(&dir.join(name));
                    saved.lock().unwrap().insert(url, destination.clone());
                }
                DownloadEvent::Finished { url, path, success } => {
                    let path = path.or_else(|| saved.lock().unwrap().remove(&url));
                    if let (true, Some(p)) = (success, path) {
                        let _ = tauri_plugin_opener::reveal_item_in_dir(p);
                    }
                }
                _ => {}
            }
            true
        });
    // 原生标题栏跟着系统深浅色走，系统深色、界面浅色时顶上一条黑边。
    // 改成透明叠在页面上，由页面自己的底色铺到顶；红绿灯挪到和编辑页顶栏（46px）居中。
    // 拖动窗口靠页面里的 data-tauri-drag-region，权限见 capabilities/window.json
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(16.0, 21.0))
        .initialization_script(MAC_SHELL_SCRIPT);
    let win = builder.build()?;
    fit_to_screen(&win);
    win.show()?;
    Ok(())
}

/// 默认尺寸在高缩放比的小屏上（如 Windows 笔记本 150%）比工作区还高，底边压在任务栏下面：
/// 超出时缩到工作区以内再居中
fn fit_to_screen(win: &WebviewWindow) {
    let Ok(Some(m)) = win.current_monitor().or_else(|_| win.primary_monitor()) else { return };
    let area = m.work_area().size;
    let (Ok(outer), Ok(inner)) = (win.outer_size(), win.inner_size()) else { return };
    // 四周留一点空，看得出是个窗口
    let (max_w, max_h) = (area.width * 95 / 100, area.height * 95 / 100);
    if outer.width <= max_w && outer.height <= max_h {
        return;
    }
    // 外框比内容区多出的标题栏和边框
    let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
    let size = PhysicalSize::new(inner.width.min(max_w.saturating_sub(dw)), inner.height.min(max_h.saturating_sub(dh)));
    let _ = win.set_size(size);
    let _ = win.center();
}

fn open_external(url: &Url) {
    if matches!(url.scheme(), "http" | "https" | "mailto") {
        let _ = tauri_plugin_opener::open_url(url.as_str(), None::<&str>);
    }
}
