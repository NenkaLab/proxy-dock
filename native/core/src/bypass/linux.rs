//! The elevated side of the Direct line on Linux: no VM. Each instance is a network namespace
//! (`proxydock-<slug>`) with its own copy of a physical network card - macvlan (bridge) for a wired card, ipvlan
//! (L2) for Wi-Fi, which takes one MAC address only - so the host's routes, VPN and firewall rules (they live in the
//! host's namespace) never see its traffic. Inside it run a DHCP client (or a fixed address) and the bundled sing-box
//! as a SOCKS5 server on 127.0.0.1 with a random account; the helper relays a 127.0.0.1 port of the host to it (a
//! thread that enters the namespace makes each connection).
//!
//! Same requests as the Windows helper: prepare, start, stop, status, remove.

use super::{clamp_memory, valid_name};
use crate::i18n::tr;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// sing-box's SOCKS5 port inside a namespace
const SOCKS_PORT: u16 = 1080;

/// the namespace of an instance
pub fn ns_name(name: &str) -> String {
    if name.is_empty() {
        "proxydock-shared".into()
    } else {
        // (hex: any letters make a valid name; long names are cut, the hash keeps them apart)
        let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
        let short: String = hex.chars().take(24).collect();
        format!("proxydock-{short}-{:08x}", fnv(name))
    }
}

/// the macvlan / ipvlan link while it is still in the host's namespace (15 characters at most)
fn link_name(name: &str) -> String {
    format!("pdl{:08x}", fnv(name))
}

fn fnv(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// a physical card's name as Linux has it: letters, digits, '.', '-', '_' (no path tricks)
pub fn valid_adapter(a: &str) -> bool {
    !a.is_empty()
        && a.len() <= 15
        && a.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && Path::new("/sys/class/net").join(a).exists()
}

/// a wireless card (macvlan does not work there: the access point accepts one MAC per station)
fn is_wireless(a: &str) -> bool {
    let d = Path::new("/sys/class/net").join(a);
    d.join("wireless").exists() || d.join("phy80211").exists()
}

fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd).args(args).stdin(Stdio::null()).output().map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{cmd} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn which(prog: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .chain(["/usr/sbin", "/sbin", "/usr/bin", "/bin"].map(PathBuf::from))
            .map(|d| d.join(prog))
            .find(|f| f.is_file())
    })
}

struct Running {
    children: Vec<Child>,
    stop: Arc<AtomicBool>,
    port: u16,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub struct Helper {
    dir: PathBuf,
    started: HashMap<String, Running>,
}

fn name_of(req: &Value) -> Result<String, String> {
    let n = req["name"].as_str().unwrap_or("").to_string();
    if n.is_empty() || valid_name(&n) { Ok(n) } else { Err(tr("Invalid name")) }
}

impl Helper {
    pub fn new(dir: PathBuf) -> Helper {
        Helper { dir, started: HashMap::new() }
    }

    pub fn handle(&mut self, req: &Value) -> Result<Value, String> {
        let name = name_of(req)?;
        match req["op"].as_str().unwrap_or("") {
            "prepare" => {
                let adapter = req["adapter"].as_str().unwrap_or("");
                if !valid_adapter(adapter) {
                    return Err(tr("Choose a network card"));
                }
                // (nothing to make ahead: the namespace comes and goes with each start)
                Ok(json!({"memory_mb": clamp_memory(req["memory_mb"].as_u64().unwrap_or(0) as u32)}))
            }
            "start" => self.start(&name, req["adapter"].as_str().unwrap_or(""), &req["net"]),
            "stop" | "remove" => {
                self.stop(&name);
                Ok(json!({}))
            }
            "status" => Ok(self.status(&name)),
            op => Err(format!("unknown request {op:?}")),
        }
    }

    fn stop(&mut self, name: &str) {
        self.started.remove(name);
        let ns = ns_name(name);
        let _ = run("ip", &["netns", "del", &ns]);
        let _ = std::fs::remove_dir_all(Path::new("/etc/netns").join(&ns));
        let _ = std::fs::remove_file(self.dir.join(format!("{ns}.json")));
    }

    pub fn stop_all(&mut self) {
        let names: Vec<String> = self.started.keys().cloned().collect();
        for n in names {
            self.stop(&n);
        }
    }

    fn status(&self, name: &str) -> Value {
        let ns = ns_name(name);
        if !Path::new("/run/netns").join(&ns).exists() {
            return json!({"vm": Value::Null});
        }
        json!({"vm": "Running", "relay": self.started.get(name).map(|r| r.port), "agent": live(&ns)})
    }

    fn start(&mut self, name: &str, adapter: &str, net: &Value) -> Result<Value, String> {
        if !valid_adapter(adapter) {
            return Err(tr("Choose a network card"));
        }
        let singbox = crate::singbox::find_bundled(crate::singbox::EXE).ok_or_else(|| tr("sing-box was not found"))?;
        self.stop(name);
        let ns = ns_name(name);
        let link = link_name(name);
        let up = || -> Result<(), String> {
            run("ip", &["netns", "add", &ns])?;
            // `ip netns exec` puts /etc/netns/<ns>/resolv.conf over /etc/resolv.conf: the namespace's own DNS
            let etc = Path::new("/etc/netns").join(&ns);
            std::fs::create_dir_all(&etc).map_err(|e| e.to_string())?;
            std::fs::write(etc.join("resolv.conf"), "").map_err(|e| e.to_string())?;
            if is_wireless(adapter) {
                run("ip", &["link", "add", &link, "link", adapter, "type", "ipvlan", "mode", "l2"])?;
            } else {
                run("ip", &["link", "add", &link, "link", adapter, "type", "macvlan", "mode", "bridge"])?;
            }
            run("ip", &["link", "set", &link, "netns", &ns])?;
            run("ip", &["-n", &ns, "link", "set", &link, "name", "eth0"])?;
            run("ip", &["-n", &ns, "link", "set", "lo", "up"])?;
            run("ip", &["-n", &ns, "link", "set", "eth0", "up"])?;
            Ok(())
        };
        if let Err(e) = up() {
            self.stop(name);
            return Err(e);
        }
        let mut children = vec![];
        let result = (|| -> Result<Value, String> {
            if net["mode"] == json!("static") {
                let s = |k: &str| net[k].as_str().unwrap_or("").to_string();
                run("ip", &["-n", &ns, "addr", "add", &s("address"), "dev", "eth0"])?;
                run("ip", &["-n", &ns, "route", "replace", "default", "via", &s("gateway"), "dev", "eth0"])?;
                let dns: String = net["dns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|d| d.parse::<std::net::IpAddr>().is_ok())
                    .map(|d| format!("nameserver {d}\n"))
                    .collect();
                std::fs::write(Path::new("/etc/netns").join(&ns).join("resolv.conf"), dns)
                    .map_err(|e| e.to_string())?;
            } else {
                children.push(dhcp_client(&ns)?);
            }
            let until = Instant::now() + Duration::from_secs(40);
            let status = loop {
                let st = live(&ns);
                if st["ip"].is_string() && st["gateway"].is_string() {
                    break st;
                }
                if Instant::now() > until {
                    return Err(
                        tr("The direct line VM got no network: {e}").replace("{e}", &tr("no address from DHCP"))
                    );
                }
                std::thread::sleep(Duration::from_millis(300));
            };
            let user = crate::singbox::process::random_hex(12);
            let pass = crate::singbox::process::random_hex(24);
            let cfg_path = self.dir.join(format!("{ns}.json"));
            let cfg = json!({
                "log": {"level": "warn"},
                "dns": {"servers": [{"type": "local", "tag": "local"}]},
                "inbounds": [{"type": "socks", "tag": "in", "listen": "127.0.0.1", "listen_port": SOCKS_PORT,
                              "users": [{"username": user, "password": pass}]}],
                "outbounds": [{"type": "direct", "tag": "direct"}],
                "route": {"final": "direct", "default_domain_resolver": "local"}
            });
            std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
            write_private(&cfg_path, cfg.to_string().as_bytes())?;
            let sb = Command::new("ip")
                .args(["netns", "exec", &ns])
                .arg(&singbox)
                .args(["run", "-c"])
                .arg(&cfg_path)
                .args(["--disable-color"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("sing-box: {e}"))?;
            children.push(sb);
            let until = Instant::now() + Duration::from_secs(10);
            while connect_in(&ns, SOCKS_PORT).is_err() {
                if Instant::now() > until {
                    return Err(tr("The direct line VM did not answer: {e}").replace("{e}", "sing-box"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let (port, stop) = relay(ns.clone())?;
            self.started.insert(name.to_string(), Running { children: std::mem::take(&mut children), stop, port });
            Ok(json!({"port": port, "user": user, "pass": pass, "status": status}))
        })();
        if result.is_err() {
            for c in &mut children {
                let _ = c.kill();
                let _ = c.wait();
            }
            self.stop(name);
        }
        result
    }
}

/// a config file only root can read (it holds the account)
fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.write_all(data).map_err(|e| e.to_string())
}

/// a DHCP client in the namespace, kept running so it renews the lease: busybox udhcpc, dhcpcd or dhclient
fn dhcp_client(ns: &str) -> Result<Child, String> {
    let spawn = |prog: &Path, args: &[&str]| {
        Command::new("ip")
            .args(["netns", "exec", ns])
            .arg(prog)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())
    };
    if let Some(p) = which("udhcpc") {
        return spawn(&p, &["-f", "-i", "eth0", "-x", "hostname:proxydock-bypass"]);
    }
    if let Some(p) = which("busybox") {
        return spawn(&p, &["udhcpc", "-f", "-i", "eth0"]);
    }
    if let Some(p) = which("dhcpcd") {
        return spawn(&p, &["-B", "-4", "eth0"]);
    }
    if let Some(p) = which("dhclient") {
        return spawn(&p, &["-d", "-4", "eth0"]);
    }
    Err(tr("No DHCP client was found (udhcpc, dhcpcd or dhclient); use a fixed address"))
}

/// address, gateway and DNS servers of eth0 in the namespace
fn live(ns: &str) -> Value {
    let addr = run("ip", &["-n", ns, "-4", "-o", "addr", "show", "dev", "eth0"]).unwrap_or_default();
    let ip = addr.split_whitespace().skip_while(|t| *t != "inet").nth(1).map(String::from);
    let route = run("ip", &["-n", ns, "-4", "route", "show", "default"]).unwrap_or_default();
    let gateway = route.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next() == Some("default") && it.next() == Some("via")).then(|| it.next().map(String::from)).flatten()
    });
    let resolv = std::fs::read_to_string(Path::new("/etc/netns").join(ns).join("resolv.conf")).unwrap_or_default();
    let dns: Vec<String> = resolv
        .lines()
        .filter_map(|l| l.trim().strip_prefix("nameserver"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect();
    json!({"ip": ip, "gateway": gateway, "dns": dns, "link": true, "singbox": true})
}

/// a TCP connection made inside the namespace (from a thread that enters it; the socket stays there)
fn connect_in(ns: &str, port: u16) -> std::io::Result<TcpStream> {
    let path = Path::new("/run/netns").join(ns);
    std::thread::scope(|s| {
        s.spawn(|| {
            let f = std::fs::File::open(&path)?;
            if unsafe { libc::setns(f.as_raw_fd(), libc::CLONE_NEWNET) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(3))
        })
        .join()
        .unwrap_or_else(|_| Err(std::io::Error::other("thread")))
    })
}

/// 127.0.0.1:<port> on the host -> sing-box in the namespace
fn relay(ns: String) -> Result<(u16, Arc<AtomicBool>), String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = l.local_addr().map_err(|e| e.to_string())?.port();
    l.set_nonblocking(true).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    std::thread::spawn(move || {
        while !st.load(Ordering::SeqCst) {
            match l.accept() {
                Ok((c, _)) => {
                    let ns = ns.clone();
                    std::thread::spawn(move || {
                        let _ = c.set_nonblocking(false);
                        let Ok(v) = connect_in(&ns, SOCKS_PORT) else { return };
                        pipe(c, v);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    });
    Ok((port, stop))
}

fn pipe(a: TcpStream, b: TcpStream) {
    let _ = a.set_nodelay(true);
    let _ = b.set_nodelay(true);
    let (Ok(mut a2), Ok(mut b2)) = (a.try_clone(), b.try_clone()) else { return };
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut a2, &mut b2);
        let _ = b2.shutdown(Shutdown::Write);
    });
    let (mut a, mut b) = (a, b);
    let _ = std::io::copy(&mut b, &mut a);
    let _ = a.shutdown(Shutdown::Write);
    let _ = t.join();
}

/// physical network cards (with a device behind them), sorted
pub fn physical_adapters() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir("/sys/class/net")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("device").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fit_linux_limits() {
        assert_eq!(ns_name(""), "proxydock-shared");
        let long = ns_name("게임 서버 아주 긴 이름입니다");
        assert!(long.starts_with("proxydock-") && long.len() < 64);
        assert_ne!(ns_name("a"), ns_name("b"));
        assert!(link_name("anything").len() <= 15);
        assert!(!valid_adapter("../etc"));
        assert!(!valid_adapter(""));
    }
}
