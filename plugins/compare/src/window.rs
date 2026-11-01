//! The window comparing two files: text side by side, lined up, with the
//! differences colored and a way from one to the next; or what differs in
//! binary files. The window itself, on a thread of its own, is
//! `sal-window`'s.

use std::path::PathBuf;

use sal_core::encoding::{display_path, format_thousands};
use sal_plugin_api::{tr, trf};
use sal_window::egui::{self, Color32, RichText};
use sal_window::Content;

use crate::diff::{self, Outcome, Row, Tag};

/// A window comparing two files.
pub struct DiffWin {
    left: PathBuf,
    right: PathBuf,
    ignore_space: bool,
    outcome: Result<Outcome, String>,
    /// The block of differences shown last (an index into `changes`).
    current: usize,
    /// Scroll to this row in the next frame.
    scroll_to: Option<usize>,
}

impl DiffWin {
    /// Compare `left` with `right`.
    pub fn new(left: PathBuf, right: PathBuf) -> DiffWin {
        let mut w = DiffWin { left, right, ignore_space: false, outcome: Err(String::new()), current: 0, scroll_to: None };
        w.compare();
        w
    }

    /// Compare the files (again), and go to the first difference.
    fn compare(&mut self) {
        self.outcome = diff::compare(&self.left, &self.right, self.ignore_space).map_err(|e| e.to_string());
        self.current = 0;
        self.scroll_to = self.changes().first().copied();
    }

    /// The first rows of the blocks of differences.
    fn changes(&self) -> &[usize] {
        match &self.outcome {
            Ok(Outcome::Text { changes, .. }) => changes,
            _ => &[],
        }
    }

    /// Go to the next (`forward`) or the previous block of differences.
    fn step(&mut self, forward: bool) {
        let n = self.changes().len();
        if n == 0 {
            return;
        }
        self.current = if forward { (self.current + 1).min(n - 1) } else { self.current.saturating_sub(1) };
        self.scroll_to = Some(self.changes()[self.current]);
    }
}

/// A file's name, for the title.
fn name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| display_path(p))
}

impl Content for DiffWin {
    fn title(&self) -> String {
        format!("{} ↔ {} - {}", name(&self.left), name(&self.right), tr("Compare"))
    }

    fn show(&mut self, ctx: &egui::Context) -> bool {
        let (next, prev) = ctx.input(|i| {
            let alt = i.modifiers.alt;
            (i.key_pressed(egui::Key::F8) && !i.modifiers.shift || alt && i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::F8) && i.modifiers.shift || alt && i.key_pressed(egui::Key::ArrowUp))
        });
        if next {
            self.step(true);
        }
        if prev {
            self.step(false);
        }
        egui::TopBottomPanel::top("compare_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let n = self.changes().len();
                let text = match &self.outcome {
                    Ok(Outcome::Text { .. }) if n == 0 => tr("The texts are the same."),
                    Ok(Outcome::Text { .. }) => trf("Difference {at} of {n}", &[("at", &(self.current + 1)), ("n", &n)]),
                    Ok(Outcome::Binary { .. }) => tr("Binary files"),
                    Err(_) => String::new(),
                };
                ui.label(RichText::new(text).strong());
                ui.separator();
                if ui.add_enabled(n > 0, egui::Button::new(tr("Previous"))).on_hover_text("Shift+F8, Alt+↑").clicked() {
                    self.step(false);
                }
                if ui.add_enabled(n > 0, egui::Button::new(tr("Next"))).on_hover_text("F8, Alt+↓").clicked() {
                    self.step(true);
                }
                ui.separator();
                if ui.checkbox(&mut self.ignore_space, tr("Ignore spaces")).on_hover_text(tr("Lines differing only in spaces and tabs count as the same")).changed() {
                    self.compare();
                }
                if ui.button(tr("Compare again")).on_hover_text(tr("Read the files again")).clicked() {
                    self.compare();
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| match &self.outcome {
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            Ok(Outcome::Binary { sizes, first_difference }) => {
                ui.add_space(12.0);
                ui.label(format!("{}: {}", display_path(&self.left), trf("{bytes} bytes", &[("bytes", &format_thousands(sizes.0))])));
                ui.label(format!("{}: {}", display_path(&self.right), trf("{bytes} bytes", &[("bytes", &format_thousands(sizes.1))])));
                ui.add_space(8.0);
                ui.label(RichText::new(match first_difference {
                    None => tr("The files are the same."),
                    Some(at) => trf("The files differ from byte {at} on.", &[("at", &format_thousands(*at))]),
                }).strong());
            }
            Ok(Outcome::Text { rows, .. }) => text_view(ui, rows, &self.left, &self.right, self.scroll_to.take()),
        });
        true
    }
}

/// The colors behind a row's two sides.
fn fills(tag: Tag, dark: bool) -> (Color32, Color32) {
    let a = if dark { 16 } else { 19 };
    let amber = Color32::from_rgba_unmultiplied(230, 160, 30, a);
    let red = Color32::from_rgba_unmultiplied(220, 70, 60, a);
    let green = Color32::from_rgba_unmultiplied(60, 180, 90, a);
    let none = Color32::TRANSPARENT;
    let gap = if dark { Color32::from_white_alpha(8) } else { Color32::from_black_alpha(10) };
    match tag {
        Tag::Same => (none, none),
        Tag::Changed => (amber, amber),
        Tag::Removed => (red, gap),
        Tag::Added => (gap, green),
    }
}

/// The two texts side by side, a row per line pair, with line numbers.
fn text_view(ui: &mut egui::Ui, rows: &[Row], left: &std::path::Path, right: &std::path::Path, scroll_to: Option<usize>) {
    let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 2.0;
    let digits = rows.len().max(1).to_string().len();
    let num_w = ui.fonts(|f| f.layout_no_wrap("0".repeat(digits), egui::TextStyle::Monospace.resolve(ui.style()), Color32::WHITE).size().x) + 10.0;
    let dark = ui.visuals().dark_mode;
    let half = ((ui.available_width() - 6.0) / 2.0).max(50.0);
    ui.horizontal(|ui| {
        for p in [left, right] {
            ui.add_sized([half, row_h], egui::Label::new(RichText::new(display_path(p)).weak()).truncate());
        }
    });
    let mut area = egui::ScrollArea::vertical().auto_shrink(false).scroll_bar_visibility(sal_ui::scroll_bar_visibility(ui));
    if let Some(r) = scroll_to {
        // the difference a few rows below the top
        area = area.vertical_scroll_offset((r.saturating_sub(3) as f32) * (row_h + ui.spacing().item_spacing.y));
    }
    area.show_rows(ui, row_h, rows.len(), |ui, range| {
        for row in &rows[range] {
            let (lf, rf) = fills(row.tag, dark);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (side, fill) in [(&row.left, lf), (&row.right, rf)] {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(half - 3.0, row_h), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, fill);
                    if let Some((n, text)) = side {
                        let mono = egui::TextStyle::Monospace.resolve(ui.style());
                        let weak = ui.visuals().weak_text_color();
                        ui.painter().text(rect.left_center() + egui::vec2(num_w - 6.0, 0.0), egui::Align2::RIGHT_CENTER, n.to_string(), mono.clone(), weak);
                        let clip = egui::Rect::from_min_max(egui::pos2(rect.left() + num_w, rect.top()), rect.max);
                        ui.painter().with_clip_rect(clip).text(clip.left_center(), egui::Align2::LEFT_CENTER, text, mono, ui.visuals().text_color());
                    }
                }
            });
        }
    });
}
