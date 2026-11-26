//! Reads a settings file (never writes it) and prints the validated graph, secrets as the start of their
//! SHA-256 - for comparing with the Python app (native/tools/py_dump_graph.py, parity_graph.py).
//!
//!     cargo run -p proxydock-core --example dump_graph -- [config.json] [--lang ko] [--seq] [--compile]
//!
//! --compile: print graph::compile() of the validated graph (the runtime proxies) instead.
//!
//! --seq: new ids are id000001, id000002, ... (the Python dump does the same) so they can be compared.

use proxydock_core::{graph, i18n, ids, paths, secret};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// every secret field anywhere in v (nested via chains, group members, routes)
fn mask_all(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for x in m.values_mut() {
                mask_all(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(mask_all),
        _ => return,
    }
    mask(v);
}

fn mask(v: &mut Value) {
    let Value::Object(m) = v else { return };
    for k in secret::SECRET_FIELDS {
        if let Some(Value::String(s)) = m.get(k) {
            let masked = if s.is_empty() {
                String::new()
            } else {
                let h = Sha256::digest(s.as_bytes());
                format!("sha256:{}", h.iter().take(6).map(|b| format!("{b:02x}")).collect::<String>())
            };
            m.insert(k.into(), json!(masked));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut path = paths::config_path();
    let mut lang = "en".to_string();
    let mut compile = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--lang" => lang = it.next().cloned().unwrap_or_default(),
            "--seq" => ids::use_sequence(),
            "--compile" => compile = true,
            p => path = p.into(),
        }
    }
    i18n::set_language(&lang);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let raw: Value = serde_json::from_str(&text).expect("JSON");
    let mut g = raw.get("graph").cloned().unwrap_or(Value::Null);
    if let Value::Object(m) = &mut g {
        for (key, field) in [("inbounds", "inbound"), ("outbounds", "outbound")] {
            if let Some(Value::Array(items)) = m.get_mut(key) {
                items.retain(Value::is_object); // (config.load keeps only dicts here)
                for item in items.iter_mut() {
                    if let Value::Object(n) = item {
                        let (plain, _, _) = secret::unseal(n.get(field).unwrap_or(&Value::Null));
                        n.insert(field.into(), plain);
                    }
                }
            }
        }
    }
    let validated = graph::validate(&g);
    if compile {
        let mut out = Value::Array(graph::compile(&validated).into_iter().map(Value::Object).collect());
        mask_all(&mut out);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return;
    }
    let mut out = Value::Object(validated);
    for (key, field) in [("inbounds", "inbound"), ("outbounds", "outbound")] {
        for n in out[key].as_array_mut().unwrap() {
            mask(&mut n[field]);
        }
    }
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
