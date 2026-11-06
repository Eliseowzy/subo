//! AI 辅助的两项能力：报错修复、⌘K 内联指令。提示词都在这里，前端只给原文片段和意图。
//!
//! 还有编译校验：把建议套进项目的影子副本编一遍，日志交回前端（前端已有日志解析，和问题面板同一套），
//! 由它对比「改之前」的问题。影子副本只把通向被改文件的那几层目录建成真目录，其余全是指回原处的
//! 符号链接，项目再大也只花几毫秒；用户目录里不写任何东西。

use crate::{ai, diagnose::{self, Diagnosis}, engine, fail::Fail, lastgood, library, logparse, project, Shared};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    fs,
    path::{Path as FsPath, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// 交给模型的上下文上限（字），前后各自截断
const CONTEXT_MAX: usize = 6_000;
/// 被改写的片段上限（字）。⌘K 选中一整章也够了
const TARGET_MAX: usize = 60_000;
/// 导言区最多读这么多（字）
const PREAMBLE_MAX: usize = 20_000;

/* ---------- 提示词 ---------- */

const SYSTEM: &str = "You are the LaTeX editing assistant inside Subo, a desktop LaTeX editor. \
The user's document is shown to you in pieces: BEFORE and AFTER are read-only context, TARGET is the excerpt you rewrite.

Rules:
- Reply with the replacement for TARGET only, exactly once, as raw LaTeX: no explanation, never wrap it in ``` code fences, no BEFORE/AFTER text, no tags.
- Keep everything you were not asked to change exactly as it is: commands, \\label/\\ref/\\cite keys, comments, indentation and line breaks.
- Use only the LaTeX kernel and the packages listed as loaded. If another package is truly unavoidable, keep the change minimal and end your reply with a LaTeX comment `% needs \\usepackage{name}`.
- Keep the language of the document's prose unless the task says otherwise.
- When TARGET is empty, reply with the text to insert at the cursor position (between BEFORE and AFTER).
- When images are attached, they are source material from the user (a photo, scan or screenshot of formulas, tables or text): \
transcribe what the task asks for faithfully as LaTeX, and do not describe the image.";

const SYSTEM_FIX: &str = "You are the LaTeX error-fixing assistant inside Subo, a desktop LaTeX editor. \
You get the compiler's diagnosis of the first error and numbered excerpts of one file, chosen for that error.

TeX reports where it noticed a problem, which is often not where the mistake is. Use the diagnosis (the macro chain and \"What this means\") \
to find the real cause in the excerpts, and fix it there. When CHANGES lists edits made since the document last compiled, \
a mistake introduced by them is the most likely cause; restoring what an edit broke, at the place it was, is usually the right fix \
(for example, put a removed line back where it was rather than where TeX stopped).

Reply with edit blocks only, with no explanation and no code fences:
<<<<<<< SEARCH
lines copied exactly from the excerpts, without the line-number prefix
=======
the replacement lines
>>>>>>> REPLACE

Rules:
- SEARCH is whole lines copied character for character from the excerpts, with just enough lines to be unique in the file.
- Make as few and as small edits as possible; do not restyle or reword anything else. Keep \\label/\\ref/\\cite keys and comments.
- To delete lines, leave the replacement empty.
- Use only the LaTeX kernel and the packages listed as loaded. If another package is truly unavoidable, add a LaTeX comment `% needs \\usepackage{name}` in the replacement.
- If the excerpts do not show the cause, do not guess: reply with NO_FIX alone.";

/// ⌘K 里的预置指令。界面上的名字走语言文件，这里是给模型的说法
fn preset(id: &str) -> Option<&'static str> {
    Some(match id {
        "polish" => "Polish the prose: improve clarity, flow and academic tone. Keep the meaning, the LaTeX markup and the language.",
        "concise" => "Make the text more concise without losing information. Keep the LaTeX markup and the language.",
        "english" => "Translate the prose into natural academic English. Keep all LaTeX commands, math, labels and citation keys unchanged.",
        "chinese" => "Translate the prose into natural academic Simplified Chinese. Keep all LaTeX commands, math, labels and citation keys unchanged.",
        "booktabs" => "Rewrite this table as a professional three-line table using the booktabs package (\\toprule, \\midrule, \\bottomrule), without vertical rules. Keep the content and the caption/label.",
        _ => return None,
    })
}

/// 导言区里看得出来的东西：文档类、加载的宏包
#[derive(Debug, Default, PartialEq)]
struct Preamble {
    class: Option<String>,
    packages: Vec<String>,
}

pub(crate) fn strip_comments(line: &str) -> &str {
    // `\%` 不是注释
    let b = line.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'%' && (i == 0 || b[i - 1] != b'\\') {
            return &line[..i];
        }
    }
    line
}

fn scan_preamble(text: &str) -> Preamble {
    let head = match text.find("\\begin{document}") {
        Some(i) => &text[..i],
        None => text,
    };
    let head: String = head.chars().take(PREAMBLE_MAX).collect();
    let src: String = head.lines().map(strip_comments).collect::<Vec<_>>().join("\n");
    let mut p = Preamble::default();
    for cmd in ["\\documentclass", "\\usepackage", "\\RequirePackage"] {
        let mut rest = src.as_str();
        while let Some(i) = rest.find(cmd) {
            rest = &rest[i + cmd.len()..];
            let mut r = rest.trim_start();
            if r.starts_with('[') {
                match r.find(']') {
                    Some(j) => r = r[j + 1..].trim_start(),
                    None => break,
                }
            }
            let Some(body) = r.strip_prefix('{') else { continue };
            let Some(end) = body.find('}') else { break };
            let names = body[..end].split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            if cmd == "\\documentclass" {
                p.class = p.class.or_else(|| names.into_iter().next());
            } else {
                for n in names {
                    if !p.packages.contains(&n) {
                        p.packages.push(n);
                    }
                }
            }
        }
    }
    p
}

/// 上下文太长时只留靠近 TARGET 的那头
fn tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    s.chars().skip(n - max).collect()
}

fn head(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorIn {
    message: String,
    line: Option<u32>,
    /// 出错处的源码（问题面板那条的 `here`，断点标 ⟨here⟩）
    context: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistIn {
    /// `fix`：修编译错误；`edit`：⌘K 指令
    kind: String,
    /// 项目内相对路径
    file: String,
    /// ⌘K：原文片段。修复不用，上下文由服务端按错误类别从 `text` 里取
    #[serde(default)]
    before: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    after: String,
    /// 修复：被修文件的整份内容（编辑器里的，可能还没存盘）
    text: Option<String>,
    /// 修复：诊断编译读出的结果（`ai/diagnose`）；没有就拿 `error` 凑一份
    diagnosis: Option<Diagnosis>,
    /// ⌘K 里用户写的话
    instruction: Option<String>,
    /// ⌘K 里点的预置指令
    preset: Option<String>,
    error: Option<ErrorIn>,
    /// `Model.id`；不给用默认模型
    model: Option<String>,
    /// ⌘K 里附的图（data URL），模型要能看图
    #[serde(default)]
    images: Vec<String>,
    /// 光标在数学环境里（编辑器从语法树看出来的）
    #[serde(default)]
    in_math: bool,
}

/// ⌘K 只附了图、没写要求时的任务
fn image_task(input: &AssistIn) -> &'static str {
    if !input.target.is_empty() {
        "Rewrite TARGET so that it matches the content of the attached image."
    } else if input.in_math {
        "Transcribe the mathematics in the attached image."
    } else {
        "Transcribe the content of the attached image into LaTeX to insert at the cursor: \
a single formula as \\[ ... \\], several aligned lines in align*, a table as tabular, prose as plain text in the language shown in the image."
    }
}

struct Prompt {
    messages: Vec<ai::Message>,
    temperature: f64,
}

/// `base`：被修文件上次编译成功时的内容（lastgood.rs），只有修复用得上
fn build_prompt(input: &AssistIn, pre: &Preamble, engine: &str, base: Option<&str>) -> Result<Prompt, Fail> {
    if input.target.chars().count() > TARGET_MAX {
        return Err(Fail::new("ai.tooLarge", "发送的内容太多").with("max", TARGET_MAX));
    }
    let (task, temperature) = match input.kind.as_str() {
        "fix" => return build_fix_prompt(input, pre, engine, base),
        "edit" => {
            let preset = input.preset.as_deref().and_then(preset);
            let said = input.instruction.as_deref().map(str::trim).filter(|s| !s.is_empty());
            let mut t = match (preset, said) {
                (Some(p), Some(s)) => format!("{p}\nAlso: {s}"),
                (Some(p), None) => p.to_string(),
                (None, Some(s)) => format!("Instruction from the user (it may be in any language):\n{s}"),
                (None, None) if !input.images.is_empty() => image_task(input).to_string(),
                (None, None) => return Err(Fail::new("ai.badRequest", "请求不完整")),
            };
            if !input.images.is_empty() && said.is_some() {
                t.push_str("\nUse the attached image(s) as the source for this.");
            }
            if input.in_math && input.target.is_empty() {
                t.push_str("\nThe cursor is already inside math mode: reply with the formula only, without $, \\( \\) or \\[ \\] delimiters and without a math environment.");
            }
            (t, 0.3)
        }
        _ => return Err(Fail::new("ai.badRequest", "请求不完整")),
    };

    let mut ctx = header(&input.file, pre, engine);
    ctx.push_str(&format!("<BEFORE>\n{}\n</BEFORE>\n", tail(&input.before, CONTEXT_MAX)));
    ctx.push_str(&format!("<TARGET>\n{}\n</TARGET>\n", input.target));
    ctx.push_str(&format!("<AFTER>\n{}\n</AFTER>\n\n", head(&input.after, CONTEXT_MAX)));
    ctx.push_str("Task: ");
    ctx.push_str(&task);
    let user = ai::Message::user(ctx).with_images(input.images.clone());
    Ok(Prompt { messages: vec![ai::Message::system(SYSTEM), user], temperature })
}

/// 两种提示词共用的开头：文件、引擎、文档类、宏包
fn header(file: &str, pre: &Preamble, engine: &str) -> String {
    let mut ctx = String::new();
    ctx.push_str(&format!("File: {file}\nEngine: {engine}\n"));
    if let Some(c) = &pre.class {
        ctx.push_str(&format!("Document class: {c}\n"));
    }
    ctx.push_str(&format!(
        "Loaded packages: {}\n\n",
        if pre.packages.is_empty() { "(none found)".to_string() } else { pre.packages.join(", ") }
    ));
    ctx
}

/// 报错修复：不再以出错行为中心切一段让模型重写，而是交一份诊断材料（diagnose.rs），
/// 让模型用 SEARCH/REPLACE 块指出改哪几处——原因常常不在报错的那一行
fn build_fix_prompt(input: &AssistIn, pre: &Preamble, engine: &str, base: Option<&str>) -> Result<Prompt, Fail> {
    let text = input.text.as_deref().ok_or_else(|| Fail::new("ai.badRequest", "请求不完整"))?;
    let d = match (&input.diagnosis, &input.error) {
        (Some(d), _) => d.clone(),
        (None, Some(e)) => diagnose::fallback(&e.message, &input.file, e.line, e.context.as_deref()),
        (None, None) => return Err(Fail::new("ai.badRequest", "请求不完整")),
    };
    let ex = diagnose::excerpts(text, &input.file, &d);
    let mut ctx = header(&input.file, pre, engine);
    let changes = base.map(|b| diagnose::diff(b, text));
    ctx.push_str(&diagnose::render(text, &input.file, &d, changes.as_ref(), &ex));
    ctx.push_str(&format!("\nTask: the compiler ({engine}) stopped with the error above. Fix it with the smallest edits."));
    Ok(Prompt { messages: vec![ai::Message::system(SYSTEM_FIX), ai::Message::user(ctx)], temperature: 0.1 })
}

/// 项目入口的导言区；读不出来就当没有
fn preamble_of(root: &FsPath) -> Preamble {
    project::entry(root)
        .and_then(|e| project::resolve(root, &e).ok())
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|t| scan_preamble(&t))
        .unwrap_or_default()
}

/* ---------- 接口：修复 / 改写（流式） ---------- */

pub async fn assist(State(s): State<Shared>, Path(id): Path<String>, Json(input): Json<AssistIn>) -> Response {
    let Some(q) = s.quire(&id) else { return crate::not_found() };
    if let Some(r) = s.read_only(&q) {
        return r;
    }
    if let Err(f) = project::resolve(&q.root, &input.file) {
        return f.status(StatusCode::BAD_REQUEST);
    }
    let pre = preamble_of(&q.root);
    let engine = project::entry(&q.root).map(|e| s.engine_for(&q, &e).0.to_string()).unwrap_or_else(|| s.engine.clone());
    let base = if input.kind == "fix" { lastgood::source(&q.root, &input.file) } else { None };
    let prompt = match build_prompt(&input, &pre, &engine, base.as_deref()) {
        Ok(p) => p,
        Err(f) => return f.status(StatusCode::BAD_REQUEST),
    };
    ai::stream_chat(&s, input.model.as_deref(), prompt.messages, Some(prompt.temperature), Some(8192)).await
}

/* ---------- 接口：编译校验 ---------- */

#[derive(Deserialize)]
pub struct VerifyIn {
    /// 被改的文件（项目内相对路径）
    file: String,
    /// 改完后的整份内容
    text: String,
}

/// 影子副本的根：`<subo 目录>/ai-verify/`，每次一个子目录，用完即删
fn verify_root() -> PathBuf {
    library::subo_dir().join("ai-verify")
}

/// 在 `dst` 下照着 `src` 摆一层：`keep` 指向的那条路径建成真目录（最后一级文件不建，由调用方写），
/// 其余条目都是指回原处的符号链接（Windows 上见 `link_plain`）
fn mirror(src: &FsPath, dst: &FsPath, keep: &[String]) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    let first = keep.first().map(String::as_str);
    for e in fs::read_dir(src)?.flatten() {
        let name = e.file_name();
        let n = name.to_string_lossy();
        if n == ".git" || n == ".subo" {
            continue;
        }
        if Some(n.as_ref()) == first {
            if keep.len() > 1 {
                mirror(&e.path(), &dst.join(&name), &keep[1..])?;
            }
            continue;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(e.path(), dst.join(&name))?;
        #[cfg(not(unix))]
        link_plain(&e.path(), &dst.join(&name))?;
    }
    Ok(())
}

/// Windows 上普通用户建不了符号链接：目录照建、往下递归，文件用硬链接（不占空间），
/// 跨盘建不了硬链接就复制
#[cfg(not(unix))]
fn link_plain(src: &FsPath, dst: &FsPath) -> std::io::Result<()> {
    if src.is_dir() {
        return mirror(src, dst, &[]);
    }
    fs::hard_link(src, dst).or_else(|_| fs::copy(src, dst).map(|_| ()))
}

/// 在影子副本里把 `file` 换成 `text` 编一遍。和正式编译同一个引擎、同样的超时与资源限额，但不进编译工位、不发事件。
/// `diag`：诊断编译（见 `engine::compile_diagnose`）
async fn shadow(s: &Shared, id: &str, file: &str, text: String, diag: bool) -> Result<(engine::CompileResult, u64), Response> {
    let Some(q) = s.quire(id) else { return Err(crate::not_found()) };
    if let Some(r) = s.read_only(&q) {
        return Err(r);
    }
    let target = project::resolve(&q.root, file).map_err(|f| f.status(StatusCode::BAD_REQUEST))?;
    let Some(entry) = project::entry(&q.root) else {
        return Err(Fail::new("build.noTex", "项目里没有找到 .tex 文件").status(StatusCode::BAD_REQUEST));
    };
    let rel: Vec<String> = match target.strip_prefix(&q.root) {
        Ok(r) => r.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect(),
        Err(_) => return Err(Fail::new("path.outside", "路径非法：指向项目目录之外").status(StatusCode::BAD_REQUEST)),
    };
    let s2 = s.clone();
    let res = tokio::task::spawn_blocking(move || {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = verify_root().join(format!("{}-{nanos}", crate::hash_name(&q.id)));
        let src = dir.join("src");
        let out = dir.join("out");
        let run = || -> Result<(engine::CompileResult, u64), Fail> {
            let fail = |e: std::io::Error| Fail::new("ai.verifyFailed", "编译校验没能进行").detail(e);
            mirror(&q.root, &src, &rel).map_err(fail)?;
            let file = rel.iter().fold(src.clone(), |p, c| p.join(c));
            fs::write(&file, &text).map_err(fail)?;
            let started = Instant::now();
            let (prog, _, _) = s2.engine_for(&q, &entry);
            let r = if diag {
                engine::compile_diagnose(prog, &src, &entry, &out, s2.timeout(), &s2.limits())
            } else {
                engine::compile(prog, &src, &entry, &out, s2.timeout(), &s2.limits(), &|_| {})
            };
            Ok((r, started.elapsed().as_millis() as u64))
        };
        let r = run();
        let _ = fs::remove_dir_all(&dir);
        r
    })
    .await;
    match res {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(f)) => Err(f.status(StatusCode::INTERNAL_SERVER_ERROR)),
        Err(_) => Err(crate::fail::internal().status(StatusCode::INTERNAL_SERVER_ERROR)),
    }
}

/// 用建议改过的内容编一遍，交回日志，以及比上次编译成功时新冒出来的排版异常（能编译不等于改对了）
pub async fn verify(State(s): State<Shared>, Path(id): Path<String>, Json(input): Json<VerifyIn>) -> Response {
    let Some(q) = s.quire(&id) else { return crate::not_found() };
    match shadow(&s, &id, &input.file, input.text, false).await {
        Ok((r, ms)) => {
            let report = logparse::parse(&r.log);
            let layout = if r.ok { diagnose::fresh(diagnose::layout_of(&report), &lastgood::layout(&q.root)) } else { Vec::new() };
            Json(json!({ "ok": r.ok, "timedOut": r.timed_out, "issues": report.issues, "ms": ms, "layout": layout })).into_response()
        }
        Err(r) => r,
    }
}

/* ---------- 接口：诊断（报错修复的第一步） ---------- */

/// 诊断编译一遍，读出第一处错误和宏调用链（diagnose.rs）。编过了、或日志里没有认得的错误，`diagnosis` 为空
pub async fn diagnose(State(s): State<Shared>, Path(id): Path<String>, Json(input): Json<VerifyIn>) -> Response {
    match shadow(&s, &id, &input.file, input.text, true).await {
        Ok((r, ms)) => {
            let d = if r.ok { None } else { diagnose::parse(&r.log) };
            Json(json!({ "diagnosis": d, "timedOut": r.timed_out, "ms": ms })).into_response()
        }
        Err(r) => r,
    }
}

/// 启动时清掉上次没删干净的影子副本（进程被杀之类）
pub fn sweep() {
    let _ = fs::remove_dir_all(verify_root());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preamble() {
        let p = scan_preamble(
            "\\documentclass[11pt]{ctexart}\n% \\usepackage{ignored}\n\\usepackage[margin=1in]{geometry}\n\\usepackage{amsmath, amssymb}\n\\RequirePackage{xcolor}\n\\begin{document}\n\\usepackage{late}",
        );
        assert_eq!(p.class.as_deref(), Some("ctexart"));
        assert_eq!(p.packages, vec!["geometry", "amsmath", "amssymb", "xcolor"]);
    }

    #[test]
    fn percent_is_not_comment() {
        assert_eq!(strip_comments(r"50\% off % note"), r"50\% off ");
    }

    fn input(kind: &str) -> AssistIn {
        AssistIn {
            kind: kind.into(),
            file: "main.tex".into(),
            before: "a".repeat(CONTEXT_MAX + 10),
            target: "$a/b$".into(),
            after: "z".into(),
            text: Some("\\documentclass{article}\n\\begin{document}\n\\fracc{a}{b}\n\\end{document}\n".into()),
            diagnosis: None,
            instruction: Some("写成分式".into()),
            preset: None,
            error: Some(ErrorIn { message: "Undefined control sequence.".into(), line: Some(3), context: Some("l.3 \\fracc".into()) }),
            model: None,
            images: Vec::new(),
            in_math: false,
        }
    }

    #[test]
    fn prompts() {
        let pre = Preamble { class: Some("article".into()), packages: vec!["amsmath".into()] };
        let fix = build_prompt(&input("fix"), &pre, "xelatex", None).unwrap();
        let u = &fix.messages[1].content;
        assert!(u.contains("Reported at: main.tex line 3") && u.contains("    3 | \\fracc{a}{b}") && u.contains("Loaded packages: amsmath"), "{u}");
        assert!(fix.messages[0].content.contains("SEARCH"));
        // 修复不用前端切的片段；没有上次编译成功的基准就没有改动一节
        assert!(!u.contains("aaaa") && !u.contains("<CHANGES"));
        let base = "\\documentclass{article}\n\\begin{document}\n\\frac{a}{b}\n\\end{document}\n";
        let with = build_prompt(&input("fix"), &pre, "xelatex", Some(base)).unwrap();
        assert!(with.messages[1].content.contains("-      | \\frac{a}{b}\n+    3 | \\fracc{a}{b}"), "{}", with.messages[1].content);
        let mut no_text = input("fix");
        no_text.text = None;
        assert!(build_prompt(&no_text, &pre, "xelatex", None).is_err());
        let edit = build_prompt(&input("edit"), &pre, "xelatex", None).unwrap();
        // 前文截到上限
        assert!(!edit.messages[1].content.contains(&"a".repeat(CONTEXT_MAX + 1)));
        assert!(edit.messages[1].content.contains("写成分式"));
        let mut i = input("edit");
        i.instruction = None;
        i.preset = Some("booktabs".into());
        assert!(build_prompt(&i, &pre, "xelatex", None).unwrap().messages[1].content.contains("booktabs"));
        i.preset = Some("nope".into());
        assert!(build_prompt(&i, &pre, "xelatex", None).is_err());

        // 只附图不写要求：按光标处的情形给默认任务，图跟着 user 消息走
        let mut img = input("edit");
        img.instruction = None;
        img.target = String::new();
        img.images = vec!["data:image/png;base64,AAAA".into()];
        let p = build_prompt(&img, &pre, "xelatex", None).unwrap();
        assert!(p.messages[1].content.contains("align*"));
        assert_eq!(p.messages[1].images.len(), 1);
        img.in_math = true;
        let p = build_prompt(&img, &pre, "xelatex", None).unwrap();
        assert!(p.messages[1].content.contains("inside math mode") && !p.messages[1].content.contains("align*"));
        img.instruction = Some("只要第二行".into());
        assert!(build_prompt(&img, &pre, "xelatex", None).unwrap().messages[1].content.contains("source for this"));
    }

    #[cfg(unix)]
    #[test]
    fn mirror_links_all_but_target() {
        let base = std::env::temp_dir().join(format!("subo-mirror-{}", std::process::id()));
        let src = base.join("src");
        fs::create_dir_all(src.join("ch/fig")).unwrap();
        fs::write(src.join("main.tex"), "m").unwrap();
        fs::write(src.join("ch/one.tex"), "1").unwrap();
        fs::write(src.join("ch/two.tex"), "2").unwrap();
        fs::write(src.join("ch/fig/a.png"), "p").unwrap();
        let dst = base.join("dst");
        mirror(&src, &dst, &["ch".into(), "one.tex".into()]).unwrap();
        assert!(fs::symlink_metadata(dst.join("main.tex")).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(dst.join("ch")).unwrap().is_dir());
        assert!(fs::symlink_metadata(dst.join("ch/two.tex")).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(dst.join("ch/fig")).unwrap().file_type().is_symlink());
        assert!(!dst.join("ch/one.tex").exists());
        fs::write(dst.join("ch/one.tex"), "new").unwrap();
        // 原文件没被动
        assert_eq!(fs::read_to_string(src.join("ch/one.tex")).unwrap(), "1");
        let _ = fs::remove_dir_all(&base);
    }
}
