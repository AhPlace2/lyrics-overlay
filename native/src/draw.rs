// Drawing helpers shared by the overlay and the settings panel: text, text as a mesh that can be moved, scaled and
// recoloured per word, soft shadows, the card's icons, keyframe animation and a small hit-testing helper.

use egui::epaint::{Mesh, Vertex};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{pos2, vec2, Align, Color32, Context, FontId, Galley, Pos2, Rect, Shape, Vec2};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Txt {
    pub wrap: f32,
    /// 0 for any number of rows; 1 cuts the text off with "…".
    pub rows: usize,
    pub spacing: f32,
    pub line_height: Option<f32>,
    pub italic: bool,
    pub align: Align,
}
impl Default for Txt {
    fn default() -> Self {
        Txt { wrap: f32::INFINITY, rows: 0, spacing: 0.0, line_height: None, italic: false, align: Align::LEFT }
    }
}

pub fn text(ctx: &Context, s: &str, font: FontId, color: Color32, t: Txt) -> Arc<Galley> {
    let format = TextFormat {
        font_id: font,
        extra_letter_spacing: t.spacing,
        line_height: t.line_height,
        color,
        italics: t.italic,
        ..Default::default()
    };
    let mut job = LayoutJob::single_section(s.to_string(), format);
    job.wrap = if t.rows == 1 {
        TextWrapping::truncate_at_width(t.wrap)
    } else {
        TextWrapping { max_width: t.wrap, max_rows: if t.rows == 0 { usize::MAX } else { t.rows }, ..Default::default() }
    };
    job.halign = t.align;
    ctx.fonts_mut(|f| f.layout_job(job))
}

/// One line of text, no wrapping.
pub fn line(ctx: &Context, s: &str, font: FontId, color: Color32) -> Arc<Galley> {
    text(ctx, s, font, color, Txt::default())
}

/// Multiplier that turns the font atlas' texel coordinates into 0..1 uv. Read it after all text of the frame is
/// laid out, since laying out new glyphs can grow the atlas.
pub fn uv_scale(ctx: &Context) -> Vec2 {
    let [w, h] = ctx.fonts(|f| f.font_image_size());
    vec2(1.0 / w as f32, 1.0 / h as f32)
}

/// Adds a galley's glyphs to a mesh at `pos`, each point passed through `xf`, all in one colour.
pub fn galley_into(mesh: &mut Mesh, galley: &Galley, pos: Pos2, color: Color32, uv: Vec2, xf: &dyn Fn(Pos2) -> Pos2) {
    for row in &galley.rows {
        let rp = pos + row.pos.to_vec2();
        let base = mesh.vertices.len() as u32;
        mesh.indices.extend(row.visuals.mesh.indices.iter().map(|i| i + base));
        mesh.vertices.extend(row.visuals.mesh.vertices.iter().map(|v| Vertex {
            pos: xf(rp + v.pos.to_vec2()),
            uv: pos2(v.uv.x * uv.x, v.uv.y * uv.y),
            color,
        }));
    }
}

/// A blurred copy of text (a CSS text-shadow), made of copies around a ring. `blur` is the CSS blur radius.
pub fn soft_text(mesh: &mut Mesh, galley: &Galley, pos: Pos2, offset: Vec2, blur: f32, color: Color32, uv: Vec2, xf: &dyn Fn(Pos2) -> Pos2) {
    if color.a() == 0 {
        return;
    }
    if blur < 1.0 {
        galley_into(mesh, galley, pos + offset, color, uv, xf);
        return;
    }
    // Two rings: a tight one and a wide one, sharing the alpha so the total reads like a gaussian.
    let rings: [(f32, usize, f32); 2] = [(blur * 0.25, 6, 0.55), (blur * 0.55, 10, 0.45)];
    let a = color.a() as f32 / 255.0;
    for (r, n, share) in rings {
        // Each copy's alpha so that n copies stacked over each other reach about `share` of the colour.
        let each = 1.0 - (1.0 - (a * share * 1.6).min(0.95)).powf(1.0 / n as f32);
        let c = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (each * 255.0).round() as u8);
        for k in 0..n {
            let ang = k as f32 / n as f32 * std::f32::consts::TAU;
            galley_into(mesh, galley, pos + offset + vec2(ang.cos(), ang.sin()) * r, c, uv, xf);
        }
    }
}

/// A rounded rectangle's outline as points, for dashed outlines.
pub fn rounded_path(r: Rect, radius: f32) -> Vec<Pos2> {
    let mut pts = Vec::new();
    let rad = radius.min(r.width() / 2.0).min(r.height() / 2.0);
    let corners = [
        (pos2(r.right() - rad, r.top() + rad), -90.0f32),
        (pos2(r.right() - rad, r.bottom() - rad), 0.0),
        (pos2(r.left() + rad, r.bottom() - rad), 90.0),
        (pos2(r.left() + rad, r.top() + rad), 180.0),
    ];
    for (c, start) in corners {
        for i in 0..=6 {
            let a = (start + i as f32 * 15.0).to_radians();
            pts.push(c + vec2(a.cos(), a.sin()) * rad);
        }
    }
    pts.push(pts[0]);
    pts
}

pub fn dashed_rect(r: Rect, radius: f32, width: f32, color: Color32, dash: f32, gap: f32) -> Vec<Shape> {
    Shape::dashed_line(&rounded_path(r, radius), egui::Stroke::new(width, color), dash, gap)
}

/// A shape made of points in a 24×24 box (the SVG viewBox), placed in `r`.
pub fn icon_poly(r: Rect, pts: &[(f32, f32)], color: Color32) -> Shape {
    let s = r.width() / 24.0;
    Shape::convex_polygon(pts.iter().map(|(x, y)| r.min + vec2(x * s, y * s)).collect(), color, egui::Stroke::NONE)
}

pub fn icon_prev(r: Rect, c: Color32) -> Vec<Shape> {
    vec![icon_poly(r, &[(7.0, 5.0), (9.0, 5.0), (9.0, 19.0), (7.0, 19.0)], c), icon_poly(r, &[(19.0, 5.2), (19.0, 18.8), (9.6, 12.0)], c)]
}
pub fn icon_next(r: Rect, c: Color32) -> Vec<Shape> {
    vec![icon_poly(r, &[(15.0, 5.0), (17.0, 5.0), (17.0, 19.0), (15.0, 19.0)], c), icon_poly(r, &[(5.0, 5.2), (14.4, 12.0), (5.0, 18.8)], c)]
}
pub fn icon_play(r: Rect, c: Color32) -> Vec<Shape> {
    vec![icon_poly(r, &[(8.0, 5.0), (19.8, 12.0), (8.0, 19.0)], c)]
}
pub fn icon_pause(r: Rect, c: Color32) -> Vec<Shape> {
    vec![icon_poly(r, &[(7.0, 5.0), (10.5, 5.0), (10.5, 19.0), (7.0, 19.0)], c), icon_poly(r, &[(13.5, 5.0), (17.0, 5.0), (17.0, 19.0), (13.5, 19.0)], c)]
}
/// The settings gear: a ring with eight teeth, turned by `turn` radians.
pub fn icon_gear(r: Rect, c: Color32, turn: f32) -> Vec<Shape> {
    let s = r.width() / 24.0;
    let centre = r.center();
    let mut out = vec![Shape::circle_stroke(centre, 5.0 * s, egui::Stroke::new(3.6 * s, c))];
    for k in 0..8 {
        let a = turn + k as f32 * std::f32::consts::TAU / 8.0;
        let (dir, side) = (vec2(a.cos(), a.sin()), vec2(-a.sin(), a.cos()));
        let pts = [
            centre + dir * 5.8 * s - side * 2.2 * s,
            centre + dir * 9.0 * s - side * 1.6 * s,
            centre + dir * 9.0 * s + side * 1.6 * s,
            centre + dir * 5.8 * s + side * 2.2 * s,
        ];
        out.push(Shape::convex_polygon(pts.to_vec(), c, egui::Stroke::NONE));
    }
    out
}

/// Keyframes: (offset 0..1, values). Returns the values at `t` (0..1), straight lines between keyframes.
pub fn keyframes<const N: usize>(frames: &[(f32, [f32; N])], t: f32) -> [f32; N] {
    let t = t.clamp(0.0, 1.0);
    for w in frames.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.0 {
            let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 1.0 };
            let mut out = [0.0; N];
            for i in 0..N {
                out[i] = a.1[i] + (b.1[i] - a.1[i]) * k;
            }
            return out;
        }
    }
    frames.last().unwrap().1
}

/// CSS cubic-bezier(x1, y1, x2, y2) at progress x.
pub fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let f = |t: f32, a: f32, b: f32| 3.0 * (1.0 - t) * (1.0 - t) * t * a + 3.0 * (1.0 - t) * t * t * b + t * t * t;
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if f(mid, x1, x2) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    f((lo + hi) / 2.0, y1, y2)
}
pub fn ease(x: f32) -> f32 {
    bezier(0.25, 0.1, 0.25, 1.0, x)
}

/// What the pointer did this frame.
#[derive(Default, Clone, Copy)]
pub struct Pointer {
    pub pos: Option<Pos2>,
    pub pressed: bool,
    pub released: bool,
    pub down: bool,
    pub secondary_pressed: bool,
}
impl Pointer {
    pub fn read(ctx: &Context) -> Self {
        ctx.input(|i| Pointer {
            pos: i.pointer.latest_pos().filter(|_| i.pointer.has_pointer()),
            pressed: i.pointer.primary_pressed(),
            released: i.pointer.primary_released(),
            down: i.pointer.primary_down(),
            secondary_pressed: i.pointer.secondary_pressed(),
        })
    }
    pub fn over(&self, r: Rect) -> bool {
        self.pos.map(|p| r.contains(p)).unwrap_or(false)
    }
}

/// Animates `id` towards `target` over `secs` (linear), returning the value now.
pub fn tween(ctx: &Context, id: &str, target: f32, secs: f32) -> f32 {
    ctx.animate_value_with_time(egui::Id::new(id), target, secs)
}

/* ---------- Words as cached pictures ---------- */
//
// A word in the big lyrics is many copies of its text: soft shadows, depth copies, the face. Drawing all of them
// every frame is most of what the overlay costs, so each stack of copies is drawn once into a picture, with a real
// Gaussian blur for soft shadows, and the picture is then moved, scaled and faded like the word.

/// One copy of the text in a stack: colour (with alpha), offset and CSS blur radius, in points.
#[derive(Clone, Copy)]
pub struct Layer {
    pub color: Color32,
    pub offset: Vec2,
    pub blur: f32,
}

struct Entry {
    tex: egui::TextureHandle,
    rect: Rect,
    used: u64,
}

#[derive(Default)]
struct Sprites {
    map: std::collections::HashMap<u64, Entry>,
    frame: u64,
    atlas: Option<egui::ColorImage>,
}

thread_local! {
    static SPRITES: std::cell::RefCell<Sprites> = std::cell::RefCell::new(Sprites::default());
}

/// Forgets pictures nobody drew in the last two frames. Call once per frame.
pub fn sprites_frame_end() {
    SPRITES.with(|s| {
        let mut s = s.borrow_mut();
        s.frame += 1;
        let f = s.frame;
        s.map.retain(|_, e| e.used + 2 >= f);
        s.atlas = None;
    });
}

fn sprite_key(galley: &Galley, layers: &[Layer], ppp: f32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    galley.text().hash(&mut h);
    for s in &galley.job.sections {
        s.format.font_id.hash(&mut h);
    }
    for v in [galley.rect.min.x, galley.rect.min.y, galley.rect.max.x, galley.rect.max.y, ppp] {
        v.to_bits().hash(&mut h);
    }
    for l in layers {
        l.color.to_array().hash(&mut h);
        for v in [l.offset.x, l.offset.y, l.blur] {
            v.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// The stack of copies as a mesh: one picture, placed at `pos`, passed through `xf`, multiplied by `tint`
/// (premultiplied; white keeps the colours as they are).
pub fn sprite(ctx: &Context, galley: &Galley, pos: Pos2, xf: egui::emath::TSTransform, layers: &[Layer], tint: Color32) -> Option<Shape> {
    if layers.is_empty() || tint == Color32::TRANSPARENT || galley.is_empty() {
        return None;
    }
    let ppp = ctx.pixels_per_point();
    let key = sprite_key(galley, layers, ppp);
    let (id, rect) = SPRITES.with(|s| {
        let mut s = s.borrow_mut();
        let frame = s.frame;
        if let Some(e) = s.map.get_mut(&key) {
            e.used = frame;
            return Some((e.tex.id(), e.rect));
        }
        if s.atlas.is_none() {
            s.atlas = Some(ctx.fonts(|f| f.image()));
        }
        let (img, rect) = build_sprite(s.atlas.as_ref().unwrap(), galley, layers, ppp)?;
        let tex = ctx.load_texture("word", img, egui::TextureOptions::LINEAR);
        let id = tex.id();
        s.map.insert(key, Entry { tex, rect, used: frame });
        Some((id, rect))
    })?;
    let mut mesh = Mesh::with_texture(id);
    mesh.add_rect_with_uv(rect.translate(pos.to_vec2()), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
    for v in &mut mesh.vertices {
        v.pos = xf * v.pos;
    }
    Some(Shape::mesh(mesh))
}

/// Draws the stack into a premultiplied picture. Returns it and where it sits relative to the galley origin.
fn build_sprite(atlas: &egui::ColorImage, galley: &Galley, layers: &[Layer], ppp: f32) -> Option<(egui::ColorImage, Rect)> {
    let pad = layers.iter().map(|l| l.offset.x.abs().max(l.offset.y.abs()) + l.blur * 1.5).fold(0.0f32, f32::max) + 2.0;
    let rect = galley.rect.expand(pad);
    let (w, h) = ((rect.width() * ppp).ceil() as usize, (rect.height() * ppp).ceil() as usize);
    if w == 0 || h == 0 || w * h > 16_000_000 {
        return None;
    }
    // The text coverage, copied glyph by glyph from the font atlas (glyphs are stored at screen resolution).
    let mut mask = vec![0u8; w * h];
    let (aw, ah) = (atlas.size[0] as isize, atlas.size[1] as isize);
    for row in &galley.rows {
        for q in row.visuals.mesh.vertices.chunks_exact(4) {
            let (mut p0, mut p1, mut u0, mut u1) = (pos2(f32::MAX, f32::MAX), pos2(f32::MIN, f32::MIN), pos2(f32::MAX, f32::MAX), pos2(f32::MIN, f32::MIN));
            for v in q {
                let p = row.pos + v.pos.to_vec2();
                p0 = p0.min(p);
                p1 = p1.max(p);
                u0 = u0.min(v.uv);
                u1 = u1.max(v.uv);
            }
            let (gw, gh) = ((u1.x - u0.x).round() as isize, (u1.y - u0.y).round() as isize);
            let (ux, uy) = (u0.x.round() as isize, u0.y.round() as isize);
            let x0 = ((p0.x - rect.min.x) * ppp).round() as isize;
            let y0 = ((p0.y - rect.min.y) * ppp).round() as isize;
            // Glyphs are drawn at the size they are stored; stepping through the atlas covers any rounding.
            let (sw, sh) = (((p1.x - p0.x) * ppp).round().max(1.0) as isize, ((p1.y - p0.y) * ppp).round().max(1.0) as isize);
            for j in 0..sh {
                let y = y0 + j;
                let ty = uy + j * gh / sh;
                if y < 0 || y >= h as isize || ty < 0 || ty >= ah {
                    continue;
                }
                for i in 0..sw {
                    let x = x0 + i;
                    let tx = ux + i * gw / sw;
                    if x < 0 || x >= w as isize || tx < 0 || tx >= aw {
                        continue;
                    }
                    let a = atlas.pixels[(ty * aw + tx) as usize].a();
                    let d = &mut mask[y as usize * w + x as usize];
                    *d = (*d).max(a);
                }
            }
        }
    }
    // Each layer over the ones before it, premultiplied, in gamma space like the rest of the drawing.
    let mut out = vec![[0.0f32; 4]; w * h];
    let mut blurred: Vec<(u32, Vec<f32>)> = Vec::new();
    let plain: Vec<f32> = mask.iter().map(|&a| a as f32 / 255.0).collect();
    for l in layers {
        let src: &[f32] = if l.blur >= 0.5 {
            let k = l.blur.to_bits();
            if !blurred.iter().any(|(b, _)| *b == k) {
                blurred.push((k, gaussian(&plain, w, h, l.blur * 0.5 * ppp)));
            }
            &blurred.iter().find(|(b, _)| *b == k).unwrap().1
        } else {
            &plain
        };
        let col = [l.color.r() as f32 / 255.0, l.color.g() as f32 / 255.0, l.color.b() as f32 / 255.0, l.color.a() as f32 / 255.0];
        let (ox, oy) = ((l.offset.x * ppp).round() as isize, (l.offset.y * ppp).round() as isize);
        for y in 0..h as isize {
            let sy = y - oy;
            if sy < 0 || sy >= h as isize {
                continue;
            }
            for x in 0..w as isize {
                let sx = x - ox;
                if sx < 0 || sx >= w as isize {
                    continue;
                }
                let cov = src[sy as usize * w + sx as usize];
                if cov <= 0.0 {
                    continue;
                }
                let o = &mut out[y as usize * w + x as usize];
                let inv = 1.0 - col[3] * cov;
                for i in 0..4 {
                    o[i] = col[i] * cov + o[i] * inv;
                }
            }
        }
    }
    let q = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    let px: Vec<Color32> = out.iter().map(|o| Color32::from_rgba_premultiplied(q(o[0]), q(o[1]), q(o[2]), q(o[3]))).collect();
    let rect = Rect::from_min_size(rect.min, vec2(w as f32 / ppp, h as f32 / ppp));
    Some((egui::ColorImage::new([w, h], px), rect))
}

/// A Gaussian blur, as three box blurs each way.
fn gaussian(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let mut a = src.to_vec();
    let r = (((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
    let mut tmp = vec![0.0f32; w.max(h)];
    for _ in 0..3 {
        for y in 0..h {
            box_line(&mut a[y * w..], 1, w, r, &mut tmp);
        }
        for x in 0..w {
            box_line(&mut a[x..], w, h, r, &mut tmp);
        }
    }
    a
}

/// A box blur of radius `r` along `n` values `step` apart; outside counts as empty.
fn box_line(a: &mut [f32], step: usize, n: usize, r: usize, tmp: &mut [f32]) {
    let k = 1.0 / (2 * r + 1) as f32;
    let get = |a: &[f32], i: isize| if i < 0 || i >= n as isize { 0.0 } else { a[i as usize * step] };
    let mut sum = 0.0;
    for i in -(r as isize)..=(r as isize) {
        sum += get(a, i);
    }
    for (i, t) in tmp.iter_mut().enumerate().take(n) {
        *t = sum * k;
        sum += get(a, i as isize + r as isize + 1) - get(a, i as isize - r as isize);
    }
    for (i, t) in tmp.iter().enumerate().take(n) {
        a[i * step] = *t;
    }
}
