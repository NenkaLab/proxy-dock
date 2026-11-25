//! Read a server's certificate to fill in the SNI, "Detect from server" in the editor (Python: `certinfo.py`).
//!
//! The official Hysteria2 server (and many TLS servers) refuse a handshake whose SNI does not match their
//! certificate. When an outbound points at an IP, the name to use is in the certificate: connect once without
//! verification (TLS over TCP, or QUIC for Hysteria2 / TUIC), read the certificate's DNS names and its SHA-256.

use crate::i18n::tr;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::Value;
use sha2::Digest;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(8);
pub const QUIC_TYPES: [&str; 2] = ["hysteria2", "tuic"];

#[derive(Debug, PartialEq)]
pub struct Detected {
    pub names: Vec<String>,
    /// the first name that is not a wildcard ("" if none)
    pub sni: String,
    /// hex
    pub sha256: String,
}

/// accepts any certificate and keeps the server's own one
#[derive(Debug)]
struct Capture {
    cert: Mutex<Option<Vec<u8>>>,
    schemes: Vec<SignatureScheme>,
}

impl ServerCertVerifier for Capture {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        *self.cert.lock().unwrap() = Some(end_entity.to_vec());
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes.clone()
    }
}

fn capture() -> Arc<Capture> {
    let schemes = rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes();
    Arc::new(Capture { cert: Mutex::new(None), schemes })
}

fn client_config(cap: &Arc<Capture>, versions: &[&'static rustls::SupportedProtocolVersion], alpn: &[&[u8]]) -> Result<rustls::ClientConfig, String> {
    let mut c = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(versions)
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(cap.clone())
        .with_no_client_auth();
    c.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    Ok(c)
}

/// the name to send, or an IP when none may be sent (rustls sends no SNI for an IP)
fn server_name(sni: Option<&str>, host: &str) -> Result<ServerName<'static>, String> {
    match sni {
        Some(n) => ServerName::try_from(n.to_string()).map_err(|e| e.to_string()),
        None => Ok(ServerName::IpAddress(crate::net::strip_scope(host).parse::<std::net::IpAddr>().unwrap_or(std::net::IpAddr::from([0, 0, 0, 0])).into())),
    }
}

fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    (crate::net::strip_scope(host), port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or_else(|| "no address".to_string())
}

/// the server's certificate (DER) over TLS on TCP
pub fn tcp_certificate(host: &str, port: u16, sni: Option<&str>) -> Result<Vec<u8>, String> {
    let cap = capture();
    let cfg = client_config(&cap, &[&rustls::version::TLS13, &rustls::version::TLS12], &[])?;
    let mut conn = rustls::ClientConnection::new(Arc::new(cfg), server_name(sni, host)?).map_err(|e| e.to_string())?;
    let addr = resolve(host, port)?;
    let mut tcp = std::net::TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| e.to_string())?;
    let _ = tcp.set_read_timeout(Some(TIMEOUT));
    let _ = tcp.set_write_timeout(Some(TIMEOUT));
    while conn.is_handshaking() {
        let r = conn.complete_io(&mut tcp);
        if let Some(c) = cap.cert.lock().unwrap().take() {
            return Ok(c);
        }
        r.map_err(|e| e.to_string())?;
    }
    cap.cert.lock().unwrap().take().ok_or_else(|| tr("The server sent no certificate"))
}

/// the server's certificate (DER) over QUIC (ALPN h3, as Hysteria2 uses)
pub fn quic_certificate(host: &str, port: u16, sni: Option<&str>) -> Result<Vec<u8>, String> {
    use quinn_proto::{ClientConfig, DatagramEvent, Endpoint, EndpointConfig, Event};
    let cap = capture();
    let cfg = client_config(&cap, &[&rustls::version::TLS13], &[b"h3"])?;
    let quic = quinn_proto::crypto::rustls::QuicClientConfig::try_from(cfg).map_err(|e| e.to_string())?;
    let remote = resolve(host, port)?;
    let local: SocketAddr = if remote.is_ipv4() { "0.0.0.0:0".parse().unwrap() } else { "[::]:0".parse().unwrap() };
    let sock = std::net::UdpSocket::bind(local).map_err(|e| e.to_string())?;
    let mut endpoint = Endpoint::new(Arc::new(EndpointConfig::default()), None, false, None);
    let name = match sni {
        Some(n) => n.to_string(),
        None => crate::net::strip_scope(host).to_string(),
    };
    let (ch, mut conn) = endpoint.connect(Instant::now(), ClientConfig::new(Arc::new(quic)), remote, &name).map_err(|e| e.to_string())?;
    let until = Instant::now() + TIMEOUT;
    let (mut out, mut resp) = (Vec::with_capacity(1500), vec![]);
    let mut buf = vec![0u8; 65536];
    loop {
        let now = Instant::now();
        while let Some(t) = conn.poll_transmit(now, 1, &mut out) {
            let _ = sock.send_to(&out[..t.size], t.destination);
            out.clear();
        }
        if let Some(c) = cap.cert.lock().unwrap().take() {
            return Ok(c);
        }
        while let Some(ev) = conn.poll() {
            if let Event::ConnectionLost { reason } = ev {
                return Err(reason.to_string());
            }
        }
        if now >= until {
            return Err(tr("Timed out"));
        }
        let wait = conn.poll_timeout().map_or(until, |t| t.min(until)).saturating_duration_since(now).max(Duration::from_millis(1));
        let _ = sock.set_read_timeout(Some(wait));
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Some(DatagramEvent::ConnectionEvent(_, ev)) = endpoint.handle(Instant::now(), from, None, None, bytes::BytesMut::from(&buf[..n]), &mut resp) {
                    conn.handle_event(ev);
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => conn.handle_timeout(Instant::now()),
            // (Windows reports an ICMP "port unreachable" this way: keep waiting, like a lost packet)
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(e) => return Err(e.to_string()),
        }
        while let Some(ev) = conn.poll_endpoint_events() {
            if let Some(cev) = endpoint.handle_event(ch, ev) {
                conn.handle_event(cev);
            }
        }
    }
}

/// DNS names of a certificate: subjectAltName DNS entries first, then the common name (Python `names_of`)
pub fn names_of(der: &[u8]) -> Result<Vec<String>, String> {
    use x509_parser::prelude::*;
    let (_, cert) = X509Certificate::from_der(der).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = vec![];
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for g in &san.value.general_names {
            if let GeneralName::DNSName(n) = g {
                names.push(n.to_string());
            }
        }
    }
    for cn in cert.subject().iter_common_name() {
        if let Ok(v) = cn.as_str() {
            if !names.iter().any(|n| n == v) && !crate::net::is_ip_literal(v) && v.contains('.') {
                names.push(v.to_string());
            }
        }
    }
    Ok(names)
}

/// `_sni(ob)`: the SNI to send (never an IP)
fn sni_of(ob: &Value) -> Option<String> {
    let sni = crate::py::strip(ob["sni"].as_str().unwrap_or(""));
    let name = if sni.is_empty() { ob["host"].as_str().unwrap_or("") } else { sni };
    (!crate::net::is_ip_literal(name)).then(|| name.to_string())
}

/// For an outbound: the certificate's names, the best SNI and the fingerprint.
pub fn detect(ob: &Value) -> Result<Detected, String> {
    let host = ob["host"].as_str().unwrap_or("");
    let port = ob["port"].as_i64().unwrap_or(0).clamp(0, 65535) as u16;
    let sni = sni_of(ob);
    let der = if QUIC_TYPES.contains(&ob["type"].as_str().unwrap_or("")) {
        quic_certificate(host, port, sni.as_deref())
    } else {
        tcp_certificate(host, port, sni.as_deref())
    }
    .map_err(|e| tr("Could not read the certificate: {e}").replace("{e}", &e))?;
    let names = names_of(&der).map_err(|e| tr("Could not read the certificate: {e}").replace("{e}", &e))?;
    let sha256 = sha2::Sha256::digest(&der).iter().map(|b| format!("{b:02x}")).collect();
    let sni = names.iter().find(|n| !n.starts_with("*.")).cloned().unwrap_or_default();
    Ok(Detected { names, sni, sha256 })
}

/// the editor's message: (found a name, title, text) (Python `_on_detect_done`)
pub fn message(d: &Detected) -> (bool, String, String) {
    let fp = format!("{}\u{2026}", (0..8).map(|i| d.sha256.get(i * 2..i * 2 + 2).unwrap_or("")).collect::<Vec<_>>().join(":").to_uppercase());
    let fp_line = tr("Fingerprint (SHA-256): {fp}").replace("{fp}", &fp);
    let names_line = tr("Names in the certificate: {names}").replace("{names}", &d.names.join(", "));
    if !d.sni.is_empty() {
        return (true, tr("SNI set to {name}").replace("{name}", &d.sni), format!("{names_line}\n{fp_line}"));
    }
    let first = if d.names.is_empty() { tr("The certificate has no DNS name") } else { names_line };
    (false, tr("No usable name in the certificate"), format!("{first}\n{fp_line}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a TLS server on a local port with a certificate for the given names (rcgen + rustls)
    fn tls_server(names: &[&str]) -> (u16, Vec<u8>) {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
        params.distinguished_name.push(rcgen::DnType::CommonName, "cn.example.org");
        let cert = params.self_signed(&key).unwrap();
        let der = cert.der().to_vec();
        let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()))
            .unwrap();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let cfg = Arc::new(cfg);
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut c = rustls::ServerConnection::new(cfg).unwrap();
                while c.is_handshaking() {
                    if c.complete_io(&mut s).is_err() {
                        break;
                    }
                }
            }
        });
        (port, der)
    }

    #[test]
    fn reads_a_tls_certificate() {
        let (port, der) = tls_server(&["*.example.com", "www.example.com", "10.0.0.1"]);
        let ob = serde_json::json!({"type": "trojan", "host": "127.0.0.1", "port": port, "sni": ""});
        let d = detect(&ob).unwrap();
        assert_eq!(d.names, ["*.example.com", "www.example.com", "cn.example.org"]);
        assert_eq!(d.sni, "www.example.com");
        assert_eq!(d.sha256, sha2::Sha256::digest(&der).iter().map(|b| format!("{b:02x}")).collect::<String>());
        assert!(message(&d).0);
        // nothing listening (port 1 on loopback)
        assert!(detect(&serde_json::json!({"type": "trojan", "host": "127.0.0.1", "port": 1, "sni": ""})).is_err());
    }
}
