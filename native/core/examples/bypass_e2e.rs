//! The VPN bypass end to end on this PC (Windows with Hyper-V; asks for UAC once, changes Hyper-V: run it only
//! when you mean to):
//!
//!     PROXYDOCK_BYPASS_ADAPTER=Wi-Fi cargo run -p proxydock-core --example bypass_e2e
//!
//! Prepares the shared VM on that network card (image from dist/bypass), runs a SOCKS5 card whose Direct exit is
//! `@bypass` and a UDP forward card (DNS to 8.8.8.8 through the VM), prints what the exits see, stops the cards (the
//! VM stops with them) and removes the VM and the switch ProxyDock made. Settings and disks go to temporary
//! folders (PROXYDOCK_HOME, PROXYDOCK_BYPASS_DIR).
use proxydock_core::bypass::{self, Consent, Instance, Store, hub};
use proxydock_core::manager::{Events, Folders, Manager};
use proxydock_core::singbox::helper::Launch;
use proxydock_core::{config, graph, singbox};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn card(id: &str, inbound: Value) -> Value {
    let g = graph::validate(&json!({
        "inbounds": [{"id": id, "name": id, "inbound": inbound, "target": "o"}],
        "outbounds": [{"id": "o", "outbound": {"type": "direct", "interface": "@bypass"}}],
    }));
    Value::Object(graph::compile(&g).remove(0))
}

fn udp_dns(port: u16) -> String {
    let q = b"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01";
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(8))).unwrap();
    s.send_to(q, ("127.0.0.1", port)).unwrap();
    let mut buf = [0u8; 512];
    match s.recv_from(&mut buf) {
        Ok((n, _)) => format!("answer {n} bytes, {} records", u16::from_be_bytes([buf[6], buf[7]])),
        Err(e) => format!("no answer: {e}"),
    }
}

fn main() {
    let adapter = std::env::var("PROXYDOCK_BYPASS_ADAPTER").unwrap_or_else(|_| "Wi-Fi".into());
    let tmp = std::env::temp_dir().join(format!("proxydock-bypass-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    // SAFETY: set before any thread starts
    unsafe {
        std::env::set_var("PROXYDOCK_HOME", tmp.join("home"));
        if std::env::var_os("PROXYDOCK_BYPASS_DIR").is_none() {
            let dir =
                if cfg!(windows) { PathBuf::from(r"C:\ProgramData\ProxyDockBypassE2E") } else { tmp.join("bypass") };
            std::env::set_var("PROXYDOCK_BYPASS_DIR", dir);
        }
        std::env::set_var("PROXYDOCK_BYPASS_IMAGE", root().join("dist").join("bypass").join(bypass::IMAGE_NAME));
    }
    println!("available: {:?}", bypass::availability());
    println!("physical adapters: {:?}", bypass::physical_adapters());
    let mut inst = Instance::new("", &adapter);
    // PROXYDOCK_BYPASS_STATIC=<address/prefix>,<gateway>,<dns>: a fixed address instead of DHCP
    if let Ok(st) = std::env::var("PROXYDOCK_BYPASS_STATIC") {
        let p: Vec<&str> = st.split(',').collect();
        inst.ip_mode = "static".into();
        inst.address = p[0].into();
        inst.gateway = p.get(1).copied().unwrap_or("").into();
        inst.dns = p.get(2).copied().unwrap_or("1.1.1.1").into();
    }
    inst.consent = Some(Consent { time: 0, adapter: adapter.clone() });
    let mut store = Store::default();
    store.put(inst.clone());
    store.save().unwrap();
    {
        let mut h = hub::hub().lock().unwrap();
        // (the helper next to this example: target/<profile>/examples/.. ; as root already: no prompt)
        let exe = std::env::current_exe().unwrap();
        let program =
            exe.parent().unwrap().parent().unwrap().join(format!("bypass_helper{}", std::env::consts::EXE_SUFFIX));
        let elevate = !proxydock_core::manager::is_admin();
        h.launch = Some(Launch { program, args: vec![bypass::helper::ARG.into()], elevate });
        let t = Instant::now();
        let r = h.prepare(&inst, &|m| println!("  {m}"));
        println!("prepare: {r:?} ({:.1}s)", t.elapsed().as_secs_f64());
        if r.is_err() {
            return;
        }
    }
    let exe = singbox::find(singbox::EXE, "", &[root()]).expect("bin/<platform>/sing-box");
    let g = config::validate_global(&json!({"singbox_path": exe.display().to_string()}));
    let events = Events {
        status: Arc::new(|id, st| println!("  [{id}] {st:?}")),
        log: Arc::new(|e| {
            if e.level != proxydock_core::log::Level::Tcp {
                println!("  log: {}", e.message)
            }
        }),
    };
    let folders = Folders {
        run_dir: tmp.join("run"),
        cert_dir: tmp.join("certs"),
        ruleset_dir: tmp.join("rulesets"),
        force_hysteria: false,
        force_helper: None,
    };
    let m = Manager::with_folders(events, folders);
    let socks = card("socks", json!({"type": "socks5", "host": "127.0.0.1", "port": 27180}));
    let dns = card(
        "dns",
        json!({"type": "forward", "host": "127.0.0.1", "port": 27153, "fwd_host": "8.8.8.8", "fwd_port": 53, "fwd_network": "udp"}),
    );
    let t = Instant::now();
    println!("start socks card: {} ({:.1}s)", m.start(&socks, &g).recv().unwrap(), t.elapsed().as_secs_f64());
    let t = Instant::now();
    println!("start dns card (same VM): {} ({:.1}s)", m.start(&dns, &g).recv().unwrap(), t.elapsed().as_secs_f64());
    let curl = |args: &[&str]| {
        std::process::Command::new(if cfg!(windows) { "curl.exe" } else { "curl" })
            .args(args)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    println!(
        "host (VPN): {}",
        curl(&["-s", "-m", "15", "https://am.i.mullvad.net/json"]).chars().take(120).collect::<String>()
    );
    for _ in 0..3 {
        let o = std::process::Command::new(if cfg!(windows) { "curl.exe" } else { "curl" })
            .args(["-sS", "-m", "15", "-x", "socks5h://127.0.0.1:27180", "https://am.i.mullvad.net/json"])
            .output()
            .unwrap();
        println!(
            "card (bypass): {} | {}",
            String::from_utf8_lossy(&o.stdout).chars().take(160).collect::<String>(),
            String::from_utf8_lossy(&o.stderr).trim()
        );
    }
    println!("UDP DNS through the bypass: {}", udp_dns(27153));
    println!(
        "speed: {}",
        curl(&[
            "-s",
            "-o",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
            "-m",
            "60",
            "-w",
            "%{speed_download} B/s",
            "-x",
            "socks5h://127.0.0.1:27180",
            "https://speed.cloudflare.com/__down?bytes=50000000"
        ])
    );
    println!("status: {:?}", hub::hub().lock().unwrap().status(""));
    m.stop("dns").recv().unwrap();
    println!("in use after one card stopped: {}", hub::hub().lock().unwrap().in_use(""));
    m.stop("socks").recv().unwrap();
    println!("status after both stopped: {:?}", hub::hub().lock().unwrap().status("").map(|v| v["vm"].clone()));
    println!("remove: {:?}", hub::hub().lock().unwrap().remove(""));
}
