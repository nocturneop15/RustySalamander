//! Draws egui's triangles on the processor, into a bitmap that GDI can put
//! on screen: no graphics card, driver or graphics library is involved.
//!
//! It draws as egui's wgpu renderer does, so the program looks the same:
//! colors in gamma space with premultiplied alpha, each pixel sampled at its
//! center, the top-left fill rule (two triangles sharing an edge cover each
//! pixel on it once), bilinear textures. Only the dithering is left out.
//!
//! Rectangles, most of what egui draws (panels, selections, glyphs, icons),
//! take a fast path; everything else goes triangle by triangle. The frame is
//! cut into bands of rows, drawn on several threads. A frame can be drawn
//! only in part ([`Painter::paint`]'s `region`), where it changed
//! ([`Damage`]).

mod damage;

use std::collections::HashMap;
use std::sync::Mutex;

use egui::epaint::{ClippedPrimitive, ImageData, ImageDelta, Mesh, Primitive, Vertex};
use egui::{Color32, Rect, TextureFilter, TextureId, TexturesDelta};

pub use damage::Damage;

/// A bitmap: rows top-down, pixels `0x00RRGGBB` (a 32-bit DIB's layout).
#[derive(Clone, Default)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl Frame {
    pub fn new(width: usize, height: usize) -> Frame {
        Frame { width, height, pixels: vec![0; width * height] }
    }

    /// Make it `width` × `height` (its pixels are lost when the size changes).
    pub fn resize(&mut self, width: usize, height: usize) {
        if (width, height) != (self.width, self.height) {
            *self = Frame::new(width, height);
        }
    }

    /// The whole frame.
    pub fn area(&self) -> Area {
        Area { x0: 0, y0: 0, x1: self.width as i32, y1: self.height as i32 }
    }

    /// The pixels as RGBA bytes (for saving as an image).
    pub fn rgba(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect()
    }
}

/// A rectangle of pixels: columns `x0..x1`, rows `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Area {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Area {
    pub fn intersect(self, o: Area) -> Area {
        Area { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) }
    }

    /// The smallest area holding both.
    pub fn union(self, o: Area) -> Area {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        Area { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }

    pub fn is_empty(self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    /// Pixels in it.
    pub fn pixels(self) -> usize {
        if self.is_empty() {
            0
        } else {
            (self.x1 - self.x0) as usize * (self.y1 - self.y0) as usize
        }
    }
}

/// A texture: premultiplied RGBA (gamma), rows top-down.
struct Texture {
    w: i32,
    h: i32,
    px: Vec<[u8; 4]>,
    linear: bool,
}

impl Texture {
    #[inline]
    fn texel(&self, x: i32, y: i32) -> [u8; 4] {
        let (x, y) = (x.clamp(0, self.w - 1), y.clamp(0, self.h - 1));
        self.px[(y * self.w + x) as usize]
    }

    /// The color at texture coordinates `u`, `v` in texels (texel centers at
    /// .5), clamped to the edges as egui's textures are.
    #[inline]
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        if !self.linear {
            return self.texel(u.floor() as i32, v.floor() as i32).map(f32::from);
        }
        let (x, y) = (u - 0.5, v - 0.5);
        let (fx, fy) = (x.floor(), y.floor());
        let (ax, ay) = (x - fx, y - fy);
        let (x0, y0) = (fx as i32, fy as i32);
        let (t00, t10, t01, t11) = (self.texel(x0, y0), self.texel(x0 + 1, y0), self.texel(x0, y0 + 1), self.texel(x0 + 1, y0 + 1));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = t00[i] as f32 + (t10[i] as f32 - t00[i] as f32) * ax;
            let bottom = t01[i] as f32 + (t11[i] as f32 - t01[i] as f32) * ax;
            out[i] = top + (bottom - top) * ay;
        }
        out
    }
}

/// Draws egui's frames into [`Frame`]s, keeping the textures they use.
pub struct Painter {
    textures: HashMap<TextureId, Texture>,
    /// Drawn instead of a texture it doesn't know: nothing.
    missing: Texture,
    threads: usize,
}

impl Default for Painter {
    fn default() -> Self {
        Painter::new()
    }
}

impl Painter {
    /// A painter drawing on as many threads as the processor has (eight at most).
    pub fn new() -> Painter {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
        Painter { textures: HashMap::new(), missing: Texture { w: 1, h: 1, px: vec![[0; 4]], linear: false }, threads }
    }

    /// Draw on `n` threads (at least one).
    pub fn set_threads(&mut self, n: usize) {
        self.threads = n.max(1);
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Take the textures egui made or changed; before drawing the frame.
    pub fn set_textures(&mut self, delta: &TexturesDelta) {
        for (id, d) in &delta.set {
            self.set_texture(*id, d);
        }
    }

    /// Drop the textures egui is done with; after drawing the frame.
    pub fn free_textures(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    /// Set a texture or a part of it.
    pub fn set_texture(&mut self, id: TextureId, d: &ImageDelta) {
        let (w, h) = (d.image.width(), d.image.height());
        let px: Vec<[u8; 4]> = match &d.image {
            ImageData::Color(c) => c.pixels.iter().map(|c| c.to_array()).collect(),
            // coverage, as egui's renderers turn it into colors
            ImageData::Font(f) => f.srgba_pixels(None).map(|c| c.to_array()).collect(),
        };
        let linear = d.options.magnification == TextureFilter::Linear;
        match (d.pos, self.textures.get_mut(&id)) {
            (Some([x, y]), Some(t)) => {
                for row in 0..h {
                    let ty = (y + row) as i32;
                    if ty >= t.h {
                        break;
                    }
                    let start = (ty * t.w) as usize + x;
                    let n = w.min((t.w as usize).saturating_sub(x));
                    t.px[start..start + n].copy_from_slice(&px[row * w..row * w + n]);
                }
            }
            _ => {
                self.textures.insert(id, Texture { w: w as i32, h: h as i32, px, linear });
            }
        }
    }

    /// Draw `primitives` into `frame` (cleared to `clear` first). With a
    /// `region`, only the pixels in it are drawn; the rest stays as it was.
    pub fn paint(&self, frame: &mut Frame, primitives: &[ClippedPrimitive], pixels_per_point: f32, clear: Color32, region: Option<Area>) -> Stats {
        let region = region.map_or(frame.area(), |r| r.intersect(frame.area()));
        let mut stats = Stats::default();
        if region.is_empty() {
            return stats;
        }
        let mut ops = Vec::new();
        for p in primitives {
            let Primitive::Mesh(mesh) = &p.primitive else { continue };
            let clip = clip_area(p.clip_rect, pixels_per_point).intersect(region);
            if !clip.is_empty() {
                self.mesh_ops(mesh, pixels_per_point, clip, &mut ops, &mut stats);
            }
        }
        let clear = pack(clear.r(), clear.g(), clear.b());
        let width = frame.width;
        // bands of rows, more than threads so a busy band doesn't hold the rest up
        let rows = ((region.y1 - region.y0) as usize).div_ceil(self.threads * 4).max(16);
        let mut bands = Vec::new();
        let mut rest = &mut frame.pixels[region.y0 as usize * width..region.y1 as usize * width];
        let mut y = region.y0;
        while !rest.is_empty() {
            let n = rows.min(rest.len() / width);
            let (band, more) = rest.split_at_mut(n * width);
            bands.push((y, band));
            y += n as i32;
            rest = more;
        }
        let draw = |(y0, band): (i32, &mut [u32])| {
            let rows = (band.len() / width) as i32;
            let area = Area { x0: region.x0, x1: region.x1, y0, y1: y0 + rows };
            let mut t = Target { px: band, width, y0 };
            for y in area.y0..area.y1 {
                t.row(y)[area.x0 as usize..area.x1 as usize].fill(clear);
            }
            for op in &ops {
                op.draw(&mut t, area);
            }
        };
        let threads = self.threads.min(bands.len());
        if threads <= 1 {
            bands.into_iter().for_each(draw);
        } else {
            let queue = Mutex::new(bands);
            std::thread::scope(|s| {
                let work = || loop {
                    let Some(band) = queue.lock().unwrap().pop() else { break };
                    draw(band);
                };
                for _ in 1..threads {
                    s.spawn(work);
                }
                work();
            });
        }
        stats.pixels = region.pixels();
        stats
    }

    /// Turn a mesh into rectangles and triangles.
    fn mesh_ops<'a>(&'a self, mesh: &Mesh, ppp: f32, clip: Area, ops: &mut Vec<Op<'a>>, stats: &mut Stats) {
        let tex = self.textures.get(&mesh.texture_id).unwrap_or(&self.missing);
        let v = &mesh.vertices;
        let idx = &mesh.indices;
        let vert = |i: u32| {
            let v = v.get(i as usize).copied().unwrap_or(Vertex { pos: Default::default(), uv: Default::default(), color: Color32::TRANSPARENT });
            P { x: v.pos.x * ppp, y: v.pos.y * ppp, a: [v.color.r() as f32, v.color.g() as f32, v.color.b() as f32, v.color.a() as f32, v.uv.x * tex.w as f32, v.uv.y * tex.h as f32] }
        };
        let mut i = 0;
        while i + 3 <= idx.len() {
            if i + 6 <= idx.len() {
                if let Some(op) = quad(v, &idx[i..i + 6], tex, ppp, clip) {
                    if let Some(op) = op {
                        ops.push(op);
                    }
                    stats.rects += 1;
                    i += 6;
                    continue;
                }
            }
            if let Some(t) = Tri::new(vert(idx[i]), vert(idx[i + 1]), vert(idx[i + 2]), tex, clip) {
                ops.push(Op::Tri(Box::new(t)));
            }
            stats.triangles += 1;
            i += 3;
        }
    }
}

/// What a frame took.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// Rectangles found (two triangles each) and other triangles.
    pub rects: usize,
    pub triangles: usize,
    /// Pixels drawn.
    pub pixels: usize,
}

/// The pixels a clip rectangle (in points) covers, as egui's wgpu renderer
/// rounds it.
fn clip_area(r: Rect, ppp: f32) -> Area {
    let f = |v: f32| (v * ppp).round().clamp(-1e9, 1e9) as i32;
    Area { x0: f(r.min.x), y0: f(r.min.y), x1: f(r.max.x), y1: f(r.max.y) }
}

/// The first pixel whose center is at or right of `edge` (a rectangle's edge,
/// in pixels): pixels whose center lies on the left or top edge are in, on the
/// right or bottom edge out, as the fill rule has it.
fn first_pixel(edge: f32) -> i32 {
    (edge - 0.5).ceil().clamp(-1e9, 1e9) as i32
}

#[inline]
fn pack(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// `x / 255`, rounded, for `x` up to 255 × 255.
#[inline]
fn div255(x: u32) -> u32 {
    let x = x + 128;
    (x + (x >> 8)) >> 8
}

/// Premultiplied `s` over `d`.
#[inline]
fn blend(d: &mut u32, s: [f32; 4]) {
    let k = 1.0 - s[3] * (1.0 / 255.0);
    let c = |shift: u32, s: f32| -> u32 { ((((*d >> shift) & 255) as f32 * k + s + 0.5) as u32).min(255) << shift };
    *d = c(16, s[0]) | c(8, s[1]) | c(0, s[2]);
}

/// A vertex in pixels: its position and its attributes (color: premultiplied
/// RGBA 0–255; texture coordinates in texels).
#[derive(Clone, Copy)]
struct P {
    x: f32,
    y: f32,
    a: [f32; 6],
}

/// Part of a frame being drawn: a band of rows.
struct Target<'a> {
    px: &'a mut [u32],
    width: usize,
    y0: i32,
}

impl Target<'_> {
    #[inline]
    fn row(&mut self, y: i32) -> &mut [u32] {
        let start = (y - self.y0) as usize * self.width;
        &mut self.px[start..start + self.width]
    }
}

enum Op<'a> {
    /// A rectangle of one color (premultiplied).
    Fill { area: Area, c: [u8; 4] },
    /// A rectangle of a texture (a glyph, an icon) times a color: at the
    /// pixel center `cx`, `cy` the texel `u.0 + u.1·cx`, `v.0 + v.1·cy`.
    Image { area: Area, tex: &'a Texture, u: (f32, f32), v: (f32, f32), c: [f32; 4] },
    Tri(Box<Tri<'a>>),
}

impl Op<'_> {
    fn draw(&self, t: &mut Target, band: Area) {
        match self {
            Op::Fill { area, c } => {
                let a = area.intersect(band);
                if a.is_empty() || c == &[0; 4] {
                    return;
                }
                let (x0, x1) = (a.x0 as usize, a.x1 as usize);
                if c[3] == 255 {
                    let p = pack(c[0], c[1], c[2]);
                    for y in a.y0..a.y1 {
                        t.row(y)[x0..x1].fill(p);
                    }
                } else {
                    let k = 255 - c[3] as u32;
                    let (r, g, b) = (c[0] as u32, c[1] as u32, c[2] as u32);
                    for y in a.y0..a.y1 {
                        for d in &mut t.row(y)[x0..x1] {
                            let ch = |shift: u32, s: u32| (s + div255(((*d >> shift) & 255) * k)).min(255) << shift;
                            *d = ch(16, r) | ch(8, g) | ch(0, b);
                        }
                    }
                }
            }
            Op::Image { area, tex, u, v, c } => {
                let a = area.intersect(band);
                if a.is_empty() {
                    return;
                }
                let (x0, x1) = (a.x0 as usize, a.x1 as usize);
                let u_first = u.0 + u.1 * (a.x0 as f32 + 0.5);
                let whole = |x: f32| x == x.round();
                for y in a.y0..a.y1 {
                    let vv = v.0 + v.1 * (y as f32 + 0.5);
                    let row = &mut t.row(y)[x0..x1];
                    // one texel per pixel, on the texels' centers (or not
                    // filtered): copy the texels
                    let copy = (u.1 == 1.0 && v.1 == 1.0).then(|| {
                        if !tex.linear {
                            Some((u_first.floor() as i32, vv.floor() as i32))
                        } else {
                            (whole(u_first - 0.5) && whole(vv - 0.5)).then_some(((u_first - 0.5) as i32, (vv - 0.5) as i32))
                        }
                    });
                    if let Some(Some((tx, ty))) = copy {
                        for (i, d) in row.iter_mut().enumerate() {
                            let s = tex.texel(tx + i as i32, ty);
                            if s != [0; 4] {
                                blend(d, [0, 1, 2, 3].map(|k| c[k] * s[k] as f32 / 255.0));
                            }
                        }
                    } else {
                        for (i, d) in row.iter_mut().enumerate() {
                            let s = tex.sample(u_first + u.1 * i as f32, vv);
                            blend(d, [0, 1, 2, 3].map(|k| c[k] * s[k] / 255.0));
                        }
                    }
                }
            }
            Op::Tri(tri) => tri.draw(t, band),
        }
    }
}

/// A rectangle given as two triangles (most of what egui draws), as a fill or
/// an image: `Some(None)` is a rectangle with nothing to draw; `None` isn't
/// one (draw the triangles).
fn quad<'a>(v: &[Vertex], idx: &[u32], tex: &'a Texture, ppp: f32, clip: Area) -> Option<Option<Op<'a>>> {
    let ids: [u32; 6] = idx.try_into().ok()?;
    // four corners, the two triangles sharing a diagonal
    let mut corners: [u32; 4] = [u32::MAX; 4];
    let mut n = 0;
    for &i in &ids {
        if !corners[..n].contains(&i) {
            if n == 4 {
                return None;
            }
            corners[n] = i;
            n += 1;
        }
    }
    if n != 4 {
        return None;
    }
    let shared: Vec<u32> = ids[..3].iter().copied().filter(|i| ids[3..].contains(i)).collect();
    if shared.len() != 2 || ids[..3].iter().any(|i| ids[..3].iter().filter(|j| *j == i).count() > 1) {
        return None;
    }
    let vs: [Vertex; 4] = [v.get(corners[0] as usize)?, v.get(corners[1] as usize)?, v.get(corners[2] as usize)?, v.get(corners[3] as usize)?].map(|v| *v);
    let (mut xmin, mut xmax, mut ymin, mut ymax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for v in &vs {
        xmin = xmin.min(v.pos.x);
        xmax = xmax.max(v.pos.x);
        ymin = ymin.min(v.pos.y);
        ymax = ymax.max(v.pos.y);
    }
    let corner = |x: f32, y: f32| vs.iter().position(|v| v.pos.x == x && v.pos.y == y);
    let (tl, tr, bl, br) = (corner(xmin, ymin)?, corner(xmax, ymin)?, corner(xmin, ymax)?, corner(xmax, ymax)?);
    let mut seen = [tl, tr, bl, br];
    seen.sort_unstable();
    if seen != [0, 1, 2, 3] {
        return None;
    }
    // the shared corners are opposite each other
    let s0 = vs[corners.iter().position(|c| *c == shared[0])?].pos;
    let s1 = vs[corners.iter().position(|c| *c == shared[1])?].pos;
    if s0.x == s1.x || s0.y == s1.y {
        return None;
    }
    let color = vs[0].color;
    if vs.iter().any(|v| v.color != color) {
        return None;
    }
    let (ul, ur, vt, vb) = (vs[tl].uv.x, vs[tr].uv.x, vs[tl].uv.y, vs[bl].uv.y);
    if vs[bl].uv.x != ul || vs[br].uv.x != ur || vs[tr].uv.y != vt || vs[br].uv.y != vb {
        return None;
    }
    let (x0, x1, y0, y1) = (xmin * ppp, xmax * ppp, ymin * ppp, ymax * ppp);
    let area = Area { x0: first_pixel(x0), x1: first_pixel(x1), y0: first_pixel(y0), y1: first_pixel(y1) }.intersect(clip);
    if area.is_empty() || color == Color32::TRANSPARENT {
        return Some(None);
    }
    let c = [color.r() as f32, color.g() as f32, color.b() as f32, color.a() as f32];
    let (tw, th) = (tex.w as f32, tex.h as f32);
    if ul == ur && vt == vb {
        let s = tex.sample(ul * tw, vt * th);
        let c = [0, 1, 2, 3].map(|i| (c[i] * s[i] / 255.0 + 0.5).min(255.0) as u8);
        return Some(Some(Op::Fill { area, c }));
    }
    // texels per pixel, and the texel at pixel coordinate 0
    let du = (ur - ul) * tw / (x1 - x0);
    let dv = (vb - vt) * th / (y1 - y0);
    let u = (ul * tw - du * x0, du);
    let v = (vt * th - dv * y0, dv);
    Some(Some(Op::Image { area, tex, u, v, c }))
}

/// A triangle, set up for drawing.
struct Tri<'a> {
    /// Its bounding box within the clip rectangle.
    area: Area,
    tex: &'a Texture,
    /// Edges `A·x + B·y + C`, positive inside, and whether a pixel center
    /// right on the edge is in (the top-left rule).
    edges: [(f32, f32, f32, bool); 3],
    /// The attributes (color, texture coordinates) at pixel `x`, `y`:
    /// `base + gx·x + gy·y`.
    base: [f32; 6],
    gx: [f32; 6],
    gy: [f32; 6],
    /// One color for the whole triangle, one texel for the whole triangle.
    flat: Option<[f32; 4]>,
    flat_tex: Option<[f32; 4]>,
}

impl<'a> Tri<'a> {
    fn new(a: P, mut b: P, mut c: P, tex: &'a Texture, clip: Area) -> Option<Tri<'a>> {
        let mut area2 = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if !area2.is_finite() || area2 == 0.0 {
            return None;
        }
        if area2 < 0.0 {
            std::mem::swap(&mut b, &mut c);
            area2 = -area2;
        }
        let bbox = Area {
            x0: a.x.min(b.x).min(c.x).floor() as i32,
            y0: a.y.min(b.y).min(c.y).floor() as i32,
            x1: a.x.max(b.x).max(c.x).ceil() as i32 + 1,
            y1: a.y.max(b.y).max(c.y).ceil() as i32 + 1,
        };
        let area = bbox.intersect(clip);
        if area.is_empty() {
            return None;
        }
        let edge = |p: P, q: P| {
            let (ea, eb) = (-(q.y - p.y), q.x - p.x);
            (ea, eb, -(ea * p.x + eb * p.y), ea > 0.0 || (ea == 0.0 && eb > 0.0))
        };
        let edges = [edge(a, b), edge(b, c), edge(c, a)];
        let (mut base, mut gx, mut gy) = ([0.0; 6], [0.0; 6], [0.0; 6]);
        for k in 0..6 {
            let (d1, d2) = (b.a[k] - a.a[k], c.a[k] - a.a[k]);
            gx[k] = (d1 * (c.y - a.y) - d2 * (b.y - a.y)) / area2;
            gy[k] = (d2 * (b.x - a.x) - d1 * (c.x - a.x)) / area2;
            base[k] = a.a[k] - gx[k] * a.x - gy[k] * a.y;
        }
        let same = |r: std::ops::Range<usize>| r.clone().all(|k| a.a[k] == b.a[k] && a.a[k] == c.a[k]);
        let flat = same(0..4).then(|| [a.a[0], a.a[1], a.a[2], a.a[3]]);
        let flat_tex = same(4..6).then(|| tex.sample(a.a[4], a.a[5]));
        if flat == Some([0.0; 4]) {
            return None;
        }
        Some(Tri { area, tex, edges, base, gx, gy, flat, flat_tex })
    }

    fn draw(&self, t: &mut Target, band: Area) {
        let r = self.area.intersect(band);
        if r.is_empty() {
            return;
        }
        let constant = match (self.flat, self.flat_tex) {
            (Some(c), Some(s)) => Some([0, 1, 2, 3].map(|i| c[i] * s[i] / 255.0)),
            _ => None,
        };
        'rows: for y in r.y0..r.y1 {
            let cy = y as f32 + 0.5;
            // the pixels between the edges, give or take one
            let (mut lo, mut hi) = (r.x0 as f32, (r.x1 - 1) as f32);
            let mut rowc = [0.0; 3];
            for (i, &(ea, eb, ec, incl)) in self.edges.iter().enumerate() {
                rowc[i] = eb * cy + ec;
                if ea > 0.0 {
                    lo = lo.max(-rowc[i] / ea - 0.5);
                } else if ea < 0.0 {
                    hi = hi.min(-rowc[i] / ea - 0.5);
                } else if rowc[i] < 0.0 || (rowc[i] == 0.0 && !incl) {
                    continue 'rows;
                }
            }
            let x0 = (lo.ceil() as i32 - 1).max(r.x0);
            let x1 = (hi.floor() as i32 + 2).min(r.x1);
            if x0 >= x1 {
                continue;
            }
            let at: [f32; 6] = std::array::from_fn(|k| self.base[k] + self.gy[k] * cy);
            let row = t.row(y);
            for x in x0..x1 {
                let cx = x as f32 + 0.5;
                let inside = self.edges.iter().zip(&rowc).all(|(&(ea, _, _, incl), &rc)| {
                    let e = ea * cx + rc;
                    e > 0.0 || (e == 0.0 && incl)
                });
                if !inside {
                    continue;
                }
                let src = match constant {
                    Some(s) => s,
                    None => {
                        let c = self.flat.unwrap_or_else(|| [0, 1, 2, 3].map(|k| (at[k] + self.gx[k] * cx).clamp(0.0, 255.0)));
                        let s = self.flat_tex.unwrap_or_else(|| self.tex.sample(at[4] + self.gx[4] * cx, at[5] + self.gx[5] * cx));
                        [0, 1, 2, 3].map(|i| c[i] * s[i] / 255.0)
                    }
                };
                blend(&mut row[x as usize], src);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Pos2};

    const WHITE: Color32 = Color32::WHITE;

    fn white_painter() -> Painter {
        let mut p = Painter::new();
        // a 1×1 white texture under TextureId::default(), as the font atlas starts
        let img = egui::ColorImage::new([1, 1], WHITE);
        p.set_texture(TextureId::default(), &ImageDelta::full(img, egui::TextureOptions::LINEAR));
        p
    }

    fn prim(mesh: Mesh) -> ClippedPrimitive {
        ClippedPrimitive { clip_rect: Rect::EVERYTHING, primitive: Primitive::Mesh(mesh) }
    }

    fn at(f: &Frame, x: usize, y: usize) -> u32 {
        f.pixels[y * f.width + x]
    }

    #[test]
    fn a_rectangle_covers_the_pixels_whose_centers_it_holds() {
        let p = white_painter();
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(Rect::from_min_max(pos2(1.0, 1.0), pos2(3.6, 2.6)), Color32::RED);
        let mut f = Frame::new(5, 4);
        let s = p.paint(&mut f, &[prim(mesh)], 1.0, Color32::BLACK, None);
        assert_eq!((s.rects, s.triangles), (1, 0));
        let red: Vec<(usize, usize)> = (0..4).flat_map(|y| (0..5).map(move |x| (x, y))).filter(|&(x, y)| at(&f, x, y) == 0xff0000).collect();
        assert_eq!(red, [(1, 1), (2, 1), (3, 1), (1, 2), (2, 2), (3, 2)]);
        // the same as two triangles of their own (no shared corners): the same pixels
        let mut tri = Mesh::default();
        let r = Rect::from_min_max(pos2(1.0, 1.0), pos2(3.6, 2.6));
        for pos in [r.left_top(), r.right_top(), r.left_bottom(), r.left_bottom(), r.right_top(), r.right_bottom()] {
            tri.colored_vertex(pos, Color32::RED);
        }
        tri.indices = (0..6).collect();
        let mut h = Frame::new(5, 4);
        let s = p.paint(&mut h, &[prim(tri)], 1.0, Color32::BLACK, None);
        assert_eq!((s.rects, s.triangles), (0, 2));
        assert_eq!(f.pixels, h.pixels);
    }

    #[test]
    fn shared_edges_are_drawn_once() {
        let p = white_painter();
        // a half-transparent quadrilateral, not a rectangle: two triangles
        let mut mesh = Mesh::default();
        let c = Color32::from_rgba_premultiplied(100, 0, 0, 128);
        for pos in [pos2(0.0, 0.0), pos2(17.3, 1.0), pos2(1.2, 13.7), pos2(19.0, 15.0)] {
            mesh.colored_vertex(pos, c);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(2, 1, 3);
        let mut f = Frame::new(20, 16);
        p.paint(&mut f, &[prim(mesh)], 1.0, Color32::from_gray(40), None);
        let mut colors: Vec<u32> = f.pixels.clone();
        colors.sort_unstable();
        colors.dedup();
        // the background and the shape, no darker seam along the diagonal
        assert_eq!(colors.len(), 2, "{colors:x?}");
    }

    #[test]
    fn colors_blend_as_on_the_graphics_card() {
        let p = white_painter();
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(Rect::from_min_max(Pos2::ZERO, pos2(2.0, 2.0)), Color32::from_rgba_premultiplied(0, 64, 0, 64));
        let mut f = Frame::new(2, 2);
        p.paint(&mut f, &[prim(mesh)], 1.0, Color32::from_rgb(200, 200, 200), None);
        // 200 · (1 − 64/255) + premultiplied color
        assert_eq!(at(&f, 0, 0), pack(150, 214, 150));
    }

    #[test]
    fn scaling_and_clipping() {
        let p = white_painter();
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(Rect::from_min_max(Pos2::ZERO, pos2(4.0, 4.0)), WHITE);
        let clipped = ClippedPrimitive { clip_rect: Rect::from_min_max(pos2(1.0, 1.0), pos2(3.0, 2.0)), primitive: Primitive::Mesh(mesh) };
        let mut f = Frame::new(8, 8);
        p.paint(&mut f, &[clipped], 2.0, Color32::BLACK, None);
        let lit = f.pixels.iter().filter(|&&p| p == 0xffffff).count();
        assert_eq!(lit, 4 * 2);
        assert_eq!(at(&f, 2, 2), 0xffffff);
        assert_eq!(at(&f, 1, 2), 0);
    }

    #[test]
    fn a_region_leaves_the_rest() {
        let p = white_painter();
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(Rect::from_min_max(Pos2::ZERO, pos2(10.0, 10.0)), WHITE);
        let mut f = Frame::new(10, 10);
        f.pixels.fill(0x123456);
        p.paint(&mut f, &[prim(mesh)], 1.0, Color32::BLACK, Some(Area { x0: 2, y0: 3, x1: 4, y1: 5 }));
        assert_eq!(f.pixels.iter().filter(|&&p| p == 0xffffff).count(), 4);
        assert_eq!(f.pixels.iter().filter(|&&p| p == 0x123456).count(), 96);
    }

    /// A frame of real widgets, tessellated by egui.
    fn widgets(ppp: f32) -> (Painter, Vec<ClippedPrimitive>) {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(300.0, 200.0))), ..Default::default() };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        let mut p = Painter::new();
        let mut prims = Vec::new();
        for _ in 0..2 {
            let out = ctx.run(raw.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("Příliš žluťoučký kůň");
                    let _ = ui.button("OK");
                    ui.checkbox(&mut true, "Hidden files");
                    ui.add(egui::Slider::new(&mut 0.4, 0.0..=1.0));
                    egui::Frame::group(ui.style()).rounding(6.0).show(ui, |ui| ui.label("úpěl ďábelské ódy"));
                });
            });
            p.set_textures(&out.textures_delta);
            prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        }
        (p, prims)
    }

    #[test]
    fn threads_draw_the_same() {
        for ppp in [1.0, 1.25, 2.0] {
            let (mut p, prims) = widgets(ppp);
            let (w, h) = ((300.0 * ppp) as usize, (200.0 * ppp) as usize);
            let mut one = Frame::new(w, h);
            p.set_threads(1);
            let s = p.paint(&mut one, &prims, ppp, Color32::BLACK, None);
            assert!(s.rects > 20 && s.triangles > 20, "{s:?}");
            let mut many = Frame::new(w, h);
            p.set_threads(7);
            p.paint(&mut many, &prims, ppp, Color32::BLACK, None);
            assert!(one.pixels == many.pixels);
            // text and widgets drew something in several colors
            let mut colors = one.pixels.clone();
            colors.sort_unstable();
            colors.dedup();
            assert!(colors.len() > 30, "{}", colors.len());
        }
    }
}
