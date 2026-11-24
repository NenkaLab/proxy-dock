//! Small Python behaviours the port has to match exactly.

/// `str.isspace()` for one character: Unicode White_Space plus the separators U+001C..U+001F
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `repr(float)`: the shortest round-trip digits like Rust, but the exponent as Python writes it (1e+16, 1e-05)
pub fn float_repr(f: f64) -> String {
    let s = format!("{f:?}");
    match s.split_once('e') {
        Some((m, e)) => {
            let (sign, digits) = match e.strip_prefix('-') {
                Some(d) => ('-', d),
                None => ('+', e),
            };
            format!("{m}e{sign}{digits:0>2}")
        }
        None => s,
    }
}

/// `str.strip()`
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s[:n]` (code points)
pub fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `int(s)` of a string: surrounding spaces, a sign, digits with single underscores between them
pub fn int(s: &str) -> Option<i64> {
    let t = strip(s);
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    let ok = !digits.is_empty()
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__")
        && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
    if !ok {
        return None;
    }
    t.replace('_', "").parse().ok()
}

/// Whether `int(s, 16)` accepts `s`: surrounding spaces, a sign, an optional 0x prefix, hex digits with single
/// underscores between them.
pub fn int16_ok(s: &str) -> bool {
    let t = strip(s);
    let t = t.strip_prefix(['+', '-']).unwrap_or(t);
    let (t, prefixed) = match t.get(..2) {
        Some(p) if p.eq_ignore_ascii_case("0x") => (&t[2..], true),
        _ => (t, false),
    };
    // after "0x" one leading underscore is allowed ("0x_ff")
    let t = if prefixed { t.strip_prefix('_').unwrap_or(t) } else { t };
    !t.is_empty()
        && !t.starts_with('_')
        && !t.ends_with('_')
        && !t.contains("__")
        && t.chars().all(|c| c.is_ascii_hexdigit() || c == '_')
}

/// `ipaddress.ip_address(s)`: IPv4 in four decimal parts (no leading zeros), IPv6 with an optional %scope
pub fn ip_address(s: &str) -> Option<std::net::IpAddr> {
    if let Ok(a) = s.parse::<std::net::Ipv4Addr>() {
        return Some(a.into());
    }
    let (addr, scope) = match s.split_once('%') {
        Some((a, sc)) => (a, Some(sc)),
        None => (s, None),
    };
    if scope.is_some_and(|sc| sc.is_empty() || sc.contains('%')) {
        return None;
    }
    addr.parse::<std::net::Ipv6Addr>().ok().map(Into::into)
}

/// `ipaddress.ip_interface(s)` / `ip_network(s, strict=False)`: (address, prefix length). The prefix is decimal
/// digits, or for IPv4 a netmask / hostmask in dotted form; none = the full length.
pub fn ip_interface(s: &str) -> Option<(std::net::IpAddr, u32)> {
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (s, None),
    };
    if prefix.is_some_and(|p| p.contains('/')) {
        return None;
    }
    let a = ip_address(addr)?;
    let max = if a.is_ipv4() { 32 } else { 128 };
    let Some(p) = prefix else { return Some((a, max)) };
    if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) {
        let n: u32 = p.parse().ok()?;
        return (n <= max).then_some((a, n));
    }
    if a.is_ipv4() {
        // a netmask (255.255.255.0) or a hostmask (0.0.0.255)
        let m = u32::from(p.parse::<std::net::Ipv4Addr>().ok()?);
        let len = |m: u32| (m.leading_ones() + m.trailing_zeros() == 32).then_some(m.leading_ones());
        return len(m).or_else(|| len(!m)).map(|n| (a, n));
    }
    None
}

fn b64_value(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

/// `binascii.a2b_base64` (not strict): characters outside the alphabet are skipped, a pad ends the data once a
/// quad is complete, and an unfinished quad is an error
pub fn a2b_base64(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![];
    let (mut quad, mut pads, mut left) = (0u32, 0u32, 0u32);
    for &c in s {
        if c == b'=' {
            pads += 1;
            if quad >= 2 && quad + pads >= 4 {
                return Some(out);
            }
            continue;
        }
        let Some(v) = b64_value(c) else { continue };
        match quad {
            0 => {
                left = v;
                quad = 1;
            }
            1 => {
                out.push(((left << 2) | (v >> 4)) as u8);
                left = v & 0x0f;
                quad = 2;
            }
            2 => {
                out.push(((left << 4) | (v >> 2)) as u8);
                left = v & 0x03;
                quad = 3;
            }
            _ => {
                out.push(((left << 6) | v) as u8);
                left = 0;
                quad = 0;
            }
        }
    }
    (quad == 0).then_some(out)
}

/// `base64.b64decode(s, validate=True)` (Python 3.13+ strict mode): only the alphabet, up to two trailing pads,
/// whole quads (no missing or excess padding)
pub fn b64decode_strict(s: &str) -> Option<Vec<u8>> {
    let body = s.trim_end_matches('=');
    if s.len() - body.len() > 2 || s.len() % 4 != 0 || !body.bytes().all(|c| b64_value(c).is_some()) {
        return None;
    }
    a2b_base64(s.as_bytes())
}

/// `base64.urlsafe_b64decode(s)` (not validated)
pub fn urlsafe_b64decode(s: &str) -> Option<Vec<u8>> {
    let t: Vec<u8> = s.bytes().map(|c| match c {
        b'-' => b'+',
        b'_' => b'/',
        c => c,
    }).collect();
    a2b_base64(&t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_like() {
        assert_eq!(strip("\u{1c} a \u{3000}"), "a");
        assert!(int16_ok("ff00aa") && int16_ok("0xffff") && int16_ok("ff_f00") && int16_ok(" fff00") && int16_ok("+fff00"));
        assert!(!int16_ok("ff__00") && !int16_ok("_fff00") && !int16_ok("gg0000") && !int16_ok(""));
    }
}
