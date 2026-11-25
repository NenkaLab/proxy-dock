//! OS integration (Python `platform_utils`): start at login, the system proxy of the PC block (Windows WinINet)
//! and the Windows Firewall rule that blocks other PCs.
//!
//! Tests point the registry keys somewhere else (`set_test_keys`), so they never change the real settings.

use crate::i18n::tr;
use serde_json::{json, Map, Value};
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
    "SOCKS5 Proxy - Block External", "Block SOCKS5 Proxy External", "Allow SOCKS5 Proxy Local",
    "Block SOCKS5 Proxy External UDP", "Allow SOCKS5 Proxy Local UDP",
];

/// (Run key, Internet Settings key) in HKCU; tests replace them
static KEYS: RwLock<Option<(String, String)>> = RwLock::new(None);

/// Tests: use these HKCU keys instead of the real Run and Internet Settings keys.
pub fn set_test_keys(run: &str, inet: &str) {
    *KEYS.write().unwrap() = Some((run.into(), inet.into()));
}

fn run_key() -> String {
    KEYS.read().unwrap().as_ref().map_or(RUN_KEY.into(), |k| k.0.clone())
}

fn inet_key() -> String {
    KEYS.read().unwrap().as_ref().map_or(INET_KEY.into(), |k| k.1.clone())
}

pub fn supported() -> bool {
    cfg!(windows)
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
                RegCreateKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, std::ptr::null(), 0, KEY_READ | KEY_SET_VALUE, std::ptr::null(), &mut h, std::ptr::null_mut())
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
        if unsafe { RegQueryValueExW(k.0, name.as_ptr(), std::ptr::null(), &mut ty, std::ptr::null_mut(), &mut len) } != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u8; len as usize + 2];
        if unsafe { RegQueryValueExW(k.0, name.as_ptr(), std::ptr::null(), &mut ty, buf.as_mut_ptr(), &mut len) } != ERROR_SUCCESS {
            return None;
        }
        buf.truncate(len as usize);
        match ty {
            REG_DWORD if buf.len() >= 4 => Some(serde_json::json!(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))),
            REG_SZ | REG_EXPAND_SZ => {
                let w: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
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
    None
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
        let _ = (exe, register, hidden);
        Err(tr("Windows only for now"))
    }
}

// --- the system proxy (the PC block) ---

/// (ProxyServer value, error) for using this inbound as the system proxy (Python `system_proxy_server`)
pub fn system_proxy_server(inbound: &Value) -> Result<String, String> {
    let t = inbound["type"].as_str().unwrap_or("");
    if !PC_INBOUND_TYPES.contains(&t) {
        return Err(tr("Windows can only use an HTTP (or SOCKS4) inbound as the system proxy - add an HTTP inbound for the PC"));
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
    Err(tr("Windows only for now"))
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
                _ if name == "ProxyEnable" => reg::set_dword(&k, name, v.as_u64().unwrap_or(0) as u32).map_err(|e| e.to_string())?,
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
    {
        let _ = values;
        Err(tr("Windows only for now"))
    }
}

#[cfg(windows)]
fn notify_inet_change() {
    use windows_sys::Win32::Networking::WinInet::{InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED};
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

// --- Windows Firewall: block connections from other PCs to ProxyDock's programs ---

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
                return Err(tr("The program path contains special characters (\" % ^ & | < >), so the rule cannot be created. Move it to another folder"));
            }
            cmds.push(format!("netsh advfirewall firewall add rule name=\"{FW_RULE}\" dir=in action=block program=\"{s}\" enable=yes"));
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
        use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
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
        let cmds = firewall_commands(true, &[PathBuf::from(r"C:\a\proxydock.exe"), PathBuf::from(r"C:\b\sing-box.exe")]).unwrap();
        assert!(cmds.ends_with(r#"add rule name="ProxyDock - Block External" dir=in action=block program="C:\b\sing-box.exe" enable=yes"#), "{cmds}");
        assert!(firewall_commands(true, &[PathBuf::from(r"C:\a&b\x.exe")]).is_err());
        // (the test keys go again)
        unsafe {
            let w: Vec<u16> = r"Software\ProxyDockTest".encode_utf16().chain(Some(0)).collect();
            windows_sys::Win32::System::Registry::RegDeleteTreeW(windows_sys::Win32::System::Registry::HKEY_CURRENT_USER, w.as_ptr());
        }
    }
}
