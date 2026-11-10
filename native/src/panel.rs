// The settings panel opened from the card's gear. Port of src/settings/settings.{js,css,html}. Electron showed it
// in its own small window; here it is drawn in the overlay's window, next to the card, and the overlay takes the
// keyboard while it is open. It closes like a menu: Esc, the ✕, or a click anywhere else.

use crate::colour::{alpha, hex, hex_to_hsv, hsv_to_rgb, mix, rgba, to_hex, Hsv};
use crate::core::{self, core};
use crate::draw::{self, line, text, tween, Pointer, Txt};
use crate::fonts;
use crate::lyrics::str_of;
use crate::overlay::{about, Group, TextDraw};
use egui::emath::TSTransform;
use egui::epaint::{Mesh, Shadow, StrokeKind};
use egui::{pos2, vec2, Color32, Context, CursorIcon, FontId, Galley, Id, Key, LayerId, Order, Pos2, Rect, Shape, Stroke, Vec2};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const WIDTH: f32 = 340.0;
const TEXT: Color32 = Color32::from_rgb(0xf4, 0xf4, 0xf1);
const TABS: [(&str, &str); 4] = [("show", "Show"), ("layout", "Layout"), ("look", "Look"), ("song", "This song")];
fn dim() -> Color32 {
    rgba(0xf4, 0xf4, 0xf1, 0.55)
}
fn line_c() -> Color32 {
    rgba(255, 255, 255, 0.08)
}
fn bg() -> Color32 {
    rgba(18, 18, 23, 0.96)
}
const HUES: [u32; 8] = [0xff4d5e, 0xffb648, 0xf5f25c, 0x5cf2b0, 0x6cc8ff, 0xb47cff, 0xff6fae, 0xff4d5e];
fn c32(n: u32) -> Color32 {
    Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
}

/// The panel's rectangle in overlay points, for the overlay to leave the pointer to it.
pub fn over_panel(p: Pos2) -> bool {
    let (px, py) = (p.x as f64, p.y as f64);
    core().panel_rect.map(|[x, y, w, h]| px >= x && px < x + w && py >= y && py < y + h).unwrap_or(false)
}

fn tab_file() -> std::path::PathBuf {
    core::data_dir().join("native-panel-tab.txt")
}

/// What is drawn this frame, and the pointer over it.
struct Pen<'a> {
    ctx: &'a Context,
    g: Group,
    ptr: Pointer,
    x: f32,
    w: f32,
    y: f32,
    o: f32,
    xf: TSTransform,
    live: bool,
    pressed: Option<String>,
    press_now: Option<String>,
    accent: Color32,
    cursor: Option<CursorIcon>,
}

impl Pen<'_> {
    fn shape(&mut self, s: impl Into<Shape>) {
        let mut s = s.into();
        s.transform(self.xf);
        self.g.shapes.push(s);
    }
    fn text(&mut self, gl: &Arc<Galley>, pos: Pos2, c: Color32) {
        self.g.texts.push(TextDraw { galley: gl.clone(), pos, color: alpha(c, self.o), xf: self.xf, offset: Vec2::ZERO, blur: 0.0 });
    }
    fn a(&self, c: Color32) -> Color32 {
        alpha(c, self.o)
    }
    fn hover(&self, r: Rect) -> bool {
        self.live && self.ptr.over(r)
    }
    /// Hover, press and click for a widget: clicked when let go over what was pressed.
    fn interact(&mut self, id: &str, r: Rect, enabled: bool) -> (bool, bool, bool) {
        if !enabled {
            return (false, false, false);
        }
        let hovered = self.hover(r);
        if hovered {
            self.cursor = Some(CursorIcon::PointingHand);
        }
        if hovered && self.ptr.pressed {
            self.press_now = Some(id.to_string());
        }
        let held = self.pressed.as_deref() == Some(id) && self.ptr.down;
        let clicked = hovered && self.ptr.released && self.pressed.as_deref() == Some(id);
        (hovered, held, clicked)
    }
    fn hover_t(&self, id: &str, on: bool) -> f32 {
        tween(self.ctx, &format!("p-{id}"), if on { 1.0 } else { 0.0 }, 0.15)
    }
}

enum Act {
    Core(&'static str, Value),
    Tab(usize),
    Picker(Option<&'static str>),
    PickColour(String, bool),
    ResetAll,
}

pub struct Panel {
    pub hwnd: Option<isize>,
    open: bool,
    opened_at: f64,
    closing: Option<f64>,
    left_side: bool,
    anchor: (f32, f32),
    root: Rect,
    last_h: f32,
    tab: usize,
    tab_from: Option<(usize, f64, f32)>,
    pressed: Option<String>,
    target: Option<&'static str>,
    hsv: Hsv,
    shown: Option<String>,
    dragging: bool,
    sent_at: f64,
    pending: Option<(String, bool, f64)>,
    hex_text: String,
    hex_bad: bool,
    sliding: Option<(&'static str, f64)>,
    slide_sent: f64,
    slide_pending: Option<(&'static str, f64, f64)>,
    reset_until: Option<f64>,
    had_focus: bool,
    wake: Option<f64>,
    stage_last: f32,
    hex_rect: Option<Rect>,
    /// The now playing card's options are unfolded in the Show tab.
    card_more: bool,
}

impl Panel {
    pub fn new() -> Self {
        let tab = std::fs::read_to_string(tab_file()).ok().and_then(|t| TABS.iter().position(|(k, _)| *k == t.trim())).unwrap_or(0);
        Panel {
            hwnd: None,
            open: false,
            opened_at: 0.0,
            closing: None,
            left_side: false,
            anchor: (0.0, 0.0),
            root: Rect::NOTHING,
            last_h: 520.0,
            tab,
            tab_from: None,
            pressed: None,
            target: None,
            hsv: Hsv { h: 0.0, s: 0.0, v: 1.0 },
            shown: None,
            dragging: false,
            sent_at: 0.0,
            pending: None,
            hex_text: String::new(),
            hex_bad: false,
            sliding: None,
            slide_sent: 0.0,
            slide_pending: None,
            reset_until: None,
            had_focus: false,
            wake: None,
            stage_last: 300.0,
            hex_rect: None,
            card_more: false,
        }
    }

    /// The overlay window was made again (switching between GPU and CPU drawing). The new window doesn't have the
    /// keyboard yet: give it the keyboard, and don't read that moment as clicking away from the panel.
    pub fn window_changed(&mut self) {
        self.had_focus = false;
        self.pressed = None;
        if self.open && self.closing.is_none() {
            if let Some(h) = self.hwnd {
                crate::winutil::set_activatable(h, true);
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.open && self.closing.is_none()
    }

    /// Opens next to the card at (x, y, width), or on its left when there is no room on the right.
    pub fn open(&mut self, ctx: &Context, at: Option<(f64, f64, f64)>) {
        let screen = ctx.content_rect();
        let (cx, cy, cw) = at.map(|(x, y, w)| (x as f32, y as f32, w as f32)).unwrap_or_else(|| {
            let c = core();
            let card = c.s("card").clone();
            (card["x"].as_f64().unwrap_or(24.0) as f32, card["y"].as_f64().unwrap_or(24.0) as f32, 340.0 * c.s("cardScale").as_f64().unwrap_or(1.0) as f32)
        });
        let win_w = WIDTH + 28.0;
        let mut x = cx + cw;
        self.left_side = false;
        if x + win_w > screen.width() {
            x = cx - win_w;
            self.left_side = true;
        }
        self.anchor = (x.max(0.0), (cy - 10.0).max(0.0));
        self.open = true;
        self.closing = None;
        self.opened_at = ctx.input(|i| i.time);
        self.pressed = None;
        self.target = None;
        self.reset_until = None;
        self.had_focus = false;
        self.tab_from = None;
        core().panel_open = true;
        if let Some(h) = self.hwnd {
            crate::winutil::set_activatable(h, true);
        }
        ctx.request_repaint();
    }

    /// Plays the closing animation, then closes.
    pub fn close(&mut self) {
        if self.open && self.closing.is_none() {
            self.closing = Some(-1.0);
            self.flush();
        }
        crate::core::notify();
    }

    fn finish_close(&mut self) {
        self.open = false;
        self.closing = None;
        let mut c = core();
        c.panel_open = false;
        c.panel_rect = None;
        drop(c);
        if let Some(h) = self.hwnd {
            crate::winutil::set_activatable(h, false);
        }
    }

    /// Sends what is still waiting (a slider or colour mid-throttle).
    fn flush(&mut self) {
        if let Some((key, v, _)) = self.slide_pending.take() {
            core::panel_action("set", json!({ "key": key, "value": v }));
        }
        if let Some((hex, commit, _)) = self.pending.take() {
            if let Some(t) = self.target {
                core::panel_action("choose", json!({ "key": t, "value": hex, "commit": commit }));
            }
        }
    }

    pub fn next_wake(&self) -> Option<Duration> {
        self.wake.map(Duration::from_secs_f64)
    }

    pub fn ui(&mut self, ctx: &Context, screen: Rect) {
        self.wake = None;
        if !self.open {
            return;
        }
        let time = ctx.input(|i| i.time);
        if let Some(t) = &mut self.closing {
            if *t < 0.0 {
                *t = time;
            }
        }
        if let Some(t0) = self.closing {
            if time - t0 >= 0.16 {
                self.finish_close();
                return;
            }
        }

        // Closes like a menu when the overlay loses the keyboard to another window.
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        if focused {
            self.had_focus = true;
        } else if self.had_focus && self.closing.is_none() && self.pending.is_none() {
            core::test_log("panel closed: lost keyboard focus");
            self.close();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.close();
        }
        if ctx.input(|i| i.key_pressed(Key::Tab) && i.modifiers.ctrl) {
            let back = ctx.input(|i| i.modifiers.shift);
            let to = (self.tab + if back { TABS.len() - 1 } else { 1 }) % TABS.len();
            self.show_tab(to, time);
        }

        // Timers: the throttled slider and colour, and Reset all's second click.
        if let Some((key, v, due)) = self.slide_pending {
            if time >= due {
                self.slide_pending = None;
                self.slide_sent = time;
                core::panel_action("set", json!({ "key": key, "value": v }));
            } else {
                self.wake = Some(due - time);
            }
        }
        if let Some((hexs, commit, due)) = self.pending.clone() {
            if time >= due {
                self.pending = None;
                self.sent_at = time;
                if let Some(t) = self.target {
                    core::panel_action("choose", json!({ "key": t, "value": hexs, "commit": commit }));
                }
            } else {
                self.wake = Some(self.wake.unwrap_or(f64::INFINITY).min(due - time));
            }
        }
        if let Some(until) = self.reset_until {
            if time > until {
                self.reset_until = None;
            } else {
                self.wake = Some(self.wake.unwrap_or(f64::INFINITY).min(until - time));
            }
        }

        let state = core().panel_state();
        let s = &state["settings"];
        let mode = str_of(s, "colourMode").to_string();
        let cover_colour = state["coverColour"].as_str().and_then(hex);
        let accent = match mode.as_str() {
            "fade" => hex(str_of(s, "fadeFrom")),
            "cover" if cover_colour.is_some() => cover_colour,
            _ => hex(str_of(s, "accent")),
        }
        .unwrap_or(Color32::from_rgb(0xff, 0xb6, 0x48));

        // Keep the picker on what it edits: closed when that colour is no longer in use, and in step with changes
        // made elsewhere, but never fighting a drag in progress.
        if (self.target == Some("accent") && mode != "accent") || (matches!(self.target, Some("fadeFrom" | "fadeTo")) && mode != "fade") {
            self.target = None;
        }
        if let Some(t) = self.target {
            let v = str_of(s, t).to_string();
            if !v.is_empty() && Some(&v) != self.shown.as_ref() {
                self.shown = Some(v.clone());
                if !self.dragging && !ctx.memory(|m| m.has_focus(Id::unique("hex"))) {
                    if let Some(c) = hex(&v) {
                        self.hsv = hex_to_hsv(c);
                        self.hex_text = v.to_uppercase();
                        self.hex_bad = false;
                    }
                }
            }
        }

        // Opening and closing: grows out of the card's side.
        let (k, closing) = match self.closing {
            Some(t0) => (1.0 - ((time - t0) / 0.16) as f32, true),
            None => (draw::bezier(0.2, 0.9, 0.25, 1.06, ((time - self.opened_at) / 0.26) as f32), false),
        };
        let animating_open = closing || time - self.opened_at < 0.26;
        let from = if self.left_side { 14.0 } else { -14.0 };
        let (scale_from, op) = if closing { (0.95, k.clamp(0.0, 1.0)) } else { (0.94, ((time - self.opened_at) / 0.26).clamp(0.0, 1.0) as f32) };
        let s_now = scale_from + (1.0 - scale_from) * k;

        // Where it goes: the window was 368 wide with the panel 14 in, and as tall as its content.
        let win_h = (self.last_h + 28.0).clamp(120.0, screen.height());
        let win_y = self.anchor.1.min(screen.height() - win_h).max(0.0);
        let root = Rect::from_min_size(pos2(self.anchor.0 + 14.0, win_y + 10.0), vec2(WIDTH, self.last_h));
        self.root = root;
        core().panel_rect = Some([root.left() as f64, root.top() as f64, root.width() as f64, root.height() as f64]);
        let origin = if self.left_side { pos2(root.right(), root.top() + 24.0) } else { pos2(root.left(), root.top() + 24.0) };
        let xf = about(origin, s_now, vec2(from * (1.0 - k), 0.0));

        let ptr = Pointer::read(ctx);
        let mut pen = Pen {
            ctx,
            g: Group::default(),
            ptr,
            x: root.left() + 14.0,
            w: WIDTH - 28.0,
            y: root.top(),
            o: op,
            xf,
            live: !closing,
            pressed: self.pressed.clone(),
            press_now: None,
            accent,
            cursor: None,
        };
        let mut acts: Vec<Act> = Vec::new();

        // Background (drawn first; its height is known from last frame).
        pen.shape(Shape::Rect(Shadow { offset: [0, 10], blur: 30, spread: 0, color: rgba(0, 0, 0, 0.45 * op) }.as_shape(root, 12.0)));
        pen.shape(Shape::Rect(egui::epaint::RectShape::new(root, 12.0, alpha(bg(), op), Stroke::new(1.0, rgba(255, 255, 255, 0.08 * op)), StrokeKind::Inside)));

        // Head.
        let name = line(ctx, "Settings", fonts::inter(800, 14.0), TEXT);
        pen.text(&name, pos2(root.left() + 14.0, root.top() + 10.0 + (26.0 - name.size().y) / 2.0), TEXT);
        let close_r = Rect::from_min_size(pos2(root.right() - 10.0 - 26.0, root.top() + 10.0), vec2(26.0, 26.0));
        let (h, _, clicked) = pen.interact("close", close_r.expand(3.0), true);
        let ht = pen.hover_t("close", h);
        if ht > 0.0 {
            pen.shape(Shape::rect_filled(close_r, 6.0, rgba(255, 255, 255, 0.08 * ht * op)));
        }
        let x = line(ctx, "✕", fonts::inter(400, 13.0), TEXT);
        pen.text(&x, close_r.center() - x.size() / 2.0, mix(TEXT, ht, dim()));
        if clicked {
            acts.push(Act::Core("close", Value::Null));
        }

        // Tabs: a segmented control with a highlight that slides to the chosen tab.
        let tabs_r = Rect::from_min_size(pos2(root.left() + 12.0, root.top() + 42.0), vec2(WIDTH - 24.0, 33.0));
        pen.shape(Shape::rect_filled(tabs_r, 9.0, rgba(255, 255, 255, 0.06 * op)));
        let tw = (tabs_r.width() - 6.0) / 4.0;
        let ind = tween(ctx, "p-tab-ind", self.tab as f32, 0.28);
        let ind_r = Rect::from_min_size(pos2(tabs_r.left() + 3.0 + ind * tw, tabs_r.top() + 3.0), vec2(tw, tabs_r.height() - 6.0));
        pen.shape(Shape::Rect(Shadow { offset: [0, 1], blur: 4, spread: 0, color: rgba(0, 0, 0, 0.3 * op) }.as_shape(ind_r, 7.0)));
        pen.shape(Shape::rect_filled(ind_r, 7.0, rgba(255, 255, 255, 0.13 * op)));
        for (i, (_, label)) in TABS.iter().enumerate() {
            let r = Rect::from_min_size(pos2(tabs_r.left() + 3.0 + i as f32 * tw, tabs_r.top() + 3.0), vec2(tw, tabs_r.height() - 6.0));
            let (h, _, clicked) = pen.interact(&format!("tab{i}"), r, true);
            let ht = pen.hover_t(&format!("tab{i}"), h || i == self.tab);
            let g = line(ctx, label, fonts::inter(600, 12.0), TEXT);
            pen.text(&g, r.center() - g.size() / 2.0, mix(TEXT, ht, dim()));
            if clicked {
                acts.push(Act::Tab(i));
            }
        }

        // The tab's content: its height animates between tabs, and the new tab slides in from its side.
        let stage_top = tabs_r.bottom() + 4.0;
        let mut stage_h;
        let stage_group;
        {
            let clip_shapes_from = pen.g.shapes.len();
            let clip_texts_from = pen.g.texts.len();
            let base_xf = pen.xf;
            let base_o = pen.o;
            let mut old_h = None;
            if let Some((from_tab, t0, h0)) = self.tab_from {
                let e = ((time - t0) / 0.16) as f32;
                if e < 1.0 {
                    pen.o = base_o * (1.0 - e);
                    pen.live = false;
                    pen.y = stage_top + 8.0;
                    self.pane(&mut pen, from_tab, &state, &mut Vec::new(), time);
                    pen.live = !closing;
                }
                old_h = Some(h0);
            }
            let (dx, o2) = match self.tab_from {
                Some((from_tab, t0, _)) => {
                    let e = draw::bezier(0.2, 0.8, 0.25, 1.0, ((time - t0) / 0.26) as f32);
                    (if self.tab > from_tab { 24.0 } else { -24.0 } * (1.0 - e), e)
                }
                None => (0.0, 1.0),
            };
            pen.xf = base_xf * TSTransform::from_translation(vec2(dx, 0.0));
            pen.o = base_o * o2;
            pen.y = stage_top + 8.0;
            let tab = self.tab;
            self.pane(&mut pen, tab, &state, &mut acts, time);
            let new_h = pen.y + 12.0 - stage_top;
            pen.xf = base_xf;
            pen.o = base_o;
            stage_h = new_h;
            if let Some((_, t0, _)) = self.tab_from {
                let e = draw::bezier(0.3, 0.8, 0.25, 1.0, ((time - t0) / 0.26) as f32);
                if let Some(h0) = old_h {
                    stage_h = h0 + (new_h - h0) * e;
                }
                if time - t0 > 0.28 {
                    self.tab_from = None;
                }
            }
            stage_group = Group { shapes: pen.g.shapes.split_off(clip_shapes_from), texts: pen.g.texts.split_off(clip_texts_from), ..Default::default() };
        }
        self.stage_last = stage_h;

        // Foot.
        let foot_top = stage_top + stage_h;
        pen.shape(Shape::rect_filled(Rect::from_min_size(pos2(root.left(), foot_top), vec2(WIDTH, 1.0)), 0.0, alpha(line_c(), op)));
        pen.y = foot_top + 11.0;
        pen.x = root.left() + 14.0;
        let arrange = button_size(ctx, "Arrange");
        let r = Rect::from_min_size(pos2(pen.x, pen.y), arrange);
        if button(&mut pen, "f-move", "Arrange", r, false, true, false) {
            acts.push(Act::Core("move", Value::Null));
        }
        let confirm = self.reset_until.is_some();
        let label = if confirm { "Click again to reset" } else { "Reset all" };
        let rs = button_size(ctx, label);
        let r2 = Rect::from_min_size(pos2(r.right() + 6.0, pen.y), rs);
        if button_styled(&mut pen, "f-reset", label, r2, true, true, false, confirm.then_some(Color32::from_rgb(0xe5, 0x48, 0x4d))) {
            acts.push(Act::ResetAll);
        }
        let qs = button_size(ctx, "Quit");
        let r3 = Rect::from_min_size(pos2(root.right() - 14.0 - qs.x, pen.y), qs);
        let quit_hover = pen.hover(r3);
        if button_styled(&mut pen, "f-quit", "Quit", r3, true, true, false, quit_hover.then_some(Color32::from_rgb(0xe5, 0x48, 0x4d))) {
            acts.push(Act::Core("quit", Value::Null));
        }
        let total = foot_top + 1.0 + 10.0 + arrange.y + 12.0 - root.top();
        if (total - self.last_h).abs() > 0.5 {
            self.last_h = total;
            ctx.request_repaint();
        }

        // The colour code field: a real text box, so it takes typing, pasting and selecting.
        if self.tab == 2 && self.target.is_some() && self.tab_from.is_none() {
            if let Some(field) = self.hex_rect {
                let area = egui::Area::new(Id::unique("hex-area")).order(Order::Foreground).fixed_pos(xf * field.min).interactable(true);
                let o = op;
                let resp = area.show(ctx, |ui| {
                    ui.visuals_mut().selection.bg_fill = alpha(accent, 0.5);
                    ui.visuals_mut().text_cursor.stroke = Stroke::new(1.5, TEXT);
                    let te = egui::TextEdit::singleline(&mut self.hex_text)
                        .id(Id::unique("hex"))
                        .font(fonts::inter(600, 12.0))
                        .text_color(alpha(TEXT, o))
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::symmetric(8, 5))
                        .desired_width(field.width() - 16.0)
                        .char_limit(7);
                    ui.add(te)
                });
                let r = resp.inner;
                if r.changed() {
                    self.hex_text = self.hex_text.to_uppercase();
                    match typed_hex(&self.hex_text) {
                        Some(v) => {
                            self.hex_bad = false;
                            if let Some(c) = hex(&v) {
                                self.hsv = hex_to_hsv(c);
                            }
                            acts.push(Act::PickColour(v, false));
                        }
                        None => self.hex_bad = true,
                    }
                }
                if r.lost_focus() {
                    if let Some(v) = typed_hex(&self.hex_text) {
                        acts.push(Act::PickColour(v, true));
                    }
                }
            }
        }

        // A press: remember what it was on; anywhere off the panel closes it (the overlay sees those presses).
        if ptr.pressed {
            self.pressed = pen.press_now.clone();
            if let Some(p) = ptr.pos {
                if !root.contains(p) && self.closing.is_none() {
                    acts.push(Act::Core("close", Value::Null));
                }
            }
        }
        if !ptr.down && !ptr.pressed {
            self.pressed = None;
            if self.dragging {
                self.dragging = false;
                acts.push(Act::PickColour(to_hex(hsv_to_rgb(self.hsv)), true));
            }
            if let Some((key, _)) = self.sliding.take() {
                let _ = key;
                if let Some((k, v, _)) = self.slide_pending.take() {
                    core::panel_action("set", json!({ "key": k, "value": v }));
                }
            }
        }
        if let Some(c) = pen.cursor {
            if ptr.pos.map(|p| root.contains(p)).unwrap_or(false) {
                ctx.set_cursor_icon(c);
            }
        }

        let uv = draw::uv_scale(ctx);
        let painter = ctx.layer_painter(LayerId::new(Order::Middle, Id::unique("panel"))).with_clip_rect(screen);
        pen.g.paint(&painter, uv);
        let stage_clip = xf * Rect::from_min_max(pos2(root.left(), stage_top), pos2(root.right(), stage_top + stage_h));
        stage_group.paint(&painter.with_clip_rect(stage_clip.intersect(screen)), uv);

        for a in acts {
            match a {
                Act::Core(name, v) => {
                    if name == "close" {
                        self.close();
                    } else {
                        core::panel_action(name, v);
                    }
                }
                Act::Tab(i) => self.show_tab(i, time),
                Act::Picker(t) => self.open_picker(t, &state),
                Act::PickColour(hexs, commit) => self.pick_colour(hexs, commit, time),
                Act::ResetAll => {
                    if self.reset_until.is_some() {
                        self.reset_until = None;
                        core::panel_action("reset-all", Value::Null);
                    } else {
                        self.reset_until = Some(time + 3.0);
                    }
                }
            }
        }
        if animating_open || self.tab_from.is_some() || self.dragging {
            self.wake = Some(1.0 / 60.0);
        }
    }

    fn show_tab(&mut self, i: usize, time: f64) {
        if i == self.tab {
            return;
        }
        self.tab_from = Some((self.tab, time, self.stage_last));
        self.tab = i;
        let _ = std::fs::write(tab_file(), TABS[i].0);
    }

    fn open_picker(&mut self, what: Option<&'static str>, state: &Value) {
        self.target = what;
        if let Some(t) = what {
            let v = str_of(&state["settings"], t).to_string();
            if let Some(c) = hex(&v) {
                self.hsv = hex_to_hsv(c);
                self.hex_text = v.to_uppercase();
                self.hex_bad = false;
            }
            self.shown = Some(v);
        }
    }

    /// Sends the colour while dragging, at most every 60 ms (each change is saved to disk). "commit" marks the
    /// colour as really picked, for the history: that happens when you let go.
    fn pick_colour(&mut self, hexs: String, commit: bool, time: f64) {
        let Some(t) = self.target else { return };
        if commit || time - self.sent_at > 0.06 {
            self.pending = None;
            self.sent_at = time;
            core::panel_action("choose", json!({ "key": t, "value": hexs, "commit": commit }));
        } else {
            self.pending = Some((hexs, commit, self.sent_at + 0.06));
        }
    }

    /* ---------- The tabs ---------- */
    fn pane(&mut self, pen: &mut Pen, tab: usize, state: &Value, acts: &mut Vec<Act>, time: f64) {
        match tab {
            0 => self.pane_show(pen, state, acts),
            1 => self.pane_layout(pen, state, acts, time),
            2 => self.pane_look(pen, state, acts),
            _ => self.pane_song(pen, state, acts),
        }
    }

    fn pane_show(&mut self, pen: &mut Pen, state: &Value, acts: &mut Vec<Act>) {
        let s = &state["settings"];
        update_banner(pen, &state["update"], acts);
        let card_on = s["showCard"].as_bool().unwrap_or(false);
        let rows: [(&str, &str, bool, Option<&str>); 12] = [
            ("enabled", "Lyrics", false, Some("Ctrl+Alt+L")),
            ("showCard", "Now playing card", false, None),
            ("showCardLyrics", "Lyrics on the card", true, None),
            ("showCover", "Cover art", true, None),
            ("showArtist", "Artist", true, None),
            ("showProgress", "Progress bar and time", true, None),
            ("showControls", "Play controls", true, None),
            ("showBig", "Big floating lyrics", false, None),
            ("recognise", "Recognise songs without lyrics", false, None),
            ("checkUpdates", "Check for updates", false, None),
            ("useGpu", "Use GPU", false, None),
            ("startupAnimation", "Startup animation", false, None),
        ];
        // The card's own options fold away under its row; the arrow opens and closes them.
        let k = tween(pen.ctx, "card-more", if self.card_more { 1.0 } else { 0.0 }, 0.2);
        for (key, label, sub, kbd) in rows {
            let on = s[key].as_bool().unwrap_or(false);
            let enabled = !sub || card_on;
            if sub {
                if k <= 0.01 {
                    continue;
                }
                // Slide open: each row takes k of its height and fades in.
                let (y0, o) = (pen.y, pen.o);
                pen.o *= k * k;
                let clicked = toggle_row(pen, key, label, sub, kbd, on, enabled && k > 0.99);
                pen.o = o;
                pen.y = y0 + (pen.y - y0) * k;
                if clicked {
                    acts.push(Act::Core("toggle", json!(key)));
                }
                continue;
            }
            let y0 = pen.y;
            let clicked = toggle_row(pen, key, label, sub, kbd, on, enabled);
            if key == "showCard" {
                // The arrow sits just after the label; the label and the arrow both open and close the options.
                let lw = line(pen.ctx, label, fonts::inter(400, 13.0), TEXT).size().x;
                let r = Rect::from_center_size(pos2(pen.x + lw + 4.0 + 12.0, y0 + 14.0), vec2(24.0, 24.0));
                let hit = Rect::from_min_max(pos2(pen.x, y0), pos2(r.right(), y0 + 28.0));
                let (h, _, open_click) = pen.interact("card-more", hit, true);
                let hk = pen.hover_t("card-more", h);
                pen.shape(Shape::rect_filled(r, 6.0, rgba(255, 255, 255, 0.08 * hk * pen.o)));
                // A chevron pointing right when closed, down when open.
                let a = k * std::f32::consts::FRAC_PI_2;
                let rot = |p: Vec2| r.center() + vec2(p.x * a.cos() - p.y * a.sin(), p.x * a.sin() + p.y * a.cos());
                let pts = vec![rot(vec2(-2.0, -4.5)), rot(vec2(2.5, 0.0)), rot(vec2(-2.0, 4.5))];
                pen.shape(Shape::line(pts, Stroke::new(1.6, pen.a(mix(TEXT, hk, dim())))));
                if open_click {
                    self.card_more = !self.card_more;
                } else if clicked {
                    acts.push(Act::Core("toggle", json!(key)));
                }
                continue;
            }
            if clicked {
                acts.push(Act::Core("toggle", json!(key)));
            }
        }
        hint(pen, "No lyrics? Asks Shazam. Unofficial, may break. Updates: asks GitHub once a day. Use GPU: less CPU, more memory.");
        pen.y += 4.0;
        label(pen, "Hide the card while holding", None);
        chips(pen, "peekKey", &state["options"]["peekKeys"], str_of(s, "peekKey"), acts);
        hint(pen, "Or just right-click the card.");
        pen.y += 6.0;
    }

    fn pane_layout(&mut self, pen: &mut Pen, state: &Value, acts: &mut Vec<Act>, time: f64) {
        let s = &state["settings"];
        group(pen, "Card", true);
        self.slider(pen, "cardScale", "Size", s, 60.0, 200.0, 5.0, true, time);
        self.slider(pen, "cardOpacity", "Background", s, 0.0, 100.0, 1.0, true, time);
        pen.y += 4.0;
        label(pen, "Lyrics on the card go", None);
        chips(pen, "cardLyricsSide", &state["options"]["sides"], str_of(s, "cardLyricsSide"), acts);
        pen.y += 6.0;
        self.slider(pen, "cardNext", "Coming lines shown", s, 0.0, 4.0, 1.0, false, time);
        group(pen, "Big lyrics", false);
        self.slider(pen, "bigSize", "Text size", s, 40.0, 250.0, 5.0, true, time);
        self.slider(pen, "bigWidth", "Line width", s, 25.0, 98.0, 1.0, true, time);
        pen.y += 4.0;
        label(pen, "Place", None);
        let pos = &s["bigPos"];
        let (px, py) = (pos["x"].as_f64().unwrap_or(0.5), pos["y"].as_f64().unwrap_or(0.44));
        let presets = [("Top", 0.18), ("Middle", 0.44), ("Lower third", 0.76)];
        let mut x = pen.x;
        let mut h = 0.0;
        for (lbl, y) in presets {
            let size = button_size(pen.ctx, lbl);
            let r = Rect::from_min_size(pos2(x, pen.y), size);
            let on = (px - 0.5).abs() < 0.01 && (py - y).abs() < 0.01;
            if button(pen, &format!("preset-{lbl}"), lbl, r, false, true, on) {
                acts.push(Act::Core("set", json!({ "key": "bigPos", "value": { "x": 0.5, "y": y } })));
            }
            x += size.x + 5.0;
            h = size.y;
        }
        pen.y += h + 6.0 + 4.0;
        label(pen, "Line up text", None);
        chips(pen, "bigAlign", &state["options"]["aligns"], str_of(s, "bigAlign"), acts);
        pen.y += 6.0 + 10.0;
        let size = button_size(pen.ctx, "Reset layout");
        let r = Rect::from_min_size(pos2(pen.x, pen.y), size);
        if button(pen, "reset-layout", "Reset layout", r, true, true, false) {
            acts.push(Act::Core("reset-layout", Value::Null));
        }
        pen.y += size.y;
        hint(pen, "Move and resize: “Arrange” (Ctrl+Alt+M).");
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(&mut self, pen: &mut Pen, key: &'static str, name: &str, s: &Value, min: f32, max: f32, step: f32, pct: bool, time: f64) {
        let mult = if pct { 100.0 } else { 1.0 };
        let saved = s[key].as_f64().unwrap_or(0.0) as f32;
        let live = match self.sliding {
            Some((k, v)) if k == key => v as f32,
            _ => saved,
        };
        let shown = if pct { format!("{}%", (live * 100.0).round()) } else { format!("{}", live) };
        pen.y += 4.0;
        label(pen, name, Some(&shown));
        pen.y -= 1.0;
        // 24 px tall to click, laid out as 16.
        let r = Rect::from_min_size(pos2(pen.x, pen.y - 4.0), vec2(pen.w, 24.0));
        let (h, held, _) = pen.interact(&format!("slider-{key}"), r, true);
        if held || (h && pen.ptr.pressed) {
            if let Some(p) = pen.ptr.pos {
                let k = ((p.x - r.left() - 7.0) / (r.width() - 14.0)).clamp(0.0, 1.0);
                let raw = min + (max - min) * k;
                let v = ((raw / step).round() * step).clamp(min, max) / mult;
                let v = ((v * 1000.0).round() / 1000.0) as f64;
                if self.sliding.map(|(_, old)| old) != Some(v) {
                    self.sliding = Some((key, v));
                    // At most every 80 ms; each change is saved to disk.
                    if time - self.slide_sent > 0.08 {
                        self.slide_sent = time;
                        self.slide_pending = None;
                        core::panel_action("set", json!({ "key": key, "value": v }));
                    } else {
                        self.slide_pending = Some((key, v, self.slide_sent + 0.08));
                    }
                } else if self.sliding.is_none() {
                    self.sliding = Some((key, v));
                }
            }
        }
        let k = ((live * mult - min) / (max - min)).clamp(0.0, 1.0);
        let track = Rect::from_center_size(r.center(), vec2(r.width(), 4.0));
        pen.shape(Shape::rect_filled(track, 2.0, rgba(255, 255, 255, 0.16 * pen.o)));
        let c = pos2(r.left() + 7.0 + (r.width() - 14.0) * k, r.center().y);
        let grow = pen.hover_t(&format!("slider-{key}"), h || held);
        let rad = 7.0 * (1.0 + 0.2 * grow);
        pen.shape(Shape::circle_filled(c, rad + 3.0, alpha(bg(), pen.o)));
        pen.shape(Shape::circle_filled(c, rad, pen.a(pen.accent)));
        pen.y += 16.0 + 6.0;
    }

    fn pane_look(&mut self, pen: &mut Pen, state: &Value, acts: &mut Vec<Act>) {
        let s = &state["settings"];
        let o = &state["options"];
        for (key, name, list) in [("look", "Look", "looks"), ("motion", "Motion", "motions"), ("entrance", "Words come in", "entrances"), ("font", "Font", "fonts")] {
            pen.y += 4.0;
            label(pen, name, None);
            chips(pen, key, &o[list], str_of(s, key), acts);
            pen.y += 6.0;
        }

        // Colour: swatches, the fade's ends, the picker.
        let mode = str_of(s, "colourMode").to_string();
        let names = [("accent", "your colour"), ("rainbow", "rainbow"), ("fade", "fade"), ("cover", "from the cover")];
        let cname = names.iter().find(|(k, _)| *k == mode).map(|(_, n)| *n).unwrap_or("");
        pen.y += 4.0;
        label(pen, "Colour", Some(cname));
        pen.y += 2.0;
        let cover = state["coverColour"].as_str().and_then(hex);
        let history: Vec<Color32> = s["history"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().and_then(hex)).collect()).unwrap_or_default();
        let accent_c = hex(str_of(s, "accent")).unwrap_or(Color32::WHITE);
        let (fa, fb) = (hex(str_of(s, "fadeFrom")).unwrap_or(Color32::WHITE), hex(str_of(s, "fadeTo")).unwrap_or(Color32::WHITE));
        let cy = pen.y + 11.0;
        let mut x = pen.x;
        let mut sw = |pen: &mut Pen, id: &str, d: f32, gap_before: f32| -> (Rect, bool, bool) {
            x += gap_before;
            let r = Rect::from_center_size(pos2(x + d / 2.0, cy), vec2(d, d));
            x += d + 8.0;
            let (h, _, clicked) = pen.interact(id, r.expand(4.0), true);
            (r, h, clicked)
        };
        let draw_sw = |pen: &mut Pen, r: Rect, h: bool, on: bool, ring: Option<Color32>, fill: &dyn Fn(&mut Pen, Pos2, f32)| {
            let k = pen.hover_t(&format!("sw{}", r.center().x as i32), h);
            let rad = r.width() / 2.0 * (1.0 + 0.12 * k);
            if on {
                pen.shape(Shape::circle_filled(r.center(), rad + 4.0, pen.a(Color32::WHITE)));
                pen.shape(Shape::circle_filled(r.center(), rad + 2.0, alpha(bg(), pen.o)));
            } else if let Some(c) = ring {
                pen.shape(Shape::circle_filled(r.center(), rad + 4.0, pen.a(c)));
                pen.shape(Shape::circle_filled(r.center(), rad + 2.0, alpha(bg(), pen.o)));
            }
            fill(pen, r.center(), rad);
        };
        // Your colour: a dot of it inside a hue ring.
        let (r, h, clicked) = sw(pen, "sw-accent", 22.0, 0.0);
        let open_ring = (self.target == Some("accent")).then_some(dim());
        draw_sw(pen, r, h, mode == "accent", open_ring, &|pen, c, rad| {
            conic(pen, c, rad);
            pen.shape(Shape::circle_filled(c, rad * 0.707, pen.a(accent_c)));
        });
        if clicked {
            if mode != "accent" {
                acts.push(Act::Core("choose", json!({ "key": "colourMode", "value": "accent" })));
                acts.push(Act::Picker(Some("accent")));
            } else {
                acts.push(Act::Picker(if self.target == Some("accent") { None } else { Some("accent") }));
            }
        }
        let (r, h, clicked) = sw(pen, "sw-rainbow", 22.0, 0.0);
        draw_sw(pen, r, h, mode == "rainbow", None, &|pen, c, rad| conic(pen, c, rad));
        if clicked {
            acts.push(Act::Core("choose", json!({ "key": "colourMode", "value": "rainbow" })));
            acts.push(Act::Picker(None));
        }
        // The last two colours you picked.
        for i in 0..2 {
            let (r, h, clicked) = sw(pen, &format!("sw-h{i}"), 18.0, if i == 0 { 6.0 } else { 0.0 });
            match history.get(i) {
                Some(c) => {
                    let c = *c;
                    draw_sw(pen, r, h, false, None, &|pen, ce, rad| {
                        pen.shape(Shape::circle_filled(ce, rad, pen.a(c)));
                        pen.shape(Shape::circle_stroke(ce, rad - 0.5, Stroke::new(1.0, rgba(0, 0, 0, 0.3 * pen.o))));
                    });
                    if clicked {
                        acts.push(Act::Core("choose", json!({ "key": "accent", "value": to_hex(c), "commit": true })));
                        if self.target.is_some() && self.target != Some("accent") {
                            acts.push(Act::Picker(None));
                        }
                    }
                }
                None => {
                    for s in Shape::dashed_line(&circle_pts(r.center(), r.width() / 2.0 - 0.75), Stroke::new(1.5, rgba(255, 255, 255, 0.22 * pen.o)), 2.5, 2.0) {
                        pen.shape(s);
                    }
                }
            }
        }
        let (r, h, clicked) = sw(pen, "sw-fade", 22.0, 6.0);
        draw_sw(pen, r, h, mode == "fade", None, &|pen, c, rad| diag(pen, c, rad, fa, fb));
        if clicked {
            if mode != "fade" {
                acts.push(Act::Core("choose", json!({ "key": "colourMode", "value": "fade" })));
            }
            if self.target == Some("accent") {
                acts.push(Act::Picker(None));
            }
        }
        let (r, h, clicked) = sw(pen, "sw-cover", 22.0, 0.0);
        draw_sw(pen, r, h, mode == "cover", None, &|pen, c, rad| {
            match cover {
                Some(col) => pen.shape(Shape::circle_filled(c, rad, pen.a(col))),
                None => {
                    pen.shape(Shape::circle_filled(c, rad, rgba(255, 255, 255, 0.1 * pen.o)));
                    pen.shape(Shape::circle_stroke(c, rad - 0.5, Stroke::new(1.0, rgba(255, 255, 255, 0.25 * pen.o))));
                }
            }
            let note = line(pen.ctx, "♪", fonts::inter(700, 12.0), TEXT);
            let col = if cover.is_some() { rgba(0, 0, 0, 0.6) } else { dim() };
            pen.text(&note, c - note.size() / 2.0, col);
        });
        if clicked {
            acts.push(Act::Core("choose", json!({ "key": "colourMode", "value": "cover" })));
            acts.push(Act::Picker(None));
        }
        pen.y += 22.0 + 4.0;

        if mode == "fade" {
            pen.y += 8.0;
            let mut x = pen.x;
            for (key, lbl, c) in [("fadeFrom", "Start", fa), ("fadeTo", "End", fb)] {
                let g = line(pen.ctx, lbl, fonts::inter(600, 12.0), TEXT);
                let size = vec2(6.0 + 16.0 + 6.0 + g.size().x + 10.0, g.size().y.max(16.0) + 10.0);
                let r = Rect::from_min_size(pos2(x, pen.y), size);
                let (h, _, clicked) = pen.interact(&format!("end-{key}"), r, true);
                let k = pen.hover_t(&format!("end-{key}"), h);
                pen.shape(Shape::Rect(egui::epaint::RectShape::new(
                    r,
                    7.0,
                    rgba(255, 255, 255, (0.08 + 0.07 * k) * pen.o),
                    if self.target == Some(key) { Stroke::new(1.0, pen.a(dim())) } else { Stroke::NONE },
                    StrokeKind::Inside,
                )));
                let dot = pos2(r.left() + 6.0 + 8.0, r.center().y);
                pen.shape(Shape::circle_filled(dot, 8.0, pen.a(c)));
                pen.text(&g, pos2(dot.x + 8.0 + 6.0, r.center().y - g.size().y / 2.0), TEXT);
                if clicked {
                    acts.push(Act::Picker(if self.target == Some(key) { None } else { Some(key) }));
                }
                x = r.right() + 8.0;
                if key == "fadeFrom" {
                    let arrow = line(pen.ctx, "→", fonts::inter(400, 13.0), dim());
                    pen.text(&arrow, pos2(x, r.center().y - arrow.size().y / 2.0), dim());
                    x += arrow.size().x + 8.0;
                }
            }
            pen.y += 26.0;
        }
        if mode == "cover" {
            hint(pen, if cover.is_some() { "Matches the cover art." } else { "Shows when a song with a cover plays." });
        }

        // The picker: saturation/brightness square, hue bar, colour code.
        self.hex_rect = None;
        if self.target.is_some() {
            pen.y += 10.0;
            let sv = Rect::from_min_size(pos2(pen.x, pen.y), vec2(pen.w, 110.0));
            let hue = hsv_to_rgb(Hsv { h: self.hsv.h, s: 1.0, v: 1.0 });
            gradient_rrect(pen, sv, 8.0, &|u, v| {
                let top = mix(hue, u, Color32::WHITE);
                mix(Color32::BLACK, v, top)
            });
            let (h, held, _) = pen.interact("sv", sv, true);
            if held || (h && pen.ptr.pressed) {
                pen.cursor = Some(CursorIcon::Crosshair);
                if let Some(p) = pen.ptr.pos {
                    self.hsv.s = ((p.x - sv.left()) / sv.width()).clamp(0.0, 1.0);
                    self.hsv.v = 1.0 - ((p.y - sv.top()) / sv.height()).clamp(0.0, 1.0);
                    self.dragging = true;
                    let hx = to_hex(hsv_to_rgb(self.hsv));
                    self.hex_text = hx.to_uppercase();
                    self.hex_bad = false;
                    acts.push(Act::PickColour(hx, false));
                }
            } else if h {
                pen.cursor = Some(CursorIcon::Crosshair);
            }
            let cur = hsv_to_rgb(self.hsv);
            knob(pen, pos2(sv.left() + self.hsv.s * sv.width(), sv.top() + (1.0 - self.hsv.v) * sv.height()), cur);
            pen.y += 110.0 + 8.0;
            let hb = Rect::from_min_size(pos2(pen.x, pen.y), vec2(pen.w, 12.0));
            gradient_rrect(pen, hb, 6.0, &|u, _| hsv_to_rgb(Hsv { h: (u * 360.0).min(359.9), s: 1.0, v: 1.0 }));
            let (h, held, _) = pen.interact("hue", hb.expand2(vec2(0.0, 6.0)), true);
            if held || (h && pen.ptr.pressed) {
                if let Some(p) = pen.ptr.pos {
                    self.hsv.h = (((p.x - hb.left()) / hb.width()).clamp(0.0, 1.0) * 360.0).min(359.9);
                    self.dragging = true;
                    let hx = to_hex(hsv_to_rgb(self.hsv));
                    self.hex_text = hx.to_uppercase();
                    self.hex_bad = false;
                    acts.push(Act::PickColour(hx, false));
                }
            }
            knob(pen, pos2(hb.left() + self.hsv.h / 360.0 * hb.width(), hb.center().y), hsv_to_rgb(Hsv { h: self.hsv.h, s: 1.0, v: 1.0 }));
            pen.y += 12.0 + 8.0;
            let preview = Rect::from_min_size(pos2(pen.x, pen.y), vec2(22.0, 22.0));
            pen.shape(Shape::Rect(egui::epaint::RectShape::new(preview, 6.0, pen.a(cur), Stroke::new(1.0, rgba(255, 255, 255, 0.2 * pen.o)), StrokeKind::Inside)));
            let field = Rect::from_min_size(pos2(preview.right() + 8.0, pen.y + (22.0 - 25.0) / 2.0), vec2(90.0, 25.0));
            let focused = pen.ctx.memory(|m| m.has_focus(Id::unique("hex")));
            let ring = if self.hex_bad { Stroke::new(1.0, Color32::from_rgb(0xff, 0x4d, 0x5e)) } else if focused { Stroke::new(1.0, pen.a(dim())) } else { Stroke::NONE };
            pen.shape(Shape::Rect(egui::epaint::RectShape::new(field, 6.0, rgba(255, 255, 255, 0.08 * pen.o), ring, StrokeKind::Inside)));
            if pen.live {
                self.hex_rect = Some(field);
            }
            pen.y += 22.0 + 6.0;
        }
    }

    fn pane_song(&mut self, pen: &mut Pen, state: &Value, acts: &mut Vec<Act>) {
        let song = &state["song"];
        if song.is_null() {
            pen.y += 8.0;
            let g = text(pen.ctx, "Play a song to adjust its timing and lyrics.", fonts::inter(400, 13.0), dim(), Txt { wrap: pen.w, italic: true, ..Default::default() });
            pen.text(&g, pos2(pen.x, pen.y), dim());
            pen.y += g.size().y + 4.0;
            return;
        }
        let title = str_of(song, "title");
        let artist = str_of(song, "artist");
        let name = if artist.is_empty() { title.to_string() } else { format!("{title} · {artist}") };
        fonts::ensure(pen.ctx, &name);
        let g = text(pen.ctx, &name, fonts::inter(600, 13.0), TEXT, Txt { wrap: pen.w, rows: 1, ..Default::default() });
        pen.text(&g, pos2(pen.x, pen.y), TEXT);
        pen.y += g.size().y + 4.0;

        let lead = song["lead"].as_f64().unwrap_or(0.0);
        let lead_s = if lead == 0.0 { "in sync".to_string() } else { format!("{:.2} s {}", lead.abs(), if lead > 0.0 { "earlier" } else { "later" }) };
        pen.y += 4.0;
        label(pen, "Timing", Some(&lead_s));
        let synced = str_of(song, "status") == "found";
        buttons(pen, &[("earlier", "Earlier", false, true), ("later", "Later", false, true), ("sync", "Sync to now", false, synced), ("reset", "Reset", true, lead != 0.0)], acts);
        hint(pen, "“Sync to now”: press when the line on screen is sung.");
        pen.y += 6.0;

        let listening = song["listening"].as_bool().unwrap_or(false);
        let provider = song["provider"].as_str();
        let source = if listening {
            "listening…".to_string()
        } else if let Some(p) = provider {
            format!("from {p}")
        } else if str_of(song, "status") == "loading" {
            "looking…".into()
        } else {
            "none found".into()
        };
        pen.y += 4.0;
        label(pen, "Lyrics", Some(&source));
        let picked = song["picked"].as_bool().unwrap_or(false);
        buttons(
            pen,
            &[("prev", "‹ Previous", false, true), ("next", "Other lyrics ›", false, true), ("first", "Back to first", true, picked), ("search", "Search again", true, true), ("recognise", "Recognise song", true, !listening)],
            acts,
        );
        if let Some(r) = song["recognisedAs"].as_object() {
            let t = r.get("title").and_then(Value::as_str).unwrap_or("");
            let a = r.get("artist").and_then(Value::as_str).unwrap_or("");
            hint(pen, &format!("Recognised as {}", if a.is_empty() { t.to_string() } else { format!("{a} – {t}") }));
        }
        pen.y += 6.0;
    }
}

fn typed_hex(s: &str) -> Option<String> {
    let mut v = s.trim().to_lowercase();
    if !v.starts_with('#') {
        v = format!("#{v}");
    }
    if v.len() == 4 && v[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        v = format!("#{}", v[1..].chars().map(|c| format!("{c}{c}")).collect::<String>());
    }
    (v.len() == 7 && v[1..].chars().all(|c| c.is_ascii_hexdigit())).then_some(v)
}

/* ---------- Widgets ---------- */
fn label(pen: &mut Pen, name: &str, value: Option<&str>) {
    let g = line(pen.ctx, name, fonts::inter(400, 13.0), dim());
    pen.text(&g, pos2(pen.x, pen.y), dim());
    if let Some(v) = value {
        let vg = line(pen.ctx, v, fonts::inter(400, 13.0), TEXT);
        pen.text(&vg, pos2(pen.x + g.size().x + 4.0, pen.y), TEXT);
    }
    pen.y += 17.5 + 5.0;
}

fn hint(pen: &mut Pen, s: &str) {
    pen.y += 5.0;
    let g = text(pen.ctx, s, fonts::inter(400, 11.0), dim(), Txt { wrap: pen.w, ..Default::default() });
    pen.text(&g, pos2(pen.x, pen.y), dim());
    pen.y += g.size().y;
}

fn group(pen: &mut Pen, name: &str, first: bool) {
    if !first {
        pen.y += 14.0;
        pen.shape(Shape::rect_filled(Rect::from_min_size(pos2(pen.x, pen.y), vec2(pen.w, 1.0)), 0.0, alpha(line_c(), pen.o)));
        pen.y += 12.0;
    } else {
        pen.y += 6.0;
    }
    let g = text(pen.ctx, &name.to_uppercase(), fonts::inter(800, 11.0), dim(), Txt { spacing: 0.66, ..Default::default() });
    pen.text(&g, pos2(pen.x, pen.y), dim());
    pen.y += g.size().y + 2.0;
}

fn toggle_row(pen: &mut Pen, key: &str, name: &str, sub: bool, kbd: Option<&str>, on: bool, enabled: bool) -> bool {
    let r = Rect::from_min_size(pos2(pen.x, pen.y), vec2(pen.w, 28.0));
    // Only the switch toggles; the label is just a label.
    let sr = Rect::from_min_size(pos2(pen.x + pen.w - 32.0, r.center().y - 9.0), vec2(32.0, 18.0));
    let (_, _, clicked) = pen.interact(&format!("tg-{key}"), sr.expand(5.0), enabled);
    let fade = if enabled { 1.0 } else { 0.4 };
    let o = pen.o;
    pen.o *= fade;
    let x = pen.x + if sub { 14.0 } else { 0.0 };
    let col = if sub { dim() } else { TEXT };
    let g = line(pen.ctx, name, fonts::inter(400, 13.0), col);
    pen.text(&g, pos2(x, r.center().y - g.size().y / 2.0), col);
    if let Some(k) = kbd {
        let kg = line(pen.ctx, k, fonts::inter(400, 10.0), dim());
        let kr = Rect::from_min_size(pos2(x + g.size().x + 6.0, r.center().y - kg.size().y / 2.0 - 1.0), kg.size() + vec2(10.0, 2.0));
        pen.shape(Shape::Rect(egui::epaint::RectShape::new(kr, 4.0, Color32::TRANSPARENT, Stroke::new(1.0, alpha(line_c(), pen.o)), StrokeKind::Inside)));
        pen.text(&kg, kr.min + vec2(5.0, 1.0), dim());
    }
    // The switch.
    let k = tween(pen.ctx, &format!("sw-{key}"), if on { 1.0 } else { 0.0 }, 0.2);
    pen.shape(Shape::rect_filled(sr, 9.0, pen.a(mix(pen.accent, k, rgba(255, 255, 255, 0.16)))));
    let knob_c = mix(Color32::from_rgb(0x16, 0x16, 0x1b), k, Color32::WHITE);
    pen.shape(Shape::circle_filled(pos2(sr.left() + 9.0 + 14.0 * k, sr.center().y), 7.0, pen.a(knob_c)));
    pen.o = o;
    pen.y += 28.0;
    clicked
}

/// "Version x is out" with an Update button, at the top of the Show tab while there is a newer release.
fn update_banner(pen: &mut Pen, u: &Value, acts: &mut Vec<Act>) {
    if u.is_null() {
        return;
    }
    let state = str_of(u, "state");
    let title = match state {
        "downloading" => format!("Updating to {}…", str_of(u, "version")),
        "failed" => "Update didn't work".to_string(),
        _ => format!("Version {} is out", str_of(u, "version")),
    };
    let detail = match state {
        "failed" => str_of(u, "error").to_string(),
        "downloading" => "Restarts by itself when it's done.".to_string(),
        _ => "Downloads it and restarts. Settings stay.".to_string(),
    };
    pen.y += 6.0;
    let top = pen.y;
    let pad = 10.0;
    let tg = line(pen.ctx, &title, fonts::inter(600, 13.0), TEXT);
    let dg = text(pen.ctx, &detail, fonts::inter(400, 11.0), dim(), Txt { wrap: pen.w - 2.0 * pad, ..Default::default() });
    let main_label = if state == "failed" { "Try again" } else { "Update" };
    let bs = button_size(pen.ctx, main_label);
    let ps = button_size(pen.ctx, "Open page");
    let h = pad + tg.size().y + 4.0 + dg.size().y + 8.0 + bs.y + pad;
    let r = Rect::from_min_size(pos2(pen.x, top), vec2(pen.w, h));
    pen.shape(Shape::Rect(egui::epaint::RectShape::new(
        r,
        9.0,
        alpha(pen.accent, 0.12 * pen.o),
        Stroke::new(1.0, alpha(pen.accent, 0.45 * pen.o)),
        StrokeKind::Inside,
    )));
    let mut y = top + pad;
    pen.text(&tg, pos2(pen.x + pad, y), TEXT);
    y += tg.size().y + 4.0;
    pen.text(&dg, pos2(pen.x + pad, y), dim());
    y += dg.size().y + 8.0;
    let br = Rect::from_min_size(pos2(pen.x + pad, y), bs);
    if button(pen, "upd-now", main_label, br, false, state != "downloading", true) {
        acts.push(Act::Core("update-now", Value::Null));
    }
    if state == "failed" {
        let pr = Rect::from_min_size(pos2(br.right() + 6.0, y), ps);
        if button(pen, "upd-page", "Open page", pr, true, true, false) {
            acts.push(Act::Core("update-page", u["page"].clone()));
        }
    }
    pen.y = r.bottom() + 6.0;
}

fn button_size(ctx: &Context, label: &str) -> Vec2 {
    let g = line(ctx, label, fonts::inter(600, 12.0), TEXT);
    vec2(g.size().x + 20.0, g.size().y + 12.0)
}

fn button(pen: &mut Pen, id: &str, label: &str, r: Rect, quiet: bool, enabled: bool, on: bool) -> bool {
    button_styled(pen, id, label, r, quiet, enabled, on, None)
}

#[allow(clippy::too_many_arguments)]
fn button_styled(pen: &mut Pen, id: &str, label: &str, r: Rect, quiet: bool, enabled: bool, on: bool, red: Option<Color32>) -> bool {
    let (h, _, clicked) = pen.interact(id, r, enabled);
    let k = pen.hover_t(id, h);
    let o = if enabled { 1.0 } else { 0.35 };
    let (fill, col) = if let Some(red) = red {
        (red, Color32::WHITE)
    } else if on {
        (pen.accent, Color32::from_rgb(0x11, 0x11, 0x11))
    } else if quiet {
        (rgba(255, 255, 255, 0.08 * k), mix(TEXT, k, dim()))
    } else {
        (rgba(255, 255, 255, 0.08 + 0.07 * k), TEXT)
    };
    pen.shape(Shape::rect_filled(r, 7.0, alpha(fill, pen.o * o)));
    let g = line(pen.ctx, label, fonts::inter(600, 12.0), col);
    let saved = pen.o;
    pen.o *= o;
    pen.text(&g, r.center() - g.size() / 2.0, col);
    pen.o = saved;
    clicked
}

/// A row of choices, the chosen one lit. Fonts show in their own font.
fn chips(pen: &mut Pen, key: &str, list: &Value, chosen: &str, acts: &mut Vec<Act>) {
    let mut x = pen.x;
    let mut y = pen.y;
    let mut row_h: f32 = 0.0;
    for item in list.as_array().into_iter().flatten() {
        let (v, lbl) = (item[0].as_str().unwrap_or(""), item[1].as_str().unwrap_or(""));
        let (font, spacing, pad): (FontId, f32, Vec2) = if key == "font" {
            match v {
                "serif" => (FontId::new(15.0, fonts::family("serif", 400, true)), 0.0, vec2(10.0, 4.0)),
                "pixel" => (FontId::new(12.0, fonts::family("pixel", 600, false)), 0.0, vec2(10.0, 6.0)),
                "bold" => (FontId::new(15.0, fonts::family("bold", 400, false)), 0.6, vec2(10.0, 5.5)),
                "round" => (FontId::new(12.0, fonts::family("round", 800, false)), 0.0, vec2(10.0, 6.0)),
                "hand" => (FontId::new(16.0, fonts::family("hand", 700, false)), 0.0, vec2(10.0, 4.0)),
                _ => (fonts::inter(600, 12.0), 0.0, vec2(10.0, 6.0)),
            }
        } else {
            (fonts::inter(600, 12.0), 0.0, vec2(10.0, 6.0))
        };
        let g = text(pen.ctx, lbl, font, TEXT, Txt { spacing, ..Default::default() });
        let size = g.size() + pad * 2.0;
        let size = vec2(size.x, size.y.max(27.0));
        if x + size.x > pen.x + pen.w + 0.5 && x > pen.x {
            x = pen.x;
            y += row_h + 5.0;
            row_h = 0.0;
        }
        let r = Rect::from_min_size(pos2(x, y), size);
        let id = format!("chip-{key}-{v}");
        let (h, _, clicked) = pen.interact(&id, r, true);
        let k = pen.hover_t(&id, h);
        let on = v == chosen;
        let (fill, col) = if on { (pen.accent, Color32::from_rgb(0x11, 0x11, 0x11)) } else { (rgba(255, 255, 255, 0.08 + 0.07 * k), TEXT) };
        pen.shape(Shape::rect_filled(r, 7.0, alpha(fill, pen.o)));
        pen.text(&g, r.center() - g.size() / 2.0, col);
        if clicked {
            acts.push(Act::Core("choose", json!({ "key": key, "value": v })));
        }
        x += size.x + 5.0;
        row_h = row_h.max(size.y);
    }
    pen.y = y + row_h;
}

fn buttons(pen: &mut Pen, list: &[(&'static str, &str, bool, bool)], acts: &mut Vec<Act>) {
    let mut x = pen.x;
    let mut y = pen.y;
    let mut h: f32 = 0.0;
    for (act, lbl, quiet, enabled) in list {
        let size = button_size(pen.ctx, lbl);
        if x + size.x > pen.x + pen.w + 0.5 && x > pen.x {
            x = pen.x;
            y += h + 5.0;
        }
        let r = Rect::from_min_size(pos2(x, y), size);
        if button(pen, &format!("act-{act}"), lbl, r, *quiet, *enabled, false) {
            acts.push(Act::Core(act, Value::Null));
        }
        x += size.x + 5.0;
        h = h.max(size.y);
    }
    pen.y = y + h;
}

fn knob(pen: &mut Pen, c: Pos2, fill: Color32) {
    pen.shape(Shape::circle_filled(c, 10.0, rgba(0, 0, 0, 0.35 * pen.o)));
    pen.shape(Shape::circle_filled(c, 9.0, pen.a(Color32::WHITE)));
    pen.shape(Shape::circle_filled(c, 7.0, pen.a(fill)));
}

fn circle_pts(c: Pos2, r: f32) -> Vec<Pos2> {
    (0..=48).map(|i| {
        let a = i as f32 / 48.0 * std::f32::consts::TAU;
        c + vec2(a.cos(), a.sin()) * r
    }).collect()
}

/// Anti-aliases the edge of a hand-built mesh the way egui feathers its own shapes: a strip one physical pixel wide
/// around the closed convex outline `pts`, fading from each point's colour to transparent. Build the fill half a pixel
/// inside the real edge so the strip straddles it.
fn feather(mesh: &mut Mesh, pts: &[(Pos2, Color32)], f: f32) {
    let mut pts: Vec<(Pos2, Color32)> = pts.to_vec();
    pts.dedup_by(|a, b| a.0.distance(b.0) < 1e-3);
    while pts.len() > 2 && pts[0].0.distance(pts[pts.len() - 1].0) < 1e-3 {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        return;
    }
    let mid = pts.iter().fold(Vec2::ZERO, |s, p| s + p.0.to_vec2()) / n as f32;
    let base = mesh.vertices.len() as u32;
    for i in 0..n {
        let (prev, (p, col), next) = (pts[(i + n - 1) % n].0, pts[i], pts[(i + 1) % n].0);
        let d = next - prev;
        let mut nrm = vec2(d.y, -d.x).normalized();
        if nrm.dot(p.to_vec2() - mid) < 0.0 {
            nrm = -nrm;
        }
        mesh.colored_vertex(p, col);
        mesh.colored_vertex(p + nrm * f, Color32::TRANSPARENT);
    }
    for i in 0..n as u32 {
        let j = (i + 1) % n as u32;
        let (a, b, c, d) = (base + 2 * i, base + 2 * i + 1, base + 2 * j, base + 2 * j + 1);
        mesh.add_triangle(a, b, d);
        mesh.add_triangle(a, d, c);
    }
}

/// One physical pixel, in points: the width of the anti-aliased edge.
fn px(pen: &Pen) -> f32 {
    1.0 / pen.ctx.pixels_per_point()
}

/// A colour wheel (conic gradient from the top, clockwise).
fn conic(pen: &mut Pen, c: Pos2, r: f32) {
    let f = px(pen);
    let r = r - f / 2.0;
    let mut mesh = Mesh::default();
    let mut rim = Vec::new();
    let n = 64;
    let at = |k: f32| -> Color32 {
        let f = k * (HUES.len() - 1) as f32;
        let i = (f.floor() as usize).min(HUES.len() - 2);
        mix(c32(HUES[i + 1]), f - i as f32, c32(HUES[i]))
    };
    for i in 0..n {
        let (k0, k1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
        let p = |k: f32| c + vec2((k * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2).cos(), (k * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2).sin()) * r;
        let base = mesh.vertices.len() as u32;
        let col = pen.a(at((k0 + k1) / 2.0));
        mesh.colored_vertex(c, col);
        mesh.colored_vertex(p(k0), pen.a(at(k0)));
        mesh.colored_vertex(p(k1), pen.a(at(k1)));
        mesh.add_triangle(base, base + 1, base + 2);
        rim.push((p(k0), pen.a(at(k0))));
    }
    feather(&mut mesh, &rim, f);
    pen.shape(Shape::mesh(mesh));
}

/// A circle filled with a 135° gradient from `a` (15%) to `b` (85%).
fn diag(pen: &mut Pen, c: Pos2, r: f32, a: Color32, b: Color32) {
    let f = px(pen);
    let r = r - f / 2.0;
    let mut mesh = Mesh::default();
    let mut rim = Vec::new();
    let col = |p: Pos2| {
        let t = ((p.x - c.x) + (p.y - c.y)) / (2.0 * r) * std::f32::consts::FRAC_1_SQRT_2 * 2.0 / 2.0 + 0.5;
        let t = ((t - 0.15) / 0.7).clamp(0.0, 1.0);
        mix(b, t, a)
    };
    let n = 64;
    let centre = mesh.vertices.len() as u32;
    mesh.colored_vertex(c, pen.a(col(c)));
    for i in 0..=n {
        let k = i as f32 / n as f32 * std::f32::consts::TAU;
        let p = c + vec2(k.cos(), k.sin()) * r;
        mesh.colored_vertex(p, pen.a(col(p)));
        if i > 0 {
            mesh.add_triangle(centre, centre + i, centre + i + 1);
        }
        if i < n {
            rim.push((p, pen.a(col(p))));
        }
    }
    feather(&mut mesh, &rim, f);
    pen.shape(Shape::mesh(mesh));
}

/// A rounded rectangle filled by `f(u, v)` (0..1 across and down), as thin vertical strips.
fn gradient_rrect(pen: &mut Pen, full: Rect, radius: f32, f: &dyn Fn(f32, f32) -> Color32) {
    let fw = px(pen);
    // The fill sits half a pixel inside; the feathered rim straddles the real edge.
    let r = full.shrink(fw / 2.0);
    let radius = (radius - fw / 2.0).max(0.0).min(r.width() / 2.0).min(r.height() / 2.0);
    let colour = |p: Pos2| pen.a(f((p.x - full.left()) / full.width(), (p.y - full.top()) / full.height()));
    let mut mesh = Mesh::default();
    let inset = |x: f32| -> f32 {
        let d = if x < r.left() + radius { r.left() + radius - x } else if x > r.right() - radius { x - (r.right() - radius) } else { 0.0 };
        radius - (radius * radius - d * d).max(0.0).sqrt()
    };
    // Columns: evenly by angle round the corners (so they stay round), every 3 points along the straight middle.
    let mut xs = Vec::new();
    let steps = 12;
    for i in 0..steps {
        let a = std::f32::consts::FRAC_PI_2 * i as f32 / steps as f32;
        xs.push(r.left() + radius - radius * a.cos());
    }
    let mid = ((r.width() - 2.0 * radius) / 3.0).ceil().max(1.0) as usize;
    for i in 0..=mid {
        xs.push(r.left() + radius + (r.width() - 2.0 * radius) * i as f32 / mid as f32);
    }
    for i in (0..steps).rev() {
        let a = std::f32::consts::FRAC_PI_2 * i as f32 / steps as f32;
        xs.push(r.right() - radius + radius * a.cos());
    }
    xs.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    let rows = 6;
    let (mut top, mut bottom) = (Vec::new(), Vec::new());
    for (i, &x) in xs.iter().enumerate() {
        let (t, b) = (r.top() + inset(x), r.bottom() - inset(x));
        for j in 0..=rows {
            let p = pos2(x, t + (b - t) * j as f32 / rows as f32);
            mesh.colored_vertex(p, colour(p));
        }
        top.push((pos2(x, t), colour(pos2(x, t))));
        bottom.push((pos2(x, b), colour(pos2(x, b))));
        if i > 0 {
            let cols = (rows + 1) as u32;
            let (a0, b0) = ((i as u32 - 1) * cols, i as u32 * cols);
            for j in 0..rows as u32 {
                mesh.add_triangle(a0 + j, b0 + j, b0 + j + 1);
                mesh.add_triangle(a0 + j, b0 + j + 1, a0 + j + 1);
            }
        }
    }
    bottom.reverse();
    top.extend(bottom);
    feather(&mut mesh, &top, fw);
    pen.shape(Shape::mesh(mesh));
}
