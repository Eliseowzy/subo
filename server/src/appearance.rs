//! 外观与语言包：`<subo 目录>/appearance/`，界面「设置 → 通用」读写。
//!
//! ```text
//! appearance/
//! ├── prefs.json            当前选择：明暗模式、配色方案、界面语言、编辑器字号
//! ├── themes/<id>.json      配色方案包（format = "subo-theme"）
//! └── locales/<code>.json   界面语言包（format = "subo-locale"）
//! ```
//!
//! 包的内容（令牌是否合法、文案键是否齐全）由前端校验：它知道有哪些设计令牌、哪些文案键。
//! 这里只把关存储层面的事：类型对、id 能当文件名、大小有上限、确实是 JSON 对象。
//! 用户也可以直接把文件丢进对应目录，下次启动生效。
//!
//! 偏好放在这里而不是 localStorage：桌面壳每次启动端口不同，localStorage 跟着源走，会丢。

use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// 单个包的大小上限。完整的语言包约 40 KB，留足余量
pub const MAX_PACK_BYTES: usize = 2 * 1024 * 1024;
/// 一个目录里最多读这么多个包，防止误把大目录指过来
const MAX_PACKS: usize = 200;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Theme,
    Locale,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "theme" => Some(Kind::Theme),
            "locale" => Some(Kind::Locale),
            _ => None,
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Kind::Theme => "themes",
            Kind::Locale => "locales",
        }
    }

    fn format(self) -> &'static str {
        match self {
            Kind::Theme => "subo-theme",
            Kind::Locale => "subo-locale",
        }
    }

    /// 包里标识自己的字段：配色用 `id`，语言用 `code`（BCP 47）
    fn id_field(self) -> &'static str {
        match self {
            Kind::Theme => "id",
            Kind::Locale => "code",
        }
    }
}

pub fn root(subo: &Path) -> PathBuf {
    subo.join("appearance")
}

/// id 直接当文件名：字母数字开头，只含字母数字和 `-` `_` `.`，不超过 64 个字符
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !id.contains("..")
}

/// 读出某类的全部包。坏文件（不是 JSON、类型不对、id 与文件名不符）跳过，不拦启动
pub fn list(subo: &Path, kind: Kind) -> Vec<Value> {
    let dir = root(subo).join(kind.dir());
    let Ok(rd) = fs::read_dir(&dir) else { return Vec::new() };
    let mut names: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    names.sort();
    names
        .into_iter()
        .take(MAX_PACKS)
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?.to_string();
            if fs::metadata(&p).ok()?.len() as usize > MAX_PACK_BYTES {
                return None;
            }
            let v: Value = serde_json::from_slice(&fs::read(&p).ok()?).ok()?;
            (check(kind, &v).ok()? == stem).then_some(v)
        })
        .collect()
}

/// 结构上的最低要求，返回包的 id
pub fn check(kind: Kind, v: &Value) -> Result<String, &'static str> {
    let obj = v.as_object().ok_or("appearance.notPack")?;
    if obj.get("format").and_then(Value::as_str) != Some(kind.format()) {
        return Err("appearance.wrongFormat");
    }
    let id = obj.get(kind.id_field()).and_then(Value::as_str).unwrap_or("");
    if !valid_id(id) {
        return Err("appearance.badId");
    }
    Ok(id.to_string())
}

/// 存一个包；同 id 的直接覆盖（导入新版本）。先写临时文件再改名
pub fn save(subo: &Path, kind: Kind, id: &str, v: &Value) -> std::io::Result<()> {
    let dir = root(subo).join(kind.dir());
    fs::create_dir_all(&dir)?;
    write_atomic(&dir.join(format!("{id}.json")), v)
}

pub fn remove(subo: &Path, kind: Kind, id: &str) -> std::io::Result<bool> {
    let p = root(subo).join(kind.dir()).join(format!("{id}.json"));
    match fs::remove_file(p) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// 偏好的键和值都是短字符串（或 null 表示恢复默认），其余忽略
const PREF_KEYS: &[&str] = &["mode", "theme", "locale", "editorSize"];

pub fn load_prefs(subo: &Path) -> Map<String, Value> {
    let v: Option<Value> = fs::read(root(subo).join("prefs.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let mut out = Map::new();
    if let Some(Value::Object(m)) = v {
        for (k, v) in m {
            if PREF_KEYS.contains(&k.as_str()) && v.as_str().is_some_and(|s| s.len() <= 64) {
                out.insert(k, v);
            }
        }
    }
    out
}

/// 合并一组改动：字符串覆盖，null 删除，其他键忽略
pub fn save_prefs(subo: &Path, patch: &Map<String, Value>) -> std::io::Result<Map<String, Value>> {
    let mut cur = load_prefs(subo);
    for k in PREF_KEYS {
        match patch.get(*k) {
            Some(Value::String(s)) if s.len() <= 64 => {
                cur.insert(k.to_string(), Value::String(s.clone()));
            }
            Some(Value::Null) => {
                cur.remove(*k);
            }
            _ => {}
        }
    }
    let dir = root(subo);
    fs::create_dir_all(&dir)?;
    write_atomic(&dir.join("prefs.json"), &Value::Object(cur.clone()))?;
    Ok(cur)
}

fn write_atomic(file: &Path, v: &Value) -> std::io::Result<()> {
    let tmp = file.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, serde_json::to_vec_pretty(v).unwrap_or_default())?;
    fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids() {
        for ok in ["ocean", "zh-TW", "my_theme.v2", "a"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", "-x", "../x", "a/b", "a..b", "空", &"x".repeat(65)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn round_trip() {
        let subo = std::env::temp_dir().join(format!("subo-appearance-{}", std::process::id()));
        let _ = fs::remove_dir_all(&subo);
        let theme = json!({ "format": "subo-theme", "version": 1, "id": "ocean", "name": "Ocean" });
        assert_eq!(check(Kind::Theme, &theme), Ok("ocean".into()));
        assert_eq!(check(Kind::Locale, &theme), Err("appearance.wrongFormat"));
        save(&subo, Kind::Theme, "ocean", &theme).unwrap();
        // 文件名和 id 对不上的跳过
        fs::write(root(&subo).join("themes/other.json"), theme.to_string()).unwrap();
        fs::write(root(&subo).join("themes/broken.json"), "{").unwrap();
        assert_eq!(list(&subo, Kind::Theme), vec![theme]);
        assert!(remove(&subo, Kind::Theme, "ocean").unwrap());
        assert!(!remove(&subo, Kind::Theme, "ocean").unwrap());

        let p = save_prefs(&subo, json!({ "mode": "dark", "theme": "ocean", "junk": "x" }).as_object().unwrap()).unwrap();
        assert_eq!(Value::Object(p), json!({ "mode": "dark", "theme": "ocean" }));
        let p = save_prefs(&subo, json!({ "theme": null, "locale": "en" }).as_object().unwrap()).unwrap();
        assert_eq!(Value::Object(p), json!({ "mode": "dark", "locale": "en" }));
        let _ = fs::remove_dir_all(&subo);
    }
}
