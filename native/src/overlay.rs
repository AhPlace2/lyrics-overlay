// The overlay: the now-playing card, the big floating lyrics, arranging them, and messages. Port of
// src/overlay/overlay.{js,css,html}, drawn with egui each frame instead of a web page.
//
// Like the page, nothing is redrawn while nothing moves: each frame works out when something will next change
// (the next lyric line, the clock's next second, the progress bar's next pixel) and asks to be woken then. While
// something animates it draws at most 60 times a second, as the page's stepped CSS animations did.

use crate::colour::{alpha, hex, hsl, mix, mix_oklab, rgba};
use crate::core::{self, core, num};
use crate::draw::{self, bezier, ease, keyframes, line, text, tween, Pointer, Txt};
use crate::fonts;
use crate::lyrics::str_of;
use egui::emath::TSTransform;
use egui::epaint::{Mesh, Shadow, StrokeKind};
use egui::{pos2, vec2, Color32, Context, CursorIcon, FontId, Galley, Id, LayerId, Order, Pos2, Rect, Shape, Stroke, TextureHandle, Vec2};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GRID: f32 = 24.0;
const MARGIN: f32 = 24.0;
const SNAP: f32 = 12.0;
/// The startup animation's length in seconds, and how long it waits for the window to show before it starts.
const INTRO: f32 = 2.6;
const INTRO_DELAY: f64 = 0.25;
const INTRO_PENDING: f64 = f64::NEG_INFINITY;
const TEXT: Color32 = Color32::from_rgb(0xf4, 0xf4, 0xf1);
fn dim() -> Color32 {
    rgba(0xf4, 0xf4, 0xf1, 0.55)
}

pub fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

fn fade_c(c: Color32, o: f32) -> Color32 {
    alpha(c, o)
}

/// Text to draw as a mesh, moved by `xf`; with `blur` > 0 it is a soft shadow.
pub struct TextDraw {
    pub galley: Arc<Galley>,
    pub pos: Pos2,
    pub color: Color32,
    pub xf: TSTransform,
    pub offset: Vec2,
    pub blur: f32,
}

/// A stack of text copies drawn as one cached picture (see draw::sprite), multiplied by `tint`.
pub struct SpriteDraw {
    pub galley: Arc<Galley>,
    pub pos: Pos2,
    pub xf: TSTransform,
    pub layers: Vec<draw::Layer>,
    pub tint: Color32,
}

#[derive(Default)]
pub struct Group {
    pub shapes: Vec<Shape>,
    pub texts: Vec<TextDraw>,
    pub sprites: Vec<SpriteDraw>,
}
impl Group {
    pub fn shape(&mut self, s: impl Into<Shape>) {
        self.shapes.push(s.into());
    }
    pub fn text(&mut self, galley: &Arc<Galley>, pos: Pos2, color: Color32) {
        self.texts.push(TextDraw { galley: galley.clone(), pos, color, xf: TSTransform::IDENTITY, offset: Vec2::ZERO, blur: 0.0 });
    }
    pub fn text_xf(&mut self, galley: &Arc<Galley>, pos: Pos2, color: Color32, xf: TSTransform, offset: Vec2, blur: f32) {
        if color.a() > 0 {
            self.texts.push(TextDraw { galley: galley.clone(), pos, color, xf, offset, blur });
        }
    }
    /// Text with the shadow a see-through card gives it.
    fn text_clear(&mut self, galley: &Arc<Galley>, pos: Pos2, color: Color32, clear: bool, o: f32) {
        if clear {
            self.text_xf(galley, pos, rgba(0, 0, 0, 0.9 * o), TSTransform::IDENTITY, Vec2::ZERO, 1.0);
            self.text_xf(galley, pos, rgba(0, 0, 0, 0.85 * o), TSTransform::IDENTITY, vec2(0.0, 1.0), 3.0);
        }
        self.text(galley, pos, fade_c(color, o));
    }
    pub fn paint(self, painter: &egui::Painter, uv: Vec2) {
        for s in self.shapes {
            painter.add(s);
        }
        if !self.texts.is_empty() {
            let mut mesh = Mesh::default();
            for t in &self.texts {
                let xf = t.xf;
                let f = move |p: Pos2| xf * p;
                if t.blur > 0.0 {
                    draw::soft_text(&mut mesh, &t.galley, t.pos, t.offset, t.blur, t.color, uv, &f);
                } else if xf.scaling == 1.0 {
                    // Sharp text: put the galley on whole physical pixels. egui lays glyphs out pixel-aligned
                    // relative to the galley's origin, so a fractional origin makes every glyph sample between
                    // texels and come out blurry.
                    let ppp = painter.ctx().pixels_per_point();
                    let at = xf * (t.pos + t.offset);
                    let snap = (at.to_vec2() * ppp).round() / ppp - at.to_vec2();
                    let g = move |p: Pos2| xf * p + snap;
                    draw::galley_into(&mut mesh, &t.galley, t.pos + t.offset, t.color, uv, &g);
                } else {
                    draw::galley_into(&mut mesh, &t.galley, t.pos + t.offset, t.color, uv, &f);
                }
            }
            painter.add(Shape::mesh(mesh));
        }
        for s in &self.sprites {
            if let Some(shape) = draw::sprite(painter.ctx(), &s.galley, s.pos, s.xf, &s.layers, s.tint) {
                painter.add(shape);
            }
        }
    }
    /// A stack of copies of a word, drawn once and then reused while it only moves, scales and fades.
    fn sprite(&mut self, galley: &Arc<Galley>, pos: Pos2, xf: TSTransform, layers: Vec<draw::Layer>, tint: Color32) {
        if tint != Color32::TRANSPARENT {
            self.sprites.push(SpriteDraw { galley: galley.clone(), pos, xf, layers, tint });
        }
    }
}

pub fn about(c: Pos2, s: f32, t: Vec2) -> TSTransform {
    TSTransform::from_translation(c.to_vec2() + t) * TSTransform::from_scaling(s) * TSTransform::from_translation(-c.to_vec2())
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

#[derive(Clone)]
struct Line {
    t: f64,
    text: String,
}

struct BigLine {
    words: Vec<String>,
    delays: Vec<f32>,
    born: f64,
    leaving: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hold {
    No,
    Yes,
    Busy,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Target {
    None,
    Done,
    CardSize,
    Prev,
    Play,
    Next,
    Gear,
    Bar,
    Card,
    BigMove,
    BigWidth,
    BigSize,
}

struct Drag {
    dx: f32,
    dy: f32,
    x0: f32,
    y0: f32,
    moved: bool,
}
struct SizeDrag {
    x0: f32,
    w0: f32,
    s0: f32,
    scale: f32,
}
#[derive(Clone, Copy, PartialEq)]
struct BigL {
    x: f32,
    y: f32,
    w: f32,
    size: f32,
}
struct BigDrag {
    kind: Target,
    start: BigL,
    live: BigL,
    r: Rect,
    p0: Pos2,
}

/// Where everything on the card is, in overlay points.
#[derive(Clone)]
struct CardL {
    z: f32,
    cz: f32,
    card: Rect,
    boxr: Rect,
    now: Rect,
    cover: Option<Rect>,
    title: Option<(Arc<Galley>, Pos2)>,
    artist: Option<(Arc<Galley>, Pos2)>,
    bar: Option<Rect>,
    times: Option<(Arc<Galley>, Pos2, Arc<Galley>, Pos2)>,
    prev: Option<Rect>,
    play: Option<Rect>,
    next: Option<Rect>,
    gear: Rect,
    eq: Pos2,
    panel: Option<Rect>,
    rows: Vec<Row>,
    hint: Option<Rect>,
    hint_text: Option<(Arc<Galley>, Pos2, Arc<Galley>, Pos2)>,
    done: Option<(Rect, Arc<Galley>)>,
    size_handle: Rect,
}

#[derive(Clone)]
enum RowKind {
    Dim,
    Cur { progress: f32 },
    Note,
    Dots { lit: usize },
}
#[derive(Clone)]
struct Row {
    kind: RowKind,
    galley: Option<Arc<Galley>>,
    pos: Pos2,
    width: f32,
}

pub struct Overlay {
    pub pointer_left: bool,
    s: Map<String, Value>,
    settings_version: u64,
    state_version: u64,
    track: Option<Value>,
    playing: bool,
    anchor_pos: f64,
    anchor_ms: f64,
    idle_since: f64,
    seek_hold: Option<(String, f64, f64)>,
    lyrics_version: u64,
    lyrics_key: Option<String>,
    lyrics_status: String,
    lines: Vec<Line>,
    cover_version: u64,
    cover: Option<(String, TextureHandle)>,
    cover_colour: Option<Color32>,
    shown_idx: Option<i64>,
    shown_key: Option<String>,
    big: Vec<BigLine>,
    pop_at: Option<f64>,
    seeking: Option<(String, f64)>,
    drag: Option<Drag>,
    size_drag: Option<SizeDrag>,
    big_drag: Option<BigDrag>,
    over_ui: Hold,
    mouse_on_card: bool,
    active: Target,
    hover: Target,
    hover_since: f64,
    toast: Option<(String, f64)>,
    /// The startup animation: when it began (seconds of egui time), INTRO_PENDING before the first frame, None
    /// once it has played or when it is turned off.
    intro: Option<f64>,
    play_override: Option<(bool, f64)>,
    sent_rect: Option<Option<[f64; 4]>>,
    guide_x: Option<f32>,
    guide_y: Option<f32>,
    card_at: Pos2,
    card_w: f32,
    editing: bool,
    peeking: bool,
    wake: Option<f64>,
}

impl Overlay {
    pub fn new() -> Self {
        Overlay {
            pointer_left: false,
            s: Map::new(),
            settings_version: 0,
            state_version: 0,
            track: None,
            playing: false,
            anchor_pos: 0.0,
            anchor_ms: 0.0,
            idle_since: now_ms(),
            seek_hold: None,
            lyrics_version: 0,
            lyrics_key: None,
            lyrics_status: "idle".into(),
            lines: Vec::new(),
            cover_version: 0,
            cover: None,
            cover_colour: None,
            shown_idx: None,
            shown_key: None,
            big: Vec::new(),
            pop_at: None,
            seeking: None,
            drag: None,
            size_drag: None,
            big_drag: None,
            over_ui: Hold::No,
            mouse_on_card: false,
            active: Target::None,
            hover: Target::None,
            hover_since: 0.0,
            toast: None,
            intro: Some(INTRO_PENDING),
            play_override: None,
            sent_rect: None,
            guide_x: None,
            guide_y: None,
            card_at: pos2(24.0, 24.0),
            card_w: 340.0,
            editing: false,
            peeking: false,
            wake: None,
        }
    }

    /* ---------- Settings ---------- */
    fn sv(&self, k: &str) -> &Value {
        self.s.get(k).unwrap_or(&Value::Null)
    }
    fn sf(&self, k: &str, d: f64) -> f64 {
        self.sv(k).as_f64().unwrap_or(d)
    }
    fn sb(&self, k: &str) -> bool {
        self.sv(k).as_bool().unwrap_or(false)
    }
    fn ss(&self, k: &str, d: &'static str) -> String {
        self.sv(k).as_str().unwrap_or(d).to_string()
    }
    fn font(&self) -> String {
        self.ss("font", "clean")
    }

    fn song_key(&self) -> String {
        self.track.as_ref().map(core::song_key).unwrap_or_default()
    }
    fn song_lead(&self) -> f64 {
        match &self.track {
            Some(t) => self.sv("offsets").get(core::song_key(t)).and_then(Value::as_f64).unwrap_or(0.0),
            None => 0.0,
        }
    }
    fn length(&self) -> f64 {
        self.track.as_ref().map(|t| t["end"].as_f64().unwrap_or(0.0) - t["start"].as_f64().unwrap_or(0.0)).unwrap_or(0.0)
    }

    /* ---------- Playback clock ---------- */
    fn song_time(&self) -> f64 {
        if let (Some((k, to)), Some(t)) = (&self.seeking, &self.track) {
            if str_of(t, "key") == k {
                return *to;
            }
        }
        let t = if self.playing { self.anchor_pos + (now_ms() - self.anchor_ms) / 1000.0 } else { self.anchor_pos };
        let length = self.length();
        if length > 0.0 {
            t.min(length)
        } else {
            t
        }
    }

    fn on_state(&mut self, s: Value, time: f64) {
        let now = now_ms();
        if s["active"].as_bool() != Some(true) {
            if self.playing {
                self.idle_since = now;
            }
            self.track = None;
            self.playing = false;
            return;
        }
        let is_new = self.track.as_ref().map(|t| str_of(t, "key")) != Some(str_of(&s, "key"));
        let updated = s["updated"].as_f64().unwrap_or(0.0);
        let stamp_ok = updated > now - 6.0 * 3600e3 && updated < now + 5000.0;
        let now_playing = str_of(&s, "status") == "Playing";
        let f = |k: &str| s[k].as_f64().unwrap_or(0.0);
        let reported = (f("position") - f("start") + if now_playing && stamp_ok { (now - updated) / 1000.0 } else { 0.0 }).max(0.0);
        let drift = (reported - self.song_time()).abs();
        let key = str_of(&s, "key").to_string();
        let held = matches!(&self.seek_hold, Some((k, to, until)) if *k == key && now < *until && (reported - to).abs() > 2.0);
        if self.seek_hold.is_some() && !held {
            self.seek_hold = None;
        }
        if !held && self.seeking.is_none() && (is_new || now_playing != self.playing || drift > 0.35) {
            self.anchor_pos = reported;
            self.anchor_ms = now;
        }
        if held && now_playing != self.playing {
            self.anchor_pos = self.song_time();
            self.anchor_ms = now;
        }
        if !now_playing && self.playing {
            self.idle_since = now;
        }
        self.playing = now_playing;
        if is_new {
            self.pop_at = Some(time);
        }
        self.play_override = None;
        self.track = Some(s);
    }

    fn line_index(&self, t: f64) -> i64 {
        let (mut lo, mut hi) = (0usize, self.lines.len());
        while lo < hi {
            let m = (lo + hi) / 2;
            if self.lines[m].t <= t {
                lo = m + 1;
            } else {
                hi = m;
            }
        }
        lo as i64 - 1
    }
    fn next_text_index(&self, from: i64) -> Option<usize> {
        (from.max(0) as usize..self.lines.len()).find(|&i| !self.lines[i].text.is_empty())
    }
    fn line_at(&self, i: i64) -> Option<&Line> {
        if i < 0 {
            None
        } else {
            self.lines.get(i as usize)
        }
    }

    fn show_big_line(&mut self, text: &str, duration: f64, elapsed: f64, time: f64) {
        for l in self.big.iter_mut().filter(|l| l.leaving.is_none()) {
            l.leaving = Some(time);
        }
        if text.is_empty() {
            return;
        }
        let words: Vec<String> = text.split(' ').map(String::from).collect();
        let step = (duration.min(8.0) * 0.45 / words.len() as f64).clamp(0.05, 0.25);
        let delays = (0..words.len()).map(|i| (((i as f64 * step - elapsed).max(-0.42) * 60.0).round() / 60.0) as f32).collect();
        self.big.push(BigLine { words, delays, born: time, leaving: None });
    }

    /* ---------- From the app ---------- */
    pub fn toast(&mut self, msg: &str) {
        self.toast = Some((msg.to_string(), -1.0));
    }

    pub fn pointer_left(&mut self) {
        self.pointer_left = true;
        self.set_over_ui(Hold::No);
        self.mouse_on_card = false;
    }

    /// Where the card is: (x, y, width), for the settings panel.
    pub fn card_place(&self) -> Option<(f64, f64, f64)> {
        Some((self.card_at.x as f64, self.card_at.y as f64, self.card_w as f64))
    }

    pub fn sync_now(&mut self) {
        let ready = self.track.is_some()
            && self.lyrics_key.as_deref() == self.track.as_ref().map(|t| str_of(t, "key"))
            && self.lyrics_status == "found";
        if !ready {
            return self.toast("No synced lyrics to line up");
        }
        let t = self.song_time();
        let idx = self.line_index(t + self.song_lead());
        let target = match self.line_at(idx) {
            Some(l) if !l.text.is_empty() => Some(l.clone()),
            _ => self.next_text_index(idx + 1).map(|i| self.lines[i].clone()),
        };
        let Some(target) = target else { return self.toast("No lyric line to line up") };
        core::send("set-offset", json!({ "key": self.song_key(), "lead": num(((target.t - t) * 100.0).round() / 100.0) }));
    }

    fn set_over_ui(&mut self, mut on: Hold) {
        if self.seeking.is_some() || self.drag.is_some() {
            on = Hold::Busy;
        }
        if on == self.over_ui {
            return;
        }
        self.over_ui = on;
        core::send("ui-hover", match on {
            Hold::No => json!(false),
            Hold::Yes => json!(true),
            Hold::Busy => json!("busy"),
        });
    }

    fn send_control(&self, cmd: &str, to: Option<f64>) {
        if let Some(t) = &self.track {
            let mut m = json!({ "cmd": cmd, "key": t["key"] });
            if let Some(to) = to {
                m["to"] = json!(to);
            }
            core::send("control", m);
        }
    }

    /// Pulls what changed from the app's state.
    fn sync(&mut self, ctx: &Context, time: f64) {
        let c = core();
        self.editing = c.editing;
        self.peeking = c.peeking;
        let mut settings_changed = false;
        if c.settings_version != self.settings_version {
            self.settings_version = c.settings_version;
            self.s = c.settings.clone();
            settings_changed = true;
        }
        let state = (c.state_version != self.state_version).then(|| {
            self.state_version = c.state_version;
            c.last_state.clone()
        });
        let lyrics = (c.lyrics_version != self.lyrics_version).then(|| {
            self.lyrics_version = c.lyrics_version;
            c.last_lyrics.clone()
        });
        let cover = (c.cover_version != self.cover_version).then(|| {
            self.cover_version = c.cover_version;
            c.last_cover.clone()
        });
        self.cover_colour = c.cover_colour.as_ref().and_then(|x| hex(&x.1));
        drop(c);

        if settings_changed {
            self.shown_idx = None; // timing or style may have changed
        }
        if let Some(Some(s)) = state {
            self.on_state(s, time);
            if let Some(t) = &self.track {
                fonts::ensure(ctx, &format!("{} {}", str_of(t, "title"), str_of(t, "artist")));
            }
        }
        if let Some(Some(l)) = lyrics {
            self.lyrics_key = Some(str_of(&l, "key").to_string());
            self.lyrics_status = str_of(&l, "status").to_string();
            self.lines = l["lines"]
                .as_array()
                .map(|a| a.iter().map(|x| Line { t: x["t"].as_f64().unwrap_or(0.0), text: str_of(x, "text").to_string() }).collect())
                .unwrap_or_default();
            let all: String = self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
            fonts::ensure(ctx, &all);
            self.shown_idx = None;
        }
        if let Some(Some((key, data))) = cover {
            if let Some(tex) = load_cover(ctx, &data) {
                self.cover = Some((key, tex));
            }
        }
    }

    /* ---------- Colour ---------- */
    fn hue(time: f64) -> f32 {
        ((time % 6.0) / 6.0 * 360.0) as f32
    }
    fn fade_ends(&self) -> (Color32, Color32) {
        (
            hex(&self.ss("fadeFrom", "#ff6fae")).unwrap_or(Color32::from_rgb(0xff, 0x6f, 0xae)),
            hex(&self.ss("fadeTo", "#6cc8ff")).unwrap_or(Color32::from_rgb(0x6c, 0xc8, 0xff)),
        )
    }
    fn accent(&self, time: f64) -> Color32 {
        let mode = self.ss("colourMode", "accent");
        if mode == "fade" {
            let (a, b) = self.fade_ends();
            return mix_oklab(a, ((Self::hue(time) / 180.0) - 1.0).abs(), b);
        }
        if mode == "cover" {
            if let Some(c) = self.cover_colour {
                return c;
            }
        }
        hex(&self.ss("accent", "#ffb648")).unwrap_or(Color32::from_rgb(0xff, 0xb6, 0x48))
    }
    /// A word's colour: the accent, or for rainbow and fade a hue that moves along the line.
    fn word_colour(&self, i: usize, time: f64, accent: Color32) -> Color32 {
        let hue = Self::hue(time) + i as f32 * 32.0;
        match self.ss("colourMode", "accent").as_str() {
            "rainbow" => hsl(hue, 0.95, 0.66),
            "fade" => {
                let (a, b) = self.fade_ends();
                mix_oklab(a, (hue.rem_euclid(360.0) / 180.0 - 1.0).abs(), b)
            }
            _ => accent,
        }
    }

    /* ---------- Card layout ---------- */
    fn card_fonts(&self) -> (FontId, f32, FontId, f32, FontId, FontId, f32, FontId) {
        // (title, title spacing, artist, artist spacing, panel, current line, panel spacing, time)
        let f = self.font();
        let fam = |w: u32| fonts::family(&f, w, false);
        let (ts, tw, ls) = match f.as_str() {
            "serif" => (18.0, 600, 0.0),
            "bold" => (19.0, 400, 0.03),
            "hand" => (19.0, 600, 0.0),
            _ => (15.0, 600, 0.0),
        };
        let as_ = match f.as_str() {
            "serif" | "bold" => 15.0,
            "hand" => 16.0,
            _ => 13.0,
        };
        let (ps, pls) = match f.as_str() {
            "bold" => (15.0, 0.02),
            "hand" => (16.0, 0.0),
            _ => (13.0, 0.0),
        };
        let (cs, cw) = match f.as_str() {
            "bold" => (18.0, 400),
            "hand" => (18.0, 700),
            _ => (15.0, 700),
        };
        (
            FontId::new(ts, fam(tw)),
            ts * ls,
            FontId::new(as_, fam(400)),
            as_ * ls,
            FontId::new(ps, fam(400)),
            FontId::new(cs, fam(cw)),
            ps * pls,
            FontId::new(11.0, fam(400)),
        )
    }

    fn scaled(f: &FontId, z: f32) -> FontId {
        FontId::new((f.size * z * 2.0).round() / 2.0, f.family.clone())
    }

    /// The card's lyrics, like renderPanel: rows of text, the current line with its underline, or the dots of an
    /// instrumental gap. Returns the rows (positions relative to the panel's content box) and its height.
    fn panel_rows(&self, ctx: &Context, width: f32, z: f32, t: f64) -> (Vec<Row>, f32, bool) {
        let (_, _, _, _, pf, cf, pls, _) = self.card_fonts();
        let (pf, cf) = (Self::scaled(&pf, z), Self::scaled(&cf, z));
        let lh = pf.size * 1.35;
        let mut rows = Vec::new();
        let mut y = 0.0;
        let mut instrumental = false;
        let font = self.font();
        let note = |rows: &mut Vec<Row>, y: &mut f32, s: &str| {
            let italic_font = if font == "serif" { FontId::new(pf.size, fonts::family("serif", 400, true)) } else { pf.clone() };
            let g = text(ctx, s, italic_font, dim(), Txt { wrap: width, spacing: pls * z, italic: font != "serif", line_height: Some(lh), ..Default::default() });
            let h = g.size().y;
            rows.push(Row { kind: RowKind::Note, galley: Some(g), pos: pos2(0.0, *y), width });
            *y += h;
        };
        let Some(track) = &self.track else {
            if self.editing {
                note(&mut rows, &mut y, "Lyrics appear under the card");
            }
            return (rows, y, false);
        };
        let key = str_of(track, "key");
        let status = if self.lyrics_key.as_deref() == Some(key) { self.lyrics_status.as_str() } else { "loading" };
        match status {
            "loading" => note(&mut rows, &mut y, "Looking for lyrics…"),
            "none" => note(&mut rows, &mut y, "No synced lyrics found for this song"),
            "unsynced" => note(&mut rows, &mut y, "Only unsynced lyrics exist for this song"),
            "error" => note(&mut rows, &mut y, "Couldn't reach the lyrics service. Use “Search lyrics again” in the tray menu."),
            "found" => {}
            _ => return (rows, 0.0, false),
        }
        if status != "found" {
            return (rows, y, false);
        }
        let idx = self.line_index(t);
        let cur = self.line_at(idx);
        let next_i = self.next_text_index(idx + 1);
        let gap = next_i.map(|i| self.lines[i].t - t).unwrap_or(f64::INFINITY);
        instrumental = cur.map(|c| c.text.is_empty()).unwrap_or(true) && gap > 2.5 && gap.is_finite();
        let prev_i = (0..idx.max(0) as usize).rev().find(|&i| !self.lines[i].text.is_empty());
        let dim_row = |rows: &mut Vec<Row>, y: &mut f32, s: &str| {
            let g = text(ctx, s, pf.clone(), dim(), Txt { wrap: width, rows: 1, spacing: pls * z, line_height: Some(lh), ..Default::default() });
            rows.push(Row { kind: RowKind::Dim, galley: Some(g), pos: pos2(0.0, *y), width });
            *y += lh;
        };
        if instrumental {
            if let Some(p) = prev_i {
                dim_row(&mut rows, &mut y, &self.lines[p].text);
            }
            let ni = next_i.unwrap();
            let span = self.lines[ni].t - cur.map(|c| c.t).unwrap_or(0.0);
            let lit = (((span - gap) / span) * 4.0).floor().clamp(0.0, 3.0) as usize;
            rows.push(Row { kind: RowKind::Dots { lit }, galley: None, pos: pos2(0.0, y), width });
            y += 19.0 * z;
            dim_row(&mut rows, &mut y, &self.lines[ni].text);
        } else if let Some(c) = cur.filter(|c| !c.text.is_empty()) {
            if let Some(p) = prev_i {
                dim_row(&mut rows, &mut y, &self.lines[p].text);
            }
            let end = self.line_at(idx + 1).map(|l| l.t).unwrap_or(c.t + 4.0);
            let dur = (end - c.t).max(0.1);
            let progress = ((t - c.t) / dur).clamp(0.0, 1.0) as f32;
            let clh = cf.size * 1.35;
            let g = text(ctx, &c.text, cf.clone(), TEXT, Txt { wrap: width, spacing: pls * z, line_height: Some(clh), ..Default::default() });
            y += 3.0 * z;
            let h = g.size().y;
            rows.push(Row { kind: RowKind::Cur { progress }, galley: Some(g), pos: pos2(0.0, y), width });
            y += h + 4.0 * z + 3.0 * z;
            let coming = self.sf("cardNext", 2.0) as usize;
            let mut shown = 0;
            let mut i = idx + 1;
            while (i as usize) < self.lines.len() && shown < coming {
                if !self.lines[i as usize].text.is_empty() {
                    dim_row(&mut rows, &mut y, &self.lines[i as usize].text.clone());
                    shown += 1;
                }
                i += 1;
            }
        } else if let Some(ni) = next_i {
            dim_row(&mut rows, &mut y, &self.lines[ni].text);
        }
        (rows, y, instrumental)
    }

    fn card_layout(&self, ctx: &Context, screen: Rect, t: f64) -> CardL {
        let z = self.size_drag.as_ref().map(|d| d.scale).unwrap_or(self.sf("cardScale", 1.0) as f32);
        let cz = z.max(0.8);
        let (tf, tls, af, als, _, _, _, timef) = self.card_fonts();
        let (show_cover, show_artist, show_progress, show_controls) =
            (self.sb("showCover"), self.sb("showArtist"), self.sb("showProgress"), self.sb("showControls"));
        let mut l = CardL {
            z,
            cz,
            card: Rect::NOTHING,
            boxr: Rect::NOTHING,
            now: Rect::NOTHING,
            cover: None,
            title: None,
            artist: None,
            bar: None,
            times: None,
            prev: None,
            play: None,
            next: None,
            gear: Rect::NOTHING,
            eq: Pos2::ZERO,
            panel: None,
            rows: Vec::new(),
            hint: None,
            hint_text: None,
            done: None,
            size_handle: Rect::NOTHING,
        };

        let now_w = 340.0 * z;
        let pad = 10.0 * z;
        let cover_w = if show_cover { 64.0 * z } else { 0.0 };
        let side_w = (20.0 * cz).max(15.0 * z);
        let meta_x = pad + cover_w + if show_cover { 12.0 * z } else { 0.0 };
        let meta_w = (now_w - meta_x - 12.0 * z - side_w - pad).max(10.0);

        let (title, artist) = match &self.track {
            Some(tr) => (
                if str_of(tr, "title").is_empty() { "Unknown title".to_string() } else { str_of(tr, "title").to_string() },
                str_of(tr, "artist").to_string(),
            ),
            None if self.editing => ("Nothing playing".into(), "The card shows up here".into()),
            None => (String::new(), String::new()),
        };
        let tg = text(ctx, &title, Self::scaled(&tf, z), TEXT, Txt { wrap: meta_w, rows: 1, spacing: tls * z, ..Default::default() });
        let ag = text(ctx, &artist, Self::scaled(&af, z), dim(), Txt { wrap: meta_w, rows: 1, spacing: als * z, ..Default::default() });
        let title_h = tg.size().y.max(Self::scaled(&tf, z).size * 1.2);
        let artist_h = if show_artist { 1.0 * z + ag.size().y.max(Self::scaled(&af, z).size * 1.2) } else { 0.0 };
        let bar_h = if show_progress { 17.0 * z } else { 0.0 };
        let length = self.length();
        let tfz = Self::scaled(&timef, z);
        let el = line(ctx, &fmt(t), tfz.clone(), dim());
        let le = line(ctx, &if length > 0.0 { fmt(length) } else { "–:––".into() }, tfz.clone(), dim());
        let has_row = show_progress || show_controls;
        let row_m = if show_progress { 2.0 * z } else { 4.0 * z };
        let row_h = if has_row { (if show_controls { 24.0 * cz } else { 0.0 }).max(if show_progress { el.size().y } else { 0.0 }) } else { 0.0 };
        let meta_h = title_h + artist_h + bar_h + if has_row { row_m + row_h } else { 0.0 };
        let side_h = 20.0 * cz + 14.0 * z;
        let inner_h = meta_h.max(cover_w).max(side_h);
        let now_h = inner_h + 2.0 * pad;

        // Where the card's lyrics go.
        let side = self.ss("cardLyricsSide", "bottom");
        let beside = side == "left" || side == "right";
        let panel_w = if beside { 250.0 * z } else { now_w };
        let show_panel = self.sb("showCardLyrics");
        let (rows, rows_h, _) = if show_panel { self.panel_rows(ctx, panel_w - 24.0 * z, z, t + self.song_lead()) } else { (Vec::new(), 0.0, false) };
        let has_panel = show_panel && !rows.is_empty();
        let panel_h = rows_h + 18.0 * z;
        let gap = 6.0 * z;
        let (now_at, panel_at, box_size) = if !has_panel {
            (vec2(0.0, 0.0), vec2(0.0, 0.0), vec2(now_w, now_h))
        } else {
            match side.as_str() {
                "top" => (vec2(0.0, panel_h + gap), vec2(0.0, 0.0), vec2(now_w, now_h + gap + panel_h)),
                "left" => (vec2(panel_w + gap, 0.0), vec2(0.0, 0.0), vec2(now_w + gap + panel_w, now_h.max(panel_h))),
                "right" => (vec2(0.0, 0.0), vec2(now_w + gap, 0.0), vec2(now_w + gap + panel_w, now_h.max(panel_h))),
                _ => (vec2(0.0, 0.0), vec2(0.0, now_h + gap), vec2(now_w, now_h + gap + panel_h)),
            }
        };

        // The arranging hint under the card (not scaled with it).
        let mut card_w = box_size.x;
        let mut hint_h = 0.0;
        let mut hint_parts = None;
        if self.editing {
            card_w = card_w.max(300.0);
            let done = line(ctx, "Done", fonts::inter(700, 13.0), Color32::from_rgb(0x11, 0x11, 0x11));
            let btn = vec2(done.size().x + 24.0, done.size().y + 12.0);
            let tw = card_w - 20.0 - 10.0 - btn.x;
            let a = text(ctx, "Drag the card and the big lyrics where you want them", fonts::inter(400, 13.0), TEXT, Txt { wrap: tw, ..Default::default() });
            let b = text(ctx, "Corners and sides resize. Snaps to lines and the grid; hold Alt to move freely.", fonts::inter(400, 11.0), dim(), Txt { wrap: tw, ..Default::default() });
            let th = a.size().y + 2.0 + b.size().y;
            hint_h = th.max(btn.y) + 16.0;
            hint_parts = Some((a, b, done, btn, th));
        }
        let total_h = box_size.y + if self.editing { 12.0 + hint_h } else { 0.0 };

        // Place it on screen (kept on it).
        let (sx, sy) = match &self.drag {
            Some(_) => (self.card_at.x, self.card_at.y),
            None => {
                let c = self.sv("card");
                (c["x"].as_f64().unwrap_or(24.0) as f32, c["y"].as_f64().unwrap_or(24.0) as f32)
            }
        };
        let x = sx.min((screen.width() - card_w).max(0.0)).max(0.0);
        let y = sy.min((screen.height() - 120.0).max(0.0)).max(0.0);
        let o = pos2(x, y);
        l.card = rect(x, y, card_w, total_h);
        l.boxr = Rect::from_min_size(o, box_size);
        let now = Rect::from_min_size(o + now_at, vec2(now_w, now_h));
        l.now = now;
        if show_cover {
            l.cover = Some(rect(now.left() + pad, now.top() + pad + (inner_h - cover_w) / 2.0, cover_w, cover_w));
        }
        let my = now.top() + pad + (inner_h - meta_h) / 2.0;
        let mx = now.left() + meta_x;
        l.title = Some((tg, pos2(mx, my)));
        let mut cy = my + title_h;
        if show_artist {
            l.artist = Some((ag, pos2(mx, cy + 1.0 * z)));
            cy += artist_h;
        }
        if show_progress {
            l.bar = Some(rect(mx, cy, meta_w, 20.0 * z));
            cy += bar_h;
        }
        if has_row {
            let ry = cy + row_m;
            let mid = ry + row_h / 2.0;
            if show_progress {
                let elp = pos2(mx, mid - el.size().y / 2.0);
                let lep = pos2(mx + meta_w - le.size().x, mid - le.size().y / 2.0);
                l.times = Some((el, elp, le, lep));
            }
            if show_controls {
                let cx = mx + meta_w / 2.0;
                l.prev = Some(rect(cx - 40.0 * cz, mid - 11.0 * cz, 24.0 * cz, 22.0 * cz));
                l.play = Some(rect(cx - 14.0 * cz, mid - 12.0 * cz, 28.0 * cz, 24.0 * cz));
                l.next = Some(rect(cx + 16.0 * cz, mid - 11.0 * cz, 24.0 * cz, 22.0 * cz));
            }
        }
        l.gear = rect(now.right() - pad + 4.0 * cz - 24.0 * cz, now.top() + pad - 4.0 * cz, 24.0 * cz, 24.0 * cz);
        l.eq = pos2(now.right() - pad - 2.0 * z, now.top() + pad + inner_h - 2.0 * z);
        if has_panel {
            let pr = Rect::from_min_size(o + panel_at, vec2(panel_w, panel_h));
            l.panel = Some(pr);
            let content = pr.min + vec2(12.0 * z, 9.0 * z);
            l.rows = rows.into_iter().map(|mut r| {
                r.pos = content + r.pos.to_vec2();
                r
            }).collect();
        }
        if let Some((a, b, done, btn, th)) = hint_parts {
            let hr = rect(x, y + box_size.y + 12.0, card_w, hint_h);
            l.hint = Some(hr);
            let ty = hr.top() + (hint_h - th) / 2.0;
            l.hint_text = Some((a.clone(), pos2(hr.left() + 10.0, ty), b, pos2(hr.left() + 10.0, ty + a.size().y + 2.0)));
            let br = rect(hr.right() - 10.0 - btn.x, hr.center().y - btn.y / 2.0, btn.x, btn.y);
            l.done = Some((br, done));
        }
        l.size_handle = Rect::from_center_size(l.boxr.max + vec2(4.0 * z, 4.0 * z), vec2(14.0 * z, 14.0 * z));
        l
    }

    /* ---------- Big lyrics layout ---------- */
    fn big_font(&self, screen: Rect, size: f32) -> (FontId, f32) {
        let f = self.font();
        let vw = screen.width() / 100.0;
        let base = match f.as_str() {
            "serif" => (4.4 * vw).clamp(38.0, 82.0),
            "bold" => (4.0 * vw).clamp(36.0, 76.0),
            "hand" => (4.6 * vw).clamp(40.0, 86.0),
            _ => (3.4 * vw).clamp(30.0, 64.0),
        };
        let fs = ((base * size) * 2.0).round() / 2.0;
        let (w, italic, ls) = match f.as_str() {
            "serif" => (400, true, 0.0),
            "pixel" => (700, false, 0.02),
            "bold" => (400, false, 0.03),
            "round" => (900, false, 0.0),
            "hand" => (700, false, 0.0),
            _ => (800, false, -0.01),
        };
        (FontId::new(fs, fonts::family(&f, w, italic)), ls * fs)
    }

    fn big_layout(&self) -> BigL {
        let p = self.sv("bigPos");
        BigL {
            x: p["x"].as_f64().unwrap_or(0.5) as f32,
            y: p["y"].as_f64().unwrap_or(0.44) as f32,
            w: self.sf("bigWidth", 0.84) as f32,
            size: self.sf("bigSize", 1.0) as f32,
        }
    }

    /// The big lyrics' box on screen, kept on it.
    fn big_rect(&self, screen: Rect, l: BigL) -> (Rect, BigL) {
        let (font, _) = self.big_font(screen, l.size);
        let w = l.w * screen.width();
        let h = 2.6 * font.size;
        let half_w = (l.w / 2.0).min(0.5);
        let half_h = (h / screen.height() / 2.0).min(0.5);
        let x = l.x.max(half_w).min(1.0 - half_w);
        let y = l.y.max(half_h).min(1.0 - half_h);
        let x = (x * 10000.0).round() / 10000.0;
        let y = (y * 10000.0).round() / 10000.0;
        (Rect::from_center_size(pos2(screen.left() + x * screen.width(), screen.top() + y * screen.height()), vec2(w, h)), BigL { x, y, ..l })
    }

    /* ---------- Each frame ---------- */
    pub fn ui(&mut self, ctx: &Context, screen: Rect, panel_open: bool) {
        let time = ctx.input(|i| i.time);
        self.sync(ctx, time);
        if let Some((_, t0)) = &mut self.toast {
            if *t0 < 0.0 {
                *t0 = time;
            }
        }
        if self.intro == Some(INTRO_PENDING) {
            // Read from core: the overlay's own copy of the settings may not have arrived on the first frame.
            self.intro = core().flag("startupAnimation").then_some(time + INTRO_DELAY);
        }
        let now = now_ms();
        let mut wake: f64 = f64::INFINITY; // seconds
        let mut animating = false;
        let accent = self.accent(time);
        let editing = self.editing;
        let p = Pointer::read(ctx);

        /* --- What shows --- */
        let recent = self.playing || now - self.idle_since < 20000.0;
        let show_card = editing
            || (self.sb("enabled") && self.sb("showCard") && self.track.is_some() && (recent || self.mouse_on_card || self.seeking.is_some()));
        if !recent && self.track.is_some() {
            // nothing
        } else if self.track.is_some() && !self.playing {
            wake = wake.min((self.idle_since + 20000.0 - now) / 1000.0 + 0.01);
        }
        let t = self.song_time();
        let lt = t + self.song_lead();

        // Big lyrics: the line for this moment.
        let ready = self.track.is_some()
            && self.lyrics_key.as_deref() == self.track.as_ref().map(|t| str_of(t, "key"))
            && self.lyrics_status == "found";
        let want_big = self.sb("enabled") && self.sb("showBig") && ready;
        let fade_big = !want_big || (!self.playing && now - self.idle_since > 4000.0);
        if want_big && !self.playing && now - self.idle_since <= 4000.0 {
            wake = wake.min((self.idle_since + 4000.0 - now) / 1000.0 + 0.01);
        }
        if let Some(track) = self.track.clone() {
            let key = str_of(&track, "key").to_string();
            let idx = if ready { self.line_index(lt) } else { -1 };
            if self.playing && self.seeking.is_none() && ready {
                if let Some(n) = self.line_at(idx + 1) {
                    wake = wake.min(n.t - lt + 0.005);
                }
            }
            if self.playing && self.seeking.is_none() {
                wake = wake.min(1.0 - t.rem_euclid(1.0) + 0.005); // the clock's next second
            }
            if want_big && (Some(idx) != self.shown_idx || self.shown_key.as_deref() != Some(key.as_str())) {
                let cur = self.line_at(idx).cloned();
                let end = self.line_at(idx + 1).map(|l| l.t).unwrap_or(cur.as_ref().map(|c| c.t + 4.0).unwrap_or(0.0));
                let joined_late = self.shown_key.as_deref() != Some(key.as_str())
                    || self.shown_idx.is_none()
                    || (idx - self.shown_idx.unwrap()).abs() > 1;
                let (text, dur, el) = match &cur {
                    Some(c) => (c.text.clone(), end - c.t, if joined_late { lt - c.t } else { 0.0 }),
                    None => (String::new(), 0.0, 0.0),
                };
                self.show_big_line(&text, dur, el, time);
                self.shown_idx = Some(idx);
                self.shown_key = Some(key);
            }
            if !want_big && self.shown_idx.is_some() {
                self.show_big_line("", 0.0, 0.0, time);
                self.shown_idx = None;
            }
        } else if self.shown_idx.is_some() {
            self.show_big_line("", 0.0, 0.0, time);
            self.shown_idx = None;
        }
        self.big.retain(|l| l.leaving.map(|s| time - s < 0.8).unwrap_or(true));

        /* --- Card layout --- */
        let cl = self.card_layout(ctx, screen, t);
        self.card_at = cl.card.min;
        self.card_w = cl.card.width();
        // Tell the app where the card is, so the mouse is only caught over a card that can appear.
        let can = editing || (self.sb("enabled") && self.sb("showCard") && self.track.is_some());
        let r = can.then(|| {
            let c = cl.card;
            [c.left() as f64, c.top() as f64, c.width() as f64, c.height() as f64]
        });
        if self.sent_rect != Some(r) {
            self.sent_rect = Some(r);
            core::send("card-rect", match r {
                Some([x, y, w, h]) => json!({ "x": x, "y": y, "width": w, "height": h }),
                None => Value::Null,
            });
        }

        let show_t = tween(ctx, "card-show", if show_card { 1.0 } else { 0.0 }, 0.35);
        let peek_t = tween(ctx, "card-peek", if self.peeking { 0.0 } else { 1.0 }, 0.15);
        let card_o = ease(show_t) * peek_t;
        let card_live = show_card && !self.peeking;

        // The big lyrics' box, live while being arranged.
        let big_l = self.big_drag.as_ref().map(|d| d.live).unwrap_or_else(|| self.big_layout());
        let (big_r, big_placed) = self.big_rect(screen, big_l);
        let big_off = !(self.sb("enabled") && self.sb("showBig"));

        /* --- Pointer --- */
        self.mouse_on_card = p.over(cl.card);
        let alt = crate::winutil::alt_down();
        let ctl_hit = |r: Rect, cz: f32| Rect::from_min_max(r.min - vec2(1.0 * cz, 4.0 * cz), r.max + vec2(1.0 * cz, 6.0 * cz));
        let controls = self.track.as_ref().map(|t| t["controls"].clone()).unwrap_or(Value::Null);
        let can_ctl = |k: &str| controls[k].as_bool().unwrap_or(false);
        let bar_off = !(can_ctl("seek") && self.length() > 0.0);
        let hit_target = |p: Pos2| -> Target {
            if card_live {
                if editing {
                    if let Some((br, _)) = &cl.done {
                        if br.contains(p) {
                            return Target::Done;
                        }
                    }
                    if cl.size_handle.expand(6.0 * cl.z).contains(p) {
                        return Target::CardSize;
                    }
                }
                if !editing {
                    for (r, tg, k) in [(cl.prev, Target::Prev, "prev"), (cl.play, Target::Play, "toggle"), (cl.next, Target::Next, "next")] {
                        if let Some(r) = r {
                            if can_ctl(k) && ctl_hit(r, cl.cz).contains(p) {
                                return tg;
                            }
                        }
                    }
                    if cl.gear.expand(4.0 * cl.cz).contains(p) {
                        return Target::Gear;
                    }
                    if let Some(b) = cl.bar {
                        if !bar_off && b.contains(p) {
                            return Target::Bar;
                        }
                    }
                } else if cl.gear.expand(4.0 * cl.cz).contains(p) {
                    return Target::Gear;
                }
                if cl.card.contains(p) {
                    return Target::Card;
                }
            }
            if editing && !big_off {
                let left = Rect::from_min_size(pos2(big_r.left() - 10.0, big_r.center().y - 18.0), vec2(8.0, 36.0));
                let right = Rect::from_min_size(pos2(big_r.right() + 2.0, big_r.center().y - 18.0), vec2(8.0, 36.0));
                let corner = Rect::from_min_size(pos2(big_r.right() - 3.0, big_r.bottom() - 3.0), vec2(14.0, 14.0));
                if corner.expand(6.0).contains(p) {
                    return Target::BigSize;
                }
                if left.expand2(vec2(9.0, 8.0)).contains(p) || right.expand2(vec2(9.0, 8.0)).contains(p) {
                    return Target::BigWidth;
                }
                if big_r.contains(p) {
                    return Target::BigMove;
                }
            }
            Target::None
        };
        let hover = if panel_open_blocks(panel_open, p.pos) { Target::None } else { p.pos.map(hit_target).unwrap_or(Target::None) };
        if hover != self.hover {
            self.hover = hover;
            self.hover_since = time;
        }

        // Pressing.
        if p.pressed && !panel_open_blocks(panel_open, p.pos) {
            self.active = hover;
            let pos = p.pos.unwrap_or_default();
            match hover {
                Target::Bar => {
                    if let (Some(tr), Some(b)) = (&self.track, cl.bar) {
                        let to = (((pos.x - b.left()) / b.width()).clamp(0.0, 1.0) as f64) * self.length();
                        self.seeking = Some((str_of(tr, "key").to_string(), to));
                        self.set_over_ui(Hold::Yes);
                    }
                }
                Target::Card => {
                    self.drag = Some(Drag { dx: pos.x - cl.card.left(), dy: pos.y - cl.card.top(), x0: pos.x, y0: pos.y, moved: false });
                    self.set_over_ui(Hold::Yes);
                }
                Target::CardSize => {
                    self.size_drag = Some(SizeDrag { x0: pos.x, w0: cl.card.width(), s0: self.sf("cardScale", 1.0) as f32, scale: self.sf("cardScale", 1.0) as f32 });
                }
                Target::BigMove | Target::BigWidth | Target::BigSize => {
                    let start = big_placed;
                    self.big_drag = Some(BigDrag { kind: hover, start: self.big_layout(), live: start, r: big_r, p0: pos });
                }
                _ => {}
            }
        }
        if p.secondary_pressed && card_live && !editing && p.over(cl.card) {
            core::send("peek-now", Value::Null);
        }

        // Moving.
        if let Some(pos) = p.pos {
            if let Some((k, _)) = self.seeking.clone() {
                if let (Some(tr), Some(b)) = (&self.track, cl.bar) {
                    if str_of(tr, "key") == k {
                        let to = (((pos.x - b.left()) / b.width()).clamp(0.0, 1.0) as f64) * self.length();
                        self.seeking = Some((k, to));
                    }
                }
            }
            if let Some(d) = &mut self.drag {
                if !d.moved && (pos.x - d.x0).hypot(pos.y - d.y0) >= 4.0 {
                    d.moved = true;
                }
                if d.moved {
                    let (mut x, mut y) = (pos.x - d.dx, pos.y - d.dy);
                    let (mut gx, mut gy) = (None, None);
                    if !alt {
                        let height = cl.card.height() - if editing { cl.hint.map(|h| h.height()).unwrap_or(0.0) + 8.0 } else { 0.0 };
                        (x, gx) = snap_axis(x, cl.card.width(), screen.width());
                        (y, gy) = snap_axis(y, height, screen.height());
                    }
                    self.card_at = pos2(
                        x.min((screen.width() - cl.card.width()).max(0.0)).max(0.0),
                        y.min((screen.height() - 120.0).max(0.0)).max(0.0),
                    );
                    self.guide_x = gx;
                    self.guide_y = gy;
                }
            }
            if let Some(d) = &mut self.size_drag {
                d.scale = ((d.s0 * (d.w0 + pos.x - d.x0) / d.w0).clamp(0.6, 2.0) * 100.0).round() / 100.0;
            }
            if let Some(d) = &mut self.big_drag {
                let r = d.r;
                match d.kind {
                    Target::BigMove => {
                        let (mut left, mut top) = (r.left() + pos.x - d.p0.x, r.top() + pos.y - d.p0.y);
                        let (mut gx, mut gy) = (None, None);
                        if !alt {
                            (left, gx) = snap_axis(left, r.width(), screen.width());
                            (top, gy) = snap_axis(top, r.height(), screen.height());
                        }
                        d.live.x = (left + r.width() / 2.0) / screen.width();
                        d.live.y = (top + r.height() / 2.0) / screen.height();
                        self.guide_x = gx;
                        self.guide_y = gy;
                    }
                    Target::BigWidth => {
                        d.live.w = (((pos.x - r.center().x).abs() * 2.0 / screen.width()).clamp(0.25, 0.98) * 100.0).round() / 100.0;
                    }
                    _ => {
                        let from = (d.p0.y - r.center().y).abs().max(10.0);
                        d.live.size = ((d.start.size * ((pos.y - r.center().y).abs() / from)).clamp(0.4, 2.5) * 100.0).round() / 100.0;
                    }
                }
            }
        }

        // Letting go.
        if p.released || (!p.down && self.active != Target::None) {
            let was = self.active;
            self.active = Target::None;
            if let Some((key, to)) = self.seeking.take() {
                if self.track.as_ref().map(|t| str_of(t, "key")) == Some(key.as_str()) {
                    self.send_control("seek", Some((to * 1000.0).round() / 1000.0));
                    self.anchor_pos = to;
                    self.anchor_ms = now_ms();
                    self.seek_hold = Some((key, to, now_ms() + 2500.0));
                }
            }
            if let Some(d) = self.drag.take() {
                self.guide_x = None;
                self.guide_y = None;
                if d.moved {
                    core::send("card-moved", json!({ "x": self.card_at.x, "y": self.card_at.y }));
                    // Keep showing it where it was dropped until the saved place comes back.
                    let mut m = self.sv("card").clone();
                    m["x"] = json!(self.card_at.x.round());
                    m["y"] = json!(self.card_at.y.round());
                    self.s.insert("card".into(), m);
                }
            }
            if let Some(d) = self.size_drag.take() {
                if d.scale != d.s0 {
                    core::send("set", json!({ "key": "cardScale", "value": d.scale }));
                    self.s.insert("cardScale".into(), json!(d.scale));
                }
                core::send("card-moved", json!({ "x": self.card_at.x, "y": self.card_at.y }));
            }
            if let Some(d) = self.big_drag.take() {
                self.guide_x = None;
                self.guide_y = None;
                let (_, at) = self.big_rect(screen, d.live);
                if d.kind == Target::BigMove && (at.x != d.start.x || at.y != d.start.y) {
                    core::send("set", json!({ "key": "bigPos", "value": { "x": at.x, "y": at.y } }));
                    self.s.insert("bigPos".into(), json!({ "x": at.x, "y": at.y }));
                }
                if d.live.w != d.start.w {
                    core::send("set", json!({ "key": "bigWidth", "value": d.live.w }));
                    self.s.insert("bigWidth".into(), json!(d.live.w));
                }
                if d.live.size != d.start.size {
                    core::send("set", json!({ "key": "bigSize", "value": d.live.size }));
                    self.s.insert("bigSize".into(), json!(d.live.size));
                }
            }
            // A click: let go over what was pressed.
            if was == hover && p.released {
                match was {
                    Target::Prev => self.send_control("prev", None),
                    Target::Next => self.send_control("next", None),
                    Target::Play => {
                        self.send_control("toggle", None);
                        let paused = self.shown_paused();
                        self.play_override = Some((!paused, time + 1.5)); // show the change now
                    }
                    Target::Gear => {
                        let c = cl.card;
                        core::send("open-settings", json!({ "x": c.left().round(), "y": c.top().round(), "width": c.width().round() }));
                    }
                    Target::Done => core::send("edit-done", Value::Null),
                    _ => {}
                }
            }
        }
        if let Some((_, until)) = self.play_override {
            if time > until {
                self.play_override = None;
            } else {
                wake = wake.min(until - time + 0.01);
            }
        }

        // Holding on to the mouse: on the card, or busy dragging and seeking.
        let on = if !editing && card_live && p.over(cl.card) { Hold::Yes } else { Hold::No };
        self.set_over_ui(on);
        if !show_card && self.seeking.is_none() && self.drag.is_none() {
            self.set_over_ui(Hold::No);
        }

        // The cursor.
        let dragging = self.drag.as_ref().map(|d| d.moved).unwrap_or(false) || self.big_drag.as_ref().map(|d| d.kind == Target::BigMove).unwrap_or(false);
        let cursor_for = |t: Target| match t {
            Target::Prev | Target::Play | Target::Next | Target::Gear | Target::Bar | Target::Done => CursorIcon::PointingHand,
            Target::Card => if editing { CursorIcon::Move } else { CursorIcon::Grab },
            Target::CardSize | Target::BigSize => CursorIcon::ResizeNwSe,
            Target::BigWidth => CursorIcon::ResizeHorizontal,
            Target::BigMove => CursorIcon::Grab,
            Target::None => CursorIcon::Default,
        };
        let cursor = if dragging { CursorIcon::Grabbing } else if self.active != Target::None { cursor_for(self.active) } else { cursor_for(hover) };
        if p.pos.is_some() && !panel_open_blocks(panel_open, p.pos) {
            ctx.set_cursor_icon(cursor);
        }

        /* --- Drawing --- */
        let painter = ctx.layer_painter(LayerId::new(Order::Background, Id::new("overlay"))).with_clip_rect(screen);
        let mut grid = Group::default();
        let mut big = Group::default();
        let mut card = Group::default();
        let mut top = Group::default();

        // Grid and guides while arranging or dragging the card.
        let grid_t = tween(ctx, "grid", if editing || dragging { 1.0 } else { 0.0 }, 0.25);
        if grid_t > 0.0 {
            let now_r = cl.now;
            let gc = pos2(cl.card.center().x, cl.card.top() + now_r.height() / 2.0 + 40.0);
            grid.shape(grid_mesh(screen, gc, grid_t));
        }
        for (g, vertical) in [(self.guide_x, true), (self.guide_y, false)] {
            if let Some(at) = g {
                let r = if vertical { rect(at.round(), screen.top(), 1.0, screen.height()) } else { rect(screen.left(), at.round(), screen.width(), 1.0) };
                grid.shape(Shape::Rect(Shadow { offset: [0, 0], blur: 6, spread: 0, color: accent }.as_shape(r, 0.0)));
                grid.shape(Shape::rect_filled(r, 0.0, accent));
            }
        }

        // The big lyrics.
        let big_fade = tween(ctx, "big-fade", if fade_big { 0.0 } else { 1.0 }, 0.8);
        if editing && !big_off {
            big.shape(Shape::rect_filled(big_r, 10.0, rgba(0, 0, 0, 0.14)));
            for s in draw::dashed_rect(big_r.expand(7.0), 16.0, 2.0, accent, 6.0, 4.0) {
                big.shape(s);
            }
            if !self.big.iter().any(|l| l.leaving.is_none()) {
                let (font, _) = self.big_font(screen, big_l.size);
                let g = line(ctx, "Big lyrics show here", FontId::new(font.size, fonts::family(&self.font(), 800, false)), accent);
                let x = match self.ss("bigAlign", "center").as_str() {
                    "left" => big_r.left(),
                    "right" => big_r.right() - g.size().x,
                    _ => big_r.center().x - g.size().x / 2.0,
                };
                let at = pos2(x, big_r.center().y - g.size().y / 2.0);
                big.text_xf(&g, at, rgba(0, 0, 0, 0.75), TSTransform::IDENTITY, vec2(0.0, 2.0), 10.0);
                big.text(&g, at, alpha(accent, 0.85));
            }
        }
        if !(editing && big_off) {
            let lines_o = if editing && fade_big { 0.7 } else { big_fade };
            if lines_o > 0.0 || self.big.iter().any(|l| l.leaving.is_some()) {
                animating |= self.paint_big(ctx, &mut big, screen, big_r, big_l.size, time, accent, lines_o, fade_big && !editing);
            }
        }
        if editing && !big_off {
            let hv = |t: Target| if self.hover == t || self.active == t { 1.25 } else { 1.0 };
            let left = Rect::from_min_size(pos2(big_r.left() - 10.0, big_r.center().y - 18.0), vec2(8.0, 36.0));
            let right = Rect::from_min_size(pos2(big_r.right() + 2.0, big_r.center().y - 18.0), vec2(8.0, 36.0));
            let corner = Rect::from_min_size(pos2(big_r.right() - 3.0, big_r.bottom() - 3.0), vec2(14.0, 14.0));
            for (r, k) in [(left, Target::BigWidth), (right, Target::BigWidth)] {
                let r = Rect::from_center_size(r.center(), r.size() * hv(k));
                big.shape(Shape::rect_filled(r.expand(2.0), 6.0, rgba(0, 0, 0, 0.5)));
                big.shape(Shape::rect_filled(r, 4.0, accent));
            }
            let r = corner.width() / 2.0 * hv(Target::BigSize);
            big.shape(Shape::circle_filled(corner.center(), r + 2.0, rgba(0, 0, 0, 0.5)));
            big.shape(Shape::circle_filled(corner.center(), r, accent));
        }

        // The card.
        if card_o > 0.0 {
            animating |= self.paint_card(ctx, &mut card, &cl, time, accent, card_o * 1.0, (1.0 - ease(show_t)) * -10.0, panel_open, bar_off, &can_ctl);
        }
        if show_t > 0.0 && show_t < 1.0 || (peek_t > 0.0 && peek_t < 1.0) {
            animating = true;
        }

        // Toast.
        if let Some((msg, t0)) = self.toast.clone() {
            let on = time - t0 < 2.4;
            let k = tween(ctx, "toast", if on { 1.0 } else { 0.0 }, 0.25);
            if on {
                wake = wake.min(t0 + 2.4 - time + 0.01);
            } else if k <= 0.0 {
                self.toast = None;
            }
            if k > 0.0 {
                let g = text(ctx, &msg, fonts::inter(600, 14.0), TEXT, Txt { wrap: screen.width() * 0.8 - 32.0, ..Default::default() });
                let size = g.size() + vec2(32.0, 18.0);
                let r = Rect::from_min_size(pos2(screen.center().x - size.x / 2.0, 24.0 - 8.0 * (1.0 - k)), size);
                top.shape(Shape::rect_filled(r, 8.0, rgba(14, 14, 18, 0.84 * k)));
                top.text(&g, r.min + vec2(16.0, 9.0), alpha(TEXT, k));
            }
        }

        // Startup animation.
        if let Some(t0) = self.intro {
            let e = (time - t0) as f32;
            if e >= INTRO {
                self.intro = None;
            } else {
                if e >= 0.0 {
                    paint_intro(ctx, &mut top, screen, e, accent);
                }
                animating = true;
            }
        }

        // Tooltips, like the page's title attributes.
        let tip = match hover {
            Target::Prev => Some("Previous".to_string()),
            Target::Play => Some(if self.shown_paused() { "Play" } else { "Pause" }.to_string()),
            Target::Next => Some("Next".into()),
            Target::Gear => Some("Settings".into()),
            Target::Bar => Some("Drag to jump".into()),
            Target::CardSize => Some("Drag to resize the card".into()),
            Target::BigWidth => Some("Drag to change the width".into()),
            Target::BigSize => Some("Drag to change the text size".into()),
            _ => None,
        };
        if let (Some(tip), Some(pos)) = (tip, p.pos) {
            if self.active == Target::None && !p.down && !panel_open {
                let wait_t = self.hover_since + 0.7 - time;
                if wait_t <= 0.0 {
                    let g = line(ctx, &tip, fonts::inter(400, 12.0), TEXT);
                    let size = g.size() + vec2(16.0, 8.0);
                    let mut at = pos + vec2(0.0, 22.0);
                    at.x = at.x.min(screen.right() - size.x - 4.0);
                    let r = Rect::from_min_size(at, size);
                    top.shape(Shape::Rect(egui::epaint::RectShape::new(r, 4.0, Color32::from_rgb(43, 43, 43), Stroke::new(1.0, rgba(255, 255, 255, 0.12)), StrokeKind::Inside)));
                    top.text(&g, r.min + vec2(8.0, 4.0), TEXT);
                } else {
                    wake = wake.min(wait_t + 0.01);
                }
            }
        }

        let uv = draw::uv_scale(ctx);
        grid.paint(&painter, uv);
        big.paint(&painter, uv);
        card.paint(&painter, uv);
        top.paint(&painter, uv);

        // When to draw again.
        if card_o > 0.0 && show_card && self.track.is_some() && self.playing && self.seeking.is_none() {
            let length = self.length();
            if let Some(b) = cl.bar {
                if length > 0.0 {
                    let px = b.width() * ctx.pixels_per_point();
                    wake = wake.min(length / px as f64);
                }
            }
            for r in &cl.rows {
                if let (RowKind::Cur { .. }, Some(g)) = (&r.kind, &r.galley) {
                    let idx = self.line_index(lt);
                    if let (Some(c), n) = (self.line_at(idx), self.line_at(idx + 1)) {
                        let dur = n.map(|n| n.t).unwrap_or(c.t + 4.0) - c.t;
                        let px = g.size().x.max(r.width) * ctx.pixels_per_point();
                        wake = wake.min(dur.max(0.1) / px as f64);
                    }
                }
                if matches!(r.kind, RowKind::Dots { .. }) {
                    wake = wake.min(0.25);
                }
            }
        }
        if animating || self.drag.is_some() || self.big_drag.is_some() || self.size_drag.is_some() || self.seeking.is_some() {
            wake = wake.min(1.0 / 60.0);
        }
        self.wake = wake.is_finite().then_some(wake.max(1.0 / 60.0));
    }

    fn shown_paused(&self) -> bool {
        if let Some((paused, _)) = self.play_override {
            return paused;
        }
        self.track.as_ref().map(|t| str_of(t, "status") != "Playing").unwrap_or(true)
    }

    pub fn next_wake(&self) -> Option<Duration> {
        self.wake.map(Duration::from_secs_f64)
    }

    /* ---------- Painting the card ---------- */
    #[allow(clippy::too_many_arguments)]
    fn paint_card(
        &self,
        ctx: &Context,
        g: &mut Group,
        cl: &CardL,
        time: f64,
        accent: Color32,
        o: f32,
        dy: f32,
        panel_open: bool,
        bar_off: bool,
        can_ctl: &dyn Fn(&str) -> bool,
    ) -> bool {
        let mut animating = false;
        let a = self.sf("cardOpacity", 0.84) as f32;
        let clear = a < 0.45;
        let shift = TSTransform::from_translation(vec2(0.0, dy));
        let mut shapes: Vec<Shape> = Vec::new();
        let mut texts: Vec<TextDraw> = Vec::new();
        let z = cl.z;
        let cz = cl.cz;
        let hv = |t: Target| self.hover == t && self.active == Target::None || self.active == t && self.hover == t;

        // Pop when a new song starts: the song part grows in.
        let (pop_s, pop_o) = match self.pop_at {
            Some(t0) if time - t0 < 0.5 => {
                animating = true;
                let k = bezier(0.2, 0.9, 0.3, 1.2, ((time - t0) / 0.5) as f32);
                (0.92 + 0.08 * k, k.clamp(0.0, 1.0))
            }
            _ => (1.0, 1.0),
        };
        let now_xf = shift * about(cl.now.center(), pop_s, Vec2::ZERO);
        let no = o * pop_o;

        let push = |shapes: &mut Vec<Shape>, mut s: Shape, xf: TSTransform| {
            s.transform(xf);
            shapes.push(s);
        };
        let push_text = |texts: &mut Vec<TextDraw>, gl: &Arc<Galley>, pos: Pos2, c: Color32, xf: TSTransform, op: f32| {
            if clear {
                texts.push(TextDraw { galley: gl.clone(), pos, color: rgba(0, 0, 0, 0.9 * op), xf, offset: Vec2::ZERO, blur: 1.0 });
                texts.push(TextDraw { galley: gl.clone(), pos, color: rgba(0, 0, 0, 0.85 * op), xf, offset: vec2(0.0, 1.0), blur: 3.0 });
            }
            texts.push(TextDraw { galley: gl.clone(), pos, color: alpha(c, op), xf, offset: Vec2::ZERO, blur: 0.0 });
        };

        // The song part.
        let r10 = 10.0 * z;
        push(&mut shapes, Shape::Rect(Shadow { offset: [0, 6], blur: 24, spread: 0, color: rgba(0, 0, 0, a * 0.42 * no) }.as_shape(cl.now, r10)), now_xf);
        push(
            &mut shapes,
            Shape::Rect(egui::epaint::RectShape::new(cl.now, r10, rgba(14, 14, 18, a * no), Stroke::new(1.0, rgba(255, 255, 255, a * 0.085 * no)), StrokeKind::Inside)),
            now_xf,
        );
        if let Some(cr) = cl.cover {
            push(&mut shapes, Shape::rect_filled(cr, 6.0 * z, alpha(Color32::from_rgb(0x24, 0x24, 0x2b), no)), now_xf);
            if let (Some((key, tex)), Some(tr)) = (&self.cover, &self.track) {
                if str_of(tr, "key") == key {
                    let s = egui::epaint::RectShape::filled(cr, 6.0 * z, alpha(Color32::WHITE, no)).with_texture(tex.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
                    push(&mut shapes, Shape::Rect(s), now_xf);
                }
            }
        }
        if let Some((gl, pos)) = &cl.title {
            push_text(&mut texts, gl, *pos, TEXT, now_xf, no);
        }
        if let Some((gl, pos)) = &cl.artist {
            push_text(&mut texts, gl, *pos, dim(), now_xf, no);
        }
        if let Some(b) = cl.bar {
            let on = !bar_off && (hv(Target::Bar) || self.seeking.is_some());
            let k = tween(ctx, "bar-hover", if on { 1.0 } else { 0.0 }, 0.12);
            let th = if bar_off { 4.0 } else { 4.0 + 2.0 * k } * z;
            let track = Rect::from_center_size(pos2(b.center().x, b.center().y), vec2(b.width(), th));
            push(&mut shapes, Shape::rect_filled(track, 2.0 * z, rgba(255, 255, 255, 0.14 * no)), now_xf);
            let length = self.length();
            let pos = if length > 0.0 { (self.song_time() / length).clamp(0.0, 1.0) as f32 } else { 0.0 };
            if pos > 0.0 {
                let fill = Rect::from_min_size(track.min, vec2(track.width() * pos, th));
                push(&mut shapes, Shape::rect_filled(fill, 2.0 * z, alpha(accent, no)), now_xf);
            }
            if !bar_off && k > 0.0 {
                let c = pos2(track.left() + track.width() * pos, track.center().y);
                let rad = 5.5 * z * k;
                push(&mut shapes, Shape::Rect(Shadow { offset: [0, 1], blur: 4, spread: 0, color: rgba(0, 0, 0, 0.4 * no * k) }.as_shape(Rect::from_center_size(c, vec2(rad * 2.0, rad * 2.0)), rad)), now_xf);
                push(&mut shapes, Shape::circle_filled(c, rad, alpha(Color32::WHITE, no)), now_xf);
            }
        }
        if let Some((el, ep, le, lp)) = &cl.times {
            push_text(&mut texts, el, *ep, dim(), now_xf, no);
            push_text(&mut texts, le, *lp, dim(), now_xf, no);
        }
        let paused = self.shown_paused();
        for (r, t, k) in [(cl.prev, Target::Prev, "prev"), (cl.play, Target::Play, "toggle"), (cl.next, Target::Next, "next")] {
            let Some(r) = r else { continue };
            let enabled = can_ctl(k);
            let h = tween(ctx, &format!("ctl-{k}"), if enabled && hv(t) { 1.0 } else { 0.0 }, 0.15);
            let base = if t == Target::Play { 0.9 } else { 0.7 };
            let op = if enabled { base + (1.0 - base) * h } else { 0.22 };
            let press = if self.active == t && self.hover == t { 0.9 } else { 1.0 };
            let bxf = now_xf * about(r.center(), press, Vec2::ZERO);
            if h > 0.0 {
                push(&mut shapes, Shape::rect_filled(r, 6.0 * cz, rgba(255, 255, 255, 0.1 * h * no)), bxf);
            }
            let content = r.shrink(3.0 * cz);
            let side = content.width().min(content.height());
            let ir = Rect::from_center_size(content.center(), vec2(side, side));
            let c = alpha(TEXT, op * no);
            let icon = match t {
                Target::Prev => draw::icon_prev(ir, c),
                Target::Next => draw::icon_next(ir, c),
                _ if paused => draw::icon_play(ir, c),
                _ => draw::icon_pause(ir, c),
            };
            if clear {
                for mut s in icon.clone() {
                    s.translate(vec2(0.0, 1.0));
                    if let Shape::Path(ref mut pth) = s {
                        pth.fill = rgba(0, 0, 0, 0.6 * op * no);
                    }
                    push(&mut shapes, s, bxf);
                }
            }
            for s in icon {
                push(&mut shapes, s, bxf);
            }
        }
        // Gear: brighter on hover, turned a little, coloured while the panel is open.
        {
            let h = tween(ctx, "gear-hover", if hv(Target::Gear) || panel_open { 1.0 } else { 0.0 }, 0.15);
            let turn = tween(ctx, "gear-turn", if hv(Target::Gear) { 1.0 } else { 0.0 }, 0.3);
            let op = 0.55 + 0.45 * h;
            if h > 0.0 {
                push(&mut shapes, Shape::rect_filled(cl.gear, 6.0 * cz, rgba(255, 255, 255, 0.1 * h * no)), now_xf);
            }
            let col = if panel_open { accent } else { mix(TEXT, h, dim()) };
            let ir = cl.gear.shrink(4.0 * cz);
            for s in draw::icon_gear(ir, alpha(col, op * no), ease(turn) * std::f32::consts::FRAC_PI_4) {
                push(&mut shapes, s, now_xf);
            }
        }
        // Equaliser bars, moving while playing.
        {
            let delays = [0.0, -0.35, -0.7];
            for (i, d) in delays.iter().enumerate() {
                let ph = if self.playing { ((time - d) % 1.0) as f32 } else { ((self.idle_since / 1000.0 - d) % 1.0) as f32 };
                let [sy] = keyframes(&[(0.0, [0.34]), (0.25, [0.67]), (0.5, [1.0]), (0.75, [0.67]), (1.0, [0.34])], ph);
                let h = 12.0 * z * sy;
                let x = cl.eq.x - (3.0 - i as f32) * 3.0 * z - (2.0 - i as f32) * 2.0 * z;
                let r = rect(x, cl.eq.y - h, 3.0 * z, h);
                let op = if self.playing { 1.0 } else { 0.5 };
                push(&mut shapes, Shape::rect_filled(r, 1.0 * z, alpha(accent, op * no)), now_xf);
            }
            if self.playing {
                animating = true;
            }
        }

        // The card's lyrics.
        if let Some(pr) = cl.panel {
            push(
                &mut shapes,
                Shape::Rect(egui::epaint::RectShape::new(pr, r10, rgba(14, 14, 18, a * 0.83 * o), Stroke::new(1.0, rgba(255, 255, 255, 0.05 * o)), StrokeKind::Inside)),
                shift,
            );
            for r in &cl.rows {
                match &r.kind {
                    RowKind::Dim | RowKind::Note => {
                        if let Some(gl) = &r.galley {
                            push_text(&mut texts, gl, r.pos, dim(), shift, o);
                        }
                    }
                    RowKind::Cur { progress } => {
                        if let Some(gl) = &r.galley {
                            push_text(&mut texts, gl, r.pos, accent, shift, o);
                            let y = r.pos.y + gl.size().y + 4.0 * z - 2.0 * z;
                            let full = rect(r.pos.x, y, r.width, 2.0 * z);
                            if *progress > 0.0 {
                                let part = rect(r.pos.x, y, r.width * progress, 2.0 * z);
                                push(&mut shapes, Shape::rect_filled(part, 1.0 * z, alpha(accent, 0.75 * o)), shift);
                            }
                            let _ = full;
                        }
                    }
                    RowKind::Dots { lit } => {
                        for i in 0..3 {
                            let c = pos2(r.pos.x + 3.5 * z + i as f32 * 13.0 * z, r.pos.y + 9.5 * z);
                            let col = if i < *lit { accent } else { rgba(255, 255, 255, 0.22) };
                            push(&mut shapes, Shape::circle_filled(c, 3.5 * z, alpha(col, o)), shift);
                        }
                    }
                }
            }
        }

        // Arranging: the dashed outline, the size handle and the hint with its Done button.
        if self.editing {
            for s in draw::dashed_rect(cl.boxr.expand(5.0), 14.0, 2.0, alpha(accent, o), 6.0, 4.0) {
                push(&mut shapes, s, shift);
            }
            let k = if self.hover == Target::CardSize || self.active == Target::CardSize { 1.25 } else { 1.0 };
            let rad = cl.size_handle.width() / 2.0 * k;
            push(&mut shapes, Shape::circle_filled(cl.size_handle.center(), rad + 2.0, rgba(0, 0, 0, 0.5 * o)), shift);
            push(&mut shapes, Shape::circle_filled(cl.size_handle.center(), rad, alpha(accent, o)), shift);
            if let Some(hr) = cl.hint {
                push(&mut shapes, Shape::rect_filled(hr, 8.0, rgba(14, 14, 18, 0.9 * o)), shift);
                if let Some((a1, p1, b1, p2)) = &cl.hint_text {
                    texts.push(TextDraw { galley: a1.clone(), pos: *p1, color: alpha(TEXT, o), xf: shift, offset: Vec2::ZERO, blur: 0.0 });
                    texts.push(TextDraw { galley: b1.clone(), pos: *p2, color: alpha(dim(), o), xf: shift, offset: Vec2::ZERO, blur: 0.0 });
                }
                if let Some((br, dg)) = &cl.done {
                    push(&mut shapes, Shape::rect_filled(*br, 6.0, alpha(accent, o)), shift);
                    texts.push(TextDraw { galley: dg.clone(), pos: br.center() - dg.size() / 2.0, color: alpha(Color32::from_rgb(0x11, 0x11, 0x11), o), xf: shift, offset: Vec2::ZERO, blur: 0.0 });
                }
            }
        }
        g.shapes.extend(shapes);
        g.texts.extend(texts);
        animating
    }

    /* ---------- Painting the big lyrics ---------- */
    #[allow(clippy::too_many_arguments)]
    fn paint_big(&self, ctx: &Context, g: &mut Group, screen: Rect, br: Rect, size: f32, time: f64, accent: Color32, lines_o: f32, paused: bool) -> bool {
        let mut animating = false;
        let (font, spacing) = self.big_font(screen, size);
        let fs = font.size;
        let lh = 1.18 * fs;
        let look = self.ss("look", "depth");
        let motion = self.ss("motion", "none");
        let entrance = self.ss("entrance", "rise");
        let align = self.ss("bigAlign", "center");
        let moving_colour = matches!(self.ss("colourMode", "accent").as_str(), "rainbow" | "fade");
        let space_w = {
            let a = line(ctx, "x x", font.clone(), TEXT).size().x;
            let b = line(ctx, "xx", font.clone(), TEXT).size().x;
            (a - b).max(fs * 0.2)
        };
        for bl in &self.big {
            let galleys: Vec<Arc<Galley>> = bl.words.iter().map(|w| text(ctx, w, font.clone(), TEXT, Txt { spacing, ..Default::default() })).collect();
            let widths: Vec<f32> = galleys.iter().map(|g| g.size().x).collect();
            let rows = balance(&widths, space_w, br.width());
            let block_h = rows.len() as f32 * lh;
            let top = br.center().y - block_h / 2.0;

            // The line leaving drifts up and fades.
            let (lxf, lo) = match bl.leaving {
                Some(t0) => {
                    animating = true;
                    let k = ((time - t0) / 0.8) as f32;
                    let [ty, s, op] = keyframes(&[(0.0, [-0.5, 1.0, 1.0]), (0.3, [-0.32, 0.984, 0.8]), (0.5, [-0.8, 0.96, 0.5]), (0.7, [-1.28, 0.936, 0.2]), (1.0, [-1.6, 0.92, 0.0])], k);
                    (about(br.center(), s, vec2(0.0, (ty + 0.5) * block_h)), op)
                }
                None => (TSTransform::IDENTITY, lines_o),
            };
            if lo <= 0.0 {
                continue;
            }
            let age = time - bl.born;
            for (ri, row) in rows.iter().enumerate() {
                let row_w: f32 = row.iter().map(|&i| widths[i]).sum::<f32>() + space_w * (row.len().saturating_sub(1)) as f32;
                let mut x = match align.as_str() {
                    "left" => br.left(),
                    "right" => br.right() - row_w,
                    _ => br.center().x - row_w / 2.0,
                };
                let y = top + ri as f32 * lh;
                for &i in row {
                    let gl = &galleys[i];
                    let w = widths[i];
                    let centre = pos2(x + w / 2.0, y + lh / 2.0);
                    let pos = pos2(x, y + (lh - gl.size().y) / 2.0);
                    x += w + space_w;

                    // Coming in.
                    let at = (age - bl.delays[i] as f64) as f32;
                    if at < 0.0 {
                        animating = true;
                        continue;
                    }
                    let k = at / 0.42;
                    if k < 1.0 {
                        animating = true;
                    }
                    let (dx, dy, s, wo) = match entrance.as_str() {
                        "pop" => {
                            let [s, o] = keyframes(&[(0.0, [0.3, 0.0]), (0.55, [1.14, 1.0]), (1.0, [1.0, 1.0])], k);
                            (0.0, 0.0, s, o)
                        }
                        "slide" => {
                            let [x, o] = keyframes(&[(0.0, [-0.7, 0.0]), (0.3, [-0.2, 0.75]), (1.0, [0.0, 1.0])], k);
                            (x * fs, 0.0, 1.0, o)
                        }
                        "drop" => {
                            let [y, s, o] = keyframes(&[(0.0, [-0.8, 1.08, 0.0]), (0.6, [0.07, 1.0, 1.0]), (1.0, [0.0, 1.0, 1.0])], k);
                            (0.0, y * fs, s, o)
                        }
                        "fade" => (0.0, 0.0, 1.0, k.min(1.0)),
                        _ => {
                            let [y, s, o] = keyframes(&[(0.0, [0.4, 0.88, 0.0]), (0.2, [0.16, 0.952, 0.6]), (0.45, [0.04, 0.988, 0.9]), (1.0, [0.0, 1.0, 1.0])], k);
                            (0.0, y * fs, s, o)
                        }
                    };
                    let wxf = about(centre, s, vec2(dx, dy));

                    // Moving.
                    let mt = if paused { 0.0 } else { age as f32 };
                    let (mdy, ms, mo) = match motion.as_str() {
                        "wave" => {
                            let ph = ((mt + i as f32 * 8.0 / 60.0) / 1.8).rem_euclid(1.0);
                            let [y] = keyframes(&[(0.0, [-0.12]), (0.125, [-0.085]), (0.25, [0.0]), (0.375, [0.085]), (0.5, [0.12]), (0.625, [0.085]), (0.75, [0.0]), (0.875, [-0.085]), (1.0, [-0.12])], ph);
                            (y * fs, 1.0, 1.0)
                        }
                        "bounce" => {
                            let local = mt - i as f32 * 6.0 / 60.0;
                            if local < 0.0 {
                                (0.0, 1.0, 1.0)
                            } else {
                                let ph = (local / 1.6).rem_euclid(1.0);
                                let [y, s] = keyframes(&[(0.0, [0.0, 1.0]), (0.06, [-0.17, 1.03]), (0.12, [-0.22, 1.04]), (0.2, [-0.12, 1.02]), (0.3, [0.0, 1.0]), (1.0, [0.0, 1.0])], ph);
                                (y * fs, s, 1.0)
                            }
                        }
                        "pulse" => {
                            let ph = (mt / 1.2).rem_euclid(1.0);
                            let [s] = keyframes(&[(0.0, [1.0]), (0.25, [1.035]), (0.5, [1.07]), (0.75, [1.035]), (1.0, [1.0])], ph);
                            (0.0, s, 1.0)
                        }
                        "float" => {
                            let ph = ((mt + i as f32 * 9.0 / 60.0) / 3.0).rem_euclid(1.0);
                            let [y] = keyframes(&[(0.0, [0.0]), (0.25, [-0.07]), (0.5, [-0.1]), (0.75, [-0.04]), (1.0, [0.0])], ph);
                            (y * fs, 1.0, 1.0)
                        }
                        "flicker" => {
                            let ph = ((mt + i as f32 * 37.0 / 60.0) / 3.2).rem_euclid(1.0);
                            let o = match ph {
                                p if p < 0.41 => 1.0,
                                p if p < 0.43 => 0.35,
                                p if p < 0.45 => 1.0,
                                p if p < 0.47 => 0.55,
                                p if p < 0.78 => 1.0,
                                p if p < 0.80 => 0.7,
                                _ => 1.0,
                            };
                            (0.0, 1.0, o)
                        }
                        _ => (0.0, 1.0, 1.0),
                    };
                    if motion != "none" && !paused {
                        animating = true;
                    }
                    let ixf = about(centre, ms, vec2(0.0, mdy));
                    let xf = lxf * wxf * ixf;
                    let op = lo * wo * mo;
                    if op <= 0.0 {
                        continue;
                    }
                    let c = self.word_colour(i, time, accent);
                    paint_word(g, gl, pos, xf, c, op, &look, fs, !moving_colour);
                }
            }
            if moving_colour && lo > 0.0 && !paused {
                animating = true;
            }
        }
        animating
    }
}

/// One word in the chosen look: its shadows, then the word.
#[allow(clippy::too_many_arguments)]
/// `steady`: the colour stays the same from frame to frame, so the whole stack can be one picture.
fn paint_word(g: &mut Group, gl: &Arc<Galley>, pos: Pos2, xf: TSTransform, c: Color32, op: f32, look: &str, fs: f32, steady: bool) {
    // Each look is up to three pictures: shadows in fixed colours; copies in the word colour, kept in grey and
    // tinted when drawn (so changing colours reuse the picture); and for some looks a fixed wash over the face.
    // mix(c, p, x) = c·p + x·(1 − p), so every word-coloured copy is a grey level times c.
    let l = |color: Color32, x: f32, y: f32, blur: f32| draw::Layer { color, offset: vec2(x, y), blur };
    let grey = |k: f32| Color32::from_gray((k * 255.0).round() as u8);
    let white = Color32::WHITE;
    let (fixed, tinted, top) = match look {
        "glow" => (
            vec![l(rgba(0, 0, 0, 0.5), 0.0, 0.03 * fs, 0.08 * fs)],
            vec![l(rgba(255, 255, 255, 0.75), 0.0, 0.0, 0.2 * fs), l(white, 0.0, 0.0, 0.09 * fs), l(white, 0.0, 0.0, (0.03 * fs).max(1.0)), l(white, 0.0, 0.0, 0.0)],
            // The face is mix(c, 0.22, white): the word colour with white at 78% over it.
            vec![l(rgba(255, 255, 255, 0.78), 0.0, 0.0, 0.0)],
        ),
        "outline" => {
            let r = 0.045 * fs;
            let mut ring: Vec<draw::Layer> = (0..16)
                .map(|k| {
                    let ang = k as f32 / 16.0 * std::f32::consts::TAU;
                    l(white, ang.cos() * r, ang.sin() * r, 0.0)
                })
                .collect();
            ring.push(l(white, 0.0, 0.0, 0.0));
            (vec![l(rgba(0, 0, 0, 0.45), 0.0, 4.0, 14.0)], ring, vec![l(rgba(0x10, 0x10, 0x14, 0.78), 0.0, 0.0, 0.0)])
        }
        "plain" => (vec![l(rgba(0, 0, 0, 0.9), 0.0, 0.0, 2.0), l(rgba(0, 0, 0, 0.75), 0.0, 2.0, 10.0)], vec![l(white, 0.0, 0.0, 0.0)], vec![]),
        _ => {
            // Depth: deep = mix(c, 0.42, black), the back copy mix(deep, 0.7, black).
            let mut t = vec![l(grey(0.42 * 0.7), 6.0, 6.0, 0.0)];
            for k in (1..=5).rev() {
                t.push(l(grey(0.42), k as f32, k as f32, 0.0));
            }
            t.push(l(white, 0.0, 0.0, 0.0));
            (vec![l(rgba(0, 0, 0, 0.5), 10.0, 12.0, 18.0)], t, vec![])
        }
    };
    let fade = Color32::WHITE.gamma_multiply(op);
    if steady {
        let mut all = fixed;
        all.extend(tinted.into_iter().map(|t| draw::Layer { color: t.color * c, ..t }));
        all.extend(top);
        g.sprite(gl, pos, xf, all, fade);
        return;
    }
    g.sprite(gl, pos, xf, fixed, fade);
    g.sprite(gl, pos, xf, tinted, c.gamma_multiply(op));
    if !top.is_empty() {
        g.sprite(gl, pos, xf, top, fade);
    }
}

/// Words into rows no wider than `max`, as evenly long as they can be (CSS text-wrap: balance).
/// The startup animation: the app's badge pops in at the middle of the screen, its three lyric lines grow in one
/// after another, the name slides in beside it, then it all lifts away. `e` is seconds since it began.
fn paint_intro(ctx: &Context, g: &mut Group, screen: Rect, e: f32, accent: Color32) {
    let at = |a: f32, b: f32| ease(((e - a) / (b - a)).clamp(0.0, 1.0));
    let n = |x: f32| x / INTRO;
    let [o, lift] = keyframes(&[(0.0, [0.0, 0.0]), (n(0.25), [1.0, 0.0]), (n(2.05), [1.0, 0.0]), (n(INTRO), [0.0, -14.0])], e / INTRO);
    let scale = 0.92 + 0.08 * at(0.0, 0.35);
    if o <= 0.0 {
        return;
    }

    let title = line(ctx, "Lyrics Overlay", fonts::inter(800, 26.0), TEXT);
    let (pad, badge, gap) = (18.0, 64.0, 16.0);
    let text_w = title.size().x;
    let size = vec2(pad + badge + gap + text_w + pad + 4.0, pad * 2.0 + badge);
    let pill = Rect::from_center_size(screen.center() + vec2(0.0, -screen.height() * 0.06), size);
    let xf = about(pill.center(), scale, vec2(0.0, lift + 6.0 * (1.0 - at(0.0, 0.35))));

    let mut shapes = vec![
        Shape::Rect(Shadow { offset: [0, 8], blur: 32, spread: 0, color: rgba(0, 0, 0, 0.45 * o) }.as_shape(pill, 18.0)),
        Shape::rect_filled(pill, 18.0, rgba(14, 14, 18, 0.88 * o)),
    ];

    // The badge: pops past full size and settles, with a soft glow in the accent colour.
    let pop = keyframes(&[(0.0, [0.0]), (n(0.1), [0.0]), (n(0.38), [1.12]), (n(0.55), [1.0]), (1.0, [1.0])], e / INTRO)[0];
    if pop > 0.0 {
        let c = pos2(pill.left() + pad + badge / 2.0, pill.center().y);
        let r = Rect::from_center_size(c, vec2(badge, badge) * pop);
        shapes.push(Shape::Rect(Shadow { offset: [0, 0], blur: 28, spread: 0, color: alpha(accent, 0.55 * o * at(0.2, 0.6)) }.as_shape(r, 14.0 * pop)));
        shapes.push(Shape::rect_filled(r, 14.0 * pop, alpha(accent, o)));
        // The tray icon's three lines (on a 32-unit square), growing from the left one after another, then
        // breathing gently like a level meter while it holds.
        let u = r.width() / 32.0;
        for (i, (y0, x1)) in [(9.0, 24.0), (15.0, 21.0), (21.0, 16.0)].into_iter().enumerate() {
            let fi = i as f32;
            let grow = at(0.35 + 0.08 * fi, 0.7 + 0.08 * fi);
            let breathe = 1.0 - 0.14 * (0.5 - 0.5 * ((e - 0.9).max(0.0) * 5.5 + fi * 1.3).cos()) * at(0.8, 1.1);
            let w = (x1 - 8.0) * grow * breathe;
            if w > 0.0 {
                let bar = Rect::from_min_size(r.min + vec2(8.0 * u, y0 * u), vec2(w.max(4.0) * u, 4.0 * u));
                shapes.push(Shape::rect_filled(bar, 2.0 * u, alpha(Color32::WHITE, o * grow.min(1.0) * 1.0)));
            }
        }
    }
    for mut s in shapes {
        s.transform(xf);
        g.shape(s);
    }

    // The name slides in beside the badge.
    let tx = pill.left() + pad + badge + gap;
    let ty = pill.center().y - title.size().y / 2.0;
    let k = at(0.4, 0.8);
    g.text_xf(&title, pos2(tx - 10.0 * (1.0 - k), ty), alpha(TEXT, o * k), xf, Vec2::ZERO, 0.0);
}

fn balance(widths: &[f32], space: f32, max: f32) -> Vec<Vec<usize>> {
    let wrap = |limit: f32| -> Vec<Vec<usize>> {
        let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
        let mut w = 0.0;
        for (i, &ww) in widths.iter().enumerate() {
            let row = rows.last_mut().unwrap();
            let add = if row.is_empty() { ww } else { space + ww };
            if !row.is_empty() && w + add > limit {
                rows.push(vec![i]);
                w = ww;
            } else {
                row.push(i);
                w += add;
            }
        }
        rows
    };
    let best = wrap(max);
    if best.len() < 2 {
        return best;
    }
    let n = best.len();
    let widest = widths.iter().cloned().fold(0.0, f32::max);
    let (mut lo, mut hi) = (widest.min(max), max);
    for _ in 0..14 {
        let mid = (lo + hi) / 2.0;
        if wrap(mid).len() <= n {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    wrap(hi)
}

/// Snaps to the nearest guide line within reach, otherwise to the grid. Returns where, and the line to show.
fn snap_axis(pos: f32, size: f32, length: f32) -> (f32, Option<f32>) {
    let lines: [(f32, &[u8]); 5] = [
        (MARGIN, &[0]),
        (length - MARGIN, &[2]),
        (length / 2.0, &[1, 0, 2]),
        (length / 3.0, &[0, 1, 2]),
        (length * 2.0 / 3.0, &[0, 1, 2]),
    ];
    let mut best: Option<(f32, f32, f32)> = None;
    for (line, parts) in lines {
        for part in parts {
            let offset = match part {
                0 => 0.0,
                1 => size / 2.0,
                _ => size,
            };
            let d = (pos + offset - line).abs();
            if d <= SNAP && best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, line - offset, line));
            }
        }
    }
    match best {
        Some((_, p, l)) => (p, Some(l)),
        None => ((pos / GRID).round() * GRID, None),
    }
}

/// The arranging grid: 24 px lines, a stronger one every 96, only near the card and fading out from it.
fn grid_mesh(screen: Rect, c: Pos2, o: f32) -> Shape {
    let mask = |p: Pos2| -> f32 {
        let d = (((p.x - c.x) / 420.0).powi(2) + ((p.y - c.y) / 300.0).powi(2)).sqrt();
        if d <= 0.35 {
            1.0
        } else if d <= 0.6 {
            1.0 - 0.45 * (d - 0.35) / 0.25
        } else if d <= 1.0 {
            0.55 * (1.0 - (d - 0.6) / 0.4)
        } else {
            0.0
        }
    };
    let mut mesh = Mesh::default();
    let x0 = ((c.x - 420.0) / GRID).floor().max(0.0) * GRID;
    let x1 = (c.x + 420.0).min(screen.width());
    let y0 = ((c.y - 300.0) / GRID).floor().max(0.0) * GRID;
    let y1 = (c.y + 300.0).min(screen.height());
    let col = |a: f32, white: bool| if white { rgba(255, 255, 255, a) } else { rgba(0, 0, 0, a) };
    let quad = |mesh: &mut Mesh, r: Rect, a: [f32; 4], white: bool| {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(r.left_top(), col(a[0], white));
        mesh.colored_vertex(r.right_top(), col(a[1], white));
        mesh.colored_vertex(r.right_bottom(), col(a[2], white));
        mesh.colored_vertex(r.left_bottom(), col(a[3], white));
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    };
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            let (xb, yb) = ((x + GRID).min(x1), (y + GRID).min(y1));
            let m = [mask(pos2(x, y)), mask(pos2(xb, y)), mask(pos2(xb, yb)), mask(pos2(x, yb))];
            if m.iter().any(|v| *v > 0.0) {
                quad(&mut mesh, Rect::from_min_max(pos2(x, y), pos2(xb, yb)), m.map(|v| 0.18 * v * o), false);
                let vline = if (x as i32) % 96 == 0 { 0.17 } else { 0.05 };
                quad(&mut mesh, Rect::from_min_max(pos2(x, y), pos2(x + 1.0, yb)), [m[0], m[0], m[3], m[3]].map(|v| vline * v * o), true);
                let hline = if (y as i32) % 96 == 0 { 0.17 } else { 0.05 };
                quad(&mut mesh, Rect::from_min_max(pos2(x, y), pos2(xb, y + 1.0)), [m[0], m[1], m[1], m[0]].map(|v| hline * v * o), true);
            }
            x += GRID;
        }
        y += GRID;
    }
    Shape::mesh(mesh)
}

fn panel_open_blocks(panel_open: bool, pos: Option<Pos2>) -> bool {
    panel_open && pos.map(crate::panel::over_panel).unwrap_or(false)
}

pub fn fmt(s: f64) -> String {
    let s = s.max(0.0).floor() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The cover from a data URL, cut to a square (like object-fit: cover) and made small: the card shows it at 64 px.
fn load_cover(ctx: &Context, data: &str) -> Option<TextureHandle> {
    let bytes = crate::cover::data_url_bytes(data)?;
    let img = image::load_from_memory(&bytes).ok()?;
    let side = img.width().min(img.height());
    let img = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
    let px = (64.0 * 2.0 * ctx.pixels_per_point()).round() as u32;
    let img = if side > px { img.resize_exact(px, px, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let ci = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
    Some(ctx.load_texture("cover", ci, egui::TextureOptions::LINEAR))
}
