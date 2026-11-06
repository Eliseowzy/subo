//! 返回给界面的错误：稳定的错误码 + 参数，由前端按当前语言渲染。
//!
//! 响应体统一是 `{ code, params, message }`：`code` 对应前端语言文件里的 `server.<code>`，
//! `params` 填进文案的占位符，`message` 是中文兜底（前端认不出这个码时显示它）。
//! 新增错误时先在 app/src/i18n/locales/zh-CN.ts 的 `server` 分组里加同名键。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Map, Value};
use std::fmt;

#[derive(Debug, Clone)]
pub struct Fail {
    pub code: &'static str,
    pub params: Map<String, Value>,
    pub message: String,
}

impl Fail {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Fail { code, params: Map::new(), message: message.into() }
    }

    /// 带一个参数，键名与文案里的 `{key}` 一致
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.params.insert(key.to_string(), value.into());
        self
    }

    /// 底层错误原文（多为系统给的英文），放进 `{detail}`
    pub fn detail(self, e: impl fmt::Display) -> Self {
        self.with("detail", e.to_string())
    }

    pub fn status(self, status: StatusCode) -> Response {
        let body = json!({ "code": self.code, "params": self.params, "message": self.message });
        (status, Json(body)).into_response()
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// 后台任务 panic 之类，用户也做不了什么
pub fn internal() -> Fail {
    Fail::new("internal", "内部错误")
}
