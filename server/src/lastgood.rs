//! 上次编译成功时的样子：AI 报错修复和编译校验的基准。
//!
//! - 源码：大部分报错是刚改出来的，「从上次能编译到现在改了什么」往往比报错位置更能说明原因；
//! - 排版信号：校验 AI 的改动时，只有新冒出来的排版异常才怪它，文档原有的不算。
//!
//! 每次正式编译成功后整份换掉：`~/.subo/lastgood/<项目路径哈希>/` 下按文件路径哈希存这次读过的
//! 项目源文件（看 `-recorder` 的 .fls），外加 `layout.json`。不往项目里写东西。

use crate::{diagnose::Layout, library};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// 记这些扩展名的源文件；图片、数据不记
const EXTS: [&str; 5] = ["tex", "sty", "cls", "bib", "ltx"];
/// 单个文件上限：再大多半不是手写的源码
const FILE_MAX: u64 = 1 << 20;
/// 一个项目合计上限
const TOTAL_MAX: u64 = 8 << 20;
const LAYOUT: &str = "layout.json";

fn dir(root: &Path) -> PathBuf {
    library::subo_dir().join("lastgood").join(crate::hash_name(root))
}

fn name(rel: &str) -> String {
    format!("{}.txt", crate::hash_name(rel))
}

/// .fls 里这次读过的项目文件（相对项目根的路径）
fn inputs(root: &Path, fls: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in fls.lines() {
        let Some(p) = l.strip_prefix("INPUT ") else { continue };
        let p = Path::new(p.trim());
        let rel = if p.is_absolute() {
            match p.strip_prefix(root) {
                Ok(r) => r.to_path_buf(),
                Err(_) => continue,
            }
        } else {
            p.components().filter(|c| !matches!(c, Component::CurDir)).collect()
        };
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            continue;
        }
        let ok = rel.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.contains(&e));
        let rel = rel.to_string_lossy().replace('\\', "/");
        if ok && !out.contains(&rel) {
            out.push(rel);
        }
    }
    out
}

/// 正式编译成功后调用。出错就算了：只是少了基准，不影响编译本身
pub fn record(root: &Path, outdir: &Path, entry: &str, log: &str) {
    let stem = Path::new(entry).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let Ok(fls) = fs::read_to_string(outdir.join(format!("{stem}.fls"))) else { return };
    let dst = dir(root);
    // 先写到旁边再换上，免得读到一半新一半旧
    let tmp = dst.with_extension("tmp");
    let _ = fs::remove_dir_all(&tmp);
    if fs::create_dir_all(&tmp).is_err() {
        return;
    }
    let mut total = 0;
    for rel in inputs(root, &fls) {
        let p = root.join(&rel);
        let Ok(meta) = fs::metadata(&p) else { continue };
        if meta.len() > FILE_MAX || total + meta.len() > TOTAL_MAX {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&p) {
            total += meta.len();
            let _ = fs::write(tmp.join(name(&rel)), text);
        }
    }
    if let Ok(j) = serde_json::to_string(&crate::diagnose::layout(log)) {
        let _ = fs::write(tmp.join(LAYOUT), j);
    }
    let _ = fs::remove_dir_all(&dst);
    let _ = fs::rename(&tmp, &dst);
}

/// 上次编译成功时 `rel` 的内容；没编译成功过、或那次没读它，返回 None
pub fn source(root: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(dir(root).join(name(rel))).ok()
}

/// 上次编译成功时日志里的排版异常；不知道时为空
pub fn layout(root: &Path) -> Vec<Layout> {
    fs::read_to_string(dir(root).join(LAYOUT)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fls_inputs_inside_project() {
        let root = Path::new("/p/简历");
        let fls = "PWD /p/简历\nINPUT /usr/tl/article.cls\nINPUT ./resume.cls\nINPUT resume.tex\nINPUT ./resume.tex\nINPUT /p/简历/ch/one.tex\n\
                   INPUT /p/简历/out/resume.aux\nINPUT ../other.tex\nINPUT fonts/a.otf\nOUTPUT resume.log\n";
        assert_eq!(inputs(root, fls), vec!["resume.cls", "resume.tex", "ch/one.tex"]);
    }
}
