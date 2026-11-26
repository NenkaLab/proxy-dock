//! Duplicate and rename on a graph, the way the app does them, for the parity check against Python
//! (tools/parity_edit.py). Input: a JSON list of {"graph": ..., "ops": [["duplicate", [keys]] | ["rename", key, name]]};
//! output: for each case the graph after its ops and what each op returned.

use serde_json::{Value, json};

fn main() {
    let path = std::env::args().nth(1).expect("input file");
    let lang = std::env::args().nth(2).unwrap_or_else(|| "en".into());
    proxydock_core::i18n::set_language(&lang);
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let out: Vec<Value> = cases
        .iter()
        .map(|c| {
            // the graph as the Python side validated it (validating twice changes some names)
            let mut g = c["graph"].as_object().cloned().unwrap_or_default();
            let mut results = vec![];
            for op in c["ops"].as_array().into_iter().flatten() {
                match op[0].as_str().unwrap_or("") {
                    "duplicate" => {
                        let keys: Vec<&str> =
                            op[1].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                        results.push(json!(proxydock_core::edit::duplicate(&mut g, &keys).len()));
                    }
                    _ => {
                        let r = proxydock_core::edit::rename(
                            &mut g,
                            op[1].as_str().unwrap_or(""),
                            op[2].as_str().unwrap_or(""),
                        );
                        results.push(json!(r));
                    }
                }
            }
            json!({"graph": g, "results": results})
        })
        .collect();
    println!("{}", serde_json::to_string(&out).unwrap());
}
