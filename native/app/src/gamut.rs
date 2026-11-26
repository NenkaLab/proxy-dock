//! The colours a monitor can show, from its colour profile (ICC): the profile's red, green and blue primaries
//! (rXYZ / gXYZ / bXYZ, relative to D50) make a matrix from the monitor's linear RGB to XYZ. A colour the
//! monitor shows has all three channels within 0..1 through its inverse. Without a profile (or one made of
//! look-up tables only) the monitor counts as sRGB.

use proxydock_core::colors::{self, Lab};

/// A monitor's gamut: its profile's name and the matrix from XYZ (D50) to its linear RGB
#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    to_rgb: [[f64; 3]; 3],
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// (offset, size) of tag `sig` in an ICC profile
fn tag(d: &[u8], sig: &[u8; 4]) -> Option<(usize, usize)> {
    let count = be32(d, 128)? as usize;
    (0..count.min(200)).find_map(|i| {
        let at = 132 + i * 12;
        (d.get(at..at + 4)? == sig).then(|| Some((be32(d, at + 4)? as usize, be32(d, at + 8)? as usize)))?
    })
}

fn xyz_tag(d: &[u8], sig: &[u8; 4]) -> Option<[f64; 3]> {
    let (at, size) = tag(d, sig)?;
    if size < 20 || d.get(at..at + 4)? != b"XYZ " {
        return None;
    }
    let f = |k: usize| be32(d, at + 8 + 4 * k).map(|v| v as i32 as f64 / 65536.0);
    Some([f(0)?, f(1)?, f(2)?])
}

/// the profile's description ("desc": v2 ASCII or v4 'mluc' UTF-16)
fn description(d: &[u8]) -> Option<String> {
    let (at, _) = tag(d, b"desc")?;
    match d.get(at..at + 4)? {
        b"desc" => {
            let n = be32(d, at + 8)? as usize;
            let s = d.get(at + 12..at + 12 + n)?;
            Some(String::from_utf8_lossy(s).trim_end_matches('\0').trim().to_string())
        }
        b"mluc" => {
            let (len, off) = (be32(d, at + 20)? as usize, be32(d, at + 24)? as usize);
            let s = d.get(at + off..at + off + len)?;
            let u: Vec<u16> = s.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
            Some(String::from_utf16_lossy(&u).trim_end_matches('\0').trim().to_string())
        }
        _ => None,
    }
}

fn invert(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-9 {
        return None;
    }
    let c = |a: usize, b: usize, c: usize, d: usize| (m[a][b] * m[c][d] - m[a][d] * m[c][b]) / det;
    Some([
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ])
}

fn mul(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// a monitor from the bytes of its ICC profile
pub fn from_profile(d: &[u8], fallback_name: &str) -> Option<Monitor> {
    if d.get(36..40)? != b"acsp" {
        return None;
    }
    let (r, g, b) = (xyz_tag(d, b"rXYZ")?, xyz_tag(d, b"gXYZ")?, xyz_tag(d, b"bXYZ")?);
    let to_xyz = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let name = description(d).filter(|s| !s.is_empty()).unwrap_or_else(|| fallback_name.to_string());
    Some(Monitor { name, to_rgb: invert(to_xyz)? })
}

// linear sRGB to XYZ (D65), then Bradford D65 -> D50 (what ICC primaries are relative to)
const SRGB_TO_XYZ: [[f64; 3]; 3] =
    [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];
const D65_TO_D50: [[f64; 3]; 3] =
    [[1.0478112, 0.0228866, -0.0501270], [0.0295424, 0.9904844, -0.0170491], [-0.0092345, 0.0150436, 0.7521316]];

impl Monitor {
    /// true if this monitor can show the colour (a little slack for the profile's rounding)
    pub fn shows(&self, lab: Lab) -> bool {
        let xyz = mul(&D65_TO_D50, mul(&SRGB_TO_XYZ, colors::oklab_to_linear_srgb(lab)));
        mul(&self.to_rgb, xyz).iter().all(|v| (-0.004..=1.004).contains(v))
    }

    /// true if the monitor shows clearly more than sRGB (P3's green, beyond sRGB, is inside)
    pub fn wide(&self) -> bool {
        self.shows(colors::linear_srgb_to_oklab(colors::linear_p3_to_srgb([0.0, 1.0, 0.0])))
    }
}

/// The monitor the window is on (None: no profile with primaries; treat it as sRGB)
#[cfg(windows)]
pub fn monitor_of(hwnd: isize) -> Option<Monitor> {
    use windows_sys::Win32::Graphics::Gdi::{GetDC, ReleaseDC};
    use windows_sys::Win32::UI::ColorSystem::GetICMProfileW;
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    let path = unsafe {
        let dc = GetDC(hwnd);
        if dc.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = GetICMProfileW(dc, &mut len, buf.as_mut_ptr());
        ReleaseDC(hwnd, dc);
        if ok == 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    };
    let data = std::fs::read(&path).ok()?;
    let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    from_profile(&data, &stem)
}

#[cfg(not(windows))]
pub fn monitor_of(_hwnd: isize) -> Option<Monitor> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a minimal ICC profile with the given primaries (and a v2 description)
    fn profile(r: [f64; 3], g: [f64; 3], b: [f64; 3], desc: &str) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[36..40].copy_from_slice(b"acsp");
        let mut tags: Vec<([u8; 4], Vec<u8>)> = vec![];
        for (sig, v) in [(b"rXYZ", r), (b"gXYZ", g), (b"bXYZ", b)] {
            let mut t = b"XYZ \0\0\0\0".to_vec();
            for x in v {
                t.extend(((x * 65536.0).round() as i32).to_be_bytes());
            }
            tags.push((*sig, t));
        }
        let mut t = b"desc\0\0\0\0".to_vec();
        t.extend(((desc.len() + 1) as u32).to_be_bytes());
        t.extend(desc.as_bytes());
        t.push(0);
        tags.push((*b"desc", t));
        d.extend((tags.len() as u32).to_be_bytes());
        let mut at = 132 + tags.len() * 12;
        let mut body: Vec<u8> = vec![];
        for (sig, t) in &tags {
            d.extend(sig);
            d.extend((at as u32).to_be_bytes());
            d.extend((t.len() as u32).to_be_bytes());
            at += t.len();
            body.extend(t);
        }
        d.extend(body);
        d
    }

    #[test]
    fn srgb_and_p3_profiles() {
        // sRGB's primaries relative to D50 (as in sRGB IEC61966-2.1 profiles)
        let srgb = profile([0.4361, 0.2225, 0.0139], [0.3851, 0.7169, 0.0971], [0.1431, 0.0606, 0.7141], "sRGB");
        let m = from_profile(&srgb, "x").unwrap();
        assert_eq!(m.name, "sRGB");
        assert!(m.shows(colors::parse_color("#00ff00").unwrap()));
        assert!(m.shows(colors::parse_color("#ff0000").unwrap()));
        assert!(!m.wide());
        // Display P3's primaries relative to D50
        let p3 = profile([0.5151, 0.2412, -0.0011], [0.2920, 0.6922, 0.0419], [0.1571, 0.0666, 0.7841], "Display P3");
        let m = from_profile(&p3, "x").unwrap();
        assert!(m.wide());
        assert!(m.shows(colors::parse_color("#00ff00").unwrap()));
        assert!(from_profile(b"not a profile", "x").is_none());
    }
}
