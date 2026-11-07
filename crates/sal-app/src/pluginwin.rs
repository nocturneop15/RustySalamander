//! The plugins' windows (a viewer, a comparison; interface 9,
//! `abi::WindowFns`), which the program shows and draws.
//!
//! A plugin keeps what its window shows and runs its egui, which only says
//! what to draw. The program owns the window on screen: each frame it sends
//! the plugin the window's input and gets back the triangles and textures,
//! which go into the window's egui frame as they are (the textures among
//! the program's own). So the plugins carry no graphics, and their windows
//! draw however the program's own window does.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, ViewportBuilder, ViewportClass, ViewportId};
use sal_plugin_api::abi::{Bytes, BytesOut, WindowFns, WINDOW_OPEN};
use sal_window::wire;

/// A window a plugin opened, not shown yet.
struct Opened {
    win: *mut c_void,
    fns: &'static WindowFns,
    init: wire::Init,
}

// The window is the plugin's, which runs it on the program's
// user-interface thread only.
unsafe impl Send for Opened {}

/// Windows the plugins opened since the last frame.
static OPENED: Mutex<Vec<Opened>> = Mutex::new(Vec::new());
/// The program's window, woken for the plugins' windows.
static CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

/// `HostApi::open_window`: a plugin's window, shown from the next frame.
pub unsafe extern "C" fn open_window(_: *mut c_void, win: *mut c_void, fns: *const WindowFns, init: Bytes) -> bool {
    let Some(init) = wire::decode::<wire::Init>(init.get()) else { return false };
    if win.is_null() || fns.is_null() || init.egui != wire::EGUI {
        crate::debuglog::write(format!("!!! ERROR a plugin's window can't be shown (egui {:?}, the program's {})", init.egui, wire::EGUI));
        return false;
    }
    crate::debuglog::write(format!("plugin window opens: {}", init.title));
    OPENED.lock().unwrap().push(Opened { win, fns: &*fns, init });
    wake(Duration::ZERO);
    true
}

/// `HostApi::repaint`: draw the plugins' windows again.
pub unsafe extern "C" fn repaint(_: *mut c_void, after_ms: u64) {
    wake(Duration::from_millis(after_ms));
}

/// Draw the program's window (and so the plugins' windows) after `delay`.
fn wake(delay: Duration) {
    if let Some(ctx) = &*CTX.lock().unwrap() {
        ctx.request_repaint_after(delay);
    }
}

/// The plugins' windows that are open.
#[derive(Default)]
pub struct Windows {
    wins: Vec<Win>,
    serial: u64,
}

impl Windows {
    /// Show the windows, and those opened since the last frame: over the
    /// program's window `owner`, with Windows' own animation if `animations`.
    pub fn show(&mut self, ctx: &egui::Context, owner: Option<isize>, animations: bool) {
        CTX.lock().unwrap().get_or_insert_with(|| ctx.clone());
        for o in std::mem::take(&mut *OPENED.lock().unwrap()) {
            self.serial += 1;
            let step = self.wins.len();
            let id = ViewportId::from_hash_of(("plugin window", self.serial));
            let wire::Init { title, icon, size, .. } = o.init;
            self.wins.push(Win { id, key: self.serial, win: o.win, fns: o.fns, title, icon: Arc::new(icon), size, step, settled: 0, textures: Textures::default(), out: Vec::new() });
        }
        let mut gone = Vec::new();
        for w in &mut self.wins {
            // made hidden, shown once Windows is told about its animation and place
            let builder = ViewportBuilder::default().with_title(&w.title).with_icon(w.icon.clone()).with_inner_size(w.size).with_visible(w.settled == SHOWN);
            ctx.show_viewport_immediate(w.id, builder, |vctx, class| {
                if !w.frame(vctx, class, owner, animations) {
                    gone.push(w.key);
                }
            });
        }
        if gone.is_empty() {
            return;
        }
        for w in self.wins.iter().filter(|w| gone.contains(&w.key)) {
            crate::debuglog::write(format!("plugin window closes: {}", w.title));
        }
        self.wins.retain(|w| !gone.contains(&w.key));
        // the closed windows go away in the next frame
        ctx.request_repaint();
    }
}

/// One window of a plugin.
struct Win {
    id: ViewportId,
    key: u64,
    win: *mut c_void,
    fns: &'static WindowFns,
    title: String,
    icon: Arc<egui::IconData>,
    size: [f32; 2],
    /// How many windows were open before it (each one a bit further down).
    step: usize,
    /// Frames it waited for its window to show it (`SHOWN` once shown).
    settled: u32,
    /// Its textures among the program's.
    textures: Textures,
    /// The last frame's output, reused.
    out: Vec<u8>,
}

/// [`Win::settled`] of a window shown.
const SHOWN: u32 = u32::MAX;

impl Drop for Win {
    /// The window is gone: the plugin frees it.
    fn drop(&mut self) {
        unsafe { (self.fns.free)(self.win) };
    }
}

impl Win {
    /// One frame of the window in `vctx`; false when it closes.
    fn frame(&mut self, vctx: &egui::Context, class: ViewportClass, owner: Option<isize>, animations: bool) -> bool {
        if class == ViewportClass::Embedded {
            // only one window here (no graphics): the plugin's windows can't show
            return false;
        }
        if self.settled != SHOWN {
            let found = sal_ui::native::thread_window(&self.title);
            if let Some(h) = found {
                sal_ui::native::set_transitions(h, animations);
                // over the program's window, on its monitor
                if let Some(owner) = owner {
                    sal_ui::native::place_over(h, owner, self.step);
                }
            }
            self.settled += 1;
            if found.is_some() || self.settled > 3 {
                // shown by the next frame's builder
                self.settled = SHOWN;
            }
            vctx.request_repaint();
        }
        // the plugin's egui has one window, the main one
        let mut raw = vctx.input(|i| i.raw.clone());
        let info = raw.viewports.remove(&raw.viewport_id).unwrap_or_default();
        raw.viewports.clear();
        raw.viewports.insert(ViewportId::ROOT, info);
        raw.viewport_id = ViewportId::ROOT;
        if crate::debuglog::full() {
            crate::debuglog::input(&raw.events, &self.title, false, &|_| String::new());
            if raw.viewports[&ViewportId::ROOT].close_requested() {
                crate::debuglog::user(format_args!("close the window  [{}]", self.title));
            }
        }
        let input = wire::encode(&raw);
        let state = unsafe { (self.fns.frame)(self.win, Bytes::of(&input), &BytesOut::new(&mut self.out)) };
        if state != WINDOW_OPEN {
            return false;
        }
        let Some(out) = wire::decode::<wire::Output>(&self.out) else { return false };
        let wire::Output { title, textures, meshes, platform, commands, repaint_after, .. } = out;
        self.title = title;
        for c in commands {
            vctx.send_viewport_cmd(c);
        }
        vctx.output_mut(|o| {
            o.cursor_icon = platform.cursor_icon;
            if !platform.copied_text.is_empty() {
                o.copied_text = platform.copied_text;
            }
            if platform.open_url.is_some() {
                o.open_url = platform.open_url;
            }
            o.ime = platform.ime;
            o.mutable_text_under_cursor = platform.mutable_text_under_cursor;
        });
        if let Some(s) = repaint_after {
            vctx.request_repaint_after(Duration::from_secs_f64(s.min(3600.0)));
        }
        // the triangles as they are, on the plugin's textures
        self.textures.set(vctx, &textures.set);
        let painter = vctx.layer_painter(egui::LayerId::background());
        for (clip, mut mesh) in meshes {
            mesh.texture_id = self.textures.ours(mesh.texture_id);
            painter.with_clip_rect(clip).add(egui::Shape::mesh(mesh));
        }
        self.textures.free(&textures.free);
        true
    }
}

/// A window's textures among the program's: the plugin's texture ids and
/// the program's for them.
#[derive(Default)]
struct Textures {
    ctx: Option<egui::Context>,
    ids: HashMap<egui::TextureId, egui::TextureId>,
}

impl Textures {
    /// Make or change the textures.
    fn set(&mut self, ctx: &egui::Context, set: &[(egui::TextureId, egui::epaint::ImageDelta)]) {
        if set.is_empty() {
            return;
        }
        let tm = ctx.tex_manager();
        let mut tm = tm.write();
        self.ctx.get_or_insert_with(|| ctx.clone());
        for (id, delta) in set {
            match (self.ids.get(id), delta.pos) {
                (Some(ours), _) => tm.set(*ours, delta.clone()),
                (None, None) => {
                    let ours = tm.alloc(format!("plugin window {id:?}"), delta.image.clone(), delta.options);
                    self.ids.insert(*id, ours);
                }
                // a part of a texture it never made
                (None, Some(_)) => {}
            }
        }
    }

    /// The program's id of the plugin's texture `id`.
    fn ours(&self, id: egui::TextureId) -> egui::TextureId {
        self.ids.get(&id).copied().unwrap_or(id)
    }

    /// Drop textures the plugin is done with (after this frame is drawn).
    fn free(&mut self, ids: &[egui::TextureId]) {
        let Some(ctx) = &self.ctx else { return };
        let tm = ctx.tex_manager();
        let mut tm = tm.write();
        for id in ids {
            if let Some(ours) = self.ids.remove(id) {
                tm.free(ours);
            }
        }
    }
}

impl Drop for Textures {
    fn drop(&mut self) {
        let ids: Vec<_> = self.ids.keys().copied().collect();
        self.free(&ids);
    }
}

/// A plugin's window in the tests: a frame of it, `None` once it closed.
#[cfg(test)]
pub type TestWindow = Box<dyn FnMut(egui::RawInput) -> Option<wire::Output>>;

/// The windows the plugins opened and the program hasn't shown yet, taken
/// (for the tests, which run the windows' frames themselves).
#[cfg(test)]
pub fn take_opened() -> Vec<(String, TestWindow)> {
    std::mem::take(&mut *OPENED.lock().unwrap())
        .into_iter()
        .map(|o| {
            let mut w = Win { id: ViewportId::ROOT, key: 0, win: o.win, fns: o.fns, title: o.init.title.clone(), icon: Arc::new(o.init.icon), size: o.init.size, step: 0, settled: SHOWN, textures: Textures::default(), out: Vec::new() };
            let f = move |raw: egui::RawInput| {
                let input = wire::encode(&raw);
                let state = unsafe { (w.fns.frame)(w.win, Bytes::of(&input), &BytesOut::new(&mut w.out)) };
                (state == WINDOW_OPEN).then(|| wire::decode(&w.out)).flatten()
            };
            (o.init.title, Box::new(f) as TestWindow)
        })
        .collect()
}
