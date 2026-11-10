// The overlay's six fonts, built in, plus Windows' own fonts for scripts they don't cover. CJK, Korean, Thai and
// Indic fonts are big, so they are only loaded the first time a song needs them.

use egui::{FontData, FontDefinitions, FontFamily, FontId};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

const BUILT_IN: &[(&str, &[u8])] = &[
    ("Inter-400", include_bytes!("../fonts/Inter-400.ttf")),
    ("Inter-600", include_bytes!("../fonts/Inter-600.ttf")),
    ("Inter-800", include_bytes!("../fonts/Inter-800.ttf")),
    ("Serif-400", include_bytes!("../fonts/InstrumentSerif-400.ttf")),
    ("Serif-Italic", include_bytes!("../fonts/InstrumentSerif-Italic.ttf")),
    ("Pixel-400", include_bytes!("../fonts/Pixelify-400.ttf")),
    ("Pixel-600", include_bytes!("../fonts/Pixelify-600.ttf")),
    ("Pixel-700", include_bytes!("../fonts/Pixelify-700.ttf")),
    ("Bebas-400", include_bytes!("../fonts/Bebas-400.ttf")),
    ("Nunito-600", include_bytes!("../fonts/Nunito-600.ttf")),
    ("Nunito-800", include_bytes!("../fonts/Nunito-800.ttf")),
    ("Nunito-900", include_bytes!("../fonts/Nunito-900.ttf")),
    ("Caveat-600", include_bytes!("../fonts/Caveat-600.ttf")),
    ("Caveat-700", include_bytes!("../fonts/Caveat-700.ttf")),
];

/// Windows fonts for whole scripts: (name, file, index in a .ttc, which characters need it).
const SYSTEM: &[(&str, &str, u32, fn(char) -> bool)] = &[
    ("sys-cjk", "msyh.ttc", 0, |c| matches!(c as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF)),
    ("sys-korean", "malgun.ttf", 0, |c| matches!(c as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF)),
    ("sys-thai", "LeelawUI.ttf", 0, |c| matches!(c as u32, 0x0E00..=0x0E7F | 0x0E80..=0x0EFF | 0x1780..=0x17FF)),
    ("sys-indic", "Nirmala.ttc", 0, |c| matches!(c as u32, 0x0900..=0x0DFF)),
];

struct State {
    defs: FontDefinitions,
    loaded: HashSet<&'static str>,
}
static STATE: Mutex<Option<State>> = Mutex::new(None);

fn font_file(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    std::fs::read(format!("{dir}\\Fonts\\{name}")).ok()
}

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let defaults_proportional = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (name, bytes) in BUILT_IN {
        defs.font_data.insert((*name).into(), Arc::new(FontData::from_static(bytes)));
    }
    // Segoe UI covers what the built-in fonts lack in Latin, Greek, Cyrillic, Arabic and Hebrew.
    let mut fallback: Vec<String> = Vec::new();
    if let Some(bytes) = font_file("segoeui.ttf") {
        defs.font_data.insert("sys-segoe".into(), Arc::new(FontData::from_owned(bytes)));
        fallback.push("sys-segoe".into());
    }
    if let Some(bytes) = font_file("seguisym.ttf") {
        defs.font_data.insert("sys-symbol".into(), Arc::new(FontData::from_owned(bytes)));
        fallback.push("sys-symbol".into());
    }
    fallback.extend(defaults_proportional.iter().cloned());
    for (name, _) in BUILT_IN {
        let mut list = vec![(*name).to_string()];
        if !name.starts_with("Inter") {
            list.push("Inter-400".into());
        }
        list.extend(fallback.iter().cloned());
        defs.families.insert(FontFamily::Name((*name).into()), list);
    }
    // egui's own widgets (the colour code field) use Inter too.
    let mut prop = vec!["Inter-400".to_string()];
    prop.extend(fallback.iter().cloned());
    defs.families.insert(FontFamily::Proportional, prop);
    ctx.set_fonts(defs.clone());
    *STATE.lock().unwrap() = Some(State { defs, loaded: HashSet::new() });
}

/// Loads the Windows fonts this text needs, if they aren't loaded yet.
pub fn ensure(ctx: &egui::Context, text: &str) {
    let mut guard = STATE.lock().unwrap();
    let Some(state) = guard.as_mut() else { return };
    let mut changed = false;
    for (name, file, index, needs) in SYSTEM {
        if state.loaded.contains(name) || !text.chars().any(needs) {
            continue;
        }
        state.loaded.insert(name);
        let Some(bytes) = font_file(file) else { continue };
        let mut data = FontData::from_owned(bytes);
        data.index = *index;
        state.defs.font_data.insert((*name).into(), Arc::new(data));
        for list in state.defs.families.values_mut() {
            list.push((*name).into());
        }
        changed = true;
    }
    if changed {
        ctx.set_fonts(state.defs.clone());
    }
}

/// The overlay's font setting ('clean', 'serif', ...) at a CSS weight, as the browser would pick it from the
/// weights that are loaded (a heavier one when the exact weight is missing).
pub fn family(font: &str, weight: u32, italic: bool) -> FontFamily {
    let pick = |have: &[u32]| -> u32 { *have.iter().find(|w| **w >= weight).unwrap_or(have.last().unwrap()) };
    let name = match font {
        "serif" => (if italic { "Serif-Italic" } else { "Serif-400" }).to_string(),
        "pixel" => format!("Pixel-{}", pick(&[400, 600, 700])),
        "bold" => "Bebas-400".to_string(),
        "round" => format!("Nunito-{}", pick(&[600, 800, 900])),
        "hand" => format!("Caveat-{}", pick(&[600, 700])),
        _ => format!("Inter-{}", if weight <= 400 { 400 } else { pick(&[400, 600, 800]) }),
    };
    FontFamily::Name(name.into())
}

pub fn inter(weight: u32, size: f32) -> FontId {
    FontId::new(size, family("clean", weight, false))
}
