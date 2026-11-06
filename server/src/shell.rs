//! 桌面壳的原生菜单。
//!
//! 菜单文案跟界面语言走，而语言只有前端知道（含用户导入的语言包），所以由前端翻译好整份菜单
//! 交过来，服务端只校验形状再转给壳去画。命令行版没有壳，`hook` 为空，收到就丢掉。
//! 点了菜单项由壳直接通知窗口（见 desktop/src/main.rs），不经过这里。

use crate::fail::Fail;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

/// 菜单的一项。整份菜单是这些项的平铺列表，按顺序读：
/// - `menu`：开始一个顶层菜单，`label` 是菜单栏上的名字，后面的项都归它，直到下一个 `menu`
/// - `item`：普通菜单项，点了把 `id` 发回前端；`accel` 是快捷键，写法如 `CmdOrCtrl+Shift+J`
/// - `native`：系统自带的项（复制、粘贴、隐藏、退出……），`id` 取 [`NATIVE`] 里的名字，只换文字
/// - `sep`：分隔线
#[derive(Debug, Clone, Deserialize)]
pub struct MenuEntry {
    pub kind: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub accel: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// 壳认得的系统菜单项
pub const NATIVE: &[&str] = &[
    "about", "services", "hide", "hideOthers", "showAll", "quit", "undo", "redo", "cut", "copy", "paste",
    "selectAll", "minimize", "zoom", "fullscreen",
];

/// 把整份菜单交给壳；在请求线程里调用，壳自己负责切到主线程
pub type MenuHook = Arc<dyn Fn(Vec<MenuEntry>) -> Result<(), Fail> + Send + Sync>;

const MAX_ENTRIES: usize = 200;
const MAX_LABEL: usize = 80;

fn check(entries: &[MenuEntry]) -> Result<(), Fail> {
    let bad = |i: usize, why: &str| {
        Fail::new("shell.menuInvalid", format!("菜单第 {} 项不对：{why}", i + 1)).with("index", i + 1).with("why", why)
    };
    if entries.len() > MAX_ENTRIES {
        return Err(bad(MAX_ENTRIES, "项数太多"));
    }
    if entries.first().is_some_and(|e| e.kind != "menu") {
        return Err(bad(0, "第一项要是 menu"));
    }
    for (i, e) in entries.iter().enumerate() {
        match e.kind.as_str() {
            "sep" => continue,
            "menu" | "item" | "native" => {}
            _ => return Err(bad(i, "kind 只能是 menu / item / native / sep")),
        }
        if e.label.trim().is_empty() || e.label.chars().count() > MAX_LABEL {
            return Err(bad(i, "label 为空或太长"));
        }
        // id 会拼进发回窗口的脚本里（壳那边另有转义），这里先卡死字符集
        let id_ok = !e.id.is_empty() && e.id.len() <= 64 && e.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
        if e.kind != "menu" && !id_ok {
            return Err(bad(i, "id 只能用字母、数字和点"));
        }
        if e.kind == "native" && !NATIVE.contains(&e.id.as_str()) {
            return Err(bad(i, "不认得的系统菜单项"));
        }
    }
    Ok(())
}

pub(crate) async fn put_menu(State(s): State<crate::Shared>, Json(entries): Json<Vec<MenuEntry>>) -> Response {
    let Some(hook) = s.menu.clone() else {
        return StatusCode::NO_CONTENT.into_response();
    };
    if let Err(e) = check(&entries) {
        return e.status(StatusCode::BAD_REQUEST);
    }
    match hook(entries) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(kind: &str, id: &str, label: &str) -> MenuEntry {
        MenuEntry { kind: kind.into(), id: id.into(), label: label.into(), accel: None, enabled: true }
    }

    #[test]
    fn accepts_a_normal_menu() {
        let m = [e("menu", "", "文件"), e("item", "file.save", "保存"), e("sep", "", ""), e("native", "copy", "拷贝")];
        assert!(check(&m).is_ok());
    }

    #[test]
    fn rejects_bad_shapes() {
        assert!(check(&[e("item", "a", "A")]).is_err());
        assert!(check(&[e("menu", "", "M"), e("item", "a\");x(\"", "A")]).is_err());
        assert!(check(&[e("menu", "", "M"), e("native", "rm", "A")]).is_err());
        assert!(check(&[e("menu", "", "M"), e("item", "a", " ")]).is_err());
    }
}
