//! Translations (Python: `proxydock/i18n.py`). Source strings are English and double as lookup keys; each
//! `locales/<code>.json` maps them to the translation, anything missing stays English. Placeholders are filled
//! after translating: `tr("Rule {n}").replace("{n}", ...)`.
//!
//! The tables are the Python app's own files, built in.

use std::collections::HashMap;
use std::sync::RwLock;

pub const DEFAULT_LANGUAGE: &str = "en";
pub const LANGUAGES: [(&str, &str); 8] = [
    ("en", "English"),
    ("ko", "한국어"),
    ("ja", "日本語"),
    ("zh_CN", "简体中文"),
    ("zh_TW", "繁體中文"),
    ("de", "Deutsch"),
    ("fr", "Français"),
    ("es", "Español"),
];

fn source(code: &str) -> Option<&'static str> {
    Some(match code {
        "ko" => include_str!("../../../locales/ko.json"),
        "ja" => include_str!("../../../locales/ja.json"),
        "zh_CN" => include_str!("../../../locales/zh_CN.json"),
        "zh_TW" => include_str!("../../../locales/zh_TW.json"),
        "de" => include_str!("../../../locales/de.json"),
        "fr" => include_str!("../../../locales/fr.json"),
        "es" => include_str!("../../../locales/es.json"),
        _ => return None,
    })
}

struct State {
    code: String,
    table: HashMap<String, String>,
}

static STATE: RwLock<Option<State>> = RwLock::new(None);

/// English -> translation table for `code` (empty for English or an unknown code)
pub fn load_table(code: &str) -> HashMap<String, String> {
    let Some(text) = source(code) else { return HashMap::new() };
    let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(text) else {
        return HashMap::new();
    };
    m.into_iter()
        .filter_map(|(k, v)| match v {
            serde_json::Value::String(s) if !s.is_empty() => Some((k, s)),
            _ => None,
        })
        .collect()
}

pub fn set_language(code: &str) {
    let code = if LANGUAGES.iter().any(|(c, _)| *c == code) { code } else { DEFAULT_LANGUAGE };
    *STATE.write().unwrap() = Some(State { code: code.to_string(), table: load_table(code) });
}

pub fn current() -> String {
    STATE.read().unwrap().as_ref().map(|s| s.code.clone()).unwrap_or_else(|| DEFAULT_LANGUAGE.into())
}

/// Python `_t(text)`
pub fn tr(text: &str) -> String {
    STATE.read().unwrap().as_ref().and_then(|s| s.table.get(text).cloned()).unwrap_or_else(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn korean_table() {
        let t = load_table("ko");
        assert_eq!(t.get("Rule {n}").map(String::as_str), Some("규칙 {n}"));
        assert!(load_table("en").is_empty());
    }
}
