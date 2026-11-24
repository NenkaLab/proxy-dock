//! make_cert <dir> <name>: write a self-signed pair like the app does (for checking it from Python)
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (c, k) = proxydock_core::tls::ensure_self_signed(std::path::Path::new(&a[1]), &a[2]).unwrap();
    println!("{}\n{}", c.display(), k.display());
}
