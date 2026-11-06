//! 命令行：一次性的命令，不起服务、不占端口，给人用，也给 agent 用。
//! `subo-server <命令>` 和桌面端的 `subo <命令>`（用随应用内置的 TeX）走的都是这里。
//!
//! 约定（docs/cli.md 有完整说明）：
//! - 输入简单：一个目录或几个名字，选项不超过几个，没有要拼的 JSON
//! - 输出简单：默认是固定格式的文本；`--json` 输出一个平铺的 JSON 对象，值只有字符串、数字、
//!   布尔和字符串数组，没有嵌套对象、没有对象数组
//! - 失败时第一行就是结论：哪里坏了、为什么（`error`/`message`），怎么修（`hint`，一句话），
//!   再列原始错误行（`errors`）和照着跑就行的下一步（`next`，只放命令）
//! - 退出码：0 成功；1 文档没编过；2 用法或输入不对；3 环境问题（没有 TeX、装不了包、离线）
//! - stdout 只放结果，进度走 stderr（`--json` 时 stderr 也不出声）；不交互、不弹窗

use crate::{case, diagnose, engine, logparse, engines, export_name, folders, fontmap, hash_name, library, project, projprefs, settings, templates};
use engine::program::{Program, Reason, Source};
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

/// 认得的命令。`subo-server` 和桌面壳据此判断第一个参数是命令还是启动参数
pub const COMMANDS: &[&str] = &["build", "install", "doctor", "inspect", "mirror", "templates", "new", "help"];

pub fn is_command(arg: &str) -> bool {
    COMMANDS.contains(&arg)
}

/// 第一个参数该不该交给命令行：不是 `-` 开头的就是（拼错的命令也在这里报错并列出可用的，
/// 不落到启动参数的解析里）
pub fn wants(first: &str) -> bool {
    !first.starts_with('-')
}

const ENGINES: &str = "auto、pdflatex、xelatex、lualatex、platex、uplatex";
/// `build --install` 最多装几轮：装完一批，重编又报出新缺的，再装
const INSTALL_ROUNDS: usize = 3;
/// `errors` 最多列几条；再多的看日志
const ERRORS_MAX: usize = 10;
/// 内容提醒（未定义的引用、缺字、溢出）每类最多列几条
const NOTES_MAX: usize = 20;
/// 测速时每个源最多等多久
const PROBE_SECS: u64 = 8;

/// 失败：`code` 稳定可分支，`message` 说清哪里坏了、为什么，`hint` 一句话说怎么修，
/// `next` 是可以直接跑的命令
struct Failure {
    code: &'static str,
    message: String,
    hint: Option<String>,
    next: Vec<String>,
    exit: i32,
}

impl Failure {
    fn usage(code: &'static str, message: impl Into<String>) -> Self {
        Failure { code, message: message.into(), hint: None, next: Vec::new(), exit: 2 }
    }
    fn env(code: &'static str, message: impl Into<String>) -> Self {
        Failure { code, message: message.into(), hint: None, next: Vec::new(), exit: 3 }
    }
    fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
    fn next(mut self, cmd: impl Into<String>) -> Self {
        self.next.push(cmd.into());
        self
    }
}

/// 一条结果：平铺的字段，按插入顺序输出（`ok` 总在最前）。只放字符串、数字、布尔、字符串数组
#[derive(Default)]
struct Out(Map<String, Value>, Vec<String>);

impl Out {
    fn set(&mut self, k: &str, v: impl Into<Value>) -> &mut Self {
        if self.0.insert(k.to_string(), v.into()).is_none() {
            self.1.push(k.to_string());
        }
        self
    }

    fn to_json(&self) -> String {
        let fields: Vec<String> = self.1.iter().map(|k| format!("{}:{}", json!(k), self.0[k])).collect();
        format!("{{{}}}", fields.join(","))
    }
    fn list(&mut self, k: &str, v: &[String]) -> &mut Self {
        self.set(k, json!(v))
    }
    fn str(&self, k: &str) -> &str {
        self.0.get(k).and_then(Value::as_str).unwrap_or_default()
    }
    fn strs(&self, k: &str) -> Vec<&str> {
        self.0.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect()
    }
    fn is(&self, k: &str) -> bool {
        self.0.get(k) == Some(&json!(true))
    }
}

type Res = Result<(Out, i32), Failure>;

/// 位置参数 + `--flag [值]`。命令声明自己认哪些 flag，别的一律报错并列出能用的
struct Args {
    pos: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(cmd: &str, raw: &[String], valued: &[&str], switches: &[&str], max_pos: usize) -> Result<Args, Failure> {
        let known = || {
            let all: Vec<String> = valued.iter().map(|f| format!("--{f} <值>")).chain(switches.iter().map(|f| format!("--{f}"))).collect();
            if all.is_empty() { format!("{cmd} 没有选项") } else { format!("{cmd} 能用的选项：{}", all.join("  ")) }
        };
        let mut pos = Vec::new();
        let mut flags = Vec::new();
        let mut it = raw.iter();
        while let Some(a) = it.next() {
            let Some(name) = a.strip_prefix("--") else {
                pos.push(a.clone());
                continue;
            };
            let (name, inline) = match name.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (name, None),
            };
            if valued.contains(&name) {
                let v = match inline.or_else(|| it.next().cloned()) {
                    Some(v) => v,
                    None => return Err(Failure::usage("cli.missingValue", format!("--{name} 后面要跟一个值")).hint(known())),
                };
                flags.push((name.to_string(), Some(v)));
            } else if switches.contains(&name) && inline.is_none() {
                flags.push((name.to_string(), None));
            } else {
                return Err(Failure::usage("cli.unknownFlag", format!("{cmd} 不认识选项 --{name}")).hint(known()));
            }
        }
        if pos.len() > max_pos {
            return Err(Failure::usage("cli.tooManyArgs", format!("{cmd} 多了参数：{}", pos[max_pos..].join(" ")))
                .hint(format!("路径里有空格要加引号；用法见 {} help {cmd}", prog())));
        }
        Ok(Args { pos, flags })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags.iter().find(|(n, _)| n == name).and_then(|(_, v)| v.as_deref())
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }
}

/// 一次命令要用的环境：用户在素帛里的设置（下载源、超时、内存上限）、宏包索引、TeX 是不是内置版
struct Ctx {
    prog: String,
    json: bool,
    settings: settings::Settings,
    /// `--mirror` 给的下载源：只这一次用，不存进设置
    mirror: Mutex<Option<settings::Settings>>,
    case: Mutex<case::Index>,
}

impl Ctx {
    /// 这次用的下载源设置：`--mirror` 优先，否则是用户设置里的
    fn source(&self) -> settings::Settings {
        self.mirror.lock().unwrap().clone().unwrap_or_else(|| self.settings.clone())
    }

    fn repo(&self) -> Option<String> {
        self.source().repository()
    }

    fn fallback(&self) -> case::Fallback {
        self.source().fallback()
    }

    /// 认下 `--mirror`；没给就什么都不做
    fn use_mirror(&self, spec: Option<&str>) -> Result<(), Failure> {
        let Some(spec) = spec else { return Ok(()) };
        let (mirror, custom) = settings::parse_mirror(spec).map_err(|code| bad_mirror(&self.prog, code, spec))?;
        let mut s = self.settings.clone();
        s.mirror = mirror;
        s.custom_mirror = custom;
        *self.mirror.lock().unwrap() = Some(s);
        Ok(())
    }

    /// 用的是素帛内置的 TeX Live：补装宏包装进用户层
    fn bundled(&self) -> bool {
        crate::tex::bundled()
    }

    /// 认下 `--tex auto|own|bundled`：只这一次用，不存进设置
    fn use_tex(&self, spec: Option<&str>) -> Result<(), Failure> {
        let Some(spec) = spec else { return Ok(()) };
        let choice = crate::tex::Choice::parse(spec).ok_or_else(|| {
            Failure::usage("cli.badTex", format!("--tex 不认识 {spec}")).hint("可选 auto（默认，本机 TeX 太旧时用内置的）、own（本机的）、bundled（素帛内置的）")
        })?;
        if crate::tex::swap(choice) {
            crate::tex::after_switch();
        }
        Ok(())
    }

    fn home(&self) -> PathBuf {
        self.settings.library.clone().filter(|p| p.is_dir()).unwrap_or_else(library::default_home)
    }

    /// 进度写 stderr；`--json` 时不出声，stdout 只有结果
    fn note(&self, msg: impl AsRef<str>) {
        if !self.json {
            eprintln!("{}", msg.as_ref());
        }
    }
}

/// 提示里拼命令用的程序名（`subo` 或 `subo-server`），`run` 开头设好
static PROG: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn prog() -> &'static str {
    PROG.get().map(String::as_str).unwrap_or("subo")
}

/// 入口。`args` 不含程序名，第一个是命令；`prog` 是给提示里拼命令用的程序名。
/// `bundled_tex` 同 `Options::bundled_tex`，桌面壳已调过 [`crate::tex::init`] 时不看。返回退出码
pub fn run(prog: &str, args: &[String], bundled_tex: bool) -> i32 {
    let _ = PROG.set(prog.to_string());
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<String> = args.iter().filter(|a| *a != "--json").cloned().collect();
    let Some((cmd, rest)) = rest.split_first() else {
        print_help(prog, None);
        return 2;
    };
    if cmd == "help" || rest.iter().any(|a| a == "-h" || a == "--help") {
        let topic = if cmd == "help" { rest.first().map(String::as_str) } else { Some(cmd.as_str()) };
        print_help(prog, topic);
        return 0;
    }
    if json {
        quiet_stderr();
    }
    // 桌面壳已经按设置选好了 TeX；单独跑的 subo-server 按 --bundled-tex
    if !crate::tex::initialized() {
        crate::tex::init_external(bundled_tex);
    }
    if crate::tex::bundled() {
        // 按需安装的引擎（LuaTeX、pTeX）的引擎根，同服务端启动时
        let stale = engines::prepare();
        engines::refresh_formats(&stale);
    }
    let ctx = Ctx {
        prog: prog.to_string(),
        json,
        settings: settings::Settings::load(&settings_file()),
        mirror: Mutex::new(None),
        case: Mutex::new(case::Index::open(case::default_index_path())),
    };
    let res = match cmd.as_str() {
        "build" => build(&ctx, rest),
        "install" => install_cmd(&ctx, rest),
        "doctor" => doctor(&ctx, rest),
        "inspect" => inspect(&ctx, rest),
        "mirror" => mirror_cmd(&ctx, rest),
        "templates" => list_templates(&ctx, rest),
        "new" => new_project(&ctx, rest),
        other => Err(Failure::usage("cli.unknownCommand", format!("没有 {other} 这个命令"))
            .hint(format!("可用的命令：{}", COMMANDS.join("、")))),
    };
    match res {
        Ok((out, code)) => {
            if json {
                println!("{}", out.to_json());
            } else {
                print_text(cmd, rest.first().map(String::as_str), &out);
            }
            code
        }
        Err(f) => {
            if json {
                let mut o = Out::default();
                o.set("ok", false).set("error", f.code).set("message", f.message.as_str());
                if let Some(h) = &f.hint {
                    o.set("hint", h.as_str());
                }
                o.list("next", &f.next);
                println!("{}", o.to_json());
            } else {
                println!("错误：{}", f.message);
                if let Some(h) = &f.hint {
                    println!("怎么办：{h}");
                }
                if !f.next.is_empty() {
                    println!("下一步：");
                    for n in &f.next {
                        println!("  {n}");
                    }
                }
            }
            f.exit
        }
    }
}

fn settings_file() -> PathBuf {
    library::subo_dir().join("settings.json")
}

/// `--json` 时 stdout 只有那个 JSON，stderr 也不能出声：下面各模块的诊断输出（字体映射、
/// 引擎登记……）改写进 `<subo 目录>/cli/stderr.log`，排查时还找得到
fn quiet_stderr() {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let dir = library::subo_dir().join("cli");
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("stderr.log");
        // 只留最近的，免得越攒越大
        let fresh = fs::metadata(&file).is_ok_and(|m| m.len() > 1 << 20);
        let opened = fs::OpenOptions::new().create(true).append(!fresh).write(true).truncate(fresh).open(&file);
        if let Ok(f) = opened {
            // SAFETY: 只是把 2 号描述符指向刚打开的文件；`f` 随后关闭不影响 dup 出来的那个
            unsafe {
                libc::dup2(f.as_raw_fd(), 2);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// build

fn build(ctx: &Ctx, raw: &[String]) -> Res {
    let a = Args::parse("build", raw, &["entry", "engine", "out", "timeout", "mirror", "tex"], &["install", "no-pdf", "fresh"], 1)?;
    ctx.use_mirror(a.value("mirror"))?;
    ctx.use_tex(a.value("tex"))?;
    let (root, file) = project_dir(ctx, a.pos.first())?;
    let explicit = a.value("entry").map(str::to_string).or(file);
    let entry = entry_of(&root, explicit.as_deref())?;
    let want = a.value("engine").unwrap_or("auto");
    let base = if want == "mock" { "mock".to_string() } else { engine::detect("auto") };
    if base == "mock" && want != "mock" {
        return Err(no_tex());
    }
    let mut choice = choose_program(ctx, &root, &entry, &base, want)?;
    // 项目要的引擎没装（日文文档的 lualatex、uplatex）：--install 时先装上，再按装好的重新选
    let mut engine_added: Option<&str> = None;
    let mut install_fail: Option<Failure> = None;
    if let Some(id) = choice.missing.filter(|_| a.has("install")) {
        match add_engine(ctx, id) {
            Ok(id) => {
                engine_added = Some(id);
                choice = choose_program(ctx, &root, &entry, &base, want)?;
            }
            Err(f) => install_fail = Some(f),
        }
    }
    let secs = match a.value("timeout") {
        Some(v) => v.parse().ok().filter(|&n: &u64| n > 0).ok_or_else(|| {
            Failure::usage("cli.badTimeout", format!("--timeout 要一个正整数（秒），收到的是 {v}"))
        })?,
        None => ctx.settings.timeout_secs.unwrap_or(120),
    };

    // 每个项目一个工作目录：中间产物留着下次接着编；同一项目同时只编一个，日志和产物不会互相覆盖
    let work = library::subo_dir().join("cli").join(hash_name(&root));
    fs::create_dir_all(&work).map_err(|e| Failure::env("build.workDir", format!("建不了工作目录 {}：{e}", work.display())))?;
    let _lock = lock_work(ctx, &work)?;
    let outdir = work.join(&choice.program);
    if a.has("fresh") {
        let _ = fs::remove_dir_all(&outdir);
    }

    let mut installed: Vec<String> = Vec::new();
    let mut rounds = 0;
    let once = loop {
        let once = compile_once(ctx, &root, &entry, &choice.program, &work, &outdir, Duration::from_secs(secs));
        if once.ok || !a.has("install") || install_fail.is_some() || rounds == INSTALL_ROUNDS {
            break once;
        }
        let todo: Vec<String> = once.packages.iter().filter(|p| !installed.contains(p)).cloned().collect();
        if todo.is_empty() {
            break once;
        }
        rounds += 1;
        ctx.note(format!("== 安装宏包 {}", todo.join(" ")));
        match install_packages(ctx, &todo) {
            Ok(also) => {
                installed.extend(todo);
                installed.extend(also);
            }
            Err(f) => {
                install_fail = Some(f);
                break once;
            }
        }
    };

    // PDF 放在项目里、与入口同名，同界面的「导出 PDF」
    let mut pdf_out: Option<String> = None;
    if let Some(pdf) = once.pdf.as_ref().filter(|_| once.ok && !a.has("no-pdf")) {
        let dest = a.value("out").map(PathBuf::from).unwrap_or_else(|| root.join(export_name(&entry)));
        if let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = fs::create_dir_all(dir);
        }
        fs::copy(pdf, &dest).map_err(|e| Failure::env("export.writeFailed", format!("PDF 写不进 {}：{e}", dest.display())))?;
        pdf_out = Some(dunce::canonicalize(&dest).unwrap_or(dest).to_string_lossy().into_owned());
    }
    // 没编过的中间产物可能是半截的（.aux 写到一半），不留给下次
    if !once.ok {
        let _ = fs::remove_dir_all(&outdir);
    }

    let target = if explicit.is_some() { root.join(&entry) } else { root.clone() };
    let dir = shell_quote(&target.to_string_lossy());
    let p = &ctx.prog;
    let mut warnings = choice.warnings.clone();
    let mut next = Vec::new();
    let mut error = None;
    let mut hint = None;
    let mut exit = if once.ok { 0 } else { 1 };
    if !once.ok {
        let why = explain(ctx, &dir, &once, secs);
        error = Some(why.summary);
        hint = why.hint;
        next = why.next;
        if let Some(f) = install_fail {
            // 自动安装没成：说清是装哪一步坏的；这是环境问题，改源码没用，退出码按环境算
            error = Some(format!("{}；自动安装没成：{}", error.unwrap_or_default(), f.message));
            hint = f.hint.or(hint);
            next = f.next;
            exit = f.exit;
        } else if (!once.packages.is_empty() || choice.missing.is_some()) && !a.has("install") {
            next.insert(0, format!("{p} build {dir} --install"));
        }
    }
    // 内核比宏包旧：改源码、装包都修不好，是环境问题
    if let Some(st) = &once.stale {
        let why = stale_fix(ctx, &dir, st);
        error = Some(why.summary);
        hint = why.hint;
        next = why.next;
        exit = 3;
    }
    warnings.extend(once.compat.iter().cloned());
    warnings.extend(once.notes.bib.iter().cloned());

    let mut o = Out::default();
    o.set("ok", once.ok);
    if let Some(e) = &error {
        o.set("error", e.as_str());
        if let Some(h) = &hint {
            o.set("hint", h.as_str());
        }
        o.set("errorFile", once.error_file.clone().map_or(Value::Null, Value::from));
        o.set("errorLine", once.error_line.map_or(Value::Null, Value::from));
        o.set("errorKind", once.error_kind.map_or(Value::Null, |k| serde_json::to_value(k).unwrap_or(Value::Null)));
        o.set("diagnosis", once.diagnosis.clone().map_or(Value::Null, Value::from));
    }
    o.set("entry", entry.as_str()).set("engine", choice.program.as_str()).set("ms", once.ms);
    o.set("pages", once.pages.map_or(Value::Null, Value::from));
    o.set("pdf", pdf_out.map_or(Value::Null, Value::from)).set("log", once.log.as_str());
    o.list("errors", &once.errors).list("packages", &once.packages).list("missing", &once.missing);
    o.list("installed", &installed).set("installedEngine", engine_added.map_or(Value::Null, Value::from));
    o.list("undefinedRefs", &once.notes.refs).list("undefinedCites", &once.notes.cites);
    o.list("missingChars", &once.notes.chars).list("overfull", &once.notes.overfull);
    o.list("warnings", &warnings).list("next", &next);
    Ok((o, exit))
}

/// 同一个项目同时只让一个 `build` 跑；另一个在跑就等它
fn lock_work(ctx: &Ctx, work: &Path) -> Result<fs::File, Failure> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(work.join(".lock"))
        .map_err(|e| Failure::env("build.workDir", format!("工作目录 {} 写不了：{e}", work.display())))?;
    if file.try_lock().is_err() {
        ctx.note("== 这个项目正在另一处编译，等它结束");
        file.lock().map_err(|e| Failure::env("build.workDir", format!("等不到另一处编译结束：{e}")))?;
    }
    Ok(file)
}

struct Once {
    ok: bool,
    /// 没编过是因为 LaTeX 内核比宏包旧
    stale: Option<crate::tex::Stale>,
    ms: u64,
    /// timeout / memory / fileSize / failed
    reason: &'static str,
    /// 日志里的错误行，带上 TeX 指出的那行源码
    errors: Vec<String>,
    /// 第一处错误在项目里的哪个文件、哪一行（出在 TeX Live 自己的文件里时为空）
    error_file: Option<String>,
    error_line: Option<u32>,
    /// 第一处错误的类别（logparse::Kind）
    error_kind: Option<logparse::Kind>,
    /// 诊断材料的路径（`<stem>.diagnosis.md`），仅失败且日志里认得出错误时有
    diagnosis: Option<String>,
    /// 查到包名、可以直接装的宏包
    packages: Vec<String>,
    /// 缺的文件、字体、程序，一项一句话：缺什么、为什么装不了、怎么办
    missing: Vec<String>,
    /// 查到了包名的文件（给 error 一句话说明用）
    found_files: Vec<String>,
    compat: Vec<String>,
    /// 编过了但内容可能不对的地方
    notes: Notes,
    pages: Option<u64>,
    log: String,
    pdf: Option<PathBuf>,
}

/// 编一次并诊断，和服务端的 `run_job` 同一套判断（`diagnose`）
fn compile_once(ctx: &Ctx, root: &Path, entry: &str, program: &str, work: &Path, outdir: &Path, timeout: Duration) -> Once {
    let mb = ctx.settings.memory_mb.unwrap_or(settings::MEMORY_DEFAULT);
    let limits = engine::Limits { memory: mb << 20, file_size: engine::FILE_SIZE_CAP };
    ctx.note(format!("== 编译 {entry}（{program}）"));
    let progress = |step: engine::Step| match step {
        engine::Step::Pass(n) => ctx.note(format!("   第 {n} 遍")),
        engine::Step::Tool(t) => ctx.note(format!("   {t}")),
        _ => {}
    };
    let started = Instant::now();
    let res = engine::compile(program, root, entry, outdir, timeout, &limits, &progress);
    let ms = started.elapsed().as_millis() as u64;

    // 完整日志留在素帛自己的目录里，不往项目里写
    let stem = Path::new(entry).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let log = work.join(format!("{stem}.log"));
    let _ = fs::write(&log, &res.log);

    let d = diagnose(root, entry, outdir, &res, program);
    let mut items = if d.is_empty() { Vec::new() } else { d.resolve(&ctx.case, ctx.repo().as_deref(), ctx.fallback()) };
    items.extend(d.fonts.iter().cloned());

    let (mut packages, mut missing, mut found_files) = (Vec::new(), Vec::new(), Vec::new());
    for m in &items {
        let f = &m.file;
        let line = match (m.kind, &m.lookup) {
            (case::Kind::File, case::Lookup::Found) => {
                let pkg = m.package.clone().unwrap_or_default();
                found_files.push(format!("{pkg}（{f}）"));
                packages.push(pkg.clone());
                format!("{f}：在宏包 {pkg} 里，可以装")
            }
            (case::Kind::File, case::Lookup::Absent) => format!("{f}：TeX Live 里没有这个文件，检查拼写，或者它该是项目自己带的"),
            (case::Kind::File, case::Lookup::Project) => {
                format!("{f}：项目里有，但在子目录 {}，TeX 不会去找；挪到项目根目录", m.project_path.as_deref().unwrap_or_default())
            }
            (case::Kind::File, _) => format!("{f}：查不到属于哪个宏包（多半是离线或没有 tlmgr），联网后重试，或用 {} mirror probe 换个下载源", ctx.prog),
            (case::Kind::Font, _) => match &m.font_file {
                Some(file) => format!("字体 {f}：TeX Live 里有 {file}，用 {} install {file} 装上", ctx.prog),
                None => format!("字体 {f}：TeX Live 和系统里都没有，装进系统，或在源码里换成已有的字体"),
            },
            (case::Kind::Tool, _) => {
                let by = if m.needed_by.is_empty() { String::new() } else { format!("（{} 要用）", m.needed_by.join("、")) };
                format!("程序 {f}{by}：转换 EPS/PS 插图要它，素帛不自带；macOS 用 brew install ghostscript 装")
            }
        };
        if !missing.contains(&line) {
            missing.push(line);
        }
    }
    packages.sort();
    packages.dedup();
    let compat = d.fixes.iter().map(|f| format!("{}:{}：已知的旧样式与新版 TeX 不兼容（{}），在素帛里打开项目可一键修复", f.file, f.line, f.id)).collect();

    let reason = match (res.ok, res.timed_out, res.over) {
        (true, ..) => "",
        (_, true, _) => "timeout",
        (_, _, Some(engine::Over::Memory)) => "memory",
        (_, _, Some(engine::Over::FileSize)) => "fileSize",
        _ => "failed",
    };
    let report = logparse::parse(&res.log);
    let first = report.errors().next().filter(|_| !res.ok);
    // 给 agent 的诊断材料：错误类别、它意味着什么、出错处附近的源码，落在日志旁边
    let diagnosis = first.map(|e| {
        let path = work.join(format!("{stem}.diagnosis.md"));
        let _ = fs::write(&path, diagnosis_text(root, program, e));
        path.to_string_lossy().into_owned()
    });
    Once {
        ok: res.ok,
        stale: (!res.ok).then(|| crate::tex::stale(&res.log)).flatten(),
        ms,
        reason,
        errors: if res.ok { Vec::new() } else { tex_errors(&report) },
        error_file: first.and_then(|e| e.file.clone()),
        error_line: first.and_then(|e| e.line),
        error_kind: first.map(|e| e.kind),
        diagnosis,
        packages,
        missing,
        found_files,
        compat,
        notes: if res.ok { notes(&report) } else { Notes::default() },
        pages: if res.ok { pages(&res.log) } else { None },
        log: log.to_string_lossy().into_owned(),
        pdf: res.pdf,
    }
}

/// 失败的说明：一句话结论、怎么修、照着跑的命令
struct Why {
    summary: String,
    hint: Option<String>,
    next: Vec<String>,
}

/// 按「最能说明原因」的顺序挑：超限 > 缺东西 > 源码错误
/// 内核太旧时怎么修：有内置 TeX 就换过去；MiKTeX 更新一次；TeX Live 只能装新一年的
fn stale_fix(ctx: &Ctx, dir: &str, st: &crate::tex::Stale) -> Why {
    use crate::tex::Kind;
    let p = &ctx.prog;
    let status = crate::tex::status(ctx.settings.tex);
    let kernel = st.kernel.as_deref().map(|k| format!("（{k}）")).unwrap_or_default();
    let want = st.want.as_deref().map(|w| format!("，这个项目要 {w} 以后的")).unwrap_or_default();
    let summary = format!("LaTeX 内核{kernel}比项目用到的宏包旧{want}");
    let kind = status.own.as_ref().map(|o| o.kind);
    let (hint, next) = if status.bundled {
        ("用的已经是素帛内置的 TeX：多半是项目自带的宏包文件（texmf 目录、.sty/.cls）比内置的新，换成和内置 TeX 同年的版本".to_string(), vec![])
    } else if status.has_bundled {
        let year = status.bundled_year.map(|y| format!(" {y}")).unwrap_or_default();
        (
            format!("改用素帛内置的 TeX Live{year} 编：加 --tex bundled；想一直用它就在素帛「设置 → 排版与安装」里选内置"),
            vec![format!("{p} build {dir} --tex bundled")],
        )
    } else if kind == Some(Kind::Miktex) {
        ("MiKTeX 按需装的宏包是最新的，内核却没跟着更新：更新一次 MiKTeX（或在 MiKTeX Console 里点「检查更新」）".to_string(), vec!["miktex packages update".to_string()])
    } else {
        (format!("本机 TeX 太旧：装 {} 年以后的 TeX Live，或用素帛桌面版自带的 TeX", &crate::tex::FLOOR[..4]), vec![])
    };
    Why { summary, hint: Some(hint), next }
}

fn explain(ctx: &Ctx, dir: &str, o: &Once, secs: u64) -> Why {
    let p = &ctx.prog;
    let rerun = format!("{p} build {dir}");
    let why = |summary: String, hint: String, next: Vec<String>| Why { summary, hint: Some(hint), next };
    match o.reason {
        "timeout" => {
            return why(
                format!("编译超过 {secs} 秒被终止"),
                format!("多半是宏死循环，先看日志 {}；文档确实大、要更久就放宽超时", o.log),
                vec![format!("{rerun} --timeout {}", (secs * 5).max(600))],
            )
        }
        "memory" => return why("编译占用内存超过上限被终止".into(), format!("多半是宏死循环；看日志 {}", o.log), Vec::new()),
        "fileSize" => return why("编译输出的文件过大被终止".into(), format!("多半是宏死循环往文件里写；看日志 {}", o.log), Vec::new()),
        _ => {}
    }
    if !o.packages.is_empty() {
        return why(
            format!("缺宏包：{}", o.found_files.join("、")),
            "装上缺的宏包再重编".into(),
            vec![format!("{p} install {}", o.packages.join(" ")), rerun],
        );
    }
    if let Some(first) = o.missing.first() {
        return why(format!("缺 {first}"), "改源码或补上缺的文件，再重编".into(), vec![rerun]);
    }
    let at = match (&o.error_file, o.error_line) {
        (Some(f), Some(l)) => format!("（{f} 第 {l} 行）"),
        (Some(f), None) => format!("（{f}）"),
        _ => String::new(),
    };
    let more = o.diagnosis.as_deref().map(|d| format!("；出错处附近的源码和分析见 {d}")).unwrap_or_default();
    match o.errors.first() {
        Some(e) => {
            let hint = match o.error_kind.and_then(kind_hint) {
                Some(k) => format!("{k}。改好源码{at}再重编{more}"),
                None => format!("改好源码{at}再重编{more}"),
            };
            why(format!("源码有错：{e}"), hint, vec![rerun])
        }
        None => why("编译失败，日志里没有认得的错误行".into(), format!("看完整日志 {}", o.log), Vec::new()),
    }
}

/// 日志里的错误，带上 TeX 停下处的源码（⟨here⟩ 是它停下的位置），告诉人是哪个命令、哪段源码出的错。
/// 最多 `ERRORS_MAX` 条（开着 -halt-on-error，通常只有一条）
fn tex_errors(r: &logparse::Report) -> Vec<String> {
    r.errors()
        .take(ERRORS_MAX)
        .map(|e| {
            let at = match (&e.file, e.line) {
                (Some(f), Some(l)) => format!("{f}:{l}: "),
                (Some(f), None) => format!("{f}: "),
                _ => String::new(),
            };
            // 缺文件时 TeX 停在下一行，指出的位置反而误导；文件名本身就是信息
            let here = e.here.as_deref().filter(|_| !e.message.contains("not found")).map(here_text);
            match here {
                Some(h) if !h.is_empty() => format!("{at}{}（出错处 {h}）", e.message),
                _ => format!("{at}{}", e.message),
            }
        })
        .collect()
}

/// 断点在末尾时标记没有信息量，去掉
fn here_text(h: &str) -> String {
    let h = h.trim();
    h.strip_suffix(logparse::HERE).unwrap_or(h).trim().to_string()
}

/// 按错误类别补一句「该去哪里找」：报错的位置常常不是原因所在
fn kind_hint(kind: logparse::Kind) -> Option<&'static str> {
    use logparse::Kind;
    Some(match kind {
        Kind::Output => "报错行只是分页的位置，错在页眉页脚、页码或浮动体的设置里",
        Kind::End => "停在 \\end{document} 只是因为最后一页在这里输出，原因在前面（多半是页码、页面样式、页眉页脚的设置）",
        Kind::Env => "有环境没闭合或首尾名字不一致，消息里写着它从哪一行开始",
        Kind::Scan => "有参数没闭合（多半少了右花括号），错在参数开始的地方，不在 TeX 停下的地方",
        Kind::Package => "宏包或文档类报的错，多半出在它的加载方式或用法（选项、加载顺序、参数）",
        Kind::Aux => "错在上一遍写下的辅助文件里，改源码里写进去的那段文字（标签、标题、图表标题或文献键），再删掉辅助文件重编",
        Kind::Driver => "TeX 排完了，但转 PDF 时失败：多半缺字体文件或插图文件",
        _ => return None,
    })
}

/// 写给 agent 的诊断材料：和素帛里 AI 修复拿到的同一套（diagnose::render）
fn diagnosis_text(root: &Path, program: &str, e: &logparse::Issue) -> String {
    let d = diagnose::Diagnosis::from(e);
    let mut s = format!("Engine: {program}\n");
    let source = e.file.as_deref().and_then(|f| fs::read_to_string(root.join(f)).ok().map(|t| (f, t)));
    match source {
        Some((file, text)) => {
            let ex = diagnose::excerpts(&text, file, &d);
            let changes = crate::lastgood::source(root, file).map(|b| diagnose::diff(&b, &text));
            s.push_str(&diagnose::render(&text, file, &d, changes.as_ref(), &ex));
        }
        None => s.push_str(&diagnose::render("", "", &d, None, &[])),
    }
    s
}

/// 编过了、但内容可能不对的地方：PDF 里会是 `??`、`[?]`、豆腐块或出了版心
#[derive(Default)]
struct Notes {
    /// 没定义的 `\ref`/`\label` 名
    refs: Vec<String>,
    /// 没定义的 `\cite` 键
    cites: Vec<String>,
    /// 字体里没有的字：`字 (U+5B57)，字体 cmr10`
    chars: Vec<String>,
    /// 明显的溢出（超出版心 30pt 以上的行、放不下的页面内容和浮动体）
    overfull: Vec<String>,
    /// biber / bibtex 自己报的错
    bib: Vec<String>,
}

fn push_note(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) && list.len() < NOTES_MAX {
        list.push(item);
    }
}

fn notes(r: &logparse::Report) -> Notes {
    use logparse::Kind;
    let mut n = Notes::default();
    for i in &r.issues {
        match (i.kind, &i.key) {
            (Kind::UndefinedRef, Some(k)) => push_note(&mut n.refs, k.clone()),
            (Kind::UndefinedCite, Some(k)) => push_note(&mut n.cites, k.clone()),
            (Kind::MissingChar, Some(k)) => push_note(&mut n.chars, i.source.as_ref().map_or_else(|| k.clone(), |f| format!("{k}，字体 {f}"))),
            (Kind::Bib, _) => push_note(&mut n.bib, format!("{}：{}", i.source.as_deref().unwrap_or("bib"), i.message)),
            _ => {}
        }
    }
    for d in diagnose::layout_of(r) {
        let item = match (d.kind, d.line) {
            (diagnose::LayoutKind::LineOverflow, Some(line)) => format!("第 {line} 行附近有一行超出版心 {:.0}pt", d.pt),
            (diagnose::LayoutKind::LineOverflow, None) => format!("有一行超出版心 {:.0}pt", d.pt),
            (diagnose::LayoutKind::PageOverflow, _) => format!("有一页的内容比版心高 {:.0}pt，多半是图表或长公式放不下", d.pt),
            (diagnose::LayoutKind::FloatTooLarge, _) => format!("有浮动体（图、表）比页面大 {:.0}pt", d.pt),
        };
        push_note(&mut n.overfull, item);
    }
    n
}

/// 日志末尾 `Output written on main.pdf (3 pages, 12345 bytes).` 里的页数。
/// 路径长时这一行会被折开，接起来再找
fn pages(log: &str) -> Option<u64> {
    let flat: String = logparse::last_pass(log).lines().collect();
    let at = flat.rfind("Output written on ")?;
    let rest = &flat[at..];
    let mut i = 0;
    while let Some(p) = rest[i..].find('(') {
        let tail = &rest[i + p + 1..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && tail[digits.len()..].starts_with(" page") {
            return digits.parse().ok();
        }
        i += p + 1;
    }
    None
}

struct ProgramChoice {
    program: String,
    /// 项目要的引擎没装、素帛能装上（内置 TeX 的按需引擎）时，它的 id
    missing: Option<&'static str>,
    /// 不影响编译的提醒：项目要的引擎没装、先用别的试编
    warnings: Vec<String>,
    /// 为什么用这个程序，一句话
    why: String,
}

/// 用哪个 LaTeX 程序，同服务端 `program_for`：`--engine` > 项目在素帛里选过的 > 源码里的线索 > pdflatex
fn choose_program(ctx: &Ctx, root: &Path, entry: &str, base: &str, want: &str) -> Result<ProgramChoice, Failure> {
    if base != "xelatex" {
        // tectonic、mock 没得选
        return Ok(ProgramChoice { program: base.to_string(), missing: None, warnings: Vec::new(), why: format!("这台机器只有 {base}") });
    }
    let chosen = match want {
        "auto" => saved_engine(ctx, root).as_deref().and_then(Program::parse),
        w => Some(Program::parse(w).filter(|p| p.name() == w).ok_or_else(|| {
            Failure::usage("engine.unknown", format!("--engine 不认识 {w}")).hint(format!("可选：{ENGINES}、mock"))
        })?),
    };
    let c = engine::program::resolve(root, entry, chosen, Program::Pdflatex, |p| engines::available(p.name()));
    let reason = |r: &Reason| match r {
        Reason::MagicComment => format!("{entry} 里写了 % !TEX program"),
        Reason::Latexmkrc => "项目里的 latexmkrc 指定了".to_string(),
        Reason::ArxivReadme => "arXiv 的 00README 指定了".to_string(),
        Reason::Preamble(w) => format!("导言区用了 {w}"),
    };
    let mut warnings = Vec::new();
    let mut missing = None;
    let why = match &c.source {
        Source::User if want != "auto" => "--engine 指定".to_string(),
        Source::User => "在素帛里给这个项目选过".to_string(),
        Source::Detected(r) => reason(r),
        Source::Default => "源码里看不出来，按默认".to_string(),
        Source::Missing { wanted, reason: r } => {
            let because = r.as_ref().map(|r| format!("（{}）", reason(r))).unwrap_or_default();
            let fix = match engines::for_program(wanted.name()) {
                Some(s) if ctx.bundled() => {
                    missing = Some(s.id);
                    format!("{} install --engine {}，或编译时加 --install", ctx.prog, s.id)
                }
                _ => format!("用 tlmgr 装上 {} 所在的引擎", wanted.name()),
            };
            warnings.push(format!("这个项目要用 {}{because}，本机没装，先用 {} 试编；装上它：{fix}", wanted.name(), c.program.name()));
            format!("要的 {} 没装", wanted.name())
        }
    };
    Ok(ProgramChoice { program: c.program.name().to_string(), missing, warnings, why })
}

/// 项目在素帛里选过的引擎：项目库里的按目录名，登记过的本地文件夹按清单里的 id
fn saved_engine(ctx: &Ctx, root: &Path) -> Option<String> {
    let subo = library::subo_dir();
    let home = dunce::canonicalize(&ctx.home()).ok();
    let id = match home.as_deref().and_then(|h| root.strip_prefix(h).ok()) {
        Some(rest) => rest.components().next()?.as_os_str().to_string_lossy().to_string(),
        None => folders::Folders::open(subo.join("folders.json"))
            .all()
            .iter()
            .find(|f| dunce::canonicalize(&f.path).ok().as_deref() == Some(root))?
            .id
            .clone(),
    };
    projprefs::load(&subo.join("projects"), &id).engine
}

// ---------------------------------------------------------------------------
// install

fn install_cmd(ctx: &Ctx, raw: &[String]) -> Res {
    let a = Args::parse("install", raw, &["engine", "mirror"], &[], usize::MAX)?;
    ctx.use_mirror(a.value("mirror"))?;
    if let Some(id) = a.value("engine") {
        return install_engine(ctx, id);
    }
    if a.pos.is_empty() {
        return Err(Failure::usage("cli.missingArg", "没说要装什么")
            .hint(format!("给宏包名或报错里缺的文件名，如 {p} install bytefield 或 {p} install bytefield.sty", p = ctx.prog)));
    }
    // 带扩展名的是文件名（从报错里直接抄来的），先查属于哪个包
    let (files, mut pkgs): (Vec<String>, Vec<String>) = a.pos.iter().cloned().partition(|n| n.contains('.'));
    if !files.is_empty() {
        if engine::which("tlmgr").is_none() {
            return Err(no_tlmgr());
        }
        let found = case::resolve(&mut ctx.case.lock().unwrap(), &files, ctx.repo().as_deref(), ctx.fallback());
        for m in found {
            match (m.lookup, m.package) {
                (case::Lookup::Found, Some(p)) => pkgs.push(p),
                (case::Lookup::Unknown, _) => {
                    return Err(Failure::env("case.lookupFailed", format!("查不到 {} 属于哪个宏包：连不上下载源", m.file))
                        .hint("联网后重试；连得上但慢或连不上，就测速换一个下载源")
                        .next(format!("{} mirror probe", ctx.prog)))
                }
                _ => {
                    return Err(Failure::usage("case.notInTexLive", format!("TeX Live 里没有 {} 这个文件", m.file))
                        .hint("检查拼写；如果它是项目自己的文件，放进项目根目录"))
                }
            }
        }
    }
    pkgs.sort();
    pkgs.dedup();
    let also = install_packages(ctx, &pkgs)?;
    let mut o = Out::default();
    o.set("ok", true).list("installed", &pkgs).list("also", &also);
    Ok((o, 0))
}

/// 装宏包，`build --install` 也用。返回顺带补装的包。要管理员授权的不装（不弹框），给出命令让人跑
fn install_packages(ctx: &Ctx, pkgs: &[String]) -> Result<Vec<String>, Failure> {
    let list = pkgs.join(" ");
    if engine::which("tlmgr").is_none() {
        return Err(no_tlmgr());
    }
    if case::needs_admin(ctx.bundled()) {
        return Err(Failure::env("case.needsAdmin", format!("装 {list} 要管理员权限：这台机器的 TeX Live 装在系统目录"))
            .hint(format!("请用户在终端里跑 sudo tlmgr install {list}（要输密码，agent 跑不了）")));
    }
    let files = ctx.case.lock().unwrap().files_of(pkgs);
    let done = case::install(pkgs, &files, ctx.bundled(), ctx.repo().as_deref(), ctx.fallback())
        .map_err(|e| Failure::env("case.installFailed", format!("装 {list} 失败：{e}")))?;
    if done.offline {
        return Err(Failure::env("case.offline", format!("装 {list} 失败：连不上下载源 {}", source_label(ctx)))
            .hint("联网后重试；连得上但慢或连不上，就测速换一个下载源")
            .next(format!("{} mirror probe", ctx.prog)));
    }
    if !done.ok {
        return Err(Failure::env("case.installFailed", format!("装 {list} 失败，tlmgr 最后几行：{}", last_lines(&done.log, 4))));
    }
    if ctx.bundled() {
        let _ = case::register_user_maps();
    }
    // 新装的包可能带字体：按名字找字体的映射要跟着更新，同服务端装完宏包时
    fontmap::refresh();
    Ok(done.also)
}

fn install_engine(ctx: &Ctx, want: &str) -> Res {
    let id = add_engine(ctx, want)?;
    let mut o = Out::default();
    o.set("ok", true).set("engine", id);
    Ok((o, 0))
}

/// 装按需引擎，`build --install` 也用。返回装好的引擎 id
fn add_engine(ctx: &Ctx, want: &str) -> Result<&'static str, Failure> {
    // 也认程序名：`--engine lualatex` 就是装 LuaTeX 所在的引擎
    let id = engines::for_program(want).map_or(want, |s| s.id);
    let installable: Vec<&str> = engines::CATALOG.iter().filter(|s| !s.builtin).map(|s| s.id).collect();
    if !ctx.bundled() {
        return Err(Failure::env("engines.unmanaged", "用的是本机自己的 TeX，素帛不管理它的引擎")
            .hint(format!("用 tlmgr 安装 {id}（如 tlmgr install {id}）")));
    }
    ctx.note(format!("== 安装引擎 {id}"));
    let repo = ctx.repo();
    match engines::install(id, engines::Source::Repo { repo: repo.as_deref(), fallback: ctx.fallback() }) {
        Ok(done) => Ok(done.id),
        Err(f) => Err(match f.code {
            "engines.unknown" => Failure::usage(f.code, format!("没有 {want} 这个可安装的引擎"))
                .hint(format!("可装的：{}（也可以写程序名，如 lualatex、platex）", installable.join("、"))),
            "engines.downloadFailed" | "engines.archiveFailed" => {
                Failure::env(f.code, format!("装引擎 {id} 失败：从 {} 下载不下来（{}）", source_label(ctx), last_lines(&f.detail, 2)))
                    .hint("检查网络；连得上但慢或连不上，就测速换一个下载源")
                    .next(format!("{} mirror probe", ctx.prog))
            }
            "engines.repoYear" | "engines.yearMismatch" => {
                Failure::env(f.code, format!("装引擎 {id} 失败：下载源的 TeX Live 年份和内置的不一致（{}）", f.detail))
                    .hint("换回预置镜像（会自动改用对应年份的归档源），自定义源要指向同一年份的 tlnet")
                    .next(format!("{} mirror probe", ctx.prog))
            }
            _ => Failure::env(f.code, format!("装引擎 {id} 失败：{}", last_lines(&f.detail, 4))),
        }),
    }
}

// ---------------------------------------------------------------------------
// mirror

/// 现在用的下载源，一句话：`tuna（https://…）`
fn source_label(ctx: &Ctx) -> String {
    let s = ctx.source();
    match s.repository() {
        Some(url) if s.mirror == settings::MIRROR_CUSTOM => url,
        Some(url) => format!("{}（{url}）", s.mirror),
        None => "TeX Live 自己设置的源".to_string(),
    }
}

fn bad_mirror(p: &str, code: &'static str, spec: &str) -> Failure {
    let ids: Vec<&str> = settings::PRESETS.iter().map(|x| x.0).collect();
    match code {
        "settings.localMirrorInvalid" => Failure::usage(code, format!("{spec} 不是可用的本机下载源：里面没有 tlpkg/texlive.tlpdb"))
            .hint("给 tlnet 目录本身（含 tlpkg/ 的那一层）的绝对路径"),
        "settings.badCustomMirror" => Failure::usage(code, format!("下载源地址 {spec} 不对"))
            .hint("要 http(s):// 开头的网址，或本机 tlnet 目录的绝对路径；不能有空格和引号"),
        _ => Failure::usage("settings.badMirror", format!("不认识下载源 {spec}"))
            .hint(format!("预置的有 {}；tlmgr 表示跟随 TeX Live 自己的设置；也可以直接给地址", ids.join("、")))
            .next(format!("{p} mirror")),
    }
}

fn mirror_cmd(ctx: &Ctx, raw: &[String]) -> Res {
    let a = Args::parse("mirror", raw, &["custom"], &["fastest"], 2)?;
    let p = &ctx.prog;
    match a.pos.first().map(String::as_str) {
        None | Some("list") => {
            if a.has("fastest") || a.value("custom").is_some() {
                return Err(Failure::usage("cli.unknownFlag", "mirror list 没有选项").hint(format!("测速用 {p} mirror probe，选源用 {p} mirror use")));
            }
            mirror_list(ctx)
        }
        Some("probe") => {
            if a.has("fastest") {
                return Err(Failure::usage("cli.unknownFlag", "mirror probe 不认识 --fastest").hint(format!("测完直接换用 {p} mirror use --fastest")));
            }
            let (results, current) = probe_all(ctx, a.value("custom"))?;
            let best = &results[0];
            let mut o = Out::default();
            o.set("ok", true).set("current", current.as_str());
            o.set("fastest", best.id.as_str()).set("fastestUrl", best.url.as_str());
            o.set("fastestKBps", best.speed.kbps.map_or(Value::Null, Value::from));
            o.set("fastestMs", best.speed.ms.map_or(Value::Null, Value::from));
            // 跟随 tlmgr 时它的地址常常就是某个预置源，按地址认出来
            let url = ctx.settings.repository().or_else(case::tlmgr_repository).unwrap_or_default();
            let is_current = |r: &Probed| r.id == current || r.url.trim_end_matches('/') == url.trim_end_matches('/');
            o.list("results", &results.iter().map(|r| r.line(is_current(r))).collect::<Vec<_>>());
            let next = if is_current(best) { Vec::new() } else { vec![format!("{p} mirror use {}", shell_quote(&best.spec()))] };
            o.list("next", &next);
            Ok((o, 0))
        }
        Some("use") => {
            let (mirror, custom, speed) = match (a.pos.get(1), a.has("fastest")) {
                (Some(_), true) => return Err(Failure::usage("cli.tooManyArgs", "mirror use 给了源又给了 --fastest，只能二选一")),
                (None, false) => {
                    return Err(Failure::usage("cli.missingArg", "没说换成哪个下载源")
                        .hint(format!("给预置 id、地址，或 --fastest 测速后用最快的；可选的看 {p} mirror")))
                }
                (Some(spec), false) => {
                    let (m, c) = settings::parse_mirror(spec).map_err(|code| bad_mirror(p, code, spec))?;
                    (m, c, None)
                }
                (None, true) => {
                    let (results, _) = probe_all(ctx, a.value("custom"))?;
                    let best = &results[0];
                    let (m, c) = settings::parse_mirror(&best.spec()).map_err(|code| bad_mirror(p, code, &best.spec()))?;
                    (m, c, Some(best.speed))
                }
            };
            if a.value("custom").is_some() && speed.is_none() {
                return Err(Failure::usage("cli.unknownFlag", "--custom 只和 --fastest 一起用").hint(format!("要用某个地址就直接 {p} mirror use <地址>")));
            }
            // 只改下载源，别的设置原样留着；文件是和界面共用的，开着的素帛下次取设置时会重读
            let file = settings_file();
            let mut s = settings::Settings::load(&file);
            s.mirror = mirror;
            s.custom_mirror = custom;
            s.save(&file).map_err(|e| Failure::env("settings.saveFailed", format!("设置写不进 {}：{e}", file.display())))?;
            let url = s.repository().or_else(case::tlmgr_repository).unwrap_or_default();
            let mut o = Out::default();
            o.set("ok", true).set("mirror", s.mirror.as_str()).set("url", url.as_str());
            if let Some(sp) = speed {
                o.set("kbps", sp.kbps.map_or(Value::Null, Value::from)).set("ms", sp.ms.map_or(Value::Null, Value::from));
            }
            Ok((o, 0))
        }
        Some(other) => Err(Failure::usage("cli.unknownCommand", format!("mirror 没有 {other} 这个子命令"))
            .hint("可用的：list（默认）、probe、use")),
    }
}

fn mirror_list(ctx: &Ctx) -> Res {
    let s = &ctx.settings;
    let mut lines: Vec<String> = settings::PRESETS.iter().map(|(id, url, _)| format!("{id}：{url}")).collect();
    let own = case::tlmgr_repository();
    lines.push(format!("{}：{}", settings::MIRROR_TLMGR, own.as_deref().unwrap_or("跟随 TeX Live 自己的设置")));
    if !s.custom_mirror.trim().is_empty() {
        lines.push(format!("{}：{}", settings::MIRROR_CUSTOM, s.custom_mirror.trim()));
    }
    let url = s.repository().or(own).unwrap_or_default();
    let mut o = Out::default();
    o.set("ok", true).set("mirror", s.mirror.as_str()).set("url", url.as_str()).list("mirrors", &lines);
    Ok((o, 0))
}

struct Probed {
    id: String,
    url: String,
    speed: settings::Speed,
}

impl Probed {
    /// 给 `mirror use` 的写法：预置源用 id，其余用地址
    fn spec(&self) -> String {
        if self.id == settings::MIRROR_CUSTOM { self.url.clone() } else { self.id.clone() }
    }

    fn line(&self, current: bool) -> String {
        let mark = if current { "（当前）" } else { "" };
        match (self.speed.ms, self.speed.kbps) {
            (None, _) => format!("{}{mark}：不通，{}", self.id, self.url),
            (Some(_), None) if !self.url.starts_with("http") => format!("{}{mark}：本机目录，可用，{}", self.id, self.url),
            (Some(ms), None) => format!("{}{mark}：首字节 {ms} ms，速度没测出来，{}", self.id, self.url),
            (Some(ms), Some(k)) => format!("{}{mark}：{k} KB/s，首字节 {ms} ms，{}", self.id, self.url),
        }
    }
}

/// 并发测所有源（预置的、设置里的自定义源、`--custom`、tlmgr 自己的），快的在前。
/// 一个都不通时报环境错误。返回结果和当前用的源的 id
fn probe_all(ctx: &Ctx, extra: Option<&str>) -> Result<(Vec<Probed>, String), Failure> {
    let s = &ctx.settings;
    let mut targets: Vec<(String, String)> = settings::PRESETS.iter().map(|(id, url, _)| (id.to_string(), url.to_string())).collect();
    let custom = extra.map(str::to_string).or_else(|| Some(s.custom_mirror.trim().to_string()).filter(|c| !c.is_empty()));
    if let Some(c) = custom {
        let (m, url) = settings::parse_mirror(&c).map_err(|code| bad_mirror(&ctx.prog, code, &c))?;
        if m == settings::MIRROR_CUSTOM {
            targets.push((m, url));
        }
    }
    if let Some(own) = case::tlmgr_repository().filter(|u| !targets.iter().any(|t| t.1 == u.trim_end_matches('/'))) {
        targets.push((settings::MIRROR_TLMGR.into(), own));
    }
    ctx.note(format!("== 测速 {} 个下载源（每个最多 {PROBE_SECS} 秒）", targets.len()));
    let handles: Vec<_> = targets
        .into_iter()
        .map(|(id, url)| std::thread::spawn(move || {
            let speed = settings::probe_speed(&url, Duration::from_secs(PROBE_SECS));
            Probed { id, url, speed }
        }))
        .collect();
    let mut results: Vec<Probed> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    // 本机目录最快；其余按下载速度，测不出速度的按首字节；不通的沉底
    let rank = |r: &Probed| match (r.speed.ms, r.speed.kbps) {
        (None, _) => (3, 0),
        (Some(_), None) if !r.url.starts_with("http") => (0, 0),
        (Some(_), Some(k)) => (1, u64::MAX - k),
        (Some(ms), None) => (2, ms),
    };
    results.sort_by_key(rank);
    if results.first().is_none_or(|r| r.speed.ms.is_none()) {
        return Err(Failure::env("mirror.allDown", format!("测了 {} 个下载源，一个都连不上", results.len()))
            .hint(format!("检查网络；内网环境用 {} mirror use <内网镜像地址或本机 tlnet 目录>", ctx.prog)));
    }
    let current = if s.mirror == settings::MIRROR_CUSTOM { settings::MIRROR_CUSTOM.to_string() } else { s.mirror.clone() };
    Ok((results, current))
}

// ---------------------------------------------------------------------------
// doctor / inspect / templates / new

fn doctor(ctx: &Ctx, raw: &[String]) -> Res {
    let a = Args::parse("doctor", raw, &["mirror"], &["net"], 0)?;
    ctx.use_mirror(a.value("mirror"))?;
    let base = engine::detect("auto");
    let tex_ok = base == "xelatex" || base == "tectonic";
    let lacking: Vec<String> = ["pdflatex", "xelatex", "lualatex", "biber", "bibtex", "makeindex", "tlmgr"]
        .iter()
        .filter(|p| engine::which(p).is_none())
        .map(|p| p.to_string())
        .collect();
    let needs_admin = case::needs_admin(ctx.bundled());
    let tex = match (case::texlive_root(), case::texlive_version()) {
        (Some(r), v) => format!("TeX Live {}（{}，{}）", v.unwrap_or_else(|| "?".into()), if ctx.bundled() { "素帛内置" } else { "本机" }, r.display()),
        (None, _) if base == "tectonic" => "tectonic".to_string(),
        _ => "没有".to_string(),
    };
    let mut hint = None;
    let mut next = Vec::new();
    if !tex_ok {
        hint = no_tex().hint;
    } else if lacking.iter().any(|p| p == "tlmgr") {
        hint = Some("没有 tlmgr：缺宏包时装不了，只能手动处理".into());
    } else if needs_admin {
        hint = Some("装宏包要管理员权限：缺包时请用户跑 sudo tlmgr install <包>".into());
    }
    // 内置 TeX 缺的引擎可以按需装
    if ctx.bundled() {
        for p in &lacking {
            if let Some(s) = engines::for_program(p).filter(|s| !s.builtin) {
                let cmd = format!("{} install --engine {}", ctx.prog, s.id);
                if !next.contains(&cmd) {
                    next.push(cmd);
                }
            }
        }
    }
    // 下载源连不连得通要联网才知道，默认不测（离线软件，不默认联网）
    let src = ctx.source();
    let url = src.repository().or_else(case::tlmgr_repository).unwrap_or_default();
    let reach = a.has("net").then(|| settings::probe(&url, Duration::from_secs(PROBE_SECS)));
    if reach == Some(None) {
        hint.get_or_insert_with(|| format!("下载源 {url} 连不上：缺宏包时装不了"));
        next.push(format!("{} mirror probe", ctx.prog));
    }
    let mut o = Out::default();
    o.set("ok", tex_ok);
    if let Some(h) = &hint {
        o.set("hint", h.as_str());
    }
    o.set("tex", tex).set("engine", base.as_str());
    o.list("lacking", &lacking).set("installNeedsAdmin", needs_admin).set("ghostscript", case::ghostscript_found());
    o.set("mirror", src.mirror.as_str()).set("mirrorUrl", url.as_str());
    o.set("mirrorReachable", reach.map_or(Value::Null, |r| Value::from(r.is_some())));
    o.set("mirrorMs", reach.flatten().map_or(Value::Null, Value::from));
    o.set("library", ctx.home().to_string_lossy().as_ref()).set("suboDir", library::subo_dir().to_string_lossy().as_ref());
    o.list("next", &next);
    Ok((o, if tex_ok { 0 } else { 3 }))
}

fn inspect(ctx: &Ctx, raw: &[String]) -> Res {
    let a = Args::parse("inspect", raw, &["entry"], &[], 1)?;
    let (root, file) = project_dir(ctx, a.pos.first())?;
    let entry = entry_of(&root, a.value("entry").map(str::to_string).or(file).as_deref())?;
    let choice = choose_program(ctx, &root, &entry, &engine::detect("auto"), "auto")?;
    let tex: Vec<String> = project::tree(&root).into_iter().filter(|e| !e.dir && e.path.ends_with(".tex")).map(|e| e.path).collect();
    let mut o = Out::default();
    o.set("ok", true).set("project", root.to_string_lossy().as_ref()).set("entry", entry.as_str());
    o.set("engine", choice.program.as_str()).set("why", choice.why.as_str());
    o.list("texFiles", &tex).list("warnings", &choice.warnings);
    Ok((o, 0))
}

fn list_templates(ctx: &Ctx, raw: &[String]) -> Res {
    Args::parse("templates", raw, &[], &[], 0)?;
    let all = templates::list(&templates::dir(&ctx.home()));
    let lines: Vec<String> = all
        .iter()
        .map(|t| {
            let v = serde_json::to_value(t).unwrap_or_default();
            let name = v["name"].as_str().unwrap_or_default();
            let kind = if t.builtin { "预置" } else { "我的模板" };
            let id = t.id.strip_prefix("builtin:").unwrap_or(&t.id);
            format!("{id}：{name}（{kind}，入口 {}）", v["entry"].as_str().unwrap_or_default())
        })
        .collect();
    let mut o = Out::default();
    o.set("ok", true).list("templates", &lines);
    Ok((o, 0))
}

fn new_project(ctx: &Ctx, raw: &[String]) -> Res {
    let a = Args::parse("new", raw, &["template"], &[], 1)?;
    let p = &ctx.prog;
    let Some(dir) = a.pos.first() else {
        return Err(Failure::usage("cli.missingArg", "没说新项目放在哪个目录").hint(format!("{p} new ./paper 或 {p} new ./paper --template article-zh")));
    };
    let dest = PathBuf::from(dir);
    if dest.exists() && fs::read_dir(&dest).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(Failure::usage("new.notEmpty", format!("{} 已经存在且不是空目录，不会往里写", abs(&dest)))
            .hint("换一个新目录名"));
    }
    let tpl = a.value("template");
    match tpl {
        Some(id) => {
            let dir = templates::dir(&ctx.home());
            // 预置模板的 id 是 `builtin:article-zh`，列表里只显示后半段，两种写法都认
            let full = if templates::list(&dir).iter().any(|t| t.id == id) { id.to_string() } else { format!("builtin:{id}") };
            templates::materialize(&dir, &full, &dest).map_err(|e| match e.code {
                "template.notFound" => Failure::usage(e.code, format!("没有 {id} 这个模板")).next(format!("{p} templates")),
                _ => Failure::env(e.code, e.message),
            })?
        }
        None => {
            let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "untitled".into());
            fs::create_dir_all(&dest)
                .and_then(|_| fs::write(dest.join("main.tex"), library::template(&name)))
                .map_err(|e| Failure::env("library.createDir", format!("建不了 {}：{e}", abs(&dest))))?;
        }
    }
    let root = dunce::canonicalize(&dest).unwrap_or(dest);
    let Some(entry) = project::entry(&root) else {
        return Err(Failure::env("build.noTex", "模板里没有 .tex 文件"));
    };
    // 素帛的空白项目和模板多按 xelatex 写（日文模板自带 lualatex 的魔法注释）；界面里靠项目设置记住，
    // 这里写进魔法注释，别的编辑器也认
    let main = root.join(&entry);
    if let Ok(text) = fs::read_to_string(&main) {
        if !text.to_ascii_lowercase().contains("!tex program") {
            let _ = fs::write(&main, format!("% !TEX program = xelatex\n{text}"));
        }
    }
    let mut o = Out::default();
    o.set("ok", true).set("project", root.to_string_lossy().as_ref()).set("entry", entry.as_str());
    o.list("next", &[format!("{p} build {} --install", shell_quote(&root.to_string_lossy()))]);
    Ok((o, 0))
}

// ---------------------------------------------------------------------------
// 公共

/// 项目目录（默认当前目录）；直接给的是 `.tex` 文件时，它所在的目录是项目、它本身是入口
fn project_dir(ctx: &Ctx, arg: Option<&String>) -> Result<(PathBuf, Option<String>), Failure> {
    let raw = arg.map(String::as_str).unwrap_or(".");
    let p = Path::new(raw);
    let (dir, file) = match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) if p.is_file() => {
            (if parent.as_os_str().is_empty() { Path::new(".") } else { parent }, Some(name.to_string_lossy().into_owned()))
        }
        _ => (p, None),
    };
    match dunce::canonicalize(&dir) {
        Ok(d) if d.is_dir() => Ok((d, file)),
        _ if raw.ends_with(".tex") => Err(Failure::usage("project.notFound", format!("找不到文件 {}", abs(p)))),
        _ => Err(Failure::usage("project.notFound", format!("找不到项目目录 {}", abs(dir)))
            .hint(format!("检查路径；要新建就用 {} new {raw}", ctx.prog))),
    }
}

/// 入口：`--entry` 给的，或自动识别。给的是入口 `.tex` 文件路径时也认
fn entry_of(root: &Path, flag: Option<&str>) -> Result<String, Failure> {
    if let Some(e) = flag {
        return match project::resolve(root, e) {
            Ok(p) if p.is_file() => Ok(e.trim_start_matches(['/', '\\']).trim_start_matches("./").to_string()),
            _ => Err(Failure::usage("build.noEntry", format!("{} 里没有 {e}", root.display()))),
        };
    }
    project::entry(root).ok_or_else(|| {
        Failure::usage("build.noTex", format!("{} 里没有 .tex 文件", root.display())).hint("确认目录对不对；入口不在根目录就用 --entry 指定")
    })
}

fn no_tex() -> Failure {
    let hint = if cfg!(target_os = "macos") {
        "装 TeX Live（MacTeX），或改用素帛自带的命令 /Applications/Subo.app/Contents/MacOS/subo，它用内置的 TeX"
    } else {
        "装 TeX Live，或改用素帛安装包里的 subo 命令，它用内置的 TeX"
    };
    Failure::env("tex.notFound", "没有找到 TeX：PATH 里没有 xelatex，也没有 tectonic").hint(hint)
}

fn no_tlmgr() -> Failure {
    Failure::env("case.noTlmgr", "没有 tlmgr，装不了宏包").hint("用完整的 TeX Live（自带 tlmgr），或改用素帛自带的 subo 命令")
}

fn abs(p: &Path) -> String {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).display().to_string()
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" / ")
}

/// 给 `next` 里的命令加引号：路径里有空格、中文等也能直接复制运行
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

// ---------------------------------------------------------------------------
// 文本输出：`键：值` 一行一项，列表缩进两格；空的不打

fn print_text(cmd: &str, sub: Option<&str>, o: &Out) {
    let line = |label: &str, v: &str| {
        if !v.is_empty() {
            println!("{label}：{v}");
        }
    };
    let list = |label: &str, k: &str| {
        let items = o.strs(k);
        if !items.is_empty() {
            println!("{label}：");
            for i in items {
                println!("  {i}");
            }
        }
    };
    match cmd {
        "build" => {
            if o.is("ok") {
                let pages = o.0.get("pages").and_then(Value::as_u64).map(|n| format!("，{n} 页")).unwrap_or_default();
                println!(
                    "成功：{} → {}（{}{pages}，{} ms）",
                    o.str("entry"),
                    o.0.get("pdf").and_then(Value::as_str).unwrap_or("未写 PDF"),
                    o.str("engine"),
                    o.0["ms"]
                );
            } else {
                println!("失败：{}", o.str("error"));
                line("怎么办", o.str("hint"));
                line("入口", o.str("entry"));
                line("引擎", o.str("engine"));
                list("错误", "errors");
                list("缺", "missing");
                line("日志", o.str("log"));
            }
            line("已安装引擎", o.str("installedEngine"));
            list("已安装", "installed");
            list("未定义的引用", "undefinedRefs");
            list("未定义的文献", "undefinedCites");
            list("缺字", "missingChars");
            list("排版溢出", "overfull");
            list("提醒", "warnings");
            list("下一步", "next");
        }
        "install" => match o.0.get("engine").and_then(Value::as_str) {
            Some(e) => println!("成功：已安装引擎 {e}"),
            None => {
                println!("成功：已安装 {}", o.strs("installed").join(" "));
                let also = o.strs("also");
                if !also.is_empty() {
                    println!("顺带装了：{}", also.join(" "));
                }
            }
        },
        "doctor" => {
            println!("{}：{}", if o.is("ok") { "可用" } else { "不可用" }, o.str("tex"));
            line("怎么办", o.str("hint"));
            line("缺的程序", &o.strs("lacking").join(" "));
            println!("装宏包要管理员权限：{}", if o.is("installNeedsAdmin") { "是" } else { "否" });
            println!("Ghostscript：{}", if o.is("ghostscript") { "有" } else { "没有（EPS 插图转不了）" });
            let reach = match o.0.get("mirrorReachable").and_then(Value::as_bool) {
                Some(true) => format!("，连得上（{} ms）", o.0["mirrorMs"]),
                Some(false) => "，连不上".to_string(),
                None => String::new(),
            };
            println!("下载源：{}（{}）{reach}", o.str("mirror"), o.str("mirrorUrl"));
            line("项目库", o.str("library"));
            line("素帛目录", o.str("suboDir"));
            list("下一步", "next");
        }
        "inspect" => {
            line("项目", o.str("project"));
            line("入口", o.str("entry"));
            println!("引擎：{}（{}）", o.str("engine"), o.str("why"));
            list(".tex 文件", "texFiles");
            list("提醒", "warnings");
        }
        "mirror" => match sub {
            Some("probe") => {
                let speed = o.0.get("fastestKBps").and_then(Value::as_u64).map(|k| format!("，{k} KB/s")).unwrap_or_default();
                println!("最快：{}（{}{speed}）", o.str("fastest"), o.str("fastestUrl"));
                println!("当前：{}", o.str("current"));
                list("测速结果", "results");
                list("下一步", "next");
            }
            Some("use") => {
                let speed = o.0.get("kbps").and_then(Value::as_u64).map(|k| format!("，{k} KB/s")).unwrap_or_default();
                println!("成功：下载源改为 {}（{}{speed}）", o.str("mirror"), o.str("url"));
            }
            _ => {
                println!("当前：{}（{}）", o.str("mirror"), o.str("url"));
                list("可选", "mirrors");
            }
        },
        "templates" => {
            for t in o.strs("templates") {
                println!("{t}");
            }
        }
        "new" => {
            println!("成功：已新建 {}（入口 {}）", o.str("project"), o.str("entry"));
            list("下一步", "next");
        }
        _ => println!("{}", o.to_json()),
    }
}

fn print_help(prog: &str, topic: Option<&str>) {
    let text = match topic {
        Some("build") => format!("\
用法：{prog} build [目录或入口.tex] [选项]

编译 LaTeX 项目。成功时 PDF 写在项目里、与入口同名（main.tex → main.pdf）。
  --install         缺的宏包能查到就自动装上再重编（最多 3 轮）
  --entry <文件>    指定入口（默认自动识别，main.tex 优先）
  --engine <名>     {ENGINES}，默认 auto
  --out <文件>      PDF 写到别处
  --no-pdf          只看能不能编过，不写 PDF
  --timeout <秒>    单次编译超时，默认 120
  --mirror <源>     这次装宏包用哪个下载源（预置 id 或地址），不改设置
  --fresh           丢掉上次留下的中间产物，从头编

中间产物按项目留在素帛目录里，下次接着编，改动小时只跑一遍。
失败时第一行是结论（缺什么宏包 / 哪行源码错了），后面是原始错误、缺的文件和下一步命令。
编过了也会列出未定义的引用和文献、字体里没有的字、明显超出版心的地方。
退出码：0 编过，1 没编过，2 用法不对，3 环境问题（含自动安装因离线、要管理员权限没成）。

例：{prog} build ./paper --install"),
        Some("install") => format!("\
用法：{prog} install <宏包名或文件名>... [--mirror <源>]
      {prog} install --engine <引擎> [--mirror <源>]

装 TeX Live 宏包。可以直接给报错里缺的文件名，会先查它在哪个包里。
TeX Live 装在系统目录、要管理员权限时不会弹框，直接失败并说明要用户跑的 sudo 命令。
--engine 装 LuaTeX、pTeX 这类按需引擎（luatex、ptex……），只用于素帛内置的 TeX。
--mirror 这次用哪个下载源，不改设置。

例：{prog} install bytefield.sty"),
        Some("doctor") => format!("\
用法：{prog} doctor [--net] [--mirror <源>]

检查环境：用哪个 TeX、缺哪些程序、装宏包要不要管理员权限、有没有 Ghostscript、用哪个下载源。
加 --net 再测一下下载源连不连得上（要联网）。没有可用的 TeX 时退出码 3。"),
        Some("inspect") => format!("用法：{prog} inspect [目录] [--entry <文件>]\n\n不编译，只看：入口是哪个、会用哪个引擎及原因、有哪些 .tex 文件。"),
        Some("mirror") => format!("\
用法：{prog} mirror                     看当前下载源和所有可选的
      {prog} mirror probe [--custom <地址>]   测速（要联网），快的在前
      {prog} mirror use <id|地址|tlmgr>       换下载源，存进设置（和界面共用）
      {prog} mirror use --fastest [--custom <地址>]   测速后换成最快的

下载源用于补装宏包和引擎。预置的是 CTAN 官方调度和国内几家镜像；地址可以是 http(s) 网址，
也可以是本机 tlnet 目录（离线安装源）；tlmgr 表示跟随 TeX Live 自己的设置。
测速下载每个源 texlive.tlpdb 开头的 512 KB，按下载速度排，不只看延迟。

例：{prog} mirror use --fastest"),
        Some("templates") => format!("用法：{prog} templates\n\n列出模板，每行「id：名称（来源，入口）」。id 给 {prog} new --template 用。"),
        Some("new") => format!("\
用法：{prog} new <目录> [--template <id>]

新建项目。目录要不存在或是空的；不给模板就建一个空白的中文文档。
入口开头没写 % !TEX program 时会写上 xelatex（素帛的模板多按 xelatex 写，日文模板自带 lualatex）。

例：{prog} new ./paper --template article-zh"),
        _ => format!("\
用法：{prog} <命令> [参数] [--json]

  build [目录]           编译；加 --install 缺宏包自动装
  install <宏包|文件>…   装宏包
  doctor                 检查 TeX 环境
  inspect [目录]         看入口和会用的引擎
  mirror [probe|use]     看、测速、换下载源
  templates              列出模板
  new <目录>             新建项目
  help [命令]            看某个命令的说明

默认输出给人读的文本；加 --json 输出一个平铺的 JSON 对象（只有字符串、数字、布尔、字符串数组）。
失败时有 error（一句话结论）、hint（怎么修）和 next（可以直接跑的命令）。
退出码：0 成功，1 文档没编过，2 用法不对，3 环境问题（没有 TeX、装不了包、离线）。"),
    };
    println!("{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_source_line_and_drop_noise() {
        let log = "./main.tex:3: Undefined control sequence.\nl.3 Hi \\foo\n\n\
                   ! LaTeX Error: File `x.sty' not found.\n\nType X to quit.\n\
                   ./main.tex:4: Emergency stop.\nl.4 \\begin\n\
                   ./main.tex:4:  ==> Fatal error occurred, no output PDF file produced!\n";
        // 开着 -halt-on-error 时只会有一个错误；这里两个都认出来，`! …` 的那个不知道在哪个文件
        assert_eq!(
            tex_errors(&logparse::parse(log)),
            ["main.tex:3: Undefined control sequence.（出错处 Hi \\foo）", "File `x.sty' not found."]
        );
    }

    #[test]
    fn noise_lends_position_to_previous_error() {
        let log = "! Missing $ inserted.\n<inserted text>\n./main.tex:5: Emergency stop.\nl.5 a_b\n";
        assert_eq!(tex_errors(&logparse::parse(log)), ["Missing $ inserted.（出错处 a_b）"]);
    }

    #[test]
    fn notes_find_undefined_refs_cites_and_missing_chars() {
        let long = format!("LaTeX Warning: Reference `{}' on page 1 undefined on input line 9.", "x".repeat(60));
        let (a, b) = long.split_at(logparse::WRAP);
        let log = format!(
            "== xelatex 第 1 遍\nLaTeX Warning: Reference `old' on page 1 undefined on input line 1.\n\
             == xelatex 第 2 遍\nLaTeX Warning: Reference `fig:a' on page 1 undefined on input line 3.\n\
             {a}\n{b}\n\
             Package natbib Warning: Citation `knuth84' on page 1 undefined on input line 4.\n\
             LaTeX Warning: Citation 'lamport' on page 2 undefined on input line 7.\n\
             Missing character: There is no 字 (U+5B57) in font [lmroman10-regular]:mapping=tex-text;!\n\
             Missing character: There is no 字 (U+5B57) in font [lmroman10-regular]:mapping=tex-text;!\n"
        );
        let n = notes(&logparse::parse(&log));
        assert_eq!(n.refs, ["fig:a".to_string(), "x".repeat(60)]);
        assert_eq!(n.cites, ["knuth84", "lamport"]);
        assert_eq!(n.chars, ["字 (U+5B57)，字体 lmroman10-regular"]);
    }

    #[test]
    fn notes_pick_up_bib_tool_errors() {
        let log = "== pdflatex 第 1 遍\n\n== bibtex\nI couldn't open database file refs.bib\n== pdflatex 第 2 遍\n";
        assert_eq!(notes(&logparse::parse(log)).bib, ["bibtex：I couldn't open database file refs.bib"]);
    }

    #[test]
    fn pages_from_wrapped_output_line() {
        let log = "== xelatex 第 1 遍\nOutput written on /a/very/long/path/that/keeps/going/and/going/and/going/main.x\ndv (12 pages, 3456 bytes).\n";
        assert_eq!(pages(log), Some(12));
        assert_eq!(pages("Output written on main.pdf (1 page, 10 bytes).\n"), Some(1));
        assert_eq!(pages("No pages of output.\n"), None);
    }

    #[test]
    fn quote_only_when_needed() {
        assert_eq!(shell_quote("/tmp/paper"), "/tmp/paper");
        assert_eq!(shell_quote("/tmp/我的 论文"), "'/tmp/我的 论文'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn args_reject_unknown_and_list_known() {
        let raw = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let a = Args::parse("build", &raw(&["d", "--engine=xelatex", "--install"]), &["engine"], &["install"], 1).ok().unwrap();
        assert_eq!((a.pos.as_slice(), a.value("engine"), a.has("install")), (&["d".to_string()][..], Some("xelatex"), true));
        let e = Args::parse("build", &raw(&["--frob"]), &["engine"], &["install"], 1).err().unwrap();
        assert_eq!((e.code, e.exit), ("cli.unknownFlag", 2));
        assert!(e.hint.unwrap().contains("--engine <值>  --install"));
        let e = Args::parse("build", &raw(&["--engine"]), &["engine"], &[], 1).err().unwrap();
        assert_eq!(e.code, "cli.missingValue");
    }
}
