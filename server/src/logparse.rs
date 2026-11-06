//! 编译日志的唯一解析：问题面板、AI 修复与编译校验、命令行、HTTP 接口都从这里取问题，不各读各的。
//!
//! 输入是 engine.rs 拼好的整份日志：各遍 TeX、辅助工具（biber、bibtex……）、dvipdfmx 依次排列，
//! 段首是 `== 程序名 …` 一行。TeX 的错误、警告、排版提示只看最后一遍 TeX 及其后（第一遍的
//! 「引用未定义」到第二遍往往就好了）；biber / bibtex 自己的报错在哪一段都算。
//!
//! 编译开着 `-halt-on-error`，失败的日志里只有一个 TeX 错误，不存在「连锁错误」。
//! 一遍线性扫完，不回头；警告、排版提示有数量上限，日志再大推给界面的也有限。
//! 行号（`log_range`）按 `\n` 切分、CRLF 和单独的 `\r` 都当换行，与界面切原始日志的方式一致。

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// 断点标记：TeX 把出错处拆成两行打印，断开处就是它停下的位置
pub const HERE: &str = "⟨here⟩";
/// TeX 的 max_print_line：日志硬折在这一列
pub const WRAP: usize = 79;
/// 调用链最多留几层
const CHAIN_MAX: usize = 12;
/// 每层留断点前后各多少字
const FRAME_BEFORE: usize = 60;
const FRAME_AFTER: usize = 30;
/// 警告、排版提示各最多带几条（总数照实数）
const WARNINGS_MAX: usize = 300;
const BOXES_MAX: usize = 200;
/// 错误首行之后最多往下找多少行上下文
const CONTEXT_SPAN: usize = 400;
/// TeX 收尾时的套话：是前一条错误的余波，不单列
const NOISE: [&str; 4] = ["Emergency stop", "==> Fatal error occurred", "job aborted", "cannot \\read from terminal"];
/// 括号后面跟这些扩展名的才算 TeX 打开了一个文件
const FILE_EXTS: [&str; 20] =
    ["tex", "sty", "cls", "cfg", "def", "clo", "fd", "bbl", "aux", "toc", "lof", "lot", "out", "ldf", "ltx", "bib", "ind", "gls", "nav", "snm"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Error,
    Warning,
    Box,
}

/// 问题的类别。错误的类别决定 AI 修复去源码哪里找证据（diagnose.rs），也决定界面怎么解释它
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /* ---- 错误 ---- */
    /// 当场出错：停下的地方就是错处，或就在它前面
    #[default]
    Here,
    /// 参数没闭合，一路读到段落或文件结尾
    Scan,
    /// 环境不配对，消息里带着开头那一行
    Env,
    /// 组装页面（页眉页脚、浮动体）时出错，报的只是分页的那一行
    Output,
    /// 宏包或文档类报的错
    Package,
    /// 出在上一遍写下的辅助文件里（.aux、.toc……）
    Aux,
    /// 没有调用链可看，但停在 `\end{document}`：原因在前面
    End,
    /// PDF 驱动（xdvipdfmx）没能写出 PDF
    Driver,
    /* ---- 警告 ---- */
    UndefinedRef,
    UndefinedCite,
    MultiplyDefined,
    /// 字体里没有的字，PDF 里是空白或豆腐块
    MissingChar,
    FloatTooLarge,
    /// 字体替换、字形缺失（LaTeX Font Warning）
    Font,
    /// biber / bibtex 自己报的错
    Bib,
    /// 「There were undefined references」之类的总结和「请重编」，前面已逐条列过
    Summary,
    Other,
    /* ---- 排版提示 ---- */
    OverfullH,
    UnderfullH,
    OverfullV,
    UnderfullV,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// 同一份源码重编后不变（不含行号），界面靠它保住选中、展开的状态
    pub id: String,
    pub level: Level,
    pub kind: Kind,
    /// TeX 的原话（折行已接回，`LaTeX Error: `、`on input line N` 已去掉）
    pub message: String,
    /// 项目内相对路径；出在 TeX 发行版自己的文件里、或不知道是哪个文件时为空
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// 排版提示跨几行时的末行
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    /// 错误：TeX 停下处的源码（`l.<行>` 那两行），断开处标 ⟨here⟩
    #[serde(skip_serializing_if = "Option::is_none")]
    pub here: Option<String>,
    /// 错误：压缩过的宏调用链，最里层在前（诊断编译才有）
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub chain: Vec<String>,
    /// 错误：`Runaway argument?` 下面那行，没闭合的参数从哪开始
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runaway: Option<String>,
    /// 谁报的：宏包或文档类名、biber / bibtex、缺字时的字体名
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// 引用标签、文献键、缺的字：同类问题按它聚合
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// 超出多少 pt（溢出、浮动体太大）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pt: Option<f64>,
    /// 在整份日志里的行号范围（从 1 起，含两端）
    pub log_range: (u32, u32),
}

/// 解析结果。`errors` 等是总数，`issues` 里的警告、排版提示可能因上限少于总数
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub issues: Vec<Issue>,
    pub errors: u32,
    pub warnings: u32,
    pub boxes: u32,
}

impl Report {
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.level == Level::Error)
    }

    #[cfg(test)]
    pub fn of(&self, kind: Kind) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |i| i.kind == kind)
    }
}

/* ---------- 入口 ---------- */

pub fn parse(log: &str) -> Report {
    let text = normalize(log);
    let lines: Vec<&str> = text.split('\n').collect();
    let tex_from = lines.iter().rposition(|l| pass_marker(l)).map_or(0, |i| i + 1);
    let mut p = Parser { lines: &lines, out: Vec::new(), seen: HashSet::new(), stack: Vec::new() };
    // 当前段是哪个辅助工具；TeX 的段为空
    let mut tool = "";
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if let Some(name) = l.strip_prefix("== ") {
            tool = if pass_marker(l) { "" } else { name.split_whitespace().next().unwrap_or("") };
            i += 1;
            continue;
        }
        if !tool.is_empty() {
            p.tool_line(i, tool);
            i += 1;
        } else if i < tex_from {
            i += 1;
        } else {
            i = p.tex_line(i);
        }
    }
    p.finish()
}

/// 最后一遍 TeX（及其后）的那段日志
pub fn last_pass(log: &str) -> &str {
    let mut at = 0;
    let mut off = 0;
    for l in log.split_inclusive('\n') {
        if pass_marker(l.trim_end()) {
            at = off + l.len();
        }
        off += l.len();
    }
    &log[at..]
}

/// engine.rs 写进日志的分隔行：`== xelatex 第 2 遍`
fn pass_marker(l: &str) -> bool {
    l.starts_with("== ") && l.contains(" 第 ") && l.trim_end().ends_with('遍')
}

fn normalize(log: &str) -> Cow<'_, str> {
    if log.contains('\r') {
        Cow::Owned(log.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(log)
    }
}

/// 被 TeX 在第 79 列折断的行。pdfTeX 按字节数折、XeTeX 按字符数折，两样都看；
/// 超过 79 列的行不是 TeX 折的（辅助工具的输出），不往下接
pub fn wrapped(l: &str) -> bool {
    l.len() == WRAP || l.chars().count() == WRAP
}

/* ---------- 单行的识别 ---------- */

/// 错误首行：`./a.tex:5: 消息`（-file-line-error）、`! 消息`，tectonic 在前面加 `error: `
pub fn head(line: &str) -> Option<(Option<&str>, Option<u32>, &str)> {
    if let Some(m) = line.strip_prefix("! ") {
        return Some((None, None, m));
    }
    let line = line.strip_prefix("error: ").unwrap_or(line);
    // Windows 的绝对路径带盘符 `E:\MikTeX\…`，盘符那个冒号不算分隔
    let drive = if win_abs(line) { 2 } else { 0 };
    let mut parts = line[drive..].splitn(3, ':');
    let (f, n, m) = (parts.next()?, parts.next()?, parts.next()?);
    let f = &line[..drive + f.len()];
    let ok = !f.is_empty() && !f.contains(' ') && !f.starts_with('(') && f.contains('.') && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| (Some(f), n.parse().ok(), m.strip_prefix(' ').unwrap_or(m)))
}

fn noise(message: &str) -> bool {
    NOISE.iter().any(|n| message.trim_start().starts_with(n))
}

/// `C:\…`、`C:/…`
fn win_abs(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')
}

/// 警告首行 `LaTeX Warning: …`、`Package hyperref Warning: …`、`LaTeX Font Warning: …`：
/// 返回（发出者、正文）。发出者是 `LaTeX`、`LaTeX Font`、`Package <名>`……
fn warning_head(l: &str) -> Option<(&str, &str)> {
    let at = l.find(" Warning: ")?;
    let who = &l[..at];
    let words = who.split(' ').count();
    let ok = match who.split(' ').next() {
        Some("LaTeX") => words <= 2,
        Some("Package" | "Class") => (2..=3).contains(&words),
        _ => false,
    };
    (ok && !who.contains("  ") && !who.ends_with(' ')).then(|| (who, &l[at + " Warning: ".len()..]))
}

fn box_head(l: &str) -> Option<Kind> {
    let rest = l.strip_prefix("Overfull \\").map(|r| (true, r)).or_else(|| l.strip_prefix("Underfull \\").map(|r| (false, r)));
    match rest? {
        (true, r) if r.starts_with("hbox") => Some(Kind::OverfullH),
        (true, r) if r.starts_with("vbox") => Some(Kind::OverfullV),
        (false, r) if r.starts_with("hbox") => Some(Kind::UnderfullH),
        (false, r) if r.starts_with("vbox") => Some(Kind::UnderfullV),
        _ => None,
    }
}

/// 下一条消息的开头：警告、排版提示、错误、分段
fn starts_message(l: &str) -> bool {
    l.starts_with("== ") || warning_head(l).is_some() || box_head(l).is_some() || head(l).is_some() || l.starts_with("Missing character: ")
}

/// 警告、宏包报错的续行：`(hyperref)                removing …`
fn paren_cont(l: &str) -> bool {
    l.starts_with('(') && l.find(')').is_some_and(|p| l[p + 1..].starts_with(char::is_whitespace))
}

/// 续行去掉 `(包名)` 和垫齐的空格
fn strip_cont(l: &str) -> &str {
    if paren_cont(l) {
        l[l.find(')').unwrap_or(0) + 1..].trim()
    } else {
        l.trim()
    }
}

/// `(689.11063pt too high)` 里的 689.11063
fn points(l: &str) -> Option<f64> {
    let p = l.find('(')? + 1;
    let e = l[p..].find("pt")? + p;
    l[p..e].parse().ok()
}

/// 紧跟 `on input line ` 的行号
fn input_line(text: &str) -> Option<(usize, u32)> {
    let at = text.find("on input line ")?;
    let digits: String = text[at + "on input line ".len()..].chars().take_while(char::is_ascii_digit).collect();
    Some((at, digits.parse().ok()?))
}

/// `Reference `fig:a' on page 1` 里的 `fig:a`；新旧 LaTeX 的左引号不一样
fn quoted(text: &str) -> Option<String> {
    let open = text.find(['`', '\''])?;
    let rest = &text[open + 1..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_string()).filter(|s| !s.is_empty())
}

/// 这个路径是 TeX 读的文件吗：`./x.tex`、`/usr/…`、`E:\…`，或带 TeX 文件的扩展名
fn open_file(s: &str) -> Option<&str> {
    let end = s.find(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '[' || c == '{').unwrap_or(s.len());
    let cand = &s[..end];
    if cand.is_empty() {
        return None;
    }
    let pathy = cand.starts_with("./") || cand.starts_with("../") || cand.starts_with('/') || cand.starts_with(".\\") || win_abs(cand);
    let ext = cand.rsplit_once('.').is_some_and(|(stem, e)| !stem.is_empty() && FILE_EXTS.contains(&e));
    (pathy || ext).then_some(cand)
}

/// 项目里能打开的路径；绝对路径多半是 TeX 发行版自己的文件，项目里打不开
fn clean(path: &str) -> Option<String> {
    if path.starts_with('/') || path.starts_with('\\') || win_abs(path) {
        return None;
    }
    Some(path.trim_start_matches("./").trim_start_matches(".\\").replace('\\', "/"))
}

fn chars_tail(s: &str, n: usize) -> String {
    let c = s.chars().count();
    if c <= n {
        s.to_string()
    } else {
        format!("…{}", s.chars().skip(c - n).collect::<String>())
    }
}

fn chars_head(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// 一层上下文的名字：`<argument>`、`\@outputpage`；TeX 截断时打的 ` ...` 没有名字
fn frame_name(s: &str) -> &str {
    if s.starts_with('<') {
        return s.find('>').map(|i| &s[..=i]).unwrap_or(s);
    }
    if let Some(rest) = s.strip_prefix('\\') {
        let n = rest.char_indices().find(|&(_, c)| !(c.is_ascii_alphabetic() || matches!(c, '@' | '_' | ':'))).map(|(i, _)| i).unwrap_or(rest.len());
        let n = if n == 0 { rest.chars().next().map(char::len_utf8).unwrap_or(0) } else { n };
        return &s[..1 + n];
    }
    ""
}

/// 对定位没用的层：expl3 内部、报错机制本身
fn boring(name: &str) -> bool {
    name.is_empty() || name.contains(':') || name.contains('_') || matches!(name, "\\GenericError" | "\\@latex@error" | "\\PackageError" | "\\ClassError" | "\\@err@" | "\\msg_error:nnn")
}

/// 错误的类别，决定去源码哪里找证据
pub fn classify(message: &str, file: Option<&str>, here: Option<&str>, chain: &[String], runaway: bool) -> Kind {
    let aux = file.is_some_and(|f| [".aux", ".toc", ".lof", ".lot", ".bbl", ".out", ".ind", ".gls", ".nav", ".snm"].iter().any(|e| f.ends_with(e)));
    if chain.iter().any(|f| f.starts_with("<output>")) {
        Kind::Output
    } else if message.contains("on input line") && message.contains("ended by") {
        Kind::Env
    } else if runaway || message.starts_with("Paragraph ended before") || message.starts_with("File ended while scanning") {
        Kind::Scan
    } else if aux {
        Kind::Aux
    } else if message.starts_with("Package ") || message.starts_with("Class ") {
        Kind::Package
    } else if here.is_some_and(|h| h.trim_start().starts_with("\\end{document}")) {
        Kind::End
    } else {
        Kind::Here
    }
}

/// `Package foo Error: …`、`Class bar Error: …` 里的 foo、bar
fn reporter(message: &str) -> Option<String> {
    let rest = message.strip_prefix("Package ").or_else(|| message.strip_prefix("Class "))?;
    let (name, after) = rest.split_once(' ')?;
    after.starts_with("Error").then(|| name.to_string())
}

fn warning_kind(who: &str, text: &str) -> Kind {
    let t = text.trim_start();
    if t.starts_with("Reference ") && t.contains("undefined") {
        Kind::UndefinedRef
    } else if (t.contains("Citation ") && t.contains("undefined")) || t.starts_with("The following entry could not be found") {
        Kind::UndefinedCite
    } else if t.starts_with("Label ") && t.contains("multiply defined") {
        Kind::MultiplyDefined
    } else if t.starts_with("Float too large for page") {
        Kind::FloatTooLarge
    } else if t.starts_with("There were undefined")
        || t.starts_with("There were multiply")
        || t.starts_with("Label(s) may have changed")
        || t.contains("Rerun to get")
        || t.starts_with("Please (re)run")
        || t.starts_with("Please rerun")
    {
        Kind::Summary
    } else if who == "LaTeX Font" {
        Kind::Font
    } else {
        Kind::Other
    }
}

/// FNV-1a：稳定、不依赖进程的哈希，做问题的 id
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/* ---------- 逐行扫 ---------- */

struct Parser<'a> {
    lines: &'a [&'a str],
    out: Vec<Issue>,
    seen: HashSet<(Level, Option<String>, Option<u32>, String)>,
    /// TeX 读文件时打印 `(./ch/one.tex`，读完打印 `)`；跟着括号走就知道此刻在哪个文件里
    stack: Vec<String>,
}

impl<'a> Parser<'a> {
    fn here_file(&self) -> Option<String> {
        self.stack.iter().rev().find(|s| !s.is_empty()).and_then(|s| clean(s))
    }

    fn push(&mut self, i: Issue) {
        if self.seen.insert((i.level, i.file.clone(), i.line, i.message.clone())) {
            self.out.push(i);
        }
    }

    fn issue(level: Level, kind: Kind, message: String, from: usize, to: usize) -> Issue {
        Issue {
            id: String::new(),
            level,
            kind,
            message,
            file: None,
            line: None,
            line_end: None,
            here: None,
            chain: Vec::new(),
            runaway: None,
            source: None,
            key: None,
            pt: None,
            log_range: (from as u32 + 1, to as u32 + 1),
        }
    }

    /// 普通的一行：只看括号，跟踪当前文件
    fn track(&mut self, i: usize) {
        let l = self.lines[i];
        for (k, c) in l.char_indices() {
            match c {
                '(' => {
                    let rest = &l[k + 1..];
                    match open_file(rest) {
                        Some(f) => {
                            let mut path = f.to_string();
                            // 路径正好碰到第 79 列：被折到下一行了，接上
                            if k + 1 + f.len() == l.len() && wrapped(l) {
                                if let Some(next) = self.lines.get(i + 1) {
                                    let end = next.find(|c: char| c.is_whitespace() || c == '(' || c == ')').unwrap_or(next.len());
                                    path.push_str(&next[..end]);
                                }
                            }
                            self.stack.push(path);
                        }
                        // 不是文件的括号（如「(25.0pt too wide)」）也要占位，保证配对
                        None => self.stack.push(String::new()),
                    }
                }
                ')' => {
                    self.stack.pop();
                }
                _ => {}
            }
        }
    }

    /// 最后一遍 TeX 里的一行；返回下一个要看的行
    fn tex_line(&mut self, i: usize) -> usize {
        let l = self.lines[i];
        if let Some((file, line, first)) = head(l) {
            if noise(first) {
                return i + 1;
            }
            return self.error(i, file, line, first);
        }
        if l.contains("dvipdfmx:fatal:") {
            let msg = l.split_once("fatal:").map_or(l, |(_, m)| m).trim().to_string();
            self.push(Self::issue(Level::Error, Kind::Driver, msg, i, i));
            return i + 1;
        }
        if let Some(rest) = l.strip_prefix("Missing character: ") {
            return self.missing_char(i, rest);
        }
        if let Some((who, text)) = warning_head(l) {
            return self.warning(i, who, text);
        }
        if let Some(kind) = box_head(l) {
            return self.boxed(i, kind);
        }
        self.track(i);
        i + 1
    }

    /// 辅助工具段里的一行：只认 biber / bibtex 的报错
    fn tool_line(&mut self, i: usize, tool: &str) {
        let l = self.lines[i];
        if l.contains("dvipdfmx:fatal:") {
            let msg = l.split_once("fatal:").map_or(l, |(_, m)| m).trim().to_string();
            self.push(Self::issue(Level::Error, Kind::Driver, msg, i, i));
            return;
        }
        if !(tool.starts_with("biber") || tool.starts_with("bibtex")) {
            return;
        }
        let bad = l.starts_with("ERROR - ") || l.starts_with("I couldn't open") || l.starts_with("I found no") || l.contains("---this can't happen");
        if bad {
            let mut it = Self::issue(Level::Warning, Kind::Bib, l.trim().trim_start_matches("ERROR - ").to_string(), i, i);
            it.source = Some(tool.to_string());
            self.push(it);
        }
    }

    fn error(&mut self, i: usize, file: Option<&str>, line: Option<u32>, first: &str) -> usize {
        let lines = self.lines;
        // 首行满 79 列就是被折了，接上后面的。LaTeX、宏包报的错（\GenericError）还会自己换行，
        // 续行用空格或 `(包名)` 垫齐：`Unicode character 汉 (U+6C49)` / `   not set up for use with LaTeX.`
        let generic = first.starts_with("LaTeX Error:") || reporter(first).is_some();
        let mut message = first.to_string();
        let mut k = i;
        while k + 1 < lines.len() && !lines[k + 1].trim().is_empty() && !starts_message(lines[k + 1]) {
            let next = lines[k + 1];
            if wrapped(lines[k]) {
                message.push_str(next);
            } else if generic && (next.starts_with(' ') || paren_cont(next)) {
                message.push(' ');
                message.push_str(strip_cont(next));
            } else {
                break;
            }
            k += 1;
        }
        let message = message.trim().trim_start_matches("LaTeX Error: ").trim().to_string();

        // `Runaway argument?` 在错误首行之前，下面一行是参数开头的那段
        let runaway = (i >= 2 && lines[i - 2].starts_with("Runaway ")).then(|| lines[i - 1].trim().to_string()).filter(|s| !s.is_empty());

        // 上下文一层两行：上一行到断点为止，下一行用空格垫到断点再接着打
        let mut chain = Vec::new();
        let mut here = None;
        let mut at_line = None;
        let mut last = k;
        let mut j = k + 1;
        let stop = (k + 1 + CONTEXT_SPAN).min(lines.len());
        while j < stop {
            let l = lines[j];
            if l.starts_with("== ") {
                break;
            }
            if let Some((_, _, m)) = head(l) {
                // `Emergency stop` 跟在后面时，它下面的 `l.<行>` 仍是这个错误的位置
                if noise(m) {
                    j += 1;
                    continue;
                }
                break;
            }
            j += 1;
            let start = !l.starts_with(' ') || l.trim_start().starts_with("...");
            if !start || l.trim().is_empty() {
                continue;
            }
            let t = l.trim_start();
            // 没有 `l.<行>` 的错误（出在 `<*>` 之类），读到收尾的统计就停
            if t.starts_with("Here is how much") || t.starts_with("No pages of output") || t.starts_with("Transcript written") {
                break;
            }
            let after = lines.get(j).filter(|n| n.starts_with(' ') && !n.trim_start().starts_with("...")).map(|n| n.trim());
            if let Some(rest) = t.strip_prefix("l.") {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                if !digits.is_empty() {
                    at_line = digits.parse().ok();
                    let before = rest[digits.len()..].strip_prefix(' ').unwrap_or("").trim_end();
                    here = Some(format!("{before}{HERE}{}", after.unwrap_or("")));
                    last = if after.is_some() { j } else { j - 1 };
                    break;
                }
            }
            let name = frame_name(t);
            // 帮助文字（「See the LaTeX manual…」「Type H <return>…」）不是上下文
            if !(t.starts_with('<') || t.starts_with('\\') || t.starts_with("...")) {
                continue;
            }
            // 往下是诊断编译自己包的那层 `\input`，到此为止
            if name == "<*>" || t.contains("\\@@input") {
                break;
            }
            last = if after.is_some() { j } else { j - 1 };
            if boring(name) || chain.len() >= CHAIN_MAX {
                continue;
            }
            let before = t[name.len()..].trim();
            chain.push(format!("{name} {}{HERE}{}", chars_tail(before, FRAME_BEFORE), chars_head(after.unwrap_or(""), FRAME_AFTER)));
        }

        // 带文件名的错误行给的是项目文件还是发行版文件，一看便知；`! …` 的只能看此刻在读哪个文件
        let (file, line) = match file {
            Some(f) => {
                let f = clean(f);
                let line = if f.is_some() { line.or(at_line) } else { None };
                (f, line)
            }
            None => {
                let f = self.here_file();
                let line = if f.is_some() { at_line } else { None };
                (f, line)
            }
        };
        let kind = classify(&message, file.as_deref(), here.as_deref(), &chain, runaway.is_some());
        let from = if runaway.is_some() { i - 2 } else { i };
        let mut it = Self::issue(Level::Error, kind, message, from, last);
        it.source = reporter(&it.message);
        it.file = file;
        it.line = line;
        it.here = here;
        it.chain = chain;
        it.runaway = runaway;
        self.push(it);
        last + 1
    }

    fn warning(&mut self, i: usize, who: &str, first: &str) -> usize {
        let lines = self.lines;
        // 警告会折行，续行以 `(包名)` 加空格开头，或者直接接着写到空行为止
        let mut text = first.to_string();
        let mut prev = lines[i];
        let mut k = i;
        while k + 1 < lines.len() && !lines[k + 1].trim().is_empty() && !starts_message(lines[k + 1]) {
            k += 1;
            let next = lines[k];
            let cont = paren_cont(next);
            // TeX 把日志硬折在 79 列，满行处是把一个词劈开了，接回去不加空格
            let glue = if !cont && wrapped(prev) { "" } else { " " };
            let body = strip_cont(next);
            text.push_str(glue);
            text.push_str(body);
            prev = next;
        }
        let at = input_line(&text);
        let mut message = match at {
            Some((p, _)) => {
                let tail = text[p..].trim_start_matches(|c: char| !c.is_ascii_digit()).trim_start_matches(|c: char| c.is_ascii_digit());
                format!("{}{}", &text[..p], tail.trim_start_matches('.'))
            }
            None => text.clone(),
        };
        message = message.split_whitespace().collect::<Vec<_>>().join(" ");
        if message.is_empty() {
            return k + 1;
        }
        let kind = warning_kind(who, &message);
        let file = at.and_then(|_| self.here_file());
        let mut it = Self::issue(Level::Warning, kind, message, i, k);
        it.line = at.filter(|_| file.is_some()).map(|(_, n)| n);
        it.file = file;
        it.source = who.strip_prefix("Package ").or_else(|| who.strip_prefix("Class ")).map(|s| s.split(' ').next().unwrap_or(s).to_string());
        match kind {
            // biblatex：`The following entry could not be found in the database: smith2020 Please verify …`
            Kind::UndefinedCite if it.message.starts_with("The following entry") => {
                it.key = it.message.split_once("database:").and_then(|(_, r)| r.split_whitespace().next()).map(str::to_string);
            }
            Kind::UndefinedRef | Kind::UndefinedCite | Kind::MultiplyDefined => {
                let after = ["Reference ", "Citation ", "Label "].iter().find_map(|w| it.message.find(w).map(|p| p + w.len())).unwrap_or(0);
                it.key = quoted(&it.message[after..]);
            }
            Kind::FloatTooLarge => {
                it.pt = it.message["Float too large for page by ".len().min(it.message.len())..].split("pt").next().and_then(|x| x.trim().parse().ok());
            }
            _ => {}
        }
        self.push(it);
        k + 1
    }

    /// `Missing character: There is no 字 (U+5B57) in font [lmroman10-regular]:mapping=tex-text;!`
    fn missing_char(&mut self, i: usize, first: &str) -> usize {
        let mut text = first.to_string();
        let mut k = i;
        while wrapped(self.lines[k]) && k + 1 < self.lines.len() && !self.lines[k + 1].is_empty() && !starts_message(self.lines[k + 1]) {
            k += 1;
            text.push_str(self.lines[k]);
        }
        let rest = text.strip_prefix("There is no ").unwrap_or(&text);
        let (ch, font) = rest.split_once(" in font ").unwrap_or((rest, ""));
        let font = font.trim_end_matches('!').split(':').next().unwrap_or_default().trim_matches(['[', ']']);
        let mut it = Self::issue(Level::Warning, Kind::MissingChar, text.trim().to_string(), i, k);
        it.key = Some(ch.trim().to_string());
        it.source = (!font.is_empty()).then(|| font.to_string());
        it.file = self.here_file();
        self.push(it);
        k + 1
    }

    fn boxed(&mut self, i: usize, kind: Kind) -> usize {
        let l = self.lines[i];
        let message = l
            .split(" in paragraph")
            .next()
            .and_then(|s| s.split(" in alignment").next())
            .and_then(|s| s.split(" detected at").next())
            .unwrap_or(l)
            .trim_end()
            .trim_end_matches("[]")
            .trim_end()
            .to_string();
        let (line, line_end) = match l.find(" at lines ").map(|p| &l[p + " at lines ".len()..]) {
            Some(r) => {
                let (a, b) = r.split_once("--").unwrap_or((r, ""));
                let n = |s: &str| s.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>().ok();
                (n(a), n(b))
            }
            None => (l.find(" at line ").and_then(|p| l[p + " at line ".len()..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()), None),
        };
        // 下面紧跟着盒子的内容（`[]\TU/lmr/m/n/10 …`）：里面的括号不是文件，不跟踪
        let mut k = i;
        while k + 1 < self.lines.len() {
            let n = self.lines[k + 1];
            if n.is_empty() || starts_message(n) || !(n.starts_with("[]") || n.starts_with('\\') || wrapped(self.lines[k])) {
                break;
            }
            k += 1;
        }
        let mut it = Self::issue(Level::Box, kind, message, i, k);
        // 行号属于此刻在读的文件；不知道是哪个文件时也留着，编译校验看改动附近有没有新的溢出要用
        it.file = self.here_file();
        it.line = line;
        it.line_end = line_end.filter(|&e| Some(e) != line);
        it.pt = matches!(kind, Kind::OverfullH | Kind::OverfullV).then(|| points(l)).flatten();
        self.push(it);
        k + 1
    }

    fn finish(self) -> Report {
        let mut out = self.out;
        let count = |l: Level| out.iter().filter(|i| i.level == l).count() as u32;
        let (errors, warnings, boxes) = (count(Level::Error), count(Level::Warning), count(Level::Box));

        // 超出上限的：警告按出现先后留前面的，排版提示留超出最多的
        let mut kept_w = 0;
        out.retain(|i| {
            if i.level != Level::Warning {
                return true;
            }
            kept_w += 1;
            kept_w <= WARNINGS_MAX
        });
        if boxes as usize > BOXES_MAX {
            let mut pts: Vec<f64> = out.iter().filter(|i| i.level == Level::Box).map(|i| i.pt.unwrap_or(0.0)).collect();
            pts.sort_by(|a, b| b.total_cmp(a));
            let floor = pts[BOXES_MAX - 1];
            let mut room = BOXES_MAX;
            out.retain(|i| {
                if i.level != Level::Box {
                    return true;
                }
                let keep = i.pt.unwrap_or(0.0) >= floor && room > 0;
                room -= keep as usize;
                keep
            });
        }

        // 错误、警告、排版提示依次，同级按日志里的先后
        let rank = |l: Level| match l {
            Level::Error => 0,
            Level::Warning => 1,
            Level::Box => 2,
        };
        out.sort_by_key(|i| (rank(i.level), i.log_range.0));
        let mut nth: HashMap<u64, u32> = HashMap::new();
        for i in &mut out {
            let h = fnv(&format!("{:?}|{:?}|{}|{}", i.level, i.kind, i.file.as_deref().unwrap_or(""), i.message));
            let n = nth.entry(h).or_default();
            i.id = format!("{h:016x}-{n}");
            *n += 1;
        }
        Report { issues: out, errors, warnings, boxes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/texlogs")
    }

    /// `testdata/texlogs/` 下每份日志（`.txt`）都配一份 `.json` 期望结果。改了解析规则，
    /// 用 `SUBO_UPDATE_SNAPSHOTS=1 cargo test logparse` 重写期望结果，再逐份看 diff 是否合理
    #[test]
    fn snapshots() {
        let update = std::env::var_os("SUBO_UPDATE_SNAPSHOTS").is_some();
        let mut seen = 0;
        let mut entries: Vec<_> = std::fs::read_dir(fixtures()).unwrap().flatten().map(|e| e.path()).collect();
        entries.sort();
        for log in entries.into_iter().filter(|p| p.extension().is_some_and(|e| e == "txt")) {
            let raw = std::fs::read(&log).unwrap();
            let got = serde_json::to_string_pretty(&parse(&String::from_utf8_lossy(&raw))).unwrap() + "\n";
            let want = log.with_extension("json");
            if update {
                std::fs::write(&want, &got).unwrap();
            } else {
                let expected = std::fs::read_to_string(&want).unwrap_or_else(|_| panic!("{} 没有期望结果，先用 SUBO_UPDATE_SNAPSHOTS=1 生成", log.display()));
                assert_eq!(got, expected.replace("\r\n", "\n"), "{} 的解析结果变了", log.display());
            }
            seen += 1;
        }
        assert!(seen > 0);
    }

    /// testdata/ 下其他测试收的真实日志（缺文件、缺字体、缺 Ghostscript……）：都读得完，错误都认得出
    #[test]
    fn other_real_logs_parse() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for e in std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "txt")) {
            let log = String::from_utf8_lossy(&std::fs::read(e.path()).unwrap()).into_owned();
            let r = parse(&log);
            for i in &r.issues {
                assert!(!i.message.is_empty() && i.log_range.0 <= i.log_range.1, "{}: {i:?}", e.path().display());
            }
        }
    }

    #[test]
    fn only_last_pass_counts_but_bib_errors_count_anywhere() {
        let log = "== pdflatex 第 1 遍\nLaTeX Warning: Reference `old' on page 1 undefined on input line 1.\n\n\
                   == bibtex\nI couldn't open database file refs.bib\n\
                   == pdflatex 第 2 遍\n(./main.tex\nLaTeX Warning: Reference `fig:a' on page 1 undefined on input line 3.\n\n)\n";
        let r = parse(log);
        let keys: Vec<_> = r.of(Kind::UndefinedRef).map(|i| i.key.clone().unwrap()).collect();
        assert_eq!(keys, ["fig:a"]);
        let bib: Vec<_> = r.of(Kind::Bib).map(|i| (i.source.clone().unwrap(), i.message.clone())).collect();
        assert_eq!(bib, [("bibtex".to_string(), "I couldn't open database file refs.bib".to_string())]);
        let w = r.of(Kind::UndefinedRef).next().unwrap();
        assert_eq!((w.file.as_deref(), w.line, w.log_range), (Some("main.tex"), Some(3), (8, 8)));
    }

    #[test]
    fn error_block_with_runaway_and_noise() {
        let log = "Runaway argument?\n{bold \\par Next. \\end {document} \n! File ended while scanning use of \\textbf .\n<inserted text> \n                \\par \n\
                   ./scan.tex:7: Emergency stop.\nl.7 \\end{document}\n\n";
        let r = parse(log);
        assert_eq!(r.errors, 1);
        let e = r.errors().next().unwrap();
        assert_eq!(e.kind, Kind::Scan);
        assert_eq!(e.runaway.as_deref(), Some("{bold \\par Next. \\end {document}"));
        assert_eq!(e.here.as_deref(), Some("\\end{document}⟨here⟩"));
        assert_eq!(e.log_range, (1, 7));
    }

    #[test]
    fn wrapped_project_path_and_bang_error_takes_file_from_stack() {
        let long = format!("(./chapters/{}", "x".repeat(WRAP - "(./chapters/".len()));
        let log = format!("== xelatex 第 1 遍\n{long}\nyz.tex\n! Undefined control sequence.\nl.4 \\foo\n         bar\n");
        let r = parse(&log);
        let e = r.errors().next().unwrap();
        assert_eq!(e.file.as_deref(), Some(format!("chapters/{}yz.tex", "x".repeat(WRAP - "(./chapters/".len())).as_str()));
        assert_eq!((e.line, e.here.as_deref()), (Some(4), Some("\\foo⟨here⟩bar")));
    }

    #[test]
    fn ids_are_stable_across_line_shifts() {
        let a = parse("(./main.tex\nLaTeX Warning: Reference `x' on page 1 undefined on input line 3.\n\n)\n");
        let b = parse("(./main.tex\n\n\nLaTeX Warning: Reference `x' on page 1 undefined on input line 9.\n\n)\n");
        assert_eq!(a.issues[0].id, b.issues[0].id);
        assert_ne!(a.issues[0].line, b.issues[0].line);
    }

    #[test]
    fn boxes_are_capped_by_size() {
        let mut log = String::from("(./main.tex\n");
        for k in 0..(BOXES_MAX + 50) {
            log.push_str(&format!("Overfull \\hbox ({k}.0pt too wide) in paragraph at lines {k}--{k}\n[]\\TU/lmr/m/n/10 a(b|\n\n"));
        }
        let r = parse(&log);
        assert_eq!(r.boxes as usize, BOXES_MAX + 50);
        let kept: Vec<_> = r.issues.iter().filter(|i| i.level == Level::Box).collect();
        assert_eq!(kept.len(), BOXES_MAX);
        assert!(kept.iter().all(|i| i.pt.unwrap() >= 50.0));
        // 盒子内容里的 `(` 没被当成文件，最后一条仍在 main.tex 里
        assert_eq!(kept.last().unwrap().file.as_deref(), Some("main.tex"));
    }

    /// 几百页、满是溢出提示的大日志：解析时间要随日志大小线性增长。不卡绝对时间——
    /// CI 的慢机器（macos-15-intel 跑 debug）比本机慢近十倍，卡死线只会误报；平方级的退化在比例上一眼可见
    #[test]
    fn big_log_scales_linearly() {
        fn log_of(bytes: usize) -> String {
            let mut log = String::from("== xelatex 第 1 遍\n(./main.tex\n");
            let mut k = 0;
            while log.len() < bytes {
                k += 1;
                log.push_str(&format!("Overfull \\hbox ({}.3pt too wide) in paragraph at lines {k}--{k}\n[]\\TU/lmr/m/n/10 Some text that overflows the line|\n\n", k % 90));
                log.push_str(&format!("LaTeX Warning: Reference `fig:{k}' on page 3 undefined on input line {k}.\n\n"));
                log.push_str(&format!("[{k}] (./chapters/two.tex [{k}])\nUnderfull \\hbox (badness 10000) in paragraph at lines {k}--{k}\n\n"));
            }
            log
        }
        // 取三次里最快的一次，少受机器忙闲的干扰
        let time = |log: &str| {
            (0..3)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let r = parse(log);
                    (t.elapsed().as_secs_f64(), r)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .unwrap()
        };
        let (small, _) = time(&log_of(1 << 20));
        let (big, r) = time(&log_of(8 << 20));
        assert!(r.boxes > 10_000 && r.warnings > 10_000);
        assert_eq!(r.issues.len(), BOXES_MAX + WARNINGS_MAX);
        let ratio = big / small;
        eprintln!("1 MB {:.0}ms，8 MB {:.0}ms，比例 {ratio:.1}（线性是 8）", small * 1e3, big * 1e3);
        assert!(ratio < 24.0, "日志大 8 倍，解析慢了 {ratio:.1} 倍：不再是线性的了");
    }
}
