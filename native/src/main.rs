// Lyrics Overlay, native build: the same app as the Electron one, drawn with egui instead of a web page. egui's
// output is drawn on the CPU (render.rs) into a layered window, so no graphics driver is loaded.
#![windows_subsystem = "windows"]

mod colour;
mod core;
mod cover;
mod draw;
mod fonts;
mod listen;
mod lyrics;
mod media;
mod overlay;
mod panel;
mod render;
mod shazam;
mod winutil;

use crate::core::{core, Ev};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId, WindowLevel};

fn tray_icon() -> Icon {
    let size = 32usize;
    let mut buf = vec![0u8; size * size * 4];
    let bars = [(9, 13, 8, 24), (15, 19, 8, 21), (21, 25, 8, 16)];
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let i = (y as usize * size + x as usize) * 4;
            let dx = (7 - x).max(x - (size as i32 - 8)).max(0);
            let dy = (7 - y).max(y - (size as i32 - 8)).max(0);
            if dx * dx + dy * dy > 49 {
                continue;
            }
            let white = bars.iter().any(|&(y0, y1, x0, x1)| y >= y0 && y < y1 && x >= x0 && x < x1);
            buf[i] = if white { 255 } else { 0x48 };
            buf[i + 1] = if white { 255 } else { 0xb6 };
            buf[i + 2] = 255;
            buf[i + 3] = 255;
        }
    }
    Icon::from_rgba(buf, size as u32, size as u32).expect("tray icon")
}

/// Wakes the event loop for a repaint at that moment.
struct Wake(Instant);

struct App {
    ctx: egui::Context,
    window: Option<Arc<Window>>,
    state: Option<egui_winit::State>,
    soft: render::Soft,
    hwnd: Option<isize>,
    placed: Option<(i32, i32, i32, i32)>,
    last_area_check: Instant,
    next_frame: Option<Instant>,
    overlay: overlay::Overlay,
    panel: panel::Panel,
    _tray: Option<TrayIcon>,
    show_item: Option<CheckMenuItem>,
    menu_version: u64,
    _hotkeys: Option<GlobalHotKeyManager>,
    prof: Option<Prof>,
}

/// LO_PROF=1: frame timings, written to %TEMP%\lo-prof.txt every two seconds.
#[derive(Default)]
struct Prof {
    since: Option<Instant>,
    frames: u32,
    ui: f64,
    tess: f64,
    raster: f64,
    present: f64,
    worst: f64,
    dirty: usize,
    tris: usize,
}

impl App {
    fn new(proxy: EventLoopProxy<Wake>) -> Self {
        let ctx = egui::Context::default();
        let _ = crate::core::UI.set(ctx.clone());
        fonts::install(&ctx);
        ctx.set_visuals(egui::Visuals::dark());
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.send_event(Wake(Instant::now() + info.delay));
        });

        // Tray: just the essentials; everything else lives in the settings panel.
        let menu = Menu::new();
        let settings = MenuItem::with_id("settings", "Settings…", true, None);
        let accel = tray_icon::menu::accelerator::Accelerator::new(
            tray_icon::menu::accelerator::Modifiers::CONTROL | tray_icon::menu::accelerator::Modifiers::ALT,
            tray_icon::menu::accelerator::Code::KeyL,
        );
        let show = CheckMenuItem::with_id("show", "Show lyrics", true, core().flag("enabled"), Some(accel));
        let quit = MenuItem::with_id("quit", "Quit", true, None);
        let _ = menu.append_items(&[&settings, &show, &PredefinedMenuItem::separator(), &quit]);
        let tray = TrayIconBuilder::new()
            .with_icon(tray_icon())
            .with_tooltip("Lyrics Overlay")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true)
            .build()
            .ok();
        MenuEvent::set_event_handler(Some(|e: MenuEvent| {
            match e.id.0.as_str() {
                "settings" => {
                    core().events.push(Ev::TogglePanel(None));
                }
                "show" => crate::core::hotkey("toggle"),
                "quit" => core().events.push(Ev::Quit),
                _ => {}
            }
            crate::core::notify();
        }));

        // Global shortcuts. Arrow keys, because brackets need AltGr on many non-US keyboards.
        let hotkeys = GlobalHotKeyManager::new().ok();
        let keys: [(&str, Code, &str); 7] = [
            ("Ctrl+Alt+L", Code::KeyL, "toggle"),
            ("Ctrl+Alt+Up", Code::ArrowUp, "earlier"),
            ("Ctrl+Alt+Down", Code::ArrowDown, "later"),
            ("Ctrl+Alt+M", Code::KeyM, "move"),
            ("Ctrl+Alt+S", Code::KeyS, "sync"),
            ("Ctrl+Alt+O", Code::KeyO, "other"),
            ("Ctrl+Alt+P", Code::KeyP, "previous"),
        ];
        let mut ids = Vec::new();
        let mut failed = Vec::new();
        for (label, code, name) in keys {
            let hk = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), code);
            match hotkeys.as_ref().map(|m| m.register(hk)) {
                Some(Ok(())) => ids.push((hk.id(), name)),
                _ => failed.push(label.to_string()),
            }
        }
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state != HotKeyState::Pressed {
                return;
            }
            if let Some((_, name)) = ids.iter().find(|(id, _)| *id == e.id) {
                crate::core::hotkey(name);
            }
        }));
        if !failed.is_empty() {
            core().events.push(Ev::Toast(format!("Shortcuts used by another app: {}", failed.join(", "))));
        }

        App {
            ctx,
            window: None,
            state: None,
            soft: render::Soft::new(),
            hwnd: None,
            placed: None,
            last_area_check: Instant::now(),
            next_frame: Some(Instant::now()),
            overlay: overlay::Overlay::new(),
            panel: panel::Panel::new(),
            _tray: tray,
            show_item: Some(show),
            menu_version: 0,
            _hotkeys: hotkeys,
            prof: std::env::var("LO_PROF").is_ok().then(Prof::default),
        }
    }

    /// Keeps the overlay over the work area (the screen minus the taskbar), like Electron's fitToScreen.
    fn fit(&mut self) {
        let Some(hwnd) = self.hwnd else { return };
        let area = winutil::work_area();
        if self.placed != Some(area) {
            self.placed = Some(area);
            winutil::place(hwnd, area.0, area.1, area.2, area.3);
            crate::core::SCREEN.lock().unwrap().origin = (area.0, area.1);
        }
    }

    fn wake_at(&mut self, at: Instant) {
        self.next_frame = Some(self.next_frame.map_or(at, |n| n.min(at)));
    }

    /// One frame: run the UI, draw what changed, and work out when the next one is due.
    fn frame(&mut self) {
        let (Some(window), Some(hwnd)) = (self.window.clone(), self.hwnd) else { return };
        self.next_frame = None;
        if self.placed.is_none() || self.last_area_check.elapsed() > Duration::from_secs(2) {
            self.last_area_check = Instant::now();
            self.fit();
        }
        let ctx = self.ctx.clone();
        let mut raw = self.state.as_mut().unwrap().take_egui_input(&window);
        egui_winit::update_viewport_info(raw.viewports.entry(egui::ViewportId::ROOT).or_default(), &ctx, &window, false);
        if self.overlay.pointer_left {
            self.overlay.pointer_left = false;
            raw.events.push(egui::Event::PointerGone);
        }
        let t0 = Instant::now();
        let out = ctx.run_pass(raw, |ctx| self.ui(ctx));
        let t_ui = t0.elapsed().as_secs_f64() * 1000.0;
        self.state.as_mut().unwrap().handle_platform_output(&window, out.platform_output);
        let ppp = out.pixels_per_point;
        crate::core::SCREEN.lock().unwrap().scale = ppp as f64;
        let t1 = Instant::now();
        let prims = ctx.tessellate(out.shapes, ppp);
        let t_tess = t1.elapsed().as_secs_f64() * 1000.0;
        let (x, y, w, h) = self.placed.unwrap_or_else(winutil::work_area);
        let floor = if core().editing { 1 } else { 0 };
        self.soft.paint(hwnd, x, y, w as usize, h as usize, ppp, floor, &prims, &out.textures_delta);
        if let Some(p) = &mut self.prof {
            let st = self.soft.stats;
            let total = t0.elapsed().as_secs_f64() * 1000.0;
            p.frames += 1;
            p.ui += t_ui;
            p.tess += t_tess;
            p.raster += st.raster;
            p.present += st.present;
            p.worst = p.worst.max(total);
            p.dirty += st.dirty;
            p.tris = p.tris.max(st.tris);
            let since = *p.since.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(2) {
                let n = p.frames.max(1) as f64;
                let line = format!(
                    "{:.1} fps | ui {:.2} tess {:.2} raster {:.2} present {:.2} ms avg | worst {:.1} ms | dirty tiles {:.0} | tris {}
",
                    n / since.elapsed().as_secs_f64(), p.ui / n, p.tess / n, p.raster / n, p.present / n, p.worst, p.dirty as f64 / n, p.tris
                );
                let path = std::env::temp_dir().join("lo-prof.txt");
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let _ = f.write_all(line.as_bytes());
                }
                *p = Prof::default();
            }
        }
        if let Some(v) = out.viewport_output.get(&egui::ViewportId::ROOT) {
            if v.repaint_delay < Duration::from_secs(3600) {
                self.wake_at(Instant::now() + v.repaint_delay);
            }
        }
        // Check the work area now and then even when nothing moves.
        self.wake_at(self.last_area_check + Duration::from_secs(2));
    }

    fn ui(&mut self, ctx: &egui::Context) {
        // What the rest of the app asked for.
        let events: Vec<Ev> = std::mem::take(&mut core().events);
        for ev in events {
            match ev {
                Ev::Toast(t) => self.overlay.toast(&t),
                Ev::SyncNow => self.overlay.sync_now(),
                Ev::PointerLeft => self.overlay.pointer_left(),
                Ev::TogglePanel(at) => {
                    if self.panel.is_open() {
                        self.panel.close();
                    } else {
                        let at = at.or_else(|| self.overlay.card_place());
                        self.panel.open(ctx, at);
                    }
                }
                Ev::ClosePanel => self.panel.close(),
                Ev::Quit => {
                    core().quitting = true;
                    std::process::exit(0);
                }
            }
        }

        let version = core().settings_version;
        if version != self.menu_version {
            self.menu_version = version;
            if let Some(item) = &self.show_item {
                item.set_checked(core().flag("enabled"));
            }
        }

        let screen = ctx.content_rect();
        self.overlay.ui(ctx, screen, self.panel.is_open());
        self.panel.ui(ctx, screen);
        // Option::min would pick None (no wish) over a wish, so combine by hand.
        let wake = match (self.overlay.next_wake(), self.panel.next_wake()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        crate::draw::sprites_frame_end();
        if let Some(w) = wake {
            ctx.request_repaint_after(w);
        }
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (x, y, w, h) = winutil::work_area();
        let attrs = Window::default_attributes()
            .with_title("Lyrics Overlay")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_skip_taskbar(true)
            .with_active(false)
            .with_position(winit::dpi::PhysicalPosition::new(x, y))
            .with_inner_size(winit::dpi::PhysicalSize::new(w as u32, h as u32));
        let window = Arc::new(el.create_window(attrs).expect("window"));
        if let Ok(h) = window.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                let hwnd = w.hwnd.get();
                winutil::make_overlay_window(hwnd);
                self.hwnd = Some(hwnd);
                self.panel.hwnd = Some(hwnd);
            }
        }
        let state = egui_winit::State::new(self.ctx.clone(), egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, Some(8192));
        self.state = Some(state);
        self.window = Some(window);
        self.wake_at(Instant::now());
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else { return };
        match event {
            WindowEvent::CloseRequested => return, // only Quit ends the app
            WindowEvent::RedrawRequested => {
                self.wake_at(Instant::now());
                return;
            }
            _ => {}
        }
        let r = self.state.as_mut().unwrap().on_window_event(&window, &event);
        if r.repaint {
            self.wake_at(Instant::now());
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, wake: Wake) {
        self.wake_at(wake.0);
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.next_frame.is_some_and(|t| t <= Instant::now()) {
            self.frame();
        }
        el.set_control_flow(match self.next_frame {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

fn main() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if !winutil::single_instance(crate::core::second_instance) {
        return;
    }
    winutil::set_app_id();
    crate::core::start();

    let el = EventLoop::<Wake>::with_user_event().build().expect("event loop");
    let mut app = App::new(el.create_proxy());
    let _ = el.run_app(&mut app);
}
