//! Reads a settings file (never writes it) and prints the sing-box config of every card, secrets masked - for
//! comparing with the Python app (native/tools/py_dump_singbox.py, parity_singbox.py).
//!
//!     cargo run -p proxydock-core --example dump_singbox -- [config.json] [--lang ko] [--seq]

use proxydock_core::singbox::compile::{self as sb, BuildEnv, BuildError, Reason};
use proxydock_core::{config, graph, i18n, ids, paths, secret};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MASK: [&str; 5] = ["password", "private_key", "pre_shared_key", "uuid", "secret"];

fn mask_all(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if let (true, Value::String(s)) = (MASK.contains(&k.as_str()), &*x) {
                    if !s.is_empty() {
                        let h = Sha256::digest(s.as_bytes());
                        *x = json!(format!(
                            "sha256:{}",
                            h.iter().take(6).map(|b| format!("{b:02x}")).collect::<String>()
                        ));
                    }
                } else {
                    mask_all(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(mask_all),
        _ => {}
    }
}

fn main() {
    let mut path = paths::config_path();
    let mut lang = "en".to_string();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--lang" => lang = it.next().cloned().unwrap_or_default(),
            "--seq" => ids::use_sequence(),
            p => path = p.into(),
        }
    }
    i18n::set_language(&lang);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let raw: Value = serde_json::from_str(&text).expect("JSON");
    let glob = config::validate_global(raw.get("global").unwrap_or(&Value::Null));
    let mut g = raw.get("graph").cloned().unwrap_or(Value::Null);
    if let Value::Object(m) = &mut g {
        for (key, field) in [("inbounds", "inbound"), ("outbounds", "outbound")] {
            if let Some(Value::Array(items)) = m.get_mut(key) {
                items.retain(Value::is_object);
                for item in items.iter_mut() {
                    let (plain, _, _) = secret::unseal(item.get(field).unwrap_or(&Value::Null));
                    item[field] = plain;
                }
            }
        }
    }
    let validated = graph::validate(&g);
    let mut out = vec![];
    for p in graph::compile(&validated) {
        let p = Value::Object(p);
        let cfg = Value::Object(config::resolve(&p, &glob));
        // the GeoIP / Geosite rule-sets the routes use (the app fetches these files)
        let mut rulesets: Vec<(String, String)> = vec![];
        for r in p["routes"].as_array().unwrap() {
            for m in r["match"].as_array().unwrap() {
                for kind in ["geoip", "geosite"] {
                    for name in m.get(kind).and_then(Value::as_array).into_iter().flatten() {
                        let t = sb::ruleset_tag(kind, name.as_str().unwrap_or(""));
                        if !rulesets.iter().any(|(x, _)| *x == t) {
                            rulesets.push((t.clone(), format!("RS/{t}")));
                        }
                    }
                }
            }
        }
        let exclude: Vec<String> = if p["inbound"]["type"] == json!("tun") {
            vec!["E/sing-box.exe".into(), "E/hysteria.exe".into()]
        } else {
            vec![]
        };
        let env = BuildEnv {
            bans_path: "BANS",
            api_port: 9090,
            api_secret: "S",
            cert: Some(("C", "K")),
            rulesets: &rulesets,
            exclude_paths: &exclude,
        };
        let entry = match sb::build(&p, &cfg, &env) {
            Ok((mut conf, reasons)) => {
                mask_all(&mut conf);
                json!({"id": p["id"], "config": conf, "reasons": reasons.iter().map(Reason::to_json).collect::<Vec<_>>()})
            }
            Err(BuildError::Missing(_)) => json!({"id": p["id"], "error": "KeyError"}),
            Err(BuildError::Unsupported(_)) => json!({"id": p["id"], "error": "ValueError"}),
        };
        out.push(entry);
    }
    println!("{}", serde_json::to_string_pretty(&Value::Array(out)).unwrap());
}
