//! stdin: one JSON list of "#rrggbb" per line; stdout: proxydock_core::colors::mix_colors of each (parity tests)
use std::io::BufRead;

fn main() {
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let colors: Vec<String> = serde_json::from_str(&line).unwrap_or_default();
        println!("{}", proxydock_core::colors::mix_colors(&colors));
    }
}
