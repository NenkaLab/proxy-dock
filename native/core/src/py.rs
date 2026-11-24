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
