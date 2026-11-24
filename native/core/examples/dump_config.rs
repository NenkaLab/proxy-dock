//! Reads the settings file (never writes it) and prints what the Rust port makes of it, with every secret
//! replaced by the start of its SHA-256 - for comparing with the Python app (native/tools/compare_config.py).
//!
//!     cargo run -p proxydock-core --example dump_config [config.json]

use proxydock_core::{config, paths, secret::SECRET_FIELDS};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

fn mask(m: &Map<String, Value>) -> Value {
    let mut out = m.clone();
    for k in SECRET_FIELDS {
        if let Some(Value::String(s)) = out.get(k) {
            let masked = if s.is_empty() {
                String::new()
            } else {
                let h = Sha256::digest(s.as_bytes());
                format!("sha256:{}", h.iter().take(6).map(|b| format!("{b:02x}")).collect::<String>())
            };
            out.insert(k.into(), json!(masked));
        }
    }
    Value::Object(out)
}

fn main() {
    let path = std::env::args().nth(1).map(Into::into).unwrap_or_else(paths::config_path);
    let loaded = match config::load_from(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}: {e:?}", path.display());
            std::process::exit(1);
        }
    };
    let blocks = |key: &str, field: &str| -> Value {
        Value::Array(
            loaded.graph[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| json!({"id": n["id"], field: mask(n[field].as_object().unwrap())}))
                .collect(),
        )
    };
    let out = json!({
        "global": loaded.global,
        "inbounds": blocks("inbounds", "inbound"),
        "outbounds": blocks("outbounds", "outbound"),
        "needs_save": loaded.notes.needs_save,
        "undecryptable": loaded.notes.undecryptable.iter().map(|(id, f)| format!("{id}.{}", f.field)).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
