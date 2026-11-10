// The app's state and everything that isn't drawing: settings, now playing, lyrics, cover, song recognition, timing
// fixes, the settings panel's actions and when the overlay catches the mouse. Port of src/main/main.js; the UI
// (overlay.rs, panel.rs) reads this state every frame and calls in here for what the pages used to send.

use crate::lyrics::{self, lines_of, same_timing, str_of, Track};
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

pub const PEEK_KEYS: &[(&str, &str)] = &[("off", "Off"), ("ctrl", "Ctrl"), ("shift", "Shift"), ("alt", "Alt")];
pub const LOOKS: &[(&str, &str)] = &[("depth", "Depth (3D)"), ("glow", "Glow"), ("outline", "Outline"), ("plain", "Plain")];
pub const MOTIONS: &[(&str, &str)] =
    &[("none", "None"), ("wave", "Wave"), ("bounce", "Bounce"), ("pulse", "Pulse"), ("float", "Float"), ("flicker", "Flicker")];
pub const ENTRANCES: &[(&str, &str)] = &[("rise", "Rise"), ("pop", "Pop"), ("slide", "Slide"), ("drop", "Drop"), ("fade", "Fade")];
pub const SIDES: &[(&str, &str)] = &[("bottom", "Below"), ("top", "Above"), ("left", "Left"), ("right", "Right")];
pub const ALIGNS: &[(&str, &str)] = &[("left", "Left"), ("center", "Centre"), ("right", "Right")];
pub const COLOUR_MODES: &[(&str, &str)] = &[("accent", "Colour"), ("rainbow", "Rainbow"), ("fade", "Fade"), ("cover", "From the cover")];
pub const FONTS: &[(&str, &str)] =
    &[("clean", "Clean"), ("serif", "Serif"), ("pixel", "Pixel"), ("bold", "Poster"), ("round", "Round"), ("hand", "Hand")];
const TOGGLES: &[&str] = &["enabled", "showCard", "showCardLyrics", "showBig", "showCover", "showArtist", "showProgress", "showControls", "recognise", "checkUpdates", "useGpu", "startupAnimation"];

fn defaults() -> Map<String, Value> {
    let v = json!({
        "enabled": true,
        "showCard": true,
        "showCardLyrics": true,
        "showBig": true,
        "look": "depth",
        "colourMode": "accent",
        "motion": "none",
        "font": "clean",
        "accent": "#ffb648",
        "history": [],
        "fadeFrom": "#ff6fae",
        "fadeTo": "#6cc8ff",
        "entrance": "rise",
        "bigPos": { "x": 0.5, "y": 0.44 },
        "bigWidth": 0.84,
        "bigSize": 1,
        "bigAlign": "center",
        "offsets": {},
        "picks": {},
        "card": { "x": 24, "y": 24 },
        "cardScale": 1,
        "cardOpacity": 0.84,
        "cardLyricsSide": "bottom",
        "cardNext": 2,
        "showCover": true,
        "showArtist": true,
        "showProgress": true,
        "showControls": true,
        "peekKey": "off",
        "recognise": true,
        "checkUpdates": true,
        "useGpu": true,
        "startupAnimation": true,
    });
    v.as_object().unwrap().clone()
}

fn range(key: &str) -> Option<(f64, f64, f64)> {
    Some(match key {
        "cardScale" => (0.6, 2.0, 0.01),
        "cardOpacity" => (0.0, 1.0, 0.01),
        "cardNext" => (0.0, 4.0, 1.0),
        "bigWidth" => (0.25, 0.98, 0.01),
        "bigSize" => (0.4, 2.5, 0.01),
        _ => return None,
    })
}

/// A number as JSON, whole numbers without a fraction (as JavaScript writes them).
pub fn num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

/// What the UI is told about, besides the state it reads.
#[derive(Clone, Debug)]
pub enum Ev {
    Toast(String),
    SyncNow,
    PointerLeft,
    /// Open the settings panel next to the card at (x, y, width) in overlay points, or close it when open.
    TogglePanel(Option<(f64, f64, f64)>),
    ClosePanel,
    Quit,
}

pub struct Core {
    pub settings: Map<String, Value>,
    pub settings_version: u64,
    pub last_state: Option<Value>,
    pub state_version: u64,
    /// (song key, data URL), and a counter that changes with each new cover.
    pub last_cover: Option<(String, String)>,
    pub cover_version: u64,
    pub cover_colour: Option<(String, String)>,
    pub last_lyrics: Option<Value>,
    pub lyrics_version: u64,
    pub editing: bool,
    pub peeking: bool,
    pub panel_open: bool,
    pub events: Vec<Ev>,
    pub failed_hotkeys: Vec<String>,
    // Catching the mouse: where the card is (overlay points), and whether the page holds the pointer.
    pub card_rect: Option<[f64; 4]>,
    /// The settings panel, drawn in the overlay too: caught like the card, and a press anywhere else closes it.
    pub panel_rect: Option<[f64; 4]>,
    pub page_hold: bool,
    pub page_busy: bool,
    pub catching: bool,
    pub quitting: bool,
    looking_for: Option<String>,
    cover_timer: u64,
    lyrics_cache: HashMap<String, Value>,
    versions_cache: HashMap<String, Vec<Value>>,
    searching_versions: bool,
    committed_accent: Option<String>,
    recognised: HashMap<String, (String, String)>,
    recognise_tries: HashMap<String, u32>,
    recognising: Option<String>,
    held_key: &'static str,
    held_since: Instant,
    last_down: Instant,
    peek_held: bool,
    was_pressed: bool,
}

static CORE: LazyLock<Mutex<Core>> = LazyLock::new(|| {
    let now = Instant::now();
    Mutex::new(Core {
        settings: defaults(),
        settings_version: 1,
        last_state: None,
        state_version: 0,
        last_cover: None,
        cover_version: 0,
        cover_colour: None,
        last_lyrics: None,
        lyrics_version: 0,
        editing: false,
        peeking: false,
        panel_open: false,
        events: Vec::new(),
        failed_hotkeys: Vec::new(),
        card_rect: None,
        panel_rect: None,
        page_hold: false,
        page_busy: false,
        catching: false,
        quitting: false,
        looking_for: None,
        cover_timer: 0,
        lyrics_cache: HashMap::new(),
        versions_cache: HashMap::new(),
        searching_versions: false,
        committed_accent: None,
        recognised: HashMap::new(),
        recognise_tries: HashMap::new(),
        recognising: None,
        held_key: "none",
        held_since: now,
        last_down: now,
        peek_held: false,
        was_pressed: false,
    })
});

/// LO_SWITCH_TEST only: notes for the renderer-switch test, in %TEMP%\lo-switch.txt.
pub fn test_log(what: &str) {
    if std::env::var("LO_SWITCH_TEST").is_ok() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("lo-switch.txt")) {
            let _ = writeln!(f, "{:?} {what}", std::time::SystemTime::now());
        }
    }
}

pub fn core() -> MutexGuard<'static, Core> {
    CORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The UI's context, so background threads can ask for a redraw.
pub static UI: OnceLock<egui::Context> = OnceLock::new();
pub fn notify() {
    if let Some(ctx) = UI.get() {
        ctx.request_repaint();
    }
}

/* ---------- Settings ---------- */
// Same folder as the Electron build, so both share settings, timing fixes and saved lyrics.
pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into())).join("lyrics-overlay")
}
fn settings_file() -> PathBuf {
    data_dir().join("settings.json")
}

fn load_settings() -> Map<String, Value> {
    let Ok(text) = std::fs::read_to_string(settings_file()) else { return defaults() };
    let Ok(Value::Object(mut saved)) = serde_json::from_str::<Value>(&text) else { return defaults() };
    saved.shift_remove("lead"); // Timing used to be one setting for every song; it is per song now.
    if let Some(effect) = saved.get("effect").and_then(Value::as_str).map(String::from) {
        if !saved.contains_key("look") {
            let (look, colour, motion) = match effect.as_str() {
                "depth" => ("depth", "accent", "none"),
                "glow" => ("glow", "accent", "none"),
                "rainbow" => ("depth", "rainbow", "none"),
                "wave" => ("glow", "accent", "wave"),
                "plain" => ("plain", "accent", "none"),
                _ => ("", "", ""),
            };
            if !look.is_empty() {
                saved.insert("look".into(), json!(look));
                saved.insert("colourMode".into(), json!(colour));
                saved.insert("motion".into(), json!(motion));
            }
        }
    }
    saved.shift_remove("effect");
    // The big lyrics used to have two places to be; now they go anywhere.
    if let Some(p) = saved.get("bigPosition").and_then(Value::as_str).map(String::from) {
        if !saved.contains_key("bigPos") {
            saved.insert("bigPos".into(), json!({ "x": 0.5, "y": if p == "bottom" { 0.76 } else { 0.44 } }));
        }
    }
    saved.shift_remove("bigPosition");
    // A look or motion that has since been removed goes back to the default.
    let valid = |list: &[(&str, &str)], v: Option<&Value>| v.and_then(Value::as_str).map(|s| list.iter().any(|(k, _)| *k == s)).unwrap_or(false);
    if !valid(LOOKS, saved.get("look")) {
        saved.shift_remove("look");
    }
    if !valid(MOTIONS, saved.get("motion")) {
        saved.shift_remove("motion");
    }
    let mut out = defaults();
    for (k, v) in saved {
        out.insert(k, v);
    }
    for k in ["offsets", "picks"] {
        if !out.get(k).map(Value::is_object).unwrap_or(false) {
            out.insert(k.into(), json!({}));
        }
    }
    out
}

impl Core {
    pub fn s(&self, key: &str) -> &Value {
        self.settings.get(key).unwrap_or(&Value::Null)
    }
    pub fn flag(&self, key: &str) -> bool {
        self.s(key).as_bool().unwrap_or(false)
    }

    fn save_settings(&self) {
        let file = settings_file();
        let _ = std::fs::create_dir_all(file.parent().unwrap());
        if let Err(e) = std::fs::write(&file, serde_json::to_string_pretty(&Value::Object(self.settings.clone())).unwrap()) {
            eprintln!("Could not save settings: {e}");
        }
    }

    fn update(&mut self, patch: Map<String, Value>) {
        for (k, v) in patch {
            self.settings.insert(k, v);
        }
        self.save_settings();
        self.settings_version += 1;
        notify();
    }
    fn update1(&mut self, key: &str, v: Value) {
        let mut m = Map::new();
        m.insert(key.into(), v);
        self.update(m);
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.events.push(Ev::Toast(msg.into()));
        notify();
    }

    /// Sets a number setting (kept in its range) or where the big lyrics are. Ignores anything else.
    fn set_value(&mut self, key: &str, value: &Value) {
        if key == "bigPos" {
            let (Some(x), Some(y)) = (value.get("x").and_then(Value::as_f64), value.get("y").and_then(Value::as_f64)) else { return };
            let c = |v: f64| (v.clamp(0.0, 1.0) * 1000.0).round() / 1000.0;
            return self.update1("bigPos", json!({ "x": num(c(x)), "y": num(c(y)) }));
        }
        let (Some((min, max, step)), Some(v)) = (range(key), value.as_f64()) else { return };
        if !v.is_finite() {
            return;
        }
        let v = ((v.clamp(min, max) / step).round() * step * 1000.0).round() / 1000.0;
        self.update1(key, num(v));
    }

    /* ---------- Now playing ---------- */
    fn set_cover(&mut self, key: String, data: String) {
        let colour = crate::cover::colour_of_cover(&data);
        self.cover_colour = colour.map(|c| (key.clone(), c));
        self.last_cover = Some((key, data));
        self.cover_version += 1;
        notify();
    }

    /// Picker colours. A colour counts as picked (and the one before it goes into the history) when the picker is
    /// let go of; the many colours sent while dragging only show it.
    fn choose_accent(&mut self, hex: String, commit: bool) {
        let mut patch = Map::new();
        patch.insert("accent".into(), json!(hex));
        patch.insert("colourMode".into(), json!("accent"));
        if commit {
            let before = self.committed_accent.clone();
            let rest: Vec<String> = self
                .s("history")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|c| *c != hex && Some(c.to_string()) != before)
                .map(String::from)
                .collect();
            let mut list = match &before {
                Some(b) if *b != hex => [vec![b.clone()], rest].concat(),
                _ => rest,
            };
            list.truncate(2);
            patch.insert("history".into(), json!(list));
            self.committed_accent = Some(hex);
        }
        self.update(patch);
    }

    pub fn song_lead(&self) -> f64 {
        match &self.last_state {
            Some(s) if s["active"].as_bool() == Some(true) => self.s("offsets").get(song_key(s)).and_then(Value::as_f64).unwrap_or(0.0),
            _ => 0.0,
        }
    }

    fn set_lead(&mut self, key: &str, lead: f64, message: Option<String>) {
        let lead = ((lead * 100.0).round() / 100.0).clamp(-120.0, 120.0);
        let mut offsets = self.s("offsets").as_object().cloned().unwrap_or_default();
        offsets.shift_remove(key);
        if lead != 0.0 {
            offsets.insert(key.into(), num(lead));
        }
        if offsets.len() > 300 {
            if let Some(first) = offsets.keys().next().cloned() {
                offsets.shift_remove(&first);
            } // Keep the file small; the oldest fix goes first.
        }
        self.update1("offsets", Value::Object(offsets));
        let msg = message.unwrap_or_else(|| {
            if lead == 0.0 {
                "Timing reset".into()
            } else {
                format!("Lyrics {:.2} s {}", lead.abs(), if lead > 0.0 { "earlier" } else { "later" })
            }
        });
        self.toast(msg);
    }

    fn nudge(&mut self, delta: f64) {
        let Some(s) = self.last_state.clone().filter(|s| s["active"].as_bool() == Some(true)) else { return self.toast("Nothing is playing") };
        let lead = self.song_lead() + delta;
        self.set_lead(&song_key(&s), lead, None);
    }

    fn toggle_enabled(&mut self) {
        let on = !self.flag("enabled");
        self.update1("enabled", json!(on));
        self.toast(if on { "Lyrics on" } else { "Lyrics hidden" });
    }

    fn set_editing(&mut self, on: bool) {
        self.editing = on;
        if on {
            self.set_peeking(false);
        }
        notify();
    }

    fn set_peeking(&mut self, on: bool) {
        if on != self.peeking {
            self.peeking = on;
            notify();
        }
    }

    fn active_state(&self) -> Option<Value> {
        self.last_state.clone().filter(|s| s["active"].as_bool() == Some(true))
    }

    fn track_of(&self, state: &Value) -> Track {
        let key = str_of(state, "key");
        let r = self.recognised.get(key);
        Track {
            title: r.map(|r| r.0.clone()).unwrap_or_else(|| str_of(state, "title").into()),
            artist: r.map(|r| r.1.clone()).unwrap_or_else(|| str_of(state, "artist").into()),
            duration: state["end"].as_f64().unwrap_or(0.0) - state["start"].as_f64().unwrap_or(0.0),
        }
    }

    fn publish_lyrics(&mut self, key: &str, value: &Value) {
        let mut l = json!({ "key": key });
        for (k, v) in value.as_object().into_iter().flatten() {
            l[k] = v.clone();
        }
        self.last_lyrics = Some(l);
        self.lyrics_version += 1;
        notify();
    }

    /* ---------- Settings panel ---------- */
    pub fn panel_state(&self) -> Value {
        let s = self.active_state();
        let lyrics = s.as_ref().and_then(|s| self.last_lyrics.as_ref().filter(|l| str_of(l, "key") == str_of(s, "key")));
        let opts = |list: &[(&str, &str)]| json!(list.iter().map(|(v, l)| json!([v, l])).collect::<Vec<_>>());
        let song = s.as_ref().map(|s| {
            let status = lyrics.map(|l| str_of(l, "status")).unwrap_or("loading");
            json!({
                "title": if str_of(s, "title").is_empty() { "Unknown title" } else { str_of(s, "title") },
                "artist": str_of(s, "artist"),
                "lead": num(self.song_lead()),
                "picked": self.s("picks").get(song_key(s)).is_some(),
                "status": status,
                "provider": if status == "found" { json!(lyrics.map(|l| str_of(l, "provider")).filter(|p| !p.is_empty()).unwrap_or("LRCLIB")) } else { Value::Null },
                "listening": self.recognising.as_deref() == Some(str_of(s, "key")),
                "recognisedAs": self.recognised.get(str_of(s, "key")).map(|(t, a)| json!({ "title": t, "artist": a })),
            })
        });
        json!({
            "settings": Value::Object(self.settings.clone()),
            "options": { "looks": opts(LOOKS), "motions": opts(MOTIONS), "fonts": opts(FONTS), "entrances": opts(ENTRANCES), "sides": opts(SIDES), "aligns": opts(ALIGNS), "peekKeys": opts(PEEK_KEYS) },
            "coverColour": self.cover_colour.as_ref().map(|c| c.1.clone()),
            "song": song,
            "update": crate::update::panel_value(),
        })
    }
}

/// Timing fixes are saved per song, keyed the same way the overlay keys them.
pub fn song_key(s: &Value) -> String {
    format!("{}|{}", str_of(s, "artist").to_lowercase(), str_of(s, "title").to_lowercase())
}

/* ---------- From the now-playing thread ---------- */
pub fn on_media(msg: Value) {
    let mut c = core();
    match str_of(&msg, "type") {
        "cover" => c.set_cover(str_of(&msg, "key").into(), str_of(&msg, "data").into()),
        "control" => {
            if msg["ok"].as_bool() != Some(true) {
                let text = match str_of(&msg, "cmd") {
                    "toggle" => "Player didn't respond",
                    "next" => "No next song here",
                    "prev" => "No previous song here",
                    "seek" => "This player can't jump",
                    _ => "The player didn't respond",
                };
                c.toast(text);
            }
        }
        "state" => {
            let changed = msg["active"].as_bool() == Some(true)
                && c.last_state.as_ref().map(|s| str_of(s, "key")) != Some(str_of(&msg, "key"));
            c.last_state = Some(msg.clone());
            c.state_version += 1;
            notify();
            if changed {
                drop(c);
                look_up(msg.clone(), false);
                schedule_cover_lookup(msg);
            }
        }
        _ => {}
    }
}

/// Playback controls from the card go to the now-playing thread, which passes them to the player.
pub fn control(msg: &Value) {
    let cmd = str_of(msg, "cmd");
    if !["toggle", "next", "prev", "seek"].contains(&cmd) || !msg["key"].is_string() {
        return;
    }
    let mut out = json!({ "cmd": cmd, "key": msg["key"] });
    if cmd == "seek" {
        let Some(to) = msg["to"].as_f64().filter(|t| t.is_finite() && *t >= 0.0) else { return };
        out["to"] = json!((to * 1000.0).round() / 1000.0);
    }
    if !crate::media::command(out) {
        core().toast("Can't reach the player right now");
    }
}

/// If the player shares no cover within a few seconds, look one up online.
fn schedule_cover_lookup(state: Value) {
    let gen = {
        let mut c = core();
        c.cover_timer += 1;
        c.cover_timer
    };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        let key = str_of(&state, "key").to_string();
        let missing = || {
            let c = core();
            c.cover_timer == gen
                && c.last_state.as_ref().map(|s| str_of(s, "key")) == Some(key.as_str())
                && c.last_cover.as_ref().map(|x| x.0.as_str()) != Some(key.as_str())
        };
        if !missing() {
            return;
        }
        if let Some(data) = crate::cover::find_cover(str_of(&state, "title"), str_of(&state, "artist")) {
            if missing() {
                core().set_cover(key, data);
            }
        }
    });
}

/* ---------- Lyrics found once are saved on disk, so the song loads instantly next time, even offline. ---------- */
const SAVED_LYRICS_VERSION: i64 = 1;
fn saved_lyrics_file(state: &Value) -> PathBuf {
    let hash = Sha1::digest(song_key(state).as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    data_dir().join("lyrics-cache").join(format!("{hex}.json"))
}

fn load_saved_lyrics(c: &Core, state: &Value) -> Option<Value> {
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(saved_lyrics_file(state)).ok()?).ok()?;
    let key = song_key(state);
    let pick = c.s("picks").get(&key).and_then(Value::as_str);
    // Only use it when it is the version this song should show now: the picked one, or the first one.
    if saved["v"].as_i64() != Some(SAVED_LYRICS_VERSION) || str_of(&saved, "key") != key {
        return None;
    }
    let ok = match pick {
        Some(p) => str_of(&saved["value"], "pick") == p,
        None => saved["chosen"].as_bool() != Some(true),
    };
    ok.then(|| saved["value"].clone())
}

fn save_lyrics(c: &Core, state: &Value, value: &Value) {
    if str_of(value, "status") != "found" {
        return;
    }
    let key = song_key(state);
    let file = saved_lyrics_file(state);
    let data = json!({ "v": SAVED_LYRICS_VERSION, "key": key, "chosen": c.s("picks").get(&key).is_some(), "value": value }).to_string();
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(file.parent().unwrap());
        if let Err(e) = std::fs::write(&file, data) {
            eprintln!("Saving lyrics failed: {e}");
        }
    });
}

fn found_value(v: &Value, recognised: Option<&(String, String)>) -> Value {
    let mut out = json!({ "status": "found", "lines": v["lines"], "source": v["source"], "pick": v["pick"], "provider": v["provider"] });
    if let Some((t, a)) = recognised {
        out["recognised"] = json!({ "title": t, "artist": a });
    }
    out
}

/// Shows a song's lyrics: from memory, from disk, or looked up online.
pub fn look_up(state: Value, force: bool) {
    let key = str_of(&state, "key").to_string();
    let publish = move |c: &mut Core, value: Value, state: &Value| {
        let key = str_of(state, "key").to_string();
        if let Some(r) = value.get("recognised") {
            c.recognised.insert(key.clone(), (str_of(r, "title").into(), str_of(r, "artist").into()));
        }
        if c.looking_for.as_deref() == Some(key.as_str()) {
            c.publish_lyrics(&key, &value);
        }
        let status = str_of(&value, "status").to_string();
        if status == "none" || status == "unsynced" {
            let s = state.clone();
            std::thread::spawn(move || recognise_song(s, false));
        }
    };
    {
        let mut c = core();
        c.looking_for = Some(key.clone());
        if str_of(&state, "title").is_empty() {
            return publish(&mut c, json!({ "status": "none", "lines": [] }), &state);
        }
        if !force {
            if let Some(v) = c.lyrics_cache.get(&key).cloned() {
                return publish(&mut c, v, &state);
            }
            if let Some(stored) = load_saved_lyrics(&c, &state) {
                c.lyrics_cache.insert(key.clone(), stored.clone());
                return publish(&mut c, stored, &state);
            }
        }
        publish(&mut c, json!({ "status": "loading", "lines": [] }), &state);
    }
    std::thread::spawn(move || {
        let (track, saved_pick) = {
            let c = core();
            (c.track_of(&state), c.s("picks").get(song_key(&state)).and_then(Value::as_str).map(String::from))
        };
        let value = match lyrics::find_lyrics(&track) {
            Ok(found) => {
                let mut result = found.version.clone();
                // A version picked earlier with "Try other lyrics" wins over the default one.
                if let Some(p) = &saved_pick {
                    if result.as_ref().map(|r| str_of(r, "pick")) != Some(p.as_str()) {
                        if let Some(v) = versions_of(&state).into_iter().find(|v| str_of(v, "pick") == p) {
                            result = Some(v);
                        }
                    }
                }
                match result.filter(|r| !lines_of(r).is_empty()) {
                    Some(r) => found_value(&r, None),
                    None => json!({ "status": if found.plain_only { "unsynced" } else { "none" }, "lines": [] }),
                }
            }
            Err(e) => {
                eprintln!("Lyrics lookup failed: {e}");
                let mut c = core();
                return publish(&mut c, json!({ "status": "error", "lines": [] }), &state);
            }
        };
        let mut c = core();
        c.lyrics_cache.insert(key, value.clone());
        save_lyrics(&c, &state, &value);
        publish(&mut c, value, &state);
    });
}

fn versions_of(state: &Value) -> Vec<Value> {
    let key = str_of(state, "key").to_string();
    if let Some(v) = core().versions_cache.get(&key) {
        return v.clone();
    }
    let track = core().track_of(state);
    let list = lyrics::find_versions(&track);
    core().versions_cache.insert(key, list.clone());
    list
}

/* ---------- Recognising songs without lyrics ----------
   When no lyrics are found, the name the player gives is often the problem (a YouTube video title, a re-upload, a
   mix). Then the app records ~11 s of what the computer plays and asks Shazam what the song really is, and looks
   the lyrics up again under that name. */
const LISTEN_MS: u64 = 11000;
const AUTO_TRIES: u32 = 3;
const RETRY_MS: u64 = 15000;

/// Automatic (no lyrics found): up to three tries per song, 15 s apart. Manual (the "Recognise song" button):
/// always, and it replaces lyrics already shown.
pub fn recognise_song(state: Value, manual: bool) {
    let key = str_of(&state, "key").to_string();
    if key.is_empty() || str_of(&state, "title").is_empty() {
        return;
    }
    let tries;
    {
        let mut c = core();
        if c.recognising.is_some() {
            if manual {
                c.toast("Already listening…");
            }
            return;
        }
        tries = c.recognise_tries.get(&key).copied().unwrap_or(0);
        if !manual && (!c.flag("recognise") || tries >= AUTO_TRIES) {
            return;
        }
        if !manual {
            c.recognise_tries.insert(key.clone(), tries + 1);
        }
        c.recognising = Some(key.clone());
        notify();
    }
    let still = || core().last_state.as_ref().map(|s| str_of(s, "key") == key).unwrap_or(false);
    let last_try = manual || tries + 1 >= AUTO_TRIES;
    let again = |k: String| {
        if !last_try {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(RETRY_MS));
                let st = {
                    let c = core();
                    let same = c.last_state.as_ref().map(|s| str_of(s, "key") == k).unwrap_or(false);
                    let missing = c.last_lyrics.as_ref().map(|l| str_of(l, "key") == k && str_of(l, "status") != "found").unwrap_or(false);
                    if same && missing { c.last_state.clone() } else { None }
                };
                if let Some(s) = st {
                    recognise_song(s, false);
                }
            });
        }
    };
    let result: Result<(), String> = (|| {
        // A paused song is only silence.
        while still() && core().last_state.as_ref().map(|s| str_of(s, "status") != "Playing").unwrap_or(true) {
            std::thread::sleep(Duration::from_secs(1));
        }
        if !still() {
            return Ok(());
        }
        if manual || tries == 0 {
            core().toast("Listening…");
        }
        let rec = crate::listen::record(LISTEN_MS)?;
        if !still() {
            return Ok(());
        }
        if rec.peak < 0.01 {
            if last_try {
                core().toast("Couldn't hear anything playing");
            }
            again(key.clone());
            return Ok(());
        }
        let found = crate::shazam::recognise(&rec.samples)?;
        eprintln!(
            "[recognise] \"{}\": {} (loudest {:.2})",
            str_of(&state, "title"),
            found.as_ref().map(|m| format!("{} – {}", m.artist, m.title)).unwrap_or("no match".into()),
            rec.peak
        );
        if !still() {
            return Ok(());
        }
        let Some(m) = found else {
            if last_try {
                core().toast("Couldn't recognise this song");
            }
            again(key.clone());
            return Ok(());
        };
        let name = if m.artist.is_empty() { m.title.clone() } else { format!("{} – {}", m.artist, m.title) };
        let track = {
            let mut c = core();
            c.recognised.insert(key.clone(), (m.title.clone(), m.artist.clone()));
            c.versions_cache.remove(&key); // "Other lyrics" should search under the new name too
            c.track_of(&state)
        };
        let result = lyrics::find_lyrics(&track)?;
        if !still() {
            return Ok(());
        }
        let Some(v) = result.version.filter(|r| !lines_of(r).is_empty()) else {
            core().toast(format!("Found “{name}”, but no lyrics"));
            return Ok(());
        };
        let value = found_value(&v, Some(&(m.title.clone(), m.artist.clone())));
        let mut c = core();
        c.lyrics_cache.insert(key.clone(), value.clone());
        save_lyrics(&c, &state, &value);
        if c.looking_for.as_deref() == Some(key.as_str()) {
            c.publish_lyrics(&key, &value);
        }
        c.toast(format!("Found: {name}"));
        Ok(())
    })();
    let mut c = core();
    if let Err(e) = result {
        eprintln!("Recognising the song failed: {e}");
        if manual {
            c.toast("Couldn't recognise it now");
        }
    }
    c.recognising = None;
    notify();
}

/// Switches to the next (or, with step -1, the previous) differently timed version of this song's lyrics.
fn try_other_lyrics(step: i32) {
    let state = {
        let mut c = core();
        let Some(state) = c.active_state() else { return c.toast("Nothing is playing") };
        if c.searching_versions {
            return;
        }
        c.searching_versions = true;
        if !c.versions_cache.contains_key(str_of(&state, "key")) {
            c.toast("Looking for other lyrics…");
        }
        state
    };
    std::thread::spawn(move || {
        let versions = versions_of(&state);
        let mut c = core();
        c.searching_versions = false;
        if c.last_state.as_ref().map(|s| str_of(s, "key")) != Some(str_of(&state, "key")) {
            return;
        }
        let current = c
            .last_lyrics
            .clone()
            .filter(|l| str_of(l, "key") == str_of(&state, "key") && str_of(l, "status") == "found");
        let at = current.as_ref().and_then(|cur| versions.iter().position(|v| str_of(v, "pick") == str_of(cur, "pick")));
        let differs = |v: &Value| current.as_ref().map(|cur| !same_timing(&lines_of(v), &lines_of(cur))).unwrap_or(true);
        let next = if step > 0 {
            let from = at.map(|a| a + 1).unwrap_or(0);
            versions[from.min(versions.len())..].iter().find(|v| differs(v)).cloned()
        } else {
            match at {
                Some(a) if a > 0 => versions[..a].iter().rev().find(|v| differs(v)).cloned(),
                _ => None,
            }
        };
        let Some(next) = next else {
            return c.toast(if step > 0 { "No other lyrics found" } else { "Already on the first version" });
        };
        let value = found_value(&next, None);
        let key = str_of(&state, "key").to_string();
        c.lyrics_cache.insert(key.clone(), value.clone());
        c.publish_lyrics(&key, &value);
        // A timing fix made for the old version would be wrong for this one.
        let sk = song_key(&state);
        let mut offsets = c.s("offsets").as_object().cloned().unwrap_or_default();
        offsets.shift_remove(&sk);
        let mut picks = c.s("picks").as_object().cloned().unwrap_or_default();
        picks.shift_remove(&sk);
        picks.insert(sk, next["pick"].clone());
        let mut patch = Map::new();
        patch.insert("picks".into(), Value::Object(picks));
        patch.insert("offsets".into(), Value::Object(offsets));
        c.update(patch);
        save_lyrics(&c, &state, &value); // after the pick is stored, so the file is marked as the chosen version
        let n = versions.iter().position(|v| str_of(v, "pick") == str_of(&next, "pick")).unwrap_or(0) + 1;
        c.toast(format!("Version {n} of {} ({})", versions.len(), str_of(&next, "provider")));
    });
}

fn use_first_lyrics() {
    let state = {
        let mut c = core();
        let Some(state) = c.active_state() else { return };
        let mut picks = c.s("picks").as_object().cloned().unwrap_or_default();
        picks.shift_remove(&song_key(&state));
        c.update1("picks", Value::Object(picks));
        c.lyrics_cache.remove(str_of(&state, "key"));
        c.toast("Back to the first lyrics");
        state
    };
    look_up(state, true);
}

/* ---------- What the overlay page used to send ---------- */
pub fn send(channel: &str, data: Value) {
    let mut c = core();
    match channel {
        "card-moved" => {
            if let (Some(x), Some(y)) = (data["x"].as_f64(), data["y"].as_f64()) {
                c.update1("card", json!({ "x": x.round() as i64, "y": y.round() as i64 }));
            }
        }
        "set" => {
            let key = str_of(&data, "key").to_string();
            c.set_value(&key, &data["value"]);
        }
        "edit-done" => c.set_editing(false),
        "open-settings" => {
            let at = match (data["x"].as_f64(), data["y"].as_f64(), data["width"].as_f64()) {
                (Some(x), Some(y), Some(w)) => Some((x, y, w)),
                _ => None,
            };
            c.events.push(Ev::TogglePanel(at));
            notify();
        }
        "control" => {
            drop(c);
            control(&data);
        }
        "ui-hover" => {
            c.page_hold = data.as_bool().unwrap_or(false) || data.as_str() == Some("busy");
            c.page_busy = data.as_str() == Some("busy");
            drop(c);
            update_catching();
        }
        "peek-now" => {
            if !c.editing {
                c.set_peeking(true);
                drop(c);
                update_catching();
            }
        }
        "card-rect" => {
            c.card_rect = match (data["x"].as_f64(), data["y"].as_f64(), data["width"].as_f64(), data["height"].as_f64()) {
                (Some(x), Some(y), Some(w), Some(h)) => Some([x, y, w, h]),
                _ => None,
            };
        }
        "set-offset" => {
            if let (Some(key), Some(lead)) = (data["key"].as_str(), data["lead"].as_f64()) {
                let msg = if lead == 0.0 {
                    "Already in sync".to_string()
                } else {
                    format!("Synced: {:.2} s {}", lead.abs(), if lead > 0.0 { "earlier" } else { "later" })
                };
                c.set_lead(key, lead, Some(msg));
            }
        }
        _ => {}
    }
}

/* ---------- The settings panel's buttons ---------- */
pub fn panel_action(name: &str, value: Value) {
    let mut c = core();
    let is_hex = |v: &Value| v.as_str().map(|s| s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|ch| ch.is_ascii_hexdigit())).unwrap_or(false);
    match name {
        "toggle" => {
            let key = value.as_str().unwrap_or("");
            if key == "enabled" {
                c.toggle_enabled();
            } else if TOGGLES.contains(&key) {
                let on = !c.flag(key);
                c.update1(key, json!(on));
                if key == "checkUpdates" && on {
                    crate::update::check_soon();
                }
            }
        }
        "set" => {
            let key = str_of(&value, "key").to_string();
            c.set_value(&key, &value["value"]);
        }
        "reset-layout" => {
            let d = defaults();
            let mut patch = Map::new();
            for k in ["card", "cardScale", "cardOpacity", "cardLyricsSide", "cardNext", "bigPos", "bigWidth", "bigSize", "bigAlign"] {
                patch.insert(k.into(), d[k].clone());
            }
            c.update(patch);
            c.toast("Layout reset");
        }
        "reset-all" => {
            // Every setting back to its default. Per-song timing fixes, lyrics picks and recent colours stay.
            let mut d = defaults();
            for k in ["offsets", "picks", "history"] {
                d.shift_remove(k);
            }
            c.update(d);
            c.toast("Settings reset");
        }
        "choose" => {
            let key = str_of(&value, "key").to_string();
            let v = &value["value"];
            // Colours from the picker come as #rrggbb, and switch to the mode they belong to so they show straight away.
            if key == "accent" && is_hex(v) {
                let commit = value["commit"].as_bool().unwrap_or(false);
                c.choose_accent(v.as_str().unwrap().to_lowercase(), commit);
            } else if (key == "fadeFrom" || key == "fadeTo") && is_hex(v) {
                let mut patch = Map::new();
                patch.insert(key, json!(v.as_str().unwrap().to_lowercase()));
                patch.insert("colourMode".into(), json!("fade"));
                c.update(patch);
            } else {
                let list = match key.as_str() {
                    "look" => LOOKS,
                    "motion" => MOTIONS,
                    "colourMode" => COLOUR_MODES,
                    "font" => FONTS,
                    "entrance" => ENTRANCES,
                    "cardLyricsSide" => SIDES,
                    "bigAlign" => ALIGNS,
                    "peekKey" => PEEK_KEYS,
                    _ => &[],
                };
                if list.iter().any(|(k, _)| Some(*k) == v.as_str()) {
                    c.update1(&key, v.clone());
                }
            }
        }
        "earlier" => c.nudge(0.25),
        "later" => c.nudge(-0.25),
        "sync" => {
            c.events.push(Ev::SyncNow);
            notify();
        }
        "reset" => {
            let lead = c.song_lead();
            c.nudge(-lead);
        }
        "next" => {
            drop(c);
            try_other_lyrics(1);
        }
        "prev" => {
            drop(c);
            try_other_lyrics(-1);
        }
        "first" => {
            drop(c);
            use_first_lyrics();
        }
        "search" => {
            if let Some(s) = c.active_state() {
                drop(c);
                look_up(s, true);
            }
        }
        "recognise" => match c.active_state() {
            Some(s) => {
                drop(c);
                std::thread::spawn(move || recognise_song(s, true));
            }
            None => c.toast("Nothing is playing"),
        },
        "move" => {
            c.events.push(Ev::ClosePanel);
            c.set_editing(true);
        }
        "update-now" => crate::update::install(),
        "update-page" => {
            if let Some(url) = value.as_str().filter(|u| u.starts_with("https://github.com/")) {
                let _ = std::process::Command::new("explorer").arg(url).spawn();
            }
        }
        "quit" => {
            c.events.push(Ev::Quit);
            notify();
        }
        "close" => {
            c.events.push(Ev::ClosePanel);
            notify();
        }
        _ => {}
    }
}

/* ---------- Tray and shortcuts ---------- */
pub fn hotkey(name: &str) {
    match name {
        "toggle" => core().toggle_enabled(),
        "earlier" => core().nudge(0.25),
        "later" => core().nudge(-0.25),
        "move" => {
            let mut c = core();
            let on = !c.editing;
            c.set_editing(on);
        }
        "sync" => {
            core().events.push(Ev::SyncNow);
            notify();
        }
        "other" => try_other_lyrics(1),
        "previous" => try_other_lyrics(-1),
        _ => {}
    }
}

pub fn second_instance() {
    core().toast("Already running. Use the tray icon.");
}

/* ---------- Click-through ----------
   The overlay lets clicks through to the desktop, except over the card. A thread looks where the pointer is 30
   times a second and catches the mouse only while it is over the card. While caught, the overlay gets the mouse as
   usual; it holds on during drags and seeks. */
pub struct Screen {
    /// The overlay window's top-left corner in physical pixels, and physical pixels per point.
    pub origin: (i32, i32),
    pub scale: f64,
}
pub static SCREEN: Mutex<Screen> = Mutex::new(Screen { origin: (0, 0), scale: 1.0 });

fn pointer_in(r: Option<[f64; 4]>) -> bool {
    let Some([x0, y0, w, h]) = r else { return false };
    let (cx, cy) = crate::winutil::cursor_pos();
    let s = SCREEN.lock().unwrap();
    let x = (cx - s.origin.0) as f64 / s.scale;
    let y = (cy - s.origin.1) as f64 / s.scale;
    x >= x0 && x < x0 + w && y >= y0 && y < y0 + h
}
fn pointer_over_card(c: &Core) -> bool {
    pointer_in(c.card_rect)
}

/// Some keyboards report a held key as quickly going up and down again. So a key only counts as let go once it has
/// stayed up for 250 ms, and as held (not tapped) once it is still down 150 ms after it was first pressed.
fn peek_key_held(c: &mut Core, now: Instant) -> bool {
    let want = c.s("peekKey").as_str().unwrap_or("off").to_string();
    let want = want.as_str();
    if !["ctrl", "shift", "alt"].contains(&want) {
        c.held_key = "none";
        return false;
    }
    let key = crate::winutil::modifier_alone();
    if key == "busy" {
        c.held_key = "none";
    } else if key != "up" {
        if key != c.held_key {
            c.held_key = key;
            c.held_since = now;
        }
        c.last_down = now;
    } else if c.held_key != "none" && now.duration_since(c.last_down) >= Duration::from_millis(250) {
        c.held_key = "none";
    }
    c.held_key == want && c.last_down.duration_since(c.held_since) >= Duration::from_millis(150)
}

pub fn update_catching() {
    let (want, changed) = {
        let mut c = core();
        let now = Instant::now();
        c.peek_held = peek_key_held(&mut c, now);
        if c.peek_held && !c.editing && !c.page_busy {
            c.set_peeking(true);
        }
        let over = pointer_over_card(&c);
        let over_panel = pointer_in(c.panel_rect);
        // The panel closes like a menu when you click anywhere else (presses on the overlay itself are handled there).
        let pressed = crate::winutil::is_down(0x01) || crate::winutil::is_down(0x02);
        if pressed && !c.was_pressed && c.panel_rect.is_some() && !over_panel && !over {
            test_log("panel closed: click outside it");
            c.events.push(Ev::ClosePanel);
            notify();
        }
        c.was_pressed = pressed;
        // A hidden card comes back once its key is let go and the pointer is off the card.
        if c.peeking && !c.peek_held && !over {
            c.set_peeking(false);
        }
        let want = c.editing || c.page_busy || over_panel || (!c.peeking && (c.page_hold || over));
        let changed = want != c.catching;
        c.catching = want;
        if changed && !want {
            c.events.push(Ev::PointerLeft);
        }
        (want, changed)
    };
    if changed {
        crate::winutil::set_passthrough(!want);
        notify();
    }
}

pub fn start() {
    let mut c = core();
    c.settings = load_settings();
    c.committed_accent = c.s("accent").as_str().map(String::from);
    c.settings_version += 1;
    drop(c);
    std::thread::Builder::new()
        .name("click-through".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_millis(33));
            update_catching();
        })
        .unwrap();
    if std::env::var("LO_DEMO").is_ok() {
        std::thread::spawn(demo);
    } else {
        crate::media::start();
    }
}

/// LO_DEMO=1: a made-up song with synced lyrics instead of the real player, for testing the animations.
fn demo() {
    let key = "demo|Demo Artist|Demo Song".to_string();
    let words = [
        "Every word should glide in smoothly now",
        "Lights across the window, moving slow",
        "Hold the line and let the chorus grow",
        "Somewhere in the static there's a song",
        "Singing it together all night long",
        "One more time before the morning comes",
    ];
    let lines: Vec<Value> = (0..200).map(|i| json!({ "t": 1.0 + i as f64 * 2.4, "text": words[i % words.len()] })).collect();
    let t0 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64;
    let start = Instant::now();
    loop {
        let pos = start.elapsed().as_secs_f64();
        let state = json!({
            "type": "state", "active": true, "key": key, "app": "Demo", "title": "Demo Song", "artist": "Demo Artist",
            "album": "", "status": "Playing", "position": pos, "start": 0.0, "end": 480.0, "updated": t0 + pos * 1000.0,
            "controls": { "toggle": true, "next": true, "prev": true, "seek": true },
        });
        {
            let mut c = core();
            let first = c.last_state.is_none();
            c.last_state = Some(state);
            c.state_version += 1;
            if first {
                c.looking_for = Some(key.clone());
                c.publish_lyrics(&key, &json!({ "status": "found", "lines": lines, "provider": "Demo" }));
            }
        }
        notify();
        std::thread::sleep(Duration::from_secs(5));
    }
}
