//! Subo 本地服务：项目库、文件、编译、事件推送。
//!
//! 库形态，两个入口共用：命令行 `subo-server`（`src/main.rs`）和桌面壳（`desktop/`，
//! 在自己进程里起服务，窗口指向它）。
//!
//! 用户磁盘上已有的项目在界面里「打开文件夹」登记，清单见 `folders`。

mod ai;
mod appearance;
mod assist;
mod diagnose;
mod logparse;
mod case;
pub mod cli;
mod compat;
mod engine;
mod engines;
pub mod fail;
mod files;
mod folders;
mod fontmap;
mod history;
mod lastgood;
mod library;
mod machine;
mod onboarding;
mod prescan;
mod presets;
mod probe;
mod project;
mod projprefs;
mod queue;
mod secret;
mod settings;
pub mod shell;
mod snapshot;
mod symbols;
mod synctex;
mod tarball;
mod templates;
pub mod tex;
pub mod update;
#[cfg(windows)]
mod winjob;

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Response,
    },
    routing::{any, get, post},
    Json, Router,
};
use tower_http::services::{ServeDir, ServeFile};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    panic::{catch_unwind, AssertUnwindSafe},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use fail::Fail;
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

/// 每个项目只保留最近几次编译的产物，更早的连目录一起删掉。
const KEEP_JOBS: usize = 5;
/// zip 上传的请求体上限
const UPLOAD_MAX_BYTES: usize = 200 << 20;
/// ⌘K 请求体上限：可能带几张图（单张、总量的限制在 ai.rs）
const AI_MAX_BYTES: usize = 16 << 20;
/// 离线导入的单个引擎容器上限（最大的 luahbtex 二进制约 5 MB）
const ENGINE_MAX_BYTES: usize = 64 << 20;
/// 上传导入的字盘快照上限（压缩后）；更大的放进项目里从项目导入
const SNAPSHOT_MAX_BYTES: usize = 1 << 30;

struct AppState {
    /// 项目库目录，每个子目录一个项目；设置里可以换位置。模板在它下面的隐藏目录里
    library: RwLock<PathBuf>,
    /// 启动参数给了 `--home`：位置由调用方管，设置页不能改
    library_locked: bool,
    settings: Mutex<settings::Settings>,
    settings_file: PathBuf,
    /// 上次读写 `settings_file` 时它的修改时间：命令行（`subo mirror use`）改了文件，下次取设置时重读
    settings_seen: Mutex<Option<std::time::SystemTime>>,
    /// 设置里没填编译超时时用这个（启动参数）
    default_timeout: Duration,
    /// 各项目合计正在跑的编译数，不超过设置里的并行编译数
    running: (Mutex<u64>, std::sync::Condvar),
    /// 用户打开过的本地文件夹
    folders: Mutex<folders::Folders>,
    /// 编译产物缓存，每个项目一个子目录；不往用户目录里写东西
    cache: PathBuf,
    /// 预览模板时展开的只读副本，每个模板一个子目录；同样不在项目库里，启动时清空
    previews: PathBuf,
    /// 按项目存的设置（projprefs.rs）
    projects: PathBuf,
    /// 文件的本地历史（history.rs），每个项目一个子目录
    history: PathBuf,
    engine: String,
    tx: broadcast::Sender<String>,
    case: Mutex<case::Index>,
    /// 离线时没取到的宏包、没查到包名的文件，联网后补（queue.rs）
    queue: Mutex<queue::Store>,
    /// 同一时间只跑一个 tlmgr install：它自己也有锁，撞上只会报错
    installing: Mutex<bool>,
    /// 打开过的项目，各自带编译状态；首次访问时建立
    quires: Mutex<HashMap<String, Arc<Quire>>>,
    picker: Option<Picker>,
    menu: Option<shell::MenuHook>,
    /// 这次启动该弹哪种上手引导
    onboarding: Mutex<onboarding::Onboarding>,
    /// AI 辅助的模型服务配置（ai.rs）
    ai: ai::Ai,
    /// 软件更新的状态（update.rs）
    updates: update::Updates,
}

/// 一个打开中的项目：目录 + 它自己的编译工位与产物。
struct Quire {
    id: String,
    root: PathBuf,
    /// 这个项目的编译产物目录，在 `AppState.cache` 下
    out: PathBuf,
    /// 模板预览：只能看、编译、下载 PDF，不能写文件、改名、存模板
    preview: bool,
    jobs: Mutex<Jobs>,
    press: Mutex<Press>,
}

/// 模板预览用的项目 id：`~` 加模板 id。项目库的目录名由 `slug` 起，不会带 `~`
const PREVIEW_PREFIX: char = '~';

fn preview_template(id: &str) -> Option<&str> {
    id.strip_prefix(PREVIEW_PREFIX)
}

struct Job {
    pdf: Option<PathBuf>,
    log: String,
    ok: bool,
    /// 这次编的入口：找它的 `-recorder` 记录（.fls）
    entry: String,
}

#[derive(Default)]
struct Jobs {
    map: HashMap<String, Job>,
    order: VecDeque<String>,
}

/// 同项目串行：正在编译时再收到请求，只记一个「待编」标记，
/// 当前这次结束后补跑一次。连按十次 ⌘↵ 最多多编一遍。不同项目互不阻塞。
#[derive(Default)]
struct Press {
    running: bool,
    queued: bool,
}

type Shared = Arc<AppState>;

/// 一份编译占着的并行名额，drop 时归还
struct Slot<'a>(&'a (Mutex<u64>, std::sync::Condvar));

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self.0 .0.lock().unwrap() -= 1;
        self.0 .1.notify_one();
    }
}

impl AppState {
    fn home(&self) -> PathBuf {
        self.library.read().unwrap().clone()
    }

    fn templates(&self) -> PathBuf {
        templates::dir(&self.home())
    }

    /// 当前设置。文件被别的进程（命令行）改过就先重读；项目库位置不跟着变，它要走换库的流程
    fn prefs(&self) -> std::sync::MutexGuard<'_, settings::Settings> {
        let mut cur = self.settings.lock().unwrap();
        let mut seen = self.settings_seen.lock().unwrap();
        let now = mtime(&self.settings_file);
        if now.is_some() && now != *seen {
            let library = cur.library.clone();
            *cur = settings::Settings::load(&self.settings_file);
            cur.library = library;
            *seen = now;
        }
        cur
    }

    /// 自己写完设置文件后记下修改时间，免得下次当成外部改动重读
    fn prefs_saved(&self) {
        *self.settings_seen.lock().unwrap() = mtime(&self.settings_file);
    }

    fn timeout(&self) -> Duration {
        self.prefs().timeout_secs.map(Duration::from_secs).unwrap_or(self.default_timeout)
    }

    /// 编译的资源限额：内存按设置，单文件写入上限固定
    fn limits(&self) -> engine::Limits {
        let mb = self.prefs().memory_mb.unwrap_or(settings::MEMORY_DEFAULT);
        engine::Limits { memory: mb << 20, file_size: engine::FILE_SIZE_CAP }
    }

    /// 同时最多跑几份编译：按设置，不超过本机逻辑核数
    fn parallel(&self) -> u64 {
        let cpus = machine::cpus();
        self.prefs().parallel.unwrap_or(cpus).clamp(settings::PARALLEL_MIN, cpus)
    }

    /// 等到有空闲名额再编；名额随返回值释放。设置改小了也只影响之后开跑的，正在跑的不打断
    fn slot(&self) -> Slot<'_> {
        let (n, cv) = &self.running;
        let mut cur = n.lock().unwrap();
        // 设置改大时没人通知，定时醒来再看一次
        while *cur >= self.parallel() {
            cur = cv.wait_timeout(cur, Duration::from_secs(1)).unwrap().0;
        }
        *cur += 1;
        Slot(&self.running)
    }

    /// 设置里选的安装源；`None` 用 tlmgr 自己的
    fn repository(&self) -> Option<String> {
        self.prefs().repository()
    }

    /// 软件源跨到新年度版本时要不要自动改用归档源
    fn fallback(&self) -> case::Fallback {
        self.prefs().fallback()
    }

    /// TeX Live 下这个项目用哪个程序；tectonic、mock 没得选，返回 `None`。
    /// 自动探测没有结论时：模板预览按 Subo 自己建的项目算（xelatex），其余是导入的，按 arXiv、
    /// Overleaf 的默认用 pdflatex
    fn program_for(&self, q: &Quire, entry: &str) -> Option<engine::program::Choice> {
        use engine::program::{resolve, Program};
        if self.engine != "xelatex" {
            return None;
        }
        let chosen = projprefs::load(&self.projects, &q.id).engine.as_deref().and_then(Program::parse);
        let fallback = if q.preview { Program::Xelatex } else { Program::Pdflatex };
        // 用户自己的 TeX Live 可能只装了 xelatex；内置的按需引擎装没装也看 PATH（引擎根接在内置 bin 后面）
        Some(resolve(&q.root, entry, chosen, fallback, |p| engines::available(p.name())))
    }

    /// 实际跑的引擎、要写在日志开头的说明（换了引擎时），以及缺的引擎（问题面板提示安装）
    fn engine_for(&self, q: &Quire, entry: &str) -> (&str, Option<String>, Option<engine::program::Program>) {
        use engine::program::Source;
        let Some(c) = self.program_for(q, entry) else {
            return (&self.engine, None, None);
        };
        match c.source {
            Source::Missing { wanted, .. } => {
                let note = format!("这个项目要用 {}，本机还没有装，先用 xelatex 试编。\n", wanted.name());
                (c.program.name(), Some(note), Some(wanted))
            }
            _ => (c.program.name(), None, None),
        }
    }

    /// Subo 自己建的项目（空白、模板）建好就记下引擎，不走探测：多数按 xelatex 写，日文模板用 lualatex
    fn pin_program(&self, id: &str, program: &str) {
        let prefs = projprefs::ProjectPrefs { engine: Some(program.into()) };
        if let Err(e) = projprefs::save(&self.projects, id, &prefs) {
            eprintln!("[engine] 记录项目 {id} 的引擎失败：{e}");
        }
    }

    /// 已归档的项目只能看、编译，不能改。每次现查：开着的项目也可能刚被归档或取消归档
    fn archived(&self, q: &Quire) -> bool {
        if q.preview {
            return false;
        }
        if folders::is_folder_id(&q.id) {
            self.folders.lock().unwrap().get(&q.id).is_some_and(|f| f.archived)
        } else {
            library::is_archived(&q.root)
        }
    }

    /// 模板预览、已归档项目都不许写；返回该回的错误
    fn read_only(&self, q: &Quire) -> Option<Response> {
        if q.preview {
            Some(preview_read_only())
        } else if self.archived(q) {
            Some(Fail::new("project.archivedReadOnly", "已归档的项目是只读的，要修改请先取消归档").status(StatusCode::FORBIDDEN))
        } else {
            None
        }
    }

    fn quire(&self, id: &str) -> Option<Arc<Quire>> {
        if !library::valid_id(id) {
            return None;
        }
        let mut map = self.quires.lock().unwrap();
        if let Some(q) = map.get(id) {
            return Some(q.clone());
        }
        let preview = preview_template(id).is_some();
        let root = if let Some(tpl) = preview_template(id) {
            // 每次进程里第一次预览时重新展开，模板改了分类之外的东西也是新的
            let p = self.previews.join(hash_name(tpl));
            templates::materialize(&self.templates(), tpl, &p).ok()?;
            p
        } else if folders::is_folder_id(id) {
            self.folders.lock().unwrap().get(id)?.path.clone()
        } else {
            // 旧版本把产物放在项目里的 .subo/press，项目库是自己的地盘，顺手清掉
            let p = self.home().join(id);
            let _ = std::fs::remove_dir_all(p.join(".subo").join("press"));
            let _ = std::fs::remove_dir(p.join(".subo"));
            p
        };
        if !root.is_dir() {
            return None;
        }
        let root = dunce::canonicalize(&root).ok()?;
        let out = self.out_dir(&root);
        // 上次运行留下的产物在新进程里已无从访问，首次打开时清空
        let _ = std::fs::remove_dir_all(&out);
        let q = Arc::new(Quire {
            id: id.to_string(),
            root,
            out,
            preview,
            jobs: Mutex::new(Jobs::default()),
            press: Mutex::new(Press::default()),
        });
        map.insert(id.to_string(), q.clone());
        Some(q)
    }

    /// 产物目录按项目的真实路径取哈希：改名后换新目录，两个同名文件夹也不会撞
    fn out_dir(&self, root: &std::path::Path) -> PathBuf {
        self.cache.join(hash_name(root))
    }

    /// 模板删了，预览也作废：下次同 id 的模板要重新展开
    fn drop_preview(&self, tpl: &str) {
        let id = format!("{PREVIEW_PREFIX}{tpl}");
        if let Some(q) = self.quires.lock().unwrap().remove(&id) {
            let _ = std::fs::remove_dir_all(&q.out);
            let _ = std::fs::remove_dir_all(&q.root);
        }
    }

    fn history(&self, q: &Quire) -> history::History {
        history::History::new(&self.history, &q.root)
    }

    /// 推事件，统一带上 projectId，前端据此只看当前项目的
    fn emit(&self, q: &Quire, mut ev: Value) {
        ev["projectId"] = json!(q.id);
        let _ = self.tx.send(ev.to_string());
    }
}

/// axum 自带的提取器拒绝（JSON 缺字段、不是 JSON、没带 content-type、路径参数类型不对）回的是纯文本；
/// 统一成 `{ code, params, message }`，脚本和 agent 只需按一种格式解析错误
async fn json_rejections(res: Response) -> Response {
    let plain = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/plain"));
    let status = res.status();
    if !status.is_client_error() || !plain {
        return res;
    }
    let bytes = axum::body::to_bytes(res.into_body(), 64 << 10).await.unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Fail::new("request.badBody", format!("请求格式不对：{text}")).detail(text).status(status)
}

async fn api_missing() -> Response {
    Fail::new("api.missing", "没有这个接口（前后端版本不一致？请重启 subo-server）").status(StatusCode::NOT_FOUND)
}

fn hash_name(x: impl Hash) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    x.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// 模板预览里的写操作一律拒绝
fn preview_read_only() -> Response {
    Fail::new("preview.readOnly", "模板预览是只读的，要修改请先用这个模板新建项目").status(StatusCode::FORBIDDEN)
}

fn not_found() -> Response {
    Fail::new("project.notFound", "没有这个项目").status(StatusCode::NOT_FOUND)
}

/// 请求缺参数：前端自己拼的 URL 出错，用户看到的机会不大
fn missing_param(names: &str) -> Response {
    Fail::new("request.missingParam", format!("缺少 {names}"))
        .with("names", names)
        .status(StatusCode::BAD_REQUEST)
}

fn read_failed(e: std::io::Error) -> Response {
    Fail::new("file.readFailed", format!("读取失败: {e}")).detail(e).status(StatusCode::NOT_FOUND)
}

pub use engine::{program_in, which};
pub use library::{home_dir, subo_dir};

/// 启动参数。命令行版和桌面壳各自填好再交给 [`serve`]。
pub struct Options {
    /// 项目库目录；`None` 用默认位置（并搬旧库）
    pub home: Option<PathBuf>,
    /// `auto` / `xelatex` / `tectonic` / `mock`
    pub engine: String,
    /// 前端构建产物；给了就单端口托管
    pub dist: Option<PathBuf>,
    /// 单次编译超时
    pub timeout: Duration,
    /// 「选择文件夹」对话框。桌面壳传原生对话框；`None` 时 macOS 走 osascript，其他系统返回 501
    pub picker: Option<Picker>,
    /// 原生菜单。桌面壳传自己的画菜单函数；`None` 时前端交来的菜单直接丢掉（见 shell.rs）
    pub menu: Option<shell::MenuHook>,
    /// TeX Live 是随应用内置的：包内只读，补装宏包走 `tlmgr --usermode` 装进用户层
    pub bundled_tex: bool,
    /// 软件更新。桌面壳传自己的实现；`None` 时界面上的「软件更新」显示不支持（见 update.rs）
    pub updater: Option<update::Hook>,
}

/// 弹出选择框并阻塞到用户选完；`Ok(None)` 表示取消。在 blocking 线程里调用。
pub type Picker = Arc<dyn Fn() -> Result<Option<PathBuf>, Fail> + Send + Sync>;

impl Default for Options {
    fn default() -> Self {
        Options { home: None, engine: "auto".into(), dist: None, timeout: Duration::from_secs(120), picker: None, menu: None, bundled_tex: false, updater: None }
    }
}

fn mtime(p: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// 在给定的监听端口上跑服务，直到出错。端口由调用方绑定：命令行版用固定端口，
/// 桌面壳绑 `127.0.0.1:0` 让系统分配，避免和别的程序或第二个实例撞车。
pub async fn serve(opts: Options, listener: tokio::net::TcpListener) -> std::io::Result<()> {
    let subo = library::subo_dir();
    let settings_file = subo.join("settings.json");
    let prefs = settings::Settings::load(&settings_file);
    let library_locked = opts.home.is_some();
    // 优先级：--home > 设置里选的位置 > 默认位置。只在用默认位置时搬旧库：其余说明用户自己管位置。
    // 设置里的位置建不出来（外接盘没插）就退回默认位置，不改设置，插上后重启即恢复
    let chosen = prefs.library.clone().filter(|p| std::fs::create_dir_all(p).is_ok());
    if prefs.library.is_some() && chosen.is_none() && !library_locked {
        println!("  设置里的项目库位置不可用，本次用默认位置");
    }
    let home = opts.home.or(chosen).unwrap_or_else(|| {
        let h = library::default_home();
        if let Some(old) = library::migrate_legacy(&h) {
            println!("  项目库已从 {} 搬到 {}", old.display(), h.display());
        }
        h
    });
    std::fs::create_dir_all(&home).expect("无法创建项目库目录");
    let home = dunce::canonicalize(&home).unwrap_or(home);
    // 要赶在下面任何东西往 subo 目录写文件之前看，否则全新安装也像用过
    let onboarding = onboarding::Onboarding::detect(&subo, library::list(&home).len());
    let engine = engine::detect(&opts.engine);
    // 桌面壳已经按设置选好了 TeX；单独跑的服务按启动参数
    if !tex::initialized() {
        tex::init_external(opts.bundled_tex);
    }
    // 重建引擎根、字体映射要扫一遍 TeX 的字体：后台先做，第一次编译时多半已经好了
    std::thread::spawn(|| {
        assist::sweep();
        tex::after_switch();
    });
    let (tx, _rx) = broadcast::channel(256);

    let timeout = prefs.timeout_secs.map(Duration::from_secs).unwrap_or(opts.timeout);
    let state: Shared = Arc::new(AppState {
        library: RwLock::new(home.clone()),
        library_locked,
        settings: Mutex::new(prefs),
        settings_seen: Mutex::new(mtime(&settings_file)),
        settings_file,
        default_timeout: opts.timeout,
        running: Default::default(),
        folders: Mutex::new(folders::Folders::open(subo.join("folders.json"))),
        cache: subo.join("press"),
        previews: {
            let p = subo.join("preview");
            let _ = std::fs::remove_dir_all(&p);
            p
        },
        projects: subo.join("projects"),
        history: subo.join("history"),
        engine: engine.clone(),
        tx,
        case: Mutex::new(case::Index::open(case::default_index_path())),
        queue: Mutex::new(queue::Store::open(queue::default_path())),
        installing: Mutex::new(false),
        quires: Mutex::new(HashMap::new()),
        picker: opts.picker,
        menu: opts.menu,
        onboarding: Mutex::new(onboarding),
        ai: ai::Ai::open(subo.join("ai.json")),
        updates: update::Updates::new(opts.updater),
    });
    update::spawn(state.clone());

    let mut app = Router::new()
        .route("/api/health", get(health))
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/import",
            post(import_project).layer(DefaultBodyLimit::max(UPLOAD_MAX_BYTES)),
        )
        .route("/api/p/:id/project", get(project_tree))
        .route("/api/p/:id/template", post(save_template))
        .route("/api/templates", get(list_templates))
        .route(
            "/api/templates/import",
            post(import_template).layer(DefaultBodyLimit::max(UPLOAD_MAX_BYTES)),
        )
        .route("/api/templates/:id", axum::routing::delete(remove_template).patch(update_template))
        .route("/api/templates/:id/use", post(use_template))
        .route("/api/folders", post(add_folder))
        .route("/api/folders/pick", post(pick_folder))
        .route("/api/folders/:id", axum::routing::delete(remove_folder))
        .route("/api/p/:id/rename", post(rename_project))
        .route("/api/p/:id/reveal", post(reveal_project))
        .route("/api/p/:id/archive", post(archive_project))
        .route("/api/p/:id/file", get(read_file).put(write_file))
        .route("/api/p/:id/fs/new", post(fs_new))
        .route("/api/p/:id/fs/rename", post(fs_rename))
        .route("/api/p/:id/fs/duplicate", post(fs_duplicate))
        .route("/api/p/:id/fs/delete", post(fs_delete))
        .route("/api/p/:id/raw", get(raw_file))
        .route("/api/p/:id/stat", get(file_stat))
        .route("/api/p/:id/symbols", get(project_symbols))
        .route("/api/p/:id/history", get(file_history).post(file_history_keep))
        .route("/api/p/:id/history/:vid", get(file_history_version))
        .route("/api/p/:id/build", post(build))
        .route("/api/p/:id/jobs", get(list_jobs))
        .route("/api/p/:id/compat", post(apply_compat))
        .route("/api/p/:id/engine", get(get_engine).put(put_engine))
        .route("/api/p/:id/pdf/:job", get(pdf))
        .route("/api/p/:id/export/:job", post(export_pdf))
        .route("/api/p/:id/log/:job", get(log))
        .route("/api/p/:id/synctex/:job/view", get(synctex_view))
        .route("/api/p/:id/synctex/:job/edit", get(synctex_edit))
        .route("/api/engines", get(list_engines))
        .route(
            "/api/engines/import/:name",
            post(import_engine).layer(DefaultBodyLimit::max(ENGINE_MAX_BYTES)),
        )
        .route("/api/engines/:id/install", post(install_engine))
        .route("/api/engines/:id/uninstall", post(uninstall_engine))
        .route("/api/case/install", post(case_install))
        .route("/api/case/install/progress", get(case_install_progress))
        .route("/api/case/install/cancel", post(case_install_cancel))
        .route("/api/case/queue", get(case_queue))
        .route("/api/case/queue/resolve", post(case_queue_resolve))
        .route("/api/case/queue/remove", post(case_queue_remove))
        .route("/api/case/snapshot", get(case_snapshot_export))
        .route(
            "/api/case/snapshot/import",
            post(case_snapshot_import).layer(DefaultBodyLimit::max(SNAPSHOT_MAX_BYTES)),
        )
        .route("/api/p/:id/snapshot", post(project_snapshot_export))
        .route("/api/p/:id/snapshot/import", post(project_snapshot_import))
        .route("/api/case/packages", get(case_packages))
        .route("/api/case/packages/remove", post(case_remove))
        .route("/api/case/packages/reinstall", post(case_reinstall))
        .route("/api/case/fonts", get(case_fonts))
        .route("/api/case/fonts/file", get(case_font_file))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/settings/library", post(set_library))
        .route("/api/settings/mirrors/probe", post(probe_mirrors))
        .route("/api/settings/reveal", post(reveal_dir))
        .route("/api/tex", get(get_tex))
        .route("/api/tex/miktex/update", post(update_miktex))
        .route("/api/tex/tlmgr", get(tlmgr_status))
        .route("/api/tex/tlmgr/update", post(update_tlmgr))
        .route("/api/tex/miktex/console", post(open_miktex_console))
        .route("/api/tex/refresh", post(refresh_tex))
        .route("/api/appearance", get(get_appearance))
        .route("/api/appearance/prefs", axum::routing::put(put_appearance_prefs))
        .route("/api/appearance/:kind/:id", axum::routing::put(put_pack).delete(remove_pack))
        .route("/api/ai", get(ai::get_ai).put(ai::put_ai))
        .route("/api/ai/services/:id/key", axum::routing::put(ai::put_key))
        .route("/api/ai/services/:id/test", post(ai::test))
        .route("/api/ai/models/:id/vision", post(ai::test_vision))
        .route("/api/ai/complete", post(ai::complete))
        .route("/api/p/:id/ai/assist", post(assist::assist).layer(DefaultBodyLimit::max(AI_MAX_BYTES)))
        .route("/api/p/:id/ai/verify", post(assist::verify))
        .route("/api/p/:id/ai/diagnose", post(assist::diagnose))
        .route("/api/about", get(about))
        .route("/api/update", get(update::get_update))
        .route("/api/update/check", post(update::post_check))
        .route("/api/update/download", post(update::post_download))
        .route("/api/update/apply", post(update::post_apply))
        .route("/api/onboarding", get(get_onboarding))
        .route("/api/onboarding/done", post(onboarding_done))
        .route("/api/shell/menu", axum::routing::put(shell::put_menu))
        .route("/api/events", get(events))
        // 没对上的 /api 请求明确报 404，不能落到下面的前端兜底拿回 index.html
        .route("/api/*rest", any(api_missing))
        .layer(axum::middleware::map_response(json_rejections))
        .with_state(state);

    // 单端口模式：把前端构建产物一起托管，桌面壳可以直接指向一个 URL
    if let Some(dist) = opts.dist {
        if dist.is_dir() {
            let index = dist.join("index.html");
            app = app.fallback_service(ServeDir::new(&dist).fallback(ServeFile::new(index)));
            println!("  前端: {}", dist.display());
        }
    }

    let addr = listener.local_addr()?;
    println!("Subo / 素帛 本地服务");
    println!("  项目库: {}", home.display());
    println!("  引擎: {engine}（超时 {} s）", timeout.as_secs());
    println!("  地址: http://{addr}");
    axum::serve(listener, app).await
}

async fn health(State(s): State<Shared>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "engine": s.engine,
        // TeX 是随应用内置的那份（而不是用户自己装的）
        "bundledTex": tex::bundled(),
        "home": s.home().to_string_lossy(),
    }))
}

/// 「关于」里显示的版本与本机环境；字段都是平铺的，前端也拿它拼「复制诊断信息」
async fn about(State(s): State<Shared>) -> Response {
    let (engine, bundled, home) = (s.engine.clone(), tex::bundled(), s.home());
    // 读 TeX Live 版本要起子进程，放到 blocking 线程
    let tex = tokio::task::spawn_blocking(|| (case::texlive_root(), case::texlive_version())).await;
    let (tex_root, tex_version) = tex.unwrap_or((None, None));
    let opt = |v: &str| (!v.is_empty()).then(|| v.to_string());
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "build": opt(env!("SUBO_BUILD")),
        "buildDate": opt(env!("SUBO_BUILD_DATE")),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "engine": engine,
        "bundledTex": bundled,
        "texRoot": tex_root.map(|p| p.to_string_lossy().into_owned()),
        "texVersion": tex_version,
        "home": home.to_string_lossy(),
    }))
    .into_response()
}

/// 该不该弹上手引导、弹哪种（fresh / upgrade / null）
async fn get_onboarding(State(s): State<Shared>) -> impl IntoResponse {
    let o = s.onboarding.lock().unwrap();
    Json(json!({ "kind": o.kind, "version": onboarding::VERSION, "previous": o.previous }))
}

/// 看完或跳过引导：记下当前版本，之后不再自动弹
async fn onboarding_done(State(s): State<Shared>) -> Response {
    match s.onboarding.lock().unwrap().done() {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => Fail::new("onboarding.saveFailed", format!("引导记录保存失败：{e}"))
            .detail(e)
            .status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn list_projects(State(s): State<Shared>) -> impl IntoResponse {
    let folders: Vec<folders::Folder> = s.folders.lock().unwrap().all().to_vec();
    let mut out: Vec<library::Summary> = folders
        .iter()
        .map(|f| {
            let name = f.path.file_name().map(|n| n.to_string_lossy().to_string());
            let mut sum = library::summary(&f.id, &name.unwrap_or_else(|| f.id.clone()), &f.path, true);
            sum.archived = f.archived;
            sum
        })
        .collect();
    out.extend(library::list(&s.home()));
    // 找不到的文件夹沉底，其余按最近修改
    out.sort_by(|a, b| a.missing.cmp(&b.missing).then(b.modified.cmp(&a.modified)));
    Json(out)
}

#[derive(Deserialize)]
struct FolderBody {
    path: String,
}

/// 登记一个本地文件夹。落在项目库里的直接返回库里的 id，不重复登记。
async fn add_folder(State(s): State<Shared>, Json(body): Json<FolderBody>) -> Response {
    let raw = body.path.trim();
    let path = match raw.strip_prefix("~/") {
        Some(rest) => library::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(raw),
    };
    if !path.is_absolute() {
        return Fail::new("folder.notAbsolute", "请填完整路径，如 /Users/你/论文").status(StatusCode::BAD_REQUEST);
    }
    if let Ok(real) = dunce::canonicalize(&path) {
        let home = s.home();
        if real == home {
            return Fail::new("folder.isLibrary", "这是项目库本身，里面的项目已经在列表里了").status(StatusCode::BAD_REQUEST);
        }
        if let Ok(rest) = real.strip_prefix(&home) {
            if let Some(first) = rest.components().next() {
                let id = first.as_os_str().to_string_lossy().to_string();
                return Json(json!({ "id": id })).into_response();
            }
        }
    }
    match s.folders.lock().unwrap().add(&path) {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(e) => e.status(StatusCode::BAD_REQUEST),
    }
}

/// 弹系统选择框；取消时返回 `{path: null}`。
async fn pick_folder(State(s): State<Shared>) -> Response {
    let picker = s.picker.clone();
    let ran = tokio::task::spawn_blocking(move || match picker {
        Some(pick) => pick(),
        None => folders::pick(),
    });
    match ran.await {
        Ok(Ok(p)) => Json(json!({ "path": p.map(|p| p.to_string_lossy().to_string()) })).into_response(),
        Ok(Err(e)) => e.status(StatusCode::NOT_IMPLEMENTED),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 移出列表：只删清单里这一条，磁盘上的文件夹原样不动。
async fn remove_folder(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    let mut map = s.quires.lock().unwrap();
    if map.get(&id).is_some_and(|q| q.press.lock().unwrap().running) {
        return Fail::new("folder.busy", "正在编译，稍后再移出").status(StatusCode::CONFLICT);
    }
    match s.folders.lock().unwrap().remove(&id) {
        Ok(true) => {
            if let Some(q) = map.remove(&id) {
                let _ = std::fs::remove_dir_all(&q.out);
            }
            Json(json!({ "ok": true })).into_response()
        }
        Ok(false) => not_found(),
        Err(e) => e.status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
struct ArchiveBody {
    archived: bool,
}

/// 归档只是从首页的常用列表里收起来，文件一个不动，随时能取消。
async fn archive_project(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<ArchiveBody>,
) -> Response {
    let res = if folders::is_folder_id(&id) {
        match s.folders.lock().unwrap().set_archived(&id, body.archived) {
            Ok(true) => Ok(()),
            Ok(false) => return not_found(),
            Err(e) => Err(e),
        }
    } else {
        match library::set_archived(&s.home(), &id, body.archived) {
            Err(e) if e.code == "project.notFound" => return not_found(),
            r => r,
        }
    };
    match res {
        Ok(()) => Json(json!({ "archived": body.archived })).into_response(),
        Err(e) => e.status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// `?path=` 在文件管理器里选中项目里的这个文件；不给就是项目文件夹本身
async fn reveal_project(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    if q.preview {
        return preview_read_only();
    }
    let target = match qs.get("path").filter(|p| !p.is_empty()) {
        Some(rel) => match project::resolve(&q.root, rel) {
            Ok(p) => p,
            Err(e) => return e.status(StatusCode::BAD_REQUEST),
        },
        None => q.root.clone(),
    };
    match tokio::task::spawn_blocking(move || folders::reveal(&target)).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => e.status(StatusCode::INTERNAL_SERVER_ERROR),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
}

async fn create_project(State(s): State<Shared>, Json(body): Json<CreateBody>) -> Response {
    match library::create(&s.home(), &body.name) {
        Ok(id) => {
            s.pin_program(&id, "xelatex");
            (StatusCode::CREATED, Json(json!({ "id": id }))).into_response()
        }
        // 名称不合规是用户的输入问题，其余是磁盘问题
        Err(e) if e.code == "name.tooLong" => e.status(StatusCode::BAD_REQUEST),
        Err(e) => e.status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 请求体就是压缩包原始字节（zip / tar.gz，按内容认）；`?name=` 给项目起名（默认取文件名）。
async fn import_project(
    State(s): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    let name = q.get("name").cloned().unwrap_or_else(|| "imported".into());
    let home = s.home();
    let res = tokio::task::spawn_blocking(move || library::import_archive(&home, &name, &body)).await;
    match res {
        Ok(Ok(id)) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Ok(Err(e)) => e.status(StatusCode::BAD_REQUEST),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn list_templates(State(s): State<Shared>) -> impl IntoResponse {
    Json(templates::list(&s.templates()))
}

/// 模板校验失败是内容问题（400），复制、落盘失败是磁盘问题（500）
fn template_status(e: Fail) -> Response {
    let status = match e.code {
        "template.store" | "template.copy" | "template.removeFailed" | "import.tempDir" => StatusCode::INTERNAL_SERVER_ERROR,
        "template.notFound" => StatusCode::NOT_FOUND,
        "template.builtin" => StatusCode::FORBIDDEN,
        _ => StatusCode::BAD_REQUEST,
    };
    e.status(status)
}

/// 把项目存为模板，名字默认用项目名。只看形状不编译，见 `templates`。
async fn save_template(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<CreateBody>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    if q.preview {
        return preview_read_only();
    }
    let dir = s.templates();
    let res = tokio::task::spawn_blocking(move || templates::from_project(&dir, &q.root, &body.name)).await;
    match res {
        Ok(Ok(stored)) => (StatusCode::CREATED, Json(stored)).into_response(),
        Ok(Err(e)) => template_status(e),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 请求体是 zip 原始字节，`?name=` 起名，同项目导入。
async fn import_template(
    State(s): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    let name = q.get("name").cloned().unwrap_or_else(|| "template".into());
    let dir = s.templates();
    let res = tokio::task::spawn_blocking(move || templates::from_zip(&dir, &name, &body)).await;
    match res {
        Ok(Ok(stored)) => (StatusCode::CREATED, Json(stored)).into_response(),
        Ok(Err(e)) => template_status(e),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 用模板新建项目，返回项目 id。缺的宏包、字体不在这里管：项目打开即编译，走缺字清单。
async fn use_template(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<CreateBody>,
) -> Response {
    let (dir, home) = (s.templates(), s.home());
    let program = presets::get(&id).map_or("xelatex", |p| p.program);
    let res = tokio::task::spawn_blocking(move || templates::instantiate(&dir, &id, &home, &body.name)).await;
    match res {
        Ok(Ok(pid)) => {
            s.pin_program(&pid, program);
            (StatusCode::CREATED, Json(json!({ "id": pid }))).into_response()
        }
        Ok(Err(e)) => template_status(e),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
struct TemplatePatch {
    category: String,
}

/// 改模板分类。预置模板只读
async fn update_template(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<TemplatePatch>,
) -> Response {
    match templates::set_category(&s.templates(), &id, &body.category) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => template_status(e),
    }
}

async fn remove_template(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    match templates::remove(&s.templates(), &id) {
        Ok(()) => {
            s.drop_preview(&id);
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => template_status(e),
    }
}

/// 改名即改目录名，项目 id 随之变化，返回新 id。本地文件夹是用户自己的，不让改；
/// 编译中的项目也不让改——编译器正握着旧路径写产物。
async fn rename_project(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<CreateBody>,
) -> Response {
    if preview_template(&id).is_some() {
        return preview_read_only();
    }
    if folders::is_folder_id(&id) {
        return Fail::new("rename.folder", "本地文件夹请在访达里改名").status(StatusCode::BAD_REQUEST);
    }
    // 全程拿着 quires 锁，改名期间没人能按旧 id 打开或开编
    let mut map = s.quires.lock().unwrap();
    if map.get(&id).is_some_and(|q| q.press.lock().unwrap().running) {
        return Fail::new("rename.busy", "正在编译，稍后再改名").status(StatusCode::CONFLICT);
    }
    match library::rename(&s.home(), &id, &body.name) {
        Ok(new_id) => {
            if new_id != id {
                // 旧条目里的产物路径都指向旧目录，丢掉，下次按新 id 重新打开
                if let Some(q) = map.remove(&id) {
                    let _ = std::fs::remove_dir_all(&q.out);
                }
                projprefs::rename(&s.projects, &id, &new_id);
            }
            Json(json!({ "id": new_id })).into_response()
        }
        Err(e) => e.status(StatusCode::BAD_REQUEST),
    }
}

async fn project_tree(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let folder = folders::is_folder_id(&q.id);
    let name = if folder {
        q.root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| q.id.clone())
    } else {
        q.id.clone()
    };
    Json(json!({
        "id": q.id,
        "name": name,
        "folder": folder,
        "preview": q.preview,
        "archived": s.archived(&q),
        "root": q.root.to_string_lossy(),
        "entry": project::entry(&q.root),
        "files": project::tree(&q.root),
    }))
    .into_response()
}

async fn read_file(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let Some(rel) = qs.get("path") else {
        return missing_param("path");
    };
    match project::resolve(&q.root, rel) {
        Ok(p) => match std::fs::read_to_string(&p) {
            Ok(content) => Json(json!({ "path": rel, "content": content, "mtime": mtime_ms(&p) })).into_response(),
            Err(e) => read_failed(e),
        },
        Err(e) => e.status(StatusCode::BAD_REQUEST),
    }
}

/// 文件的修改时间（毫秒）；不存在为 `None`。外部修改靠它认：前端记着读到时的值，
/// 保存时带上，对不上就是别的程序（git、另一个编辑器）改过
fn mtime_ms(p: &std::path::Path) -> Option<u64> {
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// 前端轮询：当前文件的修改时间，和文件树的指纹（外部新建、删除文件时变）
async fn file_stat(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let mtime = match qs.get("path").map(|rel| project::resolve(&q.root, rel)) {
        Some(Ok(p)) => mtime_ms(&p),
        Some(Err(e)) => return e.status(StatusCode::BAD_REQUEST),
        None => None,
    };
    let root = q.root.clone();
    let tree = tokio::task::spawn_blocking(move || {
        hash_name(project::tree(&root).iter().map(|e| (e.path.clone(), e.dir)).collect::<Vec<_>>())
    })
    .await
    .unwrap_or_default();
    Json(json!({ "mtime": mtime, "tree": tree })).into_response()
}

/// 大纲、标签、文献条目：编辑器的大纲面板和 `\ref`、`\cite` 补全用
async fn project_symbols(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let root = q.root.clone();
    let res = tokio::task::spawn_blocking(move || {
        let files: Vec<String> = project::tree(&root).into_iter().filter(|e| !e.dir).map(|e| e.path).collect();
        symbols::collect(&root, project::entry(&root).as_deref(), &files)
    })
    .await;
    match res {
        Ok(sym) => Json(sym).into_response(),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 文件的历史版本，由新到旧
async fn file_history(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let Some(rel) = qs.get("path") else {
        return missing_param("path");
    };
    if let Err(e) = project::resolve(&q.root, rel) {
        return e.status(StatusCode::BAD_REQUEST);
    }
    Json(json!({ "versions": s.history(&q).list(rel) })).into_response()
}

async fn file_history_version(
    State(s): State<Shared>,
    Path((id, vid)): Path<(String, u64)>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let Some(rel) = qs.get("path") else {
        return missing_param("path");
    };
    match s.history(&q).read(rel, vid) {
        Some(content) => Json(json!({ "id": vid, "content": content })).into_response(),
        None => Fail::new("history.notFound", "没有这个历史版本").status(StatusCode::NOT_FOUND),
    }
}

/// 前端要整篇换掉编辑器内容（载入外部修改）之前，先把手上没存的那份记进历史
async fn file_history_keep(State(s): State<Shared>, Path(id): Path<String>, Json(body): Json<FileBody>) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    if let Some(r) = s.read_only(&q) {
        return r;
    }
    if let Err(e) = project::resolve(&q.root, &body.path) {
        return e.status(StatusCode::BAD_REQUEST);
    }
    Json(json!({ "id": s.history(&q).keep(&body.path, &body.content, true) })).into_response()
}

/// 附件原样返回（图片、字体、PDF），供前端预览。
async fn raw_file(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let Some(rel) = qs.get("path") else {
        return missing_param("path");
    };
    let p = match project::resolve(&q.root, rel) {
        Ok(p) => p,
        Err(e) => return e.status(StatusCode::BAD_REQUEST),
    };
    match std::fs::read(&p) {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, project::mime(rel)),
                (header::CACHE_CONTROL, "no-cache"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                // 项目里的 SVG/HTML 被直接打开时也不能在本服务的源下跑脚本
                (header::CONTENT_SECURITY_POLICY, "sandbox; default-src 'none'; style-src 'unsafe-inline'"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => read_failed(e),
    }
}

#[derive(Deserialize)]
struct FileBody {
    path: String,
    content: String,
    /// 编辑器读到这个文件时的修改时间；磁盘上的对不上就是被外部改过，拒绝覆盖（409）
    #[serde(default)]
    base: Option<u64>,
    /// 写之前无条件把磁盘上的旧内容记进历史（恢复历史版本、覆盖外部修改时）
    #[serde(default)]
    keep: bool,
}

async fn write_file(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<FileBody>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    if let Some(r) = s.read_only(&q) {
        return r;
    }
    match project::resolve(&q.root, &body.path) {
        Ok(p) => {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let (Some(base), Some(now)) = (body.base, mtime_ms(&p)) {
                if base != now {
                    return Fail::new("file.changedOnDisk", "文件在别处被改过了")
                        .with("path", body.path.as_str())
                        .with("mtime", now)
                        .status(StatusCode::CONFLICT);
                }
            }
            // 覆盖前把磁盘上的旧内容记进历史（离上一份够久、或要求必记）
            if let Ok(old) = std::fs::read_to_string(&p) {
                if old != body.content {
                    s.history(&q).keep(&body.path, &old, body.keep);
                }
            }
            match std::fs::write(&p, &body.content) {
                Ok(()) => Json(json!({ "ok": true, "path": body.path, "mtime": mtime_ms(&p) })).into_response(),
                Err(e) => Fail::new("file.writeFailed", format!("写入失败: {e}"))
                    .detail(e)
                    .status(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
        Err(e) => e.status(StatusCode::BAD_REQUEST),
    }
}

#[derive(Deserialize)]
struct FsBody {
    #[serde(default)]
    path: String,
    /// 新建：true 为文件夹
    #[serde(default)]
    dir: bool,
    /// 重命名 / 移动
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    /// 副本名里的后缀（「副本」「copy」），按界面语言由前端给
    #[serde(default)]
    suffix: String,
}

/// 文件树的文件操作：只读项目（模板预览、已归档）一律拒绝；在阻塞线程里做，大文件夹复制不卡事件循环
async fn fs_op(
    s: Shared,
    id: String,
    op: impl FnOnce(&std::path::Path) -> Result<serde_json::Value, Fail> + Send + 'static,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    if let Some(r) = s.read_only(&q) {
        return r;
    }
    match tokio::task::spawn_blocking(move || op(&q.root)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => {
            let code = match e.code {
                "file.exists" => StatusCode::CONFLICT,
                "file.notFound" => StatusCode::NOT_FOUND,
                c if c.ends_with("Failed") => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::BAD_REQUEST,
            };
            e.status(code)
        }
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn fs_new(State(s): State<Shared>, Path(id): Path<String>, Json(b): Json<FsBody>) -> Response {
    fs_op(s, id, move |root| files::create(root, &b.path, b.dir).map(|()| json!({ "path": b.path }))).await
}

async fn fs_rename(State(s): State<Shared>, Path(id): Path<String>, Json(b): Json<FsBody>) -> Response {
    fs_op(s, id, move |root| files::rename(root, &b.from, &b.to).map(|()| json!({ "path": b.to }))).await
}

async fn fs_duplicate(State(s): State<Shared>, Path(id): Path<String>, Json(b): Json<FsBody>) -> Response {
    let suffix = if b.suffix.trim().is_empty() { "copy".to_string() } else { b.suffix.trim().to_string() };
    fs_op(s, id, move |root| files::duplicate(root, &b.path, &suffix).map(|p| json!({ "path": p }))).await
}

async fn fs_delete(State(s): State<Shared>, Path(id): Path<String>, Json(b): Json<FsBody>) -> Response {
    fs_op(s, id, move |root| files::trash(root, &b.path).map(|()| json!({ "ok": true }))).await
}

#[derive(Serialize)]
struct BuildAccepted {
    /// true 表示已有编译在跑，本次请求并入「跑完再编一次」
    queued: bool,
}

/// 默认立即返回 202，结果走 `/api/events`。`?wait=1` 一直等到这次编译收尾，直接返回结果
/// （脚本、agent 用，不必自己订阅事件流），见 [`await_job`]。
async fn build(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    if project::entry(&q.root).is_none() {
        return Fail::new("build.noTex", "项目里没有找到 .tex 文件").status(StatusCode::BAD_REQUEST);
    }
    let wait = qs.get("wait").is_some_and(|v| !matches!(v.as_str(), "0" | "false"));
    // 先订阅再开编，不会漏掉 job.started
    let rx = wait.then(|| s.tx.subscribe());
    let queued = {
        let mut p = q.press.lock().unwrap();
        if p.running {
            p.queued = true;
            true
        } else {
            p.running = true;
            false
        }
    };
    if !queued {
        let (s, q) = (s.clone(), q.clone());
        tokio::task::spawn_blocking(move || press_loop(&s, &q));
    }
    match rx {
        Some(rx) => await_job(&s, &q, rx).await,
        None => (StatusCode::ACCEPTED, Json(BuildAccepted { queued })).into_response(),
    }
}

/// 从事件流里收下一次开跑的编译（已在跑的那次没见到 job.started，自然跳过，等补跑的那次），
/// 直到 `job.end`，汇总成一个结果。等待上限：编译超时再加十分钟（缺字查包名要联网）。
async fn await_job(s: &Shared, q: &Quire, mut rx: broadcast::Receiver<String>) -> Response {
    let pid = q.id.clone();
    let collect = async move {
        let mut jid: Option<String> = None;
        let mut out = json!({});
        loop {
            let text = match rx.recv().await {
                Ok(t) => t,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return None,
            };
            let Ok(ev) = serde_json::from_str::<Value>(&text) else { continue };
            if ev["projectId"].as_str() != Some(pid.as_str()) {
                continue;
            }
            let ty = ev["type"].as_str().unwrap_or_default();
            let this = ev["jobId"].as_str().unwrap_or_default();
            match &jid {
                // 入口在排队时被删掉的那条只有 job.failed，没有 job.started
                None if matches!(ty, "job.started" | "job.failed") => {
                    jid = Some(this.to_string());
                    out["jobId"] = json!(this);
                }
                Some(j) if j == this => {}
                _ => continue,
            }
            match ty {
                "job.started" => out["entry"] = ev["entry"].clone(),
                "job.issues" => out["issues"] = ev["report"]["issues"].clone(),
                "job.done" | "job.failed" => {
                    out["ok"] = json!(ty == "job.done");
                    for k in ["engine", "ms", "error", "reason"] {
                        if !ev[k].is_null() {
                            out[k] = ev[k].clone();
                        }
                    }
                }
                "case.missing" => out["missing"] = ev["files"].clone(),
                "compat.found" => out["compat"] = ev["fixes"].clone(),
                "tex.stale" => out["texStale"] = json!({ "kernel": ev["kernel"], "want": ev["want"] }),
                "engine.missing" => {
                    let mut e = ev.clone();
                    for k in ["type", "jobId", "projectId"] {
                        e.as_object_mut().map(|m| m.remove(k));
                    }
                    out["engineMissing"] = e;
                }
                "job.end" => {
                    if out["ok"] == json!(false) {
                        out["errors"] = json!(error_lines(&out["issues"]));
                    }
                    return Some(out);
                }
                _ => {}
            }
        }
    };
    let limit = s.timeout() + Duration::from_secs(600);
    match tokio::time::timeout(limit, collect).await {
        Ok(Some(mut out)) => {
            let jid = out["jobId"].as_str().unwrap_or_default().to_string();
            out["pdf"] = json!(q.jobs.lock().unwrap().map.get(&jid).is_some_and(|j| j.pdf.is_some()));
            Json(out).into_response()
        }
        Ok(None) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
        Err(_) => Fail::new("build.waitTimeout", "等编译结果超时").status(StatusCode::GATEWAY_TIMEOUT),
    }
}

/// 失败时的错误，一条一句：`文件:行: 消息`（出在发行版文件里时没有位置）。解析见 logparse.rs
fn error_lines(issues: &Value) -> Vec<String> {
    let Some(list) = issues.as_array() else { return Vec::new() };
    list.iter()
        .filter(|i| i["level"] == "error")
        .map(|i| {
            let msg = i["message"].as_str().unwrap_or_default();
            match (i["file"].as_str(), i["line"].as_u64()) {
                (Some(f), Some(l)) => format!("{f}:{l}: {msg}"),
                (Some(f), None) => format!("{f}: {msg}"),
                _ => msg.to_string(),
            }
        })
        .collect()
}

/// `latest` 换成最近一次编译的 id，其余原样
fn job_id(q: &Quire, job: &str) -> String {
    if job == "latest" {
        if let Some(last) = q.jobs.lock().unwrap().order.back() {
            return last.clone();
        }
    }
    job.to_string()
}

/// 这个项目内存里还留着的编译（最多 `KEEP_JOBS` 次，重启即清空），由新到旧
async fn list_jobs(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let running = q.press.lock().unwrap().running;
    let jobs = q.jobs.lock().unwrap();
    let list: Vec<Value> = jobs
        .order
        .iter()
        .rev()
        .filter_map(|jid| {
            let j = jobs.map.get(jid)?;
            Some(json!({ "jobId": jid, "ok": j.ok, "entry": j.entry, "pdf": j.pdf.is_some() }))
        })
        .collect();
    Json(json!({ "running": running, "jobs": list })).into_response()
}

/// 编译工位：跑完一次后检查有没有新的请求，有就再跑，没有就收工。
fn press_loop(s: &AppState, q: &Quire) {
    loop {
        // 单次编译即便 panic 也不能让 running 永远卡在 true
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _slot = s.slot();
            run_job(s, q)
        }));
        let mut p = q.press.lock().unwrap();
        if p.queued {
            p.queued = false;
        } else {
            p.running = false;
            return;
        }
    }
}

fn run_job(s: &AppState, q: &Quire) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let jid = format!("j{nanos}");
    // 入口在排队期间可能被删掉或改名，开跑时再取一次
    let Some(entry) = project::entry(&q.root) else {
        s.emit(
            q,
            json!({"type": "job.failed", "jobId": jid, "engine": s.engine, "ms": 0,
                   "error": "项目里没有找到 .tex 文件"}),
        );
        s.emit(q, json!({"type": "job.end", "jobId": jid}));
        return;
    };

    s.emit(q, json!({"type": "job.started", "jobId": jid, "entry": entry}));

    let started = Instant::now();
    let outdir = press_dir(q, &jid);
    let (prog, note, wanted) = s.engine_for(q, &entry);
    let progress = |step: engine::Step| {
        let mut ev = match step {
            // 终端输出另成一种事件：界面接到就往原始日志后面追加，job.log 到了再整段换掉
            engine::Step::Output(text) => {
                s.emit(q, json!({"type": "job.output", "jobId": jid, "text": text}));
                return;
            }
            engine::Step::Pass(n) => json!({"step": "pass", "pass": n}),
            engine::Step::Pages(n) => json!({"step": "pages", "pages": n}),
            engine::Step::Tool(name) => {
                let kind = if name.contains("glossaries") || name.contains("nomencl") {
                    "glossary"
                } else if name.starts_with("makeindex") {
                    "index"
                } else {
                    "bib"
                };
                json!({"step": kind, "tool": name})
            }
            engine::Step::Pdf => json!({"step": "pdf"}),
        };
        ev["type"] = json!("job.progress");
        ev["jobId"] = json!(jid);
        s.emit(q, ev);
    };
    let mut res = engine::compile(prog, &q.root, &entry, &outdir, s.timeout(), &s.limits(), &progress);
    if let Some(n) = note {
        res.log.insert_str(0, &n);
    }
    let ms = started.elapsed().as_millis() as u64;

    if !res.log.is_empty() {
        s.emit(q, json!({"type": "job.log", "jobId": jid, "text": res.log}));
    }
    // 问题面板、编译校验的基准都用这一份；界面不再自己读日志
    let report = logparse::parse(&res.log);
    s.emit(q, json!({"type": "job.issues", "jobId": jid, "report": report}));

    // reason 给前端出文案；error 是中文兜底
    let (reason, error) = match (res.timed_out, res.over) {
        (true, _) => ("timeout", "编译超时，已终止"),
        (_, Some(engine::Over::Memory)) => ("memory", "内存用量超过上限，已终止"),
        (_, Some(engine::Over::FileSize)) => ("fileSize", "输出文件过大，已终止"),
        _ => ("failed", "编译失败，详见日志"),
    };
    let ev = if res.ok {
        json!({"type": "job.done", "jobId": jid, "engine": prog, "ms": ms})
    } else {
        json!({"type": "job.failed", "jobId": jid, "engine": prog, "ms": ms, "error": error, "reason": reason})
    };

    // AI 修复的 diff 基准、编译校验的排版基准
    if res.ok {
        lastgood::record(&q.root, &outdir, &entry, &res.log);
    }
    let d = diagnose(&q.root, &entry, &outdir, &res, prog);
    let stale = (!res.ok).then(|| tex::stale(&res.log)).flatten();
    let Diagnosis { missing, fonts, fixes, .. } = &d;

    // 先登记产物再推事件，前端收到 job.done 立刻来取 PDF 时不会扑空
    let evicted: Vec<String> = {
        let mut jobs = q.jobs.lock().unwrap();
        jobs.map.insert(jid.clone(), Job { pdf: res.pdf, log: res.log, ok: res.ok, entry: entry.clone() });
        jobs.order.push_back(jid.clone());
        let mut out = Vec::new();
        while jobs.order.len() > KEEP_JOBS {
            let old = jobs.order.pop_front().unwrap();
            jobs.map.remove(&old);
            out.push(old);
        }
        out
    };
    s.emit(q, ev);

    // 要用的引擎没装：问题面板提示安装（开了自动安装由前端直接装，装完重编）
    if let Some(w) = wanted {
        if let Some(spec) = engines::for_program(w.name()) {
            let managed = tex::bundled();
            let bytes = engines::overview(managed).engines.into_iter().find(|e| e.id == spec.id).map_or(0, |e| e.bytes);
            s.emit(
                q,
                json!({"type": "engine.missing", "jobId": jid, "program": w.name(), "engine": spec.id,
                       "name": spec.name, "bytes": bytes, "managed": managed}),
            );
        }
    }

    // 旧样式和新版 TeX 的已知不兼容：认出来交给用户确认，不擅自改文件
    if !fixes.is_empty() {
        s.emit(q, json!({"type": "compat.found", "jobId": jid, "fixes": fixes}));
    }

    // LaTeX 内核比宏包、模板旧：问题面板给出能一键做的修法（改用内置 TeX、更新 MiKTeX）
    if let Some(st) = stale {
        let status = tex::status(s.prefs().tex);
        s.emit(q, json!({"type": "tex.stale", "jobId": jid, "kernel": st.kernel, "want": st.want, "tex": status}));
    }

    // 结果已经报出去了，再慢慢查缺的文件属于哪个包：先推文件名，查完再推一次。
    // 缺的系统字体不用查，两次都原样跟在文件后面
    if !d.is_empty() {
        let send = |mut files: Vec<case::Missing>, resolved: bool| {
            files.extend(fonts.iter().cloned());
            s.emit(
                q,
                json!({"type": "case.missing", "jobId": jid, "files": files, "resolved": resolved}),
            );
        };
        if !missing.is_empty() {
            send(case::pending(missing), false);
        }
        let (repo, fallback) = (s.repository(), s.fallback());
        let files = d.resolve(&s.case, repo.as_deref(), fallback);
        // 查不到包名的（多半是离线）进待取队列，联网后先查再装
        let unknown: Vec<String> = files
            .iter()
            .filter(|f| f.kind == case::Kind::File && f.lookup == case::Lookup::Unknown)
            .map(|f| f.file.clone())
            .collect();
        if !unknown.is_empty() && !q.preview {
            s.queue.lock().unwrap().add_files(&unknown, Some(&q.id));
        }
        // 只有宽进的候选、又全是无害回退的，什么都不推
        if !files.is_empty() || !fonts.is_empty() {
            send(files, true);
        }
    }

    for old in evicted {
        let _ = std::fs::remove_dir_all(press_dir(q, &old));
    }
    // 这次编译的事件到此为止（缺字清单、兼容修复都已推完）：`build?wait=1` 靠它收尾
    s.emit(q, json!({"type": "job.end", "jobId": jid}));
}

/// 一次编译后认出缺什么、哪里不兼容。服务端（`run_job`）和命令行（`cli`）共用，两边结论一致
pub(crate) struct Diagnosis {
    /// TeX 报缺的文件，要查包名
    pub missing: Vec<String>,
    /// 日志里提到、源码里声明的候选文件，只留 TeX Live 有、本机没装的
    pub loose: Vec<String>,
    /// 不用查包名的：项目里就有的文件、缺的字体、缺的外部程序
    pub fonts: Vec<case::Missing>,
    pub fixes: Vec<compat::Fix>,
}

impl Diagnosis {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.loose.is_empty() && self.fonts.is_empty()
    }

    /// 查缺的文件属于哪个包（要问 tlmgr，可能联网）。返回查过的文件，不含 `fonts`
    pub fn resolve(&self, index: &Mutex<case::Index>, repo: Option<&str>, fallback: case::Fallback) -> Vec<case::Missing> {
        let mut cache = index.lock().unwrap();
        let mut files = Vec::new();
        if !self.missing.is_empty() {
            files = case::resolve(&mut cache, &self.missing, repo, fallback);
        }
        files.extend(case::confirm(&mut cache, &self.loose));
        files
    }
}

pub(crate) fn diagnose(root: &std::path::Path, entry: &str, outdir: &std::path::Path, res: &engine::CompileResult, prog: &str) -> Diagnosis {
    // TeX 自己的 .log 在产物目录里：缺 Ghostscript 的兜底判断、biblatex 缺哪个样式文件都要看它
    let texlog = std::path::Path::new(&entry)
        .file_stem()
        .and_then(|st| std::fs::read(outdir.join(format!("{}.log", st.to_string_lossy()))).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let mut missing = if res.ok { Vec::new() } else { case::missing_files(&res.log) };
    // bibtex 缺 .bst 时编译照样成功，只是引用全成 [?]。不改判失败：PDF 能看，
    // 跟 bibtex 失败只记日志的做法一致；缺字清单照推，前端不论成败都会亮出来
    for f in case::missing_bib_styles(&res.log, &texlog) {
        if !missing.contains(&f) {
            missing.push(f);
        }
    }
    let font_names = if res.ok { Vec::new() } else { case::missing_fonts(&res.log) };
    case::drop_font_echoes(&mut missing, &font_names);
    let mut fonts = case::fonts(&font_names);
    // TeX Live 带着、只是没装的字体（韩文模板的 UnBatang）当缺文件报，查得出包、能一键装
    for (name, file) in case::installable_fonts(&fonts) {
        fonts.retain(|m| !(m.kind == case::Kind::Font && m.file == name));
        if !missing.contains(&file) {
            missing.push(file);
        }
    }
    // 缺的外部程序（Ghostscript）也不用查，跟字体一样原样附在后面
    if !res.ok {
        let mut tools = case::missing_tools(&res.log, &texlog, case::ghostscript_found());
        // pdflatex 停在第一张转不了的图上：其余 EPS 顺着源码补齐，一次说清要转哪些
        for t in tools.iter_mut().filter(|t| t.file == "ghostscript") {
            for f in prescan::eps_figures(root, &entry) {
                if !t.needed_by.contains(&f) {
                    t.needed_by.push(f);
                }
            }
        }
        fonts.extend(tools);
    }
    // 日志里提到没找到、但没报错的文件（缺字体定义文件、xdvipdfmx 缺字体文件），查完只留 TeX Live 里有的
    let mut loose = case::loose_files(&res.log, &missing, !res.ok);
    // TeX 缺一个文件就停，日志里只有第一个。顺着源码把声明要用的都收进来，一次查全、一次装完，
    // 不必「编译 → 装一个 → 再编译」一轮轮试。同样交给 confirm 只留 TeX Live 有、本机没装的
    if !res.ok && !missing.is_empty() {
        for f in prescan::scan(root, &entry) {
            let dup = |v: &Vec<String>| v.iter().any(|s| s.eq_ignore_ascii_case(&f));
            if !dup(&missing) && !dup(&loose) {
                loose.push(f);
            }
        }
    }
    // 项目子目录里就有的文件也不用问 tlmgr，排在最前面
    let mut local = case::in_project(root, &mut missing);
    local.append(&mut fonts);
    let fonts = local;

    let fixes = if res.ok { Vec::new() } else { compat::detect(root, &res.log, prog) };
    Diagnosis { missing, loose, fonts, fixes }
}

/// 项目的引擎：用户选的（`mode`）、实际用的（`program`）和来由，给工具栏显示。
/// `switchable` 为 false 时（tectonic、mock）界面不给选
async fn get_engine(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    use engine::program::{Reason, Source};
    let Some(q) = s.quire(&id) else { return not_found() };
    let mode = projprefs::load(&s.projects, &q.id).engine.unwrap_or_else(|| "auto".into());
    let entry = project::entry(&q.root);
    let Some(c) = entry.as_deref().and_then(|e| s.program_for(&q, e)) else {
        return Json(json!({ "mode": mode, "program": s.engine, "source": "fixed", "switchable": false })).into_response();
    };
    let reason = |r: &Reason| match r {
        Reason::MagicComment => json!({ "kind": "magic" }),
        Reason::Latexmkrc => json!({ "kind": "latexmkrc" }),
        Reason::ArxivReadme => json!({ "kind": "arxiv" }),
        Reason::Preamble(w) => json!({ "kind": "preamble", "detail": w }),
    };
    let (source, why, wanted) = match &c.source {
        Source::User => ("user", None, None),
        Source::Detected(r) => ("detected", Some(reason(r)), None),
        Source::Default => ("default", None, None),
        Source::Missing { wanted, reason: r } => ("missing", r.as_ref().map(reason), Some(wanted.name())),
    };
    Json(json!({
        "mode": mode,
        "program": c.program.name(),
        "source": source,
        "reason": why,
        "wanted": wanted,
        "switchable": !q.preview && !s.archived(&q),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct EngineBody {
    /// `auto` 或程序名（`pdflatex`、`xelatex`、`lualatex`、`platex`、`uplatex`）
    mode: String,
}

async fn put_engine(State(s): State<Shared>, Path(id): Path<String>, Json(body): Json<EngineBody>) -> Response {
    if preview_template(&id).is_some() {
        return preview_read_only();
    }
    let Some(q) = s.quire(&id) else { return not_found() };
    if let Some(r) = s.read_only(&q) {
        return r;
    }
    let engine = match body.mode.as_str() {
        "auto" => None,
        m if engine::program::Program::parse(m).is_some_and(|p| p.name() == m) => Some(body.mode.clone()),
        _ => return Fail::new("engine.unknown", "不认识的引擎").with("engine", body.mode.as_str()).status(StatusCode::BAD_REQUEST),
    };
    match projprefs::save(&s.projects, &q.id, &projprefs::ProjectPrefs { engine }) {
        Ok(()) => get_engine(State(s), Path(id)).await,
        Err(e) => Fail::new("engine.save", format!("保存失败：{e}")).status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
struct CompatBody {
    id: String,
    file: String,
}

/// 用户在问题列表里点了「修复」才会调到这里。改完不自动重编，由前端决定。
async fn apply_compat(State(s): State<Shared>, Path(id): Path<String>, Json(body): Json<CompatBody>) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    if let Some(r) = s.read_only(&q) {
        return r;
    }
    // 自动修复会改用户的文件：改前留一份
    if let Ok(old) = project::resolve(&q.root, &body.file).and_then(|p| std::fs::read_to_string(p).map_err(|e| Fail::new("file.readFailed", e.to_string()))) {
        s.history(&q).keep(&body.file, &old, true);
    }
    match compat::apply(&q.root, &body.id, &body.file) {
        Ok(n) => Json(json!({ "ok": true, "changed": n })).into_response(),
        Err(e) => e.status(StatusCode::BAD_REQUEST),
    }
}

#[derive(Deserialize)]
struct InstallBody {
    packages: Vec<String>,
    /// 设置里开了自动安装、不是用户点的：要弹管理员授权框的情况不装，交回给用户
    #[serde(default)]
    auto: bool,
    /// 哪个项目要的：连不上软件源时记进待取队列
    #[serde(default)]
    project: Option<String>,
}

/// 用户在缺字清单里点了确认才会调到这里。装完不自动重编，由前端决定。
async fn case_install(State(s): State<Shared>, Json(body): Json<InstallBody>) -> Response {
    let mut pkgs = body.packages;
    pkgs.sort();
    pkgs.dedup();
    let files = {
        let idx = s.case.lock().unwrap();
        let queued = s.queue.lock().unwrap().get().packages;
        if let Some(p) = pkgs.iter().find(|p| !idx.knows_package(p) && !queued.contains_key(*p)) {
            return Fail::new("case.unknownPackage", format!("{p} 不在缺字清单里"))
                .with("package", p.as_str())
                .status(StatusCode::BAD_REQUEST);
        }
        idx.files_of(&pkgs)
    };
    if body.auto && case::needs_admin(tex::bundled()) {
        return Fail::new("case.autoNeedsAdmin", "要管理员授权，未自动安装").status(StatusCode::CONFLICT);
    }
    {
        let mut busy = s.installing.lock().unwrap();
        if *busy {
            return Fail::new("case.busy", "已有一次安装在进行").status(StatusCode::CONFLICT);
        }
        *busy = true;
        case::progress_start(&pkgs);
    }
    let s2 = s.clone();
    let (repo, fallback) = (s.repository(), s.fallback());
    let project = body.project.filter(|p| library::valid_id(p));
    let res = tokio::task::spawn_blocking(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut r = case::install(&pkgs, &files, tex::bundled(), repo.as_deref(), fallback);
            // 装上了的出待取队列；连不上软件源的进队列，联网后一键补齐
            if let Ok(done) = &r {
                let mut queue = s2.queue.lock().unwrap();
                if done.ok {
                    let all: Vec<String> = pkgs.iter().chain(done.also.iter()).cloned().collect();
                    queue.remove(&all, &files);
                } else if done.offline {
                    queue.add_packages(&pkgs, project.as_deref());
                }
            }
            // 装的可能是字体包：新字体也要能按名字找到；Type1 字体还要登记映射，pdflatex 才找得到字形
            if let Ok(done) = &mut r {
                case::track(|p| p.phase = "finish");
                fontmap::refresh();
                if tex::bundled() && done.ok {
                    if let Err(e) = case::register_user_maps() {
                        done.log.push_str(&format!("\n{e}"));
                    }
                }
            }
            r
        }));
        case::progress_done();
        *s2.installing.lock().unwrap() = false;
        r
    })
    .await;
    match res {
        Ok(Ok(Ok(done))) => Json(done).into_response(),
        // case::install 的原因文字仍是中文，只能整段塞进 {detail}
        Ok(Ok(Err(msg))) => Fail::new("case.installFailed", msg.clone())
            .detail(msg)
            .status(StatusCode::BAD_REQUEST),
        _ => Fail::new("case.installCrashed", "安装过程异常中断").status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 缺字清单安装时界面轮询的进度；没在装时 `progress` 为 null
async fn case_install_progress() -> Response {
    Json(json!({ "progress": case::progress() })).into_response()
}

/// 取消正在进行的安装：只在下载完之前有效，开始解包后返回 409
async fn case_install_cancel() -> Response {
    match case::request_cancel() {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(code) => Fail::new(code, "现在不能取消").status(StatusCode::CONFLICT),
    }
}

async fn case_queue(State(s): State<Shared>) -> Response {
    Json(s.queue.lock().unwrap().get()).into_response()
}

/// 联网后先给队列里的文件查包名；查到的挪进包里，TeX Live 没有的丢掉。返回新的队列，
/// `unresolved` 是仍然查不了的文件数（还是连不上）
async fn case_queue_resolve(State(s): State<Shared>) -> Response {
    let s2 = s.clone();
    let (repo, fallback) = (s.repository(), s.fallback());
    let res = tokio::task::spawn_blocking(move || {
        let files: Vec<String> = s2.queue.lock().unwrap().get().files.into_keys().collect();
        if files.is_empty() {
            return 0;
        }
        let found = {
            let mut cache = s2.case.lock().unwrap();
            case::resolve(&mut cache, &files, repo.as_deref(), fallback)
        };
        let settled: Vec<(String, Option<String>)> = found
            .iter()
            .filter_map(|m| match m.lookup {
                case::Lookup::Found => Some((m.file.clone(), m.package.clone())),
                case::Lookup::Absent => Some((m.file.clone(), None)),
                _ => None,
            })
            .collect();
        let left = files.len() - settled.len();
        s2.queue.lock().unwrap().settle(&settled);
        left
    })
    .await;
    match res {
        Ok(left) => Json(json!({ "queue": s.queue.lock().unwrap().get(), "unresolved": left })).into_response(),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
struct QueueRemoveBody {
    #[serde(default)]
    packages: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
}

/// 用户把不想要的从待取队列里移掉
async fn case_queue_remove(State(s): State<Shared>, Json(body): Json<QueueRemoveBody>) -> Response {
    let mut q = s.queue.lock().unwrap();
    q.remove(&body.packages, &body.files);
    Json(q.get()).into_response()
}

/// 设置页的「引擎」：内置、已装、可装的引擎
async fn list_engines() -> Response {
    let managed = tex::bundled();
    match tokio::task::spawn_blocking(move || engines::overview(managed)).await {
        Ok(o) => Json(o).into_response(),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn engine_fail(f: engines::Failed, status: StatusCode) -> Response {
    let mut e = Fail::new(f.code, f.detail.clone()).detail(f.detail);
    for (k, v) in f.params {
        e = e.with(k, v);
    }
    e.status(status)
}

#[derive(Deserialize, Default)]
struct EngineInstallBody {
    /// `import`：用导入区里的文件；默认从安装源下载
    #[serde(default)]
    source: Option<String>,
}

/// 装引擎：下载（或用离线导入的文件）、解压、生成格式。与宏包安装共用一把锁：都往用户层写
async fn install_engine(State(s): State<Shared>, Path(id): Path<String>, body: Option<Json<EngineInstallBody>>) -> Response {
    if !tex::bundled() {
        return Fail::new("engines.unmanaged", "用的是本机自己的 TeX，引擎请用 tlmgr 安装").status(StatusCode::CONFLICT);
    }
    let import = body.and_then(|b| b.0.source).as_deref() == Some("import");
    {
        let mut busy = s.installing.lock().unwrap();
        if *busy {
            return Fail::new("case.busy", "已有一次安装在进行").status(StatusCode::CONFLICT);
        }
        *busy = true;
    }
    let s2 = s.clone();
    let (repo, fallback) = (s.repository(), s.fallback());
    let res = tokio::task::spawn_blocking(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let source = if import {
                engines::Source::Import
            } else {
                engines::Source::Repo { repo: repo.as_deref(), fallback }
            };
            engines::install(&id, source)
        }));
        *s2.installing.lock().unwrap() = false;
        r
    })
    .await;
    match res {
        Ok(Ok(Ok(done))) => {
            // luaotfload 带来的字体加载器不算字体，但用户层文件变了，字体清单顺手刷新
            fontmap::refresh();
            Json(done).into_response()
        }
        Ok(Ok(Err(f))) => engine_fail(f, StatusCode::BAD_REQUEST),
        _ => Fail::new("engines.installCrashed", "安装过程异常中断").status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn uninstall_engine(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    if !tex::bundled() {
        return Fail::new("engines.unmanaged", "用的是本机自己的 TeX，引擎请用 tlmgr 管理").status(StatusCode::CONFLICT);
    }
    {
        let mut busy = s.installing.lock().unwrap();
        if *busy {
            return Fail::new("case.busy", "已有一次安装在进行").status(StatusCode::CONFLICT);
        }
        *busy = true;
    }
    let s2 = s.clone();
    let res = tokio::task::spawn_blocking(move || {
        let r = catch_unwind(AssertUnwindSafe(|| engines::uninstall(&id)));
        *s2.installing.lock().unwrap() = false;
        r
    })
    .await;
    match res {
        Ok(Ok(Ok(()))) => Json(json!({ "ok": true })).into_response(),
        Ok(Ok(Err(f))) => engine_fail(f, StatusCode::BAD_REQUEST),
        _ => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 离线导入：一次传一个容器文件，放进导入区；凑齐后由 install 带 `source: import` 装
async fn import_engine(Path(name): Path<String>, body: Bytes) -> Response {
    if !tex::bundled() {
        return Fail::new("engines.unmanaged", "用的是本机自己的 TeX，引擎请用 tlmgr 安装").status(StatusCode::CONFLICT);
    }
    match tokio::task::spawn_blocking(move || engines::stage(&name, &body)).await {
        Ok(Ok(ids)) => Json(json!({ "engines": ids })).into_response(),
        Ok(Err(f)) => engine_fail(f, StatusCode::BAD_REQUEST),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 设置页的「宏包」：已装的宏包清单。tlmgr 只读本地数据库，不联网
async fn case_packages() -> Response {
    let usermode = tex::bundled();
    match tokio::task::spawn_blocking(move || case::installed_packages(usermode)).await {
        Ok(Some(list)) => Json(json!({ "available": true, "packages": list })).into_response(),
        Ok(None) => Json(json!({ "available": false, "packages": [] })).into_response(),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
struct PackagesBody {
    packages: Vec<String>,
}

/// 设置页卸载、重装宏包前的共同检查：只有内置 TeX 的用户层归我们管；与安装共用一把锁
fn begin_user_change(s: &Shared) -> Result<(), Response> {
    if !tex::bundled() {
        return Err(Fail::new("case.unmanaged", "用的是本机自己的 TeX，宏包请用 tlmgr 管理").status(StatusCode::CONFLICT));
    }
    let mut busy = s.installing.lock().unwrap();
    if *busy {
        return Err(Fail::new("case.busy", "已有一次安装在进行").status(StatusCode::CONFLICT));
    }
    *busy = true;
    Ok(())
}

/// 设置页「导出全部」：用户层里补装的宏包全打进一个快照，直接下载
async fn case_snapshot_export(State(s): State<Shared>) -> Response {
    if let Err(r) = begin_user_change(&s) {
        return r;
    }
    let s2 = s.clone();
    let res = tokio::task::spawn_blocking(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let tmp = subo_dir().join("case").join(format!("export-{}.{}", std::process::id(), snapshot::EXT));
            let done = snapshot::export(None, None, &tmp).and_then(|_| {
                std::fs::read(&tmp).map_err(|e| Fail::new("case.snapshotWriteFailed", format!("读快照失败：{e}")).detail(e))
            });
            let _ = std::fs::remove_file(&tmp);
            done
        }));
        *s2.installing.lock().unwrap() = false;
        r
    })
    .await;
    match res {
        Ok(Ok(Ok(bytes))) => {
            let year = case::local_year().map(|y| format!("-{y}")).unwrap_or_default();
            let disposition = format!("attachment; filename=\"subo-case{year}.{}\"", snapshot::EXT);
            ([(header::CONTENT_TYPE, "application/gzip".to_string()), (header::CONTENT_DISPOSITION, disposition)], bytes).into_response()
        }
        Ok(Ok(Err(f))) => f.status(StatusCode::BAD_REQUEST),
        _ => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 导出这个项目用到的补装宏包：看最近一次编译的 `-recorder` 记录，用户层里被读过的文件属于哪些包，
/// 就打哪些。快照放进项目根目录，跟项目一起带走，离线机器上打开项目就能导入
async fn project_snapshot_export(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    if let Some(r) = s.read_only(&q) {
        return r;
    }
    // 最近一次成功的编译；都失败了就用最近一次（缺字也照样记下了读过的文件）
    let last = {
        let jobs = q.jobs.lock().unwrap();
        let pick = |ok: bool| jobs.order.iter().rev().find(|j| jobs.map.get(*j).is_some_and(|x| x.ok || !ok)).cloned();
        pick(true).or_else(|| pick(false)).and_then(|j| jobs.map.get(&j).map(|x| (j.clone(), x.entry.clone())))
    };
    let Some((jid, entry)) = last else {
        return Fail::new("case.snapshotNeedsBuild", "先编译一次，才知道这个项目用到了哪些宏包").status(StatusCode::CONFLICT);
    };
    if let Err(r) = begin_user_change(&s) {
        return r;
    }
    let name = q.root.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| "project".into());
    let file = format!("{name}.{}", snapshot::EXT);
    let (s2, out) = (s.clone(), q.root.join(&file));
    let fls = snapshot::recorder_file(&press_dir(&q, &jid), &entry);
    let res = tokio::task::spawn_blocking(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let home = case::user_home().ok_or_else(|| Fail::new("case.unmanaged", "用的是本机自己的 TeX，宏包请用 tlmgr 管理"))?;
            let text = std::fs::read_to_string(&fls).unwrap_or_default();
            let pkgs = snapshot::packages_used(&text, &home, &case::user_file_owners());
            if pkgs.is_empty() {
                return Err(Fail::new("case.snapshotEmpty", "没有要导出的宏包：用到的都是内置的"));
            }
            snapshot::export(Some(&pkgs), Some(&name), &out)
        }));
        *s2.installing.lock().unwrap() = false;
        r
    })
    .await;
    match res {
        Ok(Ok(Ok(done))) => Json(json!({ "path": file, "packages": done.packages, "files": done.files, "bytes": done.bytes })).into_response(),
        Ok(Ok(Err(f))) => f.status(StatusCode::BAD_REQUEST),
        _ => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 导入快照：核对完再写进用户层，登记字体映射，装上的出待取队列
async fn import_snapshot(s: Shared, bytes: Vec<u8>) -> Response {
    if let Err(r) = begin_user_change(&s) {
        return r;
    }
    let s2 = s.clone();
    let res = tokio::task::spawn_blocking(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let scratch = subo_dir().join("case").join(format!("import-{}", std::process::id()));
            let mut done = snapshot::import(&bytes, &scratch)?;
            if !done.packages.is_empty() {
                fontmap::refresh();
                if let Err(e) = case::register_user_maps() {
                    eprintln!("[case] 导入快照后登记字体映射失败：{e}");
                }
                s2.queue.lock().unwrap().remove(&done.packages, &[]);
            }
            done.packages.sort();
            Ok::<_, Fail>(done)
        }));
        *s2.installing.lock().unwrap() = false;
        r
    })
    .await;
    match res {
        Ok(Ok(Ok(done))) => Json(done).into_response(),
        Ok(Ok(Err(f))) => f.status(StatusCode::BAD_REQUEST),
        _ => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 设置页上传的快照
async fn case_snapshot_import(State(s): State<Shared>, body: Bytes) -> Response {
    import_snapshot(s, body.to_vec()).await
}

/// 项目里带着的快照（`?path=` 相对项目根）
async fn project_snapshot_import(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let Some(rel) = qs.get("path") else {
        return missing_param("path");
    };
    if !rel.ends_with(&format!(".{}", snapshot::EXT)) {
        return Fail::new("case.snapshotBad", "不是 Subo 字盘快照").status(StatusCode::BAD_REQUEST);
    }
    let bytes = match project::resolve(&q.root, rel).and_then(|p| std::fs::read(p).map_err(|e| Fail::new("file.readFailed", format!("读取失败: {e}")).detail(e))) {
        Ok(b) => b,
        Err(e) => return e.status(StatusCode::BAD_REQUEST),
    };
    import_snapshot(s, bytes).await
}

/// 设置页卸载补装的宏包（字体页卸载字体也走这里，传字体所属的包）
async fn case_remove(State(s): State<Shared>, Json(body): Json<PackagesBody>) -> Response {
    if let Err(r) = begin_user_change(&s) {
        return r;
    }
    let s2 = s.clone();
    let res = tokio::task::spawn_blocking(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let r = case::remove(&body.packages);
            // 卸掉的可能是字体包
            fontmap::refresh();
            r
        }));
        *s2.installing.lock().unwrap() = false;
        r
    })
    .await;
    match res {
        Ok(Ok(Ok(log))) => Json(json!({ "ok": true, "log": log })).into_response(),
        Ok(Ok(Err(msg))) => Fail::new("case.removeFailed", msg.clone()).detail(msg).status(StatusCode::BAD_REQUEST),
        _ => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 设置页重装补装的宏包：从安装源重新下载覆盖。进度与缺字清单安装共用
async fn case_reinstall(State(s): State<Shared>, Json(body): Json<PackagesBody>) -> Response {
    if let Err(r) = begin_user_change(&s) {
        return r;
    }
    let mut pkgs = body.packages;
    pkgs.sort();
    pkgs.dedup();
    case::progress_start(&pkgs);
    let s2 = s.clone();
    let (repo, fallback) = (s.repository(), s.fallback());
    let res = tokio::task::spawn_blocking(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut r = case::reinstall(&pkgs, repo.as_deref(), fallback);
            if let Ok(done) = &mut r {
                // 取消了就没东西要刷新，别让界面显示「刷新索引」
                if !done.cancelled {
                    case::track(|p| p.phase = "finish");
                }
                fontmap::refresh();
                if done.ok {
                    if let Err(e) = case::register_user_maps() {
                        done.log.push_str(&format!("\n{e}"));
                    }
                }
            }
            r
        }));
        case::progress_done();
        *s2.installing.lock().unwrap() = false;
        r
    })
    .await;
    match res {
        Ok(Ok(Ok(done))) => Json(done).into_response(),
        Ok(Ok(Err(msg))) => Fail::new("case.installFailed", msg.clone()).detail(msg).status(StatusCode::BAD_REQUEST),
        _ => Fail::new("case.installCrashed", "安装过程异常中断").status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 设置页的「字体」：TeX Live 自带、用户层、系统里的字体族。`?rescan=1` 重新扫描
async fn case_fonts(Query(q): Query<HashMap<String, String>>) -> Response {
    let rescan = q.get("rescan").is_some_and(|v| v == "1");
    match tokio::task::spawn_blocking(move || fontmap::catalog(rescan)).await {
        Ok(list) => Json(list).into_response(),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 设置页预览字体：按族名、来源、样式取清单里的那个文件
async fn case_font_file(Query(q): Query<HashMap<String, String>>) -> Response {
    let arg = |k: &str| q.get(k).cloned().unwrap_or_default();
    let (name, source, style) = (arg("name"), arg("source"), arg("style"));
    let found = tokio::task::spawn_blocking(move || {
        let path = fontmap::preview_file(&name, &source, &style)?;
        let mime = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("otf")) { "font/otf" } else { "font/ttf" };
        Some((mime, std::fs::read(path).ok()?))
    })
    .await
    .ok()
    .flatten();
    match found {
        Some((mime, bytes)) => ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "max-age=3600")], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// 设置 + 设置页要展示的本机环境
async fn get_settings(State(s): State<Shared>) -> Response {
    let (prefs, home) = (s.prefs().clone(), s.home());
    // tlmgr 要起两次子进程（约半秒），放到 blocking 线程
    let env = tokio::task::spawn_blocking(move || {
        json!({
            "texRoot": case::texlive_root().map(|p| p.to_string_lossy().into_owned()),
            "texVersion": case::texlive_version(),
            "tlmgrRepository": case::tlmgr_repository(),
            "archive": case::archive_in_use(),
        })
    })
    .await
    .unwrap_or(Value::Null);
    let presets: Vec<Value> = settings::PRESETS
        .iter()
        .map(|(id, url, region)| json!({ "id": id, "url": url, "region": region }))
        .collect();
    Json(json!({
        "settings": prefs,
        "presets": presets,
        "env": {
            "engine": s.engine,
            "bundledTex": tex::bundled(),
            "library": home.to_string_lossy(),
            "defaultLibrary": library::default_home().to_string_lossy(),
            "libraryLocked": s.library_locked,
            "suboDir": library::subo_dir().to_string_lossy(),
            "defaultTimeout": s.default_timeout.as_secs(),
            "timeoutRange": [settings::TIMEOUT_MIN, settings::TIMEOUT_MAX],
            "defaultMemory": settings::MEMORY_DEFAULT,
            "memoryRange": [settings::MEMORY_MIN, settings::MEMORY_MAX],
            "cpus": machine::cpus(),
            "totalMemory": machine::memory_mb(),
            "parallelRange": [settings::PARALLEL_MIN, machine::cpus()],
            "tex": env,
        },
    }))
    .into_response()
}

/// 改设置：只带要改的字段。`timeoutSecs: null` 恢复默认。项目库位置走单独的接口
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsPatch {
    auto_install: Option<bool>,
    mirror: Option<String>,
    custom_mirror: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    timeout_secs: Option<Option<u64>>,
    #[serde(default, deserialize_with = "nullable")]
    memory_mb: Option<Option<u64>>,
    #[serde(default, deserialize_with = "nullable")]
    parallel: Option<Option<u64>>,
    tex: Option<tex::Choice>,
    update_check: Option<bool>,
    update_download: Option<bool>,
    update_source: Option<String>,
}

/// 区分「没带这个字段」和「带了 null」
fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<u64>>, D::Error> {
    Option::<u64>::deserialize(d).map(Some)
}

async fn put_settings(State(s): State<Shared>, Json(patch): Json<SettingsPatch>) -> Response {
    let mut cur = s.prefs();
    let mut next = cur.clone();
    if let Some(v) = patch.auto_install {
        next.auto_install = v;
    }
    if let Some(v) = patch.custom_mirror {
        next.custom_mirror = v.trim().to_string();
    }
    if let Some(v) = patch.mirror {
        next.mirror = v;
    }
    if let Some(v) = patch.timeout_secs {
        if v.is_some_and(|t| !(settings::TIMEOUT_MIN..=settings::TIMEOUT_MAX).contains(&t)) {
            return Fail::new("settings.badTimeout", "超时超出范围")
                .with("min", settings::TIMEOUT_MIN)
                .with("max", settings::TIMEOUT_MAX)
                .status(StatusCode::BAD_REQUEST);
        }
        next.timeout_secs = v;
    }
    if let Some(v) = patch.memory_mb {
        if v.is_some_and(|m| !(settings::MEMORY_MIN..=settings::MEMORY_MAX).contains(&m)) {
            return Fail::new("settings.badMemory", "内存上限超出范围")
                .with("min", settings::MEMORY_MIN)
                .with("max", settings::MEMORY_MAX)
                .status(StatusCode::BAD_REQUEST);
        }
        next.memory_mb = v;
    }
    if let Some(v) = patch.parallel {
        let max = machine::cpus();
        if v.is_some_and(|n| !(settings::PARALLEL_MIN..=max).contains(&n)) {
            return Fail::new("settings.badParallel", "并行编译数超出范围")
                .with("min", settings::PARALLEL_MIN)
                .with("max", max)
                .status(StatusCode::BAD_REQUEST);
        }
        next.parallel = v;
    }
    if let Some(v) = patch.tex {
        next.tex = v;
    }
    if let Some(v) = patch.update_check {
        next.update_check = v;
    }
    if let Some(v) = patch.update_download {
        next.update_download = v;
    }
    if let Some(v) = patch.update_source {
        let v = v.trim().to_string();
        if let Err(code) = settings::check_update_source(&v) {
            return Fail::new(code, "版本清单地址要是 https 网址").status(StatusCode::BAD_REQUEST);
        }
        next.update_source = v;
    }
    if let Err(code) = settings::check_mirror(&next.mirror, &next.custom_mirror) {
        return Fail::new(code, "安装源地址不可用").status(StatusCode::BAD_REQUEST);
    }
    if let Err(e) = next.save(&s.settings_file) {
        return Fail::new("settings.saveFailed", format!("设置保存失败：{e}")).detail(e).status(StatusCode::INTERNAL_SERVER_ERROR);
    }
    *cur = next.clone();
    drop(cur);
    s.prefs_saved();
    // 换 TeX 立即生效，正在跑的编译用完原来那套
    if patch.tex.is_some() {
        tex::select(next.tex);
    }
    Json(next).into_response()
}

/// 用哪套 TeX、本机 TeX 的版本：设置页和出错卡片看
async fn get_tex(State(s): State<Shared>) -> Response {
    let choice = s.prefs().tex;
    Json(tex::status(choice)).into_response()
}

/// 用户在别处（MiKTeX Console、官网安装包）更新过 TeX：重读内核日期，自动档跟着重选
async fn refresh_tex(State(s): State<Shared>) -> Response {
    let choice = s.prefs().tex;
    let _ = tokio::task::spawn_blocking(move || {
        tex::refresh_own();
        tex::select(choice);
    })
    .await;
    Json(tex::status(choice)).into_response()
}

/// 打开 MiKTeX Console：要管理员权限、MiKTeX 太旧要升级时交给它
async fn open_miktex_console() -> Response {
    match tokio::task::spawn_blocking(tex::open_miktex_console).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => Fail::new("tex.miktex.noConsole", "找不到 MiKTeX Console").detail(e).status(StatusCode::NOT_FOUND),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 更新 MiKTeX（内核和宏包一起），修「宏包比内核新」。要几分钟
/// 设置页问本机 tlmgr 旧不旧（要下载软件源的包数据库，几秒）。
/// `state`：`na` 用的是内置 TeX 或不是 TeX Live，用不着；`current` 不旧；`stale` 旧了（附两边修订号）；
/// `unknown` 问不到（离线等，`detail` 是 tlmgr 的输出）。`admin`：更新要管理员授权
async fn tlmgr_status(State(s): State<Shared>) -> Response {
    let repo = s.repository();
    let res = tokio::task::spawn_blocking(move || {
        if tex::bundled() || case::texlive_root().is_none() {
            return json!({ "state": "na" });
        }
        let admin = case::needs_admin(false);
        match case::tlmgr_stale(repo.as_deref()) {
            Ok(None) => json!({ "state": "current", "admin": admin }),
            Ok(Some(st)) => json!({ "state": "stale", "admin": admin, "local": st.local, "remote": st.remote }),
            Err(e) => json!({ "state": "unknown", "admin": admin, "detail": e }),
        }
    })
    .await;
    match res {
        Ok(v) => Json(v).into_response(),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 设置页的「更新 tlmgr」：`tlmgr update --self`，和宏包安装共用一把锁
async fn update_tlmgr(State(s): State<Shared>) -> Response {
    if tex::bundled() {
        return Fail::new("case.unmanaged", "用的是内置 TeX，用不着更新 tlmgr").status(StatusCode::CONFLICT);
    }
    {
        let mut busy = s.installing.lock().unwrap();
        if *busy {
            return Fail::new("case.busy", "已有一次安装在进行").status(StatusCode::CONFLICT);
        }
        *busy = true;
    }
    let (repo, fallback) = (s.repository(), s.fallback());
    let res = tokio::task::spawn_blocking(move || case::update_tlmgr(repo.as_deref(), fallback)).await;
    *s.installing.lock().unwrap() = false;
    match res {
        Ok(Ok(done)) => Json(done).into_response(),
        Ok(Err(msg)) => Fail::new("case.installFailed", msg.clone()).detail(msg).status(StatusCode::BAD_REQUEST),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn update_miktex(State(s): State<Shared>) -> Response {
    {
        let mut busy = s.installing.lock().unwrap();
        if *busy {
            return Fail::new("case.busy", "已有一次安装在进行").status(StatusCode::CONFLICT);
        }
        *busy = true;
    }
    let res = tokio::task::spawn_blocking(|| tex::update_miktex(Instant::now() + Duration::from_secs(30 * 60))).await;
    *s.installing.lock().unwrap() = false;
    let choice = s.prefs().tex;
    match res {
        Ok(Ok(log)) => {
            // 内核变新了，自动档可能该换回本机的
            tex::select(choice);
            Json(json!({ "ok": true, "log": log, "tex": tex::status(choice) })).into_response()
        }
        Ok(Err(f)) => {
            let code = match f.reason {
                "admin" => "tex.miktex.admin",
                "tooOld" => "tex.miktex.tooOld",
                "offline" => "tex.miktex.offline",
                "noTool" => "tex.miktex.noTool",
                _ => "tex.miktex.failed",
            };
            Fail::new(code, "MiKTeX 更新失败").with("log", f.log).status(StatusCode::BAD_GATEWAY)
        }
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryBody {
    /// `None`：恢复默认位置
    path: Option<String>,
    /// 把现有项目和模板一起搬过去
    #[serde(default)]
    move_projects: bool,
}

/// 换项目库位置。立即生效：之后的列表、新建、导入都在新位置
async fn set_library(State(s): State<Shared>, Json(body): Json<LibraryBody>) -> Response {
    if s.library_locked {
        return Fail::new("settings.libraryLocked", "项目库位置由启动参数指定").status(StatusCode::CONFLICT);
    }
    let raw = body.path.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let target = match raw {
        Some(p) => match p.strip_prefix("~/") {
            Some(rest) => library::home_dir().unwrap_or_default().join(rest),
            None => PathBuf::from(p),
        },
        None => library::default_home(),
    };
    let current = s.home();
    // 编译中的项目在旧库里，搬走会让它写到一半的产物对不上
    if body.move_projects
        && s.quires.lock().unwrap().values().any(|q| q.root.starts_with(&current) && q.press.lock().unwrap().running)
    {
        return Fail::new("settings.libraryBusy", "有项目正在编译").status(StatusCode::CONFLICT);
    }
    let next = match settings::check_library(&target, &current) {
        Ok(p) => p,
        Err(code) => return Fail::new(code, "这个位置不能用作项目库").status(StatusCode::BAD_REQUEST),
    };
    let (moved, left) = if body.move_projects {
        let (from, to) = (current.clone(), next.clone());
        tokio::task::spawn_blocking(move || settings::move_library(&from, &to)).await.unwrap_or_default()
    } else {
        (0, Vec::new())
    };
    {
        let mut prefs = s.prefs();
        let mut updated = prefs.clone();
        // 选回默认位置就清掉，默认位置以后变了（比如 SUBO_HOME）也跟着走
        let default = library::default_home();
        let default = dunce::canonicalize(&default).unwrap_or(default);
        updated.library = raw.map(|_| next.clone()).filter(|p| *p != default);
        if let Err(e) = updated.save(&s.settings_file) {
            return Fail::new("settings.saveFailed", format!("设置保存失败：{e}")).detail(e).status(StatusCode::INTERNAL_SERVER_ERROR);
        }
        *prefs = updated;
        drop(prefs);
        s.prefs_saved();
    }
    *s.library.write().unwrap() = next.clone();
    // 旧库里项目的工位作废；本地文件夹不受影响
    s.quires.lock().unwrap().retain(|id, _| folders::is_folder_id(id));
    Json(json!({ "library": next.to_string_lossy(), "moved": moved, "left": left })).into_response()
}

#[derive(Deserialize)]
struct ProbeBody {
    /// 额外要测的地址（自定义源）
    #[serde(default)]
    custom: Option<String>,
}

/// 安装源测速：并发探测每个预置源（加上自定义源、tlmgr 自己的源），返回毫秒；不通为 null
async fn probe_mirrors(Json(body): Json<ProbeBody>) -> Response {
    let mut targets: Vec<(String, String)> =
        settings::PRESETS.iter().map(|(id, url, _)| (id.to_string(), url.to_string())).collect();
    if let Some(c) = body.custom.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) {
        if settings::check_mirror(settings::MIRROR_CUSTOM, &c).is_ok() {
            targets.push((settings::MIRROR_CUSTOM.into(), c));
        }
    }
    let res = tokio::task::spawn_blocking(move || {
        if let Some(own) = case::tlmgr_repository() {
            targets.push((settings::MIRROR_TLMGR.into(), own));
        }
        let handles: Vec<_> = targets
            .into_iter()
            .map(|(id, url)| std::thread::spawn(move || (id, settings::probe(&url, Duration::from_secs(6)))))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect::<HashMap<String, Option<u64>>>()
    })
    .await;
    match res {
        Ok(map) => Json(map).into_response(),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
struct RevealBody {
    /// `library`：项目库；`subo`：Subo 的配置与缓存
    what: String,
}

async fn reveal_dir(State(s): State<Shared>, Json(body): Json<RevealBody>) -> Response {
    let dir = match body.what.as_str() {
        "library" => Some(s.home()),
        "subo" => Some(library::subo_dir()),
        "appearance" => Some(appearance::root(&library::subo_dir())),
        _ => None,
    };
    let Some(dir) = dir else { return missing_param("what") };
    let _ = std::fs::create_dir_all(&dir);
    match tokio::task::spawn_blocking(move || folders::reveal(&dir)).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => e.status(StatusCode::INTERNAL_SERVER_ERROR),
        Err(_) => fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/* ---------- 外观与语言包 ---------- */

async fn get_appearance() -> Response {
    let subo = library::subo_dir();
    Json(json!({
        "prefs": appearance::load_prefs(&subo),
        "themes": appearance::list(&subo, appearance::Kind::Theme),
        "locales": appearance::list(&subo, appearance::Kind::Locale),
        "dir": appearance::root(&subo).to_string_lossy(),
    }))
    .into_response()
}

async fn put_appearance_prefs(Json(patch): Json<serde_json::Map<String, Value>>) -> Response {
    match appearance::save_prefs(&library::subo_dir(), &patch) {
        Ok(p) => Json(p).into_response(),
        Err(e) => Fail::new("settings.saveFailed", format!("设置保存失败：{e}")).detail(e).status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn pack_target(kind: &str, id: &str) -> Result<appearance::Kind, Response> {
    let kind = appearance::Kind::parse(kind).ok_or_else(|| missing_param("kind"))?;
    if !appearance::valid_id(id) {
        return Err(Fail::new("appearance.badId", "包的标识不合法").status(StatusCode::BAD_REQUEST));
    }
    Ok(kind)
}

async fn put_pack(Path((kind, id)): Path<(String, String)>, body: axum::body::Bytes) -> Response {
    let kind = match pack_target(&kind, &id) {
        Ok(k) => k,
        Err(r) => return r,
    };
    if body.len() > appearance::MAX_PACK_BYTES {
        return Fail::new("appearance.tooLarge", "文件太大")
            .with("max", appearance::MAX_PACK_BYTES / 1024 / 1024)
            .status(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let Ok(v) = serde_json::from_slice::<Value>(&body) else {
        return Fail::new("appearance.notPack", "不是外观或语言包").status(StatusCode::BAD_REQUEST);
    };
    match appearance::check(kind, &v) {
        Ok(own) if own == id => {}
        Ok(_) => return Fail::new("appearance.badId", "包的标识不合法").status(StatusCode::BAD_REQUEST),
        Err(code) => return Fail::new(code, "不是外观或语言包").status(StatusCode::BAD_REQUEST),
    }
    match appearance::save(&library::subo_dir(), kind, &id, &v) {
        Ok(()) => Json(json!({ "id": id })).into_response(),
        Err(e) => Fail::new("appearance.saveFailed", format!("保存失败：{e}")).detail(e).status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn remove_pack(Path((kind, id)): Path<(String, String)>) -> Response {
    let kind = match pack_target(&kind, &id) {
        Ok(k) => k,
        Err(r) => return r,
    };
    match appearance::remove(&library::subo_dir(), kind, &id) {
        Ok(removed) => Json(json!({ "removed": removed })).into_response(),
        Err(e) => Fail::new("appearance.saveFailed", format!("删除失败：{e}")).detail(e).status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn press_dir(q: &Quire, jid: &str) -> PathBuf {
    q.out.join(jid)
}

async fn events(
    State(s): State<Shared>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = s.tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|m| match m {
        Ok(text) => Some(Ok(Event::default().data(text))),
        Err(_) => None,
    });
    Sse::new(stream)
}

async fn pdf(State(s): State<Shared>, Path((id, job)): Path<(String, String)>) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let job = job_id(&q, &job);
    let path = q.jobs.lock().unwrap().map.get(&job).and_then(|j| j.pdf.clone());
    match path {
        Some(p) => match std::fs::read(&p) {
            Ok(bytes) => ([(header::CONTENT_TYPE, "application/pdf")], bytes).into_response(),
            Err(e) => Fail::new("pdf.readFailed", format!("读取 PDF 失败: {e}")).detail(e).status(StatusCode::NOT_FOUND),
        },
        None => Fail::new("pdf.none", "没有可用的 PDF").status(StatusCode::NOT_FOUND),
    }
}

/// 导出某次编译的 PDF：复制到项目根目录，与入口同名（`thesis.tex` → `thesis.pdf`），已有的直接覆盖。
async fn export_pdf(State(s): State<Shared>, Path((id, job)): Path<(String, String)>) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    if let Some(r) = s.read_only(&q) {
        return r;
    }
    let job = job_id(&q, &job);
    let Some(src) = q.jobs.lock().unwrap().map.get(&job).and_then(|j| j.pdf.clone()) else {
        return Fail::new("pdf.none", "没有可用的 PDF").status(StatusCode::NOT_FOUND);
    };
    let Some(entry) = project::entry(&q.root) else {
        return Fail::new("build.noTex", "项目里没有找到 .tex 文件").status(StatusCode::BAD_REQUEST);
    };
    let name = export_name(&entry);
    match std::fs::copy(&src, q.root.join(&name)) {
        Ok(_) => Json(json!({ "path": name })).into_response(),
        Err(e) => Fail::new("export.writeFailed", format!("写入 PDF 失败: {e}"))
            .detail(e)
            .status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 入口在子目录里也只取文件名，导出物统一放项目根目录
fn export_name(entry: &str) -> String {
    let stem = std::path::Path::new(entry).file_stem().map(|s| s.to_string_lossy().to_string());
    format!("{}.pdf", stem.filter(|s| !s.is_empty()).unwrap_or_else(|| "main".into()))
}

async fn log(State(s): State<Shared>, Path((id, job)): Path<(String, String)>) -> Response {
    let Some(q) = s.quire(&id) else { return not_found() };
    let job = job_id(&q, &job);
    let jobs = q.jobs.lock().unwrap();
    match jobs.map.get(&job) {
        Some(j) => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("{}\n\n[ok={}]", j.log, j.ok),
        )
            .into_response(),
        None => Fail::new("job.notFound", "没有该任务").status(StatusCode::NOT_FOUND),
    }
}

/// 读出某次编译的 SyncTeX；没有（如 tectonic、mock 引擎）返回 501
fn load_synctex(s: &AppState, id: &str, job: &str) -> Result<synctex::Synctex, Response> {
    let q = s.quire(id).ok_or_else(not_found)?;
    let job = &job_id(&q, job);
    let pdf = q.jobs.lock().unwrap().map.get(job).and_then(|j| j.pdf.clone());
    let pdf = pdf.ok_or_else(|| Fail::new("pdf.none", "没有该任务的 PDF").status(StatusCode::NOT_FOUND))?;
    let file = synctex::locate(&pdf).ok_or_else(|| {
        Fail::new("synctex.unavailable", format!("这次编译没有 SyncTeX 数据（引擎 {}）", s.engine))
            .with("engine", s.engine.as_str())
            .status(StatusCode::NOT_IMPLEMENTED)
    })?;
    synctex::Synctex::load(&file, &q.root)
        .map_err(|e| {
            Fail::new("synctex.readFailed", format!("读取 SyncTeX 失败: {e}"))
                .detail(e)
                .status(StatusCode::INTERNAL_SERVER_ERROR)
        })
}

/// 源码 → 预览：?file=src/intro.tex&line=12
async fn synctex_view(
    State(s): State<Shared>,
    Path((id, job)): Path<(String, String)>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let (Some(file), Some(line)) = (qs.get("file").cloned(), qs.get("line").and_then(|l| l.parse().ok()))
    else {
        return missing_param("file, line");
    };
    let found = tokio::task::spawn_blocking(move || {
        load_synctex(&s, &id, &job).map(|sx| sx.view(&file, line))
    })
    .await
    .unwrap_or_else(|_| Err(fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR)));
    match found {
        Ok(Some(spot)) => Json(spot).into_response(),
        Ok(None) => Fail::new("synctex.noSpot", "PDF 里找不到这一行").status(StatusCode::NOT_FOUND),
        Err(r) => r,
    }
}

/// 预览 → 源码：?page=3&x=120.5&y=300（bp，页左上为原点）
async fn synctex_edit(
    State(s): State<Shared>,
    Path((id, job)): Path<(String, String)>,
    Query(qs): Query<HashMap<String, String>>,
) -> Response {
    let num = |k: &str| qs.get(k).and_then(|v| v.parse::<f64>().ok());
    let (Some(page), Some(x), Some(y)) = (num("page"), num("x"), num("y")) else {
        return missing_param("page, x, y");
    };
    let found = tokio::task::spawn_blocking(move || {
        load_synctex(&s, &id, &job).map(|sx| sx.edit(page as u32, x, y))
    })
    .await
    .unwrap_or_else(|_| Err(fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR)));
    match found {
        Ok(Some(src)) => Json(src).into_response(),
        Ok(None) => Fail::new("synctex.noSource", "这里对应不到项目里的源码").status(StatusCode::NOT_FOUND),
        Err(r) => r,
    }
}

#[cfg(test)]
mod tests {
    use super::{error_lines, export_name, logparse};

    #[test]
    fn log_error_lines() {
        let log = "This is XeTeX\n./main.tex:3: Undefined control sequence.\nl.3 Hi \\foo\n\
                   ! LaTeX Error: File `x.sty' not found.\n./main.tex:3: Undefined control sequence.\n\
                   Package hyperref Warning: Token not allowed: 1 2\n";
        let issues = serde_json::to_value(logparse::parse(log).issues).unwrap();
        assert_eq!(error_lines(&issues), ["main.tex:3: Undefined control sequence.", "File `x.sty' not found."]);
    }

    #[test]
    fn export_named_after_entry() {
        assert_eq!(export_name("main.tex"), "main.pdf");
        assert_eq!(export_name("src/论文.tex"), "论文.pdf");
        assert_eq!(export_name("v1.2.tex"), "v1.2.pdf");
    }
}
