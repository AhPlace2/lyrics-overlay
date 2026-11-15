// The overlay's six fonts, built in, plus Windows' own fonts for scripts they don't cover. CJK, Korean, Thai and
// Indic fonts are big, so they are only loaded the first time a song needs them. Fonts the user adds live in the
// fonts folder next to the settings; the font setting names one as "custom:<file name>".

use egui::{FontData, FontDefinitions, FontFamily, FontId};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const CUSTOM: &str = "custom:";
const EXTENSIONS: &[&str] = &["ttf", "otf", "ttc", "otc"];

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

#[derive(PartialEq)]
enum Custom {
    /// Handed to egui this pass; usable from the next one.
    Pending,
    Ready,
    /// Missing or not a font: the font setting draws as Clean.
    Failed,
}

struct State {
    defs: FontDefinitions,
    loaded: HashSet<&'static str>,
    custom: HashMap<String, Custom>,
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
    *STATE.lock().unwrap() = Some(State { defs, loaded: HashSet::new(), custom: HashMap::new() });
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

/* ---------- Fonts the user adds ---------- */

/// Where added fonts live.
pub fn dir() -> PathBuf {
    crate::core::data_dir().join("fonts")
}

static LIST: Mutex<Option<(Instant, Vec<(String, String)>)>> = Mutex::new(None);

/// The font files in the fonts folder as (setting value, label), by name. Looked at again every two seconds, so
/// files dropped into the folder show up while the settings are open.
pub fn custom_fonts() -> Vec<(String, String)> {
    let mut guard = LIST.lock().unwrap();
    if let Some((at, list)) = guard.as_ref() {
        if at.elapsed() < Duration::from_secs(2) {
            return list.clone();
        }
    }
    let mut list: Vec<(String, String)> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| is_font_name(name))
        .map(|name| (format!("{CUSTOM}{name}"), label_of(&name)))
        .collect();
    list.sort_by_key(|(_, label)| label.to_lowercase());
    *guard = Some((Instant::now(), list.clone()));
    list
}

fn is_font_name(name: &str) -> bool {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    !name.contains(['/', '\\']) && EXTENSIONS.contains(&ext.as_str())
}

/// "Montserrat-Bold.ttf" -> "Montserrat Bold", kept short enough for a chip.
fn label_of(name: &str) -> String {
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let label = stem.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
    if label.chars().count() > 22 {
        format!("{}…", label.chars().take(21).collect::<String>().trim_end())
    } else {
        label
    }
}

/// Whether this setting names an added font that is in the fonts folder.
pub fn is_custom_choice(font: &str) -> bool {
    font.strip_prefix(CUSTOM).is_some_and(|name| is_font_name(name) && dir().join(name).is_file())
}

/// The file's bytes, if egui can read it as a font. egui crashes on a file it can't, so everything is checked here.
fn read_font(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    skrifa::FontRef::from_index(&bytes, 0).ok()?;
    Some(bytes)
}

/// Copies a font file into the fonts folder and returns its setting value, or what went wrong.
pub fn import(path: &Path) -> Result<String, String> {
    let name = path.file_name().and_then(|n| n.to_str()).ok_or("That file can't be used")?.to_string();
    if !is_font_name(&name) {
        return Err("Pick a .ttf or .otf font file".into());
    }
    let bytes = read_font(path).ok_or("That file isn't a font the app can read")?;
    let folder = dir();
    std::fs::create_dir_all(&folder).map_err(|_| "Couldn't make the fonts folder")?;
    // The same file again is used as it is; a different file with the same name gets a number.
    let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, "ttf"));
    let mut target = name.clone();
    for n in 2.. {
        match std::fs::read(folder.join(&target)) {
            Ok(have) if have == bytes => break,
            Ok(_) => target = format!("{stem} ({n}).{ext}"),
            Err(_) => {
                std::fs::write(folder.join(&target), &bytes).map_err(|_| "Couldn't copy the font")?;
                break;
            }
        }
    }
    *LIST.lock().unwrap() = None;
    let value = format!("{CUSTOM}{target}");
    // A file by this name that was missing or broken before gets another try.
    if let Some(state) = STATE.lock().unwrap().as_mut() {
        if state.custom.get(&value) == Some(&Custom::Failed) {
            state.custom.remove(&value);
        }
    }
    Ok(value)
}

/// Call first thing each pass: added fonts handed to egui last pass are in use now.
pub fn begin_pass() {
    let mut guard = STATE.lock().unwrap();
    let Some(state) = guard.as_mut() else { return };
    for c in state.custom.values_mut() {
        if *c == Custom::Pending {
            *c = Custom::Ready;
        }
    }
}

/// Loads the added font this setting names, if it is one and isn't loaded yet. egui takes it from the next pass;
/// until then (and for good when the file is gone or broken) the setting draws as Clean.
pub fn ensure_custom(ctx: &egui::Context, font: &str) {
    let Some(name) = font.strip_prefix(CUSTOM) else { return };
    let mut guard = STATE.lock().unwrap();
    let Some(state) = guard.as_mut() else { return };
    if state.custom.contains_key(font) {
        return;
    }
    let Some(bytes) = is_font_name(name).then(|| read_font(&dir().join(name))).flatten() else {
        state.custom.insert(font.into(), Custom::Failed);
        return;
    };
    state.defs.font_data.insert(font.into(), Arc::new(FontData::from_owned(bytes)));
    // What it lacks comes from the same fonts as everything else: Inter, then Windows' fonts.
    let mut list = vec![font.to_string()];
    list.extend(state.defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    state.defs.families.insert(FontFamily::Name(font.into()), list);
    ctx.set_fonts(state.defs.clone());
    ctx.request_repaint(); // the pass it can be used in
    state.custom.insert(font.into(), Custom::Pending);
}

fn custom_ready(font: &str) -> bool {
    STATE.lock().unwrap().as_ref().is_some_and(|s| s.custom.get(font) == Some(&Custom::Ready))
}

/// The overlay's font setting ('clean', 'serif', ..., or an added font) at a CSS weight, as the browser would pick
/// it from the weights that are loaded (a heavier one when the exact weight is missing).
pub fn family(font: &str, weight: u32, italic: bool) -> FontFamily {
    if font.starts_with(CUSTOM) {
        return if custom_ready(font) { FontFamily::Name(font.into()) } else { family("clean", weight, italic) };
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_labels() {
        assert!(is_font_name("Montserrat-Bold.TTF"));
        assert!(is_font_name("a.otf"));
        assert!(!is_font_name("notes.txt"));
        assert!(!is_font_name(r"..\evil.ttf"));
        assert_eq!(label_of("Montserrat-Bold.ttf"), "Montserrat Bold");
        assert_eq!(label_of("A_Very_Long_Font_Name_That_Goes_On.ttf"), "A Very Long Font Name…");
    }

    #[test]
    fn only_real_fonts_are_read() {
        let dir = std::env::temp_dir().join(format!("lo-font-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("broken.ttf");
        std::fs::write(&bad, b"not a font at all").unwrap();
        assert!(read_font(&bad).is_none());
        let good = dir.join("Caveat.ttf");
        std::fs::write(&good, BUILT_IN[12].1).unwrap();
        assert!(read_font(&good).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
