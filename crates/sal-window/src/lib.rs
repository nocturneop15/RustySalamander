//! Windows of a plugin's own (a viewer, a comparison), for plugins that show
//! files in windows instead of the program's panels.
//!
//! The program shows and draws them (interface 9, `abi::WindowFns`): the
//! plugin keeps what a window shows and runs its egui, which only says what
//! to draw; the program owns the window on screen, sends it its input and
//! draws the triangles it gets back with its own graphics. So a plugin
//! carries no graphics of its own, and its windows draw wherever the
//! program's window does.
//!
//! A window's content is made on a thread of its own (reading a file for it
//! doesn't hold up the program); its frames run on the program's
//! user-interface thread. The windows follow the program's look (`options`
//! of `FolderPlugin::open_file`): theme, fonts, scroll bars, accent colors
//! and animations.

pub mod wire;

use std::ffi::c_void;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use egui;
use egui::{ViewportCommand, ViewportId};
use sal_plugin_api::abi::{Bytes, BytesOut, HostApi, WindowFns, WINDOW_CLOSED, WINDOW_FAILED, WINDOW_OPEN};
use sal_plugin_api::Values;

/// What a window shows.
pub trait Content: Send {
    /// The window's title.
    fn title(&self) -> String;
    /// Draw the window's panels into `ctx`; false closes it. Escape and F10
    /// close it too.
    fn show(&mut self, ctx: &egui::Context) -> bool;
    /// Whether Escape is the content's now (a find bar to close) instead of
    /// closing the window.
    fn takes_escape(&self) -> bool {
        false
    }
}

/// Makes a window's content, on a thread of its own (where reading a file
/// for it doesn't hold up the program).
pub type Make = Box<dyn FnOnce() -> Box<dyn Content> + Send>;

/// A plugin's windows.
pub struct Windows {
    /// The windows' icon and size.
    icon: fn() -> Arc<egui::IconData>,
    size: [f32; 2],
    /// Set when the plugin stops: its windows close.
    stopped: Arc<AtomicBool>,
}

impl Windows {
    /// Windows with this icon (one of `sal_ui::icon`), opening at `size`.
    pub fn new(icon: fn() -> Arc<egui::IconData>, size: [f32; 2]) -> Windows {
        Windows { icon, size, stopped: Arc::new(AtomicBool::new(false)) }
    }

    /// Show what `make` makes in a new window, looking as `options` say.
    pub fn open(&self, make: Make, options: Values) -> io::Result<()> {
        let host = sal_plugin_api::export::host().filter(|h| h.version >= 9).ok_or_else(|| io::Error::other("this RustySalamander can't show the plugin's windows"))?;
        let (icon, size, stopped) = (self.icon, self.size, self.stopped.clone());
        std::thread::Builder::new().name("plugin window".into()).spawn(move || {
            let Ok(content) = catch_unwind(AssertUnwindSafe(make)) else { return };
            let win = Box::new(Window::new(content, &options, Some(host), stopped));
            let init = wire::Init { egui: wire::EGUI.into(), title: win.content.title(), icon: (*icon()).clone(), size };
            let init = wire::encode(&init);
            let win = Box::into_raw(win);
            // the program owns it now, or it couldn't take it
            if !unsafe { (host.open_window)(host.ctx, win as *mut c_void, &FNS, Bytes::of(&init)) } {
                drop(unsafe { Box::from_raw(win) });
            }
        })?;
        Ok(())
    }
}

impl Drop for Windows {
    /// The plugin stops: its windows close.
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(host) = sal_plugin_api::export::host().filter(|h| h.version >= 9) {
            unsafe { (host.repaint)(host.ctx, 0) };
        }
    }
}

/// The functions the program calls for every window.
static FNS: WindowFns = WindowFns { frame, free };

/// [`WindowFns::frame`].
unsafe extern "C" fn frame(win: *mut c_void, input: Bytes, out: *const BytesOut) -> i32 {
    let w = &mut *(win as *mut Window);
    let r = catch_unwind(AssertUnwindSafe(|| {
        let raw = wire::decode::<egui::RawInput>(input.get())?;
        Some(w.frame(raw))
    }));
    match r {
        Ok(Some((open, output))) => {
            (*out).send(&wire::encode(&output));
            if open {
                WINDOW_OPEN
            } else {
                WINDOW_CLOSED
            }
        }
        _ => WINDOW_FAILED,
    }
}

/// [`WindowFns::free`].
unsafe extern "C" fn free(win: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| drop(Box::from_raw(win as *mut Window))));
}

/// One window: its content and its egui.
struct Window {
    ctx: egui::Context,
    content: Box<dyn Content>,
    look: Look,
    /// Its title bar follows the theme.
    themed: bool,
    stopped: Arc<AtomicBool>,
}

impl Window {
    /// A window showing `content`; `host` is woken when it is to be drawn
    /// again (something in the background changed it).
    fn new(content: Box<dyn Content>, options: &Values, host: Option<&'static HostApi>, stopped: Arc<AtomicBool>) -> Window {
        let ctx = egui::Context::default();
        if let Some(host) = host {
            ctx.set_request_repaint_callback(move |info| {
                let ms = info.delay.as_millis().min(u64::MAX as u128) as u64;
                unsafe { (host.repaint)(host.ctx, ms) };
            });
        }
        let mut look = Look::default();
        look.apply(&ctx, options);
        Window { ctx, content, look, themed: false, stopped }
    }

    /// One frame: whether the window stays, and what to draw.
    fn frame(&mut self, raw: egui::RawInput) -> (bool, wire::Output) {
        let mut open = !self.stopped.load(Ordering::Acquire);
        let Window { ctx, content, look, themed, .. } = self;
        let full = ctx.run(raw, |ctx| {
            if !*themed {
                ctx.send_viewport_cmd(ViewportCommand::SetTheme(look.system_theme()));
                *themed = true;
            }
            let (closing, escape, f10) = ctx.input(|i| (i.viewport().close_requested(), i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::F10)));
            if closing || f10 || escape && !content.takes_escape() {
                open = false;
            }
            if !content.show(ctx) {
                open = false;
            }
        });
        let mut viewport = full.viewport_output.into_iter().find(|(id, _)| *id == ViewportId::ROOT).map(|(_, v)| v);
        let mut commands = viewport.as_mut().map(|v| std::mem::take(&mut v.commands)).unwrap_or_default();
        if commands.contains(&ViewportCommand::Close) {
            open = false;
            commands.retain(|c| *c != ViewportCommand::Close);
        }
        let delay = viewport.map_or(Duration::MAX, |v| v.repaint_delay);
        let meshes = ctx
            .tessellate(full.shapes, full.pixels_per_point)
            .into_iter()
            .filter_map(|p| match p.primitive {
                egui::epaint::Primitive::Mesh(m) => Some((p.clip_rect, m)),
                // a paint callback would need the plugin's own graphics
                egui::epaint::Primitive::Callback(_) => None,
            })
            .collect();
        let output = wire::Output {
            title: content.title(),
            textures: full.textures_delta,
            meshes,
            pixels_per_point: full.pixels_per_point,
            platform: full.platform_output,
            commands,
            repaint_after: (delay != Duration::MAX).then_some(delay.as_secs_f64()),
        };
        (open, output)
    }
}

/// How the program looks, which the windows follow.
#[derive(Default, PartialEq, Clone)]
struct Look {
    theme: String,
}

impl Look {
    /// Follow the program's look in `options`.
    fn apply(&mut self, ctx: &egui::Context, options: &Values) {
        let get = |k: &str| options.get(k).cloned().unwrap_or_default();
        sal_ui::fonts::install(ctx, &get("ui_font"), &get("mono_font"));
        self.theme = get("theme");
        ctx.set_theme(match self.theme.as_str() {
            "light" => egui::ThemePreference::Light,
            "dark" => egui::ThemePreference::Dark,
            _ => egui::ThemePreference::System,
        });
        sal_ui::apply_scrollbars(ctx, get("scrollbars") == "solid");
        // programs before 0.7.1 don't say: animated, as they were
        sal_ui::apply_animations(ctx, get("animations") != "off");
        // the program's spacing
        sal_ui::layout::apply(ctx);
        // programs before 0.5.9 send one color for both themes
        let one = |k: &str| if options.contains_key(k) { sal_ui::parse_accent(&get(k)) } else { sal_ui::parse_accent(&get("accent")) };
        sal_ui::apply_accent(ctx, one("accent_light"), one("accent_dark"));
    }

    /// The theme of the window's title bar.
    fn system_theme(&self) -> egui::SystemTheme {
        match self.theme.as_str() {
            "light" => egui::SystemTheme::Light,
            "dark" => egui::SystemTheme::Dark,
            _ => egui::SystemTheme::SystemDefault,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Hello(u32);
    impl Content for Hello {
        fn title(&self) -> String {
            format!("Hello {}", self.0)
        }
        fn show(&mut self, ctx: &egui::Context) -> bool {
            self.0 += 1;
            egui::CentralPanel::default().show(ctx, |ui| ui.label("Příliš žluťoučký kůň"));
            true
        }
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let mut raw = egui::RawInput { screen_rect: Some(rect), events, ..Default::default() };
        raw.viewports.insert(ViewportId::ROOT, egui::ViewportInfo { native_pixels_per_point: Some(1.25), ..Default::default() });
        raw
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    #[test]
    fn a_window_draws_and_closes() {
        let options: Values = [("theme".to_owned(), "dark".to_owned())].into();
        let stopped = Arc::new(AtomicBool::new(false));
        let mut w = Window::new(Box::new(Hello(0)), &options, None, stopped.clone());
        let (open, out) = w.frame(input(vec![]));
        assert!(open);
        assert_eq!((out.title.as_str(), out.pixels_per_point), ("Hello 1", 1.25));
        assert!(!out.meshes.is_empty(), "something to draw");
        assert!(!out.textures.set.is_empty(), "the font texture");
        assert!(out.commands.contains(&ViewportCommand::SetTheme(egui::SystemTheme::Dark)));
        // through the C functions, as the program calls them
        let raw = wire::encode(&input(vec![key(egui::Key::A)]));
        let mut bytes = Vec::new();
        let p = Box::into_raw(Box::new(w));
        assert_eq!(unsafe { frame(p as *mut c_void, Bytes::of(&raw), &BytesOut::new(&mut bytes)) }, WINDOW_OPEN);
        let out: wire::Output = wire::decode(&bytes).unwrap();
        assert_eq!(out.title, "Hello 2");
        assert_eq!(unsafe { frame(p as *mut c_void, Bytes::of(&[9, 9]), &BytesOut::new(&mut bytes)) }, WINDOW_FAILED);
        let mut w = unsafe { Box::from_raw(p) };
        // Escape, F10 and the close button close it, as does the plugin stopping
        for e in [vec![key(egui::Key::Escape)], vec![key(egui::Key::F10)]] {
            assert!(!w.frame(input(e)).0);
        }
        let mut closing = input(vec![]);
        closing.viewports.get_mut(&ViewportId::ROOT).unwrap().events.push(egui::ViewportEvent::Close);
        assert!(!w.frame(closing).0);
        assert!(w.frame(input(vec![])).0);
        stopped.store(true, Ordering::Release);
        assert!(!w.frame(input(vec![])).0);
    }
}
