// Draws egui's triangles on the CPU into a bitmap and hands it to Windows as a layered window. No graphics driver
// is loaded, which is what keeps the app small: on an NVIDIA card the OpenGL or Direct3D driver alone takes 80-400 MB
// in the process.
//
// The screen is cut into 64 px tiles. Each frame every tile gets a hash of the triangles over it; only tiles whose
// hash changed are cleared and drawn again (on several threads), and only those parts of the bitmap are passed on.
// An overlay that shows a card and a line of text redraws a few tiles per frame, or nothing at all.

use egui::epaint::{ClippedPrimitive, ImageData, Primitive, TextureId, Vertex};
use egui::{Color32, TexturesDelta};
use std::collections::HashMap;
use windows::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindowIndirect, ULW_ALPHA, UPDATELAYEREDWINDOWINFO};

const TILE: usize = 64;

struct Tex {
    w: usize,
    h: usize,
    /// Premultiplied texels, packed r | g << 8 | b << 16 | a << 24.
    px: Vec<u32>,
    /// The coverage alone, for textures that are white with alpha (the font atlas): one byte to read, not four.
    alpha: Option<Vec<u8>>,
    linear: bool,
    version: u64,
}

/// What the last frame cost.
#[derive(Default, Clone, Copy)]
pub struct Stats {
    pub tris: usize,
    pub dirty: usize,
    pub raster: f64,
    pub present: f64,
}

pub struct Soft {
    pub stats: Stats,
    w: usize,
    h: usize,
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    tex: HashMap<TextureId, Tex>,
    version: u64,
    tx: usize,
    ty: usize,
    prev: Vec<u64>,
    hash: Vec<u64>,
    tris: Vec<Vec<(u32, u32)>>,
    shown: bool,
}

#[derive(Clone, Copy)]
struct Ptr(*mut u32);
unsafe impl Send for Ptr {}
unsafe impl Sync for Ptr {}

fn mix64(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95)
}

fn vert_hash(h: u64, v: &Vertex) -> u64 {
    let h = mix64(h, (v.pos.x.to_bits() as u64) << 32 | v.pos.y.to_bits() as u64);
    let h = mix64(h, (v.uv.x.to_bits() as u64) << 32 | v.uv.y.to_bits() as u64);
    mix64(h, u32::from_le_bytes(v.color.to_array()) as u64)
}

impl Soft {
    pub fn new() -> Self {
        Soft {
            stats: Stats::default(),
            w: 0,
            h: 0,
            dc: HDC::default(),
            bmp: HBITMAP::default(),
            old: HGDIOBJ::default(),
            bits: std::ptr::null_mut(),
            tex: HashMap::new(),
            version: 0,
            tx: 0,
            ty: 0,
            prev: Vec::new(),
            hash: Vec::new(),
            tris: Vec::new(),
            shown: false,
        }
    }

    fn resize(&mut self, w: usize, h: usize) {
        if w == self.w && h == self.h && !self.bits.is_null() {
            return;
        }
        unsafe {
            if !self.dc.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bmp.into());
                let _ = DeleteDC(self.dc);
            }
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32), // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).expect("bitmap");
            self.old = SelectObject(dc, bmp.into());
            self.dc = dc;
            self.bmp = bmp;
            self.bits = bits as *mut u32;
        }
        self.w = w;
        self.h = h;
        self.tx = w.div_ceil(TILE);
        self.ty = h.div_ceil(TILE);
        let n = self.tx * self.ty;
        self.prev = vec![u64::MAX; n];
        self.hash = vec![0; n];
        self.tris = (0..n).map(|_| Vec::new()).collect();
        self.shown = false;
    }

    fn set_textures(&mut self, delta: &TexturesDelta) {
        let grey = |c: &Color32| c.r() == c.a() && c.g() == c.a() && c.b() == c.a();
        for (id, list) in delta.set.iter() {
            for d in list {
                let ImageData::Color(img) = &d.image;
                let linear = d.options.magnification == egui::TextureFilter::Linear;
                match d.pos {
                    None => {
                        // A new picture: everything drawn with this texture must be drawn again.
                        self.version += 1;
                        let alpha = img.pixels.iter().all(grey).then(|| img.pixels.iter().map(|c| c.a()).collect());
                        self.tex.insert(*id, Tex { w: img.size[0], h: img.size[1], px: img.pixels.iter().map(|c| u32::from_le_bytes(c.to_array())).collect(), alpha, linear, version: self.version });
                    }
                    Some([x0, y0]) => {
                        // New glyphs go into unused space, so what is on screen stays right: no version change.
                        if let Some(t) = self.tex.get_mut(id) {
                            if !img.pixels.iter().all(grey) {
                                t.alpha = None;
                            }
                            let w = img.size[0];
                            for y in 0..img.size[1] {
                                let row = &img.pixels[y * w..(y + 1) * w];
                                let at = (y0 + y) * t.w + x0;
                                if y0 + y < t.h && x0 + w <= t.w {
                                    for (o, c) in t.px[at..at + w].iter_mut().zip(row) {
                                        *o = u32::from_le_bytes(c.to_array());
                                    }
                                    if let Some(a) = &mut t.alpha {
                                        for (o, c) in a[at..at + w].iter_mut().zip(row) {
                                            *o = c.a();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Draws a frame and shows it at (x, y) on screen, `w`×`h` physical pixels.
    ///
    /// `floor` is the alpha left in empty pixels. Windows sends clicks on fully see-through pixels to the window below,
    /// so while the whole overlay must catch the pointer (Arrange mode) empty pixels get alpha 1: invisible, but solid.
    pub fn paint(&mut self, hwnd: isize, x: i32, y: i32, w: usize, h: usize, ppp: f32, floor: u8, prims: &[ClippedPrimitive], delta: &TexturesDelta) {
        self.resize(w, h);
        self.set_textures(delta);

        // Which triangles cover which tiles, and a hash per tile of everything drawn over it.
        for t in &mut self.tris {
            t.clear();
        }
        self.hash.iter_mut().for_each(|h| *h = 0x9e37_79b9_7f4a_7c15 ^ floor as u64);
        for (pi, cp) in prims.iter().enumerate() {
            let Primitive::Mesh(mesh) = &cp.primitive else { continue };
            let tv = self.tex.get(&mesh.texture_id).map(|t| t.version).unwrap_or(0);
            let clip = [
                (cp.clip_rect.min.x * ppp).round().max(0.0) as i32,
                (cp.clip_rect.min.y * ppp).round().max(0.0) as i32,
                ((cp.clip_rect.max.x * ppp).round() as i32).min(w as i32),
                ((cp.clip_rect.max.y * ppp).round() as i32).min(h as i32),
            ];
            if clip[0] >= clip[2] || clip[1] >= clip[3] {
                continue;
            }
            let base = mix64(mix64(0, tv), (clip[0] as u64) << 48 | (clip[1] as u64) << 32 | (clip[2] as u64) << 16 | clip[3] as u64);
            for (ti, tri) in mesh.indices.chunks_exact(3).enumerate() {
                let v = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
                if v.iter().all(|v| v.color.a() == 0) {
                    continue;
                }
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for p in &v {
                    x0 = x0.min(p.pos.x * ppp);
                    y0 = y0.min(p.pos.y * ppp);
                    x1 = x1.max(p.pos.x * ppp);
                    y1 = y1.max(p.pos.y * ppp);
                }
                let bx0 = (x0.floor() as i32).max(clip[0]);
                let by0 = (y0.floor() as i32).max(clip[1]);
                let bx1 = (x1.ceil() as i32 + 1).min(clip[2]);
                let by1 = (y1.ceil() as i32 + 1).min(clip[3]);
                if bx0 >= bx1 || by0 >= by1 {
                    continue;
                }
                let mut hsh = base;
                for vv in &v {
                    hsh = vert_hash(hsh, vv);
                }
                for ty in (by0 as usize / TILE)..=((by1 as usize - 1) / TILE) {
                    for tx in (bx0 as usize / TILE)..=((bx1 as usize - 1) / TILE) {
                        let t = ty * self.tx + tx;
                        self.hash[t] = mix64(self.hash[t], hsh);
                        self.tris[t].push((pi as u32, ti as u32));
                    }
                }
            }
        }
        let dirty: Vec<usize> = (0..self.hash.len()).filter(|&t| self.hash[t] != self.prev[t]).collect();
        self.stats = Stats { tris: self.tris.iter().map(Vec::len).sum(), dirty: dirty.len(), ..Default::default() };
        let t_raster = std::time::Instant::now();
        std::mem::swap(&mut self.prev, &mut self.hash);
        if dirty.is_empty() && self.shown {
            return;
        }

        // Draw the tiles that changed.
        let fb = Ptr(self.bits);
        let (tx, fw, fh) = (self.tx, w, h);
        let tex = &self.tex;
        let tris = &self.tris;
        let work = |t: usize| {
            let (cx, cy) = ((t % tx) * TILE, (t / tx) * TILE);
            let (ex, ey) = ((cx + TILE).min(fw), (cy + TILE).min(fh));
            let fb = fb;
            let empty = (floor as u32) << 24;
            for yy in cy..ey {
                unsafe { std::slice::from_raw_parts_mut(fb.0.add(yy * fw + cx), ex - cx).fill(empty) };
            }
            for &(pi, ti) in &tris[t] {
                let cp = &prims[pi as usize];
                let Primitive::Mesh(mesh) = &cp.primitive else { continue };
                let i = ti as usize * 3;
                let v = [mesh.vertices[mesh.indices[i] as usize], mesh.vertices[mesh.indices[i + 1] as usize], mesh.vertices[mesh.indices[i + 2] as usize]];
                let clip = [
                    ((cp.clip_rect.min.x * ppp).round() as i32).max(cx as i32),
                    ((cp.clip_rect.min.y * ppp).round() as i32).max(cy as i32),
                    ((cp.clip_rect.max.x * ppp).round() as i32).min(ex as i32),
                    ((cp.clip_rect.max.y * ppp).round() as i32).min(ey as i32),
                ];
                if clip[0] < clip[2] && clip[1] < clip[3] {
                    raster(fb.0, fw, &v, ppp, clip, tex.get(&mesh.texture_id));
                }
            }
        };
        // Threads only pay off for big redraws (Arrange mode, the panel opening); for a moving word or two they
        // cost more CPU than they save.
        if dirty.len() > 150 {
            std::thread::scope(|s| {
                let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 4);
                let chunk = dirty.len().div_ceil(n);
                for part in dirty.chunks(chunk) {
                    let work = &work;
                    s.spawn(move || part.iter().for_each(|&t| work(t)));
                }
            });
        } else {
            dirty.iter().for_each(|&t| work(t));
        }

        for id in &delta.free {
            self.tex.remove(id);
        }
        self.stats.raster = t_raster.elapsed().as_secs_f64() * 1000.0;
        let t_present = std::time::Instant::now();

        // Hand the changed parts to Windows.
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let dst = POINT { x, y };
        let size = SIZE { cx: w as i32, cy: h as i32 };
        let src = POINT { x: 0, y: 0 };
        let mut update = |dirty: Option<&RECT>| unsafe {
            let info = UPDATELAYEREDWINDOWINFO {
                cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                hdcDst: HDC::default(),
                pptDst: &dst,
                psize: &size,
                hdcSrc: self.dc,
                pptSrc: &src,
                crKey: windows::Win32::Foundation::COLORREF(0),
                pblend: &blend,
                dwFlags: ULW_ALPHA,
                prcDirty: dirty.map(|r| r as *const RECT).unwrap_or(std::ptr::null()),
            };
            let _ = UpdateLayeredWindowIndirect(HWND(hwnd as *mut _), &info);
        };
        if !self.shown {
            update(None);
            self.shown = true;
            return;
        }
        for r in clusters(&dirty, self.tx, self.ty) {
            let rect = RECT {
                left: (r[0] * TILE) as i32,
                top: (r[1] * TILE) as i32,
                right: ((r[2] * TILE).min(w)) as i32,
                bottom: ((r[3] * TILE).min(h)) as i32,
            };
            update(Some(&rect));
        }
        self.stats.present = t_present.elapsed().as_secs_f64() * 1000.0;
    }
}

impl Drop for Soft {
    fn drop(&mut self) {
        unsafe {
            if !self.dc.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bmp.into());
                let _ = DeleteDC(self.dc);
            }
        }
    }
}

/// Groups dirty tiles into a few rectangles (in tiles: x0, y0, x1, y1), merging ones that touch.
fn clusters(dirty: &[usize], tx: usize, _ty: usize) -> Vec<[usize; 4]> {
    let mut rects: Vec<[usize; 4]> = Vec::new();
    for &t in dirty {
        let (x, y) = (t % tx, t / tx);
        let r = [x, y, x + 1, y + 1];
        rects.push(r);
    }
    // Merge until stable: rectangles that overlap or touch become one.
    let mut changed = true;
    while changed {
        changed = false;
        let mut out: Vec<[usize; 4]> = Vec::new();
        'outer: for r in rects.drain(..) {
            for o in out.iter_mut() {
                if r[0] <= o[2] && o[0] <= r[2] && r[1] <= o[3] && o[1] <= r[3] {
                    o[0] = o[0].min(r[0]);
                    o[1] = o[1].min(r[1]);
                    o[2] = o[2].max(r[2]);
                    o[3] = o[3].max(r[3]);
                    changed = true;
                    continue 'outer;
                }
            }
            out.push(r);
        }
        rects = out;
    }
    rects
}

#[inline]
fn div255(x: u32) -> u32 {
    let x = x + 128;
    (x + (x >> 8)) >> 8
}

/// Premultiplied `s` (r, g, b, a in 0..=255) over the pixel.
#[inline]
fn blend(d: &mut u32, s: [u32; 4]) {
    if s[3] >= 255 {
        *d = 255 << 24 | s[0].min(255) << 16 | s[1].min(255) << 8 | s[2].min(255);
        return;
    }
    if (s[0] | s[1] | s[2] | s[3]) == 0 {
        return;
    }
    let inv = 255 - s[3];
    let dv = *d;
    let ch = |shift: u32, sc: u32| (sc + div255(((dv >> shift) & 255) * inv)).min(255);
    *d = ch(24, s[3]) << 24 | ch(16, s[0]) << 16 | ch(8, s[1]) << 8 | ch(0, s[2]);
}

#[inline]
fn mul4(c: [u32; 4], t: [u32; 4]) -> [u32; 4] {
    [div255(c[0] * t[0]), div255(c[1] * t[1]), div255(c[2] * t[2]), div255(c[3] * t[3])]
}

#[inline]
fn scale4(c: [u32; 4], k: u32) -> [u32; 4] {
    [div255(c[0] * k), div255(c[1] * k), div255(c[2] * k), div255(c[3] * k)]
}

/// Bilinear lookup in texel space: the four corners and their 8-bit weights.
#[inline]
fn taps(t: &Tex, u: f32, v: f32) -> ([usize; 4], u32, u32) {
    let fx = u * t.w as f32 - 0.5;
    let fy = v * t.h as f32 - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (ax, ay) = (((fx - x0) * 256.0) as u32, ((fy - y0) * 256.0) as u32);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let cx = |x: isize| x.clamp(0, t.w as isize - 1) as usize;
    let cy = |y: isize| y.clamp(0, t.h as isize - 1) as usize;
    let (xa, xb, ya, yb) = (cx(x0), cx(x0 + 1), cy(y0) * t.w, cy(y0 + 1) * t.w);
    ([ya + xa, ya + xb, yb + xa, yb + xb], ax, ay)
}

#[inline]
fn lerp2(p: [u32; 4], ax: u32, ay: u32) -> u32 {
    let top = p[0] * (256 - ax) + p[1] * ax;
    let bot = p[2] * (256 - ax) + p[3] * ax;
    (top * (256 - ay) + bot * ay) >> 16
}

#[inline]
fn nearest(t: &Tex, u: f32, v: f32) -> usize {
    let x = ((u * t.w as f32) as isize).clamp(0, t.w as isize - 1) as usize;
    let y = ((v * t.h as f32) as isize).clamp(0, t.h as isize - 1) as usize;
    y * t.w + x
}

/// Coverage (0..=255) from a texture that is white with alpha, like the font atlas.
#[inline]
fn sample_a(t: &Tex, a: &[u8], u: f32, v: f32) -> u32 {
    if !t.linear {
        return a[nearest(t, u, v)] as u32;
    }
    let (i, ax, ay) = taps(t, u, v);
    lerp2([a[i[0]] as u32, a[i[1]] as u32, a[i[2]] as u32, a[i[3]] as u32], ax, ay)
}

/// Mixes two packed texels (8 bits per channel), `w` of 0..=256 towards `b`: two channels per multiply.
#[inline]
fn lerp_packed(a: u32, b: u32, w: u32) -> u32 {
    let iw = 256 - w;
    let rb = (((a & 0x00ff_00ff) * iw + (b & 0x00ff_00ff) * w) >> 8) & 0x00ff_00ff;
    let ga = (((a >> 8) & 0x00ff_00ff) * iw + ((b >> 8) & 0x00ff_00ff) * w) & 0xff00_ff00;
    rb | ga
}

#[inline]
fn unpack(p: u32) -> [u32; 4] {
    [p & 255, (p >> 8) & 255, (p >> 16) & 255, p >> 24]
}

/// Premultiplied colour, packed as r | g << 8 | b << 16 | a << 24 (0 when the texture is empty there).
#[inline]
fn sample_packed(t: &Tex, u: f32, v: f32) -> u32 {
    if !t.linear {
        return t.px[nearest(t, u, v)];
    }
    let (i, ax, ay) = taps(t, u, v);
    let (p0, p1, p2, p3) = (t.px[i[0]], t.px[i[1]], t.px[i[2]], t.px[i[3]]);
    if (p0 | p1 | p2 | p3) == 0 {
        return 0;
    }
    lerp_packed(lerp_packed(p0, p1, ax), lerp_packed(p2, p3, ax), ay)
}

/// Premultiplied colour (0..=255 each) from any texture.
#[inline]
fn sample_rgba(t: &Tex, u: f32, v: f32) -> [u32; 4] {
    unpack(sample_packed(t, u, v))
}

/// What the texture contributes to a triangle.
enum Kind<'a> {
    /// The same texel everywhere (egui's white texel for plain shapes).
    Flat([u32; 4]),
    Alpha(&'a Tex, &'a [u8]),
    Rgba(&'a Tex),
}

/// One triangle into the bitmap within `clip` (pixels: x0, y0, x1, y1), blended over what is there (premultiplied).
/// Each row is walked only across the pixels inside the triangle; colour and texture coordinates step along it.
fn raster(fb: *mut u32, stride: usize, v: &[Vertex; 3], ppp: f32, clip: [i32; 4], tex: Option<&Tex>) {
    let p = |i: usize| (v[i].pos.x * ppp, v[i].pos.y * ppp);
    let (mut ia, mut ib) = (0usize, 1usize);
    let ic = 2usize;
    let cross = |p0: (f32, f32), p1: (f32, f32), q: (f32, f32)| (q.0 - p0.0) * (p1.1 - p0.1) - (q.1 - p0.1) * (p1.0 - p0.0);
    let mut area = cross(p(ia), p(ib), p(ic));
    if area == 0.0 || !area.is_finite() {
        return;
    }
    if area < 0.0 {
        std::mem::swap(&mut ia, &mut ib);
        area = -area;
    }
    let (pa, pb, pc) = (p(ia), p(ib), p(ic));
    let (x0, y0, x1, y1) = (
        (pa.0.min(pb.0).min(pc.0).floor() as i32).max(clip[0]),
        (pa.1.min(pb.1).min(pc.1).floor() as i32).max(clip[1]),
        (pa.0.max(pb.0).max(pc.0).ceil() as i32 + 1).min(clip[2]),
        (pa.1.max(pb.1).max(pc.1).ceil() as i32 + 1).min(clip[3]),
    );
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // Edge k is the one opposite vertex k: E(x, y) = A x + B y + C, positive inside, area × the weight of vertex k.
    let edge = |p0: (f32, f32), p1: (f32, f32)| {
        let a = p1.1 - p0.1;
        let b = -(p1.0 - p0.0);
        let c = -p0.0 * a - p0.1 * b;
        // Top-left rule, so pixels on an edge shared by two triangles are drawn once.
        let tl = a > 0.0 || (a == 0.0 && b > 0.0);
        (a, b, c, tl)
    };
    let edges = [edge(pb, pc), edge(pc, pa), edge(pa, pb)];
    let inside = |x: f32, y: f32| {
        edges.iter().all(|&(a, b, c, tl)| {
            let e = a * x + b * y + c;
            e > 0.0 || (e == 0.0 && tl)
        })
    };
    let inv = 1.0 / area;
    let vs = [v[ia], v[ib], v[ic]];
    let col = |k: usize| {
        let c = vs[k].color;
        [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32]
    };
    let cols = [col(0), col(1), col(2)];
    // Any attribute f is linear over the triangle: f(x, y) = Σ (A_k x + B_k y + C_k) / area × f_k.
    let grad = |f: [f32; 3]| {
        let mut g = [0.0f32; 3];
        for k in 0..3 {
            g[0] += edges[k].0 * inv * f[k];
            g[1] += edges[k].1 * inv * f[k];
            g[2] += edges[k].2 * inv * f[k];
        }
        g
    };
    let flat_colour = vs[0].color == vs[1].color && vs[1].color == vs[2].color;
    let flat_uv = vs[0].uv == vs[1].uv && vs[1].uv == vs[2].uv;
    let cg: [[f32; 3]; 4] = std::array::from_fn(|i| grad([cols[0][i], cols[1][i], cols[2][i]]));
    let ug = grad([vs[0].uv.x, vs[1].uv.x, vs[2].uv.x]);
    let vg = grad([vs[0].uv.y, vs[1].uv.y, vs[2].uv.y]);
    let c0 = [cols[0][0] as u32, cols[0][1] as u32, cols[0][2] as u32, cols[0][3] as u32];

    let kind = match tex {
        None => Kind::Flat([255; 4]),
        Some(t) if flat_uv => Kind::Flat(sample_rgba(t, vs[0].uv.x, vs[0].uv.y)),
        Some(t) => match &t.alpha {
            Some(a) => Kind::Alpha(t, a),
            None => Kind::Rgba(t),
        },
    };
    let flat_src = match kind {
        Kind::Flat(tx) if flat_colour => Some(mul4(c0, tx)),
        _ => None,
    };
    if flat_src.is_some_and(|s| (s[0] | s[1] | s[2] | s[3]) == 0) {
        return;
    }

    for y in y0..y1 {
        let py = y as f32 + 0.5;
        // The span of pixel centres where all three edges are ≥ 0, widened by one and then trimmed exactly.
        let (mut lo, mut hi) = (x0 as f32 + 0.5, x1 as f32 - 0.5);
        let mut empty = false;
        for &(a, b, c, _) in &edges {
            let r = b * py + c;
            if a > 0.0 {
                lo = lo.max(-r / a);
            } else if a < 0.0 {
                hi = hi.min(-r / a);
            } else if r < 0.0 {
                empty = true;
            }
        }
        if empty || lo > hi + 1.0 {
            continue;
        }
        let mut xs = ((lo - 0.5).ceil() as i32 - 1).max(x0);
        let mut xe = ((hi - 0.5).floor() as i32 + 1).min(x1 - 1);
        while xs <= xe && !inside(xs as f32 + 0.5, py) {
            xs += 1;
        }
        while xe >= xs && !inside(xe as f32 + 0.5, py) {
            xe -= 1;
        }
        if xs > xe {
            continue;
        }
        let row = unsafe { std::slice::from_raw_parts_mut(fb.add(y as usize * stride + xs as usize), (xe - xs + 1) as usize) };
        if let Some(s) = flat_src {
            if s[3] >= 255 {
                row.fill(255 << 24 | s[0].min(255) << 16 | s[1].min(255) << 8 | s[2].min(255));
            } else {
                row.iter_mut().for_each(|d| blend(d, s));
            }
            continue;
        }
        let px = xs as f32 + 0.5;
        let at = |g: [f32; 3]| g[0] * px + g[1] * py + g[2];
        let (mut u, mut vv) = (at(ug), at(vg));
        let mut c = [at(cg[0]), at(cg[1]), at(cg[2]), at(cg[3])];
        let dc = [cg[0][0], cg[1][0], cg[2][0], cg[3][0]];
        for d in row.iter_mut() {
            let colour = if flat_colour { c0 } else { c.map(|x| x.max(0.0) as u32) };
            let s = match kind {
                Kind::Flat(tx) => mul4(colour, tx),
                Kind::Alpha(t, a) => scale4(colour, sample_a(t, a, u, vv)),
                Kind::Rgba(t) => {
                    let p = sample_packed(t, u, vv);
                    if p == 0 {
                        [0; 4]
                    } else if colour == [255; 4] {
                        unpack(p)
                    } else {
                        mul4(colour, unpack(p))
                    }
                }
            };
            blend(d, s);
            u += ug[0];
            vv += vg[0];
            if !flat_colour {
                for i in 0..4 {
                    c[i] += dc[i];
                }
            }
        }
    }
}
