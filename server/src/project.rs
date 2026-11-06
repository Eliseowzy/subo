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

/// 探测入口文件：优先 main.tex，其次顶层含 `\documentclass` 的 .tex，最后任意顶层 .tex。
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
        .filter(|n| n.ends_with(".tex"))
        .collect();
    tex.sort();
    let is_root_doc = |n: &String| {
        fs::read_to_string(root.join(n))
            .map(|s| s.lines().any(|l| l.trim_start().starts_with("\\documentclass")))
            .unwrap_or(false)
    };
    tex.iter().find(|n| is_root_doc(n)).or(tex.first()).cloned()
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
}
