//! macOS 菜单栏。
//!
//! 文案由前端按当前语言翻译好，经本地服务交过来（见 server/src/shell.rs），这里只照着画。
//! 前端没交之前菜单是空的：比先露一份英文默认菜单再换掉好。
//! 点了菜单项，往窗口里派一个 `subo:menu` 事件（detail 是项的 id），前端接着做；
//! 只有 `shell.*` 是壳自己处理的。

use subo_server::{fail::Fail, shell::MenuEntry};
use tauri::{
    menu::{AboutMetadata, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Manager, Wry,
};

use crate::MAIN;

/// 交给服务端的回调：切到主线程把菜单整份换掉
pub fn hook(app: AppHandle) -> subo_server::shell::MenuHook {
    std::sync::Arc::new(move |entries: Vec<MenuEntry>| {
        let handle = app.clone();
        app.run_on_main_thread(move || {
            if let Err(e) = build(&handle, &entries).and_then(|m| handle.set_menu(m).map(|_| ())) {
                eprintln!("菜单更新失败: {e}");
            }
        })
        .map_err(|e| Fail::new("shell.menuFailed", format!("菜单更新失败: {e}")).detail(e))
    })
}

/// 启动时的空菜单
pub fn empty(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    Menu::new(app)
}

fn build(app: &AppHandle, entries: &[MenuEntry]) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;
    let mut sub: Option<Submenu<Wry>> = None;
    for e in entries {
        if e.kind == "menu" {
            if let Some(s) = sub.take() {
                menu.append(&s)?;
            }
            sub = Some(Submenu::new(app, &e.label, e.enabled)?);
            continue;
        }
        let Some(s) = sub.as_ref() else { continue };
        let item: Box<dyn IsMenuItem<Wry>> = match e.kind.as_str() {
            "sep" => Box::new(PredefinedMenuItem::separator(app)?),
            "native" => {
                let item = native(app, &e.id, &e.label)?;
                // 系统会把「窗口」菜单里补上窗口列表
                if matches!(e.id.as_str(), "minimize" | "zoom") {
                    s.set_as_windows_menu_for_nsapp()?;
                }
                Box::new(item)
            }
            _ => Box::new(MenuItem::with_id(app, e.id.as_str(), &e.label, e.enabled, e.accel.as_deref())?),
        };
        s.append(item.as_ref())?;
    }
    if let Some(s) = sub {
        menu.append(&s)?;
    }
    Ok(menu)
}

fn native(app: &AppHandle, id: &str, label: &str) -> tauri::Result<PredefinedMenuItem<Wry>> {
    let t = Some(label);
    match id {
        "about" => PredefinedMenuItem::about(app, t, Some(AboutMetadata::default())),
        "services" => PredefinedMenuItem::services(app, t),
        "hide" => PredefinedMenuItem::hide(app, t),
        "hideOthers" => PredefinedMenuItem::hide_others(app, t),
        "showAll" => PredefinedMenuItem::show_all(app, t),
        "quit" => PredefinedMenuItem::quit(app, t),
        "undo" => PredefinedMenuItem::undo(app, t),
        "redo" => PredefinedMenuItem::redo(app, t),
        "cut" => PredefinedMenuItem::cut(app, t),
        "copy" => PredefinedMenuItem::copy(app, t),
        "paste" => PredefinedMenuItem::paste(app, t),
        "selectAll" => PredefinedMenuItem::select_all(app, t),
        "minimize" => PredefinedMenuItem::minimize(app, t),
        "zoom" => PredefinedMenuItem::maximize(app, t),
        "fullscreen" => PredefinedMenuItem::fullscreen(app, t),
        // 服务端已按 shell::NATIVE 校验过，走到这里是两边名单没对齐
        _ => PredefinedMenuItem::separator(app),
    }
}

pub fn on_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    let Some(w) = app.get_webview_window(MAIN) else { return };
    if id == "shell.hide" {
        let _ = w.hide();
        return;
    }
    // id 要拼进脚本：服务端校验过字符集，这里再挡一次
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
        return;
    }
    let _ = w.eval(format!("window.dispatchEvent(new CustomEvent('subo:menu', {{ detail: '{id}' }}))"));
}

/// 窗口被 ⌘W 藏起来后，点 Dock 图标再叫出来
pub fn reopen(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
