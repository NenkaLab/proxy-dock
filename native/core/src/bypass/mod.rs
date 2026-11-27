//! VPN bypass: a block can leave outside a VPN that runs on the whole PC, through a tiny Linux VM whose network card
//! is bridged to a physical one (Hyper-V External switch). The VM's traffic never passes the host's IP stack, so
//! the VPN's routes and its kill switch do not see it. The guest is built by `tools/build_bypass_image.py`
//! (`native/bypass-guest`).
//!
//! * An outbound's `interface` says where it leaves: `@bypass` = the shared VM, `@bypass:<name>` = a VM of its own
//!   (several outbounds may share a named one). Nothing else in the settings changes.
//! * `bypass.json` (settings folder): the VMs ("instances") with their network card, memory, address and the
//!   user's consent; see [`Store`].
//! * Hyper-V needs administrator rights: [`helper`] runs elevated (UAC once per run of ProxyDock), creates and
//!   starts the VMs and relays 127.0.0.1 ports to them over Hyper-V sockets. [`hub`] is ProxyDock's side.
//! * A card's sing-box reaches its VM as a `socks` outbound (UDP over TCP) on that port, with an account the VM
//!   asks for, so no other program on the PC can use the way out.

pub mod helper;
pub mod hub;
#[cfg(windows)]
pub mod hvsock;

use crate::i18n::tr;
use serde_json::{Value, json};
use std::path::PathBuf;

/// what an outbound's `interface` starts with when it leaves through a bypass VM
pub const IFACE: &str = "@bypass";
pub const MIN_MEMORY_MB: u32 = 200;
pub const MAX_MEMORY_MB: u32 = 2048;
pub const DEFAULT_MEMORY_MB: u32 = 200;
/// Hyper-V socket ports of pd-agent in the VM
pub const CONTROL_PORT: u32 = 20548;
pub const DATA_PORT: u32 = 20549;
/// the image of this ProxyDock's release (`tools/build_bypass_image.py`)
pub const IMAGE_NAME: &str = "proxydock-bypass-x64.vhdx";
pub const IMAGE_ZIP: &str = "proxydock-bypass-x64.zip";

/// the instance an `interface` value names: "" = the shared VM, None = not a bypass
pub fn instance_of(iface: &str) -> Option<String> {
    if iface == IFACE {
        return Some(String::new());
    }
    iface.strip_prefix(IFACE).and_then(|r| r.strip_prefix(':')).map(|n| n.trim().to_string())
}

/// the `interface` value for an instance
pub fn iface_for(name: &str) -> String {
    if name.is_empty() { IFACE.to_string() } else { format!("{IFACE}:{name}") }
}

/// the sing-box outbound tag of an instance
pub fn tag_for(name: &str) -> String {
    if name.is_empty() { "bypass".into() } else { format!("bypass:{name}") }
}

/// how the UI names an instance
pub fn label(name: &str) -> String {
    if name.is_empty() { tr("shared") } else { name.to_string() }
}

/// The instances a card's blocks leave through, sorted, each once (every `interface` anywhere in its outbound tree
/// and its router targets).
pub fn instances_in(proxy: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if let Some(n) = m.get("interface").and_then(Value::as_str).and_then(instance_of) {
                    out.push(n);
                }
                m.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = vec![];
    walk(&proxy["outbound"], &mut out);
    walk(&proxy["routes"], &mut out);
    out.sort();
    out.dedup();
    out
}

/// an instance name the user typed: 1-24 letters, digits, spaces, '-' or '_' (it becomes part of the VM's name)
pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && n.chars().count() <= 24
        && n == name
        && n.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

/// the Hyper-V VM of an instance
pub fn vm_name(name: &str) -> String {
    if name.is_empty() { "ProxyDock Bypass".into() } else { format!("ProxyDock Bypass - {name}") }
}

/// The user said yes to the warning for this instance on this network card (asked again when the card changes).
#[derive(Clone, Debug, PartialEq)]
pub struct Consent {
    pub time: i64,
    pub adapter: String,
}

/// One bypass VM.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    /// "" = the shared one
    pub name: String,
    /// the physical network card it is bridged to (Windows' name, e.g. "Wi-Fi")
    pub adapter: String,
    pub memory_mb: u32,
    /// "dhcp" or "static"
    pub ip_mode: String,
    /// static: "192.168.0.50/24", the gateway and DNS servers (comma separated)
    pub address: String,
    pub gateway: String,
    pub dns: String,
    pub consent: Option<Consent>,
}

impl Instance {
    pub fn new(name: &str, adapter: &str) -> Instance {
        Instance {
            name: name.into(),
            adapter: adapter.into(),
            memory_mb: DEFAULT_MEMORY_MB,
            ip_mode: "dhcp".into(),
            address: String::new(),
            gateway: String::new(),
            dns: String::new(),
            consent: None,
        }
    }

    /// prepared with the user's consent for its current network card
    pub fn consented(&self) -> bool {
        self.consent.as_ref().is_some_and(|c| c.adapter == self.adapter && !self.adapter.is_empty())
    }

    /// pd-agent's `net` request
    pub fn net_request(&self) -> Value {
        if self.ip_mode == "static" {
            let dns: Vec<&str> = self.dns.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).collect();
            json!({"op": "net", "mode": "static", "address": self.address, "gateway": self.gateway, "dns": dns})
        } else {
            json!({"op": "net", "mode": "dhcp"})
        }
    }

    fn to_json(&self) -> Value {
        let mut v = json!({
            "name": self.name, "adapter": self.adapter, "memory_mb": self.memory_mb, "ip_mode": self.ip_mode,
            "address": self.address, "gateway": self.gateway, "dns": self.dns,
        });
        if let Some(c) = &self.consent {
            v["consent"] = json!({"time": c.time, "adapter": c.adapter});
        }
        v
    }

    fn from_json(v: &Value) -> Option<Instance> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let name = s("name");
        if !name.is_empty() && !valid_name(&name) {
            return None;
        }
        let mem = v.get("memory_mb").and_then(Value::as_u64).unwrap_or(DEFAULT_MEMORY_MB as u64);
        let consent = v.get("consent").and_then(|c| {
            Some(Consent { time: c.get("time")?.as_i64()?, adapter: c.get("adapter")?.as_str()?.to_string() })
        });
        Some(Instance {
            name,
            adapter: s("adapter"),
            memory_mb: clamp_memory(mem as u32),
            ip_mode: if s("ip_mode") == "static" { "static".into() } else { "dhcp".into() },
            address: s("address"),
            gateway: s("gateway"),
            dns: s("dns"),
            consent,
        })
    }
}

/// memory in MB as Hyper-V takes it: a multiple of 2, MIN..MAX
pub fn clamp_memory(mb: u32) -> u32 {
    mb.clamp(MIN_MEMORY_MB, MAX_MEMORY_MB).div_ceil(2) * 2
}

/// `bypass.json`: the instances (the shared one first) and whether VMs keep running while no card uses them.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Store {
    pub instances: Vec<Instance>,
    pub keep_running: bool,
}

pub fn store_path() -> PathBuf {
    crate::paths::app_dir().join("bypass.json")
}

/// where the image and the VMs' disks live: next to the settings, unless PROXYDOCK_BYPASS_DIR says otherwise
pub fn data_dir() -> PathBuf {
    match std::env::var_os("PROXYDOCK_BYPASS_DIR") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => {
            let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(crate::paths::app_dir);
            if std::env::var_os("PROXYDOCK_HOME").is_some_and(|p| !p.is_empty()) {
                crate::paths::app_dir().join("bypass")
            } else {
                base.join(crate::paths::APP_ID).join("bypass")
            }
        }
    }
}

impl Store {
    pub fn load() -> Store {
        let text = std::fs::read_to_string(store_path()).unwrap_or_default();
        Store::parse(&text)
    }

    pub fn parse(text: &str) -> Store {
        let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        let mut instances: Vec<Instance> = vec![];
        for i in v.get("instances").and_then(Value::as_array).into_iter().flatten().filter_map(Instance::from_json) {
            if !instances.iter().any(|x| x.name == i.name) {
                instances.push(i);
            }
        }
        instances.sort_by_key(|i| (!i.name.is_empty(), i.name.to_lowercase()));
        Store { instances, keep_running: v.get("keep_running").and_then(Value::as_bool).unwrap_or(false) }
    }

    pub fn to_text(&self) -> String {
        let v = json!({
            "instances": self.instances.iter().map(Instance::to_json).collect::<Vec<_>>(),
            "keep_running": self.keep_running,
        });
        serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = store_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, &p)
    }

    pub fn get(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name == name)
    }

    /// add or replace (by name)
    pub fn put(&mut self, inst: Instance) {
        match self.instances.iter_mut().find(|i| i.name == inst.name) {
            Some(i) => *i = inst,
            None => self.instances.push(inst),
        }
        self.instances.sort_by_key(|i| (!i.name.is_empty(), i.name.to_lowercase()));
    }

    pub fn remove(&mut self, name: &str) {
        self.instances.retain(|i| i.name != name);
    }
}

/// Can this PC run the bypass VM? Err = why not (shown next to the choice and in the settings).
pub fn availability() -> Result<(), String> {
    #[cfg(windows)]
    {
        // the Hyper-V service is there and running (Windows Pro / Enterprise / Education with Hyper-V on)
        let out = std::process::Command::new("sc.exe")
            .args(["query", "vmms"])
            .creation_flags_hidden()
            .output()
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || !text.contains("STATE") {
            return Err(tr("Hyper-V is not installed (it needs Windows Pro, Enterprise or Education)"));
        }
        if !text.contains("RUNNING") {
            return Err(tr("Hyper-V is turned off (Windows features: Hyper-V)"));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(tr("Only on Windows for now"))
    }
}

/// `Command` without a console window (Windows)
pub trait Hidden {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}

impl Hidden for std::process::Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(0x0800_0000);
        }
        self
    }
}

/// The PC's physical network cards a VM can be bridged to (Windows' names), wired and wireless.
pub fn physical_adapters() -> Vec<String> {
    #[cfg(windows)]
    {
        let script = "Get-NetAdapter -Physical | Where-Object { $_.Status -ne 'Not Present' } | Sort-Object Name | ForEach-Object { $_.Name }";
        let Ok(out) = helper::powershell(script) else { return vec![] };
        out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
    }
    #[cfg(not(windows))]
    {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_values() {
        assert_eq!(instance_of("@bypass").as_deref(), Some(""));
        assert_eq!(instance_of("@bypass:game").as_deref(), Some("game"));
        assert_eq!(instance_of("@bypassx"), None);
        assert_eq!(instance_of("Wi-Fi"), None);
        assert_eq!(iface_for(""), "@bypass");
        assert_eq!(iface_for("game"), "@bypass:game");
        assert_eq!(tag_for(""), "bypass");
    }

    #[test]
    fn instances_of_a_card() {
        let p = json!({
            "outbound": {"type": "socks5", "interface": "", "via": {"type": "hysteria2", "interface": "@bypass:b"}},
            "routes": [{"outbound": {"type": "direct", "interface": "@bypass"}},
                       {"outbound": {"type": "group", "members": [{"outbound": {"interface": "@bypass:b"}}]}}],
        });
        assert_eq!(instances_in(&p), ["", "b"]);
        assert!(instances_in(&json!({"outbound": {"type": "direct", "interface": "Wi-Fi"}})).is_empty());
    }

    #[test]
    fn names_and_memory() {
        assert!(valid_name("game 1"));
        assert!(valid_name("게임"));
        assert!(!valid_name(""));
        assert!(!valid_name(" x"));
        assert!(!valid_name("a'b"));
        assert!(!valid_name(&"x".repeat(25)));
        assert_eq!(clamp_memory(100), 200);
        assert_eq!(clamp_memory(257), 258);
        assert_eq!(clamp_memory(99999), 2048);
    }

    #[test]
    fn store_round_trip() {
        let mut s = Store::default();
        let mut a = Instance::new("", "Wi-Fi");
        a.consent = Some(Consent { time: 5, adapter: "Wi-Fi".into() });
        s.put(Instance::new("game", "이더넷"));
        s.put(a.clone());
        assert_eq!(s.instances[0].name, "");
        let back = Store::parse(&s.to_text());
        assert_eq!(back, s);
        assert!(back.get("").unwrap().consented());
        let mut moved = a;
        moved.adapter = "이더넷".into();
        assert!(!moved.consented(), "a new network card needs consent again");
        // junk is dropped, the name kept unique
        let s = Store::parse(
            r#"{"instances": [{"name": "a'b"}, {"name": "x", "memory_mb": 5}, {"name": "x"}], "keep_running": 1}"#,
        );
        assert_eq!(s.instances.len(), 1);
        assert_eq!(s.instances[0].memory_mb, 200);
        assert!(!s.keep_running);
    }
}
