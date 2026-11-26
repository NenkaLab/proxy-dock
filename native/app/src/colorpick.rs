//! The colour picker's model: one colour (OKLab) seen through RGB, HSL or OKLCH. The window draws what `view`
//! gives (a hue ring, a square for the other two values of the chosen space, three sliders, the colour as text)
//! and hands back drags and typed text. Colours go up to Display P3; one beyond sRGB is stored as
//! "oklch(L C H)" (`colors::store_color`).

use crate::gamut::Monitor;
use proxydock_core::colors::{self, Gamut, Lab};

/// the colour spaces the picker offers
pub const SPACES: [&str; 3] = ["RGB", "HSL", "OKLCH"];
/// OKLCH chroma at the right edge of the square / slider (P3's most vivid colours are below it)
pub const C_MAX: f64 = 0.37;

/// an RGBA picture (width, height, bytes)
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Rgba {
    fn new(w: u32, h: u32) -> Self {
        Rgba { w, h, px: vec![0; (w * h * 4) as usize] }
    }
    fn set(&mut self, x: u32, y: u32, c: [u8; 4]) {
        let i = ((y * self.w + x) * 4) as usize;
        self.px[i..i + 4].copy_from_slice(&c);
    }
}

fn enc(lab: Lab) -> [f64; 3] {
    colors::oklab_to_linear_srgb(lab).map(colors::srgb_encode)
}

fn from_enc(rgb: [f64; 3]) -> Lab {
    colors::linear_srgb_to_oklab(rgb.map(colors::srgb_decode))
}

fn px(rgb: [f64; 3]) -> [u8; 4] {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [c(rgb[0]), c(rgb[1]), c(rgb[2]), 255]
}

/// encoded sRGB of a colour, fitted into sRGB
fn srgb_of(lab: Lab) -> [f64; 3] {
    enc(colors::fit_gamut(lab, Gamut::Srgb)).map(|v| v.clamp(0.0, 1.0))
}

/// HSL / HSV (h in degrees, the rest 0..1) from encoded sRGB; h is None for a grey
pub fn rgb_to_hsl([r, g, b]: [f64; 3]) -> (Option<f64>, f64, f64) {
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    if d < 1e-9 {
        return (None, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-9);
    (Some(hue_of(r, g, b, mx, d)), s.min(1.0), l)
}

pub fn rgb_to_hsv([r, g, b]: [f64; 3]) -> (Option<f64>, f64, f64) {
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let d = mx - mn;
    if d < 1e-9 {
        return (None, 0.0, mx);
    }
    (Some(hue_of(r, g, b, mx, d)), if mx > 0.0 { d / mx } else { 0.0 }, mx)
}

fn hue_of(r: f64, g: f64, b: f64, mx: f64, d: f64) -> f64 {
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
}

pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [f64; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    hc_to_rgb(h, c, l - c / 2.0)
}

pub fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    let c = v * s;
    hc_to_rgb(h, c, v - c)
}

fn hc_to_rgb(h: f64, c: f64, m: f64) -> [f64; 3] {
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// the numbers in a typed text ("12, 50%, 3.5" -> [12, 50, 3.5])
fn numbers(s: &str) -> Vec<f64> {
    s.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|x| !x.is_empty() && *x != "-" && *x != ".")
        .filter_map(|x| x.parse().ok())
        .collect()
}

/// everything the window shows for the picker
pub struct View {
    pub hue: f64,
    pub square_x: f64,
    pub square_y: f64,
    pub sliders: [f64; 3],
    pub slider_labels: [String; 3],
    pub slider_values: [String; 3],
    pub slider_images: [Rgba; 3],
    /// HEX, RGB, HSL, OKLCH, Display P3
    pub fields: [String; 5],
    pub new_hex: String,
    pub old_hex: String,
    /// beyond sRGB (stored as oklch())
    pub wide: bool,
    /// the monitor cannot show it
    pub unseen: bool,
}

pub struct Picker {
    pub lab: Lab,
    pub old: Lab,
    /// the hue the ring and square use (kept while the colour is grey)
    pub hue: f64,
    pub space: usize,
    monitor: Option<Monitor>,
}

impl Picker {
    pub fn new(initial: &str, monitor: Option<Monitor>) -> Self {
        let lab = colors::parse_color(initial).unwrap_or_else(|| colors::parse_color("#5aa0f0").unwrap());
        let mut p = Picker { lab, old: lab, hue: 0.0, space: 0, monitor };
        p.hue = p.hue_in_space(0.0);
        p
    }

    /// the colour as it is stored ("#rrggbb" or "oklch(...)")
    pub fn stored(&self) -> String {
        colors::store_color(self.lab)
    }

    /// the monitor's name for the window ("" = no profile: treated as sRGB)
    pub fn monitor_name(&self) -> String {
        self.monitor.as_ref().map(|m| m.name.clone()).unwrap_or_default()
    }

    fn hue_in_space(&self, keep: f64) -> f64 {
        if self.space == 2 {
            let (_, c, h) = colors::to_lch(self.lab);
            if c > 1e-4 { h } else { keep }
        } else {
            rgb_to_hsl(srgb_of(self.lab)).0.unwrap_or(keep)
        }
    }

    fn set(&mut self, lab: Lab) {
        self.lab = colors::fit_gamut(lab, Gamut::P3);
    }

    pub fn set_space(&mut self, space: usize) {
        self.space = space.min(2);
        self.hue = self.hue_in_space(self.hue);
    }

    /// the colour at (fx, fy) in the square (0..1 each, y down) for the current hue
    fn square_color(&self, fx: f64, fy: f64) -> Lab {
        let (fx, fy) = (fx.clamp(0.0, 1.0), fy.clamp(0.0, 1.0));
        match self.space {
            0 => from_enc(hsv_to_rgb(self.hue, fx, 1.0 - fy)),
            1 => from_enc(hsl_to_rgb(self.hue, fx, 1.0 - fy)),
            _ => colors::from_lch(1.0 - fy, fx * C_MAX, self.hue),
        }
    }

    fn square_pos(&self) -> (f64, f64) {
        match self.space {
            0 => {
                let (_, s, v) = rgb_to_hsv(srgb_of(self.lab));
                (s, 1.0 - v)
            }
            1 => {
                let (_, s, l) = rgb_to_hsl(srgb_of(self.lab));
                (s, 1.0 - l)
            }
            _ => {
                let (l, c, _) = colors::to_lch(self.lab);
                ((c / C_MAX).min(1.0), 1.0 - l)
            }
        }
    }

    /// the ring dragged to `deg` (0 = right, clockwise)
    pub fn ring(&mut self, deg: f64) {
        let (fx, fy) = self.square_pos();
        self.hue = deg.rem_euclid(360.0);
        self.set(self.square_color(fx, fy));
    }

    pub fn square(&mut self, fx: f64, fy: f64) {
        self.set(self.square_color(fx, fy));
    }

    /// slider values 0..1 of the current space
    fn slider_values(&self) -> [f64; 3] {
        match self.space {
            0 => srgb_of(self.lab),
            1 => {
                let (_, s, l) = rgb_to_hsl(srgb_of(self.lab));
                [self.hue / 360.0, s, l]
            }
            _ => {
                let (l, c, _) = colors::to_lch(self.lab);
                [l, (c / C_MAX).min(1.0), self.hue / 360.0]
            }
        }
    }

    /// the colour with slider `i` at `f` (0..1)
    fn slider_color(&self, i: usize, f: f64) -> (Lab, f64) {
        let mut v = self.slider_values();
        v[i] = f.clamp(0.0, 1.0);
        match self.space {
            0 => (from_enc(v), self.hue),
            1 => (from_enc(hsl_to_rgb(v[0] * 360.0, v[1], v[2])), v[0] * 360.0),
            _ => (colors::from_lch(v[0], v[1] * C_MAX, v[2] * 360.0), v[2] * 360.0),
        }
    }

    pub fn slider(&mut self, i: usize, f: f64) {
        let (lab, hue) = self.slider_color(i.min(2), f);
        self.set(lab);
        self.hue = hue;
        if self.space == 0 {
            self.hue = self.hue_in_space(self.hue);
        }
    }

    /// text typed into field `i` (HEX, RGB, HSL, OKLCH, Display P3); false if it is not a colour
    pub fn text(&mut self, i: usize, s: &str) -> bool {
        let n = numbers(s);
        let lab = match i {
            0 => {
                let h = s.trim().trim_start_matches('#');
                let h = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect() } else { h.to_string() };
                match colors::parse_color(&format!("#{h}")) {
                    Some(l) => l,
                    None => return false,
                }
            }
            1 if n.len() == 3 => from_enc([n[0] / 255.0, n[1] / 255.0, n[2] / 255.0].map(|v| v.clamp(0.0, 1.0))),
            2 if n.len() == 3 => {
                from_enc(hsl_to_rgb(n[0], (n[1] / 100.0).clamp(0.0, 1.0), (n[2] / 100.0).clamp(0.0, 1.0)))
            }
            3 if n.len() == 3 => {
                // (L as 0..1 or as a percentage)
                let l = if n[0] > 1.0 { n[0] / 100.0 } else { n[0] };
                colors::from_lch(l.clamp(0.0, 1.0), n[1].clamp(0.0, 0.5), n[2])
            }
            4 if n.len() == 3 => {
                let lin: Vec<f64> = n.iter().map(|v| colors::srgb_decode(v.clamp(0.0, 1.0))).collect();
                colors::linear_srgb_to_oklab(colors::linear_p3_to_srgb([lin[0], lin[1], lin[2]]))
            }
            _ => return false,
        };
        self.set(lab);
        self.hue = self.hue_in_space(self.hue);
        true
    }

    /// the hue ring: `size` square pixels, the ring `thick` wide at the edge (transparent elsewhere)
    pub fn ring_image(&self, size: u32, thick: f64) -> Rgba {
        // a colour per half degree
        let table: Vec<[u8; 4]> = (0..720)
            .map(|k| {
                let h = k as f64 / 2.0;
                if self.space == 2 {
                    px(enc(colors::fit_gamut(colors::from_lch(0.72, 0.16, h), Gamut::Srgb)))
                } else {
                    px(hsl_to_rgb(h, 1.0, 0.5))
                }
            })
            .collect();
        let mut img = Rgba::new(size, size);
        let c = size as f64 / 2.0;
        for y in 0..size {
            for x in 0..size {
                let (dx, dy) = (x as f64 + 0.5 - c, y as f64 + 0.5 - c);
                let r = dx.hypot(dy);
                // (soft edges: a pixel of coverage on each side)
                let cover = (c - r).clamp(0.0, 1.0) * (r - (c - thick)).clamp(0.0, 1.0);
                if cover <= 0.0 {
                    continue;
                }
                let h = dy.atan2(dx).to_degrees().rem_euclid(360.0);
                let mut p = table[((h * 2.0) as usize).min(719)];
                p[3] = (cover * 255.0) as u8;
                img.set(x, y, p);
            }
        }
        img
    }

    /// the square for the current hue; in OKLCH, P3-only colours are striped and colours beyond P3 left out
    pub fn square_image(&self, size: u32) -> Rgba {
        let mut img = Rgba::new(size, size);
        let n = size as f64 - 1.0;
        let mut in_srgb = vec![false; (size * size) as usize];
        for y in 0..size {
            for x in 0..size {
                let lab = self.square_color(x as f64 / n, y as f64 / n);
                if self.space != 2 {
                    img.set(x, y, px(enc(lab)));
                    continue;
                }
                if !colors::in_gamut(lab, Gamut::P3) {
                    continue;
                }
                let s = colors::in_gamut(lab, Gamut::Srgb);
                in_srgb[(y * size + x) as usize] = s;
                let mut p = px(enc(colors::fit_gamut(lab, Gamut::Srgb)));
                if !s && ((x + y) / 6) % 2 == 0 {
                    p = p.map(|v| v.saturating_add(28));
                    p[3] = 255;
                }
                img.set(x, y, p);
            }
        }
        if self.space == 2 {
            // sRGB's edge as a thin light line
            for y in 1..size {
                for x in 1..size {
                    let i = (y * size + x) as usize;
                    let edge = in_srgb[i] != in_srgb[i - 1] || in_srgb[i] != in_srgb[i - size as usize];
                    if edge && img.px[i * 4 + 3] > 0 {
                        img.set(x, y, [255, 255, 255, 200]);
                    }
                }
            }
        }
        img
    }

    /// slider `i`'s track: the colour as that slider moves
    fn slider_image(&self, i: usize) -> Rgba {
        let w = 256;
        let mut img = Rgba::new(w, 1);
        for x in 0..w {
            let (lab, _) = self.slider_color(i, x as f64 / (w - 1) as f64);
            let mut p = px(enc(colors::fit_gamut(lab, Gamut::Srgb)));
            if self.space == 2 && !colors::in_gamut(lab, Gamut::P3) {
                p[3] = 60;
            }
            img.set(x, 0, p);
        }
        img
    }

    pub fn view(&self) -> View {
        let s = srgb_of(self.lab);
        let hex = colors::hex_of(s);
        let (h, sl, l) = rgb_to_hsl(s);
        let (ol, oc, oh) = colors::to_lch(self.lab);
        let p3 = colors::linear_srgb_to_p3(colors::oklab_to_linear_srgb(self.lab)).map(colors::srgb_encode);
        let v = self.slider_values();
        let (labels, values): ([&str; 3], [String; 3]) = match self.space {
            0 => (["R", "G", "B"], v.map(|x| format!("{}", (x * 255.0).round()))),
            1 => (
                ["H", "S", "L"],
                [format!("{:.0}°", v[0] * 360.0), format!("{:.0}%", v[1] * 100.0), format!("{:.0}%", v[2] * 100.0)],
            ),
            _ => (
                ["L", "C", "H"],
                [format!("{:.3}", v[0]), format!("{:.3}", v[1] * C_MAX), format!("{:.1}°", v[2] * 360.0)],
            ),
        };
        let wide = !colors::in_gamut(self.lab, Gamut::Srgb);
        let unseen = match &self.monitor {
            Some(m) => !m.shows(self.lab),
            None => wide,
        };
        let (sx, sy) = self.square_pos();
        View {
            hue: self.hue,
            square_x: sx,
            square_y: sy,
            sliders: v,
            slider_labels: labels.map(String::from),
            slider_values: values,
            slider_images: [0, 1, 2].map(|i| self.slider_image(i)),
            fields: [
                hex.clone(),
                format!("{}, {}, {}", (s[0] * 255.0).round(), (s[1] * 255.0).round(), (s[2] * 255.0).round()),
                format!("{:.0}, {:.0}%, {:.0}%", h.unwrap_or(self.hue), sl * 100.0, l * 100.0),
                format!("{ol:.3} {oc:.3} {oh:.1}"),
                format!("{:.3} {:.3} {:.3}", p3[0], p3[1], p3[2]),
            ],
            new_hex: hex,
            old_hex: colors::hex_of(srgb_of(self.old)),
            wide,
            unseen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_round_trip() {
        let mut p = Picker::new("#4f8ef7", None);
        assert_eq!(p.stored(), "#4f8ef7");
        for space in 0..3 {
            p.set_space(space);
            let (fx, fy) = p.square_pos();
            let before = p.stored();
            p.square(fx, fy);
            let after = p.stored();
            // (the square's own rounding: within a step of the same colour)
            let (a, b) = (colors::parse_color(&before).unwrap(), colors::parse_color(&after).unwrap());
            assert!((a.0 - b.0).abs() + (a.1 - b.1).abs() + (a.2 - b.2).abs() < 0.01, "{space}: {before} {after}");
        }
        assert!(p.text(0, "#f00"));
        assert_eq!(p.stored(), "#ff0000");
        assert!(p.text(1, "0, 255, 0"));
        assert_eq!(p.stored(), "#00ff00");
        assert!(p.text(2, "240, 100%, 50%"));
        assert_eq!(p.stored(), "#0000ff");
        assert!(!p.text(1, "nope"));
        // Display P3's green: beyond sRGB, stored as oklch(); this monitor (no profile) counts as sRGB
        assert!(p.text(4, "0 1 0"));
        assert!(p.stored().starts_with("oklch("));
        let v = p.view();
        assert!(v.wide && v.unseen);
        // OKLCH: the chroma slider at its end stays within P3
        p.set_space(2);
        p.slider(1, 1.0);
        assert!(colors::in_gamut(p.lab, Gamut::P3));
    }
}
