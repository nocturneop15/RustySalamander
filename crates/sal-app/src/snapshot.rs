//! Test-only software renderer: rasterizes egui output to a PNG so the UI can
//! be inspected without opening a window.

use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{self, epaint::Primitive, Color32, ImageData, TextureId};

#[derive(Default)]
pub struct Snapshot {
    textures: HashMap<TextureId, (usize, usize, Vec<Color32>)>,
}

impl Snapshot {
    /// Apply texture uploads; call with the output of every frame.
    pub fn apply(&mut self, out: &egui::FullOutput) {
        for (id, delta) in &out.textures_delta.set {
            let (w, h, px): (usize, usize, Vec<Color32>) = match &delta.image {
                ImageData::Color(c) => (c.width(), c.height(), c.pixels.clone()),
                ImageData::Font(f) => (f.width(), f.height(), f.srgba_pixels(None).collect()),
            };
            match delta.pos {
                None => {
                    self.textures.insert(*id, (w, h, px));
                }
                Some([x0, y0]) => {
                    let tex = self.textures.get_mut(id).expect("partial update of unknown texture");
                    for y in 0..h {
                        for x in 0..w {
                            tex.2[(y0 + y) * tex.0 + x0 + x] = px[y * w + x];
                        }
                    }
                }
            }
        }
    }

    pub fn render(&self, ctx: &egui::Context, out: egui::FullOutput, bg: Color32, path: &Path) {
        let ppp = out.pixels_per_point;
        let screen = ctx.screen_rect();
        let (w, h) = ((screen.width() * ppp) as usize, (screen.height() * ppp) as usize);
        let mut img = vec![[bg.r() as f32, bg.g() as f32, bg.b() as f32, 255.0]; w * h];
        for prim in ctx.tessellate(out.shapes, ppp) {
            let Primitive::Mesh(mesh) = prim.primitive else { continue };
            let Some(tex) = self.textures.get(&mesh.texture_id) else { continue };
            let clip = prim.clip_rect;
            let (cx0, cy0) = ((clip.min.x * ppp).max(0.0) as usize, (clip.min.y * ppp).max(0.0) as usize);
            let (cx1, cy1) = (((clip.max.x * ppp) as usize).min(w), ((clip.max.y * ppp) as usize).min(h));
            for tri in mesh.indices.as_chunks::<3>().0 {
                let v = [&mesh.vertices[tri[0] as usize], &mesh.vertices[tri[1] as usize], &mesh.vertices[tri[2] as usize]];
                let p = v.map(|v| (v.pos.x * ppp, v.pos.y * ppp));
                let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
                if area.abs() < 1e-6 {
                    continue;
                }
                let minx = p.iter().map(|q| q.0).fold(f32::MAX, f32::min).floor().max(cx0 as f32) as usize;
                let maxx = (p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil() as usize).min(cx1);
                let miny = p.iter().map(|q| q.1).fold(f32::MAX, f32::min).floor().max(cy0 as f32) as usize;
                let maxy = (p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as usize).min(cy1);
                for y in miny..maxy {
                    for x in minx..maxx {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
                        let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let lerp = |f: &dyn Fn(usize) -> f32| w0 * f(0) + w1 * f(1) + w2 * f(2);
                        let u = lerp(&|i| v[i].uv.x);
                        let vv = lerp(&|i| v[i].uv.y);
                        let tx = ((u * tex.0 as f32) as usize).min(tex.0 - 1);
                        let ty = ((vv * tex.1 as f32) as usize).min(tex.1 - 1);
                        let t = tex.2[ty * tex.0 + tx];
                        // premultiplied colors: out = src + dst * (1 - src.a)
                        let src = [
                            lerp(&|i| v[i].color.r() as f32) * t.r() as f32 / 255.0,
                            lerp(&|i| v[i].color.g() as f32) * t.g() as f32 / 255.0,
                            lerp(&|i| v[i].color.b() as f32) * t.b() as f32 / 255.0,
                            lerp(&|i| v[i].color.a() as f32) * t.a() as f32 / 255.0,
                        ];
                        let d = &mut img[y * w + x];
                        let k = 1.0 - src[3] / 255.0;
                        for c in 0..3 {
                            d[c] = src[c] + d[c] * k;
                        }
                    }
                }
            }
        }
        let rgb: Vec<u8> = img.iter().flat_map(|p| [p[0], p[1], p[2]].map(|c| c.clamp(0.0, 255.0) as u8)).collect();
        write_png(path, w, h, &rgb);
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Minimal RGB PNG writer using uncompressed (stored) deflate blocks.
fn write_png(path: &Path, w: usize, h: usize, rgb: &[u8]) {
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for row in rgb.chunks_exact(w * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let n = b.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        png.extend_from_slice(&c);
        png.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::write(path, png).unwrap();
}
