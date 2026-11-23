//! The window comparing two files: text side by side, lined up, with the
//! differences colored and a way from one to the next; or what differs in
//! binary files; a find bar finds text on both sides. The window itself, on
//! a thread of its own, is `sal-window`'s.

use std::path::PathBuf;

use sal_core::encoding::{display_path, format_thousands};
use sal_plugin_api::{tr, trf};
use sal_window::egui::{self, Color32, RichText};
use sal_window::Content;

use crate::diff::{self, Outcome, Row, Tag};
use crate::find::{self, Hit, Query};

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
    find: FindBar,
    /// The first row shown: a new search starts there.
    top_row: usize,
    /// Counts the comparisons made: the matches found are for one of them.
    version: u64,
}

/// Finding text (Ctrl+F): what is looked for, and every match.
#[derive(Default)]
struct FindBar {
    open: bool,
    query: Query,
    hits: Vec<Hit>,
    /// More matches than [`find::MAX_HITS`].
    more: bool,
    /// The match the cursor is on.
    current: Option<usize>,
    /// The query and the comparison `hits` are for.
    made_for: Option<(Query, u64)>,
    /// Put the keyboard in the text field (next frame).
    focus: bool,
}

impl FindBar {
    /// Find again when the text, the options or the comparison changed: a
    /// new search starts from the current match, or the first row shown.
    /// The row of a match newly current, to scroll to.
    fn refresh(&mut self, rows: &[Row], version: u64, top_row: usize) -> Option<usize> {
        if !self.open {
            self.hits.clear();
            self.made_for = None;
            self.current = None;
            return None;
        }
        let key = (self.query.clone(), version);
        if self.made_for.as_ref() == Some(&key) {
            return None;
        }
        let from = self.current.and_then(|c| self.hits.get(c)).map_or(top_row, |h| h.row);
        (self.hits, self.more) = find::find_all(rows, &self.query);
        self.current = find::first_from(&self.hits, from);
        self.made_for = Some(key);
        self.current.map(|c| self.hits[c].row)
    }

    /// Go to the next match (or the previous one), round the end; its row.
    fn step(&mut self, forward: bool) -> Option<usize> {
        let n = self.hits.len();
        if n == 0 {
            return None;
        }
        let c = match self.current {
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
            None => 0,
        };
        self.current = Some(c);
        Some(self.hits[c].row)
    }

    /// "3 of 12", "Not found", or nothing yet.
    fn status(&self) -> String {
        if self.query.text.is_empty() {
            return String::new();
        }
        let total = format_thousands(self.hits.len() as u64);
        match self.current {
            None => tr("Not found"),
            Some(c) if self.more => trf("{n} of more than {total}", &[("n", &(c + 1)), ("total", &total)]),
            Some(c) => trf("{n} of {total}", &[("n", &(c + 1)), ("total", &total)]),
        }
    }
}

impl DiffWin {
    /// Compare `left` with `right`.
    pub fn new(left: PathBuf, right: PathBuf) -> DiffWin {
        let mut w = DiffWin { left, right, ignore_space: false, outcome: Err(String::new()), current: 0, scroll_to: None, find: FindBar::default(), top_row: 0, version: 0 };
        w.compare();
        w
    }

    /// Compare the files (again), and go to the first difference.
    fn compare(&mut self) {
        self.outcome = diff::compare(&self.left, &self.right, self.ignore_space).map_err(|e| e.to_string());
        self.current = 0;
        self.scroll_to = self.changes().first().copied();
        self.version += 1;
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

    /// Escape closes the find bar first.
    fn takes_escape(&self) -> bool {
        self.find.open
    }

    fn show(&mut self, ctx: &egui::Context) -> bool {
        let is_text = matches!(self.outcome, Ok(Outcome::Text { .. }));
        // Ctrl+F finds; F3 / Shift+F3 the next / previous match
        let mut step = None;
        if is_text && ctx.input(|i| i.key_pressed(egui::Key::F) && i.modifiers.command) {
            self.find.open = true;
            self.find.focus = true;
        }
        if let Some(shift) = ctx.input(|i| i.key_pressed(egui::Key::F3).then_some(i.modifiers.shift)).filter(|_| is_text) {
            if self.find.open && !self.find.query.text.is_empty() {
                step = Some(!shift);
            } else {
                self.find.open = true;
                self.find.focus = true;
            }
        }
        if self.find.open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.find.open = false;
        }
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
        egui::TopBottomPanel::top("compare_bar").frame(sal_ui::layout::bar_frame(&ctx.style())).show(ctx, |ui| {
            sal_ui::layout::controls(ui);
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
                ui.separator();
                if ui.add_enabled(is_text, egui::SelectableLabel::new(self.find.open, sal_ui::mdi::MAGNIFY)).on_hover_text(tr("Find (Ctrl+F)")).clicked() {
                    self.find.open = !self.find.open;
                    self.find.focus = self.find.open;
                }
            });
        });
        let find = &mut self.find;
        if find.open {
            egui::TopBottomPanel::top("find_bar").frame(sal_ui::layout::bar_frame(&ctx.style())).show(ctx, |ui| {
                sal_ui::layout::controls(ui);
                ui.horizontal(|ui| {
                    ui.label(tr("Find:"));
                    let hint = if find.query.wildcards { tr("text, * any characters, ? one") } else { tr("text") };
                    let field = ui.add(egui::TextEdit::singleline(&mut find.query.text).desired_width(260.0).margin(sal_ui::layout::FIELD_MARGIN).hint_text(hint));
                    if std::mem::take(&mut find.focus) {
                        field.request_focus();
                        select_all(ui, &field, &find.query.text);
                    }
                    // Enter: the next match, Shift+Enter the previous; the keyboard stays here
                    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        step = Some(!ui.input(|i| i.modifiers.shift));
                        field.request_focus();
                    }
                    ui.checkbox(&mut find.query.match_case, tr("Match case")).on_hover_text(tr("Upper and lower case must be the same"));
                    ui.checkbox(&mut find.query.wildcards, tr("Wildcards")).on_hover_text(tr("* stands for any characters, ? for one character"));
                    let any = !find.hits.is_empty();
                    if ui.add_enabled(any, egui::Button::new(sal_ui::mdi::CHEVRON_UP)).on_hover_text(tr("Previous match (Shift+F3)")).clicked() {
                        step = Some(false);
                    }
                    if ui.add_enabled(any, egui::Button::new(sal_ui::mdi::CHEVRON_DOWN)).on_hover_text(tr("Next match (F3, Enter)")).clicked() {
                        step = Some(true);
                    }
                    let status = find.status();
                    if find.current.is_none() && !find.query.text.is_empty() {
                        ui.label(RichText::new(status).color(ui.visuals().warn_fg_color));
                    } else {
                        ui.label(status);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("×").on_hover_text(tr("Close (Esc)")).clicked() {
                            find.open = false;
                        }
                    });
                });
            });
        }
        let rows = match &self.outcome {
            Ok(Outcome::Text { rows, .. }) => rows.as_slice(),
            _ => &[],
        };
        if let Some(row) = find.refresh(rows, self.version, self.top_row) {
            self.scroll_to = Some(row);
        }
        if let Some(row) = step.and_then(|forward| find.step(forward)) {
            self.scroll_to = Some(row);
        }
        egui::CentralPanel::default().frame(sal_ui::layout::content_frame(&ctx.style())).show(ctx, |ui| match &self.outcome {
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            Ok(Outcome::Binary { sizes, first_difference }) => {
                ui.add_space(sal_ui::layout::GAP_LARGE);
                ui.label(format!("{}: {}", display_path(&self.left), trf("{bytes} bytes", &[("bytes", &format_thousands(sizes.0))])));
                ui.label(format!("{}: {}", display_path(&self.right), trf("{bytes} bytes", &[("bytes", &format_thousands(sizes.1))])));
                ui.add_space(sal_ui::layout::GAP);
                ui.label(RichText::new(match first_difference {
                    None => tr("The files are the same."),
                    Some(at) => trf("The files differ from byte {at} on.", &[("at", &format_thousands(*at))]),
                }).strong());
            }
            Ok(Outcome::Text { rows, .. }) => {
                let marks = Marks::new(ui, &self.find.hits, self.find.current);
                self.top_row = text_view(ui, rows, &self.left, &self.right, self.scroll_to.take(), &marks);
            }
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

/// The two texts side by side, a row per line pair, with line numbers and
/// the matches found marked. Returns the first row shown.
fn text_view(ui: &mut egui::Ui, rows: &[Row], left: &std::path::Path, right: &std::path::Path, scroll_to: Option<usize>, marks: &Marks) -> usize {
    let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 2.0;
    let digits = rows.len().max(1).to_string().len();
    let num_w = ui.fonts(|f| f.layout_no_wrap("0".repeat(digits), egui::TextStyle::Monospace.resolve(ui.style()), Color32::WHITE).size().x) + 10.0;
    let dark = ui.visuals().dark_mode;
    let gap = sal_ui::layout::COLUMN_GAP;
    let half = ((ui.available_width() - gap) / 2.0).max(50.0);
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
    let mut top = 0;
    area.show_rows(ui, row_h, rows.len(), |ui, range| {
        top = range.start;
        for (i, row) in range.clone().zip(&rows[range]) {
            let (lf, rf) = fills(row.tag, dark);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (right, side, fill) in [(false, &row.left, lf), (true, &row.right, rf)] {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(half - gap / 2.0, row_h), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, fill);
                    if let Some((n, text)) = side {
                        let mono = egui::TextStyle::Monospace.resolve(ui.style());
                        let weak = ui.visuals().weak_text_color();
                        ui.painter().text(rect.left_center() + egui::vec2(num_w - 6.0, 0.0), egui::Align2::RIGHT_CENTER, n.to_string(), mono.clone(), weak);
                        let clip = egui::Rect::from_min_max(egui::pos2(rect.left() + num_w, rect.top()), rect.max);
                        let job = marks.job(text, i, right, mono, ui.visuals().text_color());
                        let galley = ui.fonts(|f| f.layout_job(job));
                        let at = clip.left_center() - egui::vec2(0.0, galley.size().y / 2.0);
                        ui.painter().with_clip_rect(clip).galley(at, galley, ui.visuals().text_color());
                    }
                }
            });
        }
    });
    top
}

/// Select all the text of a text field.
fn select_all(ui: &egui::Ui, field: &egui::Response, text: &str) {
    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field.id) {
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), field.id);
    }
}

/// The matches to mark in the texts, and their colors (the viewer's).
struct Marks<'a> {
    hits: &'a [Hit],
    current: Option<Hit>,
    other: Color32,
    this: Color32,
}

impl<'a> Marks<'a> {
    fn new(ui: &egui::Ui, hits: &'a [Hit], current: Option<usize>) -> Marks<'a> {
        let dark = ui.visuals().dark_mode;
        Marks {
            hits,
            current: current.and_then(|c| hits.get(c).copied()),
            // every match a soft yellow, the current one orange
            other: if dark { Color32::from_rgba_unmultiplied(220, 180, 0, 70) } else { Color32::from_rgba_unmultiplied(255, 220, 0, 150) },
            this: Color32::from_rgb(255, 150, 0),
        }
    }

    /// One side's text of row `row`, its matches on their background.
    fn job(&self, text: &str, row: usize, right: bool, font: egui::FontId, color: Color32) -> egui::text::LayoutJob {
        use egui::text::{LayoutJob, TextFormat};
        let plain = TextFormat { font_id: font, color, ..Default::default() };
        let mut job = LayoutJob::default();
        let mut at = 0;
        for h in find::on_side(self.hits, row, right) {
            if h.start < at || h.end > text.len() {
                continue;
            }
            job.append(&text[at..h.start], 0.0, plain.clone());
            let current = Some(*h) == self.current;
            let background = if current { self.this } else { self.other };
            // the current match is solid: its text in a color readable on it
            let color = if current { sal_ui::readable_on(background) } else { color };
            job.append(&text[h.start..h.end], 0.0, TextFormat { background, color, ..plain.clone() });
            at = h.end;
        }
        job.append(&text[at..], 0.0, plain);
        job
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: &mut DiffWin, ctx: &egui::Context, events: Vec<egui::Event>) {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 400.0));
        // the keys' modifiers held down, as egui reads them
        let modifiers = events.iter().find_map(|e| if let egui::Event::Key { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
        let raw = egui::RawInput { screen_rect: Some(rect), events, modifiers, ..Default::default() };
        let _ = ctx.run(raw, |ctx| {
            w.show(ctx);
        });
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    /// Ctrl+F opens the find bar, typing finds on both sides, F3 and
    /// Shift+F3 go round the matches, Escape closes the bar.
    #[test]
    fn finds_on_both_sides() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        let mut left = (0..200).map(|i| format!("řádek {i}\n")).collect::<String>();
        left.push_str("kůň vlevo\n");
        let mut right = (0..200).map(|i| format!("řádek {i}\n")).collect::<String>();
        right.push_str("KŮŇ vpravo\nještě kůň\n");
        std::fs::write(&a, left).unwrap();
        std::fs::write(&b, right).unwrap();
        let mut w = DiffWin::new(a, b);
        let ctx = egui::Context::default();
        frame(&mut w, &ctx, vec![]);
        assert!(!w.takes_escape());
        frame(&mut w, &ctx, vec![key(egui::Key::F, egui::Modifiers::COMMAND)]);
        assert!(w.find.open && w.takes_escape());
        frame(&mut w, &ctx, vec![]);
        frame(&mut w, &ctx, vec![egui::Event::Text("kůň".into())]);
        frame(&mut w, &ctx, vec![]);
        assert_eq!(w.find.query.text, "kůň");
        assert_eq!(w.find.hits.len(), 3, "the left side's, both of the right's");
        assert_eq!(w.find.status(), "1 of 3");
        let at = |w: &DiffWin| w.find.current.map(|c| (w.find.hits[c].row, w.find.hits[c].right));
        assert_eq!(at(&w), Some((200, false)));
        frame(&mut w, &ctx, vec![key(egui::Key::F3, egui::Modifiers::NONE)]);
        assert_eq!(at(&w), Some((200, true)));
        frame(&mut w, &ctx, vec![key(egui::Key::F3, egui::Modifiers::SHIFT)]);
        frame(&mut w, &ctx, vec![key(egui::Key::F3, egui::Modifiers::SHIFT)]);
        assert_eq!(at(&w), Some((201, true)), "round the start");
        assert!(w.top_row > 150, "scrolled to the match: {}", w.top_row);
        w.find.query.match_case = true;
        frame(&mut w, &ctx, vec![]);
        assert_eq!(w.find.hits.len(), 2);
        frame(&mut w, &ctx, vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
        frame(&mut w, &ctx, vec![]);
        assert!(!w.find.open && w.find.hits.is_empty());
    }
}
