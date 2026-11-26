//! Writes the app icon as PNG files (the Linux and macOS packages' icons; tools/build_rust.py runs it).
//!     cargo run -p proxydock --example make_png -- <folder> <size>...
//! Each size becomes <folder>/<size>.png.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((dir, sizes)) = args.split_first() else {
        eprintln!("usage: make_png <folder> <size>...");
        std::process::exit(2);
    };
    std::fs::create_dir_all(dir).unwrap();
    for s in sizes {
        let size: u32 = s.parse().expect("a size in pixels");
        let path = std::path::Path::new(dir).join(format!("{size}.png"));
        std::fs::write(&path, proxydock::icon::png(size)).unwrap();
        println!("{}", path.display());
    }
}
