//! 预置模板：源码编进服务端二进制（`server/presets/<名>/`），离线可用、随版本走。
//!
//! **只带源码。** 字体、宏包都不随安装包分发：预置模板只用内置精简 TeX Live 已有的宏包，
//! 中文字体用系统自带的，西文字体用 TeX Live 里本来就有的 TeX Gyre / Libertinus。
//! 万一哪台机器缺东西，也和别的模板一样，打开即编译、走缺字清单补。
//!
//! 预置模板只读：不能删、不能改分类；想改就用它新建项目、改完再存成自己的模板。
//! id 带 `builtin:` 前缀，用户模板的 id 由 [`library::slug`] 生成，不会含冒号，两边撞不上。
//! 显示名、简介由前端按 id 查语言文件，这里的 `name` 只是兜底。

use crate::fail::Fail;
use crate::library;
use std::fs;
use std::path::Path;

pub const PREFIX: &str = "builtin:";

pub struct Preset {
    /// 不带前缀的 id，也是 `server/presets/` 下的目录名
    pub id: &'static str,
    /// 前端没有对应文案时显示的名字
    pub name: &'static str,
    pub category: &'static str,
    pub entry: &'static str,
    /// 用它建的项目记下的 LaTeX 程序。日文模板用 lualatex（按需引擎，没装时首次编译自动装）
    pub program: &'static str,
    /// (相对路径, 内容)
    pub files: &'static [(&'static str, &'static str)],
}

macro_rules! files {
    ($dir:literal: $($f:literal),* $(,)?) => {
        &[$(($f, include_str!(concat!("../presets/", $dir, "/", $f)))),*]
    };
}

/// 列表里的顺序：先文章、论文，再简历、幻灯片、海报
pub const ALL: &[Preset] = &[
    Preset {
        id: "article-zh",
        name: "中文论文",
        category: "paper",
        entry: "main.tex",
        program: "xelatex",
        files: files!("article-zh": "main.tex", "refs.bib"),
    },
    Preset {
        id: "article-zh-tw",
        name: "繁體中文論文",
        category: "paper",
        entry: "main.tex",
        program: "xelatex",
        files: files!("article-zh-tw": "main.tex", "refs.bib"),
    },
    Preset {
        id: "paper-en",
        name: "English Paper",
        category: "paper",
        entry: "main.tex",
        program: "xelatex",
        files: files!("paper-en": "main.tex", "refs.bib"),
    },
    Preset {
        id: "paper-ja",
        name: "日本語論文",
        category: "paper",
        entry: "main.tex",
        program: "lualatex",
        files: files!("paper-ja": "main.tex", "refs.bib"),
    },
    Preset {
        id: "paper-ko",
        name: "한국어 논문",
        category: "paper",
        entry: "main.tex",
        program: "xelatex",
        files: files!("paper-ko": "main.tex", "refs.bib"),
    },
    Preset {
        id: "thesis-zh",
        name: "中文学位论文",
        category: "thesis",
        entry: "main.tex",
        program: "xelatex",
        files: files!("thesis-zh":
            "main.tex", "refs.bib",
            "front/cover.tex", "front/abstract.tex", "front/thanks.tex",
            "chapters/intro.tex", "chapters/related.tex", "chapters/method.tex",
            "chapters/experiment.tex", "chapters/conclusion.tex",
        ),
    },
    Preset {
        id: "report-zh",
        name: "中文报告",
        category: "report",
        entry: "main.tex",
        program: "xelatex",
        files: files!("report-zh": "main.tex"),
    },
    Preset {
        id: "resume-zh",
        name: "中文简历",
        category: "resume",
        entry: "main.tex",
        program: "xelatex",
        files: files!("resume-zh": "main.tex"),
    },
    Preset {
        id: "resume-en",
        name: "English Resume",
        category: "resume",
        entry: "main.tex",
        program: "xelatex",
        files: files!("resume-en": "main.tex"),
    },
    Preset {
        id: "slides-zh",
        name: "中文幻灯片",
        category: "slides",
        entry: "main.tex",
        program: "xelatex",
        files: files!("slides-zh": "main.tex"),
    },
    Preset {
        id: "poster",
        name: "Academic Poster",
        category: "poster",
        entry: "main.tex",
        program: "xelatex",
        files: files!("poster": "main.tex"),
    },
];

/// `builtin:xxx` → 预置模板；别的 id 返回 `None`
pub fn get(id: &str) -> Option<&'static Preset> {
    let bare = id.strip_prefix(PREFIX)?;
    ALL.iter().find(|p| p.id == bare)
}

impl Preset {
    pub fn full_id(&self) -> String {
        format!("{PREFIX}{}", self.id)
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|(_, c)| c.len() as u64).sum()
    }

    /// 把源码写进 `dst`（已存在的空目录）。路径是编译期写死的，仍查一遍防手误
    pub fn write_to(&self, dst: &Path) -> Result<(), Fail> {
        let io = |e: std::io::Error| Fail::new("template.copy", format!("复制文件失败: {e}")).detail(e);
        for (rel, content) in self.files {
            if !rel.split('/').all(|seg| library::valid_id(seg) && seg != "..") {
                return Err(Fail::new("template.copy", format!("模板路径不合法: {rel}")));
            }
            let path = dst.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(io)?;
            }
            fs::write(&path, content).map_err(io)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_well_formed() {
        let mut ids = std::collections::HashSet::new();
        for p in ALL {
            assert!(ids.insert(p.id), "重复的预置模板 {}", p.id);
            assert!(crate::templates::CATEGORIES.contains(&p.category), "{} 的分类不在列表里", p.id);
            let entry = p.files.iter().find(|(f, _)| *f == p.entry).expect("入口文件要在 files 里");
            assert!(entry.1.contains("\\documentclass") && entry.1.contains("\\begin{document}"));
            assert_eq!(get(&p.full_id()).map(|x| x.id), Some(p.id));
        }
        assert!(get("paper-en").is_none(), "不带前缀的不算预置模板");
    }

    /// 用预置模板建的项目创建时就记了引擎，可在记 pin 之前建的老项目没有，
    /// 只能靠探测：每个预置模板都得认得出是它的引擎，否则老项目会悄悄换成 pdflatex
    #[test]
    fn presets_detect_their_program_without_pin() {
        use crate::engine::program::{detect_program, Program};
        let root = std::env::temp_dir().join(format!("subo-preset-detect-{}", std::process::id()));
        for p in ALL {
            let dir = root.join(p.id);
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            p.write_to(&dir).unwrap();
            let got = detect_program(&dir, p.entry).map(|d| d.program);
            assert_eq!(got, Program::parse(p.program), "{} 探测不出 {}", p.id, p.program);
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// 预置模板不带字体文件、不带编译产物：只有源码与参考文献
    #[test]
    fn presets_ship_sources_only() {
        for p in ALL {
            for (f, _) in p.files {
                let ext = f.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
                assert!(matches!(ext, "tex" | "bib"), "{}/{f} 不是源码", p.id);
            }
        }
    }
}
