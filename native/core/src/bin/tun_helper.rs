//! The administrator helper mode on its own (the tests run it without UAC; the app has the same mode):
//! tun_helper --tun-helper <port> <token> <config> <dir>
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(proxydock_core::singbox::helper::ARG) {
        eprintln!("usage: tun_helper --tun-helper <port> <token> <config> <dir>");
        std::process::exit(2);
    }
    std::process::exit(proxydock_core::singbox::helper::main(&args[1..]));
}
