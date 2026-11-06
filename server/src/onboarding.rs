//! 上手引导：`<subo 目录>/onboarding.json` 记最后一次看完引导时的版本。
//!
//! 启动时判定这次该给哪一种引导：
//! - **全新安装**（`fresh`）：没有记录，本机也没有用过的痕迹；
//! - **升级**（`upgrade`）：记录的版本比当前旧，或者没有记录但本机已经用过
//!   （引导是后来才加的，之前的版本不会写记录）；
//! - 记录的就是当前版本：不弹。
//!
//! 判定只在启动时做一次：启动后服务自己会往 subo 目录写东西，再看就不准了。
//! 引导的内容在前端，这里只回答「该不该弹、弹哪种」。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const FILE: &str = "onboarding.json";

/// 本机用过的痕迹：subo 目录里这些东西是用户用过之后才会有的
const TRACES: &[&str] = &["settings.json", "folders.json", "appearance", "press", "case"];

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Fresh,
    Upgrade,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Record {
    /// 看完（或跳过）引导时的版本
    seen: Option<String>,
}

pub struct Onboarding {
    file: PathBuf,
    /// 这次启动判定的引导类型；与当前版本无关的重看不改它
    pub kind: Option<Kind>,
    /// 上次看过引导的版本，升级时给前端说「从哪个版本来」
    pub previous: Option<String>,
}

impl Onboarding {
    /// `projects`：项目库里已有的项目数，有项目也算用过
    pub fn detect(subo: &Path, projects: usize) -> Self {
        let file = subo.join(FILE);
        let record: Record = fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let used = projects > 0 || TRACES.iter().any(|t| subo.join(t).exists());
        let kind = classify(record.seen.as_deref(), used);
        Onboarding { file, kind, previous: record.seen }
    }

    /// 看完或跳过：记下当前版本，本次运行也不再弹
    pub fn done(&mut self) -> std::io::Result<()> {
        if let Some(dir) = self.file.parent() {
            fs::create_dir_all(dir)?;
        }
        let body = serde_json::to_vec_pretty(&Record { seen: Some(VERSION.into()) }).unwrap_or_default();
        let tmp = self.file.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&tmp, body)?;
        fs::rename(&tmp, &self.file)?;
        self.kind = None;
        Ok(())
    }
}

fn classify(seen: Option<&str>, used: bool) -> Option<Kind> {
    match seen {
        Some(v) if v == VERSION => None,
        // 记录比当前新（回退了版本）也按升级处理：内容是新功能介绍，多看一次无妨
        Some(_) => Some(Kind::Upgrade),
        None if used => Some(Kind::Upgrade),
        None => Some(Kind::Fresh),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_cases() {
        assert_eq!(classify(None, false), Some(Kind::Fresh));
        assert_eq!(classify(None, true), Some(Kind::Upgrade));
        assert_eq!(classify(Some("0.0.1"), true), Some(Kind::Upgrade));
        assert_eq!(classify(Some(VERSION), true), None);
        assert_eq!(classify(Some(VERSION), false), None);
    }

    #[test]
    fn detect_and_done() {
        let dir = std::env::temp_dir().join(format!("subo-onboarding-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(Onboarding::detect(&dir, 0).kind, Some(Kind::Fresh));
        assert_eq!(Onboarding::detect(&dir, 2).kind, Some(Kind::Upgrade));
        fs::write(dir.join("settings.json"), "{}").unwrap();
        let mut o = Onboarding::detect(&dir, 0);
        assert_eq!(o.kind, Some(Kind::Upgrade));
        o.done().unwrap();
        assert_eq!(o.kind, None);
        let again = Onboarding::detect(&dir, 0);
        assert_eq!(again.kind, None);
        assert_eq!(again.previous.as_deref(), Some(VERSION));
        fs::remove_dir_all(&dir).unwrap();
    }
}
