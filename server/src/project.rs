//! 项目（quire）：路径解析与文件树。
//! 只做一件事——把用户提交的相对路径安全地映射到项目根目录内。

use crate::fail::Fail;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const SKIP: &[&str] = &[".subo", crate::library::ARCHIVED_MARK, ".git", "node_modules", "target", "press", ".DS_Store"];

#[derive(Serialize)]
pub struct Entry {
    pub path: String,
    pub dir: bool,
}

/// 把 `rel` 解析成 `root` 下的绝对路径；拒绝 `..`、绝对路径与符号链接逃逸。
/// `root` 须已规范化（main 启动时做过 canonicalize）。
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, Fail> {
    let rel = rel.trim_start_matches(['/', '\\']);
    if rel.split(['/', '\\']).any(|c| c == "..") {
        return Err(Fail::new("path.parent", "路径非法：包含上级目录"));
    }
    if cfg!(windows) {
        if let Some(name) = rel.split(['/', '\\']).find(|c| reserved_on_windows(c)) {
            return Err(Fail::new("path.reserved", format!("路径非法：{name} 是 Windows 的保留名，不能用作文件名")).with("name", name));
        }
    }
    let path = root.join(rel);
    // 目标可能还不存在（新建文件）：往上找到最近一个真实存在的节点再规范化。
    // 用 symlink_metadata 判断存在性，悬空链接也算「存在」，随后 canonicalize 会拒掉它，
    // 否则写入会顺着悬空链接落到项目外。
    let mut probe = path.as_path();
    while fs::symlink_metadata(probe).is_err() {
        probe = probe.parent().ok_or_else(|| Fail::new("path.invalid", "路径非法"))?;
    }
    let real = dunce::canonicalize(&probe).map_err(|_| Fail::new("path.unresolvable", "路径非法：无法解析"))?;
    if !real.starts_with(root) {
        return Err(Fail::new("path.outside", "路径非法：指向项目目录之外"));
    }
    Ok(path)
}

/// Windows 不让用的文件名：设备名（`aux.tex`、`con`、`com1.txt`，不分大小写、不管扩展名），
/// 以及以点或空格结尾的名字（会被系统悄悄去掉，存进去的和打开的就不是同一个文件了）
pub fn reserved_on_windows(name: &str) -> bool {
    if name.ends_with(['.', ' ']) && name != "." {
        return true;
    }
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

/// 按扩展名给附件定 Content-Type；认不出的一律当二进制流。
pub fn mime(rel: &str) -> &'static str {
    let ext = rel.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "otf" => "font/otf",
        "ttf" => "font/ttf",
        "ttc" => "font/collection",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// 符号链接只在目标仍落在项目内时才列出，且不跟进遍历（防环）。
fn link_inside(root: &Path, path: &Path) -> bool {
    dunce::canonicalize(&path).map(|p| p.starts_with(root)).unwrap_or(false)
}

/// 广度遍历，最多 500 项，跳过构建目录。
pub fn tree(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    walk(root, root, &mut out, 0);
    out.sort_by(|a, b| b.dir.cmp(&a.dir).then(a.path.cmp(&b.path)));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Entry>, depth: usize) {
    if depth > 6 || out.len() > 500 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        let path = e.path();
        let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        if is_link && !link_inside(root, &path) {
            continue;
        }
        let is_dir = path.is_dir();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().to_string();
        out.push(Entry { path: rel, dir: is_dir });
        if is_dir && !is_link {
            walk(root, &path, out, depth + 1);
        }
    }
}

/// 规范化项目内相对路径：去掉前导 `/`/`\`/`./`，分隔符统一为 `/`。
pub fn normalize_rel(rel: &str) -> String {
    let mut s = rel.replace('\\', "/");
    while let Some(rest) = s.strip_prefix('/') {
        s = rest.to_string();
    }
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    s
}

/// 扩展名是否为 `.tex`（不区分大小写）
pub fn is_tex(rel: &str) -> bool {
    Path::new(rel).extension().is_some_and(|e| e.eq_ignore_ascii_case("tex"))
}

/// 从 TeX 源码里提取 `\documentclass[...]{name}` 的文档类名。
/// 忽略 `%` 注释行，支持 `[...]` 可选参数跨行。
fn document_class(src: &str) -> Option<String> {
    // 去掉每行未转义的 `%` 注释，保留换行作为空白分隔
    let mut cleaned = String::with_capacity(src.len().min(8192));
    for line in src.lines() {
        let mut escaped = false;
        let mut end = line.len();
        for (i, ch) in line.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
            } else if ch == '%' {
                end = i;
                break;
            }
        }
        cleaned.push_str(&line[..end]);
        cleaned.push('\n');
        // 导言区通常在文件前部，最多扫前 32 KB 足够
        if cleaned.len() >= 32 * 1024 {
            break;
        }
    }

    let bytes = cleaned.as_bytes();
    let needle = b"\\documentclass";
    let mut pos = 0;
    while let Some(rel) = cleaned[pos..].find("\\documentclass") {
        let idx = pos + rel;
        let after = idx + needle.len();
        pos = after;
        // 确认是完整命令名，而不是 `\documentclassfoo`
        if bytes.get(after).is_some_and(|b| b.is_ascii_alphabetic()) {
            continue;
        }
        let mut cur = cleaned[after..].trim_start();
        // 跳过可选参数 `[...]`
        if let Some(rest) = cur.strip_prefix('[') {
            let mut depth = 1usize;
            let mut found = None;
            let mut esc = false;
            for (i, ch) in rest.char_indices() {
                if esc {
                    esc = false;
                    continue;
                }
                match ch {
                    '\\' => esc = true,
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            found = Some(&rest[i + 1..]);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            cur = found?.trim_start();
        }
        // 读取 `{class_name}`
        let rest = cur.strip_prefix('{')?;
        let close = rest.find('}')?;
        let name = rest[..close].trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// 这个文件是不是可独立编译的主文档：存在的 `.tex` 文件、声明了 `\documentclass`，
/// 且不是 `subfiles` / `standalone` 这类从属子文档。
pub fn is_standalone_root(root: &Path, rel: &str) -> bool {
    let rel = normalize_rel(rel);
    if !is_tex(&rel) {
        return false;
    }
    let Ok(path) = resolve(root, &rel) else { return false };
    if !path.is_file() {
        return false;
    }
    let Ok(src) = fs::read_to_string(&path) else { return false };
    document_class(&src).is_some_and(|cls| !matches!(cls.as_str(), "subfiles" | "standalone"))
}

/// 优先采用用户指定（或上次跟随记住）的入口：只要文件仍存在且合法就用它；否则退回自动探测。
pub fn entry_with_preferred(root: &Path, preferred: Option<&str>) -> Option<String> {
    if let Some(raw) = preferred {
        let rel = normalize_rel(raw);
        if is_tex(&rel) && resolve(root, &rel).is_ok_and(|p| p.is_file()) {
            return Some(rel);
        }
    }
    entry(root)
}

/// 探测入口文件：优先 main.tex，其次 arXiv 00README.json 声明，再其次顶层独立主文档（含 `\documentclass`
/// 且优先排除 `subfiles`/`standalone`），最后任意含 `\documentclass` 或第一个顶层 .tex。
/// 导入的项目入口常常不叫 main.tex（resume.tex、thesis.tex），章节文件又不能当入口。
pub fn entry(root: &Path) -> Option<String> {
    if root.join("main.tex").is_file() {
        return Some("main.tex".into());
    }
    if let Some(top) = arxiv_toplevel(root) {
        return Some(top);
    }
    let mut tex: Vec<String> = fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| is_tex(n))
        .collect();
    tex.sort();
    let doc_class = |n: &String| fs::read_to_string(root.join(n)).ok().and_then(|s| document_class(&s));
    let classes: Vec<(&String, Option<String>)> = tex.iter().map(|n| (n, doc_class(n))).collect();
    classes
        .iter()
        .find(|(_, c)| c.as_deref().is_some_and(|cls| !matches!(cls, "subfiles" | "standalone")))
        .or_else(|| classes.iter().find(|(_, c)| c.is_some()))
        .map(|(n, _)| (*n).clone())
        .or_else(|| tex.first().cloned())
}

/// arXiv 源码包里的 `00README.json` 写明了主文件：
/// `{"sources": [{"filename": "ms.tex", "usage": "toplevel"}, ...]}`。只认顶层存在的 .tex。
fn arxiv_toplevel(root: &Path) -> Option<String> {
    let meta: serde_json::Value = serde_json::from_slice(&fs::read(root.join("00README.json")).ok()?).ok()?;
    meta.get("sources")?.as_array()?.iter().find_map(|s| {
        let name = s.get("filename")?.as_str()?;
        (s.get("usage")?.as_str()? == "toplevel"
            && name.ends_with(".tex")
            && !name.contains(['/', '\\'])
            && root.join(name).is_file())
        .then(|| name.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_reserved_names() {
        for bad in ["aux.tex", "AUX", "con", "Nul.txt", "com1.log", "lpt9", "draft.", "notes "] {
            assert!(reserved_on_windows(bad), "{bad}");
        }
        for ok in ["main.tex", "auxiliary.tex", "com10.txt", "com0", "context.tex", "a.aux", ".latexmkrc"] {
            assert!(!reserved_on_windows(ok), "{ok}");
        }
    }

    #[test]
    fn document_class_detection() {
        assert_eq!(document_class("\\documentclass{resume}\n"), Some("resume".into()));
        assert_eq!(
            document_class("% \\documentclass{commented}\n\\documentclass[12pt, a4paper]{article}\n"),
            Some("article".into())
        );
        assert_eq!(
            document_class("\\documentclass[\n  twocolumn,\n  10pt\n]{IEEEtran}\n"),
            Some("IEEEtran".into())
        );
        assert_eq!(document_class("\\documentclass[../main.tex]{subfiles}\n"), Some("subfiles".into()));
        assert_eq!(document_class("\\documentclass[tikz]{standalone}\n"), Some("standalone".into()));
        assert_eq!(document_class("\\section{Intro}\n"), None);
    }

    #[test]
    fn standalone_root_and_preferred_entry() {
        let dir = std::env::temp_dir().join(format!("subo-project-entry-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        let root = dunce::canonicalize(&dir).unwrap();

        fs::write(root.join("a_fig.tex"), "\\documentclass{standalone}\n\\begin{document}fig\\end{document}\n").unwrap();
        fs::write(root.join("ch1.tex"), "\\documentclass[main.tex]{subfiles}\n\\begin{document}ch\\end{document}\n").unwrap();
        fs::write(root.join("intro.tex"), "\\section{Intro}\n").unwrap();
        fs::write(root.join("resume_en.tex"), "\\documentclass{resume}\n\\begin{document}en\\end{document}\n").unwrap();
        fs::write(root.join("resume_harness.tex"), "\\documentclass{resume}\n\\begin{document}h\\end{document}\n").unwrap();
        fs::write(root.join("sub/custom.tex"), "\\documentclass{article}\n\\begin{document}s\\end{document}\n").unwrap();

        assert!(!is_standalone_root(&root, "a_fig.tex"));
        assert!(!is_standalone_root(&root, "ch1.tex"));
        assert!(!is_standalone_root(&root, "intro.tex"));
        assert!(is_standalone_root(&root, "resume_en.tex"));
        assert!(is_standalone_root(&root, "resume_harness.tex"));
        assert!(is_standalone_root(&root, "sub/custom.tex"));

        // 自动探测应跳过 standalone/subfiles，选 resume_en.tex
        assert_eq!(entry(&root).as_deref(), Some("resume_en.tex"));
        // 指定偏好优先（含子目录、甚至 standalone）
        assert_eq!(entry_with_preferred(&root, Some("resume_harness.tex")).as_deref(), Some("resume_harness.tex"));
        assert_eq!(entry_with_preferred(&root, Some("sub/custom.tex")).as_deref(), Some("sub/custom.tex"));
        assert_eq!(entry_with_preferred(&root, Some("a_fig.tex")).as_deref(), Some("a_fig.tex"));
        // 偏好文件不存在时退回自动探测
        assert_eq!(entry_with_preferred(&root, Some("deleted.tex")).as_deref(), Some("resume_en.tex"));

        let _ = fs::remove_dir_all(&dir);
    }
}
