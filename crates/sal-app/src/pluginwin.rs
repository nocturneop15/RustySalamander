//! The plugins' windows (a viewer, a comparison; interface 9,
//! `abi::WindowFns`), which the program shows and draws.
//!
//! A plugin keeps what its window shows and runs its egui, which only says
//! what to draw. The program owns the window on screen: each frame it sends
//! the plugin the window's input and draws the triangles and textures it
//! gets back, with a renderer of the window's own on the program's wgpu.
//! So the plugins carry no graphics, and their windows draw wherever the
//! program's own window does.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, ViewportBuilder, ViewportClass, ViewportId};
use eframe::egui_wgpu::{self, wgpu};
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
    /// The program's graphics; none in the tests.
    render: Option<egui_wgpu::RenderState>,
}

impl Windows {
    /// Draw with the program's graphics.
    pub fn set_render(&mut self, render: Option<egui_wgpu::RenderState>) {
        self.render = render;
    }

    /// Show the windows, and those opened since the last frame: over the
    /// program's window `owner`, with Windows' own animation if `animations`.
    pub fn show(&mut self, ctx: &egui::Context, owner: Option<isize>, animations: bool) {
        CTX.lock().unwrap().get_or_insert_with(|| ctx.clone());
        for o in std::mem::take(&mut *OPENED.lock().unwrap()) {
            self.serial += 1;
            let step = self.wins.len();
            let id = ViewportId::from_hash_of(("plugin window", self.serial));
            let wire::Init { title, icon, size, .. } = o.init;
            self.wins.push(Win { id, key: self.serial, win: o.win, fns: o.fns, title, icon: Arc::new(icon), size, step, settled: 0, textures: Vec::new(), out: Vec::new() });
        }
        let mut gone = Vec::new();
        for w in &mut self.wins {
            // made hidden, shown once Windows is told about its animation and place
            let builder = ViewportBuilder::default().with_title(&w.title).with_icon(w.icon.clone()).with_inner_size(w.size).with_visible(w.settled == SHOWN);
            ctx.show_viewport_immediate(w.id, builder, |vctx, class| {
                if !w.frame(vctx, class, owner, animations, self.render.as_ref()) {
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
        if let Some(render) = &self.render {
            if let Some(painters) = render.renderer.write().callback_resources.get_mut::<Painters>() {
                painters.retain(|k, _| !gone.contains(k));
            }
        }
        // the closed windows go away in the next frame
        ctx.request_repaint();
    }
}

/// One window of a plugin.
struct Win {
    id: ViewportId,
    /// Its renderer's key.
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
    /// Texture changes not drawn yet (a frame that wasn't drawn, the window
    /// minimized): sent again with the next frame, as their order needs.
    textures: Vec<(Arc<AtomicBool>, egui::TexturesDelta)>,
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
    fn frame(&mut self, vctx: &egui::Context, class: ViewportClass, owner: Option<isize>, animations: bool, render: Option<&egui_wgpu::RenderState>) -> bool {
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
        let primitives = out.primitives();
        let wire::Output { title, textures, pixels_per_point, platform, commands, repaint_after, .. } = out;
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
        let Some(render) = render else { return true };
        // the changes drawn already are done with
        self.textures.retain(|(drawn, _)| !drawn.load(Ordering::Acquire));
        if !textures.is_empty() {
            self.textures.push((Arc::new(AtomicBool::new(false)), textures));
        }
        let paint = Paint { key: self.key, format: render.target_format, pixels_per_point, textures: self.textures.clone(), primitives };
        vctx.layer_painter(egui::LayerId::background()).add(egui_wgpu::Callback::new_paint_callback(vctx.screen_rect(), paint));
        true
    }
}

/// The renderers of the plugins' windows, by key (kept in the program's
/// renderer, with the paint callbacks' resources).
type Painters = HashMap<u64, Painter>;

/// A window's renderer and the textures to free after it drew.
struct Painter {
    renderer: egui_wgpu::Renderer,
    free: Vec<egui::TextureId>,
}

/// What a frame of a plugin's window draws, with the window's renderer.
struct Paint {
    key: u64,
    format: wgpu::TextureFormat,
    pixels_per_point: f32,
    /// The texture changes not drawn yet, in order; each marked once done.
    textures: Vec<(Arc<AtomicBool>, egui::TexturesDelta)>,
    primitives: Vec<egui::ClippedPrimitive>,
}

impl egui_wgpu::CallbackTrait for Paint {
    fn prepare(&self, device: &wgpu::Device, queue: &wgpu::Queue, screen: &egui_wgpu::ScreenDescriptor, encoder: &mut wgpu::CommandEncoder, resources: &mut egui_wgpu::CallbackResources) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<Painters>().is_none() {
            resources.insert(Painters::new());
        }
        let painters = resources.get_mut::<Painters>().expect("just inserted");
        // the program's renderer: no depth, no multisampling, dithered (eframe's defaults)
        let p = painters.entry(self.key).or_insert_with(|| Painter { renderer: egui_wgpu::Renderer::new(device, self.format, None, 1, true), free: Vec::new() });
        for id in p.free.drain(..) {
            p.renderer.free_texture(&id);
        }
        for (drawn, delta) in &self.textures {
            for (id, image) in &delta.set {
                p.renderer.update_texture(device, queue, *id, image);
            }
            p.free.extend(delta.free.iter().copied());
            drawn.store(true, Ordering::Release);
        }
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: screen.size_in_pixels, pixels_per_point: self.pixels_per_point };
        p.renderer.update_buffers(device, queue, encoder, &self.primitives, &screen)
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        if let Some(p) = resources.get::<Painters>().and_then(|m| m.get(&self.key)) {
            let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: info.screen_size_px, pixels_per_point: self.pixels_per_point };
            p.renderer.render(pass, &self.primitives, &screen);
        }
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
            let mut w = Win { id: ViewportId::ROOT, key: 0, win: o.win, fns: o.fns, title: o.init.title.clone(), icon: Arc::new(o.init.icon), size: o.init.size, step: 0, settled: SHOWN, textures: Vec::new(), out: Vec::new() };
            let f = move |raw: egui::RawInput| {
                let input = wire::encode(&raw);
                let state = unsafe { (w.fns.frame)(w.win, Bytes::of(&input), &BytesOut::new(&mut w.out)) };
                (state == WINDOW_OPEN).then(|| wire::decode(&w.out)).flatten()
            };
            (o.init.title, Box::new(f) as TestWindow)
        })
        .collect()
}
