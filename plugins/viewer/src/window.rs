//! The viewer's windows: a file as text (colored by its syntax) or hex, with
//! a find bar. The windows themselves, on a thread of their own, are
//! `sal-window`'s.

use sal_core::encoding;
use sal_plugin_api::{tr, trf};
use sal_window::egui::{self, Color32, RichText};
use sal_window::Content;

use crate::doc::{Colors, Doc};
use crate::find::{self, Hit, Query};
use crate::highlight::{self, Span};

/// A viewer window's content: a file as text or hex.
pub struct ViewerWin {
    pub doc: Doc,
    find: FindBar,
    /// The first line shown: a new search starts there.
    top_line: usize,
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
    /// The query and the document version `hits` are for.
    made_for: Option<(Query, u64)>,
    /// Put the keyboard in the text field (next frame).
    focus: bool,
    /// Bring the current match into view (next frame).
    reveal: bool,
}

impl FindBar {
    /// Find again when the text, the options or the document changed. A new
    /// search starts from the current match, or the first line shown.
    fn refresh(&mut self, doc: &Doc, top_line: usize) {
        if !self.open {
            self.hits.clear();
            self.made_for = None;
            self.current = None;
            return;
        }
        let key = (self.query.clone(), doc.version());
        if self.made_for.as_ref() == Some(&key) {
            return;
        }
        let from = self.current.and_then(|c| self.hits.get(c)).map_or(top_line, |h| h.line);
        (self.hits, self.more) = find::find_all(&doc.lines, &self.query);
        self.current = find::first_from(&self.hits, from);
        self.reveal = self.current.is_some();
        self.made_for = Some(key);
    }

    /// Go to the next match (or the previous one), round the end.
    fn step(&mut self, forward: bool) {
        let n = self.hits.len();
        if n == 0 {
            return;
        }
        self.current = Some(match self.current {
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
            None => 0,
        });
        self.reveal = true;
    }

    /// "3 of 12", "Not found", or nothing yet.
    fn status(&self) -> String {
        if self.query.text.is_empty() {
            return String::new();
        }
        match self.current {
            None => tr("Not found"),
            Some(c) if self.more => trf("{n} of more than {total}", &[("n", &(c + 1)), ("total", &encoding::format_thousands(self.hits.len() as u64))]),
            Some(c) => trf("{n} of {total}", &[("n", &(c + 1)), ("total", &encoding::format_thousands(self.hits.len() as u64))]),
        }
    }
}

impl ViewerWin {
    pub fn new(doc: Doc) -> ViewerWin {
        ViewerWin { doc, find: FindBar::default(), top_line: 0 }
    }
}

impl Content for ViewerWin {
    fn title(&self) -> String {
        self.doc.title.clone()
    }

    /// Escape closes the find bar first.
    fn takes_escape(&self) -> bool {
        self.find.open
    }

    /// The toolbar (Text/Hex, wrap, encoding, syntax, size), the find bar,
    /// and the text or the bytes.
    fn show(&mut self, ctx: &egui::Context) -> bool {
        let doc = &mut self.doc;
        let find = &mut self.find;
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint_of(egui::ViewportId::ROOT)
        };
        let colors = doc.colors(ctx.style().visuals.dark_mode, wake);
        if ctx.input(|i| i.key_pressed(egui::Key::F4)) {
            doc.hex = !doc.hex;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::W) && i.modifiers.ctrl) && doc.can_wrap() {
            doc.wrap = !doc.wrap;
        }
        // Ctrl+F finds; F3 / Shift+F3 the next / previous match
        let mut step = None;
        if ctx.input(|i| i.key_pressed(egui::Key::F) && i.modifiers.command) {
            find.open = true;
            find.focus = true;
        }
        if let Some(shift) = ctx.input(|i| i.key_pressed(egui::Key::F3).then_some(i.modifiers.shift)) {
            if find.open && !find.query.text.is_empty() {
                step = Some(!shift);
            } else {
                find.open = true;
                find.focus = true;
            }
        }
        if find.open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            find.open = false;
        }
        egui::TopBottomPanel::top("viewer_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut doc.hex, false, tr("Text"));
                ui.selectable_value(&mut doc.hex, true, tr("Hex")).on_hover_text(tr("F4 toggles"));
                ui.separator();
                ui.add_enabled_ui(!doc.hex && doc.can_wrap(), |ui| {
                    ui.checkbox(&mut doc.wrap, tr("Wrap")).on_hover_text("Ctrl+W");
                });
                ui.separator();
                let mut chosen = None;
                egui::ComboBox::from_id_salt("enc").selected_text(doc.encoding.name()).show_ui(ui, |ui| {
                    for enc in encoding::encoding_choices() {
                        if ui.selectable_label(enc == doc.encoding, enc.name()).clicked() {
                            chosen = Some(enc);
                        }
                    }
                });
                if let Some(enc) = chosen {
                    doc.set_encoding(enc);
                }
                ui.separator();
                let mut picked = None;
                let shown = doc.syntax.clone().unwrap_or_else(|| tr("Plain text"));
                ui.add_enabled_ui(!doc.hex, |ui| {
                    egui::ComboBox::from_id_salt("syntax").selected_text(shown).height(420.0).show_ui(ui, |ui| {
                        if ui.selectable_label(doc.syntax.is_none(), tr("Plain text")).clicked() {
                            picked = Some(None);
                        }
                        for name in highlight::names() {
                            if ui.selectable_label(doc.syntax.as_deref() == Some(name.as_str()), name.as_str()).clicked() {
                                picked = Some(Some(name.clone()));
                            }
                        }
                    })
                    .response
                    .on_hover_text(tr("Colors the text by its format; detected from the file name or its first line"));
                });
                if let Some(syntax) = picked {
                    doc.set_syntax(syntax);
                }
                ui.separator();
                if ui.selectable_label(find.open, sal_ui::mdi::MAGNIFY).on_hover_text(tr("Find (Ctrl+F)")).clicked() {
                    find.open = !find.open;
                    find.focus = find.open;
                }
                match colors {
                    Colors::Working => {
                        ui.label(RichText::new(tr("Coloring…")).weak());
                    }
                    Colors::TooLarge => {
                        ui.label(RichText::new(tr("Too large to color")).weak());
                    }
                    Colors::Off | Colors::Ready => {}
                }
                ui.separator();
                let mut info = trf("{bytes} bytes", &[("bytes", &encoding::format_thousands(doc.file_len))]);
                if doc.file_len > doc.len() as u64 {
                    info.push_str(&trf(" (showing first {size})", &[("size", &encoding::format_size(doc.len() as u64))]));
                }
                ui.label(info);
            });
        });
        if find.open {
            egui::TopBottomPanel::top("find_bar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("Find:"));
                    let hint = if find.query.wildcards { tr("text, * any characters, ? one") } else { tr("text") };
                    let field = ui.add(egui::TextEdit::singleline(&mut find.query.text).desired_width(260.0).hint_text(hint));
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
                    if doc.hex && any {
                        ui.label(RichText::new(tr("(found in the Text view)")).weak());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("×").on_hover_text(tr("Close (Esc)")).clicked() {
                            find.open = false;
                        }
                    });
                });
            });
        }
        find.refresh(doc, self.top_line);
        if let Some(forward) = step {
            find.step(forward);
        }
        // a match is shown in the Text view
        if find.reveal && doc.hex && step.is_some() {
            doc.hex = false;
        }
        let top_line = &mut self.top_line;
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(e) = &doc.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return;
            }
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
            let bars = sal_ui::scroll_bar_visibility(ui);
            let marks = Marks::new(ui, &find.hits, find.current);
            if doc.hex {
                egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars).show_rows(ui, row_h, doc.hex_rows(), |ui, range| {
                    for r in range {
                        ui.add(egui::Label::new(RichText::new(doc.hex_line(r)).monospace()).extend());
                    }
                });
            } else if doc.wrap {
                let reveal = find.reveal.then(|| find.current.map(|c| find.hits[c].line)).flatten();
                find.reveal = false;
                egui::ScrollArea::vertical().auto_shrink(false).scroll_bar_visibility(bars).show(ui, |ui| {
                    for (i, l) in doc.lines.iter().enumerate() {
                        let mut job = line_job(ui, l, doc.line_spans(i), &marks.on_line(i));
                        job.wrap.max_width = ui.available_width();
                        let r = ui.add(egui::Label::new(job).wrap());
                        if reveal == Some(i) {
                            r.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                });
            } else {
                let mut area = egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars);
                if let Some(hit) = find.current.filter(|_| find.reveal).map(|c| find.hits[c]) {
                    find.reveal = false;
                    // the match's line in the middle, its text in sight
                    let pitch = row_h + ui.spacing().item_spacing.y;
                    let view = ui.available_size();
                    let y = (hit.line as f32 * pitch - view.y / 2.0).max(0.0);
                    let line = &doc.lines[hit.line];
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let width = |s: &str| ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x);
                    let (x0, x1) = (width(&line[..hit.start]), width(&line[..hit.end]));
                    let x = if x1 > view.x * 0.9 { (x0 - view.x / 3.0).max(0.0) } else { 0.0 };
                    area = area.vertical_scroll_offset(y).horizontal_scroll_offset(x);
                }
                area.show_rows(ui, row_h, doc.lines.len(), |ui, range| {
                    *top_line = range.start;
                    for i in range {
                        let job = line_job(ui, &doc.lines[i], doc.line_spans(i), &marks.on_line(i));
                        ui.add(egui::Label::new(job).extend());
                    }
                });
            }
        });
        true
    }
}

/// Select all text of a text field (so typing replaces what was looked for).
fn select_all(ui: &egui::Ui, field: &egui::Response, text: &str) {
    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field.id) {
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), field.id);
    }
}

/// The matches to mark in the text, and their colors.
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
            this: if dark { Color32::from_rgba_unmultiplied(255, 130, 0, 170) } else { Color32::from_rgba_unmultiplied(255, 150, 0, 200) },
        }
    }

    /// Line `line`'s matches: byte ranges and their background.
    fn on_line(&self, line: usize) -> Vec<(usize, usize, Color32)> {
        find::on_line(self.hits, line).iter().map(|h| (h.start, h.end, if Some(*h) == self.current { self.this } else { self.other })).collect()
    }
}

/// A line of text in the monospace font, colored by `spans` when there are
/// any, with `marks` (byte ranges: found text) on their background.
fn line_job(ui: &egui::Ui, text: &str, spans: Option<&[Span]>, marks: &[(usize, usize, Color32)]) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = TextFormat { font_id: font.clone(), color: ui.visuals().text_color(), ..Default::default() };
    let mut job = LayoutJob::default();
    if text.is_empty() {
        job.append(" ", 0.0, plain);
        return job;
    }
    // the spans' byte ranges
    let mut colored = Vec::new();
    let mut at = 0;
    for s in spans.unwrap_or_default() {
        let end = (at + s.len).min(text.len());
        colored.push((at, end, s));
        at = end;
    }
    // pieces between every edge of a span or a mark
    let mut cuts: Vec<usize> = vec![0, text.len()];
    cuts.extend(colored.iter().map(|c| c.1));
    cuts.extend(marks.iter().flat_map(|m| [m.0, m.1]));
    cuts.retain(|&c| c <= text.len() && text.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut format = plain.clone();
        if let Some((_, _, s)) = colored.iter().find(|c| c.0 <= a && a < c.1) {
            let [r, g, bl] = s.rgb;
            let color = Color32::from_rgb(r, g, bl);
            format.color = color;
            format.italics = s.italic;
            format.underline = if s.underline { egui::Stroke::new(1.0f32, color) } else { egui::Stroke::NONE };
        }
        if let Some(m) = marks.iter().find(|m| m.0 <= a && a < m.1) {
            format.background = m.2;
        }
        job.append(&text[a..b], 0.0, format);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colored line is laid out in its colors; the rest stays plain.
    #[test]
    fn lines_in_their_colors() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let red = Span { len: 3, rgb: [200, 0, 0], italic: true, underline: false };
                let blue = Span { len: 2, rgb: [0, 0, 200], italic: false, underline: false };
                let job = line_job(ui, "abcdeřest", Some(&[red, blue]), &[]);
                let parts: Vec<(&str, egui::Color32, bool)> = job.sections.iter().map(|s| (&job.text[s.byte_range.clone()], s.format.color, s.format.italics)).collect();
                assert_eq!(parts[0], ("abc", egui::Color32::from_rgb(200, 0, 0), true));
                assert_eq!(parts[1], ("de", egui::Color32::from_rgb(0, 0, 200), false));
                assert_eq!((parts[2].0, parts[2].1), ("řest", ui.visuals().text_color()));
                assert_eq!(line_job(ui, "", None, &[]).text, " ", "an empty line keeps its height");
                assert_eq!(line_job(ui, "plain", None, &[]).sections.len(), 1);
            });
        });
    }

    /// Found text is marked on its background, across the colors' edges.
    #[test]
    fn matches_are_marked() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let red = Span { len: 3, rgb: [200, 0, 0], italic: false, underline: false };
                let mark = Color32::YELLOW;
                // "cdeř" is found: it starts inside the red span and ends in plain text
                let job = line_job(ui, "abcdeřest", Some(&[red]), &[(2, 7, mark)]);
                let parts: Vec<(&str, Color32, Color32)> =
                    job.sections.iter().map(|s| (&job.text[s.byte_range.clone()], s.format.color, s.format.background)).collect();
                let plain = ui.visuals().text_color();
                let red = Color32::from_rgb(200, 0, 0);
                assert_eq!(parts, [("ab", red, Color32::TRANSPARENT), ("c", red, mark), ("deř", plain, mark), ("est", plain, Color32::TRANSPARENT)]);
            });
        });
    }

    /// Ctrl+F opens the find bar, the text is found from the first line
    /// shown, Enter and F3 go on round the end, Escape closes the bar before
    /// the window.
    #[test]
    fn find_bar() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a.txt");
        std::fs::write(&file, "Kůň\nnic\nKŮŇ a kůň\n").unwrap();
        let mut w = ViewerWin::new(Doc::open(&file));
        let ctx = egui::Context::default();
        let frame = |w: &mut ViewerWin, keys: &[(egui::Key, egui::Modifiers)], text: &str| {
            let mut events: Vec<egui::Event> =
                keys.iter().map(|&(key, modifiers)| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }).collect();
            if !text.is_empty() {
                events.push(egui::Event::Text(text.into()));
            }
            // the keys held down, as the window reports them
            let modifiers = keys.first().map_or(egui::Modifiers::NONE, |k| k.1);
            let _ = ctx.run(egui::RawInput { events, modifiers, ..Default::default() }, |ctx| {
                w.show(ctx);
            });
        };
        let none = egui::Modifiers::NONE;
        assert!(!w.takes_escape());
        frame(&mut w, &[(egui::Key::F, egui::Modifiers::COMMAND)], "");
        assert!(w.find.open && w.takes_escape(), "Ctrl+F opens it; Escape is its now");
        frame(&mut w, &[], "");
        frame(&mut w, &[], "kůň");
        frame(&mut w, &[], "");
        assert_eq!(w.find.query.text, "kůň");
        assert_eq!((w.find.hits.len(), w.find.current), (3, Some(0)), "in any case");
        assert_eq!(w.find.status(), "1 of 3");
        frame(&mut w, &[(egui::Key::F3, none)], "");
        assert_eq!(w.find.current, Some(1));
        frame(&mut w, &[(egui::Key::F3, egui::Modifiers::SHIFT)], "");
        frame(&mut w, &[(egui::Key::F3, egui::Modifiers::SHIFT)], "");
        assert_eq!(w.find.current, Some(2), "round the start");
        w.find.query.match_case = true;
        frame(&mut w, &[], "");
        assert_eq!(w.find.hits.len(), 1, "matching case: only the last");
        w.find.query.text = "K?Ň".into();
        w.find.query.wildcards = true;
        frame(&mut w, &[], "");
        assert_eq!(w.find.hits, [Hit { line: 2, start: 0, end: 5 }]);
        frame(&mut w, &[(egui::Key::Escape, none)], "");
        assert!(!w.find.open && !w.takes_escape(), "Escape closed the bar, not the window");
        frame(&mut w, &[], "");
        assert!(w.find.hits.is_empty(), "no marks without the bar");
    }
}
