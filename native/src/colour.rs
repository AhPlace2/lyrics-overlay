// Colour helpers matching what the CSS did: hex, hsl(), hsv for the picker, and color-mix in srgb and oklab.

use egui::Color32;

pub fn hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    let n = u32::from_str_radix(s, 16).ok()?;
    match s.len() {
        6 => Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)),
        3 => {
            let d = |v: u32| (v * 17) as u8;
            Some(Color32::from_rgb(d((n >> 8) & 15), d((n >> 4) & 15), d(n & 15)))
        }
        _ => None,
    }
}

pub fn to_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// hsl(h s% l%) with s and l as 0..1.
pub fn hsl(h: f32, s: f32, l: f32) -> Color32 {
    let h = h.rem_euclid(360.0);
    let a = s * l.min(1.0 - l);
    let f = |n: f32| {
        let k = (n + h / 30.0) % 12.0;
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    Color32::from_rgb((f(0.0) * 255.0).round() as u8, (f(8.0) * 255.0).round() as u8, (f(4.0) * 255.0).round() as u8)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

pub fn hex_to_hsv(c: Color32) -> Hsv {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let mut h = 0.0;
    if d != 0.0 {
        h = if max == r { ((g - b) / d) % 6.0 } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
    }
    Hsv { h: (h * 60.0 + 360.0) % 360.0, s: if max > 0.0 { d / max } else { 0.0 }, v: max }
}

pub fn hsv_to_rgb(Hsv { h, s, v }: Hsv) -> Color32 {
    let f = |n: f32| {
        let k = (n + h / 60.0) % 6.0;
        ((v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)) * 255.0).round() as u8
    };
    Color32::from_rgb(f(5.0), f(3.0), f(1.0))
}

/// color-mix(in srgb, a p, b): p of a, the rest b (alpha too).
pub fn mix(a: Color32, p: f32, b: Color32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 * p + y as f32 * (1.0 - p)).round() as u8;
    Color32::from_rgba_unmultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

fn lin(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}
fn unlin(c: f32) -> u8 {
    let c = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055 };
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}
fn to_oklab(c: Color32) -> [f32; 3] {
    let (r, g, b) = (lin(c.r()), lin(c.g()), lin(c.b()));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}
fn from_oklab([l, a, b]: [f32; 3]) -> Color32 {
    let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s_ = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    Color32::from_rgb(
        unlin(4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_),
        unlin(-1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_),
        unlin(-0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_),
    )
}

/// color-mix(in oklab, a p, b).
pub fn mix_oklab(a: Color32, p: f32, b: Color32) -> Color32 {
    let (x, y) = (to_oklab(a), to_oklab(b));
    from_oklab([x[0] * p + y[0] * (1.0 - p), x[1] * p + y[1] * (1.0 - p), x[2] * p + y[2] * (1.0 - p)])
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (c.a() as f32 * a.clamp(0.0, 1.0)).round() as u8)
}

pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}
