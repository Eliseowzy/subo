//! 排版引擎（press）：把 `compile` 抽象成一个可替换的子进程调用。
//!
//! 三类实现：
//! - `tectonic`：按需取包的引擎，在线时缺什么取什么
//! - TeX Live：`xelatex`（Unicode 引擎，中文与 OpenType 字体）和 `pdflatex`（arXiv、Overleaf 的默认）内置，
//!   `lualatex`、`platex` / `uplatex` 按需安装（engines.rs）；项目用哪个由 [`program`] 按源码探测、用户可改
//! - `mock`    ：没有引擎时的兜底，产出占位 PDF，保证整条链路可跑

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

/// 单路输出（stdout / stderr）最多收这么多，再多的读出来丢掉。
/// 写死循环 `\message` 的文档能在几秒内刷出上 GB 日志。
const LOG_CAP: u64 = 1 << 20;

pub struct CompileResult {
    pub ok: bool,
    pub timed_out: bool,
    /// 超了资源限额被终止（超时另记在 `timed_out`）
    pub over: Option<Over>,
    pub pdf: Option<PathBuf>,
    pub log: String,
}

pub(crate) struct Ran {
    pub ok: bool,
    pub timed_out: bool,
    pub over: Option<Over>,
    /// 调用方要求停下（用户取消安装），已连进程组一起杀掉
    pub stopped: bool,
    pub log: String,
}

/// 编译的资源限额。用户的 `.tex` 是任意代码：死循环有超时管，这里管吃内存和写爆磁盘。
/// 只加在编译（引擎和辅助工具）上，tlmgr 这类我们自己调的程序不限。
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// 引擎进程的常驻内存上限（字节）。按 50 ms 一次轮询，超了连进程组一起杀。
    /// 不用 RLIMIT_AS：macOS 不执行它，Linux 上 LuaTeX 预留的虚拟地址又远大于实际用量
    pub memory: u64,
    /// 单个输出文件的大小上限（字节），RLIMIT_FSIZE，超了内核发 SIGXFSZ
    pub file_size: u64,
}

/// 单个输出文件上限：正常的 PDF、日志远到不了，到了基本是死循环往外写
pub const FILE_SIZE_CAP: u64 = 2 << 30;

/// 超了哪一项
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Over {
    Memory,
    FileSize,
}

/// 起子进程一律用它，不直接 `Command::new`：Windows 上 Subo 是图形程序，没有自己的控制台，
/// 不加 `CREATE_NO_WINDOW` 的话每跑一次 kpsewhich、xelatex 都会弹出一个黑色的命令行窗口。
/// 子进程再拉起的进程（xdvipdfmx、MiKTeX 的装包程序）继承这个隐藏的控制台，也不会弹
///
/// 带上当前这套 TeX 的环境（见 [`crate::tex`]）；只给程序名时按它的 PATH 找
pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let program = program.as_ref();
    let bare = Path::new(program).components().count() == 1 && !Path::new(program).is_absolute();
    let found = bare.then(|| program.to_str().and_then(which)).flatten();
    let mut c = bare_command(found.as_deref().map_or(program, |p| p.as_os_str()));
    crate::tex::apply(&mut c);
    c
}

/// 同 [`command`]，但不带当前 TeX 的环境：探测用户自己的 TeX、更新 MiKTeX 时用
pub(crate) fn bare_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    no_window(&mut c);
    c
}

fn no_window(c: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::os::windows::process::CommandExt::creation_flags(c, CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = c;
}

/// 跑一个子进程，到 `deadline` 还没结束就连同它的子进程一起杀掉。
/// press 编译与 case 查询（tlmgr）共用。
pub(crate) fn run(cmd: &mut Command, deadline: Instant) -> io::Result<Ran> {
    run_watched(cmd, deadline, None, None, None, None)
}

/// 同 [`run`]，另外加上资源限额（编译的辅助工具用）；`on_out` 见 [`run_watched`]
fn run_limited(cmd: &mut Command, deadline: Instant, limits: Option<&Limits>, on_out: Option<&dyn Fn(&str)>) -> io::Result<Ran> {
    run_watched(cmd, deadline, limits, None, on_out, None)
}

/// 同 [`run`]，另外把 stdout / stderr 的每一整行实时交给 `on_line`（tlmgr 的安装进度靠它）；
/// `stop` 返回 true 时连进程组一起杀掉，结果的 `stopped` 为 true（取消安装靠它）
pub(crate) fn run_lines(cmd: &mut Command, deadline: Instant, on_line: &dyn Fn(&str), stop: &dyn Fn() -> bool) -> io::Result<Ran> {
    run_watched(cmd, deadline, None, None, Some(&|chunk: &str| chunk.lines().for_each(on_line)), Some(stop))
}

/// 同 [`run`]，另外从 TeX 的终端输出里数已输出的页（`[1] [2{...}]`），数变了就回调；
/// 给了 `on_out` 就把输出实时转给它：每次轮询收到的整行用 `\n` 连成一块交一次，不逐行回调，
/// 免得 TeX 一口气刷几千行时事件也跟着刷几千条
fn run_watched(
    cmd: &mut Command,
    deadline: Instant,
    limits: Option<&Limits>,
    on_pages: Option<&dyn Fn(u32)>,
    on_out: Option<&dyn Fn(&str)>,
    stop: Option<&dyn Fn() -> bool>,
) -> io::Result<Ran> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // 万一调用方漏用了 `command()`，经这里跑的也不弹窗
    no_window(cmd);
    // 独立进程组：xelatex 会再拉起 xdvipdfmx，超时要一锅端
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    #[cfg(unix)]
    if let Some(l) = limits {
        let cap = l.file_size as libc::rlim_t;
        // SAFETY: fork 之后、exec 之前只调 setrlimit（async-signal-safe），不分配内存
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(cmd, move || {
                let r = libc::rlimit { rlim_cur: cap, rlim_max: cap };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &r) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn()?;
    #[cfg(windows)]
    let job = crate::winjob::Job::attach(&child);
    #[cfg(not(windows))]
    let job = ();

    let pages = Arc::new(AtomicU32::new(0));
    let (tx, rx) = mpsc::channel::<String>();
    let drain = |r: Option<Box<dyn Read + Send>>, count: Option<Arc<AtomicU32>>, lines: Option<mpsc::Sender<String>>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let mut scan = PageScan::default();
            let mut line = Vec::new();
            let mut chunk = [0u8; 8192];
            if let Some(mut r) = r {
                // 逐块读：页数要随输出实时更新，不能等进程结束
                while let Ok(n) = r.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Some(c) = &count {
                        c.store(scan.feed(&chunk[..n]), Ordering::Relaxed);
                    }
                    if let Some(tx) = &lines {
                        for &b in &chunk[..n] {
                            if b == b'\n' || b == b'\r' {
                                if !line.is_empty() {
                                    let _ = tx.send(String::from_utf8_lossy(&line).into_owned());
                                    line.clear();
                                }
                            } else if line.len() < 4096 {
                                line.push(b);
                            }
                        }
                    }
                    let room = (LOG_CAP as usize).saturating_sub(buf.len());
                    buf.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
            // 最后一行没有换行符也要交出去
            if let (Some(tx), false) = (&lines, line.is_empty()) {
                let _ = tx.send(String::from_utf8_lossy(&line).into_owned());
            }
            buf
        })
    };
    let want = on_out.is_some();
    let out = drain(
        child.stdout.take().map(|r| Box::new(r) as _),
        on_pages.is_some().then(|| pages.clone()),
        want.then(|| tx.clone()),
    );
    let err = drain(child.stderr.take().map(|r| Box::new(r) as _), None, want.then(|| tx.clone()));
    drop(tx);

    let mut told = 0;
    let mut over = None;
    let mut stopped = false;
    let mut tick = 0u32;
    let (ok, timed_out) = loop {
        if let Some(f) = on_out {
            pass_out(&rx, f);
        }
        if let Some(f) = on_pages {
            let n = pages.load(Ordering::Relaxed);
            if n != told {
                told = n;
                f(n);
            }
        }
        if let Some(st) = child.try_wait()? {
            if limits.is_some() && killed_by_fsize(&st) {
                over = Some(Over::FileSize);
            }
            break (st.success(), false);
        }
        if Instant::now() >= deadline {
            kill_tree(&mut child, &job);
            break (false, true);
        }
        if stop.is_some_and(|f| f()) {
            kill_tree(&mut child, &job);
            stopped = true;
            break (false, false);
        }
        // 内存四次轮询看一次（200 ms）：一次 proc_pidinfo / 读 statm 很便宜，但没必要更勤
        tick = tick.wrapping_add(1);
        if let Some(l) = limits.filter(|_| tick % 4 == 0) {
            if tree_memory(&child, &job).is_some_and(|b| b > l.memory) {
                kill_tree(&mut child, &job);
                over = Some(Over::Memory);
                break (false, false);
            }
            if tree_written(&job).is_some_and(|b| b > l.file_size) {
                kill_tree(&mut child, &job);
                over = Some(Over::FileSize);
                break (false, false);
            }
        }
        thread::sleep(Duration::from_millis(50));
    };

    let mut log = String::from_utf8_lossy(&out.join().unwrap_or_default()).to_string();
    log.push_str(&String::from_utf8_lossy(&err.join().unwrap_or_default()));
    if let Some(f) = on_out {
        pass_out(&rx, f);
    }
    Ok(Ran { ok, timed_out, over, stopped, log })
}

/// 把攒下的整行连成一块交给 `f`，没有就不调
fn pass_out(rx: &mpsc::Receiver<String>, f: &dyn Fn(&str)) {
    let lines: Vec<String> = rx.try_iter().collect();
    if !lines.is_empty() {
        f(&lines.join("\n"));
    }
}

/// 进程是不是因为写文件超了 RLIMIT_FSIZE 被内核杀掉的
fn killed_by_fsize(st: &std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::process::ExitStatusExt::signal(st) == Some(libc::SIGXFSZ)
    }
    #[cfg(not(unix))]
    {
        let _ = st;
        false
    }
}

#[cfg(windows)]
type Job = Option<crate::winjob::Job>;
#[cfg(not(windows))]
type Job = ();

/// 编译占的内存：Unix 上是引擎进程的常驻内存，Windows 上是整个作业的提交内存峰值
fn tree_memory(child: &std::process::Child, job: &Job) -> Option<u64> {
    #[cfg(windows)]
    {
        let _ = child;
        job.as_ref()?.peak_memory()
    }
    #[cfg(not(windows))]
    {
        let _ = job;
        resident(child.id())
    }
}

/// 累计写出的字节数。只有 Windows 要这样量，Unix 上单个文件的上限由内核（RLIMIT_FSIZE）管
fn tree_written(job: &Job) -> Option<u64> {
    #[cfg(windows)]
    {
        job.as_ref()?.bytes_written()
    }
    #[cfg(not(windows))]
    {
        let _ = job;
        None
    }
}

/// 进程当前的常驻内存（字节）；查不到（进程刚退出、平台不支持）为 `None`
#[cfg(not(windows))]
fn resident(pid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        // SAFETY: info 是足够大的已初始化缓冲区，大小如实传入
        let n = unsafe {
            libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTASKINFO, 0, &mut info as *mut _ as *mut libc::c_void, size)
        };
        (n == size).then_some(info.pti_resident_size)
    }
    #[cfg(target_os = "linux")]
    {
        // statm 第二列是常驻页数
        let statm = fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: sysconf 没有前置条件
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        (page > 0).then(|| pages * page as u64)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

/// 超限时写进日志的一句话
fn over_note(over: Over, limits: &Limits) -> String {
    match over {
        Over::Memory => format!("\n内存用量超过上限 {} MB，已终止。可在设置里调整上限。", limits.memory >> 20),
        Over::FileSize => format!("\n输出文件超过 {} MB，已终止（多半是死循环在往外写）。", limits.file_size >> 20),
    }
}

/// 数 TeX 终端输出里的出页标记：`[` 紧跟数字，且 `[` 前是行首、空白或上一页的收尾。
/// 行过宽 / 过松的报告会把正文原样印出来（`[]\T1/ptm/m/n/10 see [12]`），
/// 这种行里出现过 `[]` 就整行不算，免得把引用编号当成页。
#[derive(Default)]
struct PageScan {
    count: u32,
    prev: u8,
    open: bool,
    boxed: bool,
}

impl PageScan {
    fn feed(&mut self, bytes: &[u8]) -> u32 {
        for &b in bytes {
            if self.open && b.is_ascii_digit() && !self.boxed {
                self.count += 1;
            }
            if self.prev == b'[' && b == b']' {
                self.boxed = true;
            }
            self.open = b == b'[' && matches!(self.prev, 0 | b' ' | b'\n' | b']' | b'}' | b')' | b'>');
            if b == b'\n' {
                self.boxed = false;
            }
            self.prev = b;
        }
        self.count
    }
}

/// 编译进行到哪一步，给界面显示进度用
pub enum Step {
    /// 第几遍 TeX
    Pass(u32),
    /// 这一遍已输出的页数
    Pages(u32),
    /// 辅助工具（biber、bibtex、makeindex……），值是 [`Tool`] 的名字
    Tool(&'static str),
    /// DVI 转 PDF
    Pdf,
    /// 终端输出，一块里可能有好几行（不带结尾换行）；各段开头的 `== 程序名` 也走这里。
    /// 只是实时展示用，完整日志以 [`CompileResult::log`] 为准
    Output(String),
}

fn kill_tree(child: &mut std::process::Child, job: &Job) {
    #[cfg(unix)]
    {
        let _ = job;
        // 直接发信号，不借 kill 程序：Linux 上 procps-ng 的 kill 把 `-KILL -1234` 里的 `-1234`
        // 当选项解析，只取首位数字算成 pid -1，结果杀掉当前用户的所有进程（整个登录会话）
        if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
            if pgid > 1 {
                // SAFETY: kill 没有内存方面的前置条件；pgid 是自己拉起的子进程，它是进程组组长
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
            }
        }
    }
    #[cfg(windows)]
    if let Some(j) = job {
        j.kill();
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub fn detect(pref: &str) -> String {
    match pref {
        "tectonic" | "xelatex" | "mock" => pref.to_string(),
        _ => {
            // TeX Live 是底座，tectonic 只在没有 xelatex 时顶上
            if which("xelatex").is_some() {
                "xelatex".into()
            } else if which("tectonic").is_some() {
                "tectonic".into()
            } else {
                "mock".into()
            }
        }
    }
}

/// 在当前这套 TeX 的 PATH 里找程序，返回完整路径。Windows 上按 `PATHEXT` 补后缀（`xelatex` → `xelatex.exe`，
/// `tlmgr` → `tlmgr.bat`）；调用方拿返回的路径起进程，批处理文件标准库会经 cmd.exe 跑
pub fn which(bin: &str) -> Option<PathBuf> {
    crate::tex::which_in(&crate::tex::path()?, bin)
}

/// `dir` 里有没有程序 `bin`，有就返回完整路径（Windows 上带后缀）
pub fn program_in(dir: &Path, bin: &str) -> Option<PathBuf> {
    let p = dir.join(bin);
    if p.is_file() {
        return Some(p);
    }
    if cfg!(windows) && Path::new(bin).extension().is_none() {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        return exts
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| dir.join(format!("{bin}{}", e.to_ascii_lowercase())))
            .find(|p| p.is_file());
    }
    None
}

pub fn compile(
    engine: &str,
    project: &Path,
    entry: &str,
    outdir: &Path,
    timeout: Duration,
    limits: &Limits,
    on: &dyn Fn(Step),
) -> CompileResult {
    let _ = fs::create_dir_all(outdir);
    let deadline = Instant::now() + timeout;
    match engine {
        "tectonic" => run_tectonic(project, entry, outdir, deadline, limits, on),
        "xelatex" | "pdflatex" | "lualatex" | "platex" | "uplatex" => run_tex(engine, project, entry, outdir, deadline, limits, on, false),
        _ => run_mock(project, entry, outdir),
    }
}

/// 给 AI 修复用的诊断编译：和 `compile` 一样，只是让 TeX 在报错时打出完整的宏调用链。
/// LaTeX 默认 `\errorcontextlines=-1`，只剩 `l.<行>`；可「在哪一层宏里出的错」恰恰是定位原因的线索
/// ——比如页脚里出错，报的却是分页那一行（常常是 `\end{document}`）。tectonic 不支持这样启动，照常编
pub fn compile_diagnose(engine: &str, project: &Path, entry: &str, outdir: &Path, timeout: Duration, limits: &Limits) -> CompileResult {
    let _ = fs::create_dir_all(outdir);
    let deadline = Instant::now() + timeout;
    match engine {
        "xelatex" | "pdflatex" | "lualatex" | "platex" | "uplatex" => run_tex(engine, project, entry, outdir, deadline, limits, &|_| {}, true),
        _ => compile(engine, project, entry, outdir, timeout, limits, &|_| {}),
    }
}

fn stem(entry: &str) -> String {
    Path::new(entry)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "main".into())
}

/// TeX 最多跑这么多遍。正常文档两遍，带参考文献/术语表三遍，
/// 页码引用来回震荡的文档跑满就停，不再追。
const MAX_PASSES: u32 = 5;

/// `prog` 是 `xelatex`、`pdflatex`、`lualatex`、`platex` 或 `uplatex`：命令行参数、辅助工具、多遍判断都一样。
/// xelatex 要指定输出驱动；platex / uplatex 出的是 DVI，最后用 dvipdfmx 转 PDF
/// `diag`：见 `compile_diagnose`。命令行以 `\` 开头时 TeX 把它当输入执行，先设好再 `\input` 入口文件；
/// 作业名显式给出，产物名字不变
#[allow(clippy::too_many_arguments)]
fn run_tex(prog: &str, project: &Path, entry: &str, outdir: &Path, deadline: Instant, limits: &Limits, on: &dyn Fn(Step), diag: bool) -> CompileResult {
    let mut log = String::new();
    let st = stem(entry);
    let pdf = outdir.join(format!("{st}.pdf"));
    let dvi_mode = matches!(prog, "platex" | "uplatex");
    // DVI 引擎的「产物」先是 .dvi，失败时有没有 PDF 照旧只看 .pdf
    let aux = outdir.join(format!("{st}.aux"));
    // 每个辅助工具上次跑时输入的摘要：输入没变就不再跑
    let fed_file = outdir.join(format!("{st}.subo-tools"));
    // 输出目录里留着上次成功编译的 .aux（命令行按项目复用输出目录）：从它接着编，
    // 第一遍后 .aux 没变、工具输入也没变就够了，不必每次都从零跑两遍以上
    let warm = aux.is_file();
    let bibs = bib_stamp(project, 0);
    let mut fed: Vec<(&'static str, u64)> = if warm { load_fed(&fed_file, bibs) } else { Vec::new() };
    let mut prev_aux: Option<u64> = warm.then(|| digest(&aux));
    let trees = aux_trees(project);
    let out = |s: &str| on(Step::Output(s.into()));
    for pass in 1..=MAX_PASSES {
        log.push_str(&format!("== {prog} 第 {pass} 遍\n"));
        out(&format!("== {prog} 第 {pass} 遍"));
        on(Step::Pass(pass));
        let mut cmd = command(prog);
        if let Some(aux) = &trees {
            cmd.env("TEXMFAUXTREES", aux);
        }
        // -recorder 留下 .fls（读过的全部文件）：导出项目的字盘快照靠它知道用到了用户层里的哪些包
        cmd.current_dir(project).args(["-interaction=nonstopmode", "-halt-on-error", "-file-line-error", "-synctex=-1", "-recorder"]);
        if prog == "xelatex" {
            // 默认的驱动是 `xdvipdfmx -q -E`：-q 会吞掉警告，缺字体文件时只剩一句看不出原因的
            // fatal。去掉它，`... can find this font: x.pfb` 才进得了日志（正常时只多三行）
            cmd.arg("-output-driver=xdvipdfmx -E");
        }
        let pages = |n| on(Step::Pages(n));
        cmd.arg(format!("-output-directory={}", outdir.display()));
        if diag {
            cmd.arg(format!("-jobname={st}")).arg(format!("\\errorcontextlines=99\\relax\\input{{{entry}}}"));
        } else {
            cmd.arg(entry);
        }
        let ran = run_watched(&mut cmd, deadline, Some(limits), Some(&pages), Some(&out), None);
        let hint = match ran {
            Ok(r) => {
                log.push_str(&r.log);
                if r.timed_out {
                    log.push_str("\n编译超时，已终止。可用 --timeout 调整上限。");
                }
                if let Some(o) = r.over {
                    log.push_str(&over_note(o, limits));
                }
                if !r.ok {
                    return CompileResult {
                        ok: false,
                        timed_out: r.timed_out,
                        over: r.over,
                        pdf: pdf.exists().then_some(pdf),
                        log,
                    };
                }
                wants_rerun(&r.log)
            }
            Err(e) => {
                log.push_str(&format!("无法启动 {prog}: {e}"));
                return CompileResult { ok: false, timed_out: false, over: None, pdf: None, log };
            }
        };

        // 看第一遍留下的 .bcf / .aux / .glo 等，决定要不要跑 biber、bibtex、makeglossaries
        let mut fresh = false;
        for mut tool in aux_tools(project, outdir, &st) {
            if fed.iter().any(|&(n, d)| n == tool.name && d == tool.input) {
                continue;
            }
            fed.retain(|&(n, _)| n != tool.name);
            fed.push((tool.name, tool.input));
            fresh = true;
            // bibtex 找 .bst、biber 找样式也走 kpathsea，项目里 texmf 树中的同样要看得见
            if let Some(aux) = &trees {
                for c in &mut tool.cmds {
                    c.env("TEXMFAUXTREES", aux);
                }
            }
            on(Step::Tool(tool.name));
            if let Some(stop) = run_tool(tool, deadline, limits, &mut log, &out) {
                return CompileResult {
                    ok: false,
                    timed_out: stop.is_none(),
                    over: stop,
                    pdf: pdf.exists().then_some(pdf),
                    log,
                };
            }
        }

        let now = digest(&aux);
        // 至少两遍（目录、交叉引用），之后 .aux 不再变、也没有工具新产出、日志不再喊重跑就算稳定
        if (pass >= 2 || warm) && !fresh && !hint && prev_aux == Some(now) {
            break;
        }
        prev_aux = Some(now);
        if pass == MAX_PASSES {
            log.push_str(&format!("\n已跑满 {MAX_PASSES} 遍，交叉引用可能仍未稳定。\n"));
        }
    }
    save_fed(&fed_file, &fed, bibs);
    if dvi_mode {
        log.push_str("\n== dvipdfmx\n");
        out("== dvipdfmx");
        on(Step::Pdf);
        let mut cmd = command("dvipdfmx");
        cmd.current_dir(outdir).arg("-o").arg(&pdf).arg(format!("{st}.dvi"));
        match run_limited(&mut cmd, deadline, Some(limits), Some(&out)) {
            Ok(r) => {
                log.push_str(&r.log);
                if let Some(o) = r.over {
                    log.push_str(&over_note(o, limits));
                    let _ = fs::remove_file(&pdf);
                    return CompileResult { ok: false, timed_out: false, over: Some(o), pdf: None, log };
                }
                // 缺字体时 dvipdfmx 会留下一个十几字节的空壳 PDF
                if !r.ok {
                    let _ = fs::remove_file(&pdf);
                }
                if r.timed_out {
                    log.push_str("\n编译超时，已终止。可用 --timeout 调整上限。");
                    return CompileResult { ok: false, timed_out: true, over: None, pdf: None, log };
                }
            }
            Err(e) => log.push_str(&format!("无法启动 dvipdfmx: {e}")),
        }
    }
    let ok = pdf.exists();
    if !ok {
        log.push_str("编译结束但没有产出 PDF。");
    }
    CompileResult { ok, timed_out: false, over: None, pdf: ok.then_some(pdf), log }
}

/// `TEXMFAUXTREES` 的值：项目里的 texmf 树在前，字体映射在后（见 fontmap.rs），
/// 末尾留一个逗号接上默认的 TEXMF。一棵都没有返回 `None`。
fn aux_trees(project: &Path) -> Option<String> {
    let mut trees = project_trees(project);
    trees.extend(crate::fontmap::tree());
    // kpathsea 用逗号分隔路径，带逗号的目录挂不上
    let parts: Vec<String> = trees
        .iter()
        .filter_map(|t| t.to_str())
        .filter(|s| !s.contains(','))
        .map(|s| format!("{s},"))
        .collect();
    (!parts.is_empty()).then(|| parts.concat())
}

/// 项目自带的 texmf 树：根目录下名字里有 `texmf` 的目录（`texmf`、`-texmf`、`localtexmf`……），
/// 里面按 TDS 结构放着 `tex/`、`fonts/` 或 `bibtex/`。
///
/// Overleaf 上的模板常这么带自己的文档类和宏包，再用 latexmkrc 的
/// `ensure_path('TEXMFHOME', '/compile/-texmf/')` 把它挂上。我们不跑 latexmk，就按目录名认。
pub(crate) fn project_trees(project: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(project) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().contains("texmf"))
        .map(|e| e.path())
        .filter(|p| ["tex", "fonts", "bibtex"].iter().any(|d| p.join(d).is_dir()))
        .collect();
    out.sort();
    out
}

/// 日志里有没有「再跑一遍」的提示。TeX 按 79 列硬折行，先拼回去再找。
fn wants_rerun(log: &str) -> bool {
    let flat = log.replace(['\n', '\r'], "").to_lowercase();
    ["rerun to get", "rerun latex", "label(s) may have changed", "please rerun"]
        .iter()
        .any(|k| flat.contains(k))
}

/// 辅助工具的名字是固定的几个（见 [`aux_tools`]），从文件读回时按名字认回 `&'static str`
const TOOL_NAMES: &[&str] = &["biber", "bibtex", "makeglossaries", "makeindex (glossaries)", "makeindex", "makeindex (nomencl)"];

/// 上次编译时各辅助工具的输入摘要，接着编时用（见 `run_tex` 的 `warm`）。
/// `.aux`、`.bcf` 的摘要看不出 `.bib` 改没改：项目里的 `.bib`/`.bst` 变了，文献工具就当没跑过
fn load_fed(file: &Path, bibs: u64) -> Vec<(&'static str, u64)> {
    let text = fs::read_to_string(file).unwrap_or_default();
    let same_bibs = text.lines().next().and_then(|l| l.strip_prefix("bibs ")).and_then(|d| d.parse().ok()) == Some(bibs);
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let (n, d) = l.rsplit_once(' ')?;
            Some((*TOOL_NAMES.iter().find(|t| **t == n)?, d.parse().ok()?))
        })
        .filter(|(n, _)| same_bibs || !matches!(*n, "biber" | "bibtex"))
        .collect()
}

fn save_fed(file: &Path, fed: &[(&'static str, u64)], bibs: u64) {
    let text: String = fed.iter().map(|(n, d)| format!("{n} {d}\n")).collect();
    let _ = fs::write(file, format!("bibs {bibs}\n{text}"));
}

/// 项目里（往下两层）`.bib`、`.bst` 的路径、大小、修改时间的摘要
fn bib_stamp(dir: &Path, depth: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if depth < 2 {
                bib_stamp(&p, depth + 1).hash(&mut h);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bib") || x.eq_ignore_ascii_case("bst")) {
            p.hash(&mut h);
            if let Ok(m) = e.metadata() {
                m.len().hash(&mut h);
                m.modified().ok().hash(&mut h);
            }
        }
    }
    h.finish()
}

/// 一次辅助工具调用。`input` 是它读的文件的摘要，用来判断输入变了没有。
struct Tool {
    name: &'static str,
    input: u64,
    cmds: Vec<Command>,
}

/// 项目里（往下两层）有没有 .bib
fn has_bib(dir: &Path, depth: usize) -> bool {
    let Ok(rd) = fs::read_dir(dir) else { return false };
    rd.flatten().any(|e| {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            depth < 2 && has_bib(&p, depth + 1)
        } else {
            p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bib"))
        }
    })
}

/// 第一遍之后要跑的辅助工具，按 biber/bibtex → 术语表 → 索引的顺序。
/// 只用精简版内置 TeX 实际带了的：biber、bibtex、makeindex，
/// makeglossaries 是 perl 脚本，有 perl 才用，没有就照它的做法直接调 makeindex。
fn aux_tools(project: &Path, outdir: &Path, st: &str) -> Vec<Tool> {
    let mut out = Vec::new();
    let aux_text = fs::read_to_string(outdir.join(format!("{st}.aux"))).unwrap_or_default();

    // biblatex + biber：每遍都会写 .bcf
    let bcf = outdir.join(format!("{st}.bcf"));
    // arXiv 的源码包常只带编好的 .bbl、不带 .bib：这时跑 biber/bibtex 只会在输出目录
    // 写个空的 .bbl，把项目里那份盖掉。xelatex 会顺着项目目录读到原来的
    let ships_bbl = project.join(format!("{st}.bbl")).is_file() && !has_bib(project, 0);
    if !ships_bbl && bcf.is_file() {
        let mut cmd = command("biber");
        // bib 文件相对项目根找；.bcf 和产出的 .bbl 都在输出目录
        cmd.current_dir(project)
            .arg(format!("--output-directory={}", outdir.display()))
            .arg(st);
        out.push(Tool { name: "biber", input: digest(&bcf), cmds: vec![cmd] });
    } else if !ships_bbl && aux_text.contains("\\bibdata{") {
        // natbib / 传统 bibtex（含 biblatex backend=bibtex）
        let mut cmd = command("bibtex");
        cmd.current_dir(outdir).arg(st);
        // 在输出目录里跑，项目里的 .bib / .bst 要另外告诉它；末尾留空表示接着找默认路径
        for var in ["BIBINPUTS", "BSTINPUTS"] {
            let mut dirs = vec![project.to_path_buf()];
            match std::env::var_os(var) {
                Some(v) => dirs.extend(std::env::split_paths(&v)),
                None => dirs.push(PathBuf::new()),
            }
            if let Ok(v) = std::env::join_paths(dirs) {
                cmd.env(var, v);
            }
        }
        out.push(Tool { name: "bibtex", input: bibtex_input(outdir, &aux_text), cmds: vec![cmd] });
    }

    // glossaries：\makeglossaries 往 .aux 里写 \@istfilename 和每个术语表的扩展名
    if let Some(ist) = aux_arg(&aux_text, "\\@istfilename{") {
        let glossaries = new_glossaries(&aux_text);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for g in &glossaries {
            std::hash::Hash::hash(&digest(&outdir.join(format!("{st}.{}", g.input))), &mut h);
        }
        let input = std::hash::Hasher::finish(&h);
        match makeglossaries() {
            Some((perl, script)) => {
                let mut cmd = command(perl);
                cmd.current_dir(outdir).arg(script).arg(st);
                out.push(Tool { name: "makeglossaries", input, cmds: vec![cmd] });
            }
            // xindy 精简版没带，只能走 makeindex
            None if !ist.ends_with(".xdy") => {
                let mut cmds = Vec::new();
                for g in glossaries {
                    let mut cmd = command("makeindex");
                    cmd.current_dir(outdir).args([
                        "-s",
                        &ist,
                        "-t",
                        &format!("{st}.{}", g.log),
                        "-o",
                        &format!("{st}.{}", g.output),
                        &format!("{st}.{}", g.input),
                    ]);
                    cmds.push(cmd);
                }
                out.push(Tool { name: "makeindex (glossaries)", input, cmds });
            }
            None => {}
        }
    }

    // \makeindex（makeidx / imakeidx）
    let idx = outdir.join(format!("{st}.idx"));
    if idx.is_file() {
        let mut cmd = command("makeindex");
        cmd.current_dir(outdir).arg(format!("{st}.idx"));
        out.push(Tool { name: "makeindex", input: digest(&idx), cmds: vec![cmd] });
    }
    // nomencl
    let nlo = outdir.join(format!("{st}.nlo"));
    if nlo.is_file() {
        let mut cmd = command("makeindex");
        cmd.current_dir(outdir).args([
            &format!("{st}.nlo"),
            "-s",
            "nomencl.ist",
            "-o",
            &format!("{st}.nls"),
        ]);
        out.push(Tool { name: "makeindex (nomencl)", input: digest(&nlo), cmds: vec![cmd] });
    }
    out
}

/// 跑一个辅助工具，输出写进日志。工具自己出错（bib 写错、缺 .bst）只记下来接着排版，
/// 引用会显示成 [?]，原因在日志里；只有超时、超限才中止整次编译：
/// 超时返回 `Some(None)`，超限返回 `Some(Some(哪一项))`。`out` 收实时输出
fn run_tool(tool: Tool, deadline: Instant, limits: &Limits, log: &mut String, out: &dyn Fn(&str)) -> Option<Option<Over>> {
    for mut cmd in tool.cmds {
        log.push_str(&format!("\n== {}\n", tool.name));
        out(&format!("== {}", tool.name));
        match run_limited(&mut cmd, deadline, Some(limits), Some(out)) {
            Ok(r) => {
                log.push_str(&r.log);
                if r.timed_out {
                    log.push_str("\n编译超时，已终止。可用 --timeout 调整上限。");
                    return Some(None);
                }
                if let Some(o) = r.over {
                    log.push_str(&over_note(o, limits));
                    return Some(Some(o));
                }
                if !r.ok {
                    log.push_str(&format!("\n{} 没有正常结束，参考文献或术语表可能不完整。\n", tool.name));
                }
            }
            Err(e) => log.push_str(&format!("无法启动 {}: {e}\n", tool.name)),
        }
    }
    None
}

/// 找 makeglossaries 脚本和 perl。内置 TeX 的 bin 里没有它的入口，脚本在 texmf-dist/scripts 下。
fn makeglossaries() -> Option<(PathBuf, PathBuf)> {
    let perl = which("perl")?;
    if let Some(p) = which("makeglossaries") {
        return Some((perl, p));
    }
    let out = command("kpsewhich")
        .args(["-format=texmfscripts", "makeglossaries"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    p.is_file().then_some((perl, p))
}

/// `\@newglossary{main}{glg}{gls}{glo}`：名字、日志、输出、输入的扩展名
#[derive(Debug, PartialEq)]
struct Glossary {
    log: String,
    output: String,
    input: String,
}

fn new_glossaries(aux: &str) -> Vec<Glossary> {
    aux.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("\\@newglossary")?;
            let args: Vec<&str> = rest
                .split('}')
                .filter_map(|a| a.trim().strip_prefix('{'))
                .collect();
            let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            match args[..] {
                [_, log, output, input, ..] if ok(log) && ok(output) && ok(input) => Some(Glossary {
                    log: log.into(),
                    output: output.into(),
                    input: input.into(),
                }),
                _ => None,
            }
        })
        .collect()
}

/// 取 `\cmd{arg}` 的参数（.aux 里一行一条）
fn aux_arg(aux: &str, cmd: &str) -> Option<String> {
    aux.lines()
        .find_map(|l| l.trim().strip_prefix(cmd)?.split('}').next().map(str::to_string))
        .filter(|a| !a.is_empty() && !a.contains(['/', '\\']))
}

/// bibtex 只看 .aux 里的 \citation / \bibdata / \bibstyle，连同 \include 出来的子 .aux
fn bibtex_input(outdir: &Path, aux: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut feed = |text: &str| {
        for l in text.lines() {
            if ["\\citation{", "\\bibdata{", "\\bibstyle{"].iter().any(|p| l.starts_with(p)) {
                std::hash::Hash::hash(l, &mut h);
            }
        }
    };
    feed(aux);
    for l in aux.lines() {
        if let Some(sub) = l.strip_prefix("\\@input{").and_then(|r| r.strip_suffix('}')) {
            if !sub.contains("..") {
                feed(&fs::read_to_string(outdir.join(sub)).unwrap_or_default());
            }
        }
    }
    std::hash::Hasher::finish(&h)
}

/// 文件内容的摘要，读不到算 0
fn digest(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match fs::read(path) {
        Ok(b) => b.hash(&mut h),
        Err(_) => return 0,
    }
    h.finish()
}

fn run_tectonic(project: &Path, entry: &str, outdir: &Path, deadline: Instant, limits: &Limits, on: &dyn Fn(Step)) -> CompileResult {
    let ran = run_limited(
        command("tectonic")
            .current_dir(project)
            .arg("--outdir")
            .arg(outdir)
            .arg("--keep-logs")
            .arg(entry),
        deadline,
        Some(limits),
        Some(&|s: &str| on(Step::Output(s.into()))),
    );
    match ran {
        Ok(mut r) => {
            if r.timed_out {
                r.log.push_str("\n编译超时，已终止。可用 --timeout 调整上限。");
            }
            if let Some(o) = r.over {
                r.log.push_str(&over_note(o, limits));
            }
            let pdf = outdir.join(format!("{}.pdf", stem(entry)));
            let ok = r.ok && pdf.exists();
            CompileResult { ok, timed_out: r.timed_out, over: r.over, pdf: ok.then_some(pdf), log: r.log }
        }
        Err(e) => CompileResult {
            ok: false,
            timed_out: false,
            over: None,
            pdf: None,
            log: format!("无法启动 tectonic: {e}"),
        },
    }
}

fn run_mock(project: &Path, entry: &str, outdir: &Path) -> CompileResult {
    let src = fs::read_to_string(project.join(entry)).unwrap_or_default();
    let mut lines: Vec<String> = vec![
        "Subo / 素帛 -- mock engine".into(),
        "No TeX binary was found on PATH.".into(),
        "This PDF only proves the pipeline works.".into(),
        String::new(),
    ];
    lines.extend(src.lines().take(18).map(|l| l.to_string()));

    let pdf = outdir.join(format!("{}.pdf", stem(entry)));
    match mock_pdf(&pdf, &lines) {
        Ok(()) => CompileResult {
            ok: true,
            timed_out: false,
            over: None,
            pdf: Some(pdf),
            log: "mock 引擎：未找到 TeX，已生成占位 PDF（整条链路仍然验证成功）。".into(),
        },
        Err(e) => CompileResult {
            ok: false,
            timed_out: false,
            over: None,
            pdf: None,
            log: format!("写占位 PDF 失败: {e}"),
        },
    }
}

fn escape_pdf(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            _ => o.push(c),
        }
    }
    o
}

/// 最小可用的单页 PDF：只要求被 PDF.js 正常打开。
fn mock_pdf(path: &Path, lines: &[String]) -> std::io::Result<()> {
    let mut text = String::from("BT\n/F1 11 Tf\n72 780 Td\n15 TL\n");
    for l in lines {
        let ascii: String = l.chars().filter(|c| c.is_ascii() && !c.is_control()).collect();
        text.push_str(&format!("({}) Tj\nT*\n", escape_pdf(&ascii)));
    }
    text.push_str("ET\n");

    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}endstream", text.len(), text),
    ];

    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objs.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objs.len() + 1,
            xref
        )
        .as_bytes(),
    );

    let mut f = fs::File::create(path)?;
    f.write_all(&out)
}

/// 项目源码是写给哪个 TeX 程序的，以及项目最终用哪个
pub mod program {
    use std::fs;
    use std::path::Path;

    /// 文档本身要求的 TeX 程序。和 [`super::detect`] 不同：那个看本机装了哪个引擎，
    /// 这个看项目源码「是写给哪个引擎的」
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Program {
        Pdflatex,
        Xelatex,
        Lualatex,
        /// 日文：pLaTeX（日文汉字走 JIS 编码）与 upLaTeX（Unicode），都由 e-upTeX 跑、出 DVI
        Platex,
        Uplatex,
    }

    /// 探测结果和依据，依据给界面解释「为什么用这个引擎」
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Detected {
        pub program: Program,
        pub reason: Reason,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Reason {
        /// `% !TEX program = …`（TeXShop / TeXstudio / VS Code LaTeX Workshop 通用的魔法注释）
        MagicComment,
        /// latexmk 的配置 `latexmkrc` / `.latexmkrc`（Overleaf 也读它）里的 `$pdf_mode`
        Latexmkrc,
        /// arXiv 源码包 `00README.json` 的 `process.compiler`
        ArxivReadme,
        /// 导言区里只有某个引擎能用的宏包或命令，带上认出的那个名字
        Preamble(String),
    }

    impl Program {
        pub fn parse(name: &str) -> Option<Program> {
            parse_program(name)
        }

        pub fn name(self) -> &'static str {
            match self {
                Program::Pdflatex => "pdflatex",
                Program::Xelatex => "xelatex",
                Program::Lualatex => "lualatex",
                Program::Platex => "platex",
                Program::Uplatex => "uplatex",
            }
        }
    }

    /// 项目最终用的程序和来由
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Choice {
        pub program: Program,
        pub source: Source,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Source {
        /// 用户在项目里手动选的
        User,
        /// 按源码探测出来的
        Detected(Reason),
        /// 源码里看不出来，用调用方给的默认值
        Default,
        /// 要用的程序（源码探测出来的，`reason` 为 `None` 时是用户选的）还没装，先用 XeLaTeX 试编，
        /// 问题面板提示安装
        Missing { wanted: Program, reason: Option<Reason> },
    }

    /// `chosen` 是用户选的（`None` 为自动），`fallback` 是探测没有结论时用的，`available` 答这个程序装了没有。
    /// 要用的没装就退回 XeLaTeX（内置、Unicode，最可能编得过），记下缺的是哪个
    pub fn resolve(
        project: &Path,
        entry: &str,
        chosen: Option<Program>,
        fallback: Program,
        available: impl Fn(Program) -> bool,
    ) -> Choice {
        let (program, source) = match chosen {
            Some(p) => (p, Source::User),
            None => match detect_program(project, entry) {
                Some(d) => (d.program, Source::Detected(d.reason)),
                None => (fallback, Source::Default),
            },
        };
        if program == Program::Xelatex || available(program) {
            return Choice { program, source };
        }
        let reason = match source {
            Source::Detected(r) => Some(r),
            _ => None,
        };
        Choice { program: Program::Xelatex, source: Source::Missing { wanted: program, reason } }
    }

    /// 按依据从强到弱：魔法注释 → latexmkrc → arXiv 的 00README.json → 导言区特征。都没有返回 `None`，
    /// 由调用方用默认引擎。只读主文件和项目里它用到的 .cls，不跟 `\input`
    pub fn detect_program(project: &Path, entry: &str) -> Option<Detected> {
        let text = fs::read_to_string(project.join(entry)).ok()?;
        if let Some(p) = magic_program(&text) {
            return Some(Detected { program: p, reason: Reason::MagicComment });
        }
        if let Some(p) = latexmkrc_program(project) {
            return Some(Detected { program: p, reason: Reason::Latexmkrc });
        }
        if let Some(p) = arxiv_compiler(project) {
            return Some(Detected { program: p, reason: Reason::ArxivReadme });
        }
        let mut pre = preamble(&text);
        // 项目自带的文档类（arXiv 论文常见）也算导言区：deepseek.cls 里的 inputenc、fontspec 在那儿
        if let Some(cls) = documentclass(&pre) {
            if let Ok(c) = fs::read_to_string(project.join(format!("{cls}.cls"))) {
                pre.push('\n');
                pre.push_str(&strip_comments(&c));
            }
        }
        preamble_program(&pre).map(|(program, why)| Detected { program, reason: Reason::Preamble(why) })
    }

    fn parse_program(name: &str) -> Option<Program> {
        match name.trim().to_ascii_lowercase().as_str() {
            "pdflatex" | "pdftex" | "latex" => Some(Program::Pdflatex),
            "xelatex" | "xetex" => Some(Program::Xelatex),
            "lualatex" | "luatex" | "lualatexmk" => Some(Program::Lualatex),
            "platex" | "ptex" => Some(Program::Platex),
            "uplatex" | "uptex" => Some(Program::Uplatex),
            _ => None,
        }
    }

    /// 只看文件开头的注释块（TeXShop 的约定是前 20 行）。`% !TEX TS-program` 是 TeXShop 的旧写法
    fn magic_program(text: &str) -> Option<Program> {
        text.lines().take(20).find_map(|l| {
            let rest = l.trim_start().strip_prefix('%')?.trim_start().strip_prefix('!')?.trim_start();
            let rest = rest.get(3..).filter(|_| rest[..3].eq_ignore_ascii_case("TEX"))?.trim_start();
            let (key, val) = rest.split_once('=')?;
            let key = key.trim().to_ascii_lowercase();
            (key == "program" || key == "ts-program").then(|| parse_program(val)).flatten()
        })
    }

    /// `$pdf_mode = 5;`（1 pdflatex、4 lualatex、5 xelatex），或旧时 Overleaf 的
    /// `$pdflatex = 'xelatex …'` 写法
    fn latexmkrc_program(project: &Path) -> Option<Program> {
        let text = ["latexmkrc", ".latexmkrc"].iter().find_map(|n| fs::read_to_string(project.join(n)).ok())?;
        let mut found = None;
        for line in text.lines() {
            let code = line.split('#').next().unwrap_or("");
            let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
            if let Some(v) = compact.strip_prefix("$pdf_mode=") {
                found = match v.trim_end_matches(';') {
                    "1" => Some(Program::Pdflatex),
                    "4" => Some(Program::Lualatex),
                    "5" => Some(Program::Xelatex),
                    _ => found,
                };
            } else if let Some(v) = compact.strip_prefix("$pdflatex=") {
                let cmd = v.trim_start_matches(['\'', '"']);
                if cmd.starts_with("xelatex") {
                    found = Some(Program::Xelatex);
                } else if cmd.starts_with("lualatex") {
                    found = Some(Program::Lualatex);
                }
            }
        }
        found
    }

    /// `{"process": {"compiler": "pdflatex"}}`；arXiv 还可能写 `latex`（走 dvi），一样按 pdfTeX 系算
    fn arxiv_compiler(project: &Path) -> Option<Program> {
        let meta: serde_json::Value = serde_json::from_slice(&fs::read(project.join("00README.json")).ok()?).ok()?;
        parse_program(meta.get("process")?.get("compiler")?.as_str()?)
    }

    /// 去掉 `%` 注释（`\%` 不算）
    fn strip_comments(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for line in text.lines() {
            let b = line.as_bytes();
            let cut = (0..b.len()).find(|&i| b[i] == b'%' && (i == 0 || b[i - 1] != b'\\'));
            out.push_str(&line[..cut.unwrap_or(line.len())]);
            out.push('\n');
        }
        out
    }

    /// `\begin{document}` 之前、去掉注释
    fn preamble(text: &str) -> String {
        let code = strip_comments(text);
        match code.find("\\begin{document}") {
            Some(at) => code[..at].to_string(),
            None => code,
        }
    }

    fn documentclass(pre: &str) -> Option<String> {
        first_arg(pre, "\\documentclass")
    }

    /// `\cmd[…]{arg}` 的第一个花括号参数
    fn first_arg(pre: &str, cmd: &str) -> Option<String> {
        let at = pre.find(cmd)?;
        let rest = &pre[at..];
        let open = rest.find('{')?;
        let close = rest[open..].find('}')?;
        let name = rest[open + 1..open + close].trim();
        (!name.is_empty() && !name.contains(['/', '\\'])).then(|| name.to_string())
    }

    /// `\PassOptionsToPackage{选项}{宏包}` 里的宏包：文档类常这样预先配置 fontspec、xeCJK
    fn configured_packages(pre: &str) -> Vec<String> {
        let mut out = Vec::new();
        let cmd = "\\PassOptionsToPackage";
        let mut rest = pre;
        while let Some(at) = rest.find(cmd) {
            rest = &rest[at + cmd.len()..];
            let Some(o) = rest.trim_start().strip_prefix('{') else { continue };
            let Some(end) = o.find('}') else { break };
            let Some(p) = o[end + 1..].trim_start().strip_prefix('{') else { continue };
            let Some(pend) = p.find('}') else { break };
            out.extend(p[..pend].split(',').map(|n| n.trim().to_string()));
        }
        out
    }

    /// `\usepackage[…]{a,b}` / `\RequirePackage{…}` 加载的宏包及其可选参数
    fn packages(pre: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for cmd in ["\\usepackage", "\\RequirePackage"] {
            let mut rest = pre;
            while let Some(at) = rest.find(cmd) {
                rest = &rest[at + cmd.len()..];
                let mut s = rest.trim_start();
                let mut opts = String::new();
                if let Some(o) = s.strip_prefix('[') {
                    let Some(end) = o.find(']') else { break };
                    opts = o[..end].to_string();
                    s = o[end + 1..].trim_start();
                }
                let Some(body) = s.strip_prefix('{') else { continue };
                let Some(end) = body.find('}') else { break };
                for name in body[..end].split(',') {
                    out.push((name.trim().to_string(), opts.clone()));
                }
            }
        }
        out
    }

    /// 只有一个引擎能用的特征。XeTeX / LuaTeX 的特征优先：fontspec 在 pdfTeX 下必然失败，
    /// 而 `\pdfoutput=1` 只是 arXiv 的习惯写法，XeTeX 下无害
    fn preamble_program(pre: &str) -> Option<(Program, String)> {
        let pkgs = packages(pre);
        let configured = configured_packages(pre);
        let has = |n: &str| pkgs.iter().any(|(p, _)| p == n) || configured.iter().any(|p| p == n);
        for p in ["luatexja", "luacode", "luaotfload"] {
            if has(p) {
                return Some((Program::Lualatex, p.into()));
            }
        }
        // 日文文档类：u 开头的是 upLaTeX 的；jsarticle 这类两边都能用，带 uplatex 选项时归 upLaTeX
        if let Some(cls) = documentclass(pre) {
            let uplatex_opt = pre
                .find("\\documentclass")
                .and_then(|at| pre[at..].split_once('{'))
                .is_some_and(|(head, _)| head.contains("uplatex"));
            let up = ["ujarticle", "ujbook", "ujreport", "utarticle", "utbook", "utreport"];
            let p = ["jarticle", "jbook", "jreport", "tarticle", "tbook", "treport", "jsarticle", "jsbook", "jsreport"];
            if up.contains(&cls.as_str()) || (p.contains(&cls.as_str()) && uplatex_opt) {
                return Some((Program::Uplatex, cls));
            }
            if p.contains(&cls.as_str()) {
                return Some((Program::Platex, cls));
            }
        }
        // ctex 在 pdfTeX 下也能编，但走的是 CJK 宏包加 Type1 字体（精简包里不带），按 XeTeX 处理
        // 项目自带的文档类常在里面 `\LoadClass{ctexrep}`（zjuthesis 这类学位论文模板）
        for cls in [documentclass(pre), first_arg(pre, "\\LoadClass")].into_iter().flatten() {
            if cls.starts_with("ctex") {
                return Some((Program::Xelatex, cls));
            }
        }
        for p in ["fontspec", "xeCJK", "unicode-math", "ctex", "polyglossia", "xltxtra", "mathspec"] {
            if has(p) {
                return Some((Program::Xelatex, p.into()));
            }
        }
        for c in ["\\setmainfont", "\\setCJKmainfont"] {
            if pre.contains(c) {
                return Some((Program::Xelatex, c.into()));
            }
        }
        for p in ["CJK", "CJKutf8"] {
            if has(p) {
                return Some((Program::Pdflatex, p.into()));
            }
        }
        // utf8 是 XeTeX 下的空操作；latin1、cp1252、ansinew 这些 8 位编码 XeTeX 不认
        if let Some((_, opts)) = pkgs.iter().find(|(p, _)| p == "inputenc") {
            if opts.split(',').map(str::trim).any(|o| !o.is_empty() && o != "utf8" && o != "utf8x") {
                return Some((Program::Pdflatex, format!("inputenc[{opts}]")));
            }
        }
        let compact: String = pre.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("\\pdfoutput=1") {
            return Some((Program::Pdflatex, "\\pdfoutput=1".into()));
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn program_of(files: &[(&str, &str)]) -> Option<Detected> {
            let d = crate::engine::tests::scratch("program");
            for (name, body) in files {
                fs::write(d.join(name), body).unwrap();
            }
            let got = detect_program(&d, files[0].0);
            let _ = fs::remove_dir_all(&d);
            got
        }

        fn pre(program: Program, why: &str) -> Option<Detected> {
            Some(Detected { program, reason: Reason::Preamble(why.into()) })
        }

        #[test]
        fn resolve_order() {
            let d = crate::engine::tests::scratch("resolve");
            fs::write(d.join("main.tex"), "\\documentclass{article}\n\\usepackage{CJKutf8}\n").unwrap();
            let all = |_: Program| true;
            let builtin = |p: Program| matches!(p, Program::Pdflatex | Program::Xelatex);
            let auto = resolve(&d, "main.tex", None, Program::Xelatex, all);
            assert_eq!(auto.program, Program::Pdflatex);
            assert_eq!(auto.source, Source::Detected(Reason::Preamble("CJKutf8".into())));
            let user = resolve(&d, "main.tex", Some(Program::Xelatex), Program::Pdflatex, all);
            assert_eq!((user.program, user.source), (Program::Xelatex, Source::User));
            fs::write(d.join("plain.tex"), "\\documentclass{article}\n").unwrap();
            let none = resolve(&d, "plain.tex", None, Program::Pdflatex, all);
            assert_eq!((none.program, none.source), (Program::Pdflatex, Source::Default));
            fs::write(d.join("lua.tex"), "\\documentclass{article}\n\\usepackage{luacode}\n").unwrap();
            let lua = resolve(&d, "lua.tex", None, Program::Pdflatex, builtin);
            assert_eq!(lua.program, Program::Xelatex);
            assert_eq!(
                lua.source,
                Source::Missing { wanted: Program::Lualatex, reason: Some(Reason::Preamble("luacode".into())) }
            );
            // 装上了就用它
            let lua = resolve(&d, "lua.tex", None, Program::Pdflatex, all);
            assert_eq!((lua.program, lua.source), (Program::Lualatex, Source::Detected(Reason::Preamble("luacode".into()))));
            // 用户选了没装的引擎：同样退回，依据为空
            let chose = resolve(&d, "plain.tex", Some(Program::Uplatex), Program::Pdflatex, builtin);
            assert_eq!((chose.program, chose.source), (Program::Xelatex, Source::Missing { wanted: Program::Uplatex, reason: None }));
            let _ = fs::remove_dir_all(&d);
        }

        #[test]
        fn magic_comment_wins() {
            let doc = "% !TEX program = pdflatex\n\\documentclass{article}\n\\usepackage{fontspec}\n";
            assert_eq!(
                program_of(&[("main.tex", doc)]),
                Some(Detected { program: Program::Pdflatex, reason: Reason::MagicComment })
            );
            let ts = "%!TEX TS-program = XeLaTeX\n\\documentclass{article}\n";
            assert_eq!(program_of(&[("main.tex", ts)]).unwrap().program, Program::Xelatex);
            // 混合大小写（标准写法 % !TeX）
            let mixed = "% !TeX program = xelatex\n\\documentclass{article}\n";
            assert_eq!(program_of(&[("main.tex", mixed)]).unwrap().program, Program::Xelatex);
            // 正文里的同样字样不算：只看开头 20 行
            let late = format!("\\documentclass{{article}}\n{}% !TEX program = lualatex\n", "\n".repeat(30));
            assert_eq!(program_of(&[("main.tex", &late)]), None);
        }

        #[test]
        fn arxiv_readme_compiler() {
            let readme = r#"{"sources":[{"filename":"article.tex","usage":"toplevel"}],"process":{"compiler":"pdflatex"}}"#;
            let got = program_of(&[("article.tex", "\\documentclass{article}\n"), ("00README.json", readme)]);
            assert_eq!(got, Some(Detected { program: Program::Pdflatex, reason: Reason::ArxivReadme }));
        }

        #[test]
        fn xetex_packages() {
            let doc = "\\documentclass{article}\n\\usepackage[T1]{fontenc}\n\\usepackage{amsmath,unicode-math}\n";
            assert_eq!(program_of(&[("main.tex", doc)]), pre(Program::Xelatex, "unicode-math"));
            assert_eq!(program_of(&[("main.tex", "\\documentclass[UTF8]{ctexart}\n")]), pre(Program::Xelatex, "ctexart"));
            // fontspec 压过 \pdfoutput=1
            let both = "\\pdfoutput=1\n\\documentclass{article}\n\\usepackage{fontspec}\n\\setmainfont{TeX Gyre Termes}\n";
            assert_eq!(program_of(&[("main.tex", both)]), pre(Program::Xelatex, "fontspec"));
            assert_eq!(program_of(&[("main.tex", "\\documentclass{article}\n\\usepackage{luacode}\n")]), pre(Program::Lualatex, "luacode"));
        }

        #[test]
        fn japanese_classes() {
            assert_eq!(program_of(&[("main.tex", "\\documentclass[a4paper]{jsarticle}\n")]), pre(Program::Platex, "jsarticle"));
            assert_eq!(program_of(&[("main.tex", "\\documentclass[uplatex,dvipdfmx]{jsbook}\n")]), pre(Program::Uplatex, "jsbook"));
            assert_eq!(program_of(&[("main.tex", "\\documentclass{ujarticle}\n")]), pre(Program::Uplatex, "ujarticle"));
            assert_eq!(program_of(&[("main.tex", "% !TEX program = uplatex\n\\documentclass{article}\n")]).unwrap().program, Program::Uplatex);
        }

        #[test]
        fn pdftex_signals() {
            let cjk = "\\documentclass{article}\n\\usepackage{CJKutf8}\n\\begin{document}\n\\usepackage{fontspec}\n";
            assert_eq!(program_of(&[("main.tex", cjk)]), pre(Program::Pdflatex, "CJKutf8"));
            let latin1 = "\\documentclass{article}\n\\usepackage[latin1]{inputenc}\n";
            assert_eq!(program_of(&[("main.tex", latin1)]), pre(Program::Pdflatex, "inputenc[latin1]"));
            let arxiv = "\\pdfoutput = 1\n\\documentclass{article}\n\\usepackage[utf8]{inputenc}\n";
            assert_eq!(program_of(&[("main.tex", arxiv)]), pre(Program::Pdflatex, "\\pdfoutput=1"));
        }

        #[test]
        fn neutral_and_commented_out() {
            assert_eq!(program_of(&[("main.tex", "\\documentclass{article}\n\\usepackage[utf8]{inputenc}\n")]), None);
            let off = "\\documentclass{article}\n% \\usepackage{fontspec}\n\\usepackage{amsmath}% fontspec 以后再说\n";
            assert_eq!(program_of(&[("main.tex", off)]), None);
            // \% 不是注释
            let pct = "\\documentclass{article}\n\\newcommand{\\p}{50\\%}\\usepackage{xeCJK}\n";
            assert_eq!(program_of(&[("main.tex", pct)]), pre(Program::Xelatex, "xeCJK"));
        }

        #[test]
        fn latexmkrc_and_class_internals() {
            let rc = "$out_dir=\"out\";\n# $pdf_mode = 1;\n$pdf_mode = 5;\n";
            assert_eq!(
                program_of(&[("main.tex", "\\documentclass{article}\n\\usepackage{CJKutf8}\n"), (".latexmkrc", rc)]),
                Some(Detected { program: Program::Xelatex, reason: Reason::Latexmkrc })
            );
            let old = "$pdflatex = 'xelatex -synctex=1 %O %S';\n";
            assert_eq!(program_of(&[("main.tex", "\\documentclass{article}\n"), ("latexmkrc", old)]).unwrap().program, Program::Xelatex);
            // zjuthesis：主文件只有 \documentclass{zjuthesis}，ctexrep 由类文件 \LoadClass
            let cls = "\\PassOptionsToPackage{no-math}{fontspec}\n\\LoadClass[UTF8,a4paper]{ctexrep}\n";
            assert_eq!(
                program_of(&[("zjuthesis.tex", "\\documentclass[type=master]{zjuthesis}\n"), ("zjuthesis.cls", cls)]),
                pre(Program::Xelatex, "ctexrep")
            );
            assert_eq!(
                program_of(&[("main.tex", "\\documentclass{x}\n"), ("x.cls", "\\PassOptionsToPackage{AutoFakeBold}{xeCJK}\n")]),
                pre(Program::Xelatex, "xeCJK")
            );
        }

        #[test]
        fn project_class_counts_as_preamble() {
            // DeepSeek-V3 的形态：主文件只有 CJKutf8，inputenc 在自带的 deepseek.cls 里
            let cls = "\\RequirePackage[utf8]{inputenc}\n\\RequirePackage{fontspec}\n";
            let got = program_of(&[("main.tex", "\\documentclass[11pt]{deepseek}\n"), ("deepseek.cls", cls)]);
            assert_eq!(got, pre(Program::Xelatex, "fontspec"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_scan_counts_shipouts() {
        let mut p = PageScan::default();
        // 跨块切断也要数对
        p.feed(b"(./main.tex [1{/usr/share/pdftex.map}] [");
        assert_eq!(p.feed(b"2] (./intro.tex [3 <./fig.pdf>]) [4]\n"), 4);
        // 行过宽报告里的引用编号、宏包的日期选项都不算
        assert_eq!(p.feed(b"Overfull \\hbox (1.0pt too wide) in paragraph\n[]\\T1/ptm/m/n/10 see [12] and [13]\n"), 4);
        assert_eq!(p.feed(b"Package: foo[2024/01/01]\n[5]"), 5);
    }

    pub(super) fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "subo-engine-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn project_texmf_trees() {
        let d = scratch("texmf");
        for sub in ["-texmf/tex/latex/x", "localtexmf/fonts", "texmf-notes", "style/tex", "TEXMF/bibtex/bst"] {
            fs::create_dir_all(d.join(sub)).unwrap();
        }
        fs::write(d.join("texmf.cnf"), "").unwrap();
        // 名字里有 texmf、且是 TDS 结构的目录才算；空的 texmf-notes、普通的 style 不算
        assert_eq!(project_trees(&d), vec![d.join("-texmf"), d.join("TEXMF"), d.join("localtexmf")]);
        let _ = fs::remove_dir_all(&d);
    }

    fn names(tools: &[Tool]) -> Vec<&str> {
        tools.iter().map(|t| t.name).collect()
    }

    #[test]
    fn rerun_hints() {
        assert!(wants_rerun(
            "LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right.\n"
        ));
        // 79 列折行把关键词拆开
        assert!(wants_rerun("Package rerunfilecheck Warning: File `main.out' has changed.\n(rerunfilecheck)                Rer\nun to get outlines right\n"));
        assert!(wants_rerun("Package biblatex Warning: Please (re)run Biber on the file:\n(biblatex)                main\n(biblatex)                and rerun LaTeX afterwards.\n"));
        assert!(!wants_rerun("Package: rerunfilecheck 2022/07/10 v1.10\nOutput written on main.pdf (1 page).\n"));
    }

    #[test]
    fn plain_document_needs_no_tools() {
        let d = scratch("plain");
        fs::write(d.join("main.aux"), "\\relax\n\\newlabel{x}{{1}{1}}\n").unwrap();
        assert!(aux_tools(&d, &d, "main").is_empty());
    }

    #[test]
    fn biblatex_runs_biber() {
        let d = scratch("biber");
        fs::write(d.join("main.aux"), "\\relax\n\\abx@aux@cite{0}{knuth}\n").unwrap();
        fs::write(d.join("main.bcf"), "<bcf:controlfile/>").unwrap();
        let tools = aux_tools(&d, &d, "main");
        assert_eq!(names(&tools), ["biber"]);
        let args: Vec<_> = tools[0].cmds[0].get_args().collect();
        assert_eq!(args.last().unwrap().to_str(), Some("main"));
    }

    #[test]
    fn natbib_runs_bibtex_with_project_on_path() {
        let d = scratch("bibtex");
        let proj = scratch("bibtex-proj");
        fs::write(
            d.join("main.aux"),
            "\\relax\n\\citation{knuth}\n\\bibstyle{plainnat}\n\\bibdata{refs}\n",
        )
        .unwrap();
        let tools = aux_tools(&proj, &d, "main");
        assert_eq!(names(&tools), ["bibtex"]);
        let cmd = &tools[0].cmds[0];
        assert_eq!(cmd.get_current_dir(), Some(d.as_path()));
        let bib = cmd
            .get_envs()
            .find(|(k, _)| *k == "BIBINPUTS")
            .and_then(|(_, v)| v)
            .unwrap();
        assert!(std::env::split_paths(bib).next() == Some(proj.clone()));
    }

    #[test]
    fn bibtex_input_follows_included_aux() {
        let d = scratch("bibsub");
        let aux = "\\relax\n\\@input{ch1.aux}\n\\bibdata{refs}\n";
        fs::write(d.join("ch1.aux"), "\\relax\n\\citation{a}\n").unwrap();
        let before = bibtex_input(&d, aux);
        // 和引用无关的行变了不算
        fs::write(d.join("ch1.aux"), "\\relax\n\\citation{a}\n\\newlabel{x}{{1}{2}}\n").unwrap();
        assert_eq!(bibtex_input(&d, aux), before);
        fs::write(d.join("ch1.aux"), "\\relax\n\\citation{a}\n\\citation{b}\n").unwrap();
        assert_ne!(bibtex_input(&d, aux), before);
    }

    #[test]
    fn glossaries_parsed_from_aux() {
        let aux = "\\relax\n\\providecommand\\@newglossary[4]{}\n\
                   \\@newglossary{main}{glg}{gls}{glo}\n\
                   \\@newglossary{acronym}{alg}{acr}{acn}\n\
                   \\@istfilename{main.ist}\n\\@glsorder{word}\n";
        assert_eq!(aux_arg(aux, "\\@istfilename{").as_deref(), Some("main.ist"));
        let g = new_glossaries(aux);
        assert_eq!(
            g,
            [
                Glossary { log: "glg".into(), output: "gls".into(), input: "glo".into() },
                Glossary { log: "alg".into(), output: "acr".into(), input: "acn".into() },
            ]
        );
        // 路径不收
        assert_eq!(aux_arg("\\@istfilename{../x.ist}\n", "\\@istfilename{"), None);
        assert!(new_glossaries("\\@newglossary{m}{../a}{gls}{glo}\n").is_empty());
    }

    #[test]
    fn glossaries_and_index_detected() {
        let d = scratch("gls");
        fs::write(
            d.join("main.aux"),
            "\\relax\n\\@newglossary{main}{glg}{gls}{glo}\n\\@istfilename{main.ist}\n",
        )
        .unwrap();
        fs::write(d.join("main.glo"), "\\glossaryentry{tex?\\glossentry{tex}|setentrycounter[]{page}\\glsnumberformat}{1}\n").unwrap();
        fs::write(d.join("main.idx"), "\\indexentry{foo}{1}\n").unwrap();
        fs::write(d.join("main.nlo"), "\\nomenclatureentry{a}{1}\n").unwrap();
        let tools = aux_tools(&d, &d, "main");
        let n = names(&tools);
        assert!(n[0] == "makeglossaries" || n[0] == "makeindex (glossaries)", "{n:?}");
        assert_eq!(&n[1..], ["makeindex", "makeindex (nomencl)"]);
        if n[0] == "makeindex (glossaries)" {
            let args: Vec<_> = tools[0].cmds[0].get_args().map(|a| a.to_str().unwrap()).collect();
            assert_eq!(args, ["-s", "main.ist", "-t", "main.glg", "-o", "main.gls", "main.glo"]);
        }
    }

    #[test]
    fn digest_of_missing_file_is_zero() {
        let d = scratch("digest");
        assert_eq!(digest(&d.join("nope")), 0);
        fs::write(d.join("a"), "x").unwrap();
        assert_ne!(digest(&d.join("a")), 0);
    }

    #[test]
    fn memory_hog_is_killed() {
        // perl 造一个 400 MB 的字符串：内容真写进去了，常驻内存跟着涨
        let mut cmd = command("perl");
        cmd.args(["-e", "my $x = 'a' x (400*1024*1024); sleep 20"]);
        let limits = Limits { memory: 100 << 20, file_size: FILE_SIZE_CAP };
        let started = Instant::now();
        let r = run_limited(&mut cmd, Instant::now() + Duration::from_secs(30), Some(&limits), None).unwrap();
        assert_eq!(r.over, Some(Over::Memory));
        assert!(!r.ok && !r.timed_out);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn file_size_cap_stops_writer() {
        let d = scratch("fsize");
        let mut cmd = command("dd");
        cmd.current_dir(&d).args(["if=/dev/zero", "of=big", "bs=1024", "count=8192"]);
        let limits = Limits { memory: 1 << 30, file_size: 1 << 20 };
        let r = run_limited(&mut cmd, Instant::now() + Duration::from_secs(30), Some(&limits), None).unwrap();
        assert_eq!(r.over, Some(Over::FileSize));
        assert!(fs::metadata(d.join("big")).unwrap().len() <= 1 << 20);
        // 不带限额照常跑完
        let r = run(command("dd").current_dir(&d).args(["if=/dev/zero", "of=small", "bs=1024", "count=16"]), Instant::now() + Duration::from_secs(30)).unwrap();
        assert!(r.ok && r.over.is_none());
    }
}
