//! "Check connection" in the editor (Python: `probe.check_outbound`, `probe_site`, `checker_for`, the editor's
//! `_check_inbound` / `_probe_hint` / `_on_probe_done`, and `platform_utils.check_port`).
//!
//! A temporary SOCKS5 card on a free loopback port gets the edited block's outbound (and the card's local
//! settings); a connection to an HTTPS site through it shows whether the outbound works.

use crate::i18n::tr;
use crate::manager::{Remote, State};
use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
pub const CHECK_SITES: [&str; 3] = ["www.google.com", "www.cloudflare.com", "www.microsoft.com"];

// --- errors as the Python app words them ---

fn winerror(e: &std::io::Error) -> Option<i32> {
    if cfg!(windows) { e.raw_os_error() } else { None }
}

/// Python's exception class for an OS error
fn py_class(e: &std::io::Error) -> &'static str {
    use std::io::ErrorKind::*;
    match e.kind() {
        ConnectionRefused => "ConnectionRefusedError",
        ConnectionReset => "ConnectionResetError",
        ConnectionAborted => "ConnectionAbortedError",
        BrokenPipe => "BrokenPipeError",
        TimedOut | WouldBlock => "TimeoutError",
        PermissionDenied => "PermissionError",
        _ => "OSError",
    }
}

/// `str(e)` of a Python OS error: "[WinError 10054] text"
fn py_text(e: &std::io::Error) -> String {
    let full = e.to_string();
    match winerror(e) {
        Some(code) => {
            let text = full.strip_suffix(&format!(" (os error {code})")).unwrap_or(&full);
            format!("[WinError {code}] {text}")
        }
        None => full,
    }
}

/// `common.describe_error`
pub fn describe_error(e: &std::io::Error) -> String {
    let code = winerror(e);
    if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) || code == Some(10060) {
        return tr("Connection timed out");
    }
    if e.kind() == std::io::ErrorKind::ConnectionRefused {
        return tr("Connection refused");
    }
    let suffix = code.map(|c| format!(" (WinError {c})")).unwrap_or_default();
    if matches!(code, Some(10051 | 10050)) || e.kind() == std::io::ErrorKind::NetworkUnreachable || e.kind() == std::io::ErrorKind::NetworkDown {
        return tr("Network unreachable") + &suffix;
    }
    if code == Some(10065) || e.kind() == std::io::ErrorKind::HostUnreachable {
        return tr("Host unreachable") + &suffix;
    }
    format!("{}{suffix}: {}", py_class(e), py_text(e))
}

/// `platform_utils.check_port`: (available, description). The port is bound exclusively, so a port another
/// program shares does not look free.
pub fn check_port(host: &str, port: u16, udp: bool) -> (bool, String) {
    let bind_host = if host == "localhost" { "127.0.0.1" } else { host };
    let addr = match (crate::net::strip_scope(bind_host), port).to_socket_addrs().map(|mut a| a.next()) {
        Ok(Some(a)) => a,
        Ok(None) => return (false, tr("This PC does not have the address {bind_host}.").replace("{bind_host}", bind_host)),
        Err(e) => return (false, e.to_string()),
    };
    let domain = if addr.is_ipv6() { socket2::Domain::IPV6 } else { socket2::Domain::IPV4 };
    let ty = if udp { socket2::Type::DGRAM } else { socket2::Type::STREAM };
    let s = match socket2::Socket::new(domain, ty, None) {
        Ok(s) => s,
        Err(e) => return (false, e.to_string()),
    };
    #[cfg(windows)]
    unsafe {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{setsockopt, SOCKET, SOL_SOCKET, SO_EXCLUSIVEADDRUSE};
        let one: i32 = 1;
        setsockopt(s.as_raw_socket() as SOCKET, SOL_SOCKET, SO_EXCLUSIVEADDRUSE, &one as *const i32 as *const u8, 4);
    }
    match s.bind(&addr.into()) {
        Ok(()) => (true, tr("{bind_host}:{port} is available.").replace("{bind_host}", bind_host).replace("{port}", &port.to_string())),
        Err(e) => {
            let code = e.raw_os_error();
            let msg = if code == Some(10048) || e.kind() == std::io::ErrorKind::AddrInUse {
                tr("Another program (or another running proxy) is already using this port.")
            } else if code == Some(10013) {
                tr("The port is in a range reserved by Windows, or access was denied (Hyper-V/WSL reservations etc.).")
            } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                tr("Permission denied (ports below 1024 need administrator rights).")
            } else if code == Some(10049) || e.kind() == std::io::ErrorKind::AddrNotAvailable {
                tr("This PC does not have the address {bind_host}.").replace("{bind_host}", bind_host)
            } else {
                py_text(&e)
            };
            (false, msg)
        }
    }
}

// --- one site through the SOCKS5 card ---

fn rep_name(rep: u8) -> String {
    match rep {
        1 => tr("Proxy error"),
        2 => tr("Blocked (allow list / private network)"),
        3 => tr("Network unreachable"),
        4 => tr("Host unreachable (DNS / route / upstream proxy)"),
        5 => tr("Connection refused"),
        6 => tr("TTL expired"),
        7 => tr("Command not supported"),
        8 => tr("Address type not supported"),
        n => tr("SOCKS error {rep}").replace("{rep}", &n.to_string()),
    }
}

enum Fail {
    Socks(String),
    Io(std::io::Error),
}

fn recvn(s: &mut TcpStream, n: usize) -> Result<Vec<u8>, Fail> {
    let mut buf = vec![0u8; n];
    let mut got = 0;
    while got < n {
        match s.read(&mut buf[got..]) {
            Ok(0) => return Err(Fail::Io(std::io::Error::other(CLOSED))),
            Ok(k) => got += k,
            Err(e) => return Err(Fail::Io(e)),
        }
    }
    Ok(buf)
}

/// marks "the proxy closed the connection" (Python raises a plain ConnectionError for it)
const CLOSED: &str = "\u{0}closed";

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// the error of a rustls stream, if it is one
fn tls_error(e: &std::io::Error) -> Option<&rustls::Error> {
    e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>())
}

fn roots() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

/// `probe_site`: HTTPS to host:443 through the SOCKS5 proxy on 127.0.0.1:port. Keys: ok, status, connect_ms,
/// total_ms, error, kind ("" / "socks" / "tls" / "timeout" / "closed" / "net"), note.
pub fn probe_site(host: &str, port: u16) -> Map<String, Value> {
    let mut res = json!({"ok": false, "status": "", "connect_ms": null, "total_ms": null, "error": "", "kind": ""}).as_object().unwrap().clone();
    let t0 = Instant::now();
    let local: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = match TcpStream::connect_timeout(&local, PROBE_TIMEOUT) {
        Ok(s) => s,
        Err(e) => {
            res.insert("error".into(), json!(tr("Test connection failed: {describe_error}").replace("{describe_error}", &describe_error(&e))));
            return res;
        }
    };
    let _ = s.set_read_timeout(Some(PROBE_TIMEOUT));
    let _ = s.set_write_timeout(Some(PROBE_TIMEOUT));
    let r = (|| -> Result<(), Fail> {
        s.write_all(b"\x05\x01\x00").map_err(Fail::Io)?;
        recvn(&mut s, 2)?;
        let hb = host.as_bytes();
        let mut req = vec![5, 1, 0, 3, hb.len() as u8];
        req.extend_from_slice(hb);
        req.extend_from_slice(&443u16.to_be_bytes());
        s.write_all(&req).map_err(Fail::Io)?;
        let rep = recvn(&mut s, 4)?;
        if rep[1] != 0 {
            return Err(Fail::Socks(rep_name(rep[1])));
        }
        recvn(&mut s, if rep[3] == 4 { 18 } else { 6 })?;
        res.insert("connect_ms".into(), json!(ms(t0)));
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| Fail::Io(std::io::Error::other(e.to_string())))?;
        let conn = rustls::ClientConnection::new(roots(), name).map_err(|e| Fail::Io(std::io::Error::other(e)))?;
        let mut tls = rustls::StreamOwned::new(conn, s.try_clone().map_err(Fail::Io)?);
        let head = format!("HEAD / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64)\r\nConnection: close\r\n\r\n");
        tls.write_all(head.as_bytes()).map_err(Fail::Io)?;
        let mut buf = [0u8; 512];
        let n = match tls.read(&mut buf) {
            Ok(n) => n,
            // a server that closes without close_notify: what arrived before counts
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && tls.conn.is_handshaking() == false => 0,
            Err(e) => return Err(Fail::Io(e)),
        };
        let text: String = buf[..n].iter().map(|&b| b as char).collect();
        let line = text.split("\r\n").next().unwrap_or("");
        let parts: Vec<&str> = line.split_whitespace().collect();
        let status = if parts.len() > 1 && line.starts_with("HTTP/") { parts[1].to_string() } else { tr("No response") };
        let ok = !status.is_empty() && status.chars().all(|c| c.is_ascii_digit());
        res.insert("status".into(), json!(status));
        res.insert("ok".into(), json!(ok));
        if !ok {
            res.insert("error".into(), json!(tr("Not an HTTP response")));
        }
        Ok(())
    })();
    match r {
        Ok(()) => {}
        Err(Fail::Socks(m)) => {
            res.insert("error".into(), json!(m));
            res.insert("kind".into(), json!("socks"));
        }
        Err(Fail::Io(e)) => {
            if let Some(te) = tls_error(&e) {
                if matches!(te, rustls::Error::InvalidCertificate(_)) {
                    // the route and the TLS server were reached; only this PC's root certificates are in question
                    res.insert("ok".into(), json!(true));
                    res.insert("note".into(), json!(tr("Certificate not verified (no local root certificate)")));
                } else {
                    res.insert("error".into(), json!(tr("TLS error ({value})").replace("{value}", &te.to_string())));
                    res.insert("kind".into(), json!("tls"));
                }
            } else if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
                res.insert("error".into(), json!(tr("Timed out")));
                res.insert("kind".into(), json!("timeout"));
            } else if e.kind() == std::io::ErrorKind::UnexpectedEof {
                res.insert("error".into(), json!(tr("TLS error ({value})").replace("{value}", "EOF occurred in violation of protocol")));
                res.insert("kind".into(), json!("tls"));
            } else if e.to_string() == CLOSED {
                res.insert("error".into(), json!(format!("ConnectionError: {}", tr("The proxy closed the connection"))));
                res.insert("kind".into(), json!("closed"));
            } else {
                use std::io::ErrorKind::*;
                let closed = matches!(e.kind(), ConnectionRefused | ConnectionReset | ConnectionAborted | BrokenPipe);
                res.insert("error".into(), json!(describe_error(&e)));
                res.insert("kind".into(), json!(if closed { "closed" } else { "net" }));
            }
        }
    }
    let _ = s.shutdown(std::net::Shutdown::Both);
    res.insert("total_ms".into(), json!(ms(t0)));
    res
}

// --- the temporary card ---

/// `checker_for`: a SOCKS5 card on 127.0.0.1:`port` with the proxy's outbound (its whole chain: Python's lost the
/// "via" chain and group members here) and the proxy's local settings, every destination allowed, not logged
pub fn checker_for(proxy: &Value, glob: &Map<String, Value>, id: &str, port: u16) -> Value {
    let mut p = crate::config::validate_proxy(&json!({
        "id": id, "name": tr("Check connection"),
        "inbound": {"type": "socks5", "host": "127.0.0.1", "port": 1}, "outbound": proxy["outbound"].clone(),
    }));
    for k in ["via", "members"] {
        if let Some(v) = proxy["outbound"].get(k) {
            p["outbound"][k] = v.clone();
        }
    }
    p["inbound"]["port"] = json!(port);
    let mut ov = crate::config::resolve(proxy, glob);
    ov.insert("allowed_domains".into(), json!(["*"]));
    ov.insert("log_connections".into(), json!(false));
    ov.insert("log_close".into(), json!(false));
    p.insert("overrides".into(), Value::Object(ov));
    Value::Object(p)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map_or(0, |a| a.port())
}

/// `check_outbound`: one of CHECK_SITES over HTTPS through the outbound. The result of probe_site plus site,
/// detail (the failure the card logged) and reason (its kind: auth / denied / net).
pub fn check_outbound(mgr: &Remote, proxy: &Value, glob: &Map<String, Value>, check_id: &str) -> Map<String, Value> {
    let mut res = json!({"ok": false, "site": "", "error": "", "detail": ""}).as_object().unwrap().clone();
    let port = free_port();
    let checker = checker_for(proxy, glob, check_id, port);
    let started = mgr.start(&checker, glob).recv_timeout(Duration::from_secs(20)).unwrap_or(false);
    if !started {
        let msg = match mgr.state(check_id) {
            State::Error(m) if !m.is_empty() => m,
            _ => tr("Could not create the check connection"),
        };
        res.insert("error".into(), json!(msg));
        let _ = mgr.stop(check_id).recv_timeout(Duration::from_secs(5));
        return res;
    }
    for site in CHECK_SITES {
        mgr.clear_last(check_id);
        let mut r = probe_site(site, port);
        if r["ok"] != json!(true) {
            // the card's log line about the failure may come a moment later
            let until = Instant::now() + Duration::from_millis(400);
            while mgr.last_error(check_id).is_empty() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        r.insert("site".into(), json!(site));
        r.insert("detail".into(), json!(mgr.last_error(check_id)));
        r.insert("reason".into(), json!(mgr.last_reason(check_id)));
        let stop = r["ok"] == json!(true) || r["kind"] == json!("timeout"); // a timeout is a route problem: the others would time out too
        res = r;
        if stop {
            break;
        }
    }
    let _ = mgr.stop(check_id).recv_timeout(Duration::from_secs(5));
    res
}

/// `_check_inbound`: (ok, description) of the edited card's inbound
pub fn check_inbound(p: &Value, running: bool) -> (bool, String) {
    let ib = &p["inbound"];
    let host = ib["host"].as_str().unwrap_or("");
    let port = ib["port"].as_i64().unwrap_or(0);
    let it = ib["type"].as_str().unwrap_or("");
    if it == "tun" {
        return (true, tr("TUN: checked when it starts (needs administrator permission)"));
    }
    if running {
        return (true, tr("Running on {host_port}").replace("{host_port}", &crate::net::host_port(host, port)));
    }
    let (ok, msg) = check_port(host, port.clamp(0, 65535) as u16, it == "hysteria2");
    if !ok {
        return (false, tr("Port {port}: {msg}").replace("{port}", &port.to_string()).replace("{msg}", &msg));
    }
    let cert = ib["tls_cert"].as_str().unwrap_or("");
    if matches!(it, "https" | "hysteria2") && !cert.is_empty() {
        if let Err(e) = load_cert_key(cert, ib["tls_key"].as_str().unwrap_or("")) {
            return (false, tr("Could not read the certificate/key: {e}").replace("{e}", &e));
        }
    }
    (true, tr("Port {port} available").replace("{port}", &port.to_string()) + &if cert.is_empty() { String::new() } else { tr(" · certificate OK") })
}

/// whether a PEM certificate and key load and belong together (Python `tls.server_context`)
fn load_cert_key(cert: &str, key: &str) -> Result<(), String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs: Vec<CertificateDer> = CertificateDer::pem_file_iter(cert).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    if certs.is_empty() {
        return Err("no certificate".into());
    }
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| e.to_string())?;
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `_probe_hint`: a hint from the failure kind and the reason the card logged
pub fn probe_hint(ot: &str, out: &Map<String, Value>) -> String {
    let g = |k: &str| out.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let (kind, reason, detail) = (g("kind"), g("reason"), g("detail").to_lowercase());
    if ot == "shadowsocks" && matches!(kind.as_str(), "timeout" | "tls" | "closed") {
        return tr("Check that the cipher and password match the server (the server does not answer if they are wrong)");
    }
    if ot == "hysteria2" && reason == "auth" {
        return tr("Check the Hysteria2 password");
    }
    if matches!(ot, "http" | "https" | "socks5") && reason == "auth" {
        return tr("Check the upstream proxy username/password");
    }
    if matches!(ot, "https" | "hysteria2") && detail.contains("certificate") {
        return tr("If the server uses a self-signed certificate, turn off 'Verify server certificate' or pin its fingerprint");
    }
    String::new()
}

/// `_on_probe_done`: (all well, the text under the button)
pub fn result_text(ot: &str, out: &Map<String, Value>, inbound: Option<(bool, String)>) -> (bool, String) {
    let g = |k: &str| out.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let ms = |k: &str| out.get(k).and_then(Value::as_f64).map_or("-".to_string(), |v| format!("{v:.0} ms"));
    let out_ok = out.get("ok") == Some(&json!(true));
    let mut line = if out_ok {
        let status = g("status");
        let mut l = tr("Outbound OK · {site} replied {value} · connect {ms} · total {ms2}")
            .replace("{site}", &g("site"))
            .replace("{value}", if status.is_empty() { "-" } else { &status })
            .replace("{ms2}", &ms("total_ms"))
            .replace("{ms}", &ms("connect_ms"));
        if !g("note").is_empty() {
            l += &format!(" ({})", g("note"));
        }
        l
    } else {
        let err = g("error");
        let mut l = tr("Outbound failed · {value}").replace("{value}", &if err.is_empty() { tr("unknown error") } else { err });
        if !g("detail").is_empty() {
            l += &format!("\n  \u{21b3} {}", g("detail"));
        }
        let hint = probe_hint(ot, out);
        if !hint.is_empty() {
            l += &format!("\n  \u{21b3} {hint}");
        }
        l
    };
    let mut in_ok = true;
    if let Some((ok, msg)) = inbound {
        in_ok = ok;
        line += &tr("\nInbound {value} · {in_msg}").replace("{value}", &if ok { tr("OK") } else { tr("problem") }).replace("{in_msg}", &msg);
    }
    (out_ok && in_ok, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_check() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (ok, msg) = check_port("127.0.0.1", port, false);
        assert!(!ok, "{msg}");
        drop(l);
        assert!(check_port("localhost", port, false).0);
        assert!(!check_port("192.0.2.77", port, false).0, "an address this PC does not have");
    }

    #[test]
    fn nothing_listening() {
        let r = probe_site("www.example.com", free_port());
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("refused"), "{r:?}");
        let (ok, text) = result_text("shadowsocks", &r, Some((true, "x".into())));
        assert!(!ok && text.contains('\n'));
    }

    #[test]
    fn socks_refusal() {
        // a SOCKS5 server that refuses every destination (rep 2: blocked)
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut b = [0u8; 300];
                let _ = s.read(&mut b);
                let _ = s.write_all(b"\x05\x00");
                let _ = s.read(&mut b);
                let _ = s.write_all(b"\x05\x02\x00\x01\x00\x00\x00\x00\x00\x00");
            }
        });
        let r = probe_site("www.example.com", port);
        assert_eq!((r["kind"].as_str(), r["error"].as_str()), (Some("socks"), Some("Blocked (allow list / private network)")));
    }
}

/// Outbound blocks a latency test can measure (every one but Block; Python `latency_targets`)
pub fn latency_targets(g: &crate::graph::Obj) -> Vec<String> {
    g.get("outbounds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|n| n["outbound"]["type"] != "block")
        .filter_map(|n| n["id"].as_str().map(String::from))
        .collect()
}

/// A proxy that leaves through outbound block `oid`, for `check_outbound` (Python `probe.outbound_proxy`)
pub fn outbound_proxy(g: &crate::graph::Obj, oid: &str) -> Value {
    let name = crate::graph::find(g, oid).map_or(Value::String(oid.into()), |(_, n)| n["name"].clone());
    let ob = crate::graph::resolve_outbound(g, oid);
    let mut p = Value::Object(crate::config::validate_proxy(&serde_json::json!({
        "id": "__latency__", "name": name,
        "inbound": {"type": "socks5", "host": "127.0.0.1", "port": 1}, "outbound": ob,
    })));
    p["outbound"]["via"] = ob.get("via").cloned().unwrap_or(Value::Null);
    if let Some(m) = ob.get("members") {
        p["outbound"]["members"] = m.clone();
    }
    p
}

/// What a latency result depends on (the block and its chain): a result is shown only while this is unchanged
/// (Python `latency_key`)
pub fn latency_key(g: &crate::graph::Obj, oid: &str) -> Value {
    crate::graph::resolve_outbound(g, oid)
}

// --- the connection test page (Python `ui/conntest.py`) ---

/// sites tried at the same time, seconds before the test can run again, the temporary bridge card's id
pub const PROBE_WORKERS: usize = 6;
pub const PROBE_COOLDOWN: u64 = 15;
pub const BRIDGE_ID: &str = "__probe__";

/// the built-in test sites: (region, name, host); the region is translated with tr (Python `TEST_SITES`)
pub const TEST_SITES: [(&str, &str, &str); 21] = [
    ("Global", "Google", "www.google.com"),
    ("Global", "YouTube", "www.youtube.com"),
    ("Global", "Microsoft", "www.microsoft.com"),
    ("Global", "Apple", "www.apple.com"),
    ("Global", "Amazon", "www.amazon.com"),
    ("Global", "Cloudflare", "www.cloudflare.com"),
    ("Global", "GitHub", "github.com"),
    ("Global", "Wikipedia", "www.wikipedia.org"),
    ("Global", "Netflix", "www.netflix.com"),
    ("Korea", "NAVER", "www.naver.com"),
    ("Japan", "Yahoo! JAPAN", "www.yahoo.co.jp"),
    ("China", "Baidu", "www.baidu.com"),
    ("USA", "CNN", "www.cnn.com"),
    ("UK", "BBC", "www.bbc.co.uk"),
    ("Germany", "DER SPIEGEL", "www.spiegel.de"),
    ("France", "Le Monde", "www.lemonde.fr"),
    ("Brazil", "Globo", "www.globo.com"),
    ("India", "Flipkart", "www.flipkart.com"),
    ("Australia", "ABC", "www.abc.net.au"),
    ("Canada", "CBC", "www.cbc.ca"),
    ("Russia", "Yandex", "ya.ru"),
];

/// Every site of the test: (region, name, host, added by the user) (Python `ConnTestInterface.sites`)
pub fn test_sites(glob: &Map<String, Value>) -> Vec<(String, String, String, bool)> {
    let mut out: Vec<_> = TEST_SITES.iter().map(|(g, n, h)| (tr(g), n.to_string(), h.to_string(), false)).collect();
    for h in glob.get("test_sites").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        out.push((tr("Added"), h.to_string(), h.to_string(), true));
    }
    out
}

/// The temporary SOCKS5 card on 127.0.0.1:`port` that leads to the card under test (Python `bridge_for`)
pub fn bridge_for(proxy: &Value, port: u16) -> Value {
    let ob = crate::graph::chain_outbound(&proxy["inbound"]);
    let mut p = crate::config::validate_proxy(&json!({
        "id": BRIDGE_ID, "name": tr("Connection test"),
        "inbound": {"type": "socks5", "host": "127.0.0.1", "port": 1}, "outbound": ob,
    }));
    p["inbound"]["port"] = json!(port);
    if let Some(ov) = p.get_mut("overrides").and_then(Value::as_object_mut) {
        ov.insert("allowed_domains".into(), json!(["*"]));
        ov.insert("block_private".into(), json!(false));
        ov.insert("log_connections".into(), json!(false));
        ov.insert("log_close".into(), json!(false));
    }
    Value::Object(p)
}

/// A free loopback port (for a temporary card)
pub fn free_local_port() -> u16 {
    free_port()
}

/// One row's result columns: (result, connect, total, ok) (Python `ConnTestInterface._on_result`)
pub fn site_result(res: &Map<String, Value>) -> (String, String, String, bool) {
    let ms = |k: &str| res.get(k).and_then(Value::as_f64).map_or("-".to_string(), |v| format!("{v:.0} ms"));
    let g = |k: &str| res.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let ok = res.get("ok") == Some(&json!(true));
    let text = if ok {
        if !g("note").is_empty() {
            tr("Connected · {note}").replace("{note}", &g("note"))
        } else {
            tr("OK · HTTP {status}").replace("{status}", &g("status"))
        }
    } else {
        tr("Failed · {error}").replace("{error}", &g("error"))
    };
    (text, ms("connect_ms"), ms("total_ms"), ok)
}
