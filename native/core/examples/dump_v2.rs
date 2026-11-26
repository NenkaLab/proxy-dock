//! Settings v2 files and the previous version's (SOCKS5 Proxy) settings, read the way the app reads them, for the
//! parity check against Python (tools/parity_v2.py). Input: a JSON list of {"kind": "v2" | "old", "text": ...};
//! output: for each the global settings and the (validated) graph, or null.

use proxydock_core::{config, graph};
use serde_json::{Value, json};

fn main() {
    let path = std::env::args().nth(1).expect("input file");
    proxydock_core::i18n::set_language(&std::env::args().nth(2).unwrap_or_else(|| "en".into()));
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let tmp = std::env::temp_dir().join(format!("pd-dump-v2-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let out: Vec<Value> = cases
        .iter()
        .map(|c| {
            let text = c["text"].as_str().unwrap_or("");
            let got = if c["kind"] == "v2" {
                config::parse(text).ok().map(|l| (l.global, l.graph))
            } else {
                std::fs::write(tmp.join("config.json"), text).unwrap();
                config::migrate_from_old(&tmp).map(|(g, gr, _)| (g, gr))
            };
            match got {
                Some((g, gr)) => json!({"global": g, "graph": graph::validate(&Value::Object(gr))}),
                None => Value::Null,
            }
        })
        .collect();
    let _ = std::fs::remove_dir_all(&tmp);
    println!("{}", serde_json::to_string(&out).unwrap());
}
