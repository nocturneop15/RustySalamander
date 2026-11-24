//! The window comparing two files: text side by side, lined up, with the
//! differences colored and a way from one to the next; or what differs in
//! binary files; a find bar finds text on both sides. The window itself, on
//! a thread of its own, is `sal-window`'s.

use std::collections::BTreeSet;
use std::path::PathBuf;

use sal_core::encoding::{display_path, format_thousands};
use sal_plugin_api::{tr, trf};
use sal_window::egui::{self, Color32, RichText};
use sal_window::Content;

use sal_plugin_api::settings::Settings;

use crate::diff::{self, Outcome, Row, Tag};
use crate::find::{self, Hit, Query};
use crate::fold::{self, Line};

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
    /// The lines wholly in sight (of the view's): a difference or a match
    /// gone to among them is not scrolled to, the text stays still.
    in_sight: std::ops::Range<usize>,
    /// Counts the comparisons made: the matches found are for one of them.
    version: u64,
    /// How far each side's text (left, right) is moved to the left, so a
    /// match far along a long line is in sight.
    shift: [f32; 2],
    /// Bring this match into sight, sideways too (next frame).
    reveal: Option<Hit>,
    /// The texts' rows as Find searches them, beside the window.
    search_rows: std::sync::Arc<Vec<Row>>,
    /// Line numbers beside both texts; on until turned off.
    numbers: bool,
    /// The same parts hidden, but for a few lines around each difference.
    only_changes: bool,
    /// The hidden parts shown again (by their first row), until the texts
    /// are compared again or Only changes is turned on again.
    opened: BTreeSet<usize>,
    /// The lines of the view with parts hidden; `None`: every row.
    shown: Option<Vec<Line>>,
    /// What differs inside the lines changed, by row: found as they come
    /// into sight, kept until the texts are compared again.
    inline: Inline,
    /// Where the choices of the toolbar are remembered for the next window.
    settings: Settings,
}

/// What a window remembers for the next one: line numbers on, spaces
/// ignored, only the changes shown.
const NUMBERS: &str = "line_numbers";
const IGNORE_SPACE: &str = "ignore_spaces";
const ONLY_CHANGES: &str = "only_changes";

/// The parts changed inside a line changed, on the left and on the right.
type Inline = std::collections::HashMap<usize, (Vec<std::ops::Range<usize>>, Vec<std::ops::Range<usize>>)>;

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
    /// The worker searching beside the window (one per window: the latest
    /// query only), started at the first search.
    searcher: Option<Searcher>,
    /// What it was last asked for, until it is done.
    pending: Option<Pending>,
}

/// The search the worker does for the find bar: what to look for in what.
type Searcher = sal_ui::latest::Latest<(std::sync::Arc<Vec<Row>>, Query), (Vec<Hit>, bool)>;

/// The search asked for and not done yet: for `key`, to start from row
/// `from` when it is done.
struct Pending {
    key: (Query, u64),
    from: usize,
}

impl FindBar {
    /// Find again when the text, the options or the comparison changed,
    /// beside the window (big files take a while; the window stays usable):
    /// what was found shows when it is done. A new search starts from the
    /// current match, or the first row shown. `wake` draws the window again
    /// then. The row of a match newly current, to scroll to.
    fn refresh(&mut self, rows: &std::sync::Arc<Vec<Row>>, version: u64, top_row: usize, wake: impl Fn() + Send + 'static) -> Option<usize> {
        if !self.open {
            self.cancel();
            self.hits.clear();
            self.made_for = None;
            self.current = None;
            return None;
        }
        let key = (self.query.clone(), version);
        // back to what the matches shown are for (typed on, then back,
        // before the other search was done): those stay, and the other search
        // is given up (else it would stay "Searching…"); drawn again, as this
        // frame may have said so already and a search given up never wakes
        if self.made_for.as_ref() == Some(&key) {
            if self.pending.is_some() {
                self.cancel();
                wake();
            }
            return None;
        }
        // other texts (compared again): the matches found before don't fit
        // them (a row may be gone, a line shorter); only for other words in
        // the same texts do they stay shown until the search is done
        if self.made_for.as_ref().is_some_and(|m| m.1 != key.1) || self.pending.as_ref().is_some_and(|p| p.key.1 != key.1) {
            self.forget();
        }
        if let Some(p) = self.pending.as_ref().filter(|p| p.key == key) {
            let (hits, more) = self.searcher.as_ref().and_then(Searcher::take)?;
            let from = p.from;
            self.pending = None;
            (self.hits, self.more) = (hits, more);
            self.current = find::first_from(&self.hits, from);
            self.made_for = Some(key);
            return self.current.map(|c| self.hits[c].row);
        }
        // a new search: it replaces one asked for before
        let from = self.current.and_then(|c| self.hits.get(c)).map_or(top_row, |h| h.row);
        if self.query.text.is_empty() {
            self.cancel();
            (self.hits, self.more, self.current) = (Vec::new(), false, None);
            self.made_for = Some(key);
            return None;
        }
        let searcher = self.searcher.get_or_insert_with(|| Searcher::new("compare find", |(rows, query), stop| find::find_until(&rows, &query, stop), wake));
        searcher.ask((rows.clone(), self.query.clone()));
        self.pending = Some(Pending { key, from });
        None
    }

    /// Forget the matches found (the texts changed): none marked, none to go to.
    fn forget(&mut self) {
        (self.hits, self.more, self.current, self.made_for) = (Vec::new(), false, None, None);
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

    /// "3 of 12", "Not found", "Searching…", or nothing yet.
    fn status(&self) -> String {
        if self.query.text.is_empty() {
            return String::new();
        }
        // not "Not found" before it is done
        if self.searching() {
            return tr("Searching…");
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
    /// A window with the choices of a new one (none remembered).
    #[cfg(test)]
    pub fn new(left: PathBuf, right: PathBuf) -> DiffWin {
        DiffWin::with_settings(left, right, Settings::in_memory())
    }

    /// A window comparing `left` and `right` as the last one was left: line
    /// numbers, spaces ignored or not.
    pub fn with_settings(left: PathBuf, right: PathBuf, settings: Settings) -> DiffWin {
        let mut w = DiffWin {
            left,
            right,
            ignore_space: settings.bool(IGNORE_SPACE, false),
            outcome: Err(String::new()),
            current: 0,
            scroll_to: None,
            find: FindBar::default(),
            top_row: 0,
            in_sight: 0..0,
            version: 0,
            shift: [0.0; 2],
            reveal: None,
            search_rows: Default::default(),
            numbers: settings.bool(NUMBERS, true),
            only_changes: settings.bool(ONLY_CHANGES, false),
            opened: BTreeSet::new(),
            shown: None,
            inline: Inline::new(),
            settings,
        };
        w.compare();
        w
    }

    /// Hide the same parts again, or show every row (Only changes off).
    fn refold(&mut self) {
        self.shown = match &self.outcome {
            Ok(Outcome::Text { rows, .. }) if self.only_changes => Some(fold::lines(rows, &self.opened)),
            _ => None,
        };
    }

    /// Show the hidden part starting at row `start`.
    fn open(&mut self, start: usize) {
        self.opened.insert(start);
        self.refold();
    }

    /// Compare the files (again), and go to the first difference.
    fn compare(&mut self) {
        self.outcome = diff::compare(&self.left, &self.right, self.ignore_space).map_err(|e| e.to_string());
        self.current = 0;
        self.scroll_to = self.changes().first().copied();
        // other texts: nothing of them in sight yet
        self.in_sight = 0..0;
        self.version += 1;
        self.shift = [0.0; 2];
        self.reveal = None;
        // the same rows, shared: not a copy of every line
        self.search_rows = match &self.outcome {
            Ok(Outcome::Text { rows, .. }) => rows.clone(),
            _ => Default::default(),
        };
        self.opened.clear();
        self.inline.clear();
        self.refold();
    }

    /// The first rows of the blocks of differences.
    fn changes(&self) -> &[usize] {
        match &self.outcome {
            Ok(Outcome::Text { changes, .. }) => changes,
            _ => &[],
        }
    }

    /// The rows of the block of differences gone to last (Next, Previous).
    fn current_block(&self) -> Option<std::ops::Range<usize>> {
        let Ok(Outcome::Text { rows, changes }) = &self.outcome else { return None };
        let start = *changes.get(self.current)?;
        let end = rows[start..].iter().position(|r| r.tag == Tag::Same).map_or(rows.len(), |n| start + n);
        Some(start..end)
    }

    /// Go to the next (`forward`) or the previous block of differences.
    fn step(&mut self, forward: bool) {
        let n = self.changes().len();
        if n == 0 {
            return;
        }
        self.current = if forward { (self.current + 1).min(n - 1) } else { self.current.saturating_sub(1) };
        self.scroll_to = Some(self.changes()[self.current]);
        // the lines from their start again
        self.shift = [0.0; 2];
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
        // as the user leaves them, remembered at the end of the frame
        let chosen = (self.numbers, self.ignore_space);
        if is_text && ctx.input(|i| i.key_pressed(egui::Key::L) && i.modifiers.command) {
            self.numbers = !self.numbers;
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
                ui.add_enabled_ui(is_text, |ui| {
                    ui.checkbox(&mut self.numbers, tr("Line numbers")).on_hover_text(tr("Line numbers beside both texts (Ctrl+L)"));
                    let only = ui
                        .checkbox(&mut self.only_changes, tr("Only changes"))
                        .on_hover_text(tr("Hide the lines that are the same, but for a few around each difference; + beside a hidden part shows it"));
                    if only.changed() {
                        // turned on again: everything hidden again
                        self.opened.clear();
                        self.refold();
                    }
                });
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
        let found = find.refresh(&self.search_rows, self.version, self.top_row, wake).is_some();
        let stepped = step.and_then(|forward| find.step(forward)).is_some();
        if found || stepped {
            let hit = find.current.map(|c| find.hits[c]);
            self.scroll_to = hit.map(|h| h.row);
            self.reveal = hit;
        }
        if !find.open {
            self.shift = [0.0; 2];
        }
        // a match in a hidden part: the part is shown
        if let Some(start) = self.scroll_to.and_then(|r| self.shown.as_deref().and_then(|s| fold::hiding(s, r))) {
            self.open(start);
        }
        let mut open = None;
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
                let current = self.current_block().unwrap_or(0..0);
                let view = View { shown: self.shown.as_deref(), numbers: self.numbers, current, inline: &mut self.inline, ignore_space: self.ignore_space, in_sight: self.in_sight.clone() };
                (self.top_row, open, self.in_sight) = text_view(ui, rows, &self.left, &self.right, self.scroll_to.take(), &marks, &mut self.shift, self.reveal.take(), view);
            }
        });
        match open {
            Some((start, true)) => self.open(start),
            Some((start, false)) => {
                self.opened.remove(&start);
                self.refold();
            }
            None => {}
        }
        // when it changed only (a save that failed is tried again with the
        // next change, not every frame)
        if self.only_changes != self.settings.bool(ONLY_CHANGES, false) {
            let _ = self.settings.set_bool(ONLY_CHANGES, self.only_changes);
        }
        if self.numbers != chosen.0 {
            let _ = self.settings.set_bool(NUMBERS, self.numbers);
        }
        if self.ignore_space != chosen.1 {
            let _ = self.settings.set_bool(IGNORE_SPACE, self.ignore_space);
        }
        true
    }
}

/// The colors behind a row's two sides: faint, or strong for the difference
/// gone to last (`current`), so it stands out from the others.
fn fills(tag: Tag, dark: bool, current: bool) -> (Color32, Color32) {
    let a = match (current, dark) {
        (false, true) => 16,
        (false, false) => 19,
        (true, true) => 60,
        (true, false) => 85,
    };
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

/// How the texts are shown: the lines with parts hidden (`None`: every
/// row), line numbers or not, and the rows of the difference gone to last
/// (marked).
struct View<'a> {
    shown: Option<&'a [Line]>,
    numbers: bool,
    current: std::ops::Range<usize>,
    /// What differs inside the lines changed, found so far (more added as
    /// they come into sight), spaces counted or not.
    inline: &'a mut Inline,
    ignore_space: bool,
    /// The lines wholly in sight in the last frame (indexes into `shown`).
    in_sight: std::ops::Range<usize>,
}

/// How wide the bar beside the difference gone to last is.
const CURRENT_BAR: f32 = 4.0;

/// The bar at the left edge of a row of the difference gone to last, in
/// the selection's outline color (dark in the light theme, light in the
/// dark); unbroken down the rows (over the gap to the next, but the `last`).
fn current_bar(ui: &egui::Ui, row: egui::Rect, last: bool) {
    let gap = if last { 0.0 } else { ui.spacing().item_spacing.y };
    let bar = egui::Rect::from_min_size(row.min, egui::vec2(CURRENT_BAR, row.height() + gap));
    ui.painter().rect_filled(bar, 0.0, ui.visuals().selection.stroke.color);
}

/// The two texts side by side, a row per line pair, with line numbers on
/// both sides (unless not `view.numbers`) and the matches found marked, each
/// side's text moved left by `shift` (and that changed so the match
/// `reveal` is in sight); the parts hidden each in a line of their own.
/// Returns the first row shown, the first row of a part clicked, with
/// whether to show it (or hide it again), and the lines wholly in sight.
#[allow(clippy::too_many_arguments)]
fn text_view(
    ui: &mut egui::Ui,
    rows: &[Row],
    left: &std::path::Path,
    right: &std::path::Path,
    scroll_to: Option<usize>,
    marks: &Marks,
    shift: &mut [f32; 2],
    reveal: Option<Hit>,
    view: View,
) -> (usize, Option<(usize, bool)>, std::ops::Range<usize>) {
    let View { shown, numbers, current, inline, ignore_space, in_sight } = view;
    let count = shown.map_or(rows.len(), <[Line]>::len);
    let line_at = |k: usize| shown.map_or(Line::Row(k), |s| s[k]);
    // a row's place among the lines shown
    let scroll_to = scroll_to.map(|r| shown.map_or(r, |s| fold::index_of(s, r)));
    let mut clicked = None;
    let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 2.0;
    let digits = rows.len().max(1).to_string().len();
    // without numbers, the text keeps a little room from the edge
    let num_w = if numbers { ui.fonts(|f| f.layout_no_wrap("0".repeat(digits), egui::TextStyle::Monospace.resolve(ui.style()), Color32::WHITE).size().x) + 10.0 } else { 4.0 };
    let dark = ui.visuals().dark_mode;
    let gap = sal_ui::layout::COLUMN_GAP;
    let half = ((ui.available_width() - gap) / 2.0).max(50.0);
    if let Some(h) = reveal.filter(|h| h.row < rows.len()) {
        let side = if h.right { &rows[h.row].right } else { &rows[h.row].left };
        if let Some((_, text)) = side.as_ref().filter(|(_, t)| h.end <= t.len() && t.is_char_boundary(h.start) && t.is_char_boundary(h.end)) {
            let mono = egui::TextStyle::Monospace.resolve(ui.style());
            let width = |s: &str| ui.fonts(|f| f.layout_no_wrap(s.to_owned(), mono.clone(), Color32::WHITE).size().x);
            // where the text is drawn: the side's width less its line numbers
            let view = half - gap / 2.0 - num_w;
            shift[h.right as usize] = reveal_shift(shift[h.right as usize], width(&text[..h.start]), width(&text[..h.end]), view);
        }
    }
    ui.horizontal(|ui| {
        for p in [left, right] {
            ui.add_sized([half, row_h], egui::Label::new(RichText::new(display_path(p)).weak()).truncate());
        }
    });
    let mut area = egui::ScrollArea::vertical().auto_shrink(false).scroll_bar_visibility(sal_ui::scroll_bar_visibility(ui));
    // already in sight (the whole difference gone to): the text stays where
    // it is, only the mark moves; else scrolled to
    let scroll_to = scroll_to.filter(|&r| {
        if r >= count {
            return false;
        }
        let end = if line_at(r).first_row() == current.start { shown.map_or(current.end, |s| fold::index_of(s, current.end.saturating_sub(1)) + 1) } else { r + 1 };
        !(in_sight.start <= r && end <= in_sight.end)
    });
    if let Some(r) = scroll_to {
        // the difference a few rows below the top
        area = area.vertical_scroll_offset((r.saturating_sub(3) as f32) * (row_h + ui.spacing().item_spacing.y));
    }
    let mut top = 0;
    let mut seen = 0..0;
    area.show_rows(ui, row_h, count, |ui, range| {
        // the lines wholly in sight (the first and the last may be cut)
        seen = (range.start + 1).min(range.end)..range.end.saturating_sub(1).max(range.start);
        top = if range.start < count { line_at(range.start).first_row() } else { 0 };
        for k in range {
            let i = match line_at(k) {
                Line::Row(i) => i,
                Line::Fold { start, end } | Line::Opened { start, end } => {
                    let shown = matches!(line_at(k), Line::Opened { .. });
                    if fold_line(ui, egui::vec2(half * 2.0, row_h), end - start, shown, numbers.then_some((num_w, half + gap / 2.0))).clicked() {
                        clicked = Some((start, !shown));
                    }
                    continue;
                }
            };
            let row = &rows[i];
            let is_current = current.contains(&i);
            let (lf, rf) = fills(row.tag, dark, is_current);
            // a line changed: the words that differ, marked on both sides
            let parts = match (&row.left, &row.right) {
                (Some((_, l)), Some((_, r))) if row.tag == Tag::Changed => Some(&*inline.entry(i).or_insert_with(|| crate::inline::changed(l, r, ignore_space))),
                _ => None,
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (right, side, fill) in [(false, &row.left, lf), (true, &row.right, rf)] {
                    let moved = shift[right as usize];
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(half - gap / 2.0, row_h), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, fill);
                    if numbers {
                        // the numbers' column in the colors of the panels' footer, unbroken
                        // down the side (over the gap to the next row too)
                        let column = egui::Rect::from_min_size(rect.min, egui::vec2(num_w - 3.0, row_h + ui.spacing().item_spacing.y));
                        ui.painter().rect_filled(column, 0.0, sal_ui::layout::strip_color(ui.visuals()));
                    }
                    if is_current && !right {
                        current_bar(ui, rect, i + 1 == current.end);
                    }
                    if let Some((n, text)) = side {
                        let mono = egui::TextStyle::Monospace.resolve(ui.style());
                        if numbers {
                            let strong = ui.visuals().strong_text_color();
                            ui.painter().text(rect.left_center() + egui::vec2(num_w - 6.0, 0.0), egui::Align2::RIGHT_CENTER, n.to_string(), mono.clone(), strong);
                        }
                        let clip = egui::Rect::from_min_max(egui::pos2(rect.left() + num_w, rect.top()), rect.max);
                        let changed = parts.map_or(&[][..], |(l, r)| if right { &r[..] } else { &l[..] });
                        // on the strong colors of the difference gone to last, the strong
                        // text color (white in the dark theme, black in the light)
                        let color = if is_current { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
                        let job = marks.job(text, i, right, changed, is_current, mono, color);
                        let galley = ui.fonts(|f| f.layout_job(job));
                        let at = clip.left_center() - egui::vec2(moved, galley.size().y / 2.0);
                        ui.painter().with_clip_rect(clip).galley(at, galley, color);
                    }
                }
            });
        }
    });
    (top, clicked, seen)
}

/// The line standing for `hidden` same lines, `size` large: "+ Same lines:
/// 120", shown when clicked; above them when `shown`, "− Same lines: 120",
/// hidden again when clicked. With line numbers (`numbers`: their column's
/// width, and where the right side starts) their columns go on through it.
fn fold_line(ui: &mut egui::Ui, size: egui::Vec2, hidden: usize, shown: bool, numbers: Option<(f32, f32)>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let tip = if shown { tr("Hide these lines again") } else { tr("Show the lines hidden here") };
    let response = response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
    let v = ui.visuals();
    let p = ui.painter();
    // the panels' footer's colors (a strip standing out from the text, its
    // text strong), a button's when pointed at: readable in both themes
    let (fill, text) = if response.hovered() {
        (v.widgets.hovered.weak_bg_fill, v.widgets.hovered.text_color())
    } else {
        (sal_ui::layout::strip_color(v), v.strong_text_color())
    };
    p.rect_filled(rect, 0.0, fill);
    let text_x = match numbers {
        Some((num_w, right)) => {
            let strip = sal_ui::layout::strip_color(v);
            let h = size.y + ui.spacing().item_spacing.y;
            for x in [rect.left(), rect.left() + right] {
                p.rect_filled(egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(num_w - 3.0, h)), 0.0, strip);
            }
            rect.left() + num_w
        }
        None => rect.left() + 4.0,
    };
    // the small + button, then how many lines it shows
    let side = (size.y - 4.0).max(8.0);
    let button = egui::Rect::from_min_size(egui::pos2(text_x, rect.center().y - side / 2.0), egui::vec2(side, side));
    let w = if response.hovered() { &v.widgets.hovered } else { &v.widgets.inactive };
    p.rect(button, 2.0, w.weak_bg_fill, w.bg_stroke);
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    p.text(button.center(), egui::Align2::CENTER_CENTER, if shown { "−" } else { "+" }, mono.clone(), w.text_color());
    let label = trf("Same lines: {n}", &[("n", &format_thousands(hidden as u64))]);
    p.text(egui::pos2(button.right() + 6.0, rect.center().y), egui::Align2::LEFT_CENTER, label, mono, text);
    response
}

/// How far to move a side's text left (now `shift`) so the match drawn at
/// `x0..x1` of its line is in sight in `view` pixels: as it is when it is
/// already, from the line's start when it fits there, else with the match a
/// third into the view, or further when its end wouldn't fit then (a match
/// wider than the view shows from its start).
fn reveal_shift(shift: f32, x0: f32, x1: f32, view: f32) -> f32 {
    if x0 >= shift && x1 <= shift + view {
        shift
    } else if x1 <= view {
        0.0
    } else {
        (x0 - view / 3.0).max(x1 - view).min(x0).max(0.0)
    }
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
    /// Behind the words changed in a line changed, and in one of the
    /// difference gone to last.
    changed: Color32,
    changed_current: Color32,
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
            // the line's own amber (see fills), much stronger
            changed: if dark { Color32::from_rgba_unmultiplied(230, 160, 30, 75) } else { Color32::from_rgba_unmultiplied(230, 150, 20, 90) },
            // dark: a deeper amber, white text on it stays readable
            changed_current: if dark { Color32::from_rgba_unmultiplied(215, 135, 15, 115) } else { Color32::from_rgba_unmultiplied(225, 135, 10, 175) },
        }
    }

    /// One side's text of row `row`: the parts `changed` from the other
    /// side's line on a stronger amber (stronger still in the difference
    /// gone to last, `current`, whose line is stronger too), its matches on
    /// their background (over those).
    #[allow(clippy::too_many_arguments)]
    fn job(&self, text: &str, row: usize, right: bool, changed: &[std::ops::Range<usize>], current: bool, font: egui::FontId, color: Color32) -> egui::text::LayoutJob {
        let changed_bg = if current { self.changed_current } else { self.changed };
        use egui::text::{LayoutJob, TextFormat};
        let plain = TextFormat { font_id: font, color, ..Default::default() };
        let fits = |r: &std::ops::Range<usize>| r.start <= r.end && r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end);
        let hits: Vec<&Hit> = find::on_side(self.hits, row, right).filter(|h| fits(&(h.start..h.end))).collect();
        let changed: Vec<&std::ops::Range<usize>> = changed.iter().filter(|r| fits(r)).collect();
        // where the look changes: the text cut there, each piece in its own
        let mut cuts: Vec<usize> = [0, text.len()].into_iter().chain(hits.iter().flat_map(|h| [h.start, h.end])).chain(changed.iter().flat_map(|r| [r.start, r.end])).collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut job = LayoutJob::default();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let format = if let Some(h) = hits.iter().find(|h| h.start <= a && b <= h.end) {
                let current = Some(**h) == self.current;
                let background = if current { self.this } else { self.other };
                // the current match is solid: its text in a color readable on it
                let color = if current { sal_ui::readable_on(background) } else { color };
                TextFormat { background, color, ..plain.clone() }
            } else if changed.iter().any(|r| r.start <= a && b <= r.end) {
                TextFormat { background: changed_bg, ..plain.clone() }
            } else {
                plain.clone()
            };
            job.append(&text[a..b], 0.0, format);
        }
        job
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn once(w: &mut DiffWin, ctx: &egui::Context, events: Vec<egui::Event>) {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 400.0));
        // the keys' modifiers held down, as egui reads them
        let modifiers = events.iter().find_map(|e| if let egui::Event::Key { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
        let raw = egui::RawInput { screen_rect: Some(rect), events, modifiers, ..Default::default() };
        let _ = ctx.run(raw, |ctx| {
            w.show(ctx);
        });
    }

    /// A frame, then as many as the search beside the window takes.
    fn frame(w: &mut DiffWin, ctx: &egui::Context, events: Vec<egui::Event>) {
        once(w, ctx, events);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while w.find.searching() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(2));
            once(w, ctx, vec![]);
        }
    }

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

    /// Line numbers are on both sides until Ctrl+L (or the box) turns
    /// them off, on both sides together.
    #[test]
    fn line_numbers_on_both_sides() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        std::fs::write(&a, "jedna\ndva\ntři\n").unwrap();
        std::fs::write(&b, "jedna\ndva\nTŘI\n").unwrap();
        let mut w = DiffWin::new(a, b);
        let ctx = egui::Context::default();
        let run = |w: &mut DiffWin, events: Vec<egui::Event>| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 400.0));
            let modifiers = events.iter().find_map(|e| if let egui::Event::Key { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
            let out = ctx.run(egui::RawInput { screen_rect: Some(rect), events, modifiers, ..Default::default() }, |ctx| {
                w.show(ctx);
            });
            drawn_texts(&out)
        };
        run(&mut w, vec![]);
        let on = run(&mut w, vec![]);
        assert!(w.numbers, "on at first");
        let threes: Vec<f32> = on.iter().filter(|(t, _)| t == "3").map(|(_, p)| p.x).collect();
        assert_eq!(threes.len(), 2, "on both sides: {on:?}");
        let x = |texts: &[(String, egui::Pos2)], s: &str| texts.iter().find(|(t, _)| t == s).map(|(_, p)| p.x).unwrap();
        assert!(threes.iter().any(|&n| n < x(&on, "tři")) && threes.iter().any(|&n| n < x(&on, "TŘI") && n > x(&on, "tři")));
        run(&mut w, vec![key(egui::Key::L, egui::Modifiers::COMMAND)]);
        let off = run(&mut w, vec![]);
        assert!(!w.numbers);
        assert!(!w.settings.bool(NUMBERS, true), "remembered for the next window");
        w.settings.set_bool(IGNORE_SPACE, true).unwrap();
        let next = DiffWin::with_settings(w.left.clone(), w.right.clone(), w.settings.clone());
        assert!(!next.numbers && next.ignore_space, "which opens so");
        assert!(!off.iter().any(|(t, _)| t == "3" || t == "1"), "on neither side: {off:?}");
        assert!(x(&off, "tři") < x(&on, "tři") && x(&off, "TŘI") < x(&on, "TŘI"), "the texts move to the edge");
    }

    /// Only changes hides the same middle of two texts but for 3 lines
    /// beside each difference; a click on the line standing for it shows
    /// it, and so does a match found in it. Remembered for the next window.
    #[test]
    fn only_changes() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        let middle: String = (1..=40).map(|n| format!("řádek {n}\n")).collect();
        std::fs::write(&a, format!("start\n{middle}konec\n")).unwrap();
        std::fs::write(&b, format!("START\n{middle}KONEC\n")).unwrap();
        let mut w = DiffWin::new(a, b);
        let ctx = egui::Context::default();
        let run = |w: &mut DiffWin, events: Vec<egui::Event>| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 400.0));
            let out = ctx.run(egui::RawInput { screen_rect: Some(rect), events, ..Default::default() }, |ctx| {
                w.show(ctx);
            });
            drawn_texts(&out)
        };
        run(&mut w, vec![]);
        assert!(!w.only_changes && w.shown.is_none(), "off at first");
        w.only_changes = true;
        w.refold();
        run(&mut w, vec![]);
        let texts = run(&mut w, vec![]);
        let has = |texts: &[(String, egui::Pos2)], s: &str| texts.iter().any(|(t, _)| t == s);
        assert!(has(&texts, "řádek 3") && has(&texts, "řádek 38"), "3 lines beside each difference: {texts:?}");
        assert!(!has(&texts, "řádek 4") && !has(&texts, "řádek 37"), "the rest hidden");
        let (_, at) = texts.iter().find(|(t, _)| t == "Same lines: 34").expect("a line says how many");
        // clicked: shown
        let at = *at + egui::vec2(5.0, 5.0);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        run(&mut w, vec![egui::Event::PointerMoved(at)]);
        run(&mut w, vec![press(true), press(false)]);
        let texts = run(&mut w, vec![]);
        assert!(has(&texts, "řádek 4") && fold::hiding(w.shown.as_deref().unwrap(), 37).is_none(), "shown: {texts:?}");
        assert!(has(&texts, "−") && has(&texts, "Same lines: 34"), "below its line, now −: {texts:?}");
        // clicked again: hidden again
        run(&mut w, vec![press(true), press(false)]);
        let texts = run(&mut w, vec![]);
        assert!(!has(&texts, "řádek 4") && has(&texts, "+"), "hidden again: {texts:?}");
        // turned on again: hidden again; a match in the hidden part shows it
        w.opened.clear();
        w.refold();
        assert!(fold::hiding(w.shown.as_deref().unwrap(), 20).is_some());
        w.scroll_to = Some(20);
        run(&mut w, vec![]);
        assert!(fold::hiding(w.shown.as_deref().unwrap(), 20).is_none(), "shown to scroll to");
        assert!(w.settings.bool(ONLY_CHANGES, false), "remembered");
        let next = DiffWin::with_settings(w.left.clone(), w.right.clone(), w.settings.clone());
        assert!(next.only_changes && next.shown.is_some(), "the next window opens so");
    }

    /// The difference gone to last is marked: its rows, the first at first,
    /// then the next one after Next.
    #[test]
    fn current_difference_is_marked() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        std::fs::write(&a, "a\nb\nc\nd\ne\nf\n").unwrap();
        std::fs::write(&b, "a\nB\nC\nd\ne\nF\nG\n").unwrap();
        let mut w = DiffWin::new(a, b);
        assert_eq!(w.current_block(), Some(1..3), "b, c changed");
        w.step(true);
        assert_eq!(w.current_block(), Some(5..7), "f changed, G added: one block");
        let ctx = egui::Context::default();
        once(&mut w, &ctx, vec![]);
        let accent = ctx.style().visuals.selection.stroke.color;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 400.0));
        let out = ctx.run(egui::RawInput { screen_rect: Some(rect), ..Default::default() }, |ctx| {
            w.show(ctx);
        });
        let bars = out.shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == accent && r.rect.width() == CURRENT_BAR)).count();
        assert_eq!(bars, 2, "a bar beside each of its rows");
        // its own colors strong, the others' faint
        for dark in [false, true] {
            for tag in [Tag::Changed, Tag::Removed, Tag::Added] {
                let (faint, strong) = (fills(tag, dark, false), fills(tag, dark, true));
                let a = |c: (Color32, Color32)| c.0.a().max(c.1.a());
                assert!(a(strong) >= 3 * a(faint), "{tag:?} dark={dark}");
            }
        }
    }

    /// A line changed shows what changed in it: found as it comes into
    /// sight, kept, forgotten when compared again.
    #[test]
    fn words_changed_in_a_line() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        std::fs::write(&a, "x\nlet x = foo(1);\n").unwrap();
        std::fs::write(&b, "x\nlet y = foo(1, 2);\n").unwrap();
        let mut w = DiffWin::new(a, b);
        let ctx = egui::Context::default();
        once(&mut w, &ctx, vec![]);
        once(&mut w, &ctx, vec![]);
        assert_eq!(w.inline.get(&1), Some(&(vec![4..5], vec![4..5, 13..16])), "x → y, and \", 2\" added");
        assert_eq!(w.inline.len(), 1, "only lines changed");
        w.compare();
        assert!(w.inline.is_empty());
    }

    /// Next to a difference already in sight moves only the mark: the text
    /// stays still. To one out of sight, it scrolls.
    #[test]
    fn next_in_sight_keeps_the_text_still() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        let lines = |changed: &[usize]| (0..300).map(|i| if changed.contains(&i) { format!("ŘÁDEK {i}\n") } else { format!("řádek {i}\n") }).collect::<String>();
        std::fs::write(&a, lines(&[])).unwrap();
        // differences at 20, 25 (close) and 250 (far)
        std::fs::write(&b, lines(&[20, 25, 250])).unwrap();
        let mut w = DiffWin::new(a, b);
        let ctx = egui::Context::default();
        for _ in 0..3 {
            once(&mut w, &ctx, vec![]);
        }
        let top = w.top_row;
        assert!(top <= 20, "the first difference in sight: {top}");
        once(&mut w, &ctx, vec![key(egui::Key::F8, egui::Modifiers::NONE)]);
        once(&mut w, &ctx, vec![]);
        assert_eq!(w.current_block(), Some(25..26));
        assert_eq!(w.top_row, top, "in sight already: not scrolled");
        once(&mut w, &ctx, vec![key(egui::Key::F8, egui::Modifiers::NONE)]);
        once(&mut w, &ctx, vec![]);
        assert!(w.top_row > 200 && w.top_row <= 250, "out of sight: scrolled to it ({})", w.top_row);
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
        // alike: side by side
        right.push_str("KŮŇ vlevo\nještě kůň\n");
        std::fs::write(&a, left).unwrap();
        std::fs::write(&b, right).unwrap();
        let mut w = DiffWin::new(a, b);
        let ctx = egui::Context::default();
        frame(&mut w, &ctx, vec![]);
        assert!(!w.takes_escape());
        frame(&mut w, &ctx, vec![key(egui::Key::F, egui::Modifiers::COMMAND)]);
        assert!(w.find.open && w.takes_escape());
        frame(&mut w, &ctx, vec![]);
        once(&mut w, &ctx, vec![egui::Event::Text("kůň".into())]);
        // the search runs beside the window: not "Not found" meanwhile
        assert!(w.find.searching());
        assert_eq!(w.find.status(), "Searching…");
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

    /// A match far along a long line is brought into sight sideways, on its
    /// own side; one near the start of a line puts the text back.
    #[test]
    fn far_matches_come_into_sight() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        std::fs::write(&a, format!("kůň na začátku\n{}kůň daleko\n", "x".repeat(400))).unwrap();
        std::fs::write(&b, "kůň na začátku\njiný\n").unwrap();
        let mut w = DiffWin::new(a, b);
        let ctx = egui::Context::default();
        frame(&mut w, &ctx, vec![key(egui::Key::F, egui::Modifiers::COMMAND)]);
        frame(&mut w, &ctx, vec![egui::Event::Text("kůň".into())]);
        frame(&mut w, &ctx, vec![]);
        assert_eq!(w.find.hits.len(), 3);
        assert_eq!(w.shift, [0.0, 0.0], "the first match is at the start");
        // the third: on the left, past 400 characters
        frame(&mut w, &ctx, vec![key(egui::Key::F3, egui::Modifiers::NONE)]);
        frame(&mut w, &ctx, vec![key(egui::Key::F3, egui::Modifiers::NONE)]);
        frame(&mut w, &ctx, vec![]);
        assert_eq!(w.find.current, Some(2));
        assert!(w.shift[0] > 1000.0 && w.shift[1] == 0.0, "{:?}", w.shift);
        // round to the first: from the start again
        frame(&mut w, &ctx, vec![key(egui::Key::F3, egui::Modifiers::NONE)]);
        frame(&mut w, &ctx, vec![]);
        assert_eq!(w.shift, [0.0, 0.0]);
    }

    /// The files are compared again, shorter and other, while Find is open:
    /// the matches in the texts before are forgotten at once (none drawn
    /// into the new text, none gone to with F3) until the new search is done.
    #[test]
    fn find_after_comparing_again() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        let long: String = (0..200).map(|i| format!("řádek {i} žluťoučký kůň\n")).collect();
        std::fs::write(&a, &long).unwrap();
        std::fs::write(&b, &long).unwrap();
        let mut w = DiffWin::new(a.clone(), b.clone());
        let ctx = egui::Context::default();
        frame(&mut w, &ctx, vec![key(egui::Key::F, egui::Modifiers::COMMAND)]);
        w.find.query.text = "kůň".into();
        frame(&mut w, &ctx, vec![]);
        assert_eq!(w.find.hits.len(), 400);
        for _ in 0..399 {
            w.find.step(true);
        }
        // shorter lines with multi-byte letters where the matches were
        std::fs::write(&a, "kůň\nčč\n").unwrap();
        std::fs::write(&b, "ž\n").unwrap();
        w.compare();
        once(&mut w, &ctx, vec![key(egui::Key::F3, egui::Modifiers::NONE)]);
        assert!(w.find.hits.is_empty() && w.find.current.is_none(), "forgotten");
        once(&mut w, &ctx, vec![key(egui::Key::F3, egui::Modifiers::NONE), key(egui::Key::F3, egui::Modifiers::SHIFT)]);
        frame(&mut w, &ctx, vec![key(egui::Key::F3, egui::Modifiers::NONE)]);
        assert_eq!(w.find.hits.len(), 1);
    }

    /// Typing fast: one worker, the last query's matches only.
    #[test]
    fn typing_fast_searches_once_at_a_time() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        let long: String = (0..2000).map(|i| format!("{i} abcdefghij\n")).collect();
        std::fs::write(&a, &long).unwrap();
        std::fs::write(&b, &long).unwrap();
        let mut w = DiffWin::new(a, b);
        let ctx = egui::Context::default();
        frame(&mut w, &ctx, vec![key(egui::Key::F, egui::Modifiers::COMMAND)]);
        for text in ["a", "ab", "abc", "abcd", "abcde", "x", "abcdefghij", "9 abc"] {
            w.find.query.text = text.into();
            once(&mut w, &ctx, vec![]);
        }
        frame(&mut w, &ctx, vec![]);
        // "9 abc": 9, 19, 29 … 1999: a line ending in 9, on both sides
        assert_eq!(w.find.hits.len(), 2 * 200, "the last query's");
        assert_eq!(w.find.made_for.as_ref().map(|m| m.0.text.as_str()), Some("9 abc"));
    }

    /// Typed on and back again before the other search was done: the
    /// matches shown stay (and the one gone to), and nothing is left
    /// "Searching…" (whatever the worker's speed); the toolbar shows them.
    #[test]
    fn back_to_the_words_shown() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a.txt"), t.path().join("b.txt"));
        std::fs::write(&a, "kůň a\nb kůň\n").unwrap();
        std::fs::write(&b, "kůň\n").unwrap();
        let mut w = DiffWin::new(a, b);
        let ctx = egui::Context::default();
        frame(&mut w, &ctx, vec![key(egui::Key::F, egui::Modifiers::COMMAND)]);
        w.find.query.text = "kůň".into();
        frame(&mut w, &ctx, vec![]);
        w.find.step(true);
        let (hits, current) = (w.find.hits.clone(), w.find.current);
        assert_eq!((hits.len(), current), (3, Some(1)));
        let rows = w.search_rows.clone();
        let version = w.version;
        // B asked for, then back to A before B is looked at again
        w.find.query.text = "b".into();
        assert!(w.find.refresh(&rows, version, 0, || {}).is_none());
        assert!(w.find.searching());
        w.find.query.text = "kůň".into();
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wake = woken.clone();
        assert!(w.find.refresh(&rows, version, 0, move || wake.store(true, std::sync::atomic::Ordering::Relaxed)).is_none());
        assert!(w.find.pending.is_none() && !w.find.searching(), "not left searching");
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed), "drawn again");
        assert_eq!((w.find.hits.clone(), w.find.current), (hits, current), "A's matches, A's place");
        // the next frame: A's count in the toolbar, and B's result never takes over
        std::thread::sleep(std::time::Duration::from_millis(50));
        once(&mut w, &ctx, vec![]);
        assert_eq!((w.find.hits.len(), w.find.current, w.find.status().as_str()), (3, Some(1), "2 of 3"));
    }

    #[test]
    fn sideways_shift() {
        assert_eq!(reveal_shift(0.0, 10.0, 40.0, 300.0), 0.0, "in sight");
        assert_eq!(reveal_shift(500.0, 600.0, 640.0, 300.0), 500.0, "in sight, moved");
        assert_eq!(reveal_shift(500.0, 10.0, 40.0, 300.0), 0.0, "at the start");
        assert_eq!(reveal_shift(0.0, 1000.0, 1040.0, 300.0), 900.0, "a third in");
        assert_eq!(reveal_shift(0.0, 1000.0, 1250.0, 300.0), 950.0, "a long match: its end in sight");
        assert_eq!(reveal_shift(0.0, 1000.0, 1400.0, 300.0), 1000.0, "wider than the view: from its start");
    }
}
