//! fetch_latest [dir]: download the newest sing-box release (alpha / beta included) like the app's experimental
//! setting does, into dir (default: the app's engines folder), and print the result.
fn main() {
    use proxydock_core::singbox::latest;
    let dir = std::env::args().nth(1).map(std::path::PathBuf::from).unwrap_or_else(latest::engines_dir);
    println!("{:?}", latest::update_in(&dir, &latest::http_fetch, true));
    println!("{:?}", latest::installed_in(&dir));
}
