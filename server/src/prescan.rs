//! 缺包预扫描：TeX 遇到第一个缺的文件就停，光看日志只能「编译 → 装一个 → 再编译」一轮轮试。
//! 编译因缺文件失败时，顺着入口文件读项目源码，把 `\usepackage`、`\documentclass`、beamer 主题
//! 这些声明一次收齐，交给 [`crate::case::confirm`] 只留完整版 TeX Live 有、本机没装的，一起装。
//!
//! 只扫项目自己的文件，不往已装的宏包里钻：它们的依赖本来就齐，里面又满是
//! `\IfFileExists` 包着的可选依赖，钻进去只会多装。刚装上的宏包例外，见 [`requires_of`]。

use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// 顺着 `\input` 最多读这么多个项目文件
const MAX_FILES: usize = 200;
/// 单个文件超过这个大小就不读：多半是数据，不是源码
const MAX_BYTES: u64 = 2 << 20;

/// 文件里声明要用的宏包、文档类、beamer 主题（`foo.sty` / `foo.cls`），
/// 要读进来的项目文件（`\input`、`\include`、`\subfile` 的参数，原样），
/// 和插图（`\includegraphics` 的参数，原样）。
#[derive(Debug, Default, PartialEq)]
pub struct Refs {
    pub files: Vec<String>,
    pub inputs: Vec<String>,
    pub graphics: Vec<String>,
}

/// 命令 → 参数要拼成的文件名 `(前缀, 后缀)`；后缀为空的是 `\input` 一类
const COMMANDS: &[(&str, &str, &str)] = &[
    ("usepackage", "", ".sty"),
    ("RequirePackage", "", ".sty"),
    ("RequirePackageWithOptions", "", ".sty"),
    ("documentclass", "", ".cls"),
    ("LoadClass", "", ".cls"),
    ("LoadClassWithOptions", "", ".cls"),
    ("usetheme", "beamertheme", ".sty"),
    ("usecolortheme", "beamercolortheme", ".sty"),
    ("usefonttheme", "beamerfonttheme", ".sty"),
    ("useinnertheme", "beamerinnertheme", ".sty"),
    ("useoutertheme", "beameroutertheme", ".sty"),
    ("input", "", ""),
    ("include", "", ""),
    ("subfile", "", ""),
    ("includegraphics", "", ""),
];

/// 包在这些条件里的加载是可选的：有就用，没有也不报错，不算缺
const OPTIONAL: &[&str] = &["IfFileExists", "IfPackageAvailable", "IfClassAvailable", "InputIfFileExists"];

pub fn refs(text: &str) -> Refs {
    let text = strip_comments(text);
    let mut out = Refs::default();
    let b = text.as_bytes();
    let mut i = 0;
    while let Some(off) = text[i..].find('\\') {
        let start = i + off;
        let name_end = start + 1 + text[start + 1..].bytes().take_while(u8::is_ascii_alphabetic).count();
        i = name_end.max(start + 1);
        let name = &text[start + 1..name_end];
        let Some(&(_, prefix, ext)) = COMMANDS.iter().find(|(c, _, _)| *c == name) else { continue };
        let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
        if OPTIONAL.iter().any(|o| text[line_start..start].contains(o)) {
            continue;
        }
        // 可选参数 `[…]` 可能跨行，beamer 主题还可能带 `<…>` 的模式参数；`\includegraphics*` 带星号
        let mut j = skip_ws(b, name_end);
        if b.get(j) == Some(&b'*') {
            j = skip_ws(b, j + 1);
        }
        for (open, close) in [(b'<', b'>'), (b'[', b']')] {
            if b.get(j) == Some(&open) {
                let Some(k) = text[j..].find(close as char) else { break };
                j = skip_ws(b, j + k + 1);
            }
        }
        let arg = if b.get(j) == Some(&b'{') {
            match text[j + 1..].find('}') {
                Some(k) => &text[j + 1..j + 1 + k],
                None => continue,
            }
        } else if ext.is_empty() && name == "input" {
            // plain TeX 的 `\input foo`
            text[j..].split(|c: char| c.is_whitespace() || c == '\\' || c == '}').next().unwrap_or("")
        } else {
            continue;
        };
        i = j;
        for item in arg.split(',') {
            let item = item.trim();
            // 宏定义里的 `\RequirePackage{#1}`、拿宏拼的名字都认不出，跳过
            if item.is_empty() || item.contains(['#', '\\', '{', '}']) {
                continue;
            }
            if name == "includegraphics" {
                if !out.graphics.iter().any(|x| x == item) {
                    out.graphics.push(item.to_string());
                }
            } else if ext.is_empty() {
                if !out.inputs.iter().any(|x| x == item) {
                    out.inputs.push(item.to_string());
                }
            } else {
                let file = format!("{prefix}{item}{ext}");
                if crate::case::valid_name(&file) && !out.files.contains(&file) {
                    out.files.push(file);
                }
            }
        }
    }
    out
}

/// 去掉 `%` 起的注释（`\%` 不算）
pub(crate) fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut esc = false;
        let mut end = line.len();
        for (k, c) in line.char_indices() {
            if c == '%' && !esc {
                end = k;
                break;
            }
            esc = c == '\\' && !esc;
        }
        out.push_str(&line[..end]);
        out.push('\n');
    }
    out
}

fn skip_ws(b: &[u8], mut j: usize) -> usize {
    while b.get(j).is_some_and(u8::is_ascii_whitespace) {
        j += 1;
    }
    j
}

/// 从入口文件顺着 `\input`、项目里自带的 `.sty` / `.cls` 读下去，收齐项目外要的文件名。
/// 项目里就有的不算；本机装没装、TeX Live 有没有，交给 [`crate::case::confirm`] 判。
pub fn scan(root: &Path, entry: &str) -> Vec<String> {
    walk(root, entry).0
}

/// 项目源码里 `\includegraphics` 引用的 EPS / PS 图（项目里的相对路径）。缺 Ghostscript 时
/// pdflatex 停在第一张图上，日志里只有它；顺着源码把要转的图一次列全。
/// 不带扩展名的 `\includegraphics{fig}` 只有项目里没有同名 PDF / PNG / JPG 时才算 EPS。
pub fn eps_figures(root: &Path, entry: &str) -> Vec<String> {
    walk(root, entry).1
}

fn walk(root: &Path, entry: &str) -> (Vec<String>, Vec<String>) {
    let paths: Vec<String> =
        crate::project::tree(root).into_iter().filter(|e| !e.dir).map(|e| e.path.replace('\\', "/")).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue = vec![entry.replace('\\', "/")];
    let mut out: Vec<String> = Vec::new();
    let mut eps: Vec<String> = Vec::new();
    while let Some(rel) = queue.pop() {
        if seen.len() >= MAX_FILES || !seen.insert(rel.clone()) {
            continue;
        }
        let path = root.join(&rel);
        if fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(true) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let r = refs(&String::from_utf8_lossy(&bytes));
        for f in r.files {
            match crate::case::locate(&paths, &f) {
                Some(p) => queue.push(p),
                None if !out.contains(&f) => out.push(f),
                None => {}
            }
        }
        // `\input{chapters/intro}`：相对项目根，不带扩展名时补 `.tex`
        for inp in r.inputs {
            if inp.split('/').any(|seg| seg == "..") || inp.starts_with('/') {
                continue;
            }
            let hit = [inp.clone(), format!("{inp}.tex")].into_iter().find(|c| paths.contains(c)).or_else(|| {
                (!inp.contains('/')).then(|| crate::case::locate(&paths, &inp)).flatten()
            });
            if let Some(p) = hit {
                queue.push(p);
            }
        }
        for g in r.graphics {
            let g = g.trim_matches('"');
            let g = g.strip_prefix("./").unwrap_or(g);
            let lower = g.to_ascii_lowercase();
            let hit = if lower.ends_with(".eps") || lower.ends_with(".ps") {
                paths.iter().find(|p| *p == g)
            } else if ["pdf", "png", "jpg", "jpeg"].iter().any(|e| paths.contains(&format!("{g}.{e}"))) {
                None
            } else {
                ["eps", "ps"].iter().find_map(|e| paths.iter().find(|p| **p == format!("{g}.{e}")))
            };
            if let Some(p) = hit {
                if !eps.contains(p) {
                    eps.push(p.clone());
                }
            }
        }
    }
    (out, eps)
}

/// 刚装上的文件里声明要用的宏包、文档类：tlmgr 只装包自己登记的依赖，`.sty` 里
/// `\RequirePackage` 的东西大多没登记，不补的话重编一次又缺下一个。`paths` 是这些文件的绝对路径。
pub fn requires_of(paths: &[std::path::PathBuf]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        if fs::metadata(p).map(|m| m.len() > MAX_BYTES).unwrap_or(true) {
            continue;
        }
        let Ok(bytes) = fs::read(p) else { continue };
        for f in refs(&String::from_utf8_lossy(&bytes)).files {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_classes_and_themes() {
        let src = r"\documentclass[12pt,
  a4paper]{ctexart}
\usepackage{amsmath, dramatist}% 注释里的 \usepackage{nope}
% \usepackage{commented}
\usepackage [x] { tikz }
\usetheme[progressbar=frametitle]{metropolis}
\usecolortheme<presentation>{beaver}
\IfFileExists{opt.sty}{\usepackage{opt}}{}
\newcommand\load[1]{\RequirePackage{#1}}
\input{chapters/intro}
\input preamble
";
        let r = refs(src);
        assert_eq!(
            r.files,
            ["ctexart.cls", "amsmath.sty", "dramatist.sty", "tikz.sty", "beamerthememetropolis.sty", "beamercolorthemebeaver.sty"]
        );
        assert_eq!(r.inputs, ["chapters/intro", "preamble"]);
    }

    #[test]
    fn escaped_percent_is_not_a_comment() {
        let r = refs(r"50\% \usepackage{xcolor}");
        assert_eq!(r.files, ["xcolor.sty"]);
    }

    #[test]
    fn longer_command_names_are_not_matched() {
        let r = refs(r"\usepackagex{a} \inputencoding{utf8} \includegraphicsx{fig}");
        assert_eq!(r, Refs::default());
        // `\includegraphics` 不是 `\include`
        let r = refs(r"\includegraphics{fig}");
        assert_eq!(r, Refs { graphics: vec!["fig".into()], ..Refs::default() });
    }

    #[test]
    fn eps_figures_follow_inputs() {
        let dir = std::env::temp_dir().join(format!("subo-prescan-eps-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("figs")).unwrap();
        for f in ["a.eps", "figs/b.eps", "figs/c.eps", "figs/c.pdf", "d.ps", "unused.eps", "e.png"] {
            fs::write(dir.join(f), "x").unwrap();
        }
        fs::write(
            dir.join("main.tex"),
            "\\includegraphics[width=1in]{a.eps}\n% \\includegraphics{unused.eps}\n\\input{sec}\\includegraphics{e}",
        )
        .unwrap();
        fs::write(dir.join("sec.tex"), "\\includegraphics*[trim=1 1 1 1]{./figs/b}\\includegraphics{figs/c}\\includegraphics{d}").unwrap();
        assert_eq!(eps_figures(&dir, "main.tex"), ["a.eps", "figs/b.eps", "d.ps"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn follows_project_files() {
        let dir = std::env::temp_dir().join(format!("subo-prescan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("chapters")).unwrap();
        fs::create_dir_all(dir.join("style")).unwrap();
        fs::write(dir.join("main.tex"), "\\documentclass{article}\\usepackage{mystyle}\\input{chapters/one}").unwrap();
        fs::write(dir.join("style/mystyle.sty"), "\\RequirePackage{fancyhdr}").unwrap();
        fs::write(dir.join("chapters/one.tex"), "\\usepackage{dramatist}\\input{../../etc/passwd}").unwrap();
        let mut got = scan(&dir, "main.tex");
        got.sort();
        assert_eq!(got, ["article.cls", "dramatist.sty", "fancyhdr.sty"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
