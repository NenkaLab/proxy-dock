//! Writes app/icons/proxydock.ico (the exe files' icon) from the drawing in `proxydock::icon`.
//!     cargo run -p proxydock --example make_ico

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("icons").join("proxydock.ico");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, proxydock::icon::ico(&[16, 20, 24, 32, 40, 48, 64, 256])).unwrap();
    println!("{}", path.display());
}
