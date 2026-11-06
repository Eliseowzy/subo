//! 旧模板和新版 TeX 的已知不兼容：编译失败时认出来，用户点了确认才改项目里的文件。
//!
//! 跟缺字清单（[`crate::case`]）一样只诊断、不擅自动手：改的是用户的论文源码，
//! 悄悄改了会让 git 里平白多出改动，传回 arXiv / 投稿系统时也和官方样式对不上。
//!
//! - `combinedblfloats`：ACL / EMNLP / NAACL 2018–2020 年的样式文件为绕开 hyperref 的换页 bug，
//!   用 etoolbox 的 `\patchcmd` 改 `\@combinedblfloats`，补丁失败就 `\errmessage` 中止编译。
//!   LaTeX 2021 起内核改写了这个命令，补丁必然失败；新内核也没有那个 bug 了，
//!   ACL 后来的 `acl.sty` 直接删了这行。修法是把失败分支改成空，补丁打不上就跳过
//! - `cjkutf8`：`CJK` / `CJKutf8` 是 pdfLaTeX 的中日韩宏包（arXiv 按 pdfLaTeX 编，不少论文用它包一层
//!   `\begin{CJK*}{UTF8}{gbsn}`）。XeTeX 下它把 0x80–0xFF 当成 UTF-8 的首字节，碰到 `è` 这类
//!   Latin-1 字符就报 `Invalid character code`。XeTeX 本来就认 UTF-8，修法是不加载它、
//!   把两个环境定义成空壳；项目正文里真有中日韩文字时改用 ctex（`scheme=plain` 不动版式）配字体

use crate::fail::Fail;
use crate::project;
use serde::Serialize;
use std::fs;
use std::path::Path;

/// 各条的稳定编号，前后端都用它
pub const COMBINEDBLFLOATS: &str = "combinedblfloats";
pub const CJKUTF8: &str = "cjkutf8";

/// 补丁失败时的那段分支，换成 `{}` 就不再中止
const FAIL_BRANCH: &str = "{\\errmessage{\\noexpand patch failed}}";

/// 改过的行尾加上这句，用户日后看到知道是谁改的、为什么
const NOTE: &str = " % Subo: LaTeX 2021+ changed \\@combinedblfloats; skip the patch instead of failing";
const CJK_NOTE: &str = " % Subo: CJK/CJKutf8 is pdfLaTeX-only; XeTeX reads UTF-8 natively";

/// 不加载 CJK 宏包时补上的两个空壳环境，参数照收不用：`\begin{CJK*}[fontenc]{UTF8}{gbsn}`
const CJK_ENVS: &str = "\\DeclareDocumentEnvironment{CJK}{omm}{}{}\\DeclareDocumentEnvironment{CJK*}{omm}{}{}";

/// 认出来的一处可修的问题
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    pub id: &'static str,
    /// 项目内的相对路径
    pub file: String,
    /// 从 1 起的行号
    pub line: usize,
}

/// 编译失败后看一眼：日志里有对应的报错，再到项目文件里找那一行。`program` 是这次用的 LaTeX 程序。
pub fn detect(root: &Path, log: &str, program: &str) -> Vec<Fix> {
    let mut out = Vec::new();
    if log.contains("patch failed") {
        scan(root, &[".sty", ".cls"], COMBINEDBLFLOATS, &mut out);
    }
    // CJK 宏包只在 Unicode 引擎下才会这样报错；pdfLaTeX 下报的是源码真有问题（比如不是 UTF-8），不能改成空壳
    if matches!(program, "xelatex" | "lualatex" | "tectonic") && log.contains("Package CJK Error") {
        scan(root, &[".tex", ".sty", ".cls"], CJKUTF8, &mut out);
    }
    out
}

fn scan(root: &Path, exts: &[&str], id: &'static str, out: &mut Vec<Fix>) {
    for e in project::tree(root) {
        let lower = e.path.to_ascii_lowercase();
        if e.dir || !exts.iter().any(|x| lower.ends_with(x)) {
            continue;
        }
        let Ok(text) = fs::read_to_string(root.join(&e.path)) else { continue };
        for (i, l) in text.lines().enumerate() {
            if target(id, l).is_some() {
                out.push(Fix { id, file: e.path.clone(), line: i + 1 });
            }
        }
    }
}

/// 一行里要改的那一段（字节区间），不是这条要改的行返回 `None`
fn target(id: &str, line: &str) -> Option<(usize, usize)> {
    let code = line.split('%').next().unwrap_or("");
    match id {
        COMBINEDBLFLOATS => {
            if code.contains("\\patchcmd") && code.contains("\\@combinedblfloats") {
                code.find(FAIL_BRANCH).map(|at| (at, at + FAIL_BRANCH.len()))
            } else {
                None
            }
        }
        CJKUTF8 => usepackage_cjk(code),
        _ => None,
    }
}

/// `\usepackage[选项]{CJKutf8}` 或 `{CJK}` 这一句的区间。和别的宏包写在一个 `{}` 里的不认，
/// 拆开要动别人的代码。
fn usepackage_cjk(code: &str) -> Option<(usize, usize)> {
    const CMD: &str = "\\usepackage";
    let mut from = 0;
    while let Some(i) = code[from..].find(CMD) {
        let start = from + i;
        from = start + CMD.len();
        let mut rest = code[from..].trim_start();
        if rest.starts_with('[') {
            let Some(j) = rest.find(']') else { return None };
            rest = rest[j + 1..].trim_start();
        }
        let Some(body) = rest.strip_prefix('{') else { continue };
        let Some(j) = body.find('}') else { return None };
        if matches!(body[..j].trim(), "CJKutf8" | "CJK") {
            let end = code.len() - body.len() + j + 1;
            return Some((start, end));
        }
    }
    None
}

/// 项目的 .tex 正文（注释不算）里有没有中日韩文字
fn has_cjk_text(root: &Path) -> bool {
    let cjk = |c: char| {
        matches!(c, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'
            | '\u{AC00}'..='\u{D7AF}' | '\u{F900}'..='\u{FAFF}' | '\u{20000}'..='\u{2FA1F}')
    };
    project::tree(root).iter().filter(|e| !e.dir && e.path.to_ascii_lowercase().ends_with(".tex")).any(|e| {
        fs::read_to_string(root.join(&e.path))
            .is_ok_and(|t| t.lines().any(|l| l.split('%').next().unwrap_or("").chars().any(cjk)))
    })
}

/// 用户点了修复：重新读文件、只改认得出的行，返回改了几处。
pub fn apply(root: &Path, id: &str, rel: &str) -> Result<usize, Fail> {
    let with = match id {
        COMBINEDBLFLOATS => "{}".to_string(),
        CJKUTF8 if has_cjk_text(root) => format!("\\usepackage[scheme=plain]{{ctex}}{CJK_ENVS}"),
        CJKUTF8 => CJK_ENVS.to_string(),
        _ => return Err(Fail::new("compat.unknown", format!("不认识的修复项：{id}")).with("id", id)),
    };
    let path = project::resolve(root, rel)?;
    let text = fs::read_to_string(&path)
        .map_err(|e| Fail::new("file.readFailed", format!("读取失败: {e}")).detail(e))?;
    let (fixed, n) = rewrite(&text, id, &with);
    if n == 0 {
        return Err(Fail::new("compat.nothing", "文件里没有找到要修的那一行，可能已经改过了").with("file", rel));
    }
    fs::write(&path, fixed).map_err(|e| Fail::new("file.writeFailed", format!("写入失败: {e}")).detail(e))?;
    Ok(n)
}

/// 逐行把认出的那一段换成 `with`，行尾加说明；保留原来的换行符（CRLF 的样式文件也不少）
fn rewrite(text: &str, id: &str, with: &str) -> (String, usize) {
    let note = if id == CJKUTF8 { CJK_NOTE } else { NOTE };
    let mut n = 0;
    let out: String = text
        .split_inclusive('\n')
        .map(|raw| {
            let body = raw.trim_end_matches(['\n', '\r']);
            let Some((a, b)) = target(id, body) else { return raw.to_string() };
            n += 1;
            let eol = &raw[body.len()..];
            format!("{}{with}{}{note}{eol}", &body[..a], &body[b..])
        })
        .collect();
    (out, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "\\patchcmd\\@combinedblfloats{\\box\\@outputbox}{\\unvbox\\@outputbox}{}{\\errmessage{\\noexpand patch failed}}";

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("subo-compat-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        dunce::canonicalize(&d).unwrap()
    }

    #[test]
    fn rewrite_keeps_patch_and_line_ending() {
        let (out, n) = rewrite(&format!("a\r\n{LINE}\r\nb\n"), COMBINEDBLFLOATS, "{}");
        assert_eq!(n, 1);
        assert!(out.starts_with("a\r\n\\patchcmd\\@combinedblfloats{\\box\\@outputbox}{\\unvbox\\@outputbox}{}{} % Subo:"));
        assert!(out.ends_with("failing\r\nb\n"));
        assert!(!out.contains("errmessage"));
        // 改过的行认不出了，再修一次什么都不动
        assert_eq!(rewrite(&out, COMBINEDBLFLOATS, "{}").1, 0);
    }

    #[test]
    fn commented_line_is_left_alone() {
        assert!(target(COMBINEDBLFLOATS, &format!("% {LINE}")).is_none());
        assert!(target(COMBINEDBLFLOATS, "\\patchcmd\\foo{a}{b}{}{\\errmessage{\\noexpand patch failed}}").is_none());
    }

    #[test]
    fn detect_needs_the_log_and_the_line() {
        let d = scratch("detect");
        fs::create_dir_all(d.join("style")).unwrap();
        fs::write(d.join("style/emnlp.sty"), format!("x\n{LINE}\n")).unwrap();
        fs::write(d.join("main.tex"), format!("{LINE}\n")).unwrap();
        assert!(detect(&d, "! Undefined control sequence.", "xelatex").is_empty());
        let got = detect(&d, "./style/emnlp.sty:2: patch failed.", "pdflatex");
        assert_eq!(got, vec![Fix { id: COMBINEDBLFLOATS, file: "style/emnlp.sty".into(), line: 2 }]);

        assert_eq!(apply(&d, COMBINEDBLFLOATS, "style/emnlp.sty").unwrap(), 1);
        assert!(detect(&d, "patch failed", "xelatex").is_empty());
        assert_eq!(apply(&d, COMBINEDBLFLOATS, "style/emnlp.sty").unwrap_err().code, "compat.nothing");
        assert_eq!(apply(&d, "nope", "style/emnlp.sty").unwrap_err().code, "compat.unknown");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn usepackage_cjk_forms() {
        assert_eq!(usepackage_cjk("\\usepackage{CJKutf8}"), Some((0, 20)));
        assert_eq!(usepackage_cjk("\\usepackage{xspace}\\usepackage [x] { CJK }  "), Some((19, 42)));
        assert!(usepackage_cjk("\\usepackage{CJKutf8,xspace}").is_none());
        assert!(usepackage_cjk("\\usepackage{xeCJK}").is_none());
        assert!(target(CJKUTF8, "% \\usepackage{CJKutf8}").is_none());
    }

    #[test]
    fn cjkutf8_becomes_envs_or_ctex() {
        let d = scratch("cjk");
        let main = "\\documentclass{article}\r\n\\usepackage{CJKutf8} % zh\r\n\\begin{document}\n\\begin{CJK*}{UTF8}{gbsn}Rozi\u{e8}re\\end{CJK*}\n% 中文注释\n";
        fs::write(d.join("main.tex"), main).unwrap();
        assert!(detect(&d, "! Undefined control sequence.", "xelatex").is_empty());
        let log = "./main.bbl:167: Package CJK Error: Invalid character code.";
        assert_eq!(detect(&d, log, "xelatex"), vec![Fix { id: CJKUTF8, file: "main.tex".into(), line: 2 }]);
        assert!(detect(&d, log, "pdflatex").is_empty());

        // 注释里的中文不算：只补空壳环境，不拉 ctex（xeCJK 会把英文里的 ’ 当成全角标点）
        assert_eq!(apply(&d, CJKUTF8, "main.tex").unwrap(), 1);
        let got = fs::read_to_string(d.join("main.tex")).unwrap();
        assert!(got.contains(&format!("\r\n{CJK_ENVS} % zh{CJK_NOTE}\r\n")));
        assert!(!got.contains("ctex"));
        assert!(detect(&d, log, "xelatex").is_empty());

        fs::write(d.join("main.tex"), main.replace("Rozi", "罗兹")).unwrap();
        apply(&d, CJKUTF8, "main.tex").unwrap();
        let got = fs::read_to_string(d.join("main.tex")).unwrap();
        assert!(got.contains("\\usepackage[scheme=plain]{ctex}\\DeclareDocumentEnvironment{CJK}"));
        let _ = fs::remove_dir_all(&d);
    }
}
