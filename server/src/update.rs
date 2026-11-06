//! 软件更新：查新版本、后台下载、重启生效。
//!
//! 真正的查、下、装由桌面壳做（Tauri 的 updater 插件，见 desktop/src/update.rs），这里管
//! 设置、状态和定时检查，前端只跟本地服务打交道。命令行版没有壳，接口回 `supported: false`。
//!
//! 定时检查：启动半分钟后查一次，之后每小时看一眼，距上次查过满一天才再查。后台查失败（多半是没联网）
//! 不打扰用户，下个钟头再试；用户在设置页点「检查更新」失败才显示错误。

use crate::{fail::Fail, Shared};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub type BoxFut<T> = Pin<Box<dyn Future<Output = Result<T, Fail>> + Send>>;

/// 查到的新版本
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    /// 更新说明（Markdown）
    pub notes: Option<String>,
    /// 发布时间，RFC 3339
    pub date: Option<String>,
}

/// 下载进度回调：（已下字节，总字节）
pub type Progress = Box<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// 桌面壳交来的更新能力
pub trait Shell: Send + Sync {
    /// 打包时写进去的默认版本清单地址
    fn default_source(&self) -> String;
    /// `source` 为空用默认地址。没有新版本回 `Ok(None)`
    fn check(&self, source: String) -> BoxFut<Option<Release>>;
    /// 下载上一次 `check` 查到的版本并验签。
    /// macOS / Linux 下完直接装好（替换应用文件，重启生效）；Windows 先存着，`apply` 时再跑安装程序
    fn download(&self, progress: Progress) -> BoxFut<()>;
    /// 让新版本生效：macOS / Linux 重启应用；Windows 运行安装程序并退出
    fn apply(&self) -> Result<(), Fail>;
}

pub type Hook = Arc<dyn Shell>;

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// 本次启动还没查过
    Idle,
    Checking,
    /// 已是最新版本
    Latest,
    /// 有新版本，还没下载
    Available,
    Downloading,
    /// 新版本已就绪，重启生效
    Ready,
    /// 用户手动检查、下载或重启时出错，`error` 是原因
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    phase: Phase,
    /// 查到的新版本：available / downloading / ready 时有；下载失败时也留着，好重试
    release: Option<Release>,
    received: u64,
    total: Option<u64>,
    /// 上次查成功的时间（Unix 秒），只记本次启动内的
    checked_at: Option<u64>,
    /// failed 时的错误，形状同其他接口的错误体 `{ code, params, message }`
    error: Option<Value>,
}

pub struct Updates {
    hook: Option<Hook>,
    status: Mutex<Status>,
}

impl Updates {
    pub fn new(hook: Option<Hook>) -> Self {
        let status = Status { phase: Phase::Idle, release: None, received: 0, total: None, checked_at: None, error: None };
        Updates { hook, status: Mutex::new(status) }
    }

    fn get(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn set(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
    }
}

const DAY: u64 = 24 * 3600;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn fail_json(e: &Fail) -> Value {
    json!({ "code": e.code, "params": e.params, "message": e.message })
}

/// 查一次。`manual`：用户点的，失败要显示；后台查的失败就退回原状态，下次再试
async fn check(s: &Shared, manual: bool) -> Result<Status, Fail> {
    let hook = s.updates.hook.clone().ok_or_else(unsupported)?;
    let before = {
        let mut st = s.updates.status.lock().unwrap();
        match st.phase {
            // 正在下、已下好：不再查，免得把进度冲掉
            Phase::Checking | Phase::Downloading | Phase::Ready => return Ok(st.clone()),
            _ => {}
        }
        let before = st.clone();
        st.phase = Phase::Checking;
        st.error = None;
        before
    };
    let source = s.prefs().update_source.clone();
    match hook.check(source).await {
        Ok(found) => {
            s.updates.set(|st| {
                st.phase = if found.is_some() { Phase::Available } else { Phase::Latest };
                st.release = found;
                st.checked_at = Some(now());
            });
            Ok(s.updates.get())
        }
        Err(e) => {
            s.updates.set(|st| {
                if manual {
                    st.phase = Phase::Failed;
                    st.error = Some(fail_json(&e));
                } else {
                    *st = before;
                }
            });
            Err(e)
        }
    }
}

/// 开始后台下载查到的版本，立即返回
fn start_download(s: &Shared) -> Result<Status, Fail> {
    let hook = s.updates.hook.clone().ok_or_else(unsupported)?;
    {
        let mut st = s.updates.status.lock().unwrap();
        match (st.phase, &st.release) {
            (Phase::Downloading | Phase::Ready, _) => return Ok(st.clone()),
            (Phase::Available | Phase::Failed, Some(_)) => {}
            _ => return Err(Fail::new("update.nothingToDownload", "还没查到新版本，先检查更新")),
        }
        st.phase = Phase::Downloading;
        st.received = 0;
        st.total = None;
        st.error = None;
    }
    let s2 = s.clone();
    let progress: Progress = Box::new(move |got, total| {
        s2.updates.set(|st| {
            st.received = got;
            st.total = total;
        })
    });
    let s2 = s.clone();
    tokio::spawn(async move {
        let res = hook.download(progress).await;
        s2.updates.set(|st| match res {
            Ok(()) => st.phase = Phase::Ready,
            Err(e) => {
                st.phase = Phase::Failed;
                st.error = Some(fail_json(&e));
            }
        });
    });
    Ok(s.updates.get())
}

fn unsupported() -> Fail {
    Fail::new("update.unsupported", "这个版本不能自动更新，请到官网下载新版本")
}

/// 定时检查。没有壳（命令行版、浏览器里调试）就不跑
pub(crate) fn spawn(s: Shared) {
    if s.updates.hook.is_none() {
        return;
    }
    tokio::spawn(async move {
        // 先让启动时的事（扫字体、开项目）做完
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let (on, auto) = {
                let p = s.prefs();
                (p.update_check, p.update_download)
            };
            let st = s.updates.get();
            if on && st.checked_at.is_none_or(|t| now() >= t + DAY) {
                let _ = check(&s, false).await;
            }
            // 查到了、或者先查到后才打开「自动下载」：都在这里接着下
            if on && auto && s.updates.get().phase == Phase::Available {
                let _ = start_download(&s);
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

fn view(s: &Shared) -> Value {
    json!({
        "supported": s.updates.hook.is_some(),
        "current": env!("CARGO_PKG_VERSION"),
        "defaultSource": s.updates.hook.as_ref().map(|h| h.default_source()),
        "status": s.updates.get(),
    })
}

/// 设置页和首页看：能不能自动更新、当前版本、默认清单地址、更新状态
pub(crate) async fn get_update(State(s): State<Shared>) -> Response {
    Json(view(&s)).into_response()
}

/// 立即检查。查完才返回（取一份小 JSON，几秒内）
pub(crate) async fn post_check(State(s): State<Shared>) -> Response {
    match check(&s, true).await {
        Ok(_) => Json(view(&s)).into_response(),
        Err(e) if e.code == "update.unsupported" => e.status(StatusCode::NOT_IMPLEMENTED),
        Err(e) => e.status(StatusCode::BAD_GATEWAY),
    }
}

/// 开始下载，立即返回；进度从 `GET /api/update` 看
pub(crate) async fn post_download(State(s): State<Shared>) -> Response {
    match start_download(&s) {
        Ok(_) => Json(view(&s)).into_response(),
        Err(e) if e.code == "update.unsupported" => e.status(StatusCode::NOT_IMPLEMENTED),
        Err(e) => e.status(StatusCode::CONFLICT),
    }
}

/// 重启（Windows 上是运行安装程序）让新版本生效。先回响应，稍后再动手，前端才收得到
pub(crate) async fn post_apply(State(s): State<Shared>) -> Response {
    let Some(hook) = s.updates.hook.clone() else {
        return unsupported().status(StatusCode::NOT_IMPLEMENTED);
    };
    if s.updates.get().phase != Phase::Ready {
        return Fail::new("update.notReady", "新版本还没下载好").status(StatusCode::CONFLICT);
    }
    let s2 = s.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Err(e) = hook.apply() {
            s2.updates.set(|st| {
                st.phase = Phase::Failed;
                st.error = Some(fail_json(&e));
            });
        }
    });
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_camel_case() {
        let u = Updates::new(None);
        let v = serde_json::to_value(u.get()).unwrap();
        assert_eq!(v["phase"], "idle");
        assert!(v.get("checkedAt").is_some());
    }
}
