//! Self-signed server certificates for HTTPS and Hysteria2 inbounds (Python: `tls.ensure_self_signed` and
//! `ProxyInstance.server_cert_paths`).

use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Create `<directory>/<name>.crt` and `.key` if one of them is missing: ECDSA P-256, valid from yesterday for ten
/// years, for localhost / 127.0.0.1 / ::1, not a CA. Returns (certificate, key) paths.
pub fn ensure_self_signed(directory: &Path, name: &str) -> std::io::Result<(PathBuf, PathBuf)> {
    let cert_path = directory.join(format!("{name}.crt"));
    let key_path = directory.join(format!("{name}.key"));
    if cert_path.exists() && key_path.exists() {
        return Ok((cert_path, key_path));
    }
    std::fs::create_dir_all(directory)?;
    let (cert_pem, key_pem) = generate(name).map_err(std::io::Error::other)?;
    write_private(&key_path, key_pem.as_bytes())?;
    std::fs::write(&cert_path, cert_pem)?;
    Ok((cert_path, key_path))
}

fn generate(name: &str) -> Result<(String, String), rcgen::Error> {
    use rcgen::{CertificateParams, DnType, IsCa, KeyPair, PKCS_ECDSA_P256_SHA256};
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    // IP strings become IP address names, the rest DNS names
    let mut params = CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".into(), "::1".into()])?;
    params.distinguished_name.push(DnType::CommonName, format!("{} {name}", crate::paths::APP_ID));
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(3650);
    params.is_ca = IsCa::ExplicitNoCa;
    // no serial set: rcgen derives one from the public key hash, unique per generated key
    let cert = params.self_signed(&key)?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// a file only this user can read (Unix 0600; on Windows the profile folder already is private)
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(data)
}

/// The certificate a card's HTTPS / Hysteria2 inbound serves: the configured tls_cert / tls_key, else a
/// self-signed pair `proxy-<card id>` in the certificate folder (made on first use).
pub fn server_cert_paths(proxy: &Value, cert_dir: &Path) -> std::io::Result<(String, String)> {
    let empty = Map::new();
    let ib = proxy["inbound"].as_object().unwrap_or(&empty);
    let s = |k: &str| ib.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let (cert, key) = (s("tls_cert"), s("tls_key"));
    if !cert.is_empty() && !key.is_empty() {
        return Ok((cert, key));
    }
    let id = proxy["id"].as_str().unwrap_or("");
    let (c, k) = ensure_self_signed(cert_dir, &format!("proxy-{id}"))?;
    Ok((c.display().to_string(), k.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_once_and_keeps() {
        let dir = std::env::temp_dir().join(format!("proxydock-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (c, k) = ensure_self_signed(&dir, "proxy-x").unwrap();
        let pem = std::fs::read_to_string(&c).unwrap();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(std::fs::read_to_string(&k).unwrap().starts_with("-----BEGIN PRIVATE KEY-----"));
        ensure_self_signed(&dir, "proxy-x").unwrap();
        assert_eq!(std::fs::read_to_string(&c).unwrap(), pem, "an existing pair is kept");
        let p = serde_json::json!({"id": "abc", "inbound": {"tls_cert": "", "tls_key": ""}});
        assert!(server_cert_paths(&p, &dir).unwrap().0.ends_with("proxy-abc.crt"));
        let p = serde_json::json!({"id": "abc", "inbound": {"tls_cert": "a.crt", "tls_key": "a.key"}});
        assert_eq!(server_cert_paths(&p, &dir).unwrap(), ("a.crt".into(), "a.key".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
