//! AI 辅助：模型服务由用户自己配（BYOK），只说 OpenAI 兼容协议。
//!
//! 配置在 `<subo 目录>/ai.json`：若干「服务」（名称、Base URL）和挂在服务下的「模型」（显示名、模型 ID）。
//! API Key 不进这个文件，存系统钥匙串（secret.rs）。
//!
//! 请求只从这里发，前端不直连：Key 不进前端，也不受 CORS 限制。HTTP 交给 curl（和下载源测速一样），
//! 地址、Key、请求体都经 `-K -` 从标准输入给，不出现在进程参数里。只用各家都支持的最小字段：
//! `model`、`messages`、`stream`、`temperature`、`max_tokens`。带图的消息按 OpenAI 的分段格式发
//! （`content` 是 `text` + `image_url` 的数组），只发给用户勾了「能看图」的模型。
//!
//! 模型有先后：列表里第一个是首选，用不了（连不上、报错、没返回内容）就按顺序换下一个，
//! 前提是还没往前端吐出任何内容。
//!
//! 只在用户触发时发请求，没有后台调用。

use crate::{engine, fail::Fail, probe, secret, Shared};
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    convert::Infallible,
    fs,
    path::{Path as FsPath, PathBuf},
    sync::Mutex,
    time::Instant,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// 服务名、模型显示名的长度上限（字）
pub const NAME_MAX: usize = 60;
/// 模型 ID 的长度上限
const MODEL_ID_MAX: usize = 200;
const SERVICES_MAX: usize = 32;
const MODELS_MAX: usize = 256;
/// 一次请求里所有消息加起来的上限（字）。选区 + 上下文远到不了，防误传整个项目
const CONTENT_MAX: usize = 400_000;
/// 一次请求最多带几张图
pub const IMAGES_MAX: usize = 4;
/// 一张图（data URL）的上限（字节）。前端会先缩到长边 1600px，正常远到不了
pub const IMAGE_MAX: usize = 3 * 1024 * 1024;
/// 所有图加起来的上限。请求体整个经 curl 配置的一行传过去，新版 curl 一行最多 10MB
const IMAGES_TOTAL: usize = 6 * 1024 * 1024;
/// 生成一次最多等多久（秒）；推理模型想得久，给宽一点
const STREAM_SECS: u64 = 300;
/// 测试连接最多等多久（秒）
const TEST_SECS: u64 = 20;
/// 测试识图最多等多久（秒）；本地视觉模型第一次加载慢
const VISION_SECS: u64 = 90;
/// curl 在正文后面补一行状态码，以它开头
const MARK: &str = "@@subo-http@@";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub services: Vec<Service>,
    /// 有先后：第一个是首选（⌘K 默认选它），出错时依次往后换
    pub models: Vec<Model>,
    /// 建议先在临时副本上编译一遍，确认没有新增错误再交给用户
    pub verify: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { services: Vec::new(), models: Vec::new(), verify: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    /// 界面生成的标识，也是钥匙串里的账户名
    pub id: String,
    pub name: String,
    /// 规范化过：去掉结尾的 `/` 和 `/chat/completions`
    pub base_url: String,
    /// Key 存在哪；没填就是 `none`（本地服务通常不要 Key）
    #[serde(default)]
    pub key: secret::Store,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    /// 所属服务（`Service.id`）
    pub service: String,
    /// 界面上显示的名字
    pub name: String,
    /// 请求里的 `model` 字段，原样透传
    pub model: String,
    /// 能看图。只有「检测」通过（test_vision）才会变成 true；保存配置时只能取消，换了模型 ID 或服务也会清掉。
    /// 不能看图的模型收不到图
    #[serde(default)]
    pub vision: bool,
}

pub struct Ai {
    file: PathBuf,
    config: Mutex<Config>,
    /// 读过的 Key，一次启动只问钥匙串一次（每次都问会慢，也可能反复弹授权框）
    keys: Mutex<HashMap<String, String>>,
}

impl Ai {
    pub fn open(file: PathBuf) -> Self {
        let config = fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).map(upgrade).unwrap_or_default();
        Ai { file, config: Mutex::new(config), keys: Mutex::new(HashMap::new()) }
    }

    fn save(&self, c: &Config) -> Result<(), Fail> {
        write_atomic(&self.file, c).map_err(|e| Fail::new("ai.saveFailed", "AI 设置保存失败").detail(e))
    }

    /// 取某个服务的 Key；阻塞（可能要问钥匙串）
    fn key(&self, svc: &Service) -> Result<Option<String>, Fail> {
        if svc.key == secret::Store::None {
            return Ok(None);
        }
        if let Some(k) = self.keys.lock().unwrap().get(&svc.id) {
            return Ok(Some(k.clone()));
        }
        let k = secret::get(&svc.id, svc.key).map_err(|e| Fail::new("ai.keychain", "读不出 API Key").detail(e))?;
        if let Some(k) = &k {
            self.keys.lock().unwrap().insert(svc.id.clone(), k.clone());
        }
        Ok(k)
    }
}

/// 旧配置用 `defaultModel` 标默认模型；现在顺序就是优先级，把它挪到最前面
fn upgrade(mut v: Value) -> Config {
    if let (Some(d), Some(models)) = (v.get("defaultModel").and_then(|d| d.as_str()).map(str::to_string), v.get_mut("models").and_then(|m| m.as_array_mut())) {
        if let Some(i) = models.iter().position(|m| m.get("id").and_then(|x| x.as_str()) == Some(d.as_str())) {
            let m = models.remove(i);
            models.insert(0, m);
        }
    }
    serde_json::from_value(v).unwrap_or_default()
}

fn write_atomic(file: &FsPath, c: &Config) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, serde_json::to_vec_pretty(c).unwrap_or_default())?;
    fs::rename(&tmp, file)
}

/* ---------- 校验 ---------- */

fn check_id(id: &str) -> Result<(), Fail> {
    let ok = !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(Fail::new("ai.badId", "标识不合法"))
    }
}

fn check_name(name: &str) -> Result<String, Fail> {
    let name = name.trim();
    let n = name.chars().count();
    if n == 0 {
        return Err(Fail::new("ai.emptyName", "名称不能为空"));
    }
    if n > NAME_MAX {
        return Err(Fail::new("name.tooLong", "名称太长").with("count", n).with("max", NAME_MAX));
    }
    Ok(name.to_string())
}

/// 用户填的 Base URL：去掉结尾的 `/`，顺手去掉误填的 `/chat/completions`；只认 http(s)
pub fn normalize_url(raw: &str) -> Result<String, Fail> {
    let mut url = raw.trim().trim_end_matches('/').to_string();
    for tail in ["/chat/completions", "/completions"] {
        if url.ends_with(tail) {
            url.truncate(url.len() - tail.len());
        }
    }
    let url = url.trim_end_matches('/').to_string();
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    let ok = rest.is_some_and(|r| !r.is_empty() && !r.starts_with('/')) && !url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"');
    if ok {
        Ok(url)
    } else {
        Err(Fail::new("ai.badUrl", "地址要以 http:// 或 https:// 开头").with("url", raw.trim()))
    }
}

fn check_model_id(model: &str) -> Result<String, Fail> {
    let m = model.trim();
    if m.is_empty() || m.len() > MODEL_ID_MAX || m.chars().any(|c| c.is_control()) {
        return Err(Fail::new("ai.badModel", "模型 ID 不合法").with("model", m));
    }
    Ok(m.to_string())
}

/* ---------- 接口：配置 ---------- */

fn view(s: &Shared) -> Value {
    let c = s.ai.config.lock().unwrap().clone();
    json!({
        "config": c,
        "keychain": secret::keychain_available(),
        "nameMax": NAME_MAX,
    })
}

pub async fn get_ai(State(s): State<Shared>) -> Response {
    Json(view(&s)).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceIn {
    id: String,
    name: String,
    base_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigIn {
    services: Vec<ServiceIn>,
    models: Vec<Model>,
    verify: bool,
}

/// 整份替换（不含 Key）。删掉的服务连钥匙串里的 Key 一起删
pub async fn put_ai(State(s): State<Shared>, Json(input): Json<ConfigIn>) -> Response {
    let removed = {
        let mut cur = s.ai.config.lock().unwrap();
        let next = match merge(&cur, input) {
            Ok(n) => n,
            Err(f) => return f.status(StatusCode::BAD_REQUEST),
        };
        let removed: Vec<Service> = cur.services.iter().filter(|o| !next.services.iter().any(|n| n.id == o.id)).cloned().collect();
        if let Err(f) = s.ai.save(&next) {
            return f.status(StatusCode::INTERNAL_SERVER_ERROR);
        }
        *cur = next;
        removed
    };
    if !removed.is_empty() {
        {
            let mut keys = s.ai.keys.lock().unwrap();
            for svc in &removed {
                keys.remove(&svc.id);
            }
        }
        let _ = tokio::task::spawn_blocking(move || {
            for svc in removed {
                secret::remove(&svc.id, svc.key);
            }
        })
        .await;
    }
    Json(view(&s)).into_response()
}

fn merge(cur: &Config, input: ConfigIn) -> Result<Config, Fail> {
    if input.services.len() > SERVICES_MAX || input.models.len() > MODELS_MAX {
        return Err(Fail::new("ai.tooMany", "服务或模型太多"));
    }
    let mut services: Vec<Service> = Vec::new();
    for s in input.services {
        check_id(&s.id)?;
        if services.iter().any(|x| x.id == s.id) {
            return Err(Fail::new("ai.badId", "标识重复"));
        }
        let key = cur.services.iter().find(|o| o.id == s.id).map(|o| o.key).unwrap_or_default();
        services.push(Service { id: s.id, name: check_name(&s.name)?, base_url: normalize_url(&s.base_url)?, key });
    }
    let mut models: Vec<Model> = Vec::new();
    for m in input.models {
        check_id(&m.id)?;
        if models.iter().any(|x| x.id == m.id) {
            return Err(Fail::new("ai.badId", "标识重复"));
        }
        if !services.iter().any(|s| s.id == m.service) {
            return Err(Fail::new("ai.unknownService", "模型所属的服务不存在"));
        }
        let model = check_model_id(&m.model)?;
        // 显示名不填就用模型 ID
        let name = if m.name.trim().is_empty() { model.clone() } else { check_name(&m.name)? };
        // 能看图只认检测结果：原来就是能看图的同一个模型才保留，这里不能凭空设上
        let vision = m.vision && cur.models.iter().any(|o| o.id == m.id && o.vision && o.service == m.service && o.model == model);
        models.push(Model { id: m.id, service: m.service, name, model, vision });
    }
    Ok(Config { services, models, verify: input.verify })
}

#[derive(Deserialize)]
pub struct KeyIn {
    key: String,
}

/// 设置或清除（空串）某个服务的 Key。Key 只进不出：接口从不返回它
pub async fn put_key(State(s): State<Shared>, Path(id): Path<String>, Json(input): Json<KeyIn>) -> Response {
    let Some(svc) = s.ai.config.lock().unwrap().services.iter().find(|x| x.id == id).cloned() else {
        return Fail::new("ai.noService", "没有这个服务").status(StatusCode::NOT_FOUND);
    };
    let key = input.key.trim().to_string();
    if key.chars().any(|c| c.is_control()) || key.len() > 4096 {
        return Fail::new("ai.badKey", "API Key 不合法").status(StatusCode::BAD_REQUEST);
    }
    let account = svc.id.clone();
    let old = svc.key;
    let k = key.clone();
    let stored = tokio::task::spawn_blocking(move || {
        if k.is_empty() {
            secret::remove(&account, old);
            Ok(secret::Store::None)
        } else {
            // 换了存放位置（文件 → 钥匙串）时，旧的那份由 secret::set 清掉；反过来要自己删
            let r = secret::set(&account, &k);
            if let Ok(to) = r {
                if old != secret::Store::None && old != to {
                    secret::remove(&account, old);
                }
            }
            r
        }
    })
    .await;
    let stored = match stored {
        Ok(Ok(st)) => st,
        Ok(Err(e)) => return Fail::new("ai.keychain", "API Key 保存失败").detail(e).status(StatusCode::INTERNAL_SERVER_ERROR),
        Err(_) => return crate::fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    };
    {
        let mut keys = s.ai.keys.lock().unwrap();
        if key.is_empty() {
            keys.remove(&id);
        } else {
            keys.insert(id.clone(), key);
        }
    }
    let mut cur = s.ai.config.lock().unwrap();
    let mut next = cur.clone();
    if let Some(x) = next.services.iter_mut().find(|x| x.id == id) {
        x.key = stored;
    }
    if let Err(f) = s.ai.save(&next) {
        return f.status(StatusCode::INTERNAL_SERVER_ERROR);
    }
    *cur = next;
    drop(cur);
    Json(view(&s)).into_response()
}

/* ---------- curl ---------- */

struct Call {
    url: String,
    key: Option<String>,
    body: Option<Value>,
    secs: u64,
}

/// curl 配置文件里的字符串：双引号里 `\`、`"`、换行要转义
fn cq(s: &str) -> String {
    let e = s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r");
    format!("\"{e}\"")
}

fn curl_config(c: &Call) -> String {
    let mut out = String::new();
    let mut push = |k: &str, v: &str| {
        out.push_str(k);
        out.push_str(" = ");
        out.push_str(&cq(v));
        out.push('\n');
    };
    push("url", &c.url);
    push("user-agent", concat!("Subo/", env!("CARGO_PKG_VERSION")));
    if let Some(k) = c.key.as_deref().filter(|k| !k.is_empty()) {
        push("header", &format!("Authorization: Bearer {k}"));
    }
    if let Some(b) = &c.body {
        push("header", "Content-Type: application/json");
        // 请求体以 `{` 开头，不会被当成 `@文件`
        push("data-binary", &b.to_string());
    }
    // 正文后补一行状态码。这里的 \n 要让 curl 自己解释，不能经过 cq 再转义一次
    out.push_str(&format!("write-out = \"\\n{MARK}%{{http_code}}\\n\"\n"));
    out
}

async fn spawn(c: &Call) -> Result<tokio::process::Child, Fail> {
    let curl = engine::which("curl").ok_or_else(|| Fail::new("ai.noCurl", "找不到 curl"))?;
    let secs = c.secs.to_string();
    let mut child = tokio::process::Command::from(engine::command(curl))
        .args(["-sS", "-N", "--connect-timeout", "10", "--max-time", &secs, "-K", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Fail::new("ai.unreachable", "连不上模型服务").detail(e))?;
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(curl_config(c).as_bytes())
        .await
        .map_err(|e| Fail::new("ai.unreachable", "连不上模型服务").detail(e))?;
    drop(stdin);
    Ok(child)
}

/// curl 自己失败了（连不上、超时、证书不对）
fn curl_failure(code: Option<i32>, stderr: &str) -> Fail {
    let detail = stderr.trim().trim_start_matches("curl: ").to_string();
    match code {
        Some(28) => Fail::new("ai.timeout", "模型服务响应超时"),
        _ => Fail::new("ai.unreachable", "连不上模型服务").with("detail", detail),
    }
}

/// 服务端返回的错误体里挑一句人话：OpenAI 风格 `error.message`，也有直接给 `error` / `message` / `detail` 的
fn error_message(body: &str) -> String {
    let v: Option<Value> = serde_json::from_str(body.trim()).ok();
    let pick = v.as_ref().and_then(|v| {
        v.pointer("/error/message")
            .or_else(|| v.get("error").filter(|e| e.is_string()))
            .or_else(|| v.get("message"))
            .or_else(|| v.get("detail"))
            .and_then(|m| m.as_str())
            .map(str::to_string)
    });
    let text = pick.unwrap_or_else(|| body.trim().to_string());
    let mut short: String = text.chars().take(300).collect();
    if short.len() < text.len() {
        short.push('…');
    }
    short
}

fn http_failure(status: u16, body: &str) -> Fail {
    Fail::new("ai.http", "模型服务返回错误").with("status", status).with("detail", error_message(body))
}

/// 等 curl 跑完，拿状态码和正文（不流式：测试连接用）
async fn fetch(c: &Call) -> Result<(u16, String), Fail> {
    let child = spawn(c).await?;
    let out = child.wait_with_output().await.map_err(|e| Fail::new("ai.unreachable", "连不上模型服务").detail(e))?;
    if !out.status.success() {
        return Err(curl_failure(out.status.code(), &String::from_utf8_lossy(&out.stderr)));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, status) = match text.rfind(&format!("\n{MARK}")) {
        Some(i) => (&text[..i], text[i + 1 + MARK.len()..].trim().parse().unwrap_or(0)),
        None => (&text[..], 0),
    };
    Ok((status, body.to_string()))
}

/* ---------- 接口：测试连接 ---------- */

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct TestIn {
    /// 服务不支持 `/models` 时用这个模型 ID 发一条极小的请求；不给就用这个服务下的第一个模型
    model: Option<String>,
}

/// 先试 `GET /models`（顺便把模型列表带回去给界面挑），不支持再发一个只要 1 个 token 的 completion
pub async fn test(State(s): State<Shared>, Path(id): Path<String>, Json(input): Json<TestIn>) -> Response {
    let (svc, fallback) = {
        let c = s.ai.config.lock().unwrap();
        let Some(svc) = c.services.iter().find(|x| x.id == id).cloned() else {
            return Fail::new("ai.noService", "没有这个服务").status(StatusCode::NOT_FOUND);
        };
        let fallback = input.model.filter(|m| !m.trim().is_empty()).or_else(|| c.models.iter().find(|m| m.service == id).map(|m| m.model.clone()));
        (svc, fallback)
    };
    let ai = s.clone();
    let svc2 = svc.clone();
    let key = match tokio::task::spawn_blocking(move || ai.ai.key(&svc2)).await {
        Ok(Ok(k)) => k,
        Ok(Err(f)) => return f.status(StatusCode::BAD_GATEWAY),
        Err(_) => return crate::fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let started = Instant::now();
    let list = Call { url: format!("{}/models", svc.base_url), key: key.clone(), body: None, secs: TEST_SECS };
    let (status, body) = match fetch(&list).await {
        Ok(r) => r,
        Err(f) => return f.status(StatusCode::BAD_GATEWAY),
    };
    if status == 401 || status == 403 {
        return http_failure(status, &body).status(StatusCode::BAD_GATEWAY);
    }
    if status == 200 {
        if let Some(models) = model_ids(&body) {
            let ms = started.elapsed().as_millis() as u64;
            return Json(json!({ "ok": true, "ms": ms, "via": "models", "models": models })).into_response();
        }
    }
    // 不支持 /models：拿一个模型实际问一句
    let Some(model) = fallback else {
        return Fail::new("ai.testNeedsModel", "这个服务列不出模型，先添加一个模型再测试").status(StatusCode::BAD_REQUEST);
    };
    let chat = Call {
        url: format!("{}/chat/completions", svc.base_url),
        key,
        body: Some(json!({ "model": model, "messages": [{ "role": "user", "content": "ping" }], "max_tokens": 1, "stream": false })),
        secs: TEST_SECS,
    };
    match fetch(&chat).await {
        Ok((200, _)) => {
            let ms = started.elapsed().as_millis() as u64;
            Json(json!({ "ok": true, "ms": ms, "via": "chat", "models": [] })).into_response()
        }
        Ok((status, body)) => http_failure(status, &body).status(StatusCode::BAD_GATEWAY),
        Err(f) => f.status(StatusCode::BAD_GATEWAY),
    }
}

/// `/models` 的返回：OpenAI 是 `{ data: [{ id }] }`，有的直接给数组，Ollama 原生接口是 `{ models: [{ name }] }`
fn model_ids(body: &str) -> Option<Vec<String>> {
    let v: Value = serde_json::from_str(body.trim()).ok()?;
    let arr = v.get("data").or_else(|| v.get("models")).unwrap_or(&v).as_array()?;
    let mut ids: Vec<String> = arr
        .iter()
        .filter_map(|m| m.get("id").or_else(|| m.get("name")).and_then(|x| x.as_str()).map(str::to_string))
        .collect();
    ids.sort();
    ids.dedup();
    ids.truncate(1000);
    Some(ids)
}

/* ---------- 接口：测试识图 ---------- */

/// 发一张写着随机三位数的小图，问图里是什么数字；答对才算能看图，并把这个模型标成能看图。
/// 只看接口报不报错不够：有的服务收到图片字段不报错，直接丢掉，纯文本模型就会瞎答
pub async fn test_vision(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    let found = {
        let c = s.ai.config.lock().unwrap();
        c.models.iter().find(|m| m.id == id).cloned().and_then(|m| c.services.iter().find(|x| x.id == m.service).cloned().map(|svc| (svc, m)))
    };
    let Some((svc, model)) = found else {
        return Fail::new("ai.noModel", "还没有配置模型").status(StatusCode::NOT_FOUND);
    };
    let ai = s.clone();
    let svc2 = svc.clone();
    let key = match tokio::task::spawn_blocking(move || ai.ai.key(&svc2)).await {
        Ok(Ok(k)) => k,
        Ok(Err(f)) => return f.status(StatusCode::BAD_GATEWAY),
        Err(_) => return crate::fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let number = probe::number();
    let ask = Message::user("What number is written in this image? Reply with the digits only.").with_images(vec![probe::data_url(&number)]);
    // 推理模型先想再答，思考也算在 max_tokens 里，给宽一点
    let chat = Call {
        url: format!("{}/chat/completions", svc.base_url),
        key,
        body: Some(json!({ "model": model.model, "messages": [ask], "max_tokens": 1024, "temperature": 0, "stream": false })),
        secs: VISION_SECS,
    };
    let started = Instant::now();
    match fetch(&chat).await {
        Ok((200, body)) => {
            let v: Option<Value> = serde_json::from_str(body.trim()).ok();
            let answer = v.as_ref().and_then(delta_text).unwrap_or("").trim().to_string();
            if read_number(&answer, &number) {
                let ms = started.elapsed().as_millis() as u64;
                // 通过了就替用户勾上「能看图」；没通过不动，以用户勾的为准
                {
                    let mut cur = s.ai.config.lock().unwrap();
                    let mut next = cur.clone();
                    if let Some(m) = next.models.iter_mut().find(|m| m.id == id).filter(|m| !m.vision) {
                        m.vision = true;
                        if let Err(f) = s.ai.save(&next) {
                            return f.status(StatusCode::INTERNAL_SERVER_ERROR);
                        }
                        *cur = next;
                    }
                }
                Json(json!({ "ok": true, "ms": ms, "info": view(&s) })).into_response()
            } else {
                let short: String = answer.chars().take(40).collect();
                Fail::new("ai.visionWrong", "模型没认出图里的数字").with("expected", number).with("answer", short).status(StatusCode::BAD_GATEWAY)
            }
        }
        Ok((status, body)) => http_failure(status, &body).status(StatusCode::BAD_GATEWAY),
        Err(f) => f.status(StatusCode::BAD_GATEWAY),
    }
}

/// 回答里的数字连起来是否就是图上的数（容忍「答案是 583。」这类多说的话，也去掉推理模型的 <think> 段）
fn read_number(answer: &str, number: &str) -> bool {
    let tail = answer.rsplit("</think>").next().unwrap_or(answer);
    tail.split(|c: char| !c.is_ascii_digit()).any(|w| w == number)
}

/* ---------- 接口：生成（流式） ---------- */

#[derive(Deserialize, Clone)]
pub struct Message {
    role: String,
    pub(crate) content: String,
    /// 随这条消息附上的图片，`data:image/…;base64,…`。只有 user 消息能带
    #[serde(default)]
    pub(crate) images: Vec<String>,
}

impl Message {
    pub(crate) fn system(content: impl Into<String>) -> Self {
        Message { role: "system".into(), content: content.into(), images: Vec::new() }
    }
    pub(crate) fn user(content: impl Into<String>) -> Self {
        Message { role: "user".into(), content: content.into(), images: Vec::new() }
    }
    pub(crate) fn with_images(mut self, images: Vec<String>) -> Self {
        self.images = images;
        self
    }
}

/// 发给服务的样子：没图时 `content` 是字符串（各家都认），有图时是分段数组
impl Serialize for Message {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let content = if self.images.is_empty() {
            json!(self.content)
        } else {
            let mut parts = vec![json!({ "type": "text", "text": self.content })];
            parts.extend(self.images.iter().map(|u| json!({ "type": "image_url", "image_url": { "url": u } })));
            Value::Array(parts)
        };
        json!({ "role": self.role, "content": content }).serialize(ser)
    }
}

/// 只收常见的位图格式，且必须是 base64 的 data URL：不让服务端替前端去抓网址
fn check_image(url: &str) -> Result<(), Fail> {
    let ok = ["png", "jpeg", "webp", "gif"].iter().any(|t| url.starts_with(&format!("data:image/{t};base64,")));
    if !ok {
        return Err(Fail::new("ai.badImage", "图片格式不支持"));
    }
    if url.len() > IMAGE_MAX {
        return Err(Fail::new("ai.imageTooLarge", "图片太大").with("max", IMAGE_MAX / 1024 / 1024));
    }
    Ok(())
}

/// 消息里的图：数量、大小、格式
fn check_images(messages: &[Message]) -> Result<(), Fail> {
    let all: Vec<&String> = messages.iter().flat_map(|m| &m.images).collect();
    if all.is_empty() {
        return Ok(());
    }
    if messages.iter().any(|m| !m.images.is_empty() && m.role != "user") {
        return Err(Fail::new("ai.badRequest", "请求不完整"));
    }
    if all.len() > IMAGES_MAX {
        return Err(Fail::new("ai.tooManyImages", "图片太多").with("max", IMAGES_MAX));
    }
    for u in &all {
        check_image(u)?;
    }
    if all.iter().map(|u| u.len()).sum::<usize>() > IMAGES_TOTAL {
        return Err(Fail::new("ai.imageTooLarge", "图片太大").with("max", IMAGES_TOTAL / 1024 / 1024));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteIn {
    /// `Model.id`；不给用默认模型
    model: Option<String>,
    messages: Vec<Message>,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
}

/// 依次要试的模型：点名的（没点名就是列表第一个）打头，其余按列表顺序跟上。
/// 带图时只留能看图的；点名的那个不能看图就直接报错，不悄悄换掉
fn chain(c: &Config, want: Option<&str>, images: bool) -> Result<Vec<(Service, Model)>, Fail> {
    let first = match want {
        Some(id) => c.models.iter().position(|m| m.id == id).ok_or_else(|| Fail::new("ai.noModel", "还没有配置模型"))?,
        None => 0,
    };
    let head = c.models.get(first).ok_or_else(|| Fail::new("ai.noModel", "还没有配置模型"))?;
    if images && want.is_some() && !head.vision {
        return Err(Fail::new("ai.noVision", "这个模型不能看图").with("name", head.name.clone()));
    }
    let order = std::iter::once(head).chain(c.models.iter().enumerate().filter(|(i, _)| *i != first).map(|(_, m)| m));
    let list: Vec<(Service, Model)> = order
        .filter(|m| !images || m.vision)
        .filter_map(|m| c.services.iter().find(|s| s.id == m.service).map(|s| (s.clone(), m.clone())))
        .collect();
    if list.is_empty() {
        return Err(if images { Fail::new("ai.noVision", "这个模型不能看图").with("name", head.name.clone()) } else { Fail::new("ai.noModel", "还没有配置模型") });
    }
    Ok(list)
}

/// 流式生成。返回 NDJSON，一行一个事件：
/// `{"t":"delta","text":…}`、`{"t":"think","text":…}`（推理模型的思考过程，只给人看）、`{"t":"done","model":…}`、`{"t":"error","code":…,"params":…,"message":…}`，
/// 以及换模型时的 `{"t":"fallback","from":…,"to":…,"code":…,"params":…,"message":…}`（后几项是前一个模型出的错）。
/// 都失败了报首选模型的错：多半要改的是它的配置。
/// 开始前就能发现的问题（没配模型、读不出 Key）直接按普通错误返回。前端断开时 curl 随之结束
pub async fn complete(State(s): State<Shared>, Json(input): Json<CompleteIn>) -> Response {
    if input.messages.iter().any(|m| !matches!(m.role.as_str(), "system" | "user" | "assistant")) {
        return Fail::new("ai.badRequest", "请求不完整").status(StatusCode::BAD_REQUEST);
    }
    stream_chat(&s, input.model.as_deref(), input.messages, input.temperature, input.max_tokens).await
}

/// 给其他能力（assist.rs）用的同一条流式通道：消息由服务端拼好
pub(crate) async fn stream_chat(s: &Shared, model: Option<&str>, messages: Vec<Message>, temperature: Option<f64>, max_tokens: Option<u32>) -> Response {
    if messages.is_empty() {
        return Fail::new("ai.badRequest", "请求不完整").status(StatusCode::BAD_REQUEST);
    }
    if messages.iter().map(|m| m.content.chars().count()).sum::<usize>() > CONTENT_MAX {
        return Fail::new("ai.tooLarge", "发送的内容太多").with("max", CONTENT_MAX).status(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if let Err(f) = check_images(&messages) {
        return f.status(StatusCode::BAD_REQUEST);
    }
    let images = messages.iter().any(|m| !m.images.is_empty());
    let list = match chain(&s.ai.config.lock().unwrap(), model, images) {
        Ok(l) => l,
        Err(f) => return f.status(StatusCode::BAD_REQUEST),
    };
    let mut body = json!({ "messages": messages, "stream": true });
    if let Some(t) = temperature {
        body["temperature"] = json!(t.clamp(0.0, 2.0));
    }
    if let Some(n) = max_tokens {
        body["max_tokens"] = json!(n.clamp(1, 32_768));
    }

    let (tx, rx) = mpsc::channel::<Result<Bytes, Infallible>>(64);
    let s = s.clone();
    tokio::spawn(async move {
        let send = |v: Value| {
            let tx = tx.clone();
            async move { tx.send(Ok(Bytes::from(format!("{v}\n")))).await.is_ok() }
        };
        let mut first: Option<Fail> = None;
        let mut prev: Option<(String, Fail)> = None;
        for (svc, model) in list {
            if let Some((from, f)) = prev.take() {
                let ev = json!({ "t": "fallback", "from": from, "to": model.name, "code": f.code, "params": f.params, "message": f.message });
                if !send(ev).await {
                    return;
                }
            }
            let ai = s.clone();
            let key = match tokio::task::spawn_blocking(move || ai.ai.key(&svc).map(|k| (k, svc))).await {
                Ok(Ok(k)) => Ok(k),
                Ok(Err(f)) => Err(f),
                Err(_) => Err(crate::fail::internal()),
            };
            let result = match key {
                Ok((key, svc)) => {
                    let mut b = body.clone();
                    b["model"] = json!(model.model);
                    let call = Call { url: format!("{}/chat/completions", svc.base_url), key, body: Some(b), secs: STREAM_SECS };
                    let mut sent = false;
                    pump(&call, &tx, &mut sent).await.map_err(|f| (f, sent))
                }
                Err(f) => Err((f, false)),
            };
            match result {
                Ok(()) => {
                    send(json!({ "t": "done", "model": model.name })).await;
                    return;
                }
                // 已经吐出内容了，换模型会接着拼上另一段，不如直接报错
                Err((f, true)) => {
                    send(json!({ "t": "error", "code": f.code, "params": f.params, "message": f.message })).await;
                    return;
                }
                Err((f, false)) => {
                    if tx.is_closed() {
                        return;
                    }
                    first.get_or_insert_with(|| f.clone());
                    prev = Some((model.name, f));
                }
            }
        }
        let f = first.unwrap_or_else(crate::fail::internal);
        send(json!({ "t": "error", "code": f.code, "params": f.params, "message": f.message })).await;
    });
    (
        [(header::CONTENT_TYPE, "application/x-ndjson; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
        Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)),
    )
        .into_response()
}

/// SSE 里一块增量的文字。各家位置不全一样
fn delta_text(v: &Value) -> Option<&str> {
    v.pointer("/choices/0/delta/content")
        .or_else(|| v.pointer("/choices/0/message/content"))
        .or_else(|| v.pointer("/choices/0/text"))
        .and_then(|x| x.as_str())
}

/// 推理模型在正文之前吐出的思考过程（DeepSeek、Qwen 叫 reasoning_content，Ollama 等叫 reasoning）
fn delta_think(v: &Value) -> Option<&str> {
    v.pointer("/choices/0/delta/reasoning_content").or_else(|| v.pointer("/choices/0/delta/reasoning")).and_then(|x| x.as_str())
}

/// 读 curl 的输出，把 SSE 增量转成 NDJSON 发给前端。前端断开（接收端关掉）就返回，curl 随 child 一起被杀
/// `sent`：已经往前端发过东西（正文或思考过程）。发过了就不能再换模型重来
async fn pump(c: &Call, tx: &mpsc::Sender<Result<Bytes, Infallible>>, sent: &mut bool) -> Result<(), Fail> {
    let lost = |e: std::io::Error| Fail::new("ai.unreachable", "连不上模型服务").detail(e);
    let mut child = spawn(c).await?;
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let mut status = 0u16;
    // 不是 SSE 的部分：错误体，或者服务没理会 stream 直接回了整段 JSON
    let mut other = String::new();
    let mut got = false;
    loop {
        let line = tokio::select! {
            l = lines.next_line() => l.map_err(lost)?,
            _ = tx.closed() => return Ok(()),
        };
        let Some(line) = line else { break };
        if let Some(code) = line.strip_prefix(MARK) {
            status = code.trim().parse().unwrap_or(0);
            continue;
        }
        let l = line.trim_end_matches('\r');
        if let Some(data) = l.strip_prefix("data:") {
            let data = data.trim();
            if data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if v.get("error").is_some() {
                return Err(Fail::new("ai.remote", "模型服务返回错误").with("detail", error_message(data)));
            }
            if let Some(text) = delta_think(&v).filter(|t| !t.is_empty()) {
                let ev = json!({ "t": "think", "text": text });
                *sent = true;
                if tx.send(Ok(Bytes::from(format!("{ev}\n")))).await.is_err() {
                    return Ok(());
                }
            }
            if let Some(text) = delta_text(&v).filter(|t| !t.is_empty()) {
                got = true;
                let ev = json!({ "t": "delta", "text": text });
                *sent = true;
                if tx.send(Ok(Bytes::from(format!("{ev}\n")))).await.is_err() {
                    return Ok(());
                }
            }
        } else if l.is_empty() || l.starts_with(':') || l.starts_with("event:") || l.starts_with("id:") || l.starts_with("retry:") {
            // SSE 的空行、注释、其他字段
        } else if other.len() < 64 * 1024 {
            other.push_str(l);
            other.push('\n');
        }
    }
    let st = child.wait().await.map_err(lost)?;
    if !st.success() {
        let mut err = String::new();
        let _ = stderr.read_to_string(&mut err).await;
        return Err(curl_failure(st.code(), &err));
    }
    if !(200..300).contains(&status) {
        return Err(http_failure(status, &other));
    }
    if !got {
        if let Some(text) = serde_json::from_str::<Value>(other.trim()).ok().as_ref().and_then(delta_text).filter(|t| !t.is_empty()) {
            got = true;
            let ev = json!({ "t": "delta", "text": text });
            *sent = true;
            let _ = tx.send(Ok(Bytes::from(format!("{ev}\n")))).await;
        }
    }
    if !got {
        return Err(Fail::new("ai.empty", "模型没有返回内容"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_normalized() {
        assert_eq!(normalize_url(" https://api.deepseek.com/v1/ ").unwrap(), "https://api.deepseek.com/v1");
        assert_eq!(normalize_url("http://127.0.0.1:11434/v1/chat/completions").unwrap(), "http://127.0.0.1:11434/v1");
        assert!(normalize_url("api.openai.com/v1").is_err());
        assert!(normalize_url("https://").is_err());
        assert!(normalize_url("https://a b").is_err());
    }

    #[test]
    fn curl_config_escapes() {
        let c = Call { url: "https://x/v1".into(), key: Some("k\"\\".into()), body: Some(json!({"a": "line\nquote\""})), secs: 5 };
        let cfg = curl_config(&c);
        assert!(cfg.contains(r#"header = "Authorization: Bearer k\"\\""#));
        // JSON 里的 \n 是两个字符 `\` `n`，进配置后反斜杠要再转义
        assert!(cfg.contains(r#"data-binary = "{\"a\":\"line\\nquote\\\"\"}""#));
        assert!(cfg.ends_with("write-out = \"\\n@@subo-http@@%{http_code}\\n\"\n"));
    }

    #[test]
    fn error_bodies() {
        assert_eq!(error_message(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(error_message(r#"{"error":"nope"}"#), "nope");
        assert_eq!(error_message("plain"), "plain");
    }

    #[test]
    fn model_lists() {
        assert_eq!(model_ids(r#"{"data":[{"id":"b"},{"id":"a"}]}"#).unwrap(), vec!["a", "b"]);
        assert_eq!(model_ids(r#"{"models":[{"name":"qwen"}]}"#).unwrap(), vec!["qwen"]);
        assert!(model_ids("<html>").is_none());
    }

    #[test]
    fn think_delta() {
        let v: Value = serde_json::from_str(r#"{"choices":[{"delta":{"reasoning_content":"想想","content":null}}]}"#).unwrap();
        assert_eq!(delta_think(&v), Some("想想"));
        assert_eq!(delta_text(&v), None);
        let v: Value = serde_json::from_str(r#"{"choices":[{"delta":{"reasoning":"hmm"}}]}"#).unwrap();
        assert_eq!(delta_think(&v), Some("hmm"));
    }

    #[test]
    fn merge_keeps_key_store() {
        let cur = Config {
            services: vec![Service { id: "s1".into(), name: "A".into(), base_url: "https://a".into(), key: secret::Store::Keychain }],
            models: vec![Model { id: "m1".into(), service: "s1".into(), name: "".into(), model: "gpt".into(), vision: true }],
            ..Default::default()
        };
        let next = merge(
            &cur,
            ConfigIn {
                services: vec![ServiceIn { id: "s1".into(), name: " A2 ".into(), base_url: "https://a/v1/".into() }],
                models: vec![Model { id: "m1".into(), service: "s1".into(), name: "".into(), model: "gpt".into(), vision: true }],
                verify: false,
            },
        )
        .unwrap();
        assert_eq!(next.services[0].key, secret::Store::Keychain);
        assert_eq!(next.services[0].name, "A2");
        assert_eq!(next.services[0].base_url, "https://a/v1");
        assert_eq!(next.models[0].name, "gpt");
        assert!(next.models[0].vision);
    }

    #[test]
    fn vision_only_from_detection() {
        let svc = || vec![ServiceIn { id: "s".into(), name: "S".into(), base_url: "https://s".into() }];
        let cur = Config {
            services: vec![Service { id: "s".into(), name: "S".into(), base_url: "https://s".into(), key: secret::Store::None }],
            models: vec![model("a", true), model("b", false)],
            verify: true,
        };
        let save = |models: Vec<Model>| merge(&cur, ConfigIn { services: svc(), models, verify: true }).unwrap().models;
        // 原样保存：a 保留，b 不能凭空设上
        let m = save(vec![model("a", true), model("b", true)]);
        assert!(m[0].vision && !m[1].vision);
        // 换了模型 ID 就清掉
        let m = save(vec![Model { model: "a2".into(), ..model("a", true) }]);
        assert!(!m[0].vision);
        // 取消总是可以
        assert!(!save(vec![model("a", false)])[0].vision);
    }

    #[test]
    fn messages_with_images() {
        let plain = serde_json::to_value(Message::user("hi")).unwrap();
        assert_eq!(plain, json!({ "role": "user", "content": "hi" }));
        let m = Message::user("hi").with_images(vec!["data:image/png;base64,AAAA".into()]);
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["content"][0], json!({ "type": "text", "text": "hi" }));
        assert_eq!(v["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");

        assert!(check_images(&[Message::user("hi")]).is_ok());
        assert!(check_images(&[m]).is_ok());
        let url = Message::user("x").with_images(vec!["https://example.com/a.png".into()]);
        assert_eq!(check_images(&[url]).unwrap_err().code, "ai.badImage");
        let many = Message::user("x").with_images(vec!["data:image/png;base64,AA".into(); IMAGES_MAX + 1]);
        assert_eq!(check_images(&[many]).unwrap_err().code, "ai.tooManyImages");
    }

    fn model(id: &str, vision: bool) -> Model {
        Model { id: id.into(), service: "s".into(), name: id.to_uppercase(), model: id.into(), vision }
    }

    #[test]
    fn fallback_order() {
        let c = Config {
            services: vec![Service { id: "s".into(), name: "S".into(), base_url: "https://s".into(), key: secret::Store::None }],
            models: vec![model("a", false), model("b", true), model("c", false), model("d", true)],
            verify: true,
        };
        let ids = |l: Vec<(Service, Model)>| l.into_iter().map(|(_, m)| m.id).collect::<Vec<_>>();
        assert_eq!(ids(chain(&c, None, false).unwrap()), ["a", "b", "c", "d"]);
        // 点名的打头，其余照列表顺序
        assert_eq!(ids(chain(&c, Some("c"), false).unwrap()), ["c", "a", "b", "d"]);
        // 带图只走能看图的
        assert_eq!(ids(chain(&c, None, true).unwrap()), ["b", "d"]);
        assert_eq!(ids(chain(&c, Some("d"), true).unwrap()), ["d", "b"]);
        assert_eq!(chain(&c, Some("a"), true).unwrap_err().code, "ai.noVision");
        assert_eq!(chain(&c, Some("gone"), false).unwrap_err().code, "ai.noModel");
        assert_eq!(chain(&Config::default(), None, false).unwrap_err().code, "ai.noModel");
    }

    #[test]
    fn old_default_moves_to_front() {
        let v = json!({ "services": [], "models": [model("a", false), model("b", false)], "defaultModel": "b", "verify": false });
        let c = upgrade(v);
        assert_eq!(c.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert!(!c.verify);
    }

    #[test]
    fn vision_answers() {
        assert!(read_number("583", "583"));
        assert!(read_number("The number is 583.", "583"));
        assert!(read_number("<think>maybe 588</think>\n583", "583"));
        assert!(!read_number("5830", "583"));
        assert!(!read_number("I cannot see images.", "583"));
    }
}
