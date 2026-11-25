//! Subscriptions (Python: `subscription.py`): a URL that returns a provider's server list - share links, one per
//! line, usually base64-encoded as a whole, or a Clash YAML file. `update` downloads and reads it into the fields
//! of the subscription block.

use crate::i18n::tr;
use serde_json::{json, Map, Value};
use std::time::Duration;

pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);

/// Download a subscription (a readable error otherwise).
pub fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let lower = url.to_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return Err(tr("The subscription address must start with https:// or http://"));
    }
    let data = crate::http::get(url, "", TIMEOUT, MAX_BYTES + 1).map_err(|e| tr("Could not download the subscription: {e}").replace("{e}", &e))?;
    if data.len() as u64 > MAX_BYTES {
        return Err(tr("The subscription is too large"));
    }
    Ok(data)
}

/// `data.decode("utf-8", errors="replace").strip().lstrip("﻿")`
fn decoded(data: &[u8]) -> String {
    let t = String::from_utf8_lossy(data);
    crate::py::strip(&t).trim_start_matches('\u{feff}').to_string()
}

/// the share links: the text itself, or the whole list base64-encoded (standard or URL-safe)
fn text_of(data: &[u8]) -> Result<String, String> {
    let text = decoded(data);
    if text.contains("://") {
        return Ok(text);
    }
    let compact: String = text.split(crate::py::is_space).collect();
    if compact.is_ascii() {
        let padded = format!("{compact}{}", "=".repeat((4 - compact.len() % 4) % 4));
        let raw = if compact.contains(['-', '_']) { crate::py::urlsafe_b64decode(&padded) } else { crate::py::a2b_base64(padded.as_bytes()) };
        if let Some(raw) = raw {
            let d = String::from_utf8_lossy(&raw).to_string();
            if d.contains("://") {
                return Ok(d);
            }
        }
    }
    Err(tr("No servers found (expected share links or a Clash YAML file)"))
}

type Entries = (Vec<(Map<String, Value>, String)>, usize);

fn entries(data: &[u8]) -> Result<Entries, String> {
    let raw = decoded(data);
    if crate::clash::looks_like_clash(&raw) {
        return crate::clash::proxies(&raw);
    }
    let (mut out, mut skipped) = (vec![], 0);
    for line in crate::py::splitlines(&text_of(data)?) {
        let line = crate::py::strip(line);
        if line.is_empty() || !line.contains("://") {
            continue;
        }
        match crate::links::parse(line) {
            Ok(e) => out.push(e),
            Err(_) => skipped += 1,
        }
    }
    Ok((out, skipped))
}

/// -> (servers [{"name", "outbound"}], number of entries that were not usable)
pub fn parse(data: &[u8]) -> Result<(Vec<Value>, usize), String> {
    let (list, mut skipped) = entries(data)?;
    let mut servers = vec![];
    let mut names = std::collections::HashSet::new();
    for (fields, mut name) in list {
        let mut ob = crate::config::outbound_defaults();
        for (k, v) in fields {
            ob.insert(k, v);
        }
        let mut check = ob.clone();
        check.insert("via".into(), Value::Null);
        if !crate::checks::outbound_errors(&Value::Object(check)).is_empty() {
            skipped += 1;
            continue;
        }
        let head = crate::py::head(&name, 60);
        let base = if head.is_empty() { ob["type"].as_str().unwrap_or("").to_string() } else { head };
        let mut i = 2;
        while names.contains(&name) {
            name = format!("{base} ({i})");
            i += 1;
        }
        names.insert(name.clone());
        servers.push(json!({"name": name, "outbound": ob}));
    }
    if servers.is_empty() {
        return Err(tr("No usable servers in the subscription"));
    }
    Ok((servers, skipped))
}

/// the fields to put into the block, and a note ("12 servers · 2 skipped")
pub fn fields_of(servers: &[Value], skipped: usize, now: i64) -> (Map<String, Value>, String) {
    let mut note = tr("{count} servers").replace("{count}", &servers.len().to_string());
    if skipped > 0 {
        note += &format!(" · {}", tr("{count} skipped (unsupported or invalid)").replace("{count}", &skipped.to_string()));
    }
    let mut f = Map::new();
    f.insert("sub_servers".into(), json!(crate::config::py_dumps(&Value::Array(servers.to_vec()))));
    f.insert("sub_updated".into(), json!(now));
    (f, note)
}

/// Fetch and read the subscription at `url`.
pub fn update(url: &str) -> Result<(Map<String, Value>, String), String> {
    let (servers, skipped) = parse(&fetch(crate::py::strip(url))?)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    Ok(fields_of(&servers, skipped, now))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_list() {
        use base64::Engine;
        let list = "trojan://pw@a.com:443#A\nhttp://not\nvless://u@b.com:1?type=kcp#B\ntrojan://pw@a.com:443#A\n";
        let data = base64::engine::general_purpose::STANDARD.encode(list);
        let (servers, skipped) = parse(data.as_bytes()).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[1]["name"], "A (2)");
        assert_eq!(skipped, 2, "http:// and the kcp transport");
        let (f, _) = fields_of(&servers, skipped, 5);
        assert!(f["sub_servers"].as_str().unwrap().starts_with("[{\"name\": \"A\", \"outbound\": {\"type\": \"trojan\", "));
        assert!(parse(b"nothing here").is_err());
    }
}
