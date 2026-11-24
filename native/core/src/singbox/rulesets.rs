//! GeoIP / Geosite rule-sets for router rules (Python: `singbox/rulesets.py`; official SagerNet sing-geoip /
//! sing-geosite builds).
//!
//! ProxyDock downloads them itself into one shared folder instead of letting sing-box fetch them: sing-box refuses
//! to start when a remote rule-set cannot be downloaded on first use, and its download option is deprecated in
//! 1.14. A file older than a day is refreshed; if that fails the last good copy keeps working (offline start).

use crate::i18n::tr;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);

fn url(kind: &str, name: &str) -> Option<String> {
    // valid names only use [a-z0-9!@._-], which need no URL quoting
    match kind {
        "geoip" => Some(format!("https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-{name}.srs")),
        "geosite" => Some(format!("https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-{name}.srs")),
        _ => None,
    }
}

pub fn ruleset_dir() -> PathBuf {
    crate::paths::app_dir().join("rulesets")
}

pub fn tag(kind: &str, name: &str) -> String {
    format!("{kind}-{name}")
}

/// `^[a-z0-9][a-z0-9!@._-]{0,63}$`
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    let first = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    !b.is_empty() && b.len() <= 64 && first(b[0]) && b[1..].iter().all(|&c| first(c) || b"!@._-".contains(&c))
}

pub fn path_in(dir: &Path, kind: &str, name: &str) -> PathBuf {
    dir.join(format!("{}.srs", tag(kind, name)))
}

/// (kind, name) pairs a compiled proxy card's router rules use.
pub fn needed(proxy: &Value) -> Vec<(String, String)> {
    let mut out = vec![];
    for r in proxy["routes"].as_array().into_iter().flatten() {
        for m in r["match"].as_array().into_iter().flatten() {
            for kind in ["geoip", "geosite"] {
                for x in m[kind].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    out.push((kind.to_string(), x.to_string()));
                }
            }
        }
    }
    out
}

/// fetch one URL's body (the downloader is replaceable for tests)
pub type Fetch<'a> = &'a dyn Fn(&str) -> Result<Vec<u8>, String>;

/// HTTPS GET with a 30 s timeout and the ProxyDock user agent
pub fn http_get(url: &str) -> Result<Vec<u8>, String> {
    crate::http::get(url, "", Duration::from_secs(30), 64 << 20)
}

fn download(fetch: Fetch, kind: &str, name: &str, dest: &Path) -> Result<(), String> {
    let data = fetch(&url(kind, name).unwrap_or_default())?;
    if !data.starts_with(b"SRS") {
        // sing-box binary rule-set magic
        return Err(tr("not a rule-set file"));
    }
    let tmp = dest.with_extension("srs.tmp");
    std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

fn fill(t: &str, kind: &str, name: &str, e: &str) -> String {
    tr(t).replace("{kind}", kind).replace("{name}", name).replace("{e}", e)
}

/// Make sure every (kind, name) is in `dir` and not older than a day. Returns (tag, path) sorted by kind and name.
/// Err when one is missing and cannot be downloaded; log(message) hears about a failed refresh of a saved copy.
pub fn ensure_in(dir: &Path, items: &[(String, String)], fetch: Fetch, log: &dyn Fn(&str)) -> Result<Vec<(String, String)>, String> {
    let _ = std::fs::create_dir_all(dir);
    let mut out = vec![];
    for (kind, name) in items.iter().collect::<BTreeSet<_>>() {
        if url(kind, name).is_none() || !valid_name(name) {
            return Err(tr("Invalid {kind} name: {name}").replace("{kind}", kind).replace("{name}", name));
        }
        let p = path_in(dir, kind, name);
        let age = std::fs::metadata(&p).and_then(|m| m.modified()).ok().map(|t| SystemTime::now().duration_since(t).unwrap_or_default());
        if age.is_none_or(|a| a >= MAX_AGE) {
            if let Err(e) = download(fetch, kind, name, &p) {
                if !p.exists() {
                    return Err(fill("Could not download {kind} '{name}' for a router rule: {e}", kind, name, &e));
                }
                log(&fill("Could not update {kind} '{name}', using the saved copy: {e}", kind, name, &e));
            }
        }
        out.push((tag(kind, name), p.display().to_string()));
    }
    Ok(out)
}

/// ensure_in with the shared rule-set folder and real downloads
pub fn ensure(items: &[(String, String)], log: &dyn Fn(&str)) -> Result<Vec<(String, String)>, String> {
    ensure_in(&ruleset_dir(), items, &http_get, log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn names() {
        assert!(valid_name("cn") && valid_name("category-ads-all") && valid_name("google@cn") && valid_name("a!b.c_d"));
        assert!(!valid_name("") && !valid_name("-cn") && !valid_name("CN") && !valid_name("a/b") && !valid_name(&"a".repeat(65)));
        assert!(valid_name(&"a".repeat(64)));
    }

    #[test]
    fn downloads_refreshes_and_falls_back() {
        let dir = std::env::temp_dir().join(format!("proxydock-rs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let items = vec![("geosite".to_string(), "cn".to_string()), ("geoip".into(), "cn".into()), ("geoip".into(), "cn".into())];
        let urls = RefCell::new(vec![]);
        let ok = |u: &str| {
            urls.borrow_mut().push(u.to_string());
            Ok(b"SRS\x01data".to_vec())
        };
        let got = ensure_in(&dir, &items, &ok, &|_| {}).unwrap();
        assert_eq!(got.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), ["geoip-cn", "geosite-cn"]);
        assert_eq!(urls.borrow().len(), 2);
        assert!(urls.borrow()[0].ends_with("/sing-geoip/rule-set/geoip-cn.srs"));
        // fresh: no second download
        ensure_in(&dir, &items, &ok, &|_| {}).unwrap();
        assert_eq!(urls.borrow().len(), 2);

        // stale + failing download: the saved copy is used and the failure logged
        let old = SystemTime::now() - Duration::from_secs(2 * 24 * 3600);
        std::fs::File::options().write(true).open(path_in(&dir, "geoip", "cn")).unwrap().set_modified(old).unwrap();
        let logged = RefCell::new(vec![]);
        let bad = |_: &str| Ok(b"<html>".to_vec());
        ensure_in(&dir, &items[1..2], &bad, &|m| logged.borrow_mut().push(m.to_string())).unwrap();
        assert_eq!(logged.borrow().len(), 1);
        assert_eq!(std::fs::read(path_in(&dir, "geoip", "cn")).unwrap(), b"SRS\x01data");

        // missing + failing: an error
        let err = |_: &str| Err("offline".to_string());
        let e = ensure_in(&dir, &[("geoip".into(), "us".into())], &err, &|_| {}).unwrap_err();
        assert!(e.contains("'us'") && e.contains("offline"), "{e}");
        assert!(ensure_in(&dir, &[("geoip".into(), "../x".into())], &err, &|_| {}).is_err());
        assert!(ensure_in(&dir, &[("asn".into(), "x".into())], &err, &|_| {}).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
