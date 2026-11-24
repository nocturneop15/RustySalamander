//! The viewer's windows: a file as text (colored by its syntax) or hex, with
//! a find bar. The windows themselves, on a thread of their own, are
//! `sal-window`'s.

use std::time::{Duration, Instant};

use sal_core::encoding;
use sal_plugin_api::{tr, trf};
use sal_window::egui::{self, Color32, RichText};
use sal_window::Content;

use sal_plugin_api::settings::Settings;

use crate::doc::{Colors, Doc};
use crate::find::{self, Hit, Query};
use crate::highlight::{self, Span};
use crate::select::{Move, Selection, TextPos};

/// A viewer window's content: a file as text or hex.
pub struct ViewerWin {
    pub doc: Doc,
    find: FindBar,
    /// The first line shown: a new search starts there.
    top_line: usize,
    /// Following the file as it grows (Play), like `tail -f`.
    following: bool,
    /// When the file was last looked at for more.
    polled: Option<Instant>,
    /// Go to the end of the text (next frame).
    to_end: bool,
    /// Selected text (Text view).
    sel: TextSel,
    /// Line numbers beside the text (Text view); on until turned off.
    numbers: bool,
    /// Where the choices of the toolbar are remembered for the next window.
    settings: Settings,
}

/// What a window remembers for the next one: line numbers on, wrap.
const NUMBERS: &str = "line_numbers";
const WRAP: &str = "wrap";

/// The line numbers left of the text: as wide as the largest number, and
/// in place when the text is scrolled sideways.
struct Gutter {
    /// The width of the numbers' column, without the gap after it.
    width: f32,
}

impl Gutter {
    /// The column for `lines` lines (at least three digits wide, so it
    /// doesn't widen at once as a followed file grows).
    fn new(ui: &egui::Ui, lines: usize) -> Gutter {
        let digits = lines.max(1).to_string().len().max(3);
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let width = ui.fonts(|f| f.layout_no_wrap("0".repeat(digits), font, Color32::WHITE).size().x) + ui.spacing().item_spacing.x;
        Gutter { width }
    }

    /// Draw a line moved right past the column, as high as its text (a row
    /// is otherwise at least as high as a button: the lines would no longer
    /// be where the scrolling expects them).
    fn line(&self, ui: &mut egui::Ui, line: usize, label: egui::Label) -> Drawn {
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 0.0;
            ui.horizontal(|ui| {
                ui.add_space(self.width);
                draw_line(ui, line, label)
            })
            .inner
        })
        .inner
    }

    /// The column over the left edge of what is in sight, with the number
    /// of each line drawn (at its first row), over any text scrolled under it;
    /// in the colors of the panels' footer, standing out from the text.
    fn paint(&self, ui: &egui::Ui, drawn: &[Drawn]) {
        let clip = ui.clip_rect();
        let column = egui::Rect::from_min_max(clip.min, egui::pos2(clip.min.x + self.width, clip.max.y));
        let painter = ui.painter();
        painter.rect_filled(column, 0.0, sal_ui::layout::strip_color(ui.visuals()));
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let color = ui.visuals().strong_text_color();
        let right = column.max.x - ui.spacing().item_spacing.x / 2.0;
        for d in drawn {
            painter.text(egui::pos2(right, d.pos.y), egui::Align2::RIGHT_TOP, (d.line + 1).to_string(), font.clone(), color);
        }
    }
}

/// Selecting text with the mouse and Shift+keys, copying it with Ctrl+C.
#[derive(Default)]
struct TextSel {
    sel: Selection,
    /// The caret was put somewhere (a click, a key); before, Shift+keys
    /// start at the first line shown.
    placed: bool,
    /// Bring the caret into view (it moved by a key).
    reveal: bool,
    /// The lines shown last, and the view's height.
    shown: std::ops::Range<usize>,
    view_h: f32,
}

/// A line drawn in the Text view: its line, where it is, and its layout.
struct Drawn {
    line: usize,
    rect: egui::Rect,
    pos: egui::Pos2,
    galley: std::sync::Arc<egui::Galley>,
}

/// How often a followed file is looked at for more.
const POLL: Duration = Duration::from_millis(250);
/// A scroll offset past the end of any text (the scroll area stops at its end).
const TO_END: f32 = 1e10;

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
    /// The worker searching beside the window (one per window: the latest
    /// query only), started at the first search.
    searcher: Option<Searcher>,
    /// What it was last asked for, until it is done.
    pending: Option<Pending>,
}

/// The search the worker does for the find bar: what to look for in what.
type Searcher = sal_ui::latest::Latest<(std::sync::Arc<Vec<String>>, Query), (Vec<Hit>, bool)>;

/// The search asked for and not done yet: for `key`, to start from line
/// `from` when it is done.
struct Pending {
    key: (Query, u64),
    from: usize,
}

impl FindBar {
    /// Find again when the text, the options or the document changed, beside
    /// the window (a big file takes a while; the window stays usable): what
    /// was found shows when it is done. A new search starts from the current
    /// match, or the first line shown. `wake` draws the window again then.
    fn refresh(&mut self, doc: &Doc, top_line: usize, wake: impl Fn() + Send + 'static) {
        if !self.open {
            self.cancel();
            self.hits.clear();
            self.made_for = None;
            self.current = None;
            return;
        }
        let key = (self.query.clone(), doc.version());
        // back to what the matches shown are for (typed on, then back,
        // before the other search was done): those stay, and the other search
        // is given up (else it would stay "Searching…"); drawn again, as this
        // frame may have said so already and a search given up never wakes
        if self.made_for.as_ref() == Some(&key) {
            if self.pending.is_some() {
                self.cancel();
                wake();
            }
            return;
        }
        // other text: the matches found in the text before don't fit it (a
        // line may be gone, or shorter); only for other words in the same
        // text do they stay shown until the search is done
        if self.made_for.as_ref().is_some_and(|m| m.1 != key.1) || self.pending.as_ref().is_some_and(|p| p.key.1 != key.1) {
            self.forget();
        }
        if let Some(p) = self.pending.as_ref().filter(|p| p.key == key) {
            let Some((hits, more)) = self.searcher.as_ref().and_then(Searcher::take) else { return };
            let from = p.from;
            self.pending = None;
            (self.hits, self.more) = (hits, more);
            self.current = find::first_from(&self.hits, from);
            self.reveal = self.current.is_some();
            self.made_for = Some(key);
            return;
        }
        // a new search: it replaces one asked for before
        let from = self.current.and_then(|c| self.hits.get(c)).map_or(top_line, |h| h.line);
        if self.query.text.is_empty() {
            self.cancel();
            (self.hits, self.more, self.current) = (Vec::new(), false, None);
            self.made_for = Some(key);
            return;
        }
        let searcher = self.searcher.get_or_insert_with(|| Searcher::new("viewer find", |(lines, query), stop| find::find_until(&lines, &query, stop), wake));
        searcher.ask((doc.lines.clone(), self.query.clone()));
        self.pending = Some(Pending { key, from });
    }

    /// Forget the matches found (the text changed): none marked, none to go to.
    fn forget(&mut self) {
        (self.hits, self.more, self.current, self.reveal, self.made_for) = (Vec::new(), false, None, false, None);
    }

    /// Give up the search asked for, if one is.
    fn cancel(&mut self) {
        if let Some(s) = &mut self.searcher {
            s.cancel();
        }
        self.pending = None;
    }

    /// Whether a search is still running.
    fn searching(&self) -> bool {
        self.pending.is_some()
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

    /// "3 of 12", "Not found", "Searching…", or nothing yet.
    fn status(&self) -> String {
        if self.query.text.is_empty() {
            return String::new();
        }
        // not "Not found" before it is done
        if self.searching() {
            return tr("Searching…");
        }
        match self.current {
            None => tr("Not found"),
            Some(c) if self.more => trf("{n} of more than {total}", &[("n", &(c + 1)), ("total", &encoding::format_thousands(self.hits.len() as u64))]),
            Some(c) => trf("{n} of {total}", &[("n", &(c + 1)), ("total", &encoding::format_thousands(self.hits.len() as u64))]),
        }
    }
}

impl ViewerWin {
    /// A window with the choices of a new viewer (none remembered).
    #[cfg(test)]
    pub fn new(doc: Doc) -> ViewerWin {
        ViewerWin::with_settings(doc, Settings::in_memory())
    }

    /// A window as the last one was left: line numbers, and wrap if the
    /// file is small enough for it.
    pub fn with_settings(mut doc: Doc, settings: Settings) -> ViewerWin {
        doc.wrap = settings.bool(WRAP, false) && doc.can_wrap();
        let numbers = settings.bool(NUMBERS, true);
        ViewerWin { doc, find: FindBar::default(), top_line: 0, following: false, polled: None, to_end: false, sel: TextSel::default(), numbers, settings }
    }

    /// Play: follow the file from its end; Pause: stop.
    fn set_following(&mut self, on: bool) {
        self.following = on;
        if on {
            self.doc.follow();
            self.polled = Some(Instant::now());
            self.to_end = true;
        }
    }

    /// While following: what was added to the file since, now and then.
    fn poll(&mut self, ctx: &egui::Context) {
        if !self.following {
            return;
        }
        if self.polled.is_none_or(|t| t.elapsed() >= POLL) {
            self.doc.poll();
            self.polled = Some(Instant::now());
            if !self.doc.can_wrap() {
                self.doc.wrap = false;
            }
        }
        ctx.request_repaint_after(POLL);
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
        if ctx.input(|i| i.key_pressed(egui::Key::T) && i.modifiers.command) {
            self.set_following(!self.following);
        }
        self.poll(ctx);
        // as the user leaves them, remembered at the end of the frame (not
        // wrap turned off because the file grew too large for it)
        let chosen = (self.numbers, self.doc.wrap);
        let mut play = None;
        let following = self.following;
        let to_end = std::mem::take(&mut self.to_end);
        if ctx.input(|i| i.key_pressed(egui::Key::L) && i.modifiers.command) {
            self.numbers = !self.numbers;
        }
        let doc = &mut self.doc;
        let find = &mut self.find;
        let numbers = &mut self.numbers;
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
        egui::TopBottomPanel::top("viewer_bar").frame(sal_ui::layout::bar_frame(&ctx.style())).show(ctx, |ui| {
            sal_ui::layout::controls(ui);
            ui.horizontal(|ui| {
                let (icon, tip) = if following {
                    (sal_ui::mdi::PAUSE, tr("Pause: stop showing what is added to the file (Ctrl+T)"))
                } else {
                    (sal_ui::mdi::PLAY, tr("Play: show what is added to the file as it is written, like a log (Ctrl+T)"))
                };
                if ui.selectable_label(following, icon).on_hover_text(tip).clicked() {
                    play = Some(!following);
                }
                ui.separator();
                ui.selectable_value(&mut doc.hex, false, tr("Text"));
                ui.selectable_value(&mut doc.hex, true, tr("Hex")).on_hover_text(tr("F4 toggles"));
                ui.separator();
                ui.add_enabled_ui(!doc.hex && doc.can_wrap(), |ui| {
                    ui.checkbox(&mut doc.wrap, tr("Wrap")).on_hover_text("Ctrl+W");
                });
                ui.add_enabled_ui(!doc.hex, |ui| {
                    ui.checkbox(numbers, tr("Line numbers")).on_hover_text("Ctrl+L");
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
                if doc.start() > 0 {
                    info.push_str(&trf(" (showing last {size})", &[("size", &encoding::format_size(doc.len() as u64))]));
                } else if doc.file_len > doc.len() as u64 {
                    info.push_str(&trf(" (showing first {size})", &[("size", &encoding::format_size(doc.len() as u64))]));
                }
                ui.label(info);
            });
        });
        if find.open {
            egui::TopBottomPanel::top("find_bar").frame(sal_ui::layout::bar_frame(&ctx.style())).show(ctx, |ui| {
                sal_ui::layout::controls(ui);
                ui.horizontal(|ui| {
                    ui.label(tr("Find:"));
                    let hint = if find.query.wildcards { tr("text, * any characters, ? one") } else { tr("text") };
                    let field = ui.add(egui::TextEdit::singleline(&mut find.query.text).char_limit(sal_ui::layout::limit::FIND).desired_width(260.0).margin(sal_ui::layout::FIELD_MARGIN).hint_text(hint));
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
                    if find.current.is_none() && !find.query.text.is_empty() && !find.searching() {
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
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint_of(egui::ViewportId::ROOT)
        };
        find.refresh(doc, self.top_line, wake);
        if let Some(forward) = step {
            find.step(forward);
        }
        // a match is shown in the Text view
        if find.reveal && doc.hex && step.is_some() {
            doc.hex = false;
        }
        let keys = Keys::read(ctx);
        let top_line = &mut self.top_line;
        let ts = &mut self.sel;
        ts.sel.clamp(&doc.lines);
        if !doc.hex && doc.error.is_none() {
            ts.keys(ctx, &doc.lines, *top_line);
        }
        egui::CentralPanel::default().frame(sal_ui::layout::content_frame(&ctx.style())).show(ctx, |ui| {
            if let Some(e) = &doc.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return;
            }
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
            let bars = sal_ui::scroll_bar_visibility(ui);
            let marks = Marks::new(ui, &find.hits, find.current);
            let pitch = row_h + ui.spacing().item_spacing.y;
            let page = (ui.available_height() - pitch).max(pitch);
            ts.view_h = ui.available_height();
            let sel_color = ui.visuals().selection.bg_fill;
            let gutter = numbers.then(|| Gutter::new(ui, doc.lines.len()));
            let draw = |ui: &mut egui::Ui, line: usize, label: egui::Label| match &gutter {
                Some(g) => g.line(ui, line, label),
                None => draw_line(ui, line, label),
            };
            let scroll = |area: egui::ScrollArea| keys.jump(area, following && !keys.leaves_end());
            if doc.hex {
                let mut area = egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars).stick_to_bottom(following);
                if to_end {
                    area = area.vertical_scroll_offset(TO_END);
                }
                area = scroll(area);
                area.show_rows(ui, row_h, doc.hex_rows(), |ui, range| {
                    keys.scroll(ui, pitch, page);
                    for r in range {
                        ui.add(egui::Label::new(RichText::new(doc.hex_line(r)).monospace()).extend());
                    }
                });
            } else if doc.wrap {
                let reveal = find.reveal.then(|| find.current.and_then(|c| find.hits.get(c)).map(|h| h.line)).flatten();
                find.reveal = false;
                let mut area = egui::ScrollArea::vertical().auto_shrink(false).scroll_bar_visibility(bars).stick_to_bottom(following);
                if to_end {
                    area = area.vertical_scroll_offset(TO_END);
                }
                area = scroll(area);
                area = area.drag_to_scroll(false);
                area.show(ui, |ui| {
                    keys.scroll(ui, pitch, page);
                    let clip = ui.clip_rect();
                    let mut drawn = Vec::new();
                    for (i, l) in doc.lines.iter().enumerate() {
                        let (m, eol) = ts.marks(i, l.len(), sel_color, &marks);
                        let mut job = line_job(ui, l, doc.line_spans(i), &m, eol);
                        job.wrap.max_width = ui.available_width();
                        let d = draw(ui, i, egui::Label::new(job).wrap());
                        if reveal == Some(i) {
                            ui.scroll_to_rect(d.rect, Some(egui::Align::Center));
                        }
                        // only what is in sight takes the mouse
                        if d.rect.max.y >= clip.min.y && d.rect.min.y <= clip.max.y {
                            ts.reveal_in(ui, &d, &doc.lines);
                            drawn.push(d);
                        }
                    }
                    if let Some(first) = drawn.first() {
                        *top_line = first.line;
                        ts.shown = first.line..drawn.last().map_or(first.line, |d| d.line) + 1;
                    }
                    if let Some(g) = &gutter {
                        g.paint(ui, &drawn);
                    }
                    ts.mouse(ui, &drawn, &doc.lines, pitch);
                });
            } else {
                let mut area = egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars).stick_to_bottom(following);
                if to_end {
                    area = area.vertical_scroll_offset(TO_END);
                }
                let fits = |h: &Hit| doc.lines.get(h.line).is_some_and(|l| h.end <= l.len() && l.is_char_boundary(h.start) && l.is_char_boundary(h.end));
                if let Some(hit) = find.current.filter(|_| find.reveal).and_then(|c| find.hits.get(c).copied()).filter(fits) {
                    find.reveal = false;
                    // the match's line in the middle, its text in sight (beside
                    // the line numbers)
                    let view = ui.available_size() - egui::vec2(gutter.as_ref().map_or(0.0, |g| g.width + ui.spacing().item_spacing.x), 0.0);
                    let y = (hit.line as f32 * pitch - view.y / 2.0).max(0.0);
                    let line = &doc.lines[hit.line];
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let width = |s: &str| ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x);
                    let (x0, x1) = (width(&line[..hit.start]), width(&line[..hit.end]));
                    let x = if x1 > view.x * 0.9 { (x0 - view.x / 3.0).max(0.0) } else { 0.0 };
                    area = area.vertical_scroll_offset(y).horizontal_scroll_offset(x);
                }
                area = scroll(area);
                // the caret's line far off: there first, into view when drawn
                if ts.reveal && !ts.shown.contains(&ts.sel.caret.line) {
                    let line = ts.sel.caret.line as f32;
                    let y = if ts.sel.caret.line < ts.shown.start { line * pitch } else { (line + 1.0) * pitch - ts.view_h + pitch };
                    area = area.vertical_scroll_offset(y.max(0.0));
                }
                area = area.drag_to_scroll(false);
                area.show_rows(ui, row_h, doc.lines.len(), |ui, range| {
                    keys.scroll(ui, pitch, page);
                    *top_line = range.start;
                    ts.shown = range.clone();
                    let mut drawn = Vec::with_capacity(range.len());
                    for i in range {
                        let l = &doc.lines[i];
                        let (m, eol) = ts.marks(i, l.len(), sel_color, &marks);
                        let job = line_job(ui, l, doc.line_spans(i), &m, eol);
                        let d = draw(ui, i, egui::Label::new(job).extend());
                        ts.reveal_in(ui, &d, &doc.lines);
                        drawn.push(d);
                    }
                    if let Some(g) = &gutter {
                        g.paint(ui, &drawn);
                    }
                    ts.mouse(ui, &drawn, &doc.lines, pitch);
                });
            }
        });
        if let Some(on) = play {
            self.set_following(on);
            ctx.request_repaint();
        }
        if self.numbers != chosen.0 {
            let _ = self.settings.set_bool(NUMBERS, self.numbers);
        }
        if self.doc.wrap != chosen.1 {
            let _ = self.settings.set_bool(WRAP, self.doc.wrap);
        }
        true
    }
}

impl TextSel {
    /// Ctrl+A selects all, Ctrl+C (Ctrl+Insert) copies, Shift with the
    /// arrows, Home/End and Page Up/Down selects (with Ctrl by words, to the
    /// start / the end). Not while a text field has the keyboard.
    fn keys(&mut self, ctx: &egui::Context, lines: &[String], top_line: usize) {
        if ctx.wants_keyboard_input() {
            return;
        }
        // the style first: the fonts lock the context, as the style does
        let style = ctx.style();
        let font = egui::TextStyle::Monospace.resolve(&style);
        let pitch = style.spacing.item_spacing.y + ctx.fonts(|f| f.row_height(&font));
        let page = ((self.view_h / pitch) as usize).saturating_sub(1).max(1);
        let (moves, all, copy) = ctx.input(|i| {
            let (mut moves, mut all, mut copy) = (Vec::new(), false, false);
            for ev in &i.events {
                match ev {
                    egui::Event::Copy => copy = true,
                    &egui::Event::Key { key, pressed: true, modifiers: m, .. } if !m.alt => {
                        use egui::Key::*;
                        if m.command && !m.shift && key == A {
                            all = true;
                        } else if m.command && matches!(key, C | Insert) {
                            copy = true;
                        } else if m.shift {
                            let to = match (key, m.command) {
                                (ArrowLeft, false) => Move::Left,
                                (ArrowRight, false) => Move::Right,
                                (ArrowLeft, true) => Move::WordLeft,
                                (ArrowRight, true) => Move::WordRight,
                                (ArrowUp, _) => Move::Up,
                                (ArrowDown, _) => Move::Down,
                                (PageUp, _) => Move::PageUp(page),
                                (PageDown, _) => Move::PageDown(page),
                                (Home, false) => Move::LineStart,
                                (End, false) => Move::LineEnd,
                                (Home, true) => Move::DocStart,
                                (End, true) => Move::DocEnd,
                                _ => continue,
                            };
                            moves.push(to);
                        }
                    }
                    _ => {}
                }
            }
            (moves, all, copy)
        });
        if all {
            self.sel = Selection::all(lines);
            self.placed = true;
        }
        if !moves.is_empty() && !self.placed {
            self.sel = Selection::at(TextPos::new(top_line, 0));
            self.placed = true;
        }
        for to in &moves {
            self.sel.go(lines, *to, true);
            self.reveal = true;
        }
        if copy && !self.sel.is_empty() {
            ctx.copy_text(self.sel.text(lines, "\r\n"));
        }
    }

    /// Line `line`'s marks: the selection first, then the found text; and
    /// the color of its line break when that is selected too.
    fn marks(&self, line: usize, len: usize, color: Color32, found: &Marks) -> (Vec<(usize, usize, Color32)>, Option<Color32>) {
        let mut m = Vec::new();
        let mut eol = None;
        if let Some((a, b, on)) = self.sel.on_line(line, len) {
            if a < b {
                m.push((a, b, color));
            }
            eol = on.then_some(color);
        }
        m.extend(found.on_line(line));
        (m, eol)
    }

    /// When the caret moved by a key and its line is drawn: the caret into view.
    fn reveal_in(&mut self, ui: &egui::Ui, d: &Drawn, lines: &[String]) {
        if !self.reveal || d.line != self.sel.caret.line {
            return;
        }
        self.reveal = false;
        let n = lines[d.line][..self.sel.caret.col].chars().count();
        let at = d.galley.pos_from_ccursor(egui::text::CCursor::new(n)).translate(d.pos.to_vec2());
        // a little room beside it
        let room = egui::vec2(ui.fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), '0')) * 4.0, 0.0);
        ui.scroll_to_rect(at.expand2(room), None);
    }

    /// A press puts the caret (Shift+click extends), dragging selects, a
    /// double-click takes a word; dragging past the edge scrolls.
    fn mouse(&mut self, ui: &mut egui::Ui, drawn: &[Drawn], lines: &[String], pitch: f32) {
        let clip = ui.clip_rect();
        let resp = ui.interact(clip, ui.id().with("select"), egui::Sense::click_and_drag());
        if resp.hovered() || resp.is_pointer_button_down_on() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        // a double-click comes with the release: the pointer is where it is
        if resp.double_clicked() {
            if let Some(at) = resp.interact_pointer_pos().or(resp.hover_pos()).and_then(|p| pos_at(drawn, lines, p)) {
                self.sel = Selection::word_at(lines, at);
                self.placed = true;
            }
            return;
        }
        let Some(p) = resp.interact_pointer_pos() else { return };
        let Some(at) = pos_at(drawn, lines, p) else { return };
        let (pressed, shift) = ui.input(|i| (i.pointer.primary_pressed(), i.modifiers.shift));
        if resp.is_pointer_button_down_on() {
            self.sel.put(at, !pressed || shift);
            self.placed = true;
            // past the edge: the text comes along
            let past = |v: f32, lo: f32, hi: f32| if v < lo { lo - v } else if v > hi { hi - v } else { 0.0 };
            let delta = egui::vec2(past(p.x, clip.min.x, clip.max.x), past(p.y, clip.min.y, clip.max.y)).clamp(egui::Vec2::splat(-3.0 * pitch), egui::Vec2::splat(3.0 * pitch));
            if delta != egui::Vec2::ZERO {
                ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
                ui.ctx().request_repaint();
            }
        }
    }
}

/// Draw a line of the Text view (without egui's own selecting).
fn draw_line(ui: &mut egui::Ui, line: usize, label: egui::Label) -> Drawn {
    let (pos, galley, resp) = label.selectable(false).layout_in_ui(ui);
    if ui.is_rect_visible(resp.rect) {
        ui.painter().galley(pos, galley.clone(), ui.visuals().text_color());
    }
    Drawn { line, rect: resp.rect, pos, galley }
}

/// The place in the text at the screen point `p`: in the line drawn there,
/// or the nearest one.
fn pos_at(drawn: &[Drawn], lines: &[String], p: egui::Pos2) -> Option<TextPos> {
    let gap = |i: usize| drawn.get(i + 1).map_or(0.0, |n| (n.rect.min.y - drawn[i].rect.max.y) / 2.0);
    let i = (0..drawn.len()).find(|&i| p.y < drawn[i].rect.max.y + gap(i)).unwrap_or(drawn.len().checked_sub(1)?);
    let d = &drawn[i];
    let n = d.galley.cursor_from_pos(p - d.pos).ccursor.index;
    let l = lines.get(d.line)?;
    Some(TextPos::new(d.line, l.char_indices().nth(n).map_or(l.len(), |(b, _)| b)))
}

/// Scrolling by the keyboard: arrows a line (or a few characters), Page
/// Up/Down a screen, Ctrl+Home/End (or Home/End) to the start / the end.
/// Not while a text field (the find bar) has the keyboard, nor with Shift
/// (which selects).
#[derive(Default, Clone, Copy)]
struct Keys {
    /// Lines down (negative: up).
    lines: f32,
    /// Screens down (negative: up).
    pages: f32,
    /// Columns right (negative: left).
    cols: f32,
    home: bool,
    end: bool,
}

impl Keys {
    fn read(ctx: &egui::Context) -> Keys {
        if ctx.wants_keyboard_input() {
            return Keys::default();
        }
        ctx.input(|i| {
            let n = |k: egui::Key| i.num_presses(k) as f32;
            if i.modifiers.alt || i.modifiers.shift {
                return Keys::default();
            }
            use egui::Key::*;
            Keys {
                lines: n(ArrowDown) - n(ArrowUp),
                pages: n(PageDown) - n(PageUp),
                cols: n(ArrowRight) - n(ArrowLeft),
                home: i.key_pressed(Home),
                end: i.key_pressed(End),
            }
        })
    }

    /// Going up (or to the start) leaves the end a followed file sticks to.
    fn leaves_end(&self) -> bool {
        self.lines < 0.0 || self.pages < 0.0 || self.home
    }

    /// Home / End: straight to the start or the end; sticking to the end
    /// only while `stick`.
    fn jump(&self, area: egui::ScrollArea, stick: bool) -> egui::ScrollArea {
        let area = area.stick_to_bottom(stick);
        if self.home {
            area.vertical_scroll_offset(0.0).horizontal_scroll_offset(0.0)
        } else if self.end {
            area.vertical_scroll_offset(TO_END)
        } else {
            area
        }
    }

    /// Lines, pages and columns: scroll the area `ui` is in.
    fn scroll(&self, ui: &egui::Ui, pitch: f32, page: f32) {
        let col = ui.fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), '0'));
        let delta = egui::vec2(self.cols * col * 4.0, self.lines * pitch + self.pages * page);
        if delta != egui::Vec2::ZERO {
            // a positive delta moves the content down: the view up
            ui.scroll_with_delta_animation(-delta, egui::style::ScrollAnimation::none());
        }
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
            this: Color32::from_rgb(255, 150, 0),
        }
    }

    /// Line `line`'s matches: byte ranges and their background.
    fn on_line(&self, line: usize) -> Vec<(usize, usize, Color32)> {
        find::on_line(self.hits, line).iter().map(|h| (h.start, h.end, if Some(*h) == self.current { self.this } else { self.other })).collect()
    }
}

/// A line of text in the monospace font, colored by `spans` when there are
/// any, with `marks` (byte ranges: selected or found text, the first that
/// covers a piece counts) on their background; `eol`: the line break is
/// selected, a space in that color after the text.
fn line_job(ui: &egui::Ui, text: &str, spans: Option<&[Span]>, marks: &[(usize, usize, Color32)], eol: Option<Color32>) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = TextFormat { font_id: font.clone(), color: ui.visuals().text_color(), ..Default::default() };
    let mut job = LayoutJob::default();
    if text.is_empty() {
        job.append(" ", 0.0, TextFormat { background: eol.unwrap_or_default(), ..plain });
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
            // the current match is solid: its text in a color readable on it
            if m.2.is_opaque() {
                format.color = sal_ui::readable_on(m.2);
            }
        }
        job.append(&text[a..b], 0.0, format);
    }
    if let Some(color) = eol {
        job.append(" ", 0.0, TextFormat { background: color, ..plain });
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texts drawn in a frame, with where each starts.
    fn drawn_texts(out: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
        fn walk(s: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match s {
                egui::Shape::Text(t) => out.push((t.galley.text().to_owned(), t.pos)),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        out.shapes.iter().for_each(|c| walk(&c.shape, &mut v));
        v
    }

    /// Line numbers are shown left of the text until Ctrl+L (or the box)
    /// turns them off; the text then starts at the edge.
    #[test]
    fn line_numbers() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a.txt");
        std::fs::write(&file, "alfa\nbeta\ngama\n").unwrap();
        let mut w = ViewerWin::new(Doc::open(&file));
        let ctx = egui::Context::default();
        let frame = |w: &mut ViewerWin, events: Vec<egui::Event>| {
            let modifiers = events.iter().find_map(|e| if let egui::Event::Key { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0));
            let mut out = None;
            for e in [events, Vec::new()] {
                out = Some(ctx.run(egui::RawInput { screen_rect: Some(rect), events: e, modifiers, ..Default::default() }, |ctx| {
                    w.show(ctx);
                }));
            }
            drawn_texts(&out.unwrap())
        };
        let at = |texts: &[(String, egui::Pos2)], s: &str| texts.iter().find(|(t, _)| t == s).map(|(_, p)| *p);
        let on = frame(&mut w, vec![]);
        assert!(w.numbers, "on at first");
        let (three, gama) = (at(&on, "3").expect("the third line's number"), at(&on, "gama").expect("the third line"));
        assert!(three.x < gama.x && (three.y - gama.y).abs() < 1.0, "beside its line: {three:?} {gama:?}");
        assert!(at(&on, "4").is_none(), "no number past the last line");
        let ctrl_l = egui::Event::Key { key: egui::Key::L, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        let off = frame(&mut w, vec![ctrl_l]);
        assert!(!w.numbers);
        assert!(at(&off, "3").is_none(), "{off:?}");
        assert!(at(&off, "gama").unwrap().x < gama.x, "the text moves to the edge");
        assert!(!w.settings.bool(NUMBERS, true), "remembered for the next window");
        let next = ViewerWin::with_settings(Doc::open(&file), w.settings.clone());
        assert!(!next.numbers && !next.doc.wrap, "which opens so");
        // the lines are as far apart either way (the scrolling counts on it)
        let apart = |texts: &[(String, egui::Pos2)]| at(texts, "gama").unwrap().y - at(texts, "beta").unwrap().y;
        assert_eq!(apart(&on), apart(&off));
    }

    /// A colored line is laid out in its colors; the rest stays plain.
    #[test]
    fn lines_in_their_colors() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let red = Span { len: 3, rgb: [200, 0, 0], italic: true, underline: false };
                let blue = Span { len: 2, rgb: [0, 0, 200], italic: false, underline: false };
                let job = line_job(ui, "abcdeřest", Some(&[red, blue]), &[], None);
                let parts: Vec<(&str, egui::Color32, bool)> = job.sections.iter().map(|s| (&job.text[s.byte_range.clone()], s.format.color, s.format.italics)).collect();
                assert_eq!(parts[0], ("abc", egui::Color32::from_rgb(200, 0, 0), true));
                assert_eq!(parts[1], ("de", egui::Color32::from_rgb(0, 0, 200), false));
                assert_eq!((parts[2].0, parts[2].1), ("řest", ui.visuals().text_color()));
                assert_eq!(line_job(ui, "", None, &[], None).text, " ", "an empty line keeps its height");
                assert_eq!(line_job(ui, "plain", None, &[], None).sections.len(), 1);
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
                let mark = Color32::from_rgba_unmultiplied(255, 220, 0, 150);
                // "cdeř" is found: it starts inside the red span and ends in plain text
                let job = line_job(ui, "abcdeřest", Some(&[red]), &[(2, 7, mark)], None);
                let parts: Vec<(&str, Color32, Color32)> =
                    job.sections.iter().map(|s| (&job.text[s.byte_range.clone()], s.format.color, s.format.background)).collect();
                let plain = ui.visuals().text_color();
                let red = Color32::from_rgb(200, 0, 0);
                assert_eq!(parts, [("ab", red, Color32::TRANSPARENT), ("c", red, mark), ("deř", plain, mark), ("est", plain, Color32::TRANSPARENT)]);
            });
        });
    }

    /// Render a viewer window with the find bar to PNGs (dark and light) in
    /// SNAPSHOT_DIR or the temp folder, to look at its layout.
    #[test]
    #[ignore]
    fn snapshot_viewer() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("notes.json");
        let text: String = (0..60).map(|i| format!("{{\"line\": {i}, \"name\": \"Kůň {i}\", \"ok\": true}}\n")).collect();
        std::fs::write(&file, text).unwrap();
        let out_dir = std::env::var_os("SNAPSHOT_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        for (dark, name) in [(true, "viewer_dark.png"), (false, "viewer_light.png")] {
            let mut w = ViewerWin::new(Doc::open(&file));
            w.find.open = true;
            w.find.query.text = "kůň".into();
            // a selection from the middle of a line over two line breaks
            w.sel.sel = Selection::at(TextPos::new(1, 5));
            w.sel.sel.put(TextPos::new(3, 12), true);
            let ctx = egui::Context::default();
            ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
            sal_ui::fonts::install(&ctx, "", "");
            let mut snap = crate::snapshot::Snapshot::default();
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 360.0))),
                ..Default::default()
            };
            for _ in 0..3 {
                let out = ctx.run(input(), |ctx| {
                    w.show(ctx);
                });
                snap.apply(&out);
            }
            let out = ctx.run(input(), |ctx| {
                w.show(ctx);
            });
            snap.apply(&out);
            let bg = ctx.style().visuals.panel_fill;
            snap.render(&ctx, out, bg, &out_dir.join(name));
        }
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
        let once = |w: &mut ViewerWin, keys: &[(egui::Key, egui::Modifiers)], text: &str| {
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
        // a frame, then as many as the search beside the window takes
        let frame = |w: &mut ViewerWin, keys: &[(egui::Key, egui::Modifiers)], text: &str| {
            once(w, keys, text);
            let until = Instant::now() + Duration::from_secs(10);
            while w.find.searching() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(2));
                once(w, &[], "");
            }
        };
        let none = egui::Modifiers::NONE;
        assert!(!w.takes_escape());
        frame(&mut w, &[(egui::Key::F, egui::Modifiers::COMMAND)], "");
        assert!(w.find.open && w.takes_escape(), "Ctrl+F opens it; Escape is its now");
        frame(&mut w, &[], "");
        once(&mut w, &[], "kůň");
        // the search runs beside the window: not "Not found" meanwhile
        assert!(w.find.searching());
        assert_eq!(w.find.status(), "Searching…");
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

    /// The file gets shorter while Find is open: the matches found in the
    /// longer text are forgotten at once, and F3 before the new search is
    /// done goes nowhere (no line that isn't there any more).
    #[test]
    fn find_after_the_text_changed() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a.txt");
        let long: String = (0..200).map(|i| format!("line {i} kůň\n")).collect();
        std::fs::write(&file, &long).unwrap();
        let mut w = ViewerWin::new(Doc::open(&file));
        let ctx = egui::Context::default();
        let once = |w: &mut ViewerWin, events: Vec<egui::Event>| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 400.0));
            let _ = ctx.run(egui::RawInput { screen_rect: Some(rect), events, ..Default::default() }, |ctx| {
                w.show(ctx);
            });
        };
        let f3 = || egui::Event::Key { key: egui::Key::F3, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        w.find.open = true;
        w.find.query.text = "kůň".into();
        once(&mut w, vec![]);
        let until = Instant::now() + Duration::from_secs(10);
        while w.find.searching() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
            once(&mut w, vec![]);
        }
        assert_eq!(w.find.hits.len(), 200);
        // to the last match, far down
        for _ in 0..199 {
            w.find.step(true);
        }
        assert_eq!(w.find.current.map(|c| w.find.hits[c].line), Some(199));
        // the file is read again (as when following it), much shorter, and in other words
        std::fs::write(&file, "a\nkůň b\n").unwrap();
        assert!(w.doc.poll());
        once(&mut w, vec![f3()]);
        assert!(w.find.hits.iter().all(|h| h.line < 2), "nothing of the longer text: {:?}", w.find.hits.len());
        once(&mut w, vec![f3(), f3()]);
        let until = Instant::now() + Duration::from_secs(10);
        while w.find.searching() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
            once(&mut w, vec![f3()]);
        }
        assert_eq!(w.find.hits, [Hit { line: 1, start: 0, end: 5 }]);
    }

    /// Typed on and back again before the other search was done: the
    /// matches shown stay (and the one gone to), and nothing is left
    /// "Searching…" (whatever the worker's speed: the other search's result
    /// is never looked at).
    #[test]
    fn back_to_the_words_shown() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a.txt");
        std::fs::write(&file, "kůň a\nb kůň\nkůň\n").unwrap();
        let doc = Doc::open(&file);
        let mut bar = FindBar { open: true, ..Default::default() };
        let settle = |bar: &mut FindBar| {
            let until = Instant::now() + Duration::from_secs(10);
            while bar.searching() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(2));
                bar.refresh(&doc, 0, || {});
            }
        };
        bar.query.text = "kůň".into();
        bar.refresh(&doc, 0, || {});
        settle(&mut bar);
        bar.step(true);
        let (hits, current) = (bar.hits.clone(), bar.current);
        assert_eq!((hits.len(), current), (3, Some(1)));
        // B asked for, then back to A before B is looked at again
        bar.query.text = "b".into();
        bar.refresh(&doc, 0, || {});
        assert!(bar.searching());
        bar.query.text = "kůň".into();
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = woken.clone();
        bar.refresh(&doc, 0, move || w.store(true, std::sync::atomic::Ordering::Relaxed));
        assert!(bar.pending.is_none() && !bar.searching(), "not left searching");
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed), "drawn again");
        assert_eq!((bar.hits.clone(), bar.current), (hits, current), "A's matches, A's place");
        assert_eq!(bar.status(), "2 of 3");
        // and B's result, when it comes, never takes over
        std::thread::sleep(Duration::from_millis(50));
        bar.refresh(&doc, 0, || {});
        assert_eq!((bar.hits.len(), bar.current, bar.status().as_str()), (3, Some(1), "2 of 3"));
    }

    /// The arrows, Page Up/Down and Home/End scroll the text and the bytes;
    /// not while the find bar has the keyboard.
    #[test]
    fn keyboard_scrolls() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a.txt");
        let text: String = (0..500).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&file, text).unwrap();
        let mut w = ViewerWin::new(Doc::open(&file));
        let ctx = egui::Context::default();
        let clock = std::cell::Cell::new(0.0);
        let frame = |w: &mut ViewerWin, key: Option<egui::Key>| {
            let events = key.map(|key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
                events: events.into_iter().collect(),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                w.show(ctx);
            });
            // scrolling is animated: let the time go by
            for _ in 0..3 {
                clock.set(clock.get() + 1.0);
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
                    time: Some(clock.get()),
                    ..Default::default()
                };
                let _ = ctx.run(input, |ctx| {
                    w.show(ctx);
                });
            }
            w.top_line
        };
        assert_eq!(frame(&mut w, None), 0);
        assert_eq!(frame(&mut w, Some(egui::Key::ArrowDown)), 1, "a line down");
        let page = frame(&mut w, Some(egui::Key::PageDown));
        assert!(page > 10, "a screen down: {page}");
        assert_eq!(frame(&mut w, Some(egui::Key::ArrowUp)), page - 1);
        assert!(frame(&mut w, Some(egui::Key::End)) > 450, "the end");
        assert_eq!(frame(&mut w, Some(egui::Key::Home)), 0, "the start");
        w.find.open = true;
        w.find.focus = true;
        frame(&mut w, None);
        assert_eq!(frame(&mut w, Some(egui::Key::PageDown)), 0, "the find field has the keyboard");
    }

    /// Shift+arrows select from the first line shown, Ctrl+C copies, Ctrl+A
    /// takes all; dragging the mouse selects; without Shift the arrows
    /// still scroll; the find field keeps its own keys.
    #[test]
    fn selecting_and_copying() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("a.txt");
        std::fs::write(&file, "první řádek\ndruhý\ntřetí\n").unwrap();
        let mut w = ViewerWin::new(Doc::open(&file));
        let ctx = egui::Context::default();
        let screen = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0)));
        let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let frame = |w: &mut ViewerWin, events: Vec<egui::Event>, modifiers: egui::Modifiers| {
            let out = ctx.run(egui::RawInput { screen_rect: screen, events, modifiers, ..Default::default() }, |ctx| {
                w.show(ctx);
            });
            out.platform_output.copied_text
        };
        let (shift, ctrl) = (egui::Modifiers::SHIFT, egui::Modifiers::COMMAND);
        frame(&mut w, vec![], egui::Modifiers::NONE);
        frame(&mut w, vec![key(egui::Key::ArrowDown, shift), key(egui::Key::ArrowRight, shift)], shift);
        assert_eq!(frame(&mut w, vec![egui::Event::Copy], ctrl), "první řádek\r\nd");
        frame(&mut w, vec![key(egui::Key::End, shift)], shift);
        assert_eq!(frame(&mut w, vec![key(egui::Key::Insert, ctrl)], ctrl), "první řádek\r\ndruhý", "Ctrl+Insert copies too");
        frame(&mut w, vec![key(egui::Key::A, ctrl)], ctrl);
        assert_eq!(frame(&mut w, vec![egui::Event::Copy], ctrl), "první řádek\r\ndruhý\r\ntřetí");
        // a drag from the start of the first line to the middle of the second
        assert_eq!(w.sel.shown.start, 0);
        let pointer = |p: egui::Pos2, down: Option<bool>| {
            let mut ev = vec![egui::Event::PointerMoved(p)];
            if let Some(pressed) = down {
                ev.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            }
            ev
        };
        // the text starts below the toolbar: the first point below it that
        // a click puts the caret at (right of the line numbers)
        w.sel = TextSel::default();
        assert!(w.numbers);
        let font = egui::TextStyle::Monospace.resolve(&ctx.style());
        let (line_h, col) = ctx.fonts(|f| (f.row_height(&font), f.glyph_width(&font, '0')));
        let numbers = col * 3.0 + 2.0 * ctx.style().spacing.item_spacing.x;
        let top = (0..400).map(|y| egui::pos2(30.0 + numbers, y as f32)).find(|&p| {
            frame(&mut w, pointer(p, Some(true)), egui::Modifiers::NONE);
            frame(&mut w, pointer(p, Some(false)), egui::Modifiers::NONE);
            // above a line's text is its start; on it, where it is pointed at
            w.sel.placed && w.sel.sel.caret.col > 0
        });
        let top = top.expect("a point on the first line");
        let start = w.sel.sel.caret;
        assert!(start.line == 0 && start.col > 0 && w.sel.sel.is_empty(), "a click puts the caret, selecting nothing: {start:?}");
        // press there, drag down a line and right, release
        frame(&mut w, pointer(top, Some(true)), egui::Modifiers::NONE);
        let to = top + egui::vec2(col * 2.0, line_h * 1.5);
        frame(&mut w, pointer(top + egui::vec2(col, 10.0), None), egui::Modifiers::NONE);
        frame(&mut w, pointer(to, None), egui::Modifiers::NONE);
        frame(&mut w, pointer(to, Some(false)), egui::Modifiers::NONE);
        let end = w.sel.sel.caret;
        assert!(end.line == 1 && end.col > start.col, "dragged across the line break: {end:?}");
        let l = &w.doc.lines;
        let dragged = format!("{}\r\n{}", &l[0][start.col..], &l[1][..end.col]);
        assert_eq!(frame(&mut w, vec![egui::Event::Copy], ctrl), dragged);
        // the arrows without Shift scroll, and leave the selection
        frame(&mut w, vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)], egui::Modifiers::NONE);
        assert_eq!(frame(&mut w, vec![egui::Event::Copy], ctrl), dragged);
        // a double-click takes a word (a while after the clicks before: not a triple-click)
        let _ = ctx.run(egui::RawInput { screen_rect: screen, time: Some(100.0), ..Default::default() }, |ctx| {
            w.show(ctx);
        });
        for pressed in [true, false, true, false] {
            frame(&mut w, pointer(top, Some(pressed)), egui::Modifiers::NONE);
        }
        assert_eq!(frame(&mut w, vec![egui::Event::Copy], ctrl), "první");
        // the find field has the keyboard: Ctrl+A is the field's
        w.find.open = true;
        w.find.focus = true;
        frame(&mut w, vec![], egui::Modifiers::NONE);
        frame(&mut w, vec![key(egui::Key::A, ctrl)], ctrl);
        assert_eq!(w.sel.sel.text(&w.doc.lines, "\n"), "první");
    }

    /// Ctrl+T plays: lines written to the file show up (after a while);
    /// paused, they don't.
    #[test]
    fn play_and_pause() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("app.log");
        std::fs::write(&file, "start\n").unwrap();
        let mut w = ViewerWin::new(Doc::open(&file));
        let ctx = egui::Context::default();
        let frame = |w: &mut ViewerWin, ctrl_t: bool| {
            let events = if ctrl_t {
                vec![egui::Event::Key { key: egui::Key::T, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }]
            } else {
                Vec::new()
            };
            let modifiers = if ctrl_t { egui::Modifiers::COMMAND } else { egui::Modifiers::NONE };
            let _ = ctx.run(egui::RawInput { events, modifiers, ..Default::default() }, |ctx| {
                w.show(ctx);
            });
        };
        let write = |text: &str| {
            use std::io::Write;
            std::fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(text.as_bytes()).unwrap();
        };
        write("paused\n");
        frame(&mut w, false);
        assert_eq!(*w.doc.lines, ["start"], "not followed yet");
        frame(&mut w, true);
        assert!(w.following);
        assert_eq!(*w.doc.lines, ["start", "paused"], "Play catches up at once");
        write("live\n");
        w.polled = Some(Instant::now() - POLL);
        frame(&mut w, false);
        assert_eq!(w.doc.lines.last().unwrap(), "live");
        frame(&mut w, true);
        assert!(!w.following);
        write("not shown\n");
        w.polled = None;
        frame(&mut w, false);
        assert_eq!(w.doc.lines.last().unwrap(), "live", "paused");
    }
}
