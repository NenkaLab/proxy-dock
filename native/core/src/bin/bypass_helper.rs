//! The VPN bypass helper mode on its own (examples / tests start it; the app has the same mode):
//! bypass_helper --bypass-helper <port> <token> <dir>
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(proxydock_core::bypass::helper::ARG) {
        eprintln!("usage: bypass_helper --bypass-helper <port> <token> <dir>");
        std::process::exit(2);
    }
    std::process::exit(proxydock_core::bypass::helper::main(&args[1..]));
}
