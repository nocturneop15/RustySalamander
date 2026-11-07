//! What changed between two frames, from what egui asked to draw, so a frame
//! is drawn only where it differs from the last one ([`Damage`]).

use egui::epaint::{ClippedPrimitive, Primitive, Vertex};
use egui::TexturesDelta;

use crate::{clip_area, Area};

/// The triangles of the last frame, each as a fingerprint (its clip
/// rectangle, texture and vertices) and the pixels it can touch.
///
/// A new frame's triangles are compared with them in order: those the two
/// frames start and end with alike are left out, and what lies between,
/// in either frame, is what has to be drawn again. A cursor moving down a
/// row is those two rows; a frame alike in all is nothing.
#[derive(Default)]
pub struct Damage {
    last: Vec<(u64, Area)>,
    /// The frame's size and scale the triangles were for.
    size: (usize, usize),
    ppp: f32,
}

impl Damage {
    /// Forget the last frame: the next one is drawn whole.
    pub fn reset(&mut self) {
        self.last.clear();
        self.size = (0, 0);
    }

    /// The part of a `width` × `height` frame to draw again for
    /// `primitives` (empty: nothing changed), and remember them. The whole
    /// frame when the size, scale or a texture changed.
    pub fn of(&mut self, primitives: &[ClippedPrimitive], pixels_per_point: f32, width: usize, height: usize, textures: &TexturesDelta) -> Area {
        let whole = Area { x0: 0, y0: 0, x1: width as i32, y1: height as i32 };
        let new = triangles(primitives, pixels_per_point);
        let same_frame = self.size == (width, height) && self.ppp == pixels_per_point && textures.set.is_empty();
        let old = std::mem::replace(&mut self.last, new);
        self.size = (width, height);
        self.ppp = pixels_per_point;
        if !same_frame || old.is_empty() {
            return whole;
        }
        let new = &self.last;
        let start = old.iter().zip(new).take_while(|(a, b)| a.0 == b.0).count();
        let end = old[start..].iter().rev().zip(new[start..].iter().rev()).take_while(|(a, b)| a.0 == b.0).count();
        let changed = old[start..old.len() - end].iter().chain(&new[start..new.len() - end]);
        changed.fold(Area::default(), |a, t| if a.is_empty() { t.1 } else if t.1.is_empty() { a } else { a.union(t.1) }).intersect(whole)
    }
}

/// Each triangle of `primitives`: its fingerprint and the pixels it can
/// touch (its bounds, a pixel wider, inside its clip rectangle).
fn triangles(primitives: &[ClippedPrimitive], ppp: f32) -> Vec<(u64, Area)> {
    let mut out = Vec::with_capacity(primitives.iter().map(|p| if let Primitive::Mesh(m) = &p.primitive { m.indices.len() / 3 } else { 0 }).sum());
    for p in primitives {
        let Primitive::Mesh(mesh) = &p.primitive else { continue };
        let clip = clip_area(p.clip_rect, ppp);
        let r = p.clip_rect;
        let tex = match mesh.texture_id {
            egui::TextureId::Managed(n) => n << 1,
            egui::TextureId::User(n) => n << 1 | 1,
        };
        let base = mix(mix(mix(mix(mix(0x5A1C_0FF5, r.min.x.to_bits() as u64), r.min.y.to_bits() as u64), r.max.x.to_bits() as u64), r.max.y.to_bits() as u64), tex);
        let blank = Vertex { pos: Default::default(), uv: Default::default(), color: egui::Color32::TRANSPARENT };
        for t in mesh.indices.as_chunks::<3>().0 {
            let vs = [0, 1, 2].map(|k| mesh.vertices.get(t[k] as usize).copied().unwrap_or(blank));
            let mut h = base;
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for v in vs {
                h = mix(h, (v.pos.x.to_bits() as u64) << 32 | v.pos.y.to_bits() as u64);
                h = mix(h, (v.uv.x.to_bits() as u64) << 32 | v.uv.y.to_bits() as u64);
                h = mix(h, u32::from_le_bytes(v.color.to_array()) as u64);
                x0 = x0.min(v.pos.x);
                y0 = y0.min(v.pos.y);
                x1 = x1.max(v.pos.x);
                y1 = y1.max(v.pos.y);
            }
            let f = |v: f32, d: f32| (v * ppp + d).clamp(-1e9, 1e9) as i32;
            let bounds = Area { x0: f(x0, -1.0), y0: f(y0, -1.0), x1: f(x1, 2.0), y1: f(y1, 2.0) };
            out.push((h, bounds.intersect(clip)));
        }
    }
    out
}

/// One more value into a fingerprint.
#[inline]
fn mix(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frame, Painter};
    use egui::{vec2, Color32, Pos2, Rect};

    /// A list with a cursor row, and a small window that can move.
    fn ui(ctx: &egui::Context, cursor: usize, window_x: f32) {
        egui::CentralPanel::default().show(ctx, |ui| {
            for i in 0..14 {
                let text = egui::RichText::new(format!("Soubor číslo {i:02}.txt"));
                ui.label(if i == cursor { text.background_color(Color32::from_rgb(40, 80, 160)).color(Color32::WHITE) } else { text });
            }
        });
        egui::Window::new("Okno").fixed_pos([window_x, 120.0]).show(ctx, |ui| {
            ui.label("Kopírovat 12 souborů");
            ui.add(egui::ProgressBar::new(0.4));
        });
    }

    #[test]
    fn drawing_only_what_changed_is_drawing_all() {
        let (w, h) = (360usize, 300usize);
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.0);
        let mut painter = Painter::new();
        let (mut whole, mut part) = (Frame::new(w, h), Frame::new(w, h));
        let mut damage = Damage::default();
        let clear = Color32::from_gray(27);
        // the cursor moves, stays, jumps; the window moves over the list
        let steps = [(0, 200.0), (0, 200.0), (1, 200.0), (2, 200.0), (2, 200.0), (2, 200.0), (9, 200.0), (9, 150.0), (8, 150.0), (8, 150.0), (8, 150.0)];
        let mut sizes = Vec::new();
        for (step, &(cursor, x)) in steps.iter().enumerate() {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w as f32, h as f32))), ..Default::default() };
            let out = ctx.run(raw, |ctx| ui(ctx, cursor, x));
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            painter.set_textures(&out.textures_delta);
            painter.paint(&mut whole, &prims, 1.0, clear, None);
            let area = damage.of(&prims, 1.0, w, h, &out.textures_delta);
            painter.paint(&mut part, &prims, 1.0, clear, Some(area));
            painter.free_textures(&out.textures_delta);
            assert!(whole.pixels == part.pixels, "step {step}: drawn in part ({area:?}) differs from drawn whole");
            sizes.push(area.pixels());
        }
        assert_eq!(sizes[10], 0, "a frame alike in all draws nothing");
        assert!(sizes[3] > 0 && sizes[3] < w * h / 4, "a cursor moving a row is little: {}", sizes[3]);
    }
}
