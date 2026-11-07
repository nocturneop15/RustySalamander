//! The program's windows without a graphics card (`SAL_RENDER=gdi`): egui
//! draws them as always, `sal-soft` turns its triangles into pixels on the
//! processor, and GDI puts on screen only the parts that changed since the
//! last frame. Over Remote Desktop that is what is sent: a cursor moving a
//! row is a strip or two of the window, not the whole window each frame.
//!
//! In place of eframe, this runs the windows itself, as eframe does
//! (winit windows, egui-winit for their input and output): the main window,
//! and the windows egui shows from within its frames (the dialogs, the
//! plugins' windows; immediate viewports), drawn as part of the main
//! window's frame.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, ImmediateViewport, ViewportBuilder, ViewportCommand, ViewportId, ViewportIdPair, ViewportInfo, ViewportOutput};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use sal_soft::{Area, Frame, Painter};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::app::App;

/// Whether the windows are drawn this way rather than with Direct3D 12
/// (wgpu), decided once at start ([`choice`]).
pub fn chosen() -> bool {
    choice().0
}

/// GDI or not, and why: `SAL_RENDER=gdi` or `wgpu` chooses; otherwise GDI
/// in a Remote Desktop session (where only what changed is sent, much
/// quicker than Direct3D's whole frames) and where Windows has no
/// Direct3D 12 (Windows 8.1, Server 2012 R2).
pub fn choice() -> (bool, &'static str) {
    static CHOICE: std::sync::OnceLock<(bool, &'static str)> = std::sync::OnceLock::new();
    *CHOICE.get_or_init(|| match std::env::var("SAL_RENDER").map(|v| v.to_ascii_lowercase()).as_deref() {
        Ok("gdi") => (true, "SAL_RENDER=gdi"),
        Ok("wgpu") => (false, "SAL_RENDER=wgpu"),
        _ if crate::remote_session() => (true, "Remote Desktop"),
        _ if !has_d3d12() => (true, "no Direct3D 12 in this Windows"),
        _ => (false, ""),
    })
}

/// Whether Windows has Direct3D 12 (10 and later).
fn has_d3d12() -> bool {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::path::Path::new(&root).join("System32").join("d3d12.dll").exists()
}

/// A frame of a window to draw, asked for by egui (from any thread).
#[derive(Debug)]
struct Repaint {
    viewport: ViewportId,
    when: Instant,
    pass: u64,
}

/// Run the program in a main window made as `builder` says; returns when
/// it closes.
pub fn run(builder: ViewportBuilder, args: Vec<String>) -> Result<(), String> {
    let event_loop = EventLoop::<Repaint>::with_user_event().build().map_err(|e| e.to_string())?;
    let ctx = egui::Context::default();
    // the windows egui shows are windows of their own
    ctx.set_embed_viewports(false);
    let proxy = Mutex::new(event_loop.create_proxy());
    ctx.set_request_repaint_callback(move |info| {
        let _ = proxy.lock().unwrap().send_event(Repaint { viewport: info.viewport_id, when: Instant::now() + info.delay, pass: info.current_cumulative_pass_nr });
    });
    let painter = Painter::new();
    crate::debuglog::write(format!("startup: graphics GDI, drawn on the processor ({} threads)", painter.threads()));
    let shared = Rc::new(RefCell::new(Shared { ctx: ctx.clone(), painter, viewports: HashMap::new(), by_window: HashMap::new(), start: Instant::now() }));
    let weak = Rc::downgrade(&shared);
    egui::Context::set_immediate_viewport_renderer(move |_ctx, immediate| {
        if let Some(shared) = weak.upgrade() {
            show_immediate(&shared, immediate);
        }
    });
    let mut gdi = Gdi { shared, app: None, args: Some(args), builder: builder.with_visible(false), repaint_at: HashMap::new(), drew: None, close: false, _proxy: event_loop.create_proxy() };
    let r = event_loop.run_app(&mut gdi).map_err(|e| e.to_string());
    // the program ends with its windows
    drop(gdi.app.take());
    r
}

/// What the main window's frames and the windows within them share.
struct Shared {
    ctx: egui::Context,
    painter: Painter,
    viewports: HashMap<ViewportId, Viewport>,
    by_window: HashMap<WindowId, ViewportId>,
    start: Instant,
}

/// A window: egui's viewport and what shows it.
struct Viewport {
    ids: ViewportIdPair,
    builder: ViewportBuilder,
    /// Commands for the window, once it is there.
    commands: Vec<ViewportCommand>,
    info: ViewportInfo,
    actions: egui::ahash::HashSet<egui_winit::ActionRequested>,
    window: Option<Arc<Window>>,
    state: Option<egui_winit::State>,
    screen: Option<Screen>,
    /// Shown, after its first frame was drawn (the main window).
    shown: bool,
}

impl Viewport {
    fn new(ids: ViewportIdPair, builder: ViewportBuilder) -> Viewport {
        Viewport { ids, builder, commands: Vec::new(), info: ViewportInfo::default(), actions: Default::default(), window: None, state: None, screen: None, shown: false }
    }

    /// Make its window, if it has none.
    fn make_window(&mut self, event_loop: &ActiveEventLoop, ctx: &egui::Context, by_window: &mut HashMap<WindowId, ViewportId>) {
        if self.window.is_some() {
            return;
        }
        match egui_winit::create_window(ctx, event_loop, &self.builder) {
            Ok(window) => {
                let window = Arc::new(window);
                by_window.insert(window.id(), self.ids.this);
                self.state = Some(egui_winit::State::new(ctx.clone(), self.ids.this, event_loop, Some(window.scale_factor() as f32), event_loop.system_theme(), None));
                egui_winit::update_viewport_info(&mut self.info, ctx, &window, true);
                self.screen = hwnd(&window).map(Screen::new);
                self.window = Some(window);
            }
            Err(e) => crate::debuglog::error(format!("a window could not be made: {e}")),
        }
    }

    /// The input for its next frame.
    fn input(&mut self, ctx: &egui::Context, start: Instant) -> Option<egui::RawInput> {
        let (window, state) = (self.window.as_ref()?, self.state.as_mut()?);
        egui_winit::update_viewport_info(&mut self.info, ctx, window, false);
        let mut raw = state.take_egui_input(window);
        raw.time = Some(start.elapsed().as_secs_f64());
        Some(raw)
    }
}

/// The main window's loop.
struct Gdi {
    shared: Rc<RefCell<Shared>>,
    app: Option<App>,
    args: Option<Vec<String>>,
    builder: ViewportBuilder,
    /// When windows are to be drawn again.
    repaint_at: HashMap<ViewportId, Instant>,
    /// How long the last frame took to make and draw, in seconds.
    drew: Option<f32>,
    close: bool,
    _proxy: EventLoopProxy<Repaint>,
}

thread_local! {
    /// The event loop, while it runs an event (windows are made only then).
    static EVENT_LOOP: Cell<Option<*const ActiveEventLoop>> = const { Cell::new(None) };
}

/// Run `f` with the event loop at hand.
fn in_loop<R>(event_loop: &ActiveEventLoop, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<*const ActiveEventLoop>);
    impl Drop for Reset {
        fn drop(&mut self) {
            EVENT_LOOP.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(EVENT_LOOP.with(|c| c.replace(Some(event_loop as *const _))));
    f()
}

/// The event loop running an event, if any.
fn with_loop<R>(f: impl FnOnce(&ActiveEventLoop) -> R) -> Option<R> {
    // SAFETY: set only for the time `in_loop` runs, from a live reference
    EVENT_LOOP.with(|c| c.get()).map(|p| f(unsafe { &*p }))
}

fn hwnd(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}

impl ApplicationHandler<Repaint> for Gdi {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        in_loop(event_loop, || {
            let mut sh = self.shared.borrow_mut();
            if sh.viewports.contains_key(&ViewportId::ROOT) {
                return;
            }
            let Shared { ctx, viewports, by_window, .. } = &mut *sh;
            let mut root = Viewport::new(ViewportIdPair::ROOT, self.builder.clone());
            root.make_window(event_loop, ctx, by_window);
            viewports.insert(ViewportId::ROOT, root);
            let ctx = ctx.clone();
            drop(sh);
            if let Some(args) = self.args.take() {
                self.app = Some(App::started(&ctx, args));
            }
            self.repaint_at.insert(ViewportId::ROOT, Instant::now());
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, r: Repaint) {
        let pass = self.shared.borrow().ctx.cumulative_pass_nr_for(r.viewport);
        // an older request: drawn since
        if pass == r.pass || pass == r.pass + 1 {
            self.schedule(r.viewport, r.when);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        in_loop(event_loop, || {
            let Some(vid) = self.shared.borrow().by_window.get(&window_id).copied() else { return };
            match &event {
                // Windows wants the window's pixels again (uncovered): as they were
                WindowEvent::RedrawRequested => {
                    if let Some(s) = self.shared.borrow_mut().viewports.get_mut(&vid).and_then(|v| v.screen.as_mut()) {
                        s.show_all();
                    }
                    return;
                }
                WindowEvent::Resized(size) if vid == ViewportId::ROOT && size.width > 0 => {
                    crate::debuglog::write(format!("··· window   {}×{}", size.width, size.height));
                }
                _ => {}
            }
            let repaint = {
                let mut sh = self.shared.borrow_mut();
                let Some(v) = sh.viewports.get_mut(&vid) else { return };
                if matches!(event, WindowEvent::CloseRequested) {
                    // its frame (or its parent's) closes it, or not
                    v.info.events.push(egui::ViewportEvent::Close);
                }
                let (Some(window), Some(state)) = (v.window.clone(), v.state.as_mut()) else { return };
                state.on_window_event(&window, &event).repaint
            };
            let resized = matches!(event, WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. });
            if resized {
                // drawn at once, while the size changes
                self.paint(event_loop);
            } else if repaint || matches!(event, WindowEvent::CloseRequested) {
                self.schedule(vid, Instant::now());
            }
        });
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        in_loop(event_loop, || {
            let now = Instant::now();
            if self.repaint_at.values().any(|t| *t <= now) {
                self.repaint_at.retain(|_, t| *t > now);
                self.paint(event_loop);
            }
            if self.close {
                event_loop.exit();
                return;
            }
            match self.repaint_at.values().min() {
                Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(*t)),
                None => event_loop.set_control_flow(ControlFlow::Wait),
            }
        });
    }
}

impl Gdi {
    /// Draw `vid` at `when`: the windows within the main window's frame
    /// are drawn with it.
    fn schedule(&mut self, vid: ViewportId, when: Instant) {
        let vid = if self.shared.borrow().viewports.contains_key(&vid) { ViewportId::ROOT } else { return };
        let t = self.repaint_at.entry(vid).or_insert(when);
        *t = (*t).min(when);
    }

    /// A frame of the main window (and of the windows shown within it).
    fn paint(&mut self, event_loop: &ActiveEventLoop) {
        let Some(app) = self.app.as_mut() else { return };
        self.repaint_at.remove(&ViewportId::ROOT);
        let started = Instant::now();
        let (ctx, mut raw, hwnd) = {
            let mut sh = self.shared.borrow_mut();
            let Shared { ctx, viewports, start, .. } = &mut *sh;
            let ctx = ctx.clone();
            let Some(root) = viewports.get_mut(&ViewportId::ROOT) else { return };
            let hwnd = root.window.as_deref().and_then(hwnd);
            let Some(mut raw) = root.input(&ctx, *start) else { return };
            raw.viewports = viewports.iter().map(|(id, v)| (*id, v.info.clone())).collect();
            (ctx, raw, hwnd)
        };
        app.input_hook(&ctx, &mut raw);
        let closing = raw.viewport().close_requested();
        // the program's frame: it may show windows of its own (no borrows held)
        let drew = self.drew;
        let full = ctx.run(raw, |ctx| app.update_frame(ctx, hwnd, drew));
        if closing && !full.viewport_output.get(&ViewportId::ROOT).is_some_and(|v| v.commands.contains(&ViewportCommand::CancelClose)) {
            self.close = true;
        }
        let mut sh = self.shared.borrow_mut();
        let sh = &mut *sh;
        sh.viewports.retain(|id, _| full.viewport_output.contains_key(id) || *id == ViewportId::ROOT);
        sh.by_window.retain(|_, id| sh.viewports.contains_key(id));
        if let Some(root) = sh.viewports.get_mut(&ViewportId::ROOT) {
            root.info.events.clear();
            if let (Some(window), Some(state)) = (&root.window, root.state.as_mut()) {
                state.handle_platform_output(window, full.platform_output);
            }
        }
        draw(sh, ViewportId::ROOT, full.shapes, full.pixels_per_point, &full.textures_delta);
        take_actions(sh, ViewportId::ROOT);
        viewport_output(sh, &full.viewport_output);
        self.drew = Some(started.elapsed().as_secs_f32());
        if self.close {
            event_loop.exit();
        }
    }
}

/// A window egui shows from within a frame (a dialog, a plugin's window):
/// its frame, drawn now.
fn show_immediate(shared: &RefCell<Shared>, immediate: ImmediateViewport<'_>) {
    let ImmediateViewport { ids, builder, mut viewport_ui_cb } = immediate;
    let (ctx, raw) = {
        let mut sh = shared.borrow_mut();
        let Shared { ctx, viewports, by_window, start, .. } = &mut *sh;
        let ctx = ctx.clone();
        let v = update_viewport(viewports, ids, builder);
        if v.window.is_none() {
            with_loop(|event_loop| v.make_window(event_loop, &ctx, by_window));
        }
        let Some(mut raw) = v.input(&ctx, *start) else { return };
        raw.viewports = viewports.iter().map(|(id, v)| (*id, v.info.clone())).collect();
        (ctx, raw)
    };
    let full = ctx.run(raw, |ctx| viewport_ui_cb(ctx));
    let mut sh = shared.borrow_mut();
    let sh = &mut *sh;
    if let Some(v) = sh.viewports.get_mut(&ids.this) {
        v.info.events.clear();
        if let (Some(window), Some(state)) = (&v.window, v.state.as_mut()) {
            state.handle_platform_output(window, full.platform_output);
        }
    }
    draw(sh, ids.this, full.shapes, full.pixels_per_point, &full.textures_delta);
    take_actions(sh, ids.this);
    viewport_output(sh, &full.viewport_output);
}

/// The viewport `ids`, made or changed as `builder` says.
fn update_viewport(viewports: &mut HashMap<ViewportId, Viewport>, ids: ViewportIdPair, mut builder: ViewportBuilder) -> &mut Viewport {
    if builder.icon.is_none() {
        builder.icon = viewports.get(&ids.parent).and_then(|v| v.builder.icon.clone());
    }
    match viewports.entry(ids.this) {
        std::collections::hash_map::Entry::Vacant(e) => e.insert(Viewport::new(ids, builder)),
        std::collections::hash_map::Entry::Occupied(e) => {
            let v = e.into_mut();
            v.ids.parent = ids.parent;
            let (mut commands, recreate) = v.builder.patch(builder);
            if recreate {
                v.window = None;
                v.state = None;
                v.screen = None;
                v.shown = false;
            }
            v.commands.append(&mut commands);
            v
        }
    }
}

/// What a frame said about the windows: their builders and commands.
fn viewport_output(sh: &mut Shared, output: &egui::ViewportIdMap<ViewportOutput>) {
    for (id, out) in output.clone() {
        // only immediate viewports, which made their windows as they were shown
        let ViewportOutput { parent, builder, mut commands, .. } = out;
        let v = update_viewport(&mut sh.viewports, ViewportIdPair::from_self_and_parent(id, parent), builder);
        if let Some(window) = &v.window {
            v.commands.append(&mut commands);
            egui_winit::process_viewport_commands(&sh.ctx, &mut v.info, std::mem::take(&mut v.commands), window, &mut v.actions);
        }
    }
    sh.viewports.retain(|id, _| output.contains_key(id) || *id == ViewportId::ROOT);
    let viewports = &sh.viewports;
    sh.by_window.retain(|_, id| viewports.contains_key(id));
}

/// Cut, copy and paste the window was asked for, as input to its next frame.
fn take_actions(sh: &mut Shared, vid: ViewportId) {
    let Some(v) = sh.viewports.get_mut(&vid) else { return };
    let Some(state) = v.state.as_mut() else { return };
    for action in v.actions.drain() {
        let ev = match action {
            egui_winit::ActionRequested::Cut => egui::Event::Cut,
            egui_winit::ActionRequested::Copy => egui::Event::Copy,
            egui_winit::ActionRequested::Paste => match state.clipboard_text().map(|t| t.replace("\r\n", "\n")) {
                Some(t) if !t.is_empty() => egui::Event::Paste(t),
                _ => continue,
            },
            egui_winit::ActionRequested::Screenshot => continue,
        };
        state.egui_input_mut().events.push(ev);
    }
}

/// Draw a frame of the window `vid` and show what changed.
fn draw(sh: &mut Shared, vid: ViewportId, shapes: Vec<egui::epaint::ClippedShape>, ppp: f32, textures: &egui::TexturesDelta) {
    sh.painter.set_textures(textures);
    let clear = sh.ctx.style().visuals.panel_fill;
    if let Some(v) = sh.viewports.get_mut(&vid) {
        if let (Some(window), Some(screen)) = (&v.window, v.screen.as_mut()) {
            let size = window.inner_size();
            if size.width > 0 && size.height > 0 {
                let started = Instant::now();
                let prims = sh.ctx.tessellate(shapes, ppp);
                screen.frame.resize(size.width as usize, size.height as usize);
                sh.painter.paint(&mut screen.frame, &prims, ppp, clear, None);
                let drawn = started.elapsed();
                let sent = screen.show_changes();
                if crate::debuglog::full() && sent.0 > 0 {
                    crate::debuglog::write(format!(
                        "··· gdi      drawn in {}, shown in {}: {} parts, {:.1} % of {}×{}",
                        crate::debuglog::ms(drawn),
                        crate::debuglog::ms(started.elapsed() - drawn),
                        sent.0,
                        sent.1 as f64 * 100.0 / (size.width as f64 * size.height as f64),
                        size.width,
                        size.height
                    ));
                }
                if !v.shown {
                    v.shown = true;
                    if vid == ViewportId::ROOT {
                        window.set_visible(true);
                    }
                }
            }
        }
    }
    sh.painter.free_textures(textures);
}

/// A window's pixels: the frame drawn, and a copy in a GDI bitmap of what
/// is on screen.
struct Screen {
    hwnd: isize,
    frame: Frame,
    bitmap: Option<Bitmap>,
}

impl Screen {
    fn new(hwnd: isize) -> Screen {
        Screen { hwnd, frame: Frame::default(), bitmap: None }
    }

    /// Put on screen what changed in the frame; the parts and pixels shown.
    fn show_changes(&mut self) -> (usize, usize) {
        let (w, h) = (self.frame.width, self.frame.height);
        let fresh = self.bitmap.as_ref().is_none_or(|b| (b.width, b.height) != (w, h));
        if fresh {
            self.bitmap = Bitmap::new(w, h);
        }
        let Some(b) = self.bitmap.as_mut() else { return (0, 0) };
        let bits = b.pixels();
        let parts = if fresh {
            bits.copy_from_slice(&self.frame.pixels);
            vec![Area { x0: 0, y0: 0, x1: w as i32, y1: h as i32 }]
        } else {
            changes(&self.frame.pixels, bits, w)
        };
        let shown = parts.iter().map(|a| a.pixels()).sum();
        b.blit(self.hwnd, &parts);
        (parts.len(), shown)
    }

    /// Put the whole window on screen again.
    fn show_all(&mut self) {
        if let Some(b) = &self.bitmap {
            b.blit(self.hwnd, &[Area { x0: 0, y0: 0, x1: b.width as i32, y1: b.height as i32 }]);
        }
    }
}

/// Rows compared together, a band.
const BAND: usize = 8;

/// Copy what differs from `new` into `old` (both `width` wide); the parts
/// that differed, bands of rows merged where they line up.
fn changes(new: &[u32], old: &mut [u32], width: usize) -> Vec<Area> {
    let height = new.len() / width.max(1);
    let mut parts: Vec<Area> = Vec::new();
    for y0 in (0..height).step_by(BAND) {
        let y1 = (y0 + BAND).min(height);
        let (mut x0, mut x1) = (width, 0);
        for y in y0..y1 {
            let (n, o) = (&new[y * width..(y + 1) * width], &old[y * width..(y + 1) * width]);
            if n == o {
                continue;
            }
            x0 = x0.min(n.iter().zip(o).position(|(a, b)| a != b).unwrap_or(width));
            x1 = x1.max(width - n.iter().rev().zip(o.iter().rev()).position(|(a, b)| a != b).unwrap_or(width));
        }
        if x0 >= x1 {
            continue;
        }
        for y in y0..y1 {
            old[y * width + x0..y * width + x1].copy_from_slice(&new[y * width + x0..y * width + x1]);
        }
        let band = Area { x0: x0 as i32, y0: y0 as i32, x1: x1 as i32, y1: y1 as i32 };
        // under the last part and about as wide: one part
        match parts.last_mut() {
            Some(last) if last.y1 == band.y0 && (last.x0 - band.x0).abs() <= 32 && (last.x1 - band.x1).abs() <= 32 => *last = last.union(band),
            _ => parts.push(band),
        }
    }
    parts
}

/// A 32-bit GDI bitmap (a DIB section) in a memory device context.
struct Bitmap {
    width: usize,
    height: usize,
    dc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    old: windows::Win32::Graphics::Gdi::HGDIOBJ,
    bits: *mut u32,
}

impl Bitmap {
    fn new(width: usize, height: usize) -> Option<Bitmap> {
        use windows::Win32::Graphics::Gdi::*;
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    // rows top-down
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bitmap) = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let old = SelectObject(dc, bitmap);
            Some(Bitmap { width, height, dc, bitmap, old, bits: bits as *mut u32 })
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: the DIB section's bits, width × height pixels, ours until dropped
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.width * self.height) }
    }

    /// Copy `parts` of it into the window `hwnd`.
    fn blit(&self, hwnd: isize, parts: &[Area]) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::*;
        if parts.is_empty() {
            return;
        }
        unsafe {
            let h = HWND(hwnd as *mut _);
            let dc = GetDC(h);
            for a in parts {
                let _ = BitBlt(dc, a.x0, a.y0, a.x1 - a.x0, a.y1 - a.y0, self.dc, a.x0, a.y0, SRCCOPY);
            }
            ReleaseDC(h, dc);
            let _ = GdiFlush();
        }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::*;
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changes_are_shown() {
        let w = 64;
        let mut old = vec![0u32; w * 40];
        let mut new = old.clone();
        // a row moved: rows 9 and 20, columns 3..30
        for x in 3..30 {
            new[9 * w + x] = 1;
            new[20 * w + x] = 2;
        }
        // two bands one under the other, as wide: one part
        let parts = changes(&new, &mut old, w);
        assert_eq!(parts, [Area { x0: 3, y0: 8, x1: 30, y1: 24 }]);
        assert_eq!(old, new);
        // nothing changed: nothing shown
        assert!(changes(&new, &mut old, w).is_empty());
        // far apart: two parts
        new[0] = 5;
        new[39 * w + 63] = 5;
        let parts = changes(&new, &mut old, w);
        assert_eq!(parts, [Area { x0: 0, y0: 0, x1: 1, y1: 8 }, Area { x0: 63, y0: 32, x1: 64, y1: 40 }]);
    }
}
