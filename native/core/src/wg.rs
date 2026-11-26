//! WireGuard helpers (Python: `wgconf.keypair / public_of / next_peer_address / client_conf`,
//! `platform_utils.local_lan_ip`).

use base64::Engine;
use serde_json::Value;
use std::net::IpAddr;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// (private, public) as base64, like `wg genkey | wg pubkey`
pub fn keypair() -> (String, String) {
    let raw = crate::singbox::process::random_bytes(32);
    let private = B64.encode(&raw);
    let public = public_of(&private);
    (private, public)
}

/// the public key of a base64 private key ("" if it is not a valid key)
pub fn public_of(private_key: &str) -> String {
    let Some(raw) = crate::py::b64decode_strict(crate::py::strip(private_key)) else { return String::new() };
    let Ok(bytes) = <[u8; 32]>::try_from(raw.as_slice()) else { return String::new() };
    let secret = x25519_dalek::StaticSecret::from(bytes);
    B64.encode(x25519_dalek::PublicKey::from(&secret).as_bytes())
}

/// the next free device address (/32 or /128) in the server's network, or None
pub fn next_peer_address(server_address: &str, used: &[String]) -> Option<String> {
    let (ip, prefix) = crate::py::ip_interface(crate::py::strip(server_address))?;
    let taken: Vec<IpAddr> =
        used.iter().filter_map(|x| crate::py::ip_interface(x).map(|(a, _)| a)).chain([ip]).collect();
    match ip {
        IpAddr::V4(v4) => {
            let host_bits = 32 - prefix;
            let net = if prefix == 0 { 0 } else { u32::from(v4) & (u32::MAX << host_bits) };
            let size: u64 = 1u64 << host_bits;
            // hosts(): without the network and broadcast addresses (a /31 or /32 has only its own)
            let (first, last) = if host_bits <= 1 { (0, size - 1) } else { (1, size - 2) };
            (first..=last.min(first + 1_000_000))
                .map(|k| IpAddr::from(std::net::Ipv4Addr::from(net.wrapping_add(k as u32))))
                .find(|a| !taken.contains(a))
                .map(|a| format!("{a}/32"))
        }
        IpAddr::V6(v6) => {
            let host_bits = 128 - prefix;
            let net = if prefix == 0 { 0 } else { u128::from(v6) & (u128::MAX << host_bits) };
            // hosts(): every address but the network (subnet-router anycast) one; a /127 or /128 has all
            let last: u128 = if host_bits >= 128 { u128::MAX } else { (1u128 << host_bits) - 1 };
            let first: u128 = if host_bits <= 1 { 0 } else { 1 };
            (first..=last.min(first + 1_000_000))
                .map(|k| IpAddr::from(std::net::Ipv6Addr::from(net.wrapping_add(k))))
                .find(|a| !taken.contains(a))
                .map(|a| format!("{a}/128"))
        }
    }
}

/// the LAN address of this PC other PCs reach it on (a UDP connect sends no packets)
pub fn local_lan_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("10.255.255.255:1")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

/// the WireGuard config of one device of a ProxyDock WireGuard server
pub fn client_conf(ib: &Value, peer: &Value, endpoint_host: &str) -> String {
    let mtu = ib["wg_mtu"].as_i64().filter(|m| *m != 0).unwrap_or(1408);
    [
        "[Interface]".to_string(),
        format!("PrivateKey = {}", peer["private_key"].as_str().unwrap_or("")),
        format!("Address = {}", peer["address"].as_str().unwrap_or("")),
        "DNS = 1.1.1.1".into(),
        format!("MTU = {mtu}"),
        String::new(),
        "[Peer]".into(),
        format!("PublicKey = {}", public_of(ib["wg_private_key"].as_str().unwrap_or(""))),
        format!("Endpoint = {}", crate::net::host_port(endpoint_host, ib["port"].as_i64().unwrap_or(0))),
        "AllowedIPs = 0.0.0.0/0".into(),
        "PersistentKeepalive = 25".into(),
    ]
    .join("\n")
        // (ends with a newline, as Python's does: the .conf file's last line)
        + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_addresses() {
        // RFC 7748 test vector (Alice)
        let alice = B64.encode(hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a"));
        assert_eq!(
            public_of(&alice),
            B64.encode(hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"))
        );
        let (k, p) = keypair();
        assert!(crate::checks::valid_wg_key(&k) && public_of(&k) == p);
        assert_eq!(next_peer_address("10.66.0.1/24", &["10.66.0.2/32".into()]).as_deref(), Some("10.66.0.3/32"));
        assert_eq!(next_peer_address("10.66.0.1/30", &["10.66.0.2/32".into()]), None);
        assert_eq!(next_peer_address("fd00::1/64", &[]).as_deref(), Some("fd00::2/128"));
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
}

/// Python `wgconf.parse_conf`: a WireGuard .conf (the kind VPN providers hand out) → (outbound fields, notes).
/// Only the first [Peer] is used; DNS is not (ProxyDock is not a system VPN).
pub fn parse_conf(text: &str) -> Result<(serde_json::Map<String, Value>, Vec<String>), String> {
    use crate::i18n::tr;
    use crate::py::strip;
    use serde_json::json;
    use std::collections::HashMap;
    let mut section = String::new();
    let mut iface: HashMap<String, String> = HashMap::new();
    let mut peers: Vec<HashMap<String, String>> = vec![];
    let add = |m: &mut HashMap<String, String>, k: String, v: &str| {
        let joined = match m.get(&k) {
            Some(old) => format!("{old}, {v}"),
            None => v.to_string(),
        };
        m.insert(k, joined);
    };
    for raw in text.lines() {
        let line = strip(raw.split('#').next().unwrap_or(""));
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = strip(&line[1..line.len() - 1]).to_lowercase();
            if section == "peer" {
                peers.push(HashMap::new());
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (strip(k).to_lowercase(), strip(v));
        if section == "interface" {
            add(&mut iface, k, v);
        } else if section == "peer"
            && let Some(p) = peers.last_mut()
        {
            add(p, k, v);
        }
    }
    let usable = iface.contains_key("privatekey")
        && peers.first().is_some_and(|p| p.contains_key("publickey") && p.contains_key("endpoint"));
    if !usable {
        return Err(tr(
            "Not a WireGuard client config (needs [Interface] PrivateKey and a [Peer] with PublicKey and Endpoint)",
        ));
    }
    let peer = &peers[0];
    let ep = peer["endpoint"].as_str();
    let (host, port) = match ep.strip_prefix('[') {
        Some(rest) => rest.split_once("]:").unwrap_or((rest, "")),
        None => ep.rsplit_once(':').unwrap_or(("", ep)),
    };
    let digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    if host.is_empty() || !digits(port) {
        return Err(tr("Cannot read the Endpoint '{value}'").replace("{value}", ep));
    }
    let get = |m: &HashMap<String, String>, k: &str| m.get(k).cloned().unwrap_or_default();
    let num = |t: String, default: i64| if digits(&t) { t.parse::<i64>().unwrap_or(default) } else { default };
    let mut out = serde_json::Map::new();
    out.insert("type".into(), json!("wireguard"));
    out.insert("host".into(), json!(host));
    out.insert("port".into(), json!(port.parse::<i64>().unwrap_or(0)));
    out.insert("wg_private_key".into(), json!(iface["privatekey"]));
    out.insert("wg_address".into(), json!(get(&iface, "address")));
    out.insert("wg_peer_public_key".into(), json!(peer["publickey"]));
    out.insert("wg_preshared_key".into(), json!(get(peer, "presharedkey")));
    out.insert(
        "wg_allowed_ips".into(),
        json!(peer.get("allowedips").cloned().unwrap_or_else(|| "0.0.0.0/0, ::/0".into())),
    );
    out.insert("wg_mtu".into(), json!(num(get(&iface, "mtu"), 1408)));
    out.insert("wg_keepalive".into(), json!(num(get(peer, "persistentkeepalive"), 0)));
    let mut notes = vec![];
    if peers.len() > 1 {
        notes.push(
            tr("The file has {count} peers; only the first is used").replace("{count}", &peers.len().to_string()),
        );
    }
    if !get(&iface, "dns").is_empty() {
        notes.push(tr("DNS from the file is not used (domains are resolved by ProxyDock)"));
    }
    Ok((out, notes))
}

#[cfg(test)]
mod conf_tests {
    use super::parse_conf;

    #[test]
    fn reads_a_provider_conf() {
        let text = "[Interface]\nPrivateKey = aaa=\nAddress = 10.2.0.2/32\nAddress = fd00::2/128\nDNS = 10.2.0.1\nMTU = 1380\n\n[Peer]\nPublicKey = bbb=\nAllowedIPs = 0.0.0.0/0 # all\nEndpoint = [2001:db8::1]:51820\nPersistentKeepalive = 25\n[Peer]\nPublicKey = c\nEndpoint = h:1\n";
        let (ob, notes) = parse_conf(text).unwrap();
        assert_eq!(ob["host"], "2001:db8::1");
        assert_eq!(ob["port"], 51820);
        assert_eq!(ob["wg_address"], "10.2.0.2/32, fd00::2/128");
        assert_eq!(ob["wg_mtu"], 1380);
        assert_eq!(ob["wg_keepalive"], 25);
        assert_eq!(ob["wg_allowed_ips"], "0.0.0.0/0");
        assert_eq!(notes.len(), 2);
        assert!(parse_conf("[Interface]\nPrivateKey = x\n").is_err());
        assert!(parse_conf("[Interface]\nPrivateKey = x\n[Peer]\nPublicKey = y\nEndpoint = host\n").is_err());
    }
}
