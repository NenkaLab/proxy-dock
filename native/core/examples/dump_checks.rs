//! dump_checks <settings> [--lang ko]: the problems the app reports for every card (check_proxy) and every router
//! rule (match_errors), as JSON - for comparing with tools/py_dump_checks.py.
use serde_json::{Map, Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--lang") {
        proxydock_core::i18n::set_language(&args[i + 1]);
    }
    proxydock_core::ids::use_sequence();
    let l = proxydock_core::config::load_from(std::path::Path::new(&args[0])).expect("load");
    let g = proxydock_core::graph::validate(&Value::Object(l.graph));
    let proxies: Vec<Value> = proxydock_core::graph::compile(&g).into_iter().map(Value::Object).collect();
    let mut cards = Map::new();
    for p in &proxies {
        let others: Vec<Value> = proxies.iter().filter(|o| o["id"] != p["id"]).cloned().collect();
        cards.insert(p["id"].as_str().unwrap().into(), json!(proxydock_core::checks::check_proxy(p, &others)));
    }
    let mut rules = Map::new();
    for r in g["routers"].as_array().into_iter().flatten() {
        for (i, x) in r["rules"].as_array().into_iter().flatten().enumerate() {
            let name =
                x["name"].as_str().filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| (i + 1).to_string());
            rules.insert(
                x["id"].as_str().unwrap().into(),
                json!(proxydock_core::checks::match_errors(&name, &x["match"])),
            );
        }
    }
    println!("{}", serde_json::to_string_pretty(&json!({"cards": cards, "rules": rules})).unwrap());
}
