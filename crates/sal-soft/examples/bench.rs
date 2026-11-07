//! Draws a screen like the program's (two panels of files) and says how long
//! a frame takes; saves it as a PNG to look at.
//!
//! cargo run --release -p sal-soft --example bench -- [width] [height] [scale] [out.png]

use std::time::Instant;

use egui::{vec2, Color32, Pos2, Rect};
use sal_soft::{Area, Frame, Painter};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, d: f32| args.get(i).and_then(|a| a.parse().ok()).unwrap_or(d);
    let (w, h, ppp) = (num(0, 1920.0), num(1, 1080.0), num(2, 1.0));
    let out = args.get(3).cloned().unwrap_or_else(|| "soft.png".into());

    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());
    let mut raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w / ppp, h / ppp))), ..Default::default() };
    raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
    let mut painter = Painter::new();
    let mut frame = Frame::new(w as usize, h as usize);
    let mut cursor = 3;
    let mut times = Vec::new();
    let mut last = None;
    for pass in 0..60 {
        cursor = (cursor + 1) % 40;
        let t0 = Instant::now();
        let full = ctx.run(raw.clone(), |ctx| ui(ctx, cursor));
        let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
        let t1 = Instant::now();
        painter.set_textures(&full.textures_delta);
        let stats = painter.paint(&mut frame, &prims, ppp, Color32::from_gray(27), None);
        painter.free_textures(&full.textures_delta);
        let t2 = Instant::now();
        if pass >= 10 {
            times.push(((t1 - t0).as_secs_f64() * 1000.0, (t2 - t1).as_secs_f64() * 1000.0));
        }
        last = Some(stats);
    }
    let avg = |f: fn(&(f64, f64)) -> f64| times.iter().map(f).sum::<f64>() / times.len() as f64;
    let min = times.iter().map(|t| t.1).fold(f64::MAX, f64::min);
    println!("{w}x{h} at {ppp}x, {} threads: egui {:.2} ms, drawing {:.2} ms (best {min:.2}), {:?}", painter.threads(), avg(|t| t.0), avg(|t| t.1), last.unwrap());

    // one thread, and only two rows of the panel redrawn (a cursor moved)
    let full = ctx.run(raw.clone(), |ctx| ui(ctx, cursor));
    let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
    painter.set_textures(&full.textures_delta);
    let row = Area { x0: 0, y0: (60.0 * ppp) as i32, x1: (w / 2.0) as i32, y1: (100.0 * ppp) as i32 };
    for (threads, region) in [(1, None), (painter.threads(), Some(row)), (1, Some(row))] {
        painter.set_threads(threads);
        let t = Instant::now();
        for _ in 0..20 {
            painter.paint(&mut frame, &prims, ppp, Color32::from_gray(27), region);
        }
        println!("{threads} threads, {}: {:.2} ms", if region.is_some() { "two rows" } else { "whole" }, t.elapsed().as_secs_f64() * 1000.0 / 20.0);
    }

    let file = std::fs::File::create(&out).expect("create the PNG");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), frame.width as u32, frame.height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.write_header().unwrap().write_image_data(&frame.rgba()).unwrap();
    println!("saved {out}");
}

/// Something like the program's main window.
fn ui(ctx: &egui::Context, cursor: usize) {
    egui::TopBottomPanel::top("menu").show(ctx, |ui| {
        ui.horizontal(|ui| {
            for m in ["Files", "Edit", "View", "Commands", "Plugins", "Options", "Help"] {
                let _ = ui.button(m);
            }
        });
    });
    egui::TopBottomPanel::bottom("keys").show(ctx, |ui| {
        ui.horizontal(|ui| {
            for k in ["F3 View", "F4 Edit", "F5 Copy", "F6 Move", "F7 New folder", "F8 Delete", "F10 Quit"] {
                let _ = ui.button(k);
            }
        });
    });
    egui::SidePanel::left("left").exact_width(ctx.screen_rect().width() / 2.0).show(ctx, |ui| panel(ui, "C:\\Users\\kokes\\Documents", cursor));
    egui::CentralPanel::default().show(ctx, |ui| panel(ui, "D:\\Projects\\RustySalamander\\crates", 99));
    egui::Window::new("Copy").default_pos([600.0, 300.0]).show(ctx, |ui| {
        ui.label("Copy 12 files (3,4 MB) to");
        ui.text_edit_singleline(&mut "D:\\Backup\\".to_string());
        ui.checkbox(&mut true, "Overwrite older files");
        ui.add(egui::ProgressBar::new(0.37).show_percentage());
        ui.horizontal(|ui| {
            let _ = ui.button("Copy");
            let _ = ui.button("Cancel");
        });
    });
}

fn panel(ui: &mut egui::Ui, path: &str, cursor: usize) {
    ui.label(egui::RichText::new(path).strong());
    ui.separator();
    egui::Grid::new(path).striped(true).num_columns(4).show(ui, |ui| {
        for h in ["Name", "Ext", "Size", "Date"] {
            ui.label(egui::RichText::new(h).strong());
        }
        ui.end_row();
        for i in 0..60 {
            let name = format!("Příliš žluťoučký soubor {i:02}");
            if i == cursor {
                ui.label(egui::RichText::new(name).background_color(Color32::from_rgb(40, 80, 160)).color(Color32::WHITE));
            } else {
                ui.label(name);
            }
            ui.label(["txt", "rs", "exe", "png"][i % 4]);
            ui.label(format!("{} 512", i * 37));
            ui.label(format!("2026-11-{:02} 12:{:02}", i % 28 + 1, i % 60));
            ui.end_row();
        }
    });
}
