// Default renderer (cargo feature "dx12", on by default): draws egui's output on the graphics card with Direct3D 12,
// presented through a DirectComposition visual so the window can be see-through. About 5x less CPU than the CPU
// renderer (render.rs) while lyrics animate, at the cost of the graphics driver's memory. When it can't start, or
// fails later, the app falls back to the CPU renderer (main.rs).

use egui::epaint::ClippedPrimitive;
use egui::TexturesDelta;
use egui_wgpu::wgpu;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use winit::window::Window;

/// Set when the graphics device reports an error or is lost; the app then switches to the CPU renderer.
static FAILED: AtomicBool = AtomicBool::new(false);

pub struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: egui_wgpu::Renderer,
}

impl Gpu {
    pub fn new(window: Arc<Window>, w: u32, h: u32) -> Result<Self, String> {
        // For testing the fallback.
        if std::env::var("LO_GPU_FAIL").is_ok() {
            return Err("LO_GPU_FAIL is set".into());
        }
        FAILED.store(false, Ordering::Relaxed);
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12;
        desc.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
        let instance = wgpu::Instance::new(desc);
        let surface = instance.create_surface(window).map_err(|e| format!("surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("graphics adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| format!("graphics device: {e}"))?;
        // wgpu panics on device errors unless they're handled; the app is built with panic = "abort".
        device.on_uncaptured_error(Arc::new(|_e| FAILED.store(true, Ordering::Relaxed)));
        // Driver crash or update, card removed: draw on the CPU instead.
        device.set_device_lost_callback(|_, _| FAILED.store(true, Ordering::Relaxed));
        let caps = surface.get_capabilities(&adapter);
        if caps.formats.is_empty() || caps.alpha_modes.is_empty() {
            return Err("the graphics card can't draw into this window".into());
        }
        let mut config = surface.get_default_config(&adapter, w.max(1), h.max(1)).ok_or("surface config")?;
        // egui blends in gamma space, so a plain (not sRGB) target, and premultiplied alpha for a see-through window.
        config.format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(config.format);
        config.alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            caps.alpha_modes[0]
        };
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        let renderer = egui_wgpu::Renderer::new(&device, config.format, egui_wgpu::RendererOptions::default());
        if FAILED.load(Ordering::Relaxed) {
            return Err("graphics device error while setting up".into());
        }
        Ok(Gpu { surface, device, queue, config, renderer })
    }

    /// Draws a frame. False when the graphics device has failed and the CPU renderer should take over.
    pub fn paint(&mut self, w: u32, h: u32, ppp: f32, prims: &[ClippedPrimitive], delta: &TexturesDelta) -> bool {
        if FAILED.load(Ordering::Relaxed) {
            return false;
        }
        if (w, h) != (self.config.width, self.config.height) && w > 0 && h > 0 {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }
        for (id, list) in delta.set.iter() {
            for d in list {
                self.renderer.update_texture(&self.device, &self.queue, *id, d);
            }
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Lost => {
                FAILED.store(true, Ordering::Relaxed);
                return false;
            }
            _ => {
                self.surface.configure(&self.device, &self.config);
                return !FAILED.load(Ordering::Relaxed);
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: [self.config.width, self.config.height], pixels_per_point: ppp };
        let cmds = self.renderer.update_buffers(&self.device, &self.queue, &mut enc, prims, &sd);
        {
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            self.renderer.render(&mut pass.forget_lifetime(), prims, &sd);
        }
        self.queue.submit(cmds.into_iter().chain([enc.finish()]));
        self.queue.present(frame);
        for id in &delta.free {
            self.renderer.free_texture(id);
        }
        !FAILED.load(Ordering::Relaxed)
    }
}
