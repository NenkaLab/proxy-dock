//! OS integration (Python `platform_utils`): start at login (Windows: the HKCU Run value, macOS: a LaunchAgent,
//! Linux: an XDG autostart entry), the system proxy of the PC block (Windows WinINet) and the Windows Firewall
//! rule that blocks other PCs (the last two are Windows only, as in the Python app).
//!
//! Tests point the registry keys and the login-item folder somewhere else (`set_test_keys`, `set_test_dir`), so
//! they never change the real settings.

use crate::i18n::tr;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub const APP_ID: &str = crate::paths::APP_ID;
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const INET_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
/// the WinINet values the PC block changes (and backs up)
pub const INET_VALUES: [&str; 4] = ["ProxyEnable", "ProxyServer", "ProxyOverride", "AutoConfigURL"];
/// inbounds Windows can use as the system proxy (WinINet's "socks=" is SOCKS4)
pub const PC_INBOUND_TYPES: [&str; 2] = ["http", "socks4"];
pub const FW_RULE: &str = "ProxyDock - Block External";
/// rule names of earlier versions (removed with ours)
pub const FW_OLD_RULES: [&str; 5] = [
    "SOCKS5 Proxy - Block External",
    "Block SOCKS5 Proxy External",
    "Allow SOCKS5 Proxy Local",
    "Block SOCKS5 Proxy External UDP",
    "Allow SOCKS5 Proxy Local UDP",
];

/// (Run key, Internet Settings key) in HKCU; tests replace them
static KEYS: RwLock<Option<(String, String)>> = RwLock::new(None);

/// Tests: use these HKCU keys instead of the real Run and Internet Settings keys.
pub fn set_test_keys(run: &str, inet: &str) {
    *KEYS.write().unwrap() = Some((run.into(), inet.into()));
}

#[cfg(windows)]
fn run_key() -> String {
    KEYS.read().unwrap().as_ref().map_or(RUN_KEY.into(), |k| k.0.clone())
}

#[cfg(windows)]
fn inet_key() -> String {
    KEYS.read().unwrap().as_ref().map_or(INET_KEY.into(), |k| k.1.clone())
}

/// the system proxy and the firewall rule (Windows only, as in the Python app)
pub fn supported() -> bool {
    cfg!(windows)
}

/// start at login (Windows, macOS, Linux)
pub fn startup_supported() -> bool {
    cfg!(any(windows, target_os = "macos", target_os = "linux"))
}

/// Tests: the folder that stands for ~/Library/LaunchAgents (macOS) or ~/.config/autostart (Linux)
static TEST_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

pub fn set_test_dir(dir: &Path) {
    *TEST_DIR.write().unwrap() = Some(dir.to_path_buf());
}

/// the login-item file of `name` ("current" or the previous version's): macOS LaunchAgent plist, Linux autostart
/// .desktop (Python `MAC_AGENT`, `LINUX_AUTOSTART` and the OLD_ ones)
#[cfg(not(windows))]
fn login_item(old: bool) -> Option<PathBuf> {
    let mac = cfg!(target_os = "macos");
    let file = match (mac, old) {
        (true, false) => "com.proxydock.app.plist",
        (true, true) => "com.proxylocal.socks5.plist",
        (false, false) => "proxydock.desktop",
        (false, true) => "proxylocal.desktop",
    };
    if let Some(d) = TEST_DIR.read().unwrap().as_ref() {
        return Some(d.join(file));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = if mac {
        home.join("Library").join("LaunchAgents")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
            .join("autostart")
    };
    Some(dir.join(file))
}

/// the program a login item starts: inside an AppImage the AppImage file itself (the running program lives in a
/// mount that is gone after a restart)
pub fn launch_exe(exe: &Path) -> PathBuf {
    match std::env::var_os("APPIMAGE") {
        Some(a) if cfg!(target_os = "linux") && !a.is_empty() => PathBuf::from(a),
        _ => exe.to_path_buf(),
    }
}

/// the login item's text: a LaunchAgent plist (macOS) or an autostart .desktop entry (Linux)
pub fn login_item_text(exe: &Path, hidden: bool, mac: bool) -> String {
    let mut argv = vec![launch_exe(exe).display().to_string()];
    if hidden {
        argv.push("--hidden".into());
    }
    if mac {
        let esc = |a: &str| a.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let args: String = argv.iter().map(|a| format!("<string>{}</string>", esc(a))).collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key>\
             <string>com.proxydock.app</string><key>ProgramArguments</key><array>{args}</array><key>RunAtLoad</key>\
             <true/></dict></plist>\n"
        )
    } else {
        // (Python shlex.quote: quoted when the argument has anything but safe characters)
        let quote = |a: &str| {
            if !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c)) {
                a.to_string()
            } else {
                format!("'{}'", a.replace('\'', "'\"'\"'"))
            }
        };
        let exec: Vec<String> = argv.iter().map(|a| quote(a)).collect();
        format!(
            "[Desktop Entry]\nType=Application\nName=ProxyDock\nExec={}\nX-GNOME-Autostart-enabled=true\nTerminal=false\n",
            exec.join(" ")
        )
    }
}

// --- the registry (HKCU) ---

#[cfg(windows)]
mod reg {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }

    pub fn open(path: &str, write: bool) -> std::io::Result<Key> {
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe {
            if write {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    wide(path).as_ptr(),
                    0,
                    std::ptr::null(),
                    0,
                    KEY_READ | KEY_SET_VALUE,
                    std::ptr::null(),
                    &mut h,
                    std::ptr::null_mut(),
                )
            } else {
                RegOpenKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, KEY_READ, &mut h)
            }
        };
        if rc != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(rc as i32));
        }
        Ok(Key(h))
    }

    /// a value: Some(string) for REG_SZ / REG_EXPAND_SZ, Some(number) for REG_DWORD, None when missing
    pub fn get(k: &Key, name: &str) -> Option<serde_json::Value> {
        let name = wide(name);
        let (mut ty, mut len) = (0u32, 0u32);
        if unsafe { RegQueryValueExW(k.0, name.as_ptr(), std::ptr::null(), &mut ty, std::ptr::null_mut(), &mut len) }
            != ERROR_SUCCESS
        {
            return None;
        }
        let mut buf = vec![0u8; len as usize + 2];
        if unsafe { RegQueryValueExW(k.0, name.as_ptr(), std::ptr::null(), &mut ty, buf.as_mut_ptr(), &mut len) }
            != ERROR_SUCCESS
        {
            return None;
        }
        buf.truncate(len as usize);
        match ty {
            REG_DWORD if buf.len() >= 4 => {
                Some(serde_json::json!(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])))
            }
            REG_SZ | REG_EXPAND_SZ => {
                let w: Vec<u16> = buf
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|c| *c != 0)
                    .collect();
                Some(serde_json::json!(String::from_utf16_lossy(&w)))
            }
            _ => None,
        }
    }

    pub fn set_str(k: &Key, name: &str, v: &str) -> std::io::Result<()> {
        let data: Vec<u8> = wide(v).iter().flat_map(|c| c.to_le_bytes()).collect();
        let rc = unsafe { RegSetValueExW(k.0, wide(name).as_ptr(), 0, REG_SZ, data.as_ptr(), data.len() as u32) };
        if rc != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(rc as i32));
        }
        Ok(())
    }

    pub fn set_dword(k: &Key, name: &str, v: u32) -> std::io::Result<()> {
        let rc = unsafe { RegSetValueExW(k.0, wide(name).as_ptr(), 0, REG_DWORD, v.to_le_bytes().as_ptr(), 4) };
        if rc != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(rc as i32));
        }
        Ok(())
    }

    pub fn delete(k: &Key, name: &str) {
        unsafe { RegDeleteValueW(k.0, wide(name).as_ptr()) };
    }
}

// --- start at login (Windows: HKCU Run value "ProxyDock") ---

/// the command that starts this program at login (Python `_launch_argv`)
pub fn launch_command(exe: &Path, hidden: bool) -> String {
    let mut cmd = format!("\"{}\"", exe.display());
    if hidden {
        cmd += " --hidden";
    }
    cmd
}

/// the registered start-at-login command, if any
pub fn startup_command() -> Option<String> {
    #[cfg(windows)]
    {
        let k = reg::open(&run_key(), false).ok()?;
        reg::get(&k, APP_ID).and_then(|v| v.as_str().map(String::from))
    }
    #[cfg(not(windows))]
    login_item(false).and_then(|p| std::fs::read_to_string(p).ok())
}

/// (registered, hidden) (Python `is_startup_registered` / `is_startup_hidden`)
pub fn startup_state() -> (bool, bool) {
    match startup_command() {
        Some(c) => (true, c.contains("--hidden")),
        None => (false, false),
    }
}

/// Register (or remove) start at login for `exe` (Python `update_startup_registry`).
pub fn set_startup(exe: &Path, register: bool, hidden: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        let k = reg::open(&run_key(), true).map_err(|e| e.to_string())?;
        if register {
            reg::set_str(&k, APP_ID, &launch_command(exe, hidden)).map_err(|e| e.to_string())?;
        } else {
            reg::delete(&k, APP_ID);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let Some(path) = login_item(false) else { return Err(tr("Windows only for now")) };
        if !register {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            };
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, login_item_text(exe, hidden, cfg!(target_os = "macos"))).map_err(|e| e.to_string())
    }
}

/// the previous version's (SOCKS5 Proxy) start-at-login value name
pub const OLD_STARTUP_NAME: &str = "SOCKS5 Proxy";

/// Python `migrate_autostart`: the previous version's start-at-login entry is removed and ProxyDock registered
/// instead (keeping a hidden start). True when it was moved.
pub fn migrate_autostart(exe: &Path) -> bool {
    #[cfg(windows)]
    {
        let Ok(k) = reg::open(&run_key(), false) else { return false };
        let Some(old) = reg::get(&k, OLD_STARTUP_NAME).and_then(|v| v.as_str().map(String::from)) else { return false };
        drop(k);
        let Ok(k) = reg::open(&run_key(), true) else { return false };
        reg::delete(&k, OLD_STARTUP_NAME);
        set_startup(exe, true, old.contains("--hidden")).is_ok()
    }
    #[cfg(not(windows))]
    {
        let Some(old_path) = login_item(true) else { return false };
        let Ok(old) = std::fs::read_to_string(&old_path) else { return false };
        if std::fs::remove_file(&old_path).is_err() {
            return false;
        }
        set_startup(exe, true, old.contains("--hidden")).is_ok()
    }
}

// --- the system proxy (the PC block) ---

/// (ProxyServer value, error) for using this inbound as the system proxy (Python `system_proxy_server`)
pub fn system_proxy_server(inbound: &Value) -> Result<String, String> {
    let t = inbound["type"].as_str().unwrap_or("");
    if cfg!(windows) && !PC_INBOUND_TYPES.contains(&t) {
        return Err(tr(
            "Windows can only use an HTTP (or SOCKS4) inbound as the system proxy - add an HTTP inbound for the PC",
        ));
    }
    // (macOS and Linux desktops also take a SOCKS5 proxy)
    if !cfg!(windows) && !["http", "socks4", "socks5"].contains(&t) {
        return Err(tr("The system proxy needs an HTTP or SOCKS inbound - add one for the PC"));
    }
    let host = match inbound["host"].as_str().unwrap_or("") {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        h => h,
    };
    let port = inbound["port"].as_i64().unwrap_or(0);
    let hp = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    Ok(if t == "http" { hp } else { format!("socks={hp}") })
}

/// the current WinINet values (missing = null) (Python `get_system_proxy`)
pub fn get_system_proxy() -> Result<Map<String, Value>, String> {
    #[cfg(windows)]
    {
        let k = reg::open(&inet_key(), false).map_err(|e| e.to_string())?;
        Ok(INET_VALUES.iter().map(|n| (n.to_string(), reg::get(&k, n).unwrap_or(Value::Null))).collect())
    }
    #[cfg(not(windows))]
    get_system_proxy_unix()
}

/// Write WinINet values (null = delete; only the names given) and tell running apps (Python `set_system_proxy`)
pub fn set_system_proxy(values: &Map<String, Value>) -> Result<(), String> {
    #[cfg(windows)]
    {
        let k = reg::open(&inet_key(), true).map_err(|e| e.to_string())?;
        for name in INET_VALUES {
            let Some(v) = values.get(name) else { continue };
            match v {
                Value::Null => reg::delete(&k, name),
                _ if name == "ProxyEnable" => {
                    reg::set_dword(&k, name, v.as_u64().unwrap_or(0) as u32).map_err(|e| e.to_string())?
                }
                Value::String(s) => reg::set_str(&k, name, s).map_err(|e| e.to_string())?,
                other => reg::set_str(&k, name, &other.to_string()).map_err(|e| e.to_string())?,
            }
        }
        if KEYS.read().unwrap().is_none() {
            notify_inet_change();
        }
        Ok(())
    }
    #[cfg(not(windows))]
    set_system_proxy_unix(values)
}

#[cfg(windows)]
fn notify_inet_change() {
    use windows_sys::Win32::Networking::WinInet::{
        INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, InternetSetOptionW,
    };
    unsafe {
        InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_SETTINGS_CHANGED, std::ptr::null(), 0);
        InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_REFRESH, std::ptr::null(), 0);
    }
}

/// The values to apply: our server, local addresses bypassed (the user's own exceptions kept), no PAC script
/// (Python `proxy_values_for`)
pub fn proxy_values_for(server: &str, backup: &Value) -> Map<String, Value> {
    let mut bypass: Vec<String> = vec!["localhost".into(), "127.*".into(), "<local>".into()];
    let old = backup["ProxyOverride"].as_str().unwrap_or("");
    for item in old.split(';') {
        let item = crate::py::strip(item);
        if !item.is_empty() && !bypass.iter().any(|b| b == item) {
            let at = bypass.len() - 1;
            bypass.insert(at, item.to_string());
        }
    }
    let mut m = Map::new();
    m.insert("ProxyEnable".into(), json!(1));
    m.insert("ProxyServer".into(), json!(server));
    m.insert("ProxyOverride".into(), json!(bypass.join(";")));
    m.insert("AutoConfigURL".into(), Value::Null);
    m
}

// --- the system proxy on macOS (networksetup) and Linux (GNOME gsettings, KDE kioslaverc) ---
//
// The same four values as WinINet's, so the PC block keeps one backup format: ProxyEnable (0/1), ProxyServer
// ("host:port" for an HTTP proxy, "socks=host:port" for SOCKS), ProxyOverride (bypass entries joined by ";") and
// AutoConfigURL (a PAC address or null). The command lines are made and the answers read by plain functions
// (tested on every system); only `run` touches the system.

/// "host:port" / "socks=host:port" / "[v6]:port" -> (socks, host, port)
pub fn parse_server(s: &str) -> Option<(bool, String, u16)> {
    let (socks, hp) = match s.strip_prefix("socks=") {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (host, port) = hp.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((socks, host.to_string(), port.parse().ok()?))
}

/// WinINet bypass entries in a system's own words: "<local>" (plain host names) and "127.*"
fn bypass_items(values: &Map<String, Value>, mac: bool) -> Vec<String> {
    let mut out = vec![];
    for item in values.get("ProxyOverride").and_then(Value::as_str).unwrap_or("").split(';') {
        let item = item.trim();
        let mapped = match item {
            "" => continue,
            "<local>" if mac => "*.local".to_string(),
            "<local>" => continue,
            "127.*" => "127.0.0.0/8".to_string(),
            other => other.to_string(),
        };
        if !out.contains(&mapped) {
            out.push(mapped);
        }
    }
    out
}

fn arg(s: &str) -> String {
    s.to_string()
}

/// macOS: the networksetup commands that put `values` on each network service
pub fn mac_set_commands(services: &[String], values: &Map<String, Value>) -> Vec<Vec<String>> {
    let on = values.get("ProxyEnable").and_then(Value::as_u64) == Some(1);
    let server = values.get("ProxyServer").and_then(Value::as_str).and_then(parse_server);
    let pac = values.get("AutoConfigURL").and_then(Value::as_str).filter(|u| !u.is_empty());
    let bypass = bypass_items(values, true);
    let mut cmds = vec![];
    for svc in services {
        let ns = |a: &[&str]| -> Vec<String> {
            std::iter::once("networksetup".to_string()).chain(a.iter().map(|x| arg(x))).collect()
        };
        match (on, &server) {
            (true, Some((false, host, port))) => {
                let p = port.to_string();
                cmds.push(ns(&["-setwebproxy", svc, host, &p]));
                cmds.push(ns(&["-setsecurewebproxy", svc, host, &p]));
                cmds.push(ns(&["-setsocksfirewallproxystate", svc, "off"]));
            }
            (true, Some((true, host, port))) => {
                cmds.push(ns(&["-setsocksfirewallproxy", svc, host, &port.to_string()]));
                cmds.push(ns(&["-setwebproxystate", svc, "off"]));
                cmds.push(ns(&["-setsecurewebproxystate", svc, "off"]));
            }
            _ => {
                cmds.push(ns(&["-setwebproxystate", svc, "off"]));
                cmds.push(ns(&["-setsecurewebproxystate", svc, "off"]));
                cmds.push(ns(&["-setsocksfirewallproxystate", svc, "off"]));
            }
        }
        if values.contains_key("ProxyOverride") {
            let mut c = ns(&["-setproxybypassdomains", svc]);
            if bypass.is_empty() {
                c.push("Empty".into());
            } else {
                c.extend(bypass.iter().cloned());
            }
            cmds.push(c);
        }
        if values.contains_key("AutoConfigURL") {
            match pac {
                Some(u) => cmds.push(ns(&["-setautoproxyurl", svc, u])),
                None => cmds.push(ns(&["-setautoproxystate", svc, "off"])),
            }
        }
    }
    cmds
}

/// networksetup's "Enabled: Yes / Server: x / Port: n" answer -> (enabled, host, port)
fn mac_field(out: &str, key: &str) -> String {
    out.lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':')).map(|r| r.trim().to_string()))
        .unwrap_or_default()
}

/// macOS: the four values from networksetup's answers for one service (-getwebproxy, -getsocksfirewallproxy,
/// -getproxybypassdomains, -getautoproxyurl)
pub fn mac_values(web: &str, socks: &str, bypass: &str, auto: &str) -> Map<String, Value> {
    let mut m = Map::new();
    let hp = |out: &str| format!("{}:{}", mac_field(out, "Server"), mac_field(out, "Port"));
    let (server, enabled) = if mac_field(web, "Enabled") == "Yes" {
        (json!(hp(web)), 1)
    } else if mac_field(socks, "Enabled") == "Yes" {
        (json!(format!("socks={}", hp(socks))), 1)
    } else {
        (Value::Null, 0)
    };
    m.insert("ProxyEnable".into(), json!(enabled));
    m.insert("ProxyServer".into(), server);
    let items: Vec<&str> =
        bypass.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("There aren't")).collect();
    m.insert("ProxyOverride".into(), if items.is_empty() { Value::Null } else { json!(items.join(";")) });
    let url = mac_field(auto, "URL");
    let pac_on = mac_field(auto, "Enabled") == "Yes" && !url.is_empty() && url != "(null)";
    m.insert("AutoConfigURL".into(), if pac_on { json!(url) } else { Value::Null });
    m
}

/// GNOME: the gsettings commands for `values`
pub fn gnome_set_commands(values: &Map<String, Value>) -> Vec<Vec<String>> {
    let on = values.get("ProxyEnable").and_then(Value::as_u64) == Some(1);
    let server = values.get("ProxyServer").and_then(Value::as_str).and_then(parse_server);
    let pac = values.get("AutoConfigURL").and_then(Value::as_str).filter(|u| !u.is_empty());
    let set = |schema: &str, key: &str, v: String| vec![arg("gsettings"), arg("set"), arg(schema), arg(key), v];
    let mut cmds = vec![];
    let mode = match (pac, on, &server) {
        (Some(_), _, _) => "auto",
        (None, true, Some(_)) => "manual",
        _ => "none",
    };
    if let (true, Some((socks, host, port))) = (on, &server) {
        let (used, unused) =
            if *socks { (vec!["socks"], vec!["http", "https"]) } else { (vec!["http", "https"], vec!["socks"]) };
        for kind in used {
            let schema = format!("org.gnome.system.proxy.{kind}");
            cmds.push(set(&schema, "host", host.clone()));
            cmds.push(set(&schema, "port", port.to_string()));
        }
        for kind in unused {
            cmds.push(set(&format!("org.gnome.system.proxy.{kind}"), "host", String::new()));
        }
    }
    if values.contains_key("ProxyOverride") {
        let items: Vec<String> =
            bypass_items(values, false).iter().map(|i| format!("'{}'", i.replace('\'', ""))).collect();
        cmds.push(set("org.gnome.system.proxy", "ignore-hosts", format!("[{}]", items.join(", "))));
    }
    if let Some(u) = pac {
        cmds.push(set("org.gnome.system.proxy", "autoconfig-url", u.to_string()));
    }
    cmds.push(set("org.gnome.system.proxy", "mode", mode.to_string()));
    cmds
}

/// a gsettings value as text ("'x'" -> x, "['a', 'b']" -> [a, b])
fn gvariant_str(s: &str) -> String {
    s.trim().trim_matches('\'').to_string()
}

/// GNOME: the four values from `gsettings get` answers (mode, http host / port, socks host / port, ignore-hosts,
/// autoconfig-url)
pub fn gnome_values(
    mode: &str,
    http: (&str, &str),
    socks: (&str, &str),
    ignore: &str,
    pac: &str,
) -> Map<String, Value> {
    let mut m = Map::new();
    let mode = gvariant_str(mode);
    let (hh, hp, sh, sp) = (gvariant_str(http.0), http.1.trim(), gvariant_str(socks.0), socks.1.trim());
    let server = if mode == "manual" && !hh.is_empty() {
        json!(format!("{hh}:{hp}"))
    } else if mode == "manual" && !sh.is_empty() {
        json!(format!("socks={sh}:{sp}"))
    } else {
        Value::Null
    };
    m.insert("ProxyEnable".into(), json!(if server.is_null() { 0 } else { 1 }));
    m.insert("ProxyServer".into(), server);
    let items: Vec<String> = ignore
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(gvariant_str)
        .filter(|x| !x.is_empty() && !x.starts_with('@'))
        .collect();
    m.insert("ProxyOverride".into(), if items.is_empty() { Value::Null } else { json!(items.join(";")) });
    let pac = gvariant_str(pac);
    m.insert("AutoConfigURL".into(), if mode == "auto" && !pac.is_empty() { json!(pac) } else { Value::Null });
    m
}

/// KDE: the kwriteconfig commands for `values` (kioslaverc, group "Proxy Settings")
pub fn kde_set_commands(tool: &str, values: &Map<String, Value>) -> Vec<Vec<String>> {
    let on = values.get("ProxyEnable").and_then(Value::as_u64) == Some(1);
    let server = values.get("ProxyServer").and_then(Value::as_str).and_then(parse_server);
    let pac = values.get("AutoConfigURL").and_then(Value::as_str).filter(|u| !u.is_empty());
    let set = |key: &str, v: String| {
        vec![
            arg(tool),
            arg("--file"),
            arg("kioslaverc"),
            arg("--group"),
            arg("Proxy Settings"),
            arg("--key"),
            arg(key),
            v,
        ]
    };
    let mut cmds = vec![];
    let kind = match (pac, on, &server) {
        (Some(_), _, _) => "2",
        (None, true, Some(_)) => "1",
        _ => "0",
    };
    if let (true, Some((socks, host, port))) = (on, &server) {
        if *socks {
            cmds.push(set("socksProxy", format!("socks://{host} {port}")));
            cmds.push(set("httpProxy", String::new()));
            cmds.push(set("httpsProxy", String::new()));
        } else {
            cmds.push(set("httpProxy", format!("http://{host} {port}")));
            cmds.push(set("httpsProxy", format!("http://{host} {port}")));
            cmds.push(set("socksProxy", String::new()));
        }
    }
    if values.contains_key("ProxyOverride") {
        cmds.push(set("NoProxyFor", bypass_items(values, false).join(",")));
    }
    if let Some(u) = pac {
        cmds.push(set("Proxy Config Script", u.to_string()));
    }
    cmds.push(set("ProxyType", kind.to_string()));
    cmds
}

/// KDE: the four values from kioslaverc (ProxyType, httpProxy, socksProxy, NoProxyFor, Proxy Config Script)
pub fn kde_values(kind: &str, http: &str, socks: &str, no_proxy: &str, pac: &str) -> Map<String, Value> {
    let hp = |s: &str| -> Option<String> {
        let rest = s.trim().split_once("://").map_or(s.trim(), |(_, r)| r);
        let (h, p) = rest.split_once(' ').or_else(|| rest.rsplit_once(':'))?;
        (!h.is_empty()).then(|| format!("{}:{}", h, p.trim()))
    };
    let mut m = Map::new();
    let server = match kind.trim() {
        "1" => hp(http).map(|s| json!(s)).or_else(|| hp(socks).map(|s| json!(format!("socks={s}")))),
        _ => None,
    };
    m.insert("ProxyEnable".into(), json!(if server.is_some() { 1 } else { 0 }));
    m.insert("ProxyServer".into(), server.unwrap_or(Value::Null));
    let items: Vec<&str> = no_proxy.split(',').map(str::trim).filter(|x| !x.is_empty()).collect();
    m.insert("ProxyOverride".into(), if items.is_empty() { Value::Null } else { json!(items.join(";")) });
    let pac = pac.trim();
    m.insert("AutoConfigURL".into(), if kind.trim() == "2" && !pac.is_empty() { json!(pac) } else { Value::Null });
    m
}

#[cfg(not(windows))]
mod sys {
    pub fn output(cmd: &[&str]) -> Option<String> {
        let out = std::process::Command::new(cmd[0]).args(&cmd[1..]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn run(cmds: &[Vec<String>]) -> Result<(), String> {
        for c in cmds {
            let st = std::process::Command::new(&c[0]).args(&c[1..]).output().map_err(|e| format!("{}: {e}", c[0]))?;
            if !st.status.success() {
                return Err(format!("{}: {}", c.join(" "), String::from_utf8_lossy(&st.stderr).trim()));
            }
        }
        Ok(())
    }

    pub fn on_path(tool: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
    }

    /// macOS: the enabled network services (Wi-Fi, Ethernet, ...)
    pub fn mac_services() -> Vec<String> {
        output(&["networksetup", "-listallnetworkservices"])
            .map(|o| {
                o.lines().skip(1).filter(|l| !l.trim().is_empty() && !l.starts_with('*')).map(String::from).collect()
            })
            .unwrap_or_default()
    }

    pub fn kde() -> Option<&'static str> {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_uppercase();
        if !desktop.contains("KDE") {
            return None;
        }
        ["kwriteconfig6", "kwriteconfig5"].into_iter().find(|t| on_path(t))
    }
}

/// whether ProxyDock can set this computer's system proxy (Windows; macOS; Linux with GNOME or KDE)
pub fn system_proxy_supported() -> bool {
    #[cfg(windows)]
    return true;
    // (never in tests: they set a test folder, and the system proxy has no test stand-in here)
    #[cfg(target_os = "macos")]
    return TEST_DIR.read().unwrap().is_none();
    #[cfg(target_os = "linux")]
    return TEST_DIR.read().unwrap().is_none() && (sys::on_path("gsettings") || sys::kde().is_some());
    #[allow(unreachable_code)]
    false
}

#[cfg(not(windows))]
fn get_system_proxy_unix() -> Result<Map<String, Value>, String> {
    if cfg!(target_os = "macos") {
        let svc = sys::mac_services().into_iter().next().ok_or_else(|| tr("No network service found"))?;
        let get = |flag: &str| sys::output(&["networksetup", flag, &svc]).unwrap_or_default();
        return Ok(mac_values(
            &get("-getwebproxy"),
            &get("-getsocksfirewallproxy"),
            &get("-getproxybypassdomains"),
            &get("-getautoproxyurl"),
        ));
    }
    if let Some(tool) = sys::kde() {
        let read = tool.replace("kwrite", "kread");
        let get = |key: &str| {
            sys::output(&[&read, "--file", "kioslaverc", "--group", "Proxy Settings", "--key", key]).unwrap_or_default()
        };
        return Ok(kde_values(
            &get("ProxyType"),
            &get("httpProxy"),
            &get("socksProxy"),
            &get("NoProxyFor"),
            &get("Proxy Config Script"),
        ));
    }
    let get = |schema: &str, key: &str| sys::output(&["gsettings", "get", schema, key]).unwrap_or_default();
    Ok(gnome_values(
        &get("org.gnome.system.proxy", "mode"),
        (&get("org.gnome.system.proxy.http", "host"), &get("org.gnome.system.proxy.http", "port")),
        (&get("org.gnome.system.proxy.socks", "host"), &get("org.gnome.system.proxy.socks", "port")),
        &get("org.gnome.system.proxy", "ignore-hosts"),
        &get("org.gnome.system.proxy", "autoconfig-url"),
    ))
}

#[cfg(not(windows))]
fn set_system_proxy_unix(values: &Map<String, Value>) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        return sys::run(&mac_set_commands(&sys::mac_services(), values));
    }
    let mut done = false;
    if let Some(tool) = sys::kde() {
        sys::run(&kde_set_commands(tool, values))?;
        // (KDE programs read the settings again)
        let _ = std::process::Command::new("dbus-send")
            .args(["--type=signal", "/KIO/Scheduler", "org.kde.KIO.Scheduler.reparseSlaveConfiguration", "string:"])
            .status();
        done = true;
    }
    if sys::on_path("gsettings") {
        sys::run(&gnome_set_commands(values))?;
        done = true;
    }
    if done { Ok(()) } else { Err(tr("This desktop has no system proxy setting ProxyDock can change")) }
}

// --- Windows Firewall: block connections from other PCs to ProxyDock's programs ---

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// whether our block rule exists (Python `firewall_rule_exists`)
pub fn firewall_rule_exists() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("netsh")
            .args(["advfirewall", "firewall", "show", "rule", &format!("name={FW_RULE}")])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .is_ok_and(|o| o.status.success())
    }
    #[cfg(not(windows))]
    false
}

/// the commands for the elevated cmd.exe: old rules removed, then (block) one rule per program
pub fn firewall_commands(block: bool, programs: &[PathBuf]) -> Result<String, String> {
    let mut cmds: Vec<String> = FW_OLD_RULES
        .iter()
        .chain(std::iter::once(&FW_RULE))
        .map(|n| format!("netsh advfirewall firewall delete rule name=\"{n}\" >nul 2>&1"))
        .collect();
    if block {
        for p in programs {
            let s = p.display().to_string();
            if s.chars().any(|c| "\"%^&|<>".contains(c)) {
                return Err(tr(
                    "The program path contains special characters (\" % ^ & | < >), so the rule cannot be created. Move it to another folder",
                ));
            }
            cmds.push(format!(
                "netsh advfirewall firewall add rule name=\"{FW_RULE}\" dir=in action=block program=\"{s}\" enable=yes"
            ));
        }
    }
    Ok(cmds.join(" & "))
}

/// Ask for administrator rights (UAC), add or remove the rule and wait for it (Python `firewall_apply`). The rule
/// covers every program that accepts connections: this app and the sing-box / Hysteria programs it runs.
pub fn firewall_apply(block: bool, programs: &[PathBuf]) -> Result<String, String> {
    let cmds = firewall_commands(block, programs)?;
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (verb, file, params) = (wide("runas"), wide("cmd.exe"), wide(&format!("/s /c \"{cmds}\"")));
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = 0;
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let err = unsafe { GetLastError() };
            return Err(if err == 1223 {
                tr("The administrator permission request was cancelled.")
            } else {
                tr("Failed to run (error {err})").replace("{err}", &err.to_string())
            });
        }
        if info.hProcess.is_null() {
            return Ok(tr("Request sent"));
        }
        let mut code = 1u32;
        unsafe {
            WaitForSingleObject(info.hProcess, 30_000);
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
        }
        if code != 0 {
            return Err(tr("netsh failed (exit code {value})").replace("{value}", &code.to_string()));
        }
        Ok(tr(if block { "Added the external block rule" } else { "Removed the external block rule" }))
    }
    #[cfg(not(windows))]
    {
        let _ = (block, cmds);
        Err(tr("Windows only for now"))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn startup_and_system_proxy_on_test_keys() {
        let base = format!(r"Software\ProxyDockTest\{}", std::process::id());
        set_test_keys(&format!(r"{base}\Run"), &format!(r"{base}\Inet"));
        let exe = Path::new(r"C:\Apps\Proxy Dock\proxydock.exe");
        assert_eq!(startup_state(), (false, false));
        set_startup(exe, true, true).unwrap();
        assert_eq!(startup_command().as_deref(), Some(r#""C:\Apps\Proxy Dock\proxydock.exe" --hidden"#));
        assert_eq!(startup_state(), (true, true));
        set_startup(exe, true, false).unwrap();
        assert_eq!(startup_state(), (true, false));
        set_startup(exe, false, false).unwrap();
        assert_eq!(startup_state(), (false, false));
        // the previous version's entry moves to ProxyDock, keeping a hidden start (Python `migrate_autostart`)
        let k = reg::open(&run_key(), true).unwrap();
        reg::set_str(&k, OLD_STARTUP_NAME, r#""C:\old\socks5proxy.exe" --hidden"#).unwrap();
        assert!(migrate_autostart(exe));
        assert!(reg::get(&k, OLD_STARTUP_NAME).is_none());
        assert_eq!(startup_state(), (true, true));
        assert!(!migrate_autostart(exe), "nothing left to move");
        set_startup(exe, false, false).unwrap();

        // the PC block: back up, apply (the user's exceptions kept), restore
        let mut before = Map::new();
        before.insert("ProxyEnable".into(), json!(0));
        before.insert("ProxyOverride".into(), json!("*.corp;localhost"));
        set_system_proxy(&before).unwrap();
        let backup = get_system_proxy().unwrap();
        assert_eq!(backup["ProxyEnable"], json!(0));
        assert_eq!(backup["ProxyServer"], Value::Null);
        let server = system_proxy_server(&json!({"type": "http", "host": "0.0.0.0", "port": 8080})).unwrap();
        assert_eq!(server, "127.0.0.1:8080");
        set_system_proxy(&proxy_values_for(&server, &Value::Object(backup.clone()))).unwrap();
        let now = get_system_proxy().unwrap();
        assert_eq!(now["ProxyEnable"], json!(1));
        assert_eq!(now["ProxyServer"], json!("127.0.0.1:8080"));
        assert_eq!(now["ProxyOverride"], json!("localhost;127.*;*.corp;<local>"));
        set_system_proxy(&backup).unwrap();
        assert_eq!(get_system_proxy().unwrap(), backup);

        assert_eq!(system_proxy_server(&json!({"type": "socks4", "host": "::", "port": 1})).unwrap(), "socks=[::1]:1");
        assert!(system_proxy_server(&json!({"type": "socks5", "host": "127.0.0.1", "port": 1})).is_err());
        let cmds =
            firewall_commands(true, &[PathBuf::from(r"C:\a\proxydock.exe"), PathBuf::from(r"C:\b\sing-box.exe")])
                .unwrap();
        assert!(cmds.ends_with(r#"add rule name="ProxyDock - Block External" dir=in action=block program="C:\b\sing-box.exe" enable=yes"#), "{cmds}");
        assert!(firewall_commands(true, &[PathBuf::from(r"C:\a&b\x.exe")]).is_err());
        // (the test keys go again)
        unsafe {
            let w: Vec<u16> = r"Software\ProxyDockTest".encode_utf16().chain(Some(0)).collect();
            windows_sys::Win32::System::Registry::RegDeleteTreeW(
                windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
                w.as_ptr(),
            );
        }
    }
}

#[cfg(all(test, not(windows)))]
mod login_item_tests {
    use super::*;

    #[test]
    fn login_items_in_a_test_folder() {
        let dir = std::env::temp_dir().join(format!("proxydock-login-{}", std::process::id()));
        set_test_dir(&dir);
        let exe = Path::new("/opt/Proxy Dock/proxydock");
        assert_eq!(startup_state(), (false, false));
        set_startup(exe, true, true).unwrap();
        assert_eq!(startup_state(), (true, true));
        let text = startup_command().unwrap();
        assert!(text.contains("Proxy Dock"), "{text}");
        set_startup(exe, false, false).unwrap();
        assert_eq!(startup_state(), (false, false));
        // the previous version's entry becomes ours, keeping a hidden start
        std::fs::write(login_item(true).unwrap(), "x --hidden").unwrap();
        assert!(migrate_autostart(exe));
        assert_eq!(startup_state(), (true, true));
        assert!(!login_item(true).unwrap().exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod login_text_tests {
    #[test]
    fn plist_and_desktop_entry() {
        let exe = std::path::Path::new("/Applications/ProxyDock.app/Contents/MacOS/proxydock");
        let plist = super::login_item_text(exe, true, true);
        assert!(plist.contains(
            "<string>/Applications/ProxyDock.app/Contents/MacOS/proxydock</string><string>--hidden</string>"
        ));
        assert!(plist.contains("<key>RunAtLoad</key><true/>"));
        let desktop = super::login_item_text(std::path::Path::new("/opt/it's here/proxydock"), false, false);
        assert!(desktop.contains("Exec='/opt/it'\"'\"'s here/proxydock'\n"), "{desktop}");
        assert!(desktop.starts_with("[Desktop Entry]\nType=Application\nName=ProxyDock\n"));
    }
}

#[cfg(test)]
mod sysproxy_tests {
    use super::*;

    fn values(server: &str, bypass: &str) -> Map<String, Value> {
        proxy_values_for(server, &json!({"ProxyOverride": bypass}))
    }

    #[test]
    fn macos_networksetup() {
        let cmds = mac_set_commands(&["Wi-Fi".into()], &values("127.0.0.1:8080", "*.corp"));
        let lines: Vec<String> = cmds.iter().map(|c| c.join(" ")).collect();
        assert!(lines.contains(&"networksetup -setwebproxy Wi-Fi 127.0.0.1 8080".to_string()), "{lines:?}");
        assert!(lines.contains(&"networksetup -setsecurewebproxy Wi-Fi 127.0.0.1 8080".to_string()));
        assert!(
            lines.contains(
                &"networksetup -setproxybypassdomains Wi-Fi localhost 127.0.0.0/8 *.corp *.local".to_string()
            )
        );
        assert!(lines.contains(&"networksetup -setautoproxystate Wi-Fi off".to_string()));
        let socks = mac_set_commands(&["Wi-Fi".into()], &values("socks=127.0.0.1:1080", ""));
        assert_eq!(socks[0].join(" "), "networksetup -setsocksfirewallproxy Wi-Fi 127.0.0.1 1080");
        // read back
        let v = mac_values(
            "Enabled: Yes\nServer: 10.0.0.1\nPort: 3128\nAuthenticated Proxy Enabled: 0\n",
            "Enabled: No\nServer: \nPort: 0\n",
            "*.local\n169.254/16\n",
            "URL: (null)\nEnabled: No\n",
        );
        assert_eq!(v["ProxyServer"], json!("10.0.0.1:3128"));
        assert_eq!(v["ProxyOverride"], json!("*.local;169.254/16"));
        assert_eq!(v["AutoConfigURL"], Value::Null);
        // off: every proxy off
        let off = mac_set_commands(&["Wi-Fi".into()], &v_off());
        assert!(off.iter().all(|c| c[1].ends_with("state") || c[1] == "-setproxybypassdomains"));
    }

    fn v_off() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("ProxyEnable".into(), json!(0));
        m
    }

    #[test]
    fn gnome_and_kde() {
        let lines: Vec<String> =
            gnome_set_commands(&values("127.0.0.1:8080", "")).iter().map(|c| c.join(" ")).collect();
        assert!(lines.contains(&"gsettings set org.gnome.system.proxy.http host 127.0.0.1".to_string()), "{lines:?}");
        assert!(
            lines.contains(
                &"gsettings set org.gnome.system.proxy ignore-hosts ['localhost', '127.0.0.0/8']".to_string()
            )
        );
        assert_eq!(lines.last().unwrap(), "gsettings set org.gnome.system.proxy mode manual");
        let v = gnome_values("'manual'", ("'127.0.0.1'", "8080"), ("''", "0"), "['localhost', '127.0.0.0/8']", "''");
        assert_eq!(v["ProxyServer"], json!("127.0.0.1:8080"));
        assert_eq!(v["ProxyOverride"], json!("localhost;127.0.0.0/8"));
        let none = gnome_values("'none'", ("''", "0"), ("''", "0"), "@as []", "''");
        assert_eq!((none["ProxyEnable"].clone(), none["ProxyOverride"].clone()), (json!(0), Value::Null));
        let kde: Vec<String> = kde_set_commands("kwriteconfig6", &values("socks=127.0.0.1:1080", ""))
            .iter()
            .map(|c| c.join(" "))
            .collect();
        assert!(
            kde.contains(
                &"kwriteconfig6 --file kioslaverc --group Proxy Settings --key socksProxy socks://127.0.0.1 1080"
                    .to_string()
            ),
            "{kde:?}"
        );
        assert_eq!(kde.last().unwrap(), "kwriteconfig6 --file kioslaverc --group Proxy Settings --key ProxyType 1");
        let v = kde_values("1", "http://127.0.0.1 8080", "", "localhost,127.0.0.1", "");
        assert_eq!(v["ProxyServer"], json!("127.0.0.1:8080"));
        assert_eq!(v["ProxyOverride"], json!("localhost;127.0.0.1"));
        assert_eq!(parse_server("socks=[::1]:1080"), Some((true, "::1".into(), 1080)));
    }
}
