//! The VM's network card (eth0): DHCP through busybox udhcpc (kept running so it renews the lease) or a fixed
//! address set with `ip`. Every command gets its arguments directly, never through a shell.

use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr};
use std::process::{Child, Command, Stdio};

pub const IFACE: &str = "eth0";
const DHCP_SCRIPT: &str = "/usr/share/udhcpc/default.script";

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Dhcp,
    Static { address: Ipv4Addr, prefix: u8, gateway: Ipv4Addr, dns: Vec<IpAddr> },
}

impl Mode {
    /// `{"mode": "dhcp"}` or `{"mode": "static", "address": "192.168.0.50/24", "gateway": "192.168.0.1",
    /// "dns": ["1.1.1.1"]}`
    pub fn from_request(req: &Value) -> Result<Mode, String> {
        let s = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        match s("mode").as_str() {
            "dhcp" => Ok(Mode::Dhcp),
            "static" => {
                let cidr = s("address");
                let (ip, prefix) = cidr.split_once('/').ok_or("address needs a prefix length, e.g. 192.168.0.50/24")?;
                let address: Ipv4Addr = ip.parse().map_err(|_| format!("not an IPv4 address: {ip}"))?;
                let prefix: u8 = prefix.parse().ok().filter(|p| (1..=32).contains(p)).ok_or("prefix must be 1-32")?;
                let gw = s("gateway");
                let gateway: Ipv4Addr = gw.parse().map_err(|_| format!("not an IPv4 gateway: {gw}"))?;
                if !same_subnet(address, gateway, prefix) {
                    return Err(format!("gateway {gateway} is not in {address}/{prefix}"));
                }
                let mut dns = vec![];
                for d in req.get("dns").and_then(Value::as_array).into_iter().flatten() {
                    let t = d.as_str().unwrap_or("").trim();
                    dns.push(t.parse::<IpAddr>().map_err(|_| format!("not a DNS server address: {t}"))?);
                }
                if dns.is_empty() {
                    return Err("give at least one DNS server".into());
                }
                dns.truncate(4);
                Ok(Mode::Static { address, prefix, gateway, dns })
            }
            m => Err(format!("unknown network mode {m:?}")),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Mode::Dhcp => "dhcp",
            Mode::Static { .. } => "static",
        }
    }
}

fn same_subnet(a: Ipv4Addr, b: Ipv4Addr, prefix: u8) -> bool {
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix as u32) };
    u32::from(a) & mask == u32::from(b) & mask
}

pub struct Net {
    dry: bool,
    mode: Mode,
    dhcp: Option<Child>,
    error: Option<String>,
}

impl Net {
    pub fn new(dry: bool) -> Net {
        Net { dry, mode: Mode::Dhcp, dhcp: None, error: None }
    }

    pub fn apply(&mut self, mode: &Mode) -> Result<(), String> {
        let r = if self.dry { Ok(()) } else { self.apply_real(mode) };
        self.mode = mode.clone();
        self.error = r.as_ref().err().cloned();
        r
    }

    fn apply_real(&mut self, mode: &Mode) -> Result<(), String> {
        if let Some(mut c) = self.dhcp.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        run("ip", &["link", "set", IFACE, "up"])?;
        run("ip", &["-4", "addr", "flush", "dev", IFACE])?;
        match mode {
            Mode::Dhcp => {
                // -f stay in the foreground (we own it), -S log to syslog quietly, retry every 5 s while there is
                // no server; the default script sets the address, the default route and resolv.conf
                let c = Command::new("udhcpc")
                    .args(["-f", "-S", "-i", IFACE, "-T", "3", "-A", "5", "-s", DHCP_SCRIPT])
                    .args(["-x", "hostname:proxydock-bypass"])
                    .stdout(Stdio::null())
                    .spawn()
                    .map_err(|e| format!("cannot start udhcpc: {e}"))?;
                self.dhcp = Some(c);
                Ok(())
            }
            Mode::Static { address, prefix, gateway, dns } => {
                run("ip", &["addr", "add", &format!("{address}/{prefix}"), "dev", IFACE])?;
                run("ip", &["route", "replace", "default", "via", &gateway.to_string(), "dev", IFACE])?;
                let text: String = dns.iter().map(|d| format!("nameserver {d}\n")).collect();
                std::fs::write("/etc/resolv.conf", text).map_err(|e| format!("resolv.conf: {e}"))
            }
        }
    }

    pub fn status(&self) -> Value {
        let (ip, gateway, dns, link) = if self.dry { (None, None, vec![], true) } else { live() };
        json!({
            "mode": self.mode.name(),
            "link": link,
            "ip": ip,
            "gateway": gateway,
            "dns": dns,
            "mac": std::fs::read_to_string(format!("/sys/class/net/{IFACE}/address")).ok().map(|s| s.trim().to_string()),
            "error": self.error,
        })
    }
}

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(cmd).args(args).output().map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{cmd} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn output(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// address (CIDR), default gateway, DNS servers and carrier of eth0 right now
fn live() -> (Option<String>, Option<String>, Vec<String>, bool) {
    let ip = parse_addr(&output("ip", &["-4", "-o", "addr", "show", "dev", IFACE]));
    let gateway = parse_gateway(&output("ip", &["-4", "route", "show", "default"]));
    let dns = parse_resolv(&std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default());
    let link = std::fs::read_to_string(format!("/sys/class/net/{IFACE}/carrier")).map(|s| s.trim() == "1").unwrap_or(false);
    (ip, gateway, dns, link)
}

/// `2: eth0    inet 172.30.1.27/24 brd 172.30.1.255 scope global eth0` -> 172.30.1.27/24
pub fn parse_addr(text: &str) -> Option<String> {
    let mut it = text.split_whitespace();
    while let Some(t) = it.next() {
        if t == "inet" {
            return it.next().map(str::to_string);
        }
    }
    None
}

/// `default via 172.30.1.254 dev eth0  metric 202` -> 172.30.1.254
pub fn parse_gateway(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next() == Some("default") && it.next() == Some("via")).then(|| it.next().map(str::to_string)).flatten()
    })
}

pub fn parse_resolv(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("nameserver"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ip_output() {
        let a = "2: eth0    inet 172.30.1.27/24 brd 172.30.1.255 scope global eth0\\       valid_lft forever";
        assert_eq!(parse_addr(a).as_deref(), Some("172.30.1.27/24"));
        assert_eq!(parse_addr(""), None);
        assert_eq!(parse_gateway("default via 172.30.1.254 dev eth0  metric 202 \n").as_deref(), Some("172.30.1.254"));
        assert_eq!(parse_gateway("172.30.1.0/24 dev eth0 scope link"), None);
        assert_eq!(parse_resolv("search kornet\nnameserver 168.126.63.1\nnameserver 168.126.63.2\n"), [
            "168.126.63.1",
            "168.126.63.2"
        ]);
    }

    #[test]
    fn static_mode_validation() {
        let ok = json!({"mode": "static", "address": "192.168.0.50/24", "gateway": "192.168.0.1", "dns": ["1.1.1.1", "2606:4700:4700::1111"]});
        assert!(matches!(Mode::from_request(&ok), Ok(Mode::Static { prefix: 24, .. })));
        assert_eq!(Mode::from_request(&json!({"mode": "dhcp"})), Ok(Mode::Dhcp));
        for bad in [
            json!({"mode": "static", "address": "192.168.0.50", "gateway": "192.168.0.1", "dns": ["1.1.1.1"]}),
            json!({"mode": "static", "address": "192.168.0.50/33", "gateway": "192.168.0.1", "dns": ["1.1.1.1"]}),
            json!({"mode": "static", "address": "192.168.0.50/24", "gateway": "10.0.0.1", "dns": ["1.1.1.1"]}),
            json!({"mode": "static", "address": "192.168.0.50/24", "gateway": "192.168.0.1", "dns": []}),
            json!({"mode": "static", "address": "192.168.0.50/24", "gateway": "192.168.0.1", "dns": ["; rm -rf /"]}),
            json!({"mode": "pppoe"}),
        ] {
            assert!(Mode::from_request(&bad).is_err(), "{bad}");
        }
    }
}
