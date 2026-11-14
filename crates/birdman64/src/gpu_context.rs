//! Birdman64VR: creating the wgpu instance, window surface, adapter and
//! device, split out of `App::start` (window.rs). VR mode will add a second
//! constructor that builds the same objects on the Vulkan device the OpenXR
//! runtime picks; the default path below is the code `App::start` ran
//! before, unchanged.

use std::sync::Arc;
use winit::window::Window;

/// What `App::start` builds the window's `Gpu` from.
pub(crate) struct GpuContext {
    pub(crate) instance: wgpu::Instance,
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
}

impl GpuContext {
    /// The default path: wgpu's default backends and a high-performance
    /// adapter that can present to `window`. Shows a fatal dialog when any
    /// step fails.
    pub(crate) fn for_window(window: &Arc<Window>) -> Self {
        // Dev switch (L1): force the software adapter (WARP on DX12) to test
        // the low-end preset.
        let force_fallback = std::env::var_os("PW64_FALLBACK_ADAPTER").is_some();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).unwrap_or_else(|e| {
            crate::fatal(&format!(
                "Birdman64 could not set up graphics for its window. Updating your \
                 graphics driver may help. ({e})"
            ))
        });
        let adapter = pw64_gfx::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: force_fallback,
        }));
        let adapter = match adapter {
            Ok(a) => a,
            Err(e) => crate::fatal(&format!(
                "No compatible graphics adapter was found. Birdman64 needs a GPU with \
                 Vulkan, DirectX 12 or OpenGL support. Updating your graphics driver may \
                 help. ({e})"
            )),
        };
        let (device, queue) =
            pw64_gfx::block_on(adapter.request_device(&pw64_gfx::device_descriptor(&adapter)))
                .unwrap_or_else(|e| {
                    crate::fatal(&format!(
                        "Birdman64 could not set up the graphics device ({}). Updating your \
                 graphics driver may help. ({e})",
                        adapter.get_info().name
                    ))
                });
        Self {
            instance,
            surface,
            adapter,
            device,
            queue,
        }
    }
}
