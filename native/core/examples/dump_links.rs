//! Share links and subscription bodies read the way the app reads them, for the parity check against Python
//! (tools/parity_links.py). Input: a JSON list of {"kind": "link" | "sub", "text": ...}; output: one result each.

use serde_json::{json, Value};

fn main() {
    let path = std::env::args().nth(1).expect("input file");
    let lang = std::env::args().nth(2).unwrap_or_else(|| "en".into());
    proxydock_core::i18n::set_language(&lang);
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let out: Vec<Value> = cases
        .iter()
        .map(|c| {
            let text = c["text"].as_str().unwrap_or("");
            if c["kind"] == "link" {
                match proxydock_core::links::parse(text) {
                    Ok((fields, name)) => json!({"ok": true, "fields": fields, "name": name}),
                    Err(e) => json!({"ok": false, "error": e}),
                }
            } else {
                match proxydock_core::subscription::parse(text.as_bytes()) {
                    Ok((servers, skipped)) => {
                        let (f, note) = proxydock_core::subscription::fields_of(&servers, skipped, 0);
                        json!({"ok": true, "sub_servers": f["sub_servers"], "note": note})
                    }
                    Err(e) => json!({"ok": false, "error": e}),
                }
            }
        })
        .collect();
    println!("{}", serde_json::to_string(&out).unwrap());
}
