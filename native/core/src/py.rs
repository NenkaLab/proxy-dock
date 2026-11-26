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

/// `binascii.a2b_base64` (not strict, Python 3.14): characters outside the alphabet are skipped and so are pads;
/// the data may end in an unfinished quad only when enough pads follow its last character
pub fn a2b_base64(s: &[u8]) -> Option<Vec<u8>> {
    a2b_base64_count(s).ok()
}

/// a2b_base64, or Err(data characters) when it fails
fn a2b_base64_count(s: &[u8]) -> Result<Vec<u8>, (u32, usize)> {
    let mut out = vec![];
    let (mut quad, mut pads, mut left, mut count) = (0u32, 0u32, 0u32, 0usize);
    for &c in s {
        if c == b'=' {
            pads += 1;
            continue;
        }
        let Some(v) = b64_value(c) else { continue };
        pads = 0;
        count += 1;
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
    if quad == 0 || (quad >= 2 && quad + pads >= 4) { Ok(out) } else { Err((quad, count)) }
}

/// `base64.b64decode(s, validate=True)` (Python 3.13+ strict mode): only the alphabet, up to two trailing pads,
/// whole quads (no missing or excess padding)
pub fn b64decode_strict(s: &str) -> Option<Vec<u8>> {
    let body = s.trim_end_matches('=');
    if s.len() - body.len() > 2 || !s.len().is_multiple_of(4) || !body.bytes().all(|c| b64_value(c).is_some()) {
        return None;
    }
    a2b_base64(s.as_bytes())
}

/// `base64.urlsafe_b64decode(s)` (not validated)
pub fn urlsafe_b64decode(s: &str) -> Option<Vec<u8>> {
    let t: Vec<u8> = s
        .bytes()
        .map(|c| match c {
            b'-' => b'+',
            b'_' => b'/',
            c => c,
        })
        .collect();
    a2b_base64(&t)
}

/// `base64.urlsafe_b64decode(s)` with Python's error messages (a str with other than ASCII is refused)
pub fn urlsafe_b64decode_msg(s: &str) -> Result<Vec<u8>, String> {
    if !s.is_ascii() {
        return Err("string argument should contain only ASCII characters".into());
    }
    let t: Vec<u8> = s
        .bytes()
        .map(|c| match c {
            b'-' => b'+',
            b'_' => b'/',
            c => c,
        })
        .collect();
    a2b_base64_count(&t).map_err(|(quad, count)| {
        if quad == 1 {
            format!("Invalid base64-encoded string: number of data characters ({count}) cannot be 1 more than a multiple of 4")
        } else {
            "Incorrect padding".into()
        }
    })
}

/// `bytes.decode("utf-8")` with Python's error message
pub fn utf8(raw: &[u8]) -> Result<String, String> {
    match std::str::from_utf8(raw) {
        Ok(t) => Ok(t.to_string()),
        Err(e) => {
            let start = e.valid_up_to();
            let (end, why) = match e.error_len() {
                None => (raw.len(), "unexpected end of data"),
                Some(n) => {
                    let b = raw[start];
                    let bad_start = (0x80..=0xc1).contains(&b) || b >= 0xf5;
                    (start + n, if bad_start { "invalid start byte" } else { "invalid continuation byte" })
                }
            };
            Err(if end - start == 1 {
                format!("'utf-8' codec can't decode byte 0x{:02x} in position {start}: {why}", raw[start])
            } else {
                format!("'utf-8' codec can't decode bytes in position {start}-{}: {why}", end - 1)
            })
        }
    }
}

/// `repr(str)` (characters other than ASCII controls are kept as they are)
pub fn repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// `urllib.parse.quote(s, safe)`: UTF-8 bytes as %XX except letters, digits, `_.-~` and the `safe` characters
pub fn quote(s: &str, safe: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || "_.-~".contains(ch) || (ch.is_ascii() && safe.contains(ch)) {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for b in ch.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// `urllib.parse.unquote(s)`: %XX in each run of ASCII characters decoded as UTF-8 (bad bytes become U+FFFD)
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut run: Vec<u8> = vec![];
    let flush = |run: &mut Vec<u8>, out: &mut String| {
        out.push_str(&String::from_utf8_lossy(run));
        run.clear();
    };
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c >= 0x80 {
            flush(&mut run, &mut out);
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if c == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            run.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
            continue;
        }
        run.push(c);
        i += 1;
    }
    flush(&mut run, &mut out);
    out
}

/// `urllib.parse.parse_qsl(qs, keep_blank_values=True)`
pub fn parse_qsl(qs: &str) -> Vec<(String, String)> {
    qs.split('&')
        .filter(|nv| !nv.is_empty())
        .map(|nv| {
            let (n, v) = nv.split_once('=').unwrap_or((nv, ""));
            (unquote(&n.replace('+', " ")), unquote(&v.replace('+', " ")))
        })
        .collect()
}

/// `urllib.parse.urlsplit(url)`
#[derive(Debug, Default)]
pub struct Split {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

/// `urllib.parse.urlsplit` (Python 3.14): leading controls and spaces dropped, tabs and line breaks removed, the
/// bracket checks of the address. Not done: the NFKC check of an address with other than ASCII characters.
pub fn urlsplit(url: &str) -> Result<Split, String> {
    let url: String = url
        .trim_start_matches(|c: char| (c as u32) <= 0x20)
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut sp = Split::default();
    let mut rest: &str = &url;
    if let Some(i) = rest.find(':') {
        let first = rest.chars().next().unwrap();
        if i > 0
            && first.is_ascii_alphabetic()
            && rest[..i].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            sp.scheme = rest[..i].to_lowercase();
            rest = &rest[i + 1..];
        }
    }
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        sp.netloc = after[..end].to_string();
        rest = &after[end..];
        let (open, close) = (sp.netloc.contains('['), sp.netloc.contains(']'));
        if open != close {
            return Err("Invalid IPv6 URL".into());
        }
        if open {
            check_bracketed(&sp.netloc)?;
        }
    }
    if let Some((a, f)) = rest.split_once('#') {
        sp.fragment = f.to_string();
        rest = a;
    }
    if let Some((a, q)) = rest.split_once('?') {
        sp.query = q.to_string();
        rest = a;
    }
    sp.path = rest.to_string();
    Ok(sp)
}

fn check_bracketed(netloc: &str) -> Result<(), String> {
    let hp = netloc.rsplit_once('@').map_or(netloc, |(_, b)| b);
    let hostname = match hp.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return Err("Invalid IPv6 URL".into());
            }
            let (h, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !port.is_empty() && !port.starts_with(':') {
                return Err("Invalid IPv6 URL".into());
            }
            h
        }
        None => hp.split_once(':').map_or(hp, |(h, _)| h),
    };
    if let Some(v) = hostname.strip_prefix('v') {
        let (hex, tail) = v.split_once('.').unwrap_or((v, ""));
        if hex.is_empty() || !hex.bytes().all(|c| c.is_ascii_hexdigit()) || tail.is_empty() {
            return Err("IPvFuture address is invalid".into());
        }
        return Ok(());
    }
    match ip_address(hostname) {
        None => Err(format!("{} does not appear to be an IPv4 or IPv6 address", repr(hostname))),
        Some(std::net::IpAddr::V4(_)) => Err("An IPv4 address cannot be in brackets".into()),
        Some(_) => Ok(()),
    }
}

/// `json.loads(text)` (without Python's NaN / Infinity)
pub fn json_loads(text: &str) -> Option<serde_json::Value> {
    serde_json::from_str(text).ok()
}

/// `str.splitlines()`
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' && it.peek().is_some_and(|(_, n)| *n == '\n') {
                it.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_like_python_314() {
        // (input, result) as binascii.a2b_base64 gives them in Python 3.14
        for (t, want) in [
            ("QUI=", Some(&b"AB"[..])),
            ("QUI=R", Some(b"AB")),
            ("QQ==QQ==", Some(b"A")),
            ("QQ=a", None),
            ("QQ=", None),
            ("QUI==QUI=", None),
            ("QUI=REVG===", Some(b"ABQ")),
            ("QUJD=QQ==", Some(b"ABCA")),
            ("QQ=!=", Some(b"A")),
            ("QQ", None),
        ] {
            assert_eq!(a2b_base64(t.as_bytes()).as_deref(), want, "{t}");
        }
    }

    #[test]
    fn python_like() {
        assert_eq!(strip("\u{1c} a \u{3000}"), "a");
        assert!(
            int16_ok("ff00aa") && int16_ok("0xffff") && int16_ok("ff_f00") && int16_ok(" fff00") && int16_ok("+fff00")
        );
        assert!(!int16_ok("ff__00") && !int16_ok("_fff00") && !int16_ok("gg0000") && !int16_ok(""));
    }
}
