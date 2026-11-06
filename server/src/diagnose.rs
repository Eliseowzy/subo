//! AI 报错修复的诊断材料。给人看的日志不经过这里。
//!
//! TeX 报错的位置常常不是原因所在，而偏差有规律：页脚里出的错报在分页那一行（常常是 `\end{document}`），
//! 少了右括号报在段落或文件结尾，环境没闭合报在 `\end{document}`……所以先从诊断编译的日志里读出
//! 第一处错误和宏调用链，判断是哪一类，再按类别到源码里取证据片段，只把这些交给模型。
//! 有上次编译成功时的源码（lastgood.rs），就把从那以后的改动放在最前面：大部分错误是刚改出来的。
//!
//! 这里也读日志里的排版异常（内容比整页还高、浮动体太大、一行超出版心太多），给编译校验用：
//! 能编译不等于改对了。

use serde::{Deserialize, Serialize};

/// 证据片段合计的上限：行数、字数
const LINES_MAX: usize = 150;
const CHARS_MAX: usize = 12_000;
/// 片段里最多带几条宏定义、几处页面样式设置
const DEFS_MAX: usize = 4;
const STYLE_MAX: usize = 8;
/// 改动最多列几处、合计多少字；每处删掉的、加上的各最多几行
const HUNKS_MAX: usize = 6;
const HUNK_CHARS_MAX: usize = 4_000;
const HUNK_LINES_MAX: usize = 8;
/// 逐行比对的规模上限（去掉首尾相同的行之后，两边行数的乘积）
const DIFF_CELLS_MAX: usize = 4_000_000;
/// 排版异常的门槛（pt）：低于它的是日常的小毛病，不算 AI 改坏的
const PAGE_OVER_PT: f64 = 20.0;
const LINE_OVER_PT: f64 = 30.0;

pub use crate::logparse::{Kind, HERE};
use crate::logparse::{self, Issue, Level, Report};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Diagnosis {
    pub kind: Kind,
    pub message: String,
    /// 项目内相对路径；出在 TeX Live 自己的文件里时为空
    pub file: Option<String>,
    pub line: Option<u32>,
    /// `l.<行>` 那一处，断开处标 ⟨here⟩
    pub here: Option<String>,
    /// 压缩过的宏调用链，最里层在前
    pub chain: Vec<String>,
    /// `Runaway argument?` 下面那行：没闭合的参数从哪开始
    pub runaway: Option<String>,
}

impl From<&Issue> for Diagnosis {
    fn from(i: &Issue) -> Self {
        Diagnosis {
            kind: i.kind,
            message: i.message.clone(),
            file: i.file.clone(),
            line: i.line,
            here: i.here.clone(),
            chain: i.chain.clone(),
            runaway: i.runaway.clone(),
        }
    }
}

/* ---------- 读日志 ---------- */

/// 从日志里读出第一处错误（解析见 logparse.rs）。没有认得的错误行返回 None
pub fn parse(log: &str) -> Option<Diagnosis> {
    logparse::parse(log).errors().next().map(Diagnosis::from)
}

/// 没有诊断编译的结果（超时、没复现）时，拿问题面板里的那条凑一份：没有调用链。
/// `here` 是面板那条的断点处源码（带 ⟨here⟩）；旧版界面给的是 `l.5 前半 后半`，断点在哪看不出来，就标在末尾
pub fn fallback(message: &str, file: &str, line: Option<u32>, here: Option<&str>) -> Diagnosis {
    let here = here.map(|c| {
        let c = c.trim();
        if c.contains(HERE) {
            return c.to_string();
        }
        let c = c.strip_prefix("l.").map(|r| r.trim_start_matches(|ch: char| ch.is_ascii_digit()).trim_start()).unwrap_or(c);
        format!("{c}{HERE}")
    });
    let message = message.trim().to_string();
    let kind = logparse::classify(&message, Some(file), here.as_deref(), &[], false);
    Diagnosis { kind, message, file: Some(file.to_string()), line, here, ..Default::default() }
}

/* ---------- 取证据 ---------- */

/// 源码里的一段，行号从 1 起、含两端
#[derive(Debug, Clone, PartialEq)]
pub struct Excerpt {
    pub from: usize,
    pub to: usize,
    pub why: Vec<&'static str>,
}

/// 设置页面样式、页码、页眉页脚的命令：页面输出时出的错，原因多半在这些地方
const PAGE_STYLE: &[&str] = &[
    "\\pagenumbering", "\\pagestyle", "\\thispagestyle", "\\fancyhead", "\\fancyfoot", "\\fancyhf", "\\fancypagestyle", "\\lhead", "\\chead", "\\rhead",
    "\\lfoot", "\\cfoot", "\\rfoot", "\\thepage", "\\setcounter{page}", "\\markboth", "\\markright", "\\headrule", "\\footrule", "\\@oddfoot", "\\@evenfoot",
    "\\@oddhead", "\\@evenhead", "\\ps@",
];

/// 定义命令的写法；去掉空格、花括号和星号之后，后面紧跟被定义的宏名
const DEFINERS: &[&str] = &[
    "\\newcommand", "\\renewcommand", "\\providecommand", "\\DeclareRobustCommand", "\\NewDocumentCommand", "\\RenewDocumentCommand",
    "\\ProvideDocumentCommand", "\\DeclareDocumentCommand", "\\def", "\\gdef", "\\edef", "\\xdef", "\\let",
];

/// 某个命令在这一行里出现，且不是更长命令名的前缀。以 `@` 结尾的（`\ps@`）按前缀算
fn has_cmd(line: &str, cmd: &str) -> bool {
    let tail_letter = cmd.chars().last().is_some_and(|c| c.is_ascii_alphabetic());
    line.match_indices(cmd).any(|(i, _)| !tail_letter || !line[i + cmd.len()..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '@'))
}

/// 一段文字里的控制序列名（不带反斜杠）
fn cs_names(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        if c != '\\' {
            continue;
        }
        let rest = &s[i + 1..];
        let n = rest.find(|c: char| !(c.is_ascii_alphabetic() || c == '@')).unwrap_or(rest.len());
        if n > 0 {
            let name = rest[..n].to_string();
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// 定义 `\name` 的那一行（行号从 1 起）
fn definition(lines: &[&str], name: &str) -> Option<usize> {
    lines.iter().position(|l| {
        let flat: String = crate::assist::strip_comments(l).chars().filter(|c| !matches!(c, ' ' | '\t' | '{' | '}' | '*')).collect();
        DEFINERS.iter().any(|d| has_cmd(&flat, &format!("{d}\\{name}")))
    }).map(|i| i + 1)
}

/// 到第 `upto` 行为止还开着的环境（名字、开头那一行），外层在前；document 不算
fn open_envs(lines: &[&str], upto: usize) -> Vec<(String, usize)> {
    let mut stack: Vec<(String, usize)> = Vec::new();
    for (i, l) in lines.iter().enumerate().take(upto) {
        let l = crate::assist::strip_comments(l);
        let mut rest = l;
        while let Some(p) = rest.find('\\') {
            rest = &rest[p + 1..];
            let (open, r) = if let Some(r) = rest.strip_prefix("begin{") {
                (true, r)
            } else if let Some(r) = rest.strip_prefix("end{") {
                (false, r)
            } else {
                continue;
            };
            let Some(e) = r.find('}') else { break };
            let name = r[..e].to_string();
            if open {
                stack.push((name, i + 1));
            } else if let Some(k) = stack.iter().rposition(|(n, _)| *n == name) {
                stack.truncate(k);
            }
            rest = &r[e..];
        }
    }
    stack.retain(|(n, _)| n != "document");
    stack
}

fn begin_document(lines: &[&str]) -> Option<usize> {
    lines.iter().position(|l| crate::assist::strip_comments(l).contains("\\begin{document}")).map(|i| i + 1)
}

/// 按错误类别到 `text`（被修的那个文件）里取证据。`file` 是它在项目里的路径：
/// 诊断出的行号只在说的是同一个文件时才用
pub fn excerpts(text: &str, file: &str, d: &Diagnosis) -> Vec<Excerpt> {
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    if n == 0 {
        return Vec::new();
    }
    let at = d.line.filter(|_| d.file.as_deref() == Some(file)).map(|l| (l as usize).clamp(1, n));
    // 按优先级排：前面的先占预算
    let mut want: Vec<(usize, usize, &'static str)> = Vec::new();
    let mut add = |from: isize, to: isize, why: &'static str| {
        let from = from.clamp(1, n as isize) as usize;
        let to = to.clamp(from as isize, n as isize) as usize;
        want.push((from, to, why));
    };

    match d.kind {
        Kind::Output | Kind::End => {
            let mut found = 0;
            for (i, l) in lines.iter().enumerate() {
                let l = crate::assist::strip_comments(l);
                if found < STYLE_MAX && PAGE_STYLE.iter().any(|c| has_cmd(l, c)) {
                    add(i as isize, i as isize + 2, "page style, numbering, header and footer settings");
                    found += 1;
                }
            }
            if found == 0 {
                // 没有显式的设置：交出导言区，连同正文开头几行
                let end = begin_document(&lines).map(|b| b + 3).unwrap_or(40).min(60);
                add(1, end as isize, "preamble and start of the document");
            }
        }
        Kind::Env => {
            for r in refs(&d.message) {
                add(r as isize - 1, r as isize + 1, "the environment was opened here");
            }
        }
        Kind::Scan => {
            if let Some(l) = scan_start(&lines, d, at) {
                add(l as isize - 1, l as isize + 1, "the unclosed argument starts here");
            }
        }
        Kind::Package => {
            let name = d.message.split_whitespace().nth(1).unwrap_or("");
            for (i, l) in lines.iter().enumerate() {
                let l = crate::assist::strip_comments(l);
                let loads = ["\\usepackage", "\\RequirePackage", "\\documentclass"].iter().any(|c| has_cmd(l, c));
                if loads && l.split(|c| matches!(c, '{' | '}' | ',')).any(|p| p.trim() == name) {
                    add(i as isize, i as isize + 2, "where the package is loaded");
                }
            }
        }
        Kind::Aux => {
            // 辅助文件里的那段文字是从源码里写过去的：拿其中够长的词回源码里找
            let words: Vec<&str> = d.here.as_deref().unwrap_or("").split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4).collect();
            let mut hit = 0;
            for (i, l) in lines.iter().enumerate() {
                if hit < 3 && words.iter().any(|w| l.contains(w)) {
                    add(i as isize, i as isize + 2, "source text written to the auxiliary file");
                    hit += 1;
                }
            }
        }
        _ => {}
    }

    // 链上和出错处用到的宏，项目里自己定义的，带上定义
    let mut names: Vec<String> = d.chain.iter().flat_map(|f| cs_names(f.split(HERE).next().unwrap_or(""))).collect();
    if let Some(h) = &d.here {
        names.extend(cs_names(h.split(HERE).next().unwrap_or("")).into_iter().rev().take(3));
    }
    let mut defs = 0;
    let mut seen = Vec::new();
    for name in names {
        if defs >= DEFS_MAX || seen.contains(&name) {
            continue;
        }
        seen.push(name.clone());
        if let Some(l) = definition(&lines, &name) {
            add(l as isize, l as isize + 3, "definition of a macro involved in the error");
            defs += 1;
        }
    }

    if let Some(l) = at {
        let l = l as isize;
        match d.kind {
            // 这几类停下的地方只是露馅处，给一两行交代清楚就够了
            Kind::Output | Kind::End | Kind::Env => add(l - 1, l, "where TeX stopped"),
            _ => add(l - 4, l + 2, "where TeX stopped"),
        }
        // 出错处所在的环境，从开头那行起（表格、列表的报错常常要看列格式、开头参数）
        for (_, b) in open_envs(&lines, l as usize).iter().rev().take(2) {
            add(*b as isize, *b as isize + 1, "the environment around the error starts here");
        }
    }

    budget(&lines, want)
}

/// 消息里提到的其他行：`\begin{itemize} on input line 3 ended by …`
fn refs(message: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = message;
    while let Some(p) = rest.find("on input line ") {
        rest = &rest[p + "on input line ".len()..];
        let d: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(v) = d.parse() {
            out.push(v);
        }
    }
    out
}

/// 没闭合的参数从哪一行开始：消息里点了名的宏（`before \pagenumbering`、`use of \textbf`），
/// 出错行之前最后一处、后面接的文字和 `Runaway argument?` 打出来的开头对得上的那个
fn scan_start(lines: &[&str], d: &Diagnosis, at: Option<usize>) -> Option<usize> {
    let m = &d.message;
    let p = m.find(" before \\").map(|p| p + " before ".len()).or_else(|| m.find(" use of \\").map(|p| p + " use of ".len()))?;
    let name = format!("\\{}", cs_names(&m[p..]).into_iter().next()?);
    let upto = at.unwrap_or(lines.len());
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let start: String = d.runaway.as_deref().map(|r| squash(r).chars().take(10).collect()).unwrap_or_default();
    let cands: Vec<usize> = (0..upto).filter(|&i| has_cmd(crate::assist::strip_comments(lines[i]), &name)).collect();
    let fits = |i: &usize| {
        let l = lines[*i];
        let after = l.find(name.as_str()).map(|p| squash(&l[p + name.len()..])).unwrap_or_default();
        !start.is_empty() && (after.starts_with(&start) || (!after.is_empty() && start.starts_with(&after)))
    };
    cands.iter().rev().find(|i| fits(i)).or(cands.last()).map(|i| i + 1)
}

/// 按优先级收进预算，再按行号合并相邻、重叠的片段
fn budget(lines: &[&str], want: Vec<(usize, usize, &'static str)>) -> Vec<Excerpt> {
    let mut taken: Vec<Excerpt> = Vec::new();
    let (mut nlines, mut nchars) = (0, 0);
    for (from, to, why) in want {
        let fresh: Vec<usize> = (from..=to).filter(|l| !taken.iter().any(|e| (e.from..=e.to).contains(l))).collect();
        let cost: usize = fresh.iter().map(|&l| lines[l - 1].chars().count() + 8).sum();
        if nlines + fresh.len() > LINES_MAX || nchars + cost > CHARS_MAX {
            continue;
        }
        nlines += fresh.len();
        nchars += cost;
        taken.push(Excerpt { from, to, why: vec![why] });
    }
    taken.sort_by_key(|e| e.from);
    let mut out: Vec<Excerpt> = Vec::new();
    for e in taken {
        if let Some(last) = out.last_mut().filter(|l| e.from <= l.to + 1) {
            last.to = last.to.max(e.to);
            for w in e.why {
                if !last.why.contains(&w) {
                    last.why.push(w);
                }
            }
            continue;
        }
        out.push(e);
    }
    out
}

/* ---------- 上次编译成功以来的改动 ---------- */

/// 一处改动。`removed` 是上次能编译时有、现在没了的行；`added` 是现在文件里新出来的行（行号从 1 起、含两端，
/// 纯删除时 `to < from`，`from` 指删除处后面那一行）
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub removed: Vec<String>,
    pub from: usize,
    pub to: usize,
    /// 这处改动让花括号或 \begin/\end 不再配平
    pub unbalanced: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Changes {
    /// 和上次编译成功时一模一样
    Unchanged,
    /// 改得太多，逐行比对不划算
    TooMany,
    Hunks(Vec<Hunk>),
}

/// 一段源码的「开减闭」：花括号、环境
fn balance(lines: &[&str]) -> (i64, i64) {
    let mut braces = 0;
    let mut envs = 0;
    for l in lines {
        let l = crate::assist::strip_comments(l);
        let mut prev = ' ';
        for c in l.chars() {
            if prev != '\\' {
                match c {
                    '{' => braces += 1,
                    '}' => braces -= 1,
                    _ => {}
                }
            }
            prev = if prev == '\\' && c == '\\' { ' ' } else { c };
        }
        envs += l.matches("\\begin{").count() as i64 - l.matches("\\end{").count() as i64;
    }
    (braces, envs)
}

/// 逐行比对 `base`（上次能编译时）和 `text`（现在）
pub fn diff(base: &str, text: &str) -> Changes {
    let a: Vec<&str> = base.lines().collect();
    let b: Vec<&str> = text.lines().collect();
    if a == b {
        return Changes::Unchanged;
    }
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let (m, n) = (am.len(), bm.len());
    if (m + 1) * (n + 1) > DIFF_CELLS_MAX {
        return Changes::TooMany;
    }
    // 最长公共子序列，从后往前填，回溯时从前往后走
    let w = n + 1;
    let mut t = vec![0u32; (m + 1) * w];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            t[i * w + j] = if am[i] == bm[j] { t[(i + 1) * w + j + 1] + 1 } else { t[(i + 1) * w + j].max(t[i * w + j + 1]) };
        }
    }
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < m || j < n {
        if i < m && j < n && am[i] == bm[j] {
            i += 1;
            j += 1;
            continue;
        }
        let (i0, j0) = (i, j);
        while (i < m || j < n) && !(i < m && j < n && am[i] == bm[j]) {
            if j >= n || (i < m && t[(i + 1) * w + j] >= t[i * w + j + 1]) {
                i += 1;
            } else {
                j += 1;
            }
        }
        let removed: Vec<&str> = am[i0..i].to_vec();
        let added: Vec<&str> = bm[j0..j].to_vec();
        hunks.push(Hunk {
            removed: removed.iter().map(|l| l.to_string()).collect(),
            from: pre + j0 + 1,
            to: pre + j,
            unbalanced: balance(&removed) != balance(&added),
        });
    }
    Changes::Hunks(hunks)
}

/// 挑出要给模型看的几处：配不平的在前，再按离出错行的远近、改动的大小
fn pick(hunks: &[Hunk], at: Option<usize>) -> Vec<&Hunk> {
    let mut v: Vec<&Hunk> = hunks.iter().collect();
    v.sort_by_key(|h| (!h.unbalanced, at.map(|a| a.abs_diff(h.from)).unwrap_or(0), h.removed.len() + (h.to + 1).saturating_sub(h.from)));
    let mut out = Vec::new();
    let mut chars = 0;
    for h in v {
        let c: usize = h.removed.iter().map(|l| l.chars().count()).sum::<usize>() + 200;
        if out.len() >= HUNKS_MAX || chars + c > HUNK_CHARS_MAX {
            continue;
        }
        chars += c;
        out.push(h);
    }
    out.sort_by_key(|h| h.from);
    out
}

fn render_changes(s: &mut String, lines: &[&str], c: &Changes, at: Option<usize>) {
    s.push_str("<CHANGES since=\"the last successful compile\">\n");
    match c {
        Changes::Unchanged => s.push_str("None: this file is unchanged since the document last compiled without errors, so the cause is probably not in this file's recent edits.\n"),
        Changes::TooMany => s.push_str("This file changed too much since the last successful compile to list the changes.\n"),
        Changes::Hunks(all) => {
            let shown = pick(all, at);
            s.push_str("These edits were made after the document last compiled without errors; a mistake introduced by them is the most likely cause.\n");
            s.push_str("\"-\" lines were removed and are no longer in the file. \"+\" lines and unmarked context lines are in the current file, prefixed with their line number.\n");
            if shown.len() < all.len() {
                s.push_str(&format!("Showing {} of {} changes.\n", shown.len(), all.len()));
            }
            let row = |s: &mut String, mark: char, l: usize| s.push_str(&format!("{mark}{l:>5} | {}\n", lines[l - 1]));
            for h in shown {
                let note = if h.unbalanced { " (this change leaves braces or \\begin/\\end unbalanced)" } else { "" };
                s.push_str(&format!("--- change at line {}{note}\n", h.from));
                if h.from > 1 {
                    row(s, ' ', h.from - 1);
                }
                for (k, r) in h.removed.iter().enumerate() {
                    if k == HUNK_LINES_MAX {
                        s.push_str(&format!("-      | … ({} more removed lines)\n", h.removed.len() - k));
                        break;
                    }
                    s.push_str(&format!("-      | {r}\n"));
                }
                for (k, l) in (h.from..=h.to).enumerate() {
                    if k == HUNK_LINES_MAX {
                        s.push_str(&format!("+      | … ({} more added lines)\n", h.to + 1 - l));
                        break;
                    }
                    row(s, '+', l);
                }
                let next = h.to.max(h.from - 1) + 1;
                if next <= lines.len() {
                    row(s, ' ', next);
                }
            }
        }
    }
    s.push_str("</CHANGES>\n\n");
}

/* ---------- 排版异常 ---------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LayoutKind {
    /// 组装页面时内容比整页还高：有一块东西放不进任何一页
    PageOverflow,
    /// 浮动体比页面还大
    FloatTooLarge,
    /// 一行超出版心很多
    LineOverflow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub kind: LayoutKind,
    /// 超出多少
    pub pt: f64,
    /// 日志给出的源码行（只有超宽的行有）
    pub line: Option<u32>,
}

/// 日志（最后一遍）里够得上门槛的排版异常
pub fn layout(log: &str) -> Vec<Layout> {
    layout_of(&logparse::parse(log))
}

/// 同上，用已经解析好的结果；按日志里的先后
pub fn layout_of(r: &Report) -> Vec<Layout> {
    let mut found: Vec<(u32, Layout)> = Vec::new();
    for i in r.issues.iter().filter(|i| i.level != Level::Error) {
        let Some(pt) = i.pt else { continue };
        let kind = match i.kind {
            Kind::OverfullV if i.message.contains("\\output is active") && pt > PAGE_OVER_PT => LayoutKind::PageOverflow,
            Kind::OverfullH if pt > LINE_OVER_PT => LayoutKind::LineOverflow,
            Kind::FloatTooLarge => LayoutKind::FloatTooLarge,
            _ => continue,
        };
        let line = (kind == LayoutKind::LineOverflow).then_some(i.line).flatten();
        found.push((i.log_range.0, Layout { kind, pt, line }));
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, l)| l).collect()
}

/// `after` 里新冒出来的：同类、超出量差不到半个 pt 的算原来就有的（每条只抵一次）
pub fn fresh(after: Vec<Layout>, base: &[Layout]) -> Vec<Layout> {
    let mut left: Vec<&Layout> = base.iter().collect();
    after
        .into_iter()
        .filter(|a| match left.iter().position(|b| b.kind == a.kind && (b.pt - a.pt).abs() < 0.5) {
            Some(k) => {
                left.remove(k);
                false
            }
            None => true,
        })
        .collect()
}

/* ---------- 交给模型 ---------- */

fn meaning(d: &Diagnosis) -> String {
    match d.kind {
        Kind::Output => {
            let part = if d.chain.iter().any(|f| f.contains("\\@thefoot") || f.contains("foot")) {
                "the footer of a page"
            } else if d.chain.iter().any(|f| f.contains("\\@thehead") || f.contains("head")) {
                "the header of a page"
            } else {
                "a page (header, footer or floats)"
            };
            format!(
                "TeX failed while assembling {part}. The reported line is only where that page happened to be output, not where the mistake is. \
                 Look at the page style, page numbering and header/footer settings (and any macro they use)."
            )
        }
        Kind::End => "TeX only noticed the problem at \\end{document}, where the last page is output; the cause is earlier, \
                      often in a setting that only takes effect when a page is output (page numbering, page style, headers and footers)."
            .into(),
        Kind::Env => "An environment was opened and never closed, or was closed with a different name. Fix the opening/closing pair.".into(),
        Kind::Scan => "An argument was never closed: TeX kept reading until a blank line or the end of the file. \
                       The mistake is where that argument starts (usually a missing closing brace), not where TeX stopped."
            .into(),
        Kind::Package => "A package or class raised this error; it usually comes from how it is loaded or used (options, load order, arguments).".into(),
        Kind::Aux => "The error is in an auxiliary file written by the previous run. The cause is the source text that was written there \
                      (a \\label, a heading, a caption or a citation key); fix that text in the source."
            .into(),
        Kind::Driver => "The PDF driver (xdvipdfmx) failed after TeX finished; usually a font or image file it needs is missing or broken.".into(),
        _ => "TeX stopped at the marked point; the mistake is there or shortly before it.".into(),
    }
}

/// 诊断材料的正文：错误、它意味着什么、调用链、上次编译成功以来的改动、带行号的源码片段
pub fn render(text: &str, file: &str, d: &Diagnosis, changes: Option<&Changes>, ex: &[Excerpt]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut s = String::new();
    s.push_str("<DIAGNOSIS>\n");
    s.push_str(&format!("Error: {}\n", d.message));
    match (&d.file, d.line) {
        (Some(f), Some(l)) => s.push_str(&format!("Reported at: {f} line {l}\n")),
        (Some(f), None) => s.push_str(&format!("Reported in: {f}\n")),
        _ => {}
    }
    if let Some(h) = &d.here {
        s.push_str(&format!("TeX stopped at {HERE}: {}\n", h.trim()));
    }
    if let Some(r) = &d.runaway {
        s.push_str(&format!("The unclosed argument begins with: {r}\n"));
    }
    if !d.chain.is_empty() {
        s.push_str("Macro chain at the error, innermost first:\n");
        for f in &d.chain {
            s.push_str(&format!("  {f}\n"));
        }
    }
    s.push_str(&format!("What this means: {}\n", meaning(d)));
    s.push_str("</DIAGNOSIS>\n\n");
    if let Some(c) = changes {
        let at = d.line.filter(|_| d.file.as_deref() == Some(file)).map(|l| l as usize);
        render_changes(&mut s, &lines, c, at);
    }
    s.push_str(&format!("<EXCERPTS file=\"{file}\">\n"));
    s.push_str("Each line is prefixed with its line number and \" | \"; the prefix is not part of the file.\n");
    for e in ex {
        s.push_str(&format!("--- lines {}-{}: {}\n", e.from, e.to, e.why.join("; ")));
        for l in e.from..=e.to {
            s.push_str(&format!("{l:>5} | {}\n", lines[l - 1]));
        }
    }
    s.push_str("</EXCERPTS>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一页文档里 `\pagenumbering{}`：页脚出错，报在 `\end{document}`（诊断编译的真实日志，删了无关行）
    const OUTPUT_LOG: &str = r"== xelatex 第 1 遍
(/x/ot1phv.fd)
./out.tex:7: Missing number, treated as zero.
<to be read again>
                   \hfil
<argument> ...color \hb@xt@ \textwidth {\@thefoot
                                                  }\color@endbox
\use_ii:nn #1#2->#2

\@outputpage ...twidth {\@thefoot }\color@endbox }
                                                   \pdfannot@link@on@@ } } \...

\@opcol ...te@singlecol@structures@@ \@outputpage
                                                  \fi \global \@mparbottom \...
<output> ...specialoutput \else \@makecol \@opcol
                                                  \@startcolumn \@whilesw \i...

\newpage ...erypar {}\fi \par \vfil \penalty -\@M

\clearpage ...opskip \hbox {}\fi \fi \fi \newpage
                                                  \@@write \m@ne {}\vbox {}\...

\enddocument ...t \@checkend {document}\clearpage
                                                  \@kernel@before@enddocumen...

\end  ...ook {env/#1/end}}\csname end#1\endcsname
                                                  \@checkend {#1}\endgroup \...
l.7 \end{document}

No pages of output.
";

    const OUT_TEX: &str = "\\documentclass{article}\n\\begin{document}\n\\pagenumbering{}\n\nHello.\n\n\\end{document}\n";

    #[test]
    fn output_routine_error_points_at_page_style() {
        let d = parse(OUTPUT_LOG).unwrap();
        assert_eq!(d.kind, Kind::Output);
        assert_eq!(d.message, "Missing number, treated as zero.");
        assert_eq!((d.file.as_deref(), d.line), (Some("out.tex"), Some(7)));
        assert_eq!(d.here.as_deref(), Some("\\end{document}⟨here⟩"));
        assert!(d.chain[0].starts_with("<to be read again> ⟨here⟩\\hfil"), "{:?}", d.chain);
        assert!(d.chain.iter().any(|f| f.contains("\\@thefoot")));
        // expl3 的内部层不要
        assert!(!d.chain.iter().any(|f| f.contains("use_ii")));
        let ex = excerpts(OUT_TEX, "out.tex", &d);
        // 第 3 行（\pagenumbering{}）进了片段，导言区的其他东西没进
        assert!(ex.iter().any(|e| (e.from..=e.to).contains(&3)), "{ex:?}");
        assert!(ex.iter().any(|e| (e.from..=e.to).contains(&7)));
        assert!(!ex.iter().any(|e| e.from == 1));
        let r = render(OUT_TEX, "out.tex", &d, None, &ex);
        assert!(r.contains("    3 | \\pagenumbering{}") && r.contains("footer"), "{r}");
    }

    #[test]
    fn unclosed_argument_points_at_its_start() {
        let log = "Runaway argument?\n{bold \\par Next. \\end {document} \n! File ended while scanning use of \\textbf .\n<inserted text> \n                \\par \n<argument> ...entFile /before}\\@@input \"scan.tex\" \n                                                  \n<*> \\errorcontextlines=99\\relax\\input{scan.tex}\n";
        let d = parse(log).unwrap();
        assert_eq!(d.kind, Kind::Scan);
        assert_eq!(d.runaway.as_deref(), Some("{bold \\par Next. \\end {document}"));
        // 诊断编译自己包的 \input 那几层不进链
        assert!(d.chain.iter().all(|f| !f.contains("@@input") && !f.starts_with("<*>")), "{:?}", d.chain);
        let tex = "\\documentclass{article}\n\\begin{document}\n\\textbf{ok} and\n\\textbf{bold\n\nNext.\n\\end{document}\n";
        let ex = excerpts(tex, "scan.tex", &d);
        assert!(ex.iter().any(|e| (e.from..=e.to).contains(&4)), "{ex:?}");
    }

    #[test]
    fn environment_mismatch_brings_both_ends() {
        let log = "./env.tex:6: LaTeX Error: \\begin{itemize} on input line 3 ended by \\end{documen\nt}.\n\nSee the LaTeX manual or LaTeX Companion for explanation.\nType  H <return>  for immediate help.\n ...                                              \n                                                  \n\\GenericError  ...                                \n                                                  \\endgroup \n\\@checkend ...ed@a \\@currenvir \\else \\@badend {#1}\n                                                  \\fi \nl.6 \\end{document}\n                  \n";
        let d = parse(log).unwrap();
        assert_eq!(d.message, "\\begin{itemize} on input line 3 ended by \\end{document}.");
        assert_eq!(d.kind, Kind::Env);
        assert!(d.chain.iter().all(|f| !f.starts_with("\\GenericError")));
        let tex = "\\documentclass{article}\n\\begin{document}\n\\begin{itemize}\n\\item a\n\n\\end{document}\n";
        let ex = excerpts(tex, "env.tex", &d);
        assert!(ex.iter().any(|e| (e.from..=e.to).contains(&3)) && ex.iter().any(|e| (e.from..=e.to).contains(&6)), "{ex:?}");
    }

    #[test]
    fn user_macro_definition_comes_along() {
        let log = "./user.tex:40: Undefined control sequence.\n<recently read> \\undefinedthing \n                                \nl.40 Hi \\mybad{x}\n                .\n";
        let d = parse(log).unwrap();
        assert_eq!(d.kind, Kind::Here);
        assert_eq!(d.here.as_deref(), Some("Hi \\mybad{x}⟨here⟩."));
        let mut tex = String::from("\\documentclass{article}\n\\newcommand{\\mybad}[1]{\\textbf{#1}\\undefinedthing}\n\\begin{document}\n");
        for _ in 0..36 {
            tex.push_str("filler\n");
        }
        tex.push_str("Hi \\mybad{x}.\n\\end{document}\n");
        let ex = excerpts(&tex, "user.tex", &d);
        assert!(ex.iter().any(|e| e.from == 2), "{ex:?}");
        assert!(ex.iter().any(|e| (e.from..=e.to).contains(&40)));
        // 中间的填充行不浪费
        assert!(!ex.iter().any(|e| (e.from..=e.to).contains(&20)));
    }

    #[test]
    fn fallback_without_chain_still_suspects_end_document() {
        let d = fallback("Missing number, treated as zero.", "resume.tex", Some(79), Some("l.79 \\end{document}"));
        assert_eq!(d.kind, Kind::End);
        let ex = excerpts(OUT_TEX, "resume.tex", &Diagnosis { line: Some(7), ..d });
        assert!(ex.iter().any(|e| (e.from..=e.to).contains(&3)));
    }

    #[test]
    fn package_error_brings_the_loading_line() {
        let d = fallback("Package xcolor Error: Undefined color `foo'.", "a.tex", Some(9), Some("l.9 \\color{foo}"));
        assert_eq!(d.kind, Kind::Package);
        let tex = "\\documentclass{article}\n\\usepackage{amsmath}\n\\usepackage[dvipsnames]{xcolor}\n\\begin{document}\n\n\n\n\n\\color{foo}\n\\end{document}\n";
        let ex = excerpts(tex, "a.tex", &d);
        assert!(ex.iter().any(|e| (e.from..=e.to).contains(&3)), "{ex:?}");
    }

    /// 删掉第一个 \end{minipage}：报在文末，改动指着第 4 行原处
    #[test]
    fn removed_end_shows_up_as_unbalanced_change() {
        let base = "\\begin{document}\n\\begin{minipage}{1cm}\n  name\n\\end{minipage}\n\n\\begin{minipage}{1cm}\n  job\n\\end{minipage}\nbody\n\\end{document}\n";
        let text = base.replacen("\\end{minipage}\n", "", 1);
        let Changes::Hunks(h) = diff(base, &text) else { panic!() };
        assert_eq!(h, vec![Hunk { removed: vec!["\\end{minipage}".into()], from: 4, to: 3, unbalanced: true }]);
        let d = Diagnosis {
            kind: Kind::Env,
            message: "\\begin{minipage} on input line 2 ended by \\end{document}.".into(),
            file: Some("a.tex".into()),
            line: Some(9),
            ..Default::default()
        };
        let r = render(&text, "a.tex", &d, Some(&diff(base, &text)), &excerpts(&text, "a.tex", &d));
        assert!(r.contains("--- change at line 4 (this change leaves braces"), "{r}");
        assert!(r.contains("     3 |   name\n-      | \\end{minipage}\n     4 | \n"), "{r}");
        assert_eq!(diff(base, base), Changes::Unchanged);
    }

    #[test]
    fn diff_keeps_line_numbers_of_the_current_file() {
        let base = "a\nb\nc\nd\ne\n";
        let Changes::Hunks(h) = diff(base, "a\nB\nc\nd\nx\ny\ne\n") else { panic!() };
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].removed.clone(), h[0].from, h[0].to), (vec!["b".to_string()], 2, 2));
        assert_eq!((h[1].removed.len(), h[1].from, h[1].to), (0, 5, 6));
        assert!(!h[0].unbalanced);
        // 只删了一个字的那行：花括号没动，不算配不平
        let Changes::Hunks(h) = diff("\\pagenumbering{gobble}\n", "\\pagenumbering{}\n") else { panic!() };
        assert!(!h[0].unbalanced && h[0].removed == ["\\pagenumbering{gobble}"]);
    }

    #[test]
    fn layout_signals_and_what_is_new() {
        let log = "== xelatex 第 1 遍\nOverfull \\vbox (999.0pt too high) has occurred while \\output is active []\n\
                   == xelatex 第 2 遍\nOverfull \\hbox (3.1pt too wide) in paragraph at lines 5--5\n\
                   Overfull \\hbox (45.5pt too wide) in paragraph at lines 64--64\n\
                   Overfull \\vbox (689.11063pt too high) has occurred while \\output is active []\n\
                   Overfull \\vbox (5.0pt too high) has occurred while \\output is active []\n\
                   LaTeX Warning: Float too large for page by 12.5pt on input line 40.\n";
        let l = layout(log);
        assert_eq!(
            l,
            vec![
                Layout { kind: LayoutKind::LineOverflow, pt: 45.5, line: Some(64) },
                Layout { kind: LayoutKind::PageOverflow, pt: 689.11063, line: None },
                Layout { kind: LayoutKind::FloatTooLarge, pt: 12.5, line: None },
            ]
        );
        // 原来就有的那条超宽不算新的
        let base = vec![Layout { kind: LayoutKind::LineOverflow, pt: 45.4, line: Some(60) }];
        let f = fresh(l, &base);
        assert_eq!(f.iter().map(|x| x.kind).collect::<Vec<_>>(), vec![LayoutKind::PageOverflow, LayoutKind::FloatTooLarge]);
    }

    #[test]
    fn env_scan_ignores_comments() {
        let lines = ["\\begin{table}", "% \\end{table}", "\\begin{tabular}{ll}", "a & b"];
        assert_eq!(open_envs(&lines, 4), vec![("table".to_string(), 1), ("tabular".to_string(), 3)]);
    }

    #[test]
    fn miktex_error_with_drive_letter_and_crlf() {
        let log = "== xelatex 第 1 遍\r\n(main.tex\r\nLaTeX2e <2021-11-15>\r\n (E:\\MikTeX\\tex/latex/ctex\\ctexart.cls\r\n\r\nE:\\MikTeX\\tex/latex/ctex\\ctexart.cls:43: Class ctexart Error: Support package `\r\nexpl3' too old.\r\n\r\nFor immediate help type H <return>.\r\n ...                                              \r\n                                                  \r\nl.43 ...or:nnn { ctex } { l3-too-old } { expl3 } }\r\n                                                  \r\nNo pages of output.\r\n";
        let d = parse(log).unwrap();
        assert_eq!(d.message, "Class ctexart Error: Support package `expl3' too old.");
        assert_eq!((d.file.as_deref(), d.line), (None, None));
    }
}
