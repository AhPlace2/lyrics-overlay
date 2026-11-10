// Album art for songs whose player shares no cover with Windows, looked up on the iTunes Search API, and the
// cover's main colour for the "From the cover" colour. Port of src/main/cover.js and colourOfCover in main.js.

use crate::lyrics::{candidates, Want, HTTP};
use base64::Engine;
use serde_json::Value;
use std::collections::HashSet;

fn norm(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::new();
    let mut gap = false;
    for ch in lower.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(ch);
        } else {
            gap = true;
        }
    }
    out
}

fn overlap(a: &str, b: &str) -> f64 {
    let x: HashSet<String> = norm(a).split(' ').filter(|w| !w.is_empty()).map(String::from).collect();
    let y: HashSet<String> = norm(b).split(' ').filter(|w| !w.is_empty()).map(String::from).collect();
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    let shared = x.iter().filter(|w| y.contains(*w)).count();
    shared as f64 / x.len().min(y.len()) as f64
}

/// A data: URL with the cover, or None.
pub fn find_cover(title: &str, artist: &str) -> Option<String> {
    for want in candidates(title, artist) {
        match search(&want) {
            Ok(Some(found)) => return Some(found),
            Ok(None) => {}
            Err(e) => {
                eprintln!("Cover lookup failed: {e}");
                return None;
            }
        }
    }
    None
}

fn search(want: &Want) -> Result<Option<String>, String> {
    let term = format!("{} {}", want.artist, want.title).trim().to_string();
    let mut res = HTTP
        .get("https://itunes.apple.com/search")
        .query_pairs([("term", term.as_str()), ("entity", "song"), ("limit", "10")])
        .call()
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Ok(None);
    }
    let data: Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
    let mut top: Option<&Value> = None;
    let mut top_score = 0.99;
    let list = data.get("results").and_then(Value::as_array);
    for r in list.into_iter().flatten() {
        let s = overlap(crate::lyrics::str_of(r, "trackName"), &want.title) * 2.0
            + if want.artist.is_empty() { 1.0 } else { overlap(crate::lyrics::str_of(r, "artistName"), &want.artist) };
        if s > top_score {
            top = Some(r);
            top_score = s;
        }
    }
    let Some(art) = top.and_then(|r| r.get("artworkUrl100")).and_then(Value::as_str) else { return Ok(None) };
    let url = art.replacen("100x100bb", "300x300bb", 1);
    let mut img = HTTP.get(&url).call().map_err(|e| e.to_string())?;
    if !img.status().is_success() {
        return Ok(None);
    }
    let kind = img
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = img.body_mut().read_to_vec().map_err(|e| e.to_string())?;
    Ok(Some(format!("data:{kind};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))))
}

/// The bytes inside a base64 data: URL.
pub fn data_url_bytes(url: &str) -> Option<Vec<u8>> {
    let (_, data) = url.split_once(";base64,")?;
    base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()
}

/// An average of the cover that counts its lively pixels most, so a bright patch on a dark cover beats the murky
/// mean, then made light and saturated enough to read well as text over anything.
pub fn colour_of_cover(data_url: &str) -> Option<String> {
    let bytes = data_url_bytes(data_url)?;
    let img = image::load_from_memory(&bytes).ok()?;
    let small = img.resize_exact(24, 24, image::imageops::FilterType::Triangle).to_rgba8();
    let (mut r, mut g, mut b, mut total) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for px in small.pixels() {
        let (red, green, blue) = (px[0] as f64, px[1] as f64, px[2] as f64);
        let max = red.max(green).max(blue);
        let sat = if max > 0.0 { (max - red.min(green).min(blue)) / max } else { 0.0 };
        let weight = 0.04 + sat * sat * (max / 255.0); // grey and dark pixels count for little
        r += red * weight;
        g += green * weight;
        b += blue * weight;
        total += weight;
    }
    if total == 0.0 {
        return None;
    }
    Some(readable_colour(r / total, g / total, b / total))
}

fn readable_colour(r: f64, g: f64, b: f64) -> String {
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let (mut h, mut s, mut l) = (0.0, 0.0, (max + min) / 2.0);
    if d != 0.0 {
        s = d / (1.0 - (2.0 * l - 1.0f64).abs());
        h = if max == r { ((g - b) / d) % 6.0 } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
        h = (h * 60.0 + 360.0) % 360.0;
    }
    if s > 0.12 {
        s = s.max(0.65); // a real colour: make it vivid; a grey cover stays a light grey
    }
    l = l.max(0.64).min(0.8);
    let k = |n: f64| (n + h / 30.0) % 12.0;
    let a = s * l.min(1.0 - l);
    let f = |n: f64| ((l - a * (k(n) - 3.0).min(9.0 - k(n)).min(1.0).max(-1.0)) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", f(0.0), f(8.0), f(4.0))
}
