//! The viewer windows. They run on a thread of the plugin's own, with an
//! event loop of their own (egui and its window library are inside this
//! DLL, apart from the program's), kept for the whole session: the loop
//! runs while a window is open and waits for the next file when the last
//! one closes.
//!
//! The first window is the loop's main window; more files open in more
//! windows beside it. When the main window is closed while others are open,
//! it takes over the newest of them (its file and its place), so every
//! window closes on its own.

use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, ViewportBuilder, ViewportCommand, ViewportId};
use sal_core::encoding;
use sal_plugin_api::{tr, trf, Values};

use crate::doc::{Colors, Doc};
use crate::highlight::{self, Span};

enum Msg {
    Open(PathBuf, Values),
    Quit,
}

type Slot = Arc<Mutex<Option<egui::Context>>>;

/// The viewer windows' thread, started with the first file.
#[derive(Default)]
pub struct Windows {
    tx: Mutex<Option<Sender<Msg>>>,
    /// The windows' context while the loop runs, to wake it for a new file.
    ctx: Slot,
}

impl Windows {
    /// Show `path` in a new window.
    pub fn open(&self, path: PathBuf, options: Values) -> io::Result<()> {
        let mut tx = self.tx.lock().unwrap();
        let mut msg = Msg::Open(path, options);
        if let Some(t) = tx.as_ref() {
            match t.send(msg) {
                Ok(()) => {
                    self.wake();
                    return Ok(());
                }
                // the thread ended: start it again
                Err(mpsc::SendError(m)) => msg = m,
            }
        }
        let t = spawn(self.ctx.clone())?;
        t.send(msg).map_err(|_| io::Error::other("the viewer thread ended"))?;
        *tx = Some(t);
        Ok(())
    }

    fn wake(&self) {
        if let Some(ctx) = &*self.ctx.lock().unwrap() {
            ctx.request_repaint();
        }
    }
}

impl Drop for Windows {
    /// The plugin stops: its windows close.
    fn drop(&mut self) {
        if let Some(t) = self.tx.lock().unwrap().take() {
            let _ = t.send(Msg::Quit);
            self.wake();
        }
    }
}

fn spawn(slot: Slot) -> io::Result<Sender<Msg>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new().name("viewer windows".into()).spawn(move || run(rx, slot))?;
    Ok(tx)
}

/// The thread: a loop of windows per run of files, until the plugin stops.
fn run(rx: Receiver<Msg>, slot: Slot) {
    let mut quit = false;
    while !quit {
        let Ok(Msg::Open(path, options)) = rx.recv() else { return };
        let doc = Doc::open(&path);
        let native = eframe::NativeOptions {
            viewport: window(&doc.title),
            // the program's thread has its own loop; this one is apart
            event_loop_builder: Some(Box::new(|b| {
                use winit::platform::windows::EventLoopBuilderExtWindows;
                b.with_any_thread(true);
            })),
            // the loop is kept for the next file
            run_and_return: true,
            ..Default::default()
        };
        let r = catch_unwind(AssertUnwindSafe(|| {
            eframe::run_native(
                "RustySalamander viewer",
                native,
                Box::new(|cc| {
                    *slot.lock().unwrap() = Some(cc.egui_ctx.clone());
                    let mut look = Look::default();
                    look.apply(&cc.egui_ctx, &options);
                    Ok(Box::new(App { wins: vec![Win::new(ViewportId::ROOT, doc)], rx: &rx, look, serial: 0, quit: &mut quit }))
                }),
            )
        }));
        *slot.lock().unwrap() = None;
        flush_messages();
        if r.is_err() {
            // a panic in a window: the next file starts afresh
            continue;
        }
    }
}

/// Handle the messages left for this thread's windows. A window that is
/// dropped when the loop ends is only asked to go away (a message); without
/// this it would stay on screen, frozen, until the next file.
fn flush_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
    let _ = catch_unwind(|| {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}

/// A viewer window's size and title.
fn window(title: &str) -> ViewportBuilder {
    ViewportBuilder::default().with_title(title).with_inner_size([900.0, 650.0])
}

/// How the program looks, which the windows follow.
#[derive(Default, PartialEq, Clone)]
struct Look {
    theme: String,
    ui_font: String,
    mono_font: String,
    solid_bars: bool,
    accent: Option<[u8; 3]>,
    fonts_done: bool,
}

impl Look {
    fn apply(&mut self, ctx: &egui::Context, options: &Values) {
        let get = |k: &str| options.get(k).cloned().unwrap_or_default();
        let (ui_font, mono_font) = (get("ui_font"), get("mono_font"));
        if !self.fonts_done || ui_font != self.ui_font || mono_font != self.mono_font {
            sal_ui::fonts::install(ctx, &ui_font, &mono_font);
            self.fonts_done = true;
        }
        (self.ui_font, self.mono_font) = (ui_font, mono_font);
        self.theme = get("theme");
        ctx.set_theme(match self.theme.as_str() {
            "light" => egui::ThemePreference::Light,
            "dark" => egui::ThemePreference::Dark,
            _ => egui::ThemePreference::System,
        });
        self.solid_bars = get("scrollbars") == "solid";
        sal_ui::apply_scrollbars(ctx, self.solid_bars);
        self.accent = sal_ui::parse_accent(&get("accent"));
        sal_ui::apply_accent(ctx, self.accent);
    }

    /// The theme of the windows' title bars.
    fn system_theme(&self) -> egui::SystemTheme {
        match self.theme.as_str() {
            "light" => egui::SystemTheme::Light,
            "dark" => egui::SystemTheme::Dark,
            _ => egui::SystemTheme::SystemDefault,
        }
    }
}

struct App<'a> {
    /// The windows; the first is the loop's main window.
    wins: Vec<Win>,
    rx: &'a Receiver<Msg>,
    look: Look,
    serial: u64,
    quit: &'a mut bool,
}

impl eframe::App for App<'_> {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Open(path, options) => {
                    let before = self.look.clone();
                    self.look.apply(ctx, &options);
                    if self.look != before {
                        self.wins.iter_mut().for_each(|w| w.themed = false);
                    }
                    self.serial += 1;
                    self.wins.push(Win::new(ViewportId::from_hash_of(("viewer", self.serial)), Doc::open(&path)));
                }
                Msg::Quit => {
                    *self.quit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                    return;
                }
            }
        }

        // the main window
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.wins.len() == 1 {
                return;
            }
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.wins[0].open = false;
        }
        let look = &self.look;
        self.wins[0].show(ctx, look);
        // the others
        for w in &mut self.wins[1..] {
            ctx.show_viewport_immediate(w.id, window(&w.doc.title), |ctx, _| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    w.open = false;
                }
                w.show(ctx, look);
            });
        }
        let before = self.wins.len();
        let others: Vec<Win> = self.wins.drain(1..).filter(|w| w.open).collect();
        self.wins.extend(others);
        // a window that is no longer shown goes away in the next frame
        if self.wins.len() != before || !self.wins[0].open {
            ctx.request_repaint();
        }
        if !self.wins[0].open {
            match self.wins.pop() {
                // the main window takes over the newest other one
                Some(next) if !self.wins.is_empty() => {
                    if let Some((pos, size)) = next.place {
                        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
                        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
                    }
                    ctx.send_viewport_cmd(ViewportCommand::Title(next.doc.title.clone()));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                    self.wins[0] = Win { id: ViewportId::ROOT, ..next };
                }
                _ => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
    }
}

/// One viewer window.
struct Win {
    id: ViewportId,
    doc: Doc,
    open: bool,
    /// Its title bar follows the theme.
    themed: bool,
    /// Where it is: the outer position and the inner size.
    place: Option<(egui::Pos2, egui::Vec2)>,
}

impl Win {
    fn new(id: ViewportId, doc: Doc) -> Win {
        Win { id, doc, open: true, themed: false, place: None }
    }

    fn show(&mut self, ctx: &egui::Context, look: &Look) {
        if !self.themed {
            ctx.send_viewport_cmd(ViewportCommand::SetTheme(look.system_theme()));
            self.themed = true;
        }
        if let Some((outer, inner)) = ctx.input(|i| Some((i.viewport().outer_rect?, i.viewport().inner_rect?))) {
            self.place = Some((outer.min, inner.size()));
        }
        let doc = &mut self.doc;
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint_of(ViewportId::ROOT)
        };
        let colors = doc.colors(ctx.style().visuals.dark_mode, wake);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::F10)) {
            self.open = false;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F4)) {
            doc.hex = !doc.hex;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::W) && i.modifiers.ctrl) && doc.can_wrap() {
            doc.wrap = !doc.wrap;
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
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(e) = &doc.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return;
            }
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
            let bars = sal_ui::scroll_bar_visibility(ui);
            if doc.hex {
                egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars).show_rows(ui, row_h, doc.hex_rows(), |ui, range| {
                    for r in range {
                        ui.add(egui::Label::new(RichText::new(doc.hex_line(r)).monospace()).extend());
                    }
                });
            } else if doc.wrap {
                egui::ScrollArea::vertical().auto_shrink(false).scroll_bar_visibility(bars).show(ui, |ui| {
                    for (i, l) in doc.lines.iter().enumerate() {
                        let mut job = line_job(ui, l, doc.line_spans(i));
                        job.wrap.max_width = ui.available_width();
                        ui.add(egui::Label::new(job).wrap());
                    }
                });
            } else {
                egui::ScrollArea::both().auto_shrink(false).scroll_bar_visibility(bars).show_rows(ui, row_h, doc.lines.len(), |ui, range| {
                    for i in range {
                        let job = line_job(ui, &doc.lines[i], doc.line_spans(i));
                        ui.add(egui::Label::new(job).extend());
                    }
                });
            }
        });
    }
}

/// A line of text in the monospace font, colored by `spans` when there are
/// any.
fn line_job(ui: &egui::Ui, text: &str, spans: Option<&[Span]>) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = TextFormat { font_id: font.clone(), color: ui.visuals().text_color(), ..Default::default() };
    let mut job = LayoutJob::default();
    if text.is_empty() {
        job.append(" ", 0.0, plain);
        return job;
    }
    let mut at = 0;
    for s in spans.unwrap_or_default() {
        let end = (at + s.len).min(text.len());
        let Some(part) = text.get(at..end) else { break };
        let [r, g, b] = s.rgb;
        let underline = if s.underline { egui::Stroke::new(1.0f32, egui::Color32::from_rgb(r, g, b)) } else { egui::Stroke::NONE };
        job.append(part, 0.0, TextFormat { font_id: font.clone(), color: egui::Color32::from_rgb(r, g, b), italics: s.italic, underline, ..Default::default() });
        at = end;
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, plain);
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
                let job = line_job(ui, "abcdeřest", Some(&[red, blue]));
                let parts: Vec<(&str, egui::Color32, bool)> = job.sections.iter().map(|s| (&job.text[s.byte_range.clone()], s.format.color, s.format.italics)).collect();
                assert_eq!(parts[0], ("abc", egui::Color32::from_rgb(200, 0, 0), true));
                assert_eq!(parts[1], ("de", egui::Color32::from_rgb(0, 0, 200), false));
                assert_eq!((parts[2].0, parts[2].1), ("řest", ui.visuals().text_color()));
                assert_eq!(line_job(ui, "", None).text, " ", "an empty line keeps its height");
                assert_eq!(line_job(ui, "plain", None).sections.len(), 1);
            });
        });
    }
}
