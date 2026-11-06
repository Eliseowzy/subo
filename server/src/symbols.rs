//! 项目里的符号：文档大纲、`\label`、`.bib` 条目。给编辑器的大纲面板和 `\ref` / `\cite` 补全用。
//!
//! 大纲从入口文件起顺着 `\input`、`\include`、`\subfile`、`\import` 按文档顺序读，
//! 跨文件的章节也排得对；标签收全项目所有 `.tex`（没被入口引用到的章节文件里的也算），
//! 文献收全项目所有 `.bib`。每次几毫秒到几十毫秒，前端在保存后、编译后重取。

use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// 单个文件超过这个大小就不读：多半是数据，不是手写的源码
const MAX_BYTES: u64 = 4 << 20;
/// 各类最多给这么多条
const MAX_ITEMS: usize = 5000;
/// 顺着 `\input` 最多读这么多个文件
const MAX_FILES: usize = 300;

#[derive(Debug, Serialize, PartialEq)]
pub struct Heading {
    /// 0 `\part` … 5 `\subparagraph`；beamer 的 frame 标题是 6
    pub level: u8,
    pub title: String,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Label {
    pub key: String,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct BibEntry {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Serialize, Default)]
pub struct Symbols {
    pub outline: Vec<Heading>,
    pub labels: Vec<Label>,
    pub bib: Vec<BibEntry>,
}

const SECTIONS: &[(&str, u8)] = &[
    ("part", 0),
    ("chapter", 1),
    ("section", 2),
    ("subsection", 3),
    ("subsubsection", 4),
    ("paragraph", 5),
    ("frametitle", 6),
];

/// 整个项目的符号。`files` 是项目文件的相对路径（项目文件树）
pub fn collect(root: &Path, entry: Option<&str>, files: &[String]) -> Symbols {
    let mut out = Symbols::default();
    if let Some(entry) = entry {
        let mut seen = HashSet::new();
        walk(root, entry, &mut seen, &mut out.outline, 0);
    }
    for f in files.iter().filter(|f| f.ends_with(".tex")) {
        if let Some(text) = read(&root.join(f)) {
            labels(&text, f, &mut out.labels);
        }
        if out.labels.len() >= MAX_ITEMS {
            break;
        }
    }
    for f in files.iter().filter(|f| f.to_ascii_lowercase().ends_with(".bib")) {
        if let Some(text) = read(&root.join(f)) {
            bib(&text, f, &mut out.bib);
        }
        if out.bib.len() >= MAX_ITEMS {
            break;
        }
    }
    out.outline.truncate(MAX_ITEMS);
    out.labels.truncate(MAX_ITEMS);
    out.bib.truncate(MAX_ITEMS);
    out
}

fn read(p: &Path) -> Option<String> {
    if fs::metadata(p).ok()?.len() > MAX_BYTES {
        return None;
    }
    fs::read_to_string(p).ok()
}

/// 1 起的行号
fn line_of(text: &str, pos: usize) -> usize {
    text[..pos].bytes().filter(|&b| b == b'\n').count() + 1
}

/// `{…}` 的内容（`pos` 指着 `{`），括号要配平；返回内容和右括号后的位置
fn group(text: &str, pos: usize) -> Option<(&str, usize)> {
    let b = text.as_bytes();
    if b.get(pos) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut esc = false;
    for (k, &c) in b.iter().enumerate().skip(pos) {
        if esc {
            esc = false;
            continue;
        }
        match c {
            b'\\' => esc = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[pos + 1..k], k + 1));
                }
            }
            _ => {}
        }
    }
    None
}

fn skip_ws(b: &[u8], mut j: usize) -> usize {
    while b.get(j).is_some_and(u8::is_ascii_whitespace) {
        j += 1;
    }
    j
}

/// 跳过可选参数 `[…]`（不嵌套方括号的常见写法）
fn skip_opt(text: &str, mut j: usize) -> usize {
    let b = text.as_bytes();
    j = skip_ws(b, j);
    if b.get(j) == Some(&b'[') {
        if let Some(end) = text[j..].find(']') {
            return skip_ws(b, j + end + 1);
        }
    }
    j
}

/// 标题里的排版命令去掉，留下看得懂的字：`\textbf{A}~B\\C` → `A B C`
fn clean(title: &str) -> String {
    let mut out = String::new();
    let mut chars = title.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let mut name = String::new();
                while let Some(&n) = chars.peek().filter(|n| n.is_ascii_alphabetic()) {
                    name.push(n);
                    chars.next();
                }
                if name.is_empty() {
                    // `\\`、`\&` 这类：换行算空格，转义字符留下字符本身
                    if let Some(n) = chars.next() {
                        out.push(if n == '\\' { ' ' } else { n });
                    }
                } else if matches!(name.as_str(), "LaTeX" | "TeX") {
                    out.push_str(&name);
                }
            }
            '{' | '}' | '$' => {}
            '~' => out.push(' '),
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `\input{chap1}` 的参数换成项目里的文件：没写扩展名补 `.tex`
fn include_target(root: &Path, base: &Path, arg: &str) -> Option<String> {
    let arg = arg.trim();
    if arg.is_empty() || arg.contains("..") {
        return None;
    }
    let cands = [arg.to_string(), format!("{arg}.tex")];
    for c in cands {
        let p = base.join(&c);
        if p.is_file() && p.starts_with(root) {
            return Some(p.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/"));
        }
    }
    None
}

fn walk(root: &Path, rel: &str, seen: &mut HashSet<String>, out: &mut Vec<Heading>, depth: usize) {
    if depth > 12 || seen.len() >= MAX_FILES || !seen.insert(rel.to_string()) || out.len() >= MAX_ITEMS {
        return;
    }
    let Some(raw) = read(&root.join(rel)) else { return };
    let text = crate::prescan::strip_comments(&raw);
    let b = text.as_bytes();
    let dir: PathBuf = Path::new(rel).parent().map(|d| root.join(d)).unwrap_or_else(|| root.to_path_buf());
    let mut i = 0;
    while let Some(off) = text[i..].find('\\') {
        let start = i + off;
        let name_end = start + 1 + text[start + 1..].bytes().take_while(u8::is_ascii_alphabetic).count();
        i = name_end.max(start + 1);
        let name = &text[start + 1..name_end];
        if let Some(&(_, level)) = SECTIONS.iter().find(|(n, _)| *n == name) {
            let mut j = name_end;
            if b.get(j) == Some(&b'*') {
                j += 1;
            }
            let j = skip_opt(&text, j);
            if let Some((title, end)) = group(&text, j) {
                let title = clean(title);
                if !title.is_empty() {
                    out.push(Heading { level, title, file: rel.to_string(), line: line_of(&text, start) });
                }
                i = end;
            }
            continue;
        }
        // beamer：`\begin{frame}{标题}`、`\begin{frame}[opt]{标题}`
        if name == "begin" {
            if let Some((env, end)) = group(&text, skip_ws(b, name_end)) {
                if env == "frame" {
                    let j = skip_opt(&text, end);
                    if let Some((title, after)) = group(&text, j) {
                        let title = clean(title);
                        if !title.is_empty() {
                            out.push(Heading { level: 6, title, file: rel.to_string(), line: line_of(&text, start) });
                        }
                        i = after;
                        continue;
                    }
                }
                i = end;
            }
            continue;
        }
        let target = match name {
            "input" | "include" | "subfile" | "subfileinclude" => {
                group(&text, skip_ws(b, name_end)).and_then(|(arg, _)| include_target(root, &dir, arg))
            }
            // `\import{dir/}{file}`：路径相对项目根；`\subimport` 相对当前文件
            "import" | "subimport" => group(&text, skip_ws(b, name_end)).and_then(|(d, end)| {
                let (f, _) = group(&text, skip_ws(b, end))?;
                let base = if name == "import" { root.to_path_buf() } else { dir.clone() };
                include_target(root, &base.join(d.trim()), f)
            }),
            _ => None,
        };
        if let Some(t) = target {
            walk(root, &t, seen, out, depth + 1);
        }
    }
}

fn labels(raw: &str, file: &str, out: &mut Vec<Label>) {
    let text = crate::prescan::strip_comments(raw);
    let mut i = 0;
    while let Some(off) = text[i..].find("\\label") {
        let start = i + off;
        i = start + 6;
        let j = skip_ws(text.as_bytes(), i);
        if let Some((key, end)) = group(&text, j) {
            let key = key.trim();
            if !key.is_empty() && !key.contains(['#', '\\']) {
                out.push(Label { key: key.to_string(), file: file.to_string(), line: line_of(&text, start) });
            }
            i = end;
        }
    }
}

/// 一个字段的值：`{…}`、`"…"` 或裸词（数字、`@string` 宏名）
fn field_value(text: &str, j: usize) -> Option<(String, usize)> {
    let b = text.as_bytes();
    match b.get(j)? {
        b'{' => group(text, j).map(|(v, end)| (v.to_string(), end)),
        b'"' => {
            let mut depth = 0;
            for (k, &c) in b.iter().enumerate().skip(j + 1) {
                match c {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    b'"' if depth == 0 => return Some((text[j + 1..k].to_string(), k + 1)),
                    _ => {}
                }
            }
            None
        }
        _ => {
            let end = j + text[j..].bytes().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b':')).count();
            (end > j).then(|| (text[j..end].to_string(), end))
        }
    }
}

/// 解析 `.bib`：`@type{key, field = value, …}`。`@string`、`@preamble`、`@comment` 跳过。
/// 不求完整——补全只要键和几项说明，坏掉的条目跳过去接着读
fn bib(text: &str, file: &str, out: &mut Vec<BibEntry>) {
    let b = text.as_bytes();
    let mut i = 0;
    while let Some(off) = text[i..].find('@') {
        let at = i + off;
        i = at + 1;
        let kind_end = i + text[i..].bytes().take_while(u8::is_ascii_alphabetic).count();
        let kind = text[i..kind_end].to_ascii_lowercase();
        let open = skip_ws(b, kind_end);
        if kind.is_empty() || !matches!(b.get(open), Some(b'{' | b'(')) {
            continue;
        }
        if matches!(kind.as_str(), "string" | "preamble" | "comment") {
            i = group(text, open).map_or(open + 1, |(_, end)| end);
            continue;
        }
        let key_start = skip_ws(b, open + 1);
        let key_end = key_start + text[key_start..].bytes().take_while(|&c| c != b',' && c != b'}' && !c.is_ascii_whitespace()).count();
        let key = text[key_start..key_end].trim();
        if key.is_empty() {
            continue;
        }
        let mut entry = BibEntry {
            key: key.to_string(),
            kind,
            title: None,
            author: None,
            year: None,
            file: file.to_string(),
            line: line_of(text, at),
        };
        // 读字段直到这个条目的右括号
        let mut j = key_end;
        loop {
            j = skip_ws(b, j);
            match b.get(j) {
                Some(b',') => j += 1,
                Some(b'}' | b')') | None => {
                    j += 1;
                    break;
                }
                _ => {
                    let name_end = j + text[j..].bytes().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')).count();
                    let name = text[j..name_end].to_ascii_lowercase();
                    let eq = skip_ws(b, name_end);
                    if name.is_empty() || b.get(eq) != Some(&b'=') {
                        break;
                    }
                    let Some((mut value, mut end)) = field_value(text, skip_ws(b, eq + 1)) else { break };
                    // `a # " and " # b` 拼接：照样往后读，值只留第一段
                    while b.get(skip_ws(b, end)) == Some(&b'#') {
                        let Some((more, e)) = field_value(text, skip_ws(b, skip_ws(b, end) + 1)) else { break };
                        value.push_str(&more);
                        end = e;
                    }
                    let value = clean(&value);
                    match name.as_str() {
                        "title" => entry.title = Some(value),
                        "author" | "editor" if entry.author.is_none() => entry.author = Some(value),
                        "year" => entry.year = Some(value),
                        "date" if entry.year.is_none() => entry.year = Some(value.chars().take(4).collect()),
                        _ => {}
                    }
                    j = end;
                }
            }
        }
        out.push(entry);
        // 文件在条目中间结束时 j 会越过末尾
        i = j.min(text.len()).max(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "subo-symbols-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(d.join("chapters")).unwrap();
        d
    }

    #[test]
    fn outline_follows_inputs_in_document_order() {
        let d = scratch();
        fs::write(
            d.join("main.tex"),
            "\\documentclass{book}\n\\begin{document}\n\\chapter{Intro}\\label{ch:intro}\n% \\section{Commented}\n\\input{chapters/one}\n\\section*[short]{Last \\textbf{one}~here}\n\\end{document}\n",
        )
        .unwrap();
        fs::write(d.join("chapters/one.tex"), "\\section{Method}\n\\label{sec:m}\n\\subsection{Details of $x$}\n").unwrap();
        fs::write(d.join("orphan.tex"), "\\label{orphan}\n").unwrap();
        let files: Vec<String> = ["main.tex", "chapters/one.tex", "orphan.tex"].iter().map(|s| s.to_string()).collect();
        let s = collect(&d, Some("main.tex"), &files);
        let titles: Vec<(u8, &str, &str, usize)> = s.outline.iter().map(|h| (h.level, h.title.as_str(), h.file.as_str(), h.line)).collect();
        assert_eq!(
            titles,
            vec![
                (1, "Intro", "main.tex", 3),
                (2, "Method", "chapters/one.tex", 1),
                (3, "Details of x", "chapters/one.tex", 3),
                (2, "Last one here", "main.tex", 6),
            ]
        );
        let keys: Vec<&str> = s.labels.iter().map(|l| l.key.as_str()).collect();
        assert_eq!(keys, vec!["ch:intro", "sec:m", "orphan"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn beamer_frames_and_include_cycles() {
        let d = scratch();
        fs::write(d.join("main.tex"), "\\begin{frame}[t]{Hello}\\end{frame}\n\\input{main}\n\\begin{frame}\\frametitle{World}\\end{frame}\n").unwrap();
        let s = collect(&d, Some("main.tex"), &[]);
        let titles: Vec<&str> = s.outline.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(titles, vec!["Hello", "World"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn bib_entries_parse() {
        let text = r#"@string{acl = "ACL"}
@comment{ignore me}
@article{knuth84,
  author = {Donald E. Knuth},
  title  = "Literate {P}rogramming",
  year   = 1984,
  journal = acl # " Journal",
}
@InProceedings{ vaswani2017,
  title={Attention Is All You Need}, author={Vaswani, Ashish and others},
  date={2017-06-12}}
@misc{broken
"#;
        let mut out = Vec::new();
        bib(text, "refs.bib", &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].key, "knuth84");
        assert_eq!(out[0].kind, "article");
        assert_eq!(out[0].title.as_deref(), Some("Literate Programming"));
        assert_eq!(out[0].year.as_deref(), Some("1984"));
        assert_eq!(out[0].line, 3);
        assert_eq!(out[1].key, "vaswani2017");
        assert_eq!(out[1].kind, "inproceedings");
        assert_eq!(out[1].author.as_deref(), Some("Vaswani, Ashish and others"));
        assert_eq!(out[1].year.as_deref(), Some("2017"));
        assert_eq!(out[2].key, "broken");
    }
}
