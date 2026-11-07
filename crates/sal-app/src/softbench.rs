//! `SAL_SOFT=bench`: draws every frame of the main window a second time with
//! the software renderer (`sal-soft`), besides the graphics card, and writes
//! to the debug log how long that took: what drawing without a graphics card
//! would cost on this computer (over Remote Desktop, say). It also says how
//! much of the window changed since the last frame and what redrawing only
//! that took, and saves the 5th frame as a PNG next to the debug log.
//!
//! A measurement only: nothing it draws reaches the screen.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use eframe::egui;
use egui::{LayerId, Order, ViewportId};
use sal_soft::{Area, Frame, Painter};

/// Whether `SAL_SOFT=bench` is set.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("SAL_SOFT").is_ok_and(|v| v.eq_ignore_ascii_case("bench")))
}

struct Bench {
    painter: Painter,
    frame: Frame,
    /// The last frame, to see what changed.
    last: Frame,
    passes: u64,
    /// The font atlas's size when it was last taken.
    font_size: [usize; 2],
}

fn bench() -> &'static Mutex<Bench> {
    static B: OnceLock<Mutex<Bench>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(Bench { painter: Painter::new(), frame: Frame::default(), last: Frame::default(), passes: 0, font_size: [0, 0] }))
}

/// Start measuring the main window's frames, when `SAL_SOFT=bench`.
pub fn install(ctx: &egui::Context) {
    if !enabled() {
        return;
    }
    crate::debuglog::write(format!("startup: software drawing measured alongside ({} threads)", bench().lock().unwrap().painter.threads()));
    ctx.on_end_pass("soft bench", Arc::new(end_pass));
}

/// A texture the program made (an icon), for the software renderer to have too.
pub fn texture(id: egui::TextureId, image: egui::ColorImage) {
    if enabled() {
        let delta = egui::epaint::ImageDelta::full(image, egui::TextureOptions::LINEAR);
        bench().lock().unwrap().painter.set_texture(id, &delta);
    }
}

/// The shapes of the frame just made, in the order egui draws them (layer
/// transforms left out: the program uses none).
fn shapes(ctx: &egui::Context) -> Vec<egui::epaint::ClippedShape> {
    let listed: Vec<LayerId> = ctx.memory(|m| m.layer_ids().collect());
    // the panels' layer isn't an area's: not listed
    let extra = [LayerId::background(), LayerId::debug()];
    ctx.graphics(|g| {
        let mut all = Vec::new();
        for order in [Order::Background, Order::PanelResizeLine, Order::Middle, Order::Foreground, Order::Tooltip, Order::Debug] {
            let ids = listed.iter().chain(extra.iter().filter(|l| !listed.contains(l)));
            for id in ids.filter(|l| l.order == order) {
                if let Some(list) = g.get(*id) {
                    all.extend(list.all_entries().cloned());
                }
            }
        }
        all
    })
}

fn end_pass(ctx: &egui::Context) {
    if ctx.viewport_id() != ViewportId::ROOT {
        return;
    }
    let mut b = bench().lock().unwrap();
    let b = &mut *b;
    b.passes += 1;
    let ppp = ctx.pixels_per_point();
    let size = ctx.screen_rect().size() * ppp;
    let (w, h) = (size.x.round() as usize, size.y.round() as usize);
    if w == 0 || h == 0 {
        return;
    }
    // the font atlas: when it grew, and now and then for new glyphs
    let font_size = ctx.fonts(|f| f.font_image_size());
    if font_size != b.font_size || b.passes % 10 == 1 {
        let image = ctx.fonts(|f| f.image());
        let delta = egui::epaint::ImageDelta::full(image, egui::TextureOptions::LINEAR);
        b.painter.set_texture(egui::TextureId::default(), &delta);
        b.font_size = font_size;
    }
    let t0 = Instant::now();
    let prims = ctx.tessellate(shapes(ctx), ppp);
    let t1 = Instant::now();
    let clear = ctx.style().visuals.panel_fill;
    std::mem::swap(&mut b.frame, &mut b.last);
    b.frame.resize(w, h);
    let stats = b.painter.paint(&mut b.frame, &prims, ppp, clear, None);
    let t2 = Instant::now();
    // what changed, and redrawing only that
    let changed = if (b.last.width, b.last.height) == (w, h) { changed(&b.last, &b.frame) } else { b.frame.area() };
    let t3 = Instant::now();
    if !changed.is_empty() {
        b.painter.paint(&mut b.frame, &prims, ppp, clear, Some(changed));
    }
    let t4 = Instant::now();
    let ms = |a: Instant, b: Instant| crate::debuglog::ms(b - a);
    crate::debuglog::write(format!(
        "··· soft     {w}×{h}: whole window {} (+{} shapes to triangles), {} rectangles {} triangles; changed {}×{} at {},{} ({:.1} %), redrawn in {}",
        ms(t1, t2),
        ms(t0, t1),
        stats.rects,
        stats.triangles,
        changed.x1 - changed.x0,
        changed.y1 - changed.y0,
        changed.x0,
        changed.y0,
        changed.pixels() as f64 * 100.0 / (w * h) as f64,
        ms(t3, t4),
    ));
    if b.passes == 5 {
        save(&b.frame);
    }
}

/// The smallest area holding every pixel that differs.
fn changed(a: &Frame, b: &Frame) -> Area {
    let mut area = Area::default();
    for (y, (ra, rb)) in a.pixels.chunks_exact(a.width).zip(b.pixels.chunks_exact(b.width)).enumerate() {
        if ra == rb {
            continue;
        }
        let x0 = ra.iter().zip(rb).position(|(p, q)| p != q).unwrap_or(0);
        let x1 = a.width - ra.iter().rev().zip(rb.iter().rev()).position(|(p, q)| p != q).unwrap_or(0);
        area = area.union(Area { x0: x0 as i32, y0: y as i32, x1: x1 as i32, y1: y as i32 + 1 });
    }
    area
}

/// Save a frame next to the debug log, to compare with the screen.
fn save(frame: &Frame) {
    let Some(log) = crate::debuglog::path() else { return };
    let path = log.with_extension("soft.png");
    let r = (|| -> Result<(), Box<dyn std::error::Error>> {
        let file = std::fs::File::create(&path)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), frame.width as u32, frame.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.write_header()?.write_image_data(&frame.rgba())?;
        Ok(())
    })();
    crate::debuglog::write(match r {
        Ok(()) => format!("··· soft     frame saved: {}", path.display()),
        Err(e) => format!("··· soft     frame not saved to {}: {e}", path.display()),
    });
}
