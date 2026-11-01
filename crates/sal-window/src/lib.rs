//! Windows of a plugin's own (a viewer, a comparison), for plugins that show
//! files in windows instead of the program's panels.
//!
//! They run on a thread of the plugin's own, with an event loop of their own
//! (egui and its window library are inside the plugin's DLL, apart from the
//! program's), kept for the whole session: the loop runs while a window is
//! open and waits for the next one when the last one closes.
//!
//! The first window is the loop's main window; more open beside it. When the
//! main window is closed while others are open, it takes over the newest of
//! them (its content and its place), so every window closes on its own. The
//! windows follow the program's look (`options` of
//! `FolderPlugin::open_file`): theme, fonts, scroll bars, accent colors and animations.

use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

pub use eframe::egui;
use eframe::egui::{ViewportBuilder, ViewportCommand, ViewportId};
use sal_plugin_api::Values;

/// What a window shows.
pub trait Content {
    /// The window's title.
    fn title(&self) -> String;
    /// Draw the window's panels into `ctx`; false closes it. Escape and F10
    /// close it too.
    fn show(&mut self, ctx: &egui::Context) -> bool;
}

/// Makes a window's content, on the windows' thread (where reading a file
/// for it doesn't hold up the program).
pub type Make = Box<dyn FnOnce() -> Box<dyn Content> + Send>;

enum Msg {
    Open(Make, Values),
    Quit,
}

type Slot = Arc<Mutex<Option<egui::Context>>>;

/// A plugin's windows: a thread started with the first one.
pub struct Windows {
    tx: Mutex<Option<Sender<Msg>>>,
    /// The windows' context while the loop runs, to wake it for a new window.
    ctx: Slot,
    /// The windows' icon and size.
    icon: fn() -> Arc<egui::IconData>,
    size: [f32; 2],
}

impl Windows {
    /// Windows with this icon (one of `sal_ui::icon`), opening at `size`.
    pub fn new(icon: fn() -> Arc<egui::IconData>, size: [f32; 2]) -> Windows {
        Windows { tx: Mutex::new(None), ctx: Arc::new(Mutex::new(None)), icon, size }
    }

    /// Show what `make` makes in a new window, looking as `options` say.
    pub fn open(&self, make: Make, options: Values) -> io::Result<()> {
        let mut tx = self.tx.lock().unwrap();
        let mut msg = Msg::Open(make, options);
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
        let t = spawn(self.ctx.clone(), Frame { icon: self.icon, size: self.size })?;
        t.send(msg).map_err(|_| io::Error::other("the windows' thread ended"))?;
        *tx = Some(t);
        Ok(())
    }

    /// Wake the windows' loop to handle a new window.
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

/// What every window of the thread has: its icon and first size.
#[derive(Clone, Copy)]
struct Frame {
    icon: fn() -> Arc<egui::IconData>,
    size: [f32; 2],
}

impl Frame {
    /// A window's title, icon and size.
    fn builder(self, title: &str) -> ViewportBuilder {
        ViewportBuilder::default().with_title(title).with_icon((self.icon)()).with_inner_size(self.size)
    }
}

/// Start the windows' thread; the sender of its windows.
fn spawn(slot: Slot, frame: Frame) -> io::Result<Sender<Msg>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new().name("plugin windows".into()).spawn(move || run(rx, slot, frame))?;
    Ok(tx)
}

/// The thread: a loop of windows per run of them, until the plugin stops.
fn run(rx: Receiver<Msg>, slot: Slot, frame: Frame) {
    let mut quit = false;
    while !quit {
        let Ok(Msg::Open(make, options)) = rx.recv() else { return };
        let r = catch_unwind(AssertUnwindSafe(|| {
            let content = make();
            let native = eframe::NativeOptions {
                viewport: frame.builder(&content.title()),
                // the program's thread has its own loop; this one is apart
                event_loop_builder: Some(Box::new(|b| {
                    use winit::platform::windows::EventLoopBuilderExtWindows;
                    b.with_any_thread(true);
                })),
                // the loop is kept for the next window
                run_and_return: true,
                ..Default::default()
            };
            eframe::run_native(
                "RustySalamander plugin",
                native,
                Box::new(|cc| {
                    *slot.lock().unwrap() = Some(cc.egui_ctx.clone());
                    let mut look = Look::default();
                    look.apply(&cc.egui_ctx, &options);
                    Ok(Box::new(App { wins: vec![Win::new(ViewportId::ROOT, content)], rx: &rx, look, frame, serial: 0, quit: &mut quit }))
                }),
            )
        }));
        *slot.lock().unwrap() = None;
        flush_messages();
        if r.is_err() {
            // a panic in a window: the next one starts afresh
            continue;
        }
    }
}

/// Handle the messages left for this thread's windows. A window that is
/// dropped when the loop ends is only asked to go away (a message); without
/// this it would stay on screen, frozen, until the next window.
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

/// How the program looks, which the windows follow.
#[derive(Default, PartialEq, Clone)]
struct Look {
    theme: String,
    ui_font: String,
    mono_font: String,
    solid_bars: bool,
    /// The accent colors of the light and the dark theme.
    accent: (Option<[u8; 3]>, Option<[u8; 3]>),
    fonts_done: bool,
}

impl Look {
    /// Follow the program's look in `options` (fonts only when they changed).
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
        // programs before 0.7.1 don't say: animated, as they were
        sal_ui::apply_animations(ctx, get("animations") != "off");
        // programs before 0.5.9 send one color for both themes
        let one = |k: &str| if options.contains_key(k) { sal_ui::parse_accent(&get(k)) } else { sal_ui::parse_accent(&get("accent")) };
        self.accent = (one("accent_light"), one("accent_dark"));
        sal_ui::apply_accent(ctx, self.accent.0, self.accent.1);
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
    frame: Frame,
    serial: u64,
    quit: &'a mut bool,
}

impl eframe::App for App<'_> {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Open(make, options) => {
                    let before = self.look.clone();
                    self.look.apply(ctx, &options);
                    if self.look != before {
                        self.wins.iter_mut().for_each(|w| w.themed = false);
                    }
                    self.serial += 1;
                    self.wins.push(Win::new(ViewportId::from_hash_of(("plugin window", self.serial)), make()));
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
        let frame = self.frame;
        for w in &mut self.wins[1..] {
            ctx.show_viewport_immediate(w.id, frame.builder(&w.content.title()), |ctx, _| {
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
                    ctx.send_viewport_cmd(ViewportCommand::Title(next.content.title()));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                    self.wins[0] = Win { id: ViewportId::ROOT, ..next };
                }
                _ => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
    }
}

/// One window.
struct Win {
    id: ViewportId,
    content: Box<dyn Content>,
    open: bool,
    /// Its title bar follows the theme.
    themed: bool,
    /// Where it is: the outer position and the inner size.
    place: Option<(egui::Pos2, egui::Vec2)>,
}

impl Win {
    /// A window for `content`.
    fn new(id: ViewportId, content: Box<dyn Content>) -> Win {
        Win { id, content, open: true, themed: false, place: None }
    }

    /// Draw the window; Escape and F10 close it.
    fn show(&mut self, ctx: &egui::Context, look: &Look) {
        if !self.themed {
            ctx.send_viewport_cmd(ViewportCommand::SetTheme(look.system_theme()));
            self.themed = true;
        }
        if let Some((outer, inner)) = ctx.input(|i| Some((i.viewport().outer_rect?, i.viewport().inner_rect?))) {
            self.place = Some((outer.min, inner.size()));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::F10)) {
            self.open = false;
        }
        if !self.content.show(ctx) {
            self.open = false;
        }
    }
}
