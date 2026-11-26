//! The window's colours from one seed colour (Material 3's way: tonal palettes, then a tone per role for light
//! and dark). Tones are CIELAB lightness (L*, as in M3's HCT); hue and chroma are OKLCh's, each colour fitted
//! into sRGB at its tone. The primary palette keeps the seed's hue, secondary is a quiet version of it, tertiary
//! is turned 60 degrees, and the surfaces are near-neutral with a trace of the hue.

use crate::colors::{self, Gamut, Lab};

/// the app's own seed (the baseline the hand-made colours came from)
pub const APP_SEED: &str = "#0061a4";

/// the app's preset seeds
pub const PRESETS: [&str; 8] = ["#0061a4", "#006a6a", "#386a20", "#6750a4", "#984061", "#a63b2a", "#8b5000", "#5d5f71"];

/// the roles the window colours from the seed, and their tone in light / dark
pub const ROLES: [(&str, char, f64, f64); 16] = [
    ("primary", 'p', 40.0, 80.0),
    ("on-primary", 'p', 100.0, 20.0),
    ("primary-container", 'p', 90.0, 30.0),
    ("on-primary-container", 'p', 10.0, 90.0),
    ("secondary-container", 's', 90.0, 30.0),
    ("on-secondary-container", 's', 10.0, 90.0),
    ("tertiary-container", 't', 90.0, 30.0),
    ("on-tertiary-container", 't', 10.0, 90.0),
    ("surface", 'n', 98.0, 6.0),
    ("surface-container-low", 'n', 96.0, 10.0),
    ("surface-container", 'n', 94.0, 12.0),
    ("surface-container-high", 'n', 92.0, 17.0),
    ("surface-container-highest", 'n', 90.0, 22.0),
    ("on-surface", 'n', 10.0, 90.0),
    ("on-surface-variant", 'v', 30.0, 80.0),
    ("outline-variant", 'v', 80.0, 30.0),
];

/// relative luminance of a colour fitted into sRGB
fn luminance(lab: Lab) -> f64 {
    let [r, g, b] = colors::oklab_to_linear_srgb(colors::fit_gamut(lab, Gamut::Srgb)).map(|v| v.clamp(0.0, 1.0));
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// CIELAB L* (0..100) to relative luminance
fn tone_to_y(t: f64) -> f64 {
    let f = (t + 16.0) / 116.0;
    if t > 8.0 { f * f * f } else { t / 903.2963 }
}

/// the colour of hue `h` and chroma `c` (OKLCh) at tone `t`, in sRGB, as "#rrggbb"
pub fn tone(h: f64, c: f64, t: f64) -> String {
    let y = tone_to_y(t.clamp(0.0, 100.0));
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if luminance(colors::from_lch(mid, c, h)) < y { lo = mid } else { hi = mid }
    }
    let lab = colors::fit_gamut(colors::from_lch((lo + hi) / 2.0, c, h), Gamut::Srgb);
    colors::hex_of(colors::oklab_to_linear_srgb(lab).map(colors::srgb_encode))
}

/// (role, light colour, dark colour) for every role, from a seed ("#rrggbb" / "oklch(...)"; anything else = the
/// app's seed)
pub fn scheme(seed: &str) -> Vec<(&'static str, String, String)> {
    let lab = colors::parse_color(seed).or_else(|| colors::parse_color(APP_SEED)).unwrap();
    let (_, sc, h) = colors::to_lch(lab);
    // a grey seed gives grey palettes; otherwise the primary's chroma within what reads as a calm accent
    let grey = sc < 0.02;
    let p = if grey { sc } else { sc.clamp(0.06, 0.15) };
    let palette = |k: char| -> (f64, f64) {
        match k {
            'p' => (h, p),
            's' => (h, p / 3.5),
            't' => (h + 60.0, p * 0.55),
            'n' => (h, if grey { 0.0 } else { 0.008 }),
            _ => (h, if grey { 0.0 } else { 0.016 }),
        }
    };
    ROLES
        .iter()
        .map(|&(role, k, light, dark)| {
            let (hh, c) = palette(k);
            (role, tone(hh, c, light), tone(hh, c, dark))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: &str, b: &str) -> f64 {
        let (x, y) = (colors::parse_color(a).unwrap(), colors::parse_color(b).unwrap());
        ((x.0 - y.0).powi(2) + (x.1 - y.1).powi(2) + (x.2 - y.2).powi(2)).sqrt()
    }

    #[test]
    fn baseline_seed_is_close_to_the_hand_made_colours() {
        let s = scheme(APP_SEED);
        let get = |r: &str| s.iter().find(|x| x.0 == r).unwrap().clone();
        // (M3's baseline for #0061a4)
        for (role, light, dark) in [
            ("primary", "#0061a4", "#9ecaff"),
            ("primary-container", "#d1e4ff", "#00497d"),
            ("surface", "#f8f9ff", "#111418"),
            ("surface-container-highest", "#e0e2e8", "#32353a"),
            ("on-surface", "#191c20", "#e1e2e8"),
        ] {
            let (_, l, d) = get(role);
            assert!(dist(&l, light) < 0.05, "{role} light {l} vs {light}");
            assert!(dist(&d, dark) < 0.05, "{role} dark {d} vs {dark}");
        }
        // a grey seed gives greys; any seed gives colours for every role
        assert!(scheme("#808080").iter().all(|(_, l, _)| {
            let (_, c, _) = colors::to_lch(colors::parse_color(l).unwrap());
            c < 0.01
        }));
        assert_eq!(scheme("oklch(0.8 0.3 145)").len(), ROLES.len());
    }
}
