//! SyncTeX：源码行 ↔ 预览位置
//!
//! 不依赖 TeX Live 的 `synctex` 命令行（精简安装里常常没有），直接读引擎写出的
//! `.synctex`。格式是逐行文本，这里只用到其中一部分：
//!
//! ```text
//! Input:<tag>:<path>              源文件登记
//! Unit / Magnification / X Offset / Y Offset
//! {<page>  }<page>                页的开始与结束
//! [tag,line:h,v:W,H,D  ]          vbox
//! (tag,line:h,v:W,H,D  )          hbox（一行字通常就是一个）
//! x|k|g|$|r|h|v tag,line:h,v...   字距、粘连、数学、标尺、空盒子等叶子记录
//! ```
//!
//! 坐标以页左上角为原点、向下为正，单位换算成 PDF 的 bp（1/72 英寸），
//! 与 pdf.js 在 scale = 1 时的视口一致。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

#[derive(Clone, Copy)]
struct Rec {
    page: u32,
    /// 记录类型的首字符
    kind: u8,
    tag: u32,
    line: u32,
    h: f64,
    v: f64,
    w: f64,
    height: f64,
    depth: f64,
    /// 外层最近的 hbox（在 recs 里的下标）
    parent: Option<usize>,
}

impl Rec {
    fn is_hbox(&self) -> bool {
        self.kind == b'('
    }
    fn is_box(&self) -> bool {
        matches!(self.kind, b'(' | b'[')
    }
    fn contains(&self, x: f64, y: f64, slack: f64) -> bool {
        x >= self.h - slack
            && x <= self.h + self.w + slack
            && y >= self.v - self.height - slack
            && y <= self.v + self.depth + slack
    }
}

pub struct Synctex {
    /// tag → 项目内相对路径；项目以外的文件（宏包、.aux）不登记
    inputs: HashMap<u32, String>,
    recs: Vec<Rec>,
}

/// 预览上的一块区域，单位 bp，原点在页左上
#[derive(Serialize)]
pub struct Spot {
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Serialize)]
pub struct Source {
    pub file: String,
    pub line: u32,
}

/// PDF 旁边的 SyncTeX 文件；引擎按 `-synctex=-1` 写的是不压缩的版本
pub fn locate(pdf: &Path) -> Option<PathBuf> {
    let p = pdf.with_extension("synctex");
    p.is_file().then_some(p)
}

/// 去掉 `./`、消解 `..`，不碰磁盘
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// 引擎记下的路径换成项目内相对路径；`/` 分隔，和文件树一致
fn relative(root: &Path, raw: &str) -> Option<String> {
    let p = Path::new(raw);
    let abs = if p.is_absolute() { normalize(p) } else { normalize(&root.join(p)) };
    let rel = abs
        .strip_prefix(root)
        .ok()
        .map(Path::to_path_buf)
        .or_else(|| dunce::canonicalize(&abs).ok()?.strip_prefix(root).ok().map(Path::to_path_buf))?;
    let parts: Vec<String> =
        rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl Synctex {
    pub fn load(file: &Path, root: &Path) -> std::io::Result<Self> {
        let text = std::fs::read(file)?;
        Ok(Self::parse(&String::from_utf8_lossy(&text), root))
    }

    fn parse(text: &str, root: &Path) -> Self {
        let mut inputs = HashMap::new();
        let mut recs: Vec<Rec> = Vec::new();
        let (mut unit, mut mag, mut xoff, mut yoff) = (1.0, 1000.0, 0.0, 0.0);
        let mut page = 0u32;
        // 盒子栈：记录各层是不是 hbox 以及下标，叶子记录挂到最近的 hbox 上
        let mut stack: Vec<usize> = Vec::new();
        let mut in_content = false;

        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Input:") {
                if let Some((tag, path)) = rest.split_once(':') {
                    if let (Ok(tag), Some(rel)) = (tag.parse(), relative(root, path)) {
                        inputs.insert(tag, rel);
                    }
                }
                continue;
            }
            if !in_content {
                let num = |s: &str| s.trim().parse::<f64>().ok();
                if let Some(v) = line.strip_prefix("Unit:").and_then(num) {
                    unit = v;
                } else if let Some(v) = line.strip_prefix("Magnification:").and_then(num) {
                    mag = v;
                } else if let Some(v) = line.strip_prefix("X Offset:").and_then(num) {
                    xoff = v;
                } else if let Some(v) = line.strip_prefix("Y Offset:").and_then(num) {
                    yoff = v;
                } else if line == "Content:" {
                    in_content = true;
                }
                continue;
            }
            if line.starts_with("Postamble:") {
                break;
            }
            let Some(&kind) = line.as_bytes().first() else { continue };
            let body = &line[1..];
            match kind {
                b'{' => {
                    page = body.parse().unwrap_or(page + 1);
                    stack.clear();
                }
                b'}' => stack.clear(),
                b']' | b')' => {
                    stack.pop();
                }
                b'[' | b'(' | b'x' | b'k' | b'g' | b'$' | b'r' | b'h' | b'v' => {
                    // sp → bp：65781.76 = 65536 × 72.27 / 72
                    let k = unit * mag / 1000.0 / 65781.76;
                    let Some(r) = parse_rec(body, k, xoff, yoff) else { continue };
                    let parent = stack.iter().rev().copied().find(|&i: &usize| recs[i].kind == b'(');
                    let i = recs.len();
                    recs.push(Rec { page, kind, parent, ..r });
                    if kind == b'[' || kind == b'(' {
                        stack.push(i);
                    }
                }
                _ => {}
            }
        }
        Synctex { inputs, recs }
    }

    /// 源码 → 预览：没有正好那一行的记录就往后找最近的一行，再不行往前找
    pub fn view(&self, file: &str, line: u32) -> Option<Spot> {
        let tags: Vec<u32> =
            self.inputs.iter().filter(|(_, p)| p.as_str() == file).map(|(t, _)| *t).collect();
        let ours = |r: &&Rec| tags.contains(&r.tag) && r.page > 0;
        let after = self.recs.iter().filter(ours).filter(|r| r.line >= line).map(|r| r.line).min();
        let before = || self.recs.iter().filter(ours).filter(|r| r.line < line).map(|r| r.line).max();
        let target = after.or_else(before)?;

        let hits: Vec<&Rec> = self.recs.iter().filter(ours).filter(|r| r.line == target).collect();
        let page = hits.iter().map(|r| r.page).min()?;
        // 每条命中记录取所在的那一行（外层 hbox），合起来就是这行源码排出来的范围
        let mut rects: Vec<&Rec> = hits
            .iter()
            .filter(|r| r.page == page)
            .filter_map(|r| if r.is_hbox() { Some(*r) } else { r.parent.map(|i| &self.recs[i]) })
            .filter(|b| b.w > 0.0)
            .collect();
        if rects.is_empty() {
            let r = hits.iter().find(|r| r.page == page)?;
            return Some(Spot { page, x: r.h, y: r.v - r.height.max(8.0), w: r.w.max(0.0), h: r.height.max(8.0) + r.depth });
        }
        // 一行源码可能排成很多行（长段落），只取从第一处起连在一起的那一片，最多半页
        rects.sort_by(|a, b| (a.v - a.height).total_cmp(&(b.v - b.height)));
        let first = rects[0];
        let (mut x0, mut y0) = (first.h, first.v - first.height);
        let (mut x1, mut y1) = (first.h + first.w, first.v + first.depth);
        for b in &rects[1..] {
            let top = b.v - b.height;
            if top - y1 > 24.0 || b.v + b.depth - y0 > 400.0 {
                break;
            }
            x0 = x0.min(b.h);
            x1 = x1.max(b.h + b.w);
            y1 = y1.max(b.v + b.depth);
            y0 = y0.min(top);
        }
        Some(Spot { page, x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    /// 预览 → 源码：先找点中的那一行（最小的 hbox），再在行里找离点最近的记录
    pub fn edit(&self, page: u32, x: f64, y: f64) -> Option<Source> {
        let on_page = || self.recs.iter().enumerate().filter(move |(_, r)| r.page == page);
        let known = |r: &Rec| self.inputs.contains_key(&r.tag);
        let area = |r: &Rec| r.w * (r.height + r.depth);

        let pick = |r: &Rec| Some(Source { file: self.inputs.get(&r.tag)?.clone(), line: r.line });

        let boxes = on_page()
            .filter(|(_, r)| r.is_hbox() && r.contains(x, y, 1.0))
            .min_by(|a, b| area(a.1).total_cmp(&area(b.1)));
        if let Some((bi, b)) = boxes {
            let dist = |r: &Rec| {
                // 记录的 h 是它的左端，点在它右边一点的最像「点中了它」
                let dx = x - r.h;
                if dx >= 0.0 { dx } else { -dx * 3.0 }
            };
            let leaf = on_page()
                .filter(|(_, r)| r.parent == Some(bi) && !r.is_box() && known(r))
                .min_by(|a, b| dist(a.1).total_cmp(&dist(b.1)));
            if let Some((_, r)) = leaf {
                return pick(r);
            }
            if known(b) {
                return pick(b);
            }
        }
        // 点在空白处：取整页最近的记录
        let d2 = |r: &Rec| {
            let dx = if x < r.h { r.h - x } else if x > r.h + r.w { x - r.h - r.w } else { 0.0 };
            let dy = if y < r.v - r.height { r.v - r.height - y } else if y > r.v + r.depth { y - r.v - r.depth } else { 0.0 };
            dx * dx + dy * dy
        };
        on_page()
            .map(|(_, r)| r)
            .filter(|r| known(r) && r.kind != b'[')
            .min_by(|a, b| d2(a).total_cmp(&d2(b)))
            .and_then(pick)
    }
}

/// `tag,line[,col]:h,v[:W,H,D]` 或 `tag,line:h,v:W`（字距、数学）
fn parse_rec(body: &str, k: f64, xoff: f64, yoff: f64) -> Option<Rec> {
    let mut parts = body.split(':');
    let mut tl = parts.next()?.split(',');
    let tag = tl.next()?.parse().ok()?;
    let line = tl.next()?.parse().ok()?;
    let mut hv = parts.next()?.split(',');
    let h: f64 = hv.next()?.parse().ok()?;
    let v: f64 = hv.next()?.parse().ok()?;
    let mut size = parts.next().map(|s| s.split(',').map(|n| n.parse::<f64>().unwrap_or(0.0)));
    let mut next = || size.as_mut().and_then(|s| s.next()).unwrap_or(0.0);
    let (w, height, depth) = (next(), next(), next());
    Some(Rec {
        page: 0,
        kind: 0,
        tag,
        line,
        h: h * k + xoff,
        v: v * k + yoff,
        w: w * k,
        height: height * k,
        depth: depth * k,
        parent: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "SyncTeX Version:1
Input:1:/proj/./main.tex
Input:2:/tex/article.cls
Input:3:/proj/./src/intro.tex
Output:pdf
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
{1
[1,10:4736287,49385636:32206764,44649349,0
(1,10:4736287,8200848:32206764,778001,267261
x1,10:4978597,8200848
x3,4:20000000,8200848
)
(1,12:4736287,10598312:32206764,753947,9043
x1,12:4736287,10598312
g1,13:16731356,10598312
)
]
}1
{2
(3,7:4736287,8200848:32206764,778001,267261
x3,7:4736287,8200848
)
}2
Postamble:
";

    fn sx() -> Synctex {
        Synctex::parse(SAMPLE, Path::new("/proj"))
    }

    #[test]
    fn inputs_are_project_relative() {
        let s = sx();
        assert_eq!(s.inputs.get(&1).unwrap(), "main.tex");
        assert_eq!(s.inputs.get(&3).unwrap(), "src/intro.tex");
        assert!(!s.inputs.contains_key(&2));
    }

    #[test]
    fn view_finds_line_box() {
        let s = sx();
        let v = s.view("main.tex", 12).unwrap();
        assert_eq!(v.page, 1);
        assert!((v.x - 72.0).abs() < 0.1);
        let v = s.view("src/intro.tex", 7).unwrap();
        assert_eq!(v.page, 2);
        // 没有第 11 行的记录，就落到第 12 行
        assert_eq!(s.view("main.tex", 11).unwrap().y, s.view("main.tex", 12).unwrap().y);
        assert!(s.view("nope.tex", 1).is_none());
    }

    #[test]
    fn edit_picks_nearest_leaf_in_line() {
        let s = sx();
        let k = 1.0 / 65781.76;
        let y = 8200848.0 * k - 2.0;
        let src = s.edit(1, 5000000.0 * k, y).unwrap();
        assert_eq!((src.file.as_str(), src.line), ("main.tex", 10));
        let src = s.edit(1, 21000000.0 * k, y).unwrap();
        assert_eq!((src.file.as_str(), src.line), ("src/intro.tex", 4));
        let src = s.edit(2, 100.0, y).unwrap();
        assert_eq!((src.file.as_str(), src.line), ("src/intro.tex", 7));
    }
}
