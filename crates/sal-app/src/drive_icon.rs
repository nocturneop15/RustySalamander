//! Drive icons (Material Design Icons): each drive type has its own icon and
//! color, so local and network drives can be told apart at a glance in
//! either theme.

use eframe::egui::{self, pos2, vec2, Color32, Painter, Rect, Response, Sense, Ui};
use sal_ui::mdi;
use sal_core::drives::DriveType;

/// The color of a drive type's icon.
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

/// The icon of a drive type (Material Design Icons).
pub fn glyph(kind: DriveType) -> &'static str {
    match kind {
        DriveType::Fixed => mdi::HARDDISK,
        DriveType::Network => mdi::FOLDER_NETWORK,
        DriveType::Removable => mdi::USB_FLASH_DRIVE,
        DriveType::CdRom => mdi::DISC,
        DriveType::RamDisk => mdi::MEMORY,
        DriveType::Unknown => mdi::HELP_CIRCLE_OUTLINE,
    }
}

/// Draw the icon for `kind` into the square `r`, in its color.
pub fn paint(p: &Painter, r: Rect, kind: DriveType) {
    paint_glyph(p, r, glyph(kind), color(kind));
}

/// Draw an icon (one of [`mdi`]) filling the square `r`.
pub fn paint_glyph(p: &Painter, r: Rect, glyph: &str, color: Color32) {
    let s = r.width().min(r.height());
    p.text(r.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(s), color);
}

/// A button showing the drive icon followed by `text` (may be empty).
pub fn button(ui: &mut Ui, kind: DriveType, text: &str, selected: bool) -> Response {
    glyph_button(ui, glyph(kind), color(kind), text, selected)
}

/// A button showing `icon` (one of [`mdi`]) in `icon_color`, followed by
/// `text` (may be empty).
pub fn glyph_button(ui: &mut Ui, icon_glyph: &str, icon_color: Color32, text: &str, selected: bool) -> Response {
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
        // not grown when hovered or pressed (as egui's buttons are), and the
        // frame inside the button, not across its edge: the row sits at the
        // panel's top, where whatever reaches above is cut off
        ui.painter().rect(rect.shrink(v.bg_stroke.width / 2.0), v.rounding, fill, v.bg_stroke);
        let ir = Rect::from_min_size(pos2(rect.left() + pad.x, rect.center().y - icon / 2.0), vec2(icon, icon));
        paint_glyph(ui.painter(), ir, icon_glyph, icon_color);
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
        paint(p, ir, kind);
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

/// A full-width row for an open connection: `icon` (one of [`mdi`]) and its
/// name, which ends `reserve` points before the right edge (room for a
/// button) and is shortened with "…" when it doesn't fit.
pub fn connection_row(ui: &mut Ui, icon: &str, label: &str, selected: bool, reserve: f32) -> Response {
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
        let size = h - 6.0;
        let ir = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - size / 2.0), vec2(size, size));
        paint_glyph(p, ir, icon, color(DriveType::Network));
        let fg = if selected { vis.selection.stroke.color } else { vis.text_color() };
        let font = egui::TextStyle::Body.resolve(ui.style());
        let x = ir.right() + 8.0;
        let room = (rect.right() - reserve - 8.0 - x).max(10.0);
        let g = fit_head(p, label, &font, fg, room);
        p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, fg);
    }
    resp
}

/// Fit `text` into `max_w` by cutting from the right ("scp://admin@long-na…").
fn fit_head(p: &egui::Painter, text: &str, font: &egui::FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let g = p.layout_no_wrap(text.to_owned(), font.clone(), color);
    if g.size().x <= max_w {
        return g;
    }
    let chars: Vec<char> = text.chars().collect();
    for keep in (1..chars.len()).rev() {
        let t: String = chars[..keep].iter().copied().chain(std::iter::once('…')).collect();
        let g = p.layout_no_wrap(t, font.clone(), color);
        if g.size().x <= max_w {
            return g;
        }
    }
    p.layout_no_wrap("…".into(), font.clone(), color)
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
        paint(p, ir, kind);
        let fg = if selected { vis.selection.stroke.color } else { vis.text_color() };
        let font = egui::TextStyle::Body.resolve(ui.style());
        let x = ir.right() + 8.0;
        let g = fit_tail(p, path, &font, fg, rect.right() - 8.0 - x);
        p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, fg);
    }
    resp
}
