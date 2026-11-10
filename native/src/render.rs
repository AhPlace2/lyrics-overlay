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
    px: Vec<Color32>,
    linear: bool,
    version: u64,
}

pub struct Soft {
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
        for (id, list) in delta.set.iter() {
            for d in list {
                let ImageData::Color(img) = &d.image;
                self.version += 1;
                let linear = d.options.magnification == egui::TextureFilter::Linear;
                match d.pos {
                    None => {
                        self.tex.insert(*id, Tex { w: img.size[0], h: img.size[1], px: img.pixels.clone(), linear, version: self.version });
                    }
                    Some([x0, y0]) => {
                        if let Some(t) = self.tex.get_mut(id) {
                            for y in 0..img.size[1] {
                                let row = &img.pixels[y * img.size[0]..(y + 1) * img.size[0]];
                                let at = (y0 + y) * t.w + x0;
                                if y0 + y < t.h && x0 + img.size[0] <= t.w {
                                    t.px[at..at + img.size[0]].copy_from_slice(row);
                                }
                            }
                            t.version = self.version;
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
        if dirty.len() > 4 {
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
fn premul_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]]
}

fn sample(t: &Tex, u: f32, v: f32) -> [f32; 4] {
    let c = |c: Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0];
    if !t.linear {
        let x = ((u * t.w as f32) as isize).clamp(0, t.w as isize - 1) as usize;
        let y = ((v * t.h as f32) as isize).clamp(0, t.h as isize - 1) as usize;
        return c(t.px[y * t.w + x]);
    }
    let fx = u * t.w as f32 - 0.5;
    let fy = v * t.h as f32 - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let (ax, ay) = (fx - x0, fy - y0);
    let get = |x: isize, y: isize| {
        let x = x.clamp(0, t.w as isize - 1) as usize;
        let y = y.clamp(0, t.h as isize - 1) as usize;
        c(t.px[y * t.w + x])
    };
    let (x0, y0) = (x0 as isize, y0 as isize);
    let (p00, p10, p01, p11) = (get(x0, y0), get(x0 + 1, y0), get(x0, y0 + 1), get(x0 + 1, y0 + 1));
    let mut out = [0.0; 4];
    for i in 0..4 {
        let top = p00[i] + (p10[i] - p00[i]) * ax;
        let bot = p01[i] + (p11[i] - p01[i]) * ax;
        out[i] = top + (bot - top) * ay;
    }
    out
}

/// One triangle into the bitmap within `clip` (pixels: x0, y0, x1, y1), blended over what is there (premultiplied).
fn raster(fb: *mut u32, stride: usize, v: &[Vertex; 3], ppp: f32, clip: [i32; 4], tex: Option<&Tex>) {
    let p = |i: usize| (v[i].pos.x * ppp, v[i].pos.y * ppp);
    let (mut a, mut b, c) = (0usize, 1usize, 2usize);
    let edge = |p0: (f32, f32), p1: (f32, f32), q: (f32, f32)| (q.0 - p0.0) * (p1.1 - p0.1) - (q.1 - p0.1) * (p1.0 - p0.0);
    let mut area = edge(p(a), p(b), p(c));
    if area == 0.0 {
        return;
    }
    if area < 0.0 {
        std::mem::swap(&mut a, &mut b);
        area = -area;
    }
    let (pa, pb, pc) = (p(a), p(b), p(c));
    let (x0, y0, x1, y1) = (
        (pa.0.min(pb.0).min(pc.0).floor() as i32).max(clip[0]),
        (pa.1.min(pb.1).min(pc.1).floor() as i32).max(clip[1]),
        (pa.0.max(pb.0).max(pc.0).ceil() as i32 + 1).min(clip[2]),
        (pa.1.max(pb.1).max(pc.1).ceil() as i32 + 1).min(clip[3]),
    );
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // Top-left rule, so shared edges are drawn once.
    let tl = |p0: (f32, f32), p1: (f32, f32)| {
        let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
        dy > 0.0 || (dy == 0.0 && dx < 0.0)
    };
    let edges = [(pb, pc, tl(pb, pc)), (pc, pa, tl(pc, pa)), (pa, pb, tl(pa, pb))];
    let col = |i: usize| {
        let c = v[i].color;
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
    };
    let (ca, cb, cc) = (col(a), col(b), col(c));
    let same_colour = v[0].color == v[1].color && v[1].color == v[2].color;
    let same_uv = v[0].uv == v[1].uv && v[1].uv == v[2].uv;
    let flat_tex = match tex {
        Some(t) if same_uv => Some(sample(t, v[0].uv.x, v[0].uv.y)),
        None => Some([1.0; 4]),
        _ => None,
    };
    let flat = match flat_tex {
        Some(tx) if same_colour => Some(premul_mul(ca, tx)),
        _ => None,
    };
    let inv = 1.0 / area;
    for y in y0..y1 {
        let py = y as f32 + 0.5;
        let row = unsafe { fb.add(y as usize * stride) };
        for x in x0..x1 {
            let q = (x as f32 + 0.5, py);
            let mut w = [0.0f32; 3];
            let mut inside = true;
            for (k, (e0, e1, t)) in edges.iter().enumerate() {
                let e = edge(*e0, *e1, q);
                if e < 0.0 || (e == 0.0 && !t) {
                    inside = false;
                    break;
                }
                w[k] = e * inv;
            }
            if !inside {
                continue;
            }
            let src = match flat {
                Some(s) => s,
                None => {
                    let mut col = [0.0; 4];
                    for i in 0..4 {
                        col[i] = ca[i] * w[0] + cb[i] * w[1] + cc[i] * w[2];
                    }
                    let tx = match flat_tex {
                        Some(t) => t,
                        None => {
                            let (ua, ub, uc) = (v[a].uv, v[b].uv, v[c].uv);
                            let u = ua.x * w[0] + ub.x * w[1] + uc.x * w[2];
                            let vv = ua.y * w[0] + ub.y * w[1] + uc.y * w[2];
                            sample(tex.unwrap(), u, vv)
                        }
                    };
                    premul_mul(col, tx)
                }
            };
            if src[3] <= 0.0 && src[0] <= 0.0 && src[1] <= 0.0 && src[2] <= 0.0 {
                continue;
            }
            unsafe {
                let d = row.add(x as usize);
                let dv = *d;
                let inv_a = 1.0 - src[3].min(1.0);
                let ch = |shift: u32, s: f32| -> u32 {
                    let dc = ((dv >> shift) & 0xff) as f32;
                    ((s * 255.0 + dc * inv_a).round().clamp(0.0, 255.0)) as u32
                };
                *d = ch(24, src[3]) << 24 | ch(16, src[0]) << 16 | ch(8, src[1]) << 8 | ch(0, src[2]);
            }
        }
    }
}
