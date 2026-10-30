//! Vector drive icons: each drive type has its own shape and color, so local
//! and network drives can be told apart at a glance in either theme.

use eframe::egui::{self, pos2, vec2, Color32, Painter, Rect, Response, Sense, Stroke, Ui};
use sal_core::drives::DriveType;

pub fn color(kind: DriveType) -> Color32 {
    match kind {
        DriveType::Fixed => Color32::from_rgb(98, 140, 196),
        DriveType::Network => Color32::from_rgb(52, 168, 112),
        DriveType::Removable => Color32::from_rgb(226, 150, 50),
        DriveType::CdRom => Color32::from_rgb(150, 116, 214),
        DriveType::RamDisk => Color32::from_rgb(112, 160, 160),
        DriveType::Unknown => Color32::from_gray(140),
    }
}

/// Draw the icon for `kind` into the square `r`; `bg` fills holes (CD centre).
pub fn paint(p: &Painter, r: Rect, kind: DriveType, bg: Color32) {
    let s = r.width().min(r.height());
    let r = Rect::from_center_size(r.center(), vec2(s, s));
    let c = color(kind);
    let dark = c.gamma_multiply(0.55);
    let led = Color32::from_rgb(120, 235, 120);
    let at = |x: f32, y: f32| pos2(r.left() + x * s, r.top() + y * s);
    // disk body with a slot and an activity LED
    let disk = |body: Rect| {
        p.rect_filled(body, 2.0, c);
        let cy = body.center().y;
        p.line_segment([pos2(body.left() + 0.14 * s, cy), pos2(body.left() + 0.52 * s, cy)], Stroke::new(1.2f32, dark));
        p.circle_filled(pos2(body.right() - 0.16 * s, cy), 0.075 * s, led);
    };
    match kind {
        DriveType::Fixed => disk(Rect::from_min_max(at(0.0, 0.26), at(1.0, 0.80))),
        DriveType::Network => {
            disk(Rect::from_min_max(at(0.04, 0.02), at(0.96, 0.48)));
            // cable down to a network bus
            let w = Stroke::new((0.11 * s).max(1.4), c);
            p.line_segment([at(0.5, 0.48), at(0.5, 0.86)], w);
            p.line_segment([at(0.08, 0.86), at(0.92, 0.86)], w);
            p.rect_filled(Rect::from_center_size(at(0.5, 0.86), vec2(0.22 * s, 0.18 * s)), 1.0, c);
        }
        DriveType::Removable => {
            p.rect_filled(Rect::from_min_max(at(0.0, 0.30), at(0.70, 0.72)), 2.0, c);
            p.rect_filled(Rect::from_min_max(at(0.70, 0.37), at(1.0, 0.65)), 1.0, Color32::from_gray(150));
            p.rect_filled(Rect::from_min_max(at(0.80, 0.44), at(0.88, 0.50)), 0.0, Color32::from_gray(90));
        }
        DriveType::CdRom => {
            p.circle_filled(r.center(), 0.46 * s, c);
            p.circle_stroke(r.center(), 0.30 * s, Stroke::new(1.0f32, c.gamma_multiply(1.4)));
            p.circle_filled(r.center(), 0.12 * s, bg);
        }
        DriveType::RamDisk => {
            let chip = Rect::from_min_max(at(0.18, 0.22), at(0.82, 0.78));
            for i in 0..3 {
                let x = 0.30 + 0.20 * i as f32;
                p.line_segment([at(x, 0.08), at(x, 0.92)], Stroke::new(1.2f32, dark));
            }
            p.rect_filled(chip, 2.0, c);
        }
        DriveType::Unknown => {
            p.rect_stroke(Rect::from_min_max(at(0.05, 0.26), at(0.95, 0.80)), 2.0, Stroke::new(1.2f32, c));
        }
    }
}

/// A button showing the drive icon followed by `text` (may be empty).
pub fn button(ui: &mut Ui, kind: DriveType, text: &str, selected: bool) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, ui.visuals().text_color());
    let icon = galley.size().y.max(14.0);
    let pad = ui.spacing().button_padding;
    let gap = if text.is_empty() { 0.0 } else { 4.0 };
    let size = vec2(pad.x * 2.0 + icon + gap + galley.size().x, pad.y * 2.0 + icon);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact_selectable(&resp, selected);
        let fill = if selected { ui.visuals().selection.bg_fill } else { v.weak_bg_fill };
        ui.painter().rect(rect.expand(v.expansion), v.rounding, fill, v.bg_stroke);
        let ir = Rect::from_min_size(pos2(rect.left() + pad.x, rect.center().y - icon / 2.0), vec2(icon, icon));
        paint(ui.painter(), ir, kind, fill);
        let tc = if selected { ui.visuals().selection.stroke.color } else { v.text_color() };
        ui.painter().galley(pos2(ir.right() + gap, rect.center().y - galley.size().y / 2.0), galley, tc);
    }
    resp
}

/// A full-width row for the drive menu: icon, letter, label, type and share.
pub fn row(ui: &mut Ui, kind: DriveType, letter: &str, label: &str, detail: &str, selected: bool) -> Response {
    let h = ui.text_style_height(&egui::TextStyle::Body) + 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    if ui.is_rect_visible(rect) {
        let vis = ui.visuals();
        let fill = if selected {
            vis.selection.bg_fill
        } else if resp.hovered() {
            vis.widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        let p = ui.painter();
        p.rect_filled(rect, 3.0, fill);
        let icon = h - 6.0;
        let ir = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - icon / 2.0), vec2(icon, icon));
        paint(p, ir, kind, if selected { vis.selection.bg_fill } else { vis.window_fill });
        let fg = if selected { vis.selection.stroke.color } else { vis.text_color() };
        let weak = if selected { fg } else { vis.weak_text_color() };
        let body = egui::TextStyle::Body.resolve(ui.style());
        let mut x = ir.right() + 8.0;
        let cy = rect.center().y;
        let g = p.layout_no_wrap(letter.to_owned(), egui::FontId::new(body.size, body.family.clone()), fg);
        p.galley(pos2(x, cy - g.size().y / 2.0), g.clone(), fg);
        x += g.size().x.max(body.size * 1.6) + 8.0;
        let g = p.layout_no_wrap(label.to_owned(), body.clone(), fg);
        p.galley(pos2(x, cy - g.size().y / 2.0), g.clone(), fg);
        let g2 = p.layout_no_wrap(detail.to_owned(), body, weak);
        // the type/share column is right-aligned, but never overlaps the label
        let dx = (rect.right() - 8.0 - g2.size().x).max(x + g.size().x + 12.0);
        p.galley(pos2(dx, cy - g2.size().y / 2.0), g2, weak);
    }
    resp
}

/// Fit `text` into `max_w` by cutting from the left ("…\deep\folder"), so the
/// end of a path stays readable.
fn fit_tail(p: &egui::Painter, text: &str, font: &egui::FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let g = p.layout_no_wrap(text.to_owned(), font.clone(), color);
    if g.size().x <= max_w {
        return g;
    }
    let chars: Vec<char> = text.chars().collect();
    for skip in 1..chars.len() {
        let t: String = std::iter::once('…').chain(chars[skip..].iter().copied()).collect();
        let g = p.layout_no_wrap(t, font.clone(), color);
        if g.size().x <= max_w {
            return g;
        }
    }
    p.layout_no_wrap("…".into(), font.clone(), color)
}

/// A full-width row for a remembered path: drive or network icon and the path.
pub fn path_row(ui: &mut Ui, kind: DriveType, path: &str, selected: bool) -> Response {
    let h = ui.text_style_height(&egui::TextStyle::Body) + 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    if ui.is_rect_visible(rect) {
        let vis = ui.visuals();
        let fill = if selected {
            vis.selection.bg_fill
        } else if resp.hovered() {
            vis.widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        let p = ui.painter();
        p.rect_filled(rect, 3.0, fill);
        let icon = h - 6.0;
        let ir = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - icon / 2.0), vec2(icon, icon));
        paint(p, ir, kind, if selected { vis.selection.bg_fill } else { vis.window_fill });
        let fg = if selected { vis.selection.stroke.color } else { vis.text_color() };
        let font = egui::TextStyle::Body.resolve(ui.style());
        let x = ir.right() + 8.0;
        let g = fit_tail(p, path, &font, fg, rect.right() - 8.0 - x);
        p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, fg);
    }
    resp
}
