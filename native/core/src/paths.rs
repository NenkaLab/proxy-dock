//! Settings folder and files (Python: `config.app_dir`, `CONFIG_PATH`, ...).

use std::path::PathBuf;

pub const APP_ID: &str = "ProxyDock";
/// settings folder name of the version before ProxyDock (SOCKS5 Proxy)
pub const OLD_APP_ID: &str = "ProxyLocal";

/// Windows `%APPDATA%\<name>`, macOS `~/Library/Application Support/<name>`, elsewhere `$XDG_CONFIG_HOME/<name>`
/// (default `~/.config`).
pub fn app_dir_named(name: &str) -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_default();
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home)
    } else if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"))
    };
    base.join(name)
}

/// the settings folder; PROXYDOCK_HOME replaces it (tests, a portable copy)
pub fn app_dir() -> PathBuf {
    match std::env::var_os("PROXYDOCK_HOME") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => app_dir_named(APP_ID),
    }
}

/// the previous version's (SOCKS5 Proxy) settings folder; None with PROXYDOCK_HOME (tests, a portable copy: never
/// the user's real folder)
pub fn old_app_dir() -> Option<PathBuf> {
    match std::env::var_os("PROXYDOCK_HOME") {
        Some(p) if !p.is_empty() => None,
        _ => Some(app_dir_named(OLD_APP_ID)),
    }
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

pub fn log_dir() -> PathBuf {
    app_dir().join("logs")
}

pub fn cert_dir() -> PathBuf {
    app_dir().join("certs")
}
