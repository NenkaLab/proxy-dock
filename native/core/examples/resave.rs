//! resave <settings in> <settings out> [--seq]: read a settings file and write it back like the app does
//! (validated graph, secrets sealed) - for comparing with the Python app's save (tools/parity_save.py).
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--seq") {
        proxydock_core::ids::use_sequence();
    }
    let l = proxydock_core::config::load_from(std::path::Path::new(&args[0])).expect("load");
    let g = proxydock_core::graph::validate(&serde_json::Value::Object(l.graph));
    proxydock_core::config::save_to(std::path::Path::new(&args[1]), &l.global, &g).expect("save");
}
