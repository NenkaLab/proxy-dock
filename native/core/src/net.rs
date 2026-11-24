//! Address helpers (Python: `common.split_host_port`, `is_ip_literal`, `valid_domain`, `strip_scope`).

use std::net::IpAddr;

pub fn strip_scope(addr: &str) -> &str {
    addr.split('%').next().unwrap_or(addr)
}

pub fn is_ip_literal(host: &str) -> bool {
    strip_scope(host).parse::<IpAddr>().is_ok()
}

pub fn valid_domain(addr: &str) -> bool {
    !addr.is_empty() && !addr.chars().any(|c| (c as u32) < 33 || c as u32 == 0x7f) && addr.chars().count() <= 255
}

#[derive(Debug, PartialEq)]
pub struct BadAddress;

/// 'host:port' / '[v6]:port' -> (host, port); without a port, `default_port` (error if None).
pub fn split_host_port(s: &str, default_port: Option<u16>) -> Result<(String, u16), BadAddress> {
    let s = s.trim();
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (host, rest) = rest.split_once(']').unwrap_or((rest, ""));
        (host, rest.strip_prefix(':').unwrap_or(""))
    } else if s.matches(':').count() == 1 {
        s.rsplit_once(':').unwrap()
    } else {
        (s, "")
    };
    if port.is_empty() {
        return default_port.map(|p| (host.to_string(), p)).ok_or(BadAddress);
    }
    // Python int(): surrounding spaces allowed, sign allowed (then rejected by the range check)
    let p: i64 = port.trim().parse().map_err(|_| BadAddress)?;
    if !(1..65536).contains(&p) {
        return Err(BadAddress);
    }
    Ok((host.to_string(), p as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_forms() {
        assert_eq!(split_host_port("1.1.1.1:5353", None), Ok(("1.1.1.1".into(), 5353)));
        assert_eq!(split_host_port("[::1]:53", None), Ok(("::1".into(), 53)));
        assert_eq!(split_host_port("[::1]", Some(53)), Ok(("::1".into(), 53)));
        assert_eq!(split_host_port("::1", Some(53)), Ok(("::1".into(), 53)));
        assert_eq!(split_host_port("dns.google", Some(853)), Ok(("dns.google".into(), 853)));
        assert_eq!(split_host_port("a:0", None), Err(BadAddress));
        assert_eq!(split_host_port("a:x", None), Err(BadAddress));
        assert_eq!(split_host_port("a", None), Err(BadAddress));
    }
}
