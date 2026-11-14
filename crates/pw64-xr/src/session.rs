//! The OpenXR session: loader, lifecycle events, the panel swapchain and
//! the per-frame wait / begin / end.

use crate::{XrError, XrGpu};
use ash::vk::{self, Handle};
use openxr as xr;
use std::path::PathBuf;
use wgpu::hal;
use wgpu::hal::api::Vulkan;

/// Where the flat panel floats, in metres, in the seated (`LOCAL`) space:
/// the origin is the player's head when the session started or was last
/// recentred, -z is forward.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Panel {
    /// Texture size in pixels.
    pub size_px: (u32, u32),
    /// Width in metres; the height follows the texture's aspect.
    pub width_m: f32,
    /// Distance in front of the player.
    pub distance_m: f32,
    /// Height of the panel's centre relative to the head.
    pub height_m: f32,
}

impl Default for Panel {
    /// A 4:3 panel filling about 60 degrees at 2.5 m.
    fn default() -> Self {
        Self {
            size_px: (1440, 1080),
            width_m: 2.9,
            distance_m: 2.5,
            height_m: 0.0,
        }
    }
}

/// What [`XrContext::frame`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    /// The panel was drawn and shown.
    Rendered,
    /// The session is running but nothing is visible (headset off the
    /// head, system menu): no draw this frame.
    Hidden,
    /// The session is not running yet or any more; call again later.
    Idle,
    /// The runtime asked us to quit (or the session was lost).
    Quit,
}

struct PanelSwapchain {
    handle: xr::Swapchain<xr::Vulkan>,
    views: Vec<wgpu::TextureView>,
    size_px: (u32, u32),
}

/// One OpenXR session and its wgpu device.
///
/// Field order matters: the OpenXR objects drop before `gpu`, whose device
/// they were created on.
pub struct XrContext {
    panel: PanelSwapchain,
    space: xr::Space,
    frame_stream: xr::FrameStream<xr::Vulkan>,
    frame_waiter: xr::FrameWaiter,
    session: xr::Session<xr::Vulkan>,
    instance: xr::Instance,
    blend_mode: xr::EnvironmentBlendMode,
    format: wgpu::TextureFormat,
    running: bool,
    events: xr::EventDataBuffer,
    gpu: XrGpu,
}

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

/// The loader library: `PW64_OPENXR_LOADER`, next to the exe, the dev
/// tree's `tools/openxr/`, then the system search path.
fn load_entry() -> Result<xr::Entry, XrError> {
    #[cfg(windows)]
    const NAME: &str = "openxr_loader.dll";
    #[cfg(not(windows))]
    const NAME: &str = "libopenxr_loader.so";
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("PW64_OPENXR_LOADER") {
        candidates.push(p.into());
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
    {
        candidates.push(dir.join(NAME));
    }
    candidates.push(PathBuf::from("tools/openxr").join(NAME));
    let mut last = String::from("not found");
    for p in candidates.iter().filter(|p| p.is_file()) {
        // SAFETY: a file with the loader's name; it is the Khronos loader.
        match unsafe { xr::Entry::load_from(p, &()) } {
            Ok(e) => return Ok(e),
            Err(e) => last = format!("{}: {e}", p.display()),
        }
    }
    // SAFETY: as above, found by the OS loader.
    unsafe { xr::Entry::load(&()) }.map_err(|e| {
        if last == "not found" {
            last = e.to_string();
        }
        XrError::Loader(last)
    })
}

impl XrContext {
    /// Connects to the active OpenXR runtime and creates the wgpu device on
    /// the GPU it picks. `wanted_features`: see [`XrGpu`].
    pub fn new(wanted_features: wgpu::Features, panel: Panel) -> Result<Self, XrError> {
        let entry = load_entry()?;
        let available = entry
            .enumerate_extensions()
            .map_err(|e| XrError::NoHeadset(format!("is the VR runtime running? {e}")))?;
        if !available.khr_vulkan_enable2 {
            return Err(XrError::Unsupported(
                "Vulkan support (XR_KHR_vulkan_enable2)".into(),
            ));
        }
        let mut enabled = xr::ExtensionSet::default();
        enabled.khr_vulkan_enable2 = true;
        let instance = entry
            .create_instance(
                &xr::ApplicationInfo {
                    application_name: "Birdman64VR",
                    application_version: 0,
                    engine_name: "pw64-xr",
                    engine_version: 0,
                    api_version: xr::Version::new(1, 0, 0),
                },
                &enabled,
                &[],
                &(),
            )
            .map_err(|e| XrError::NoHeadset(format!("is the VR runtime running? {e}")))?;
        if let Ok(p) = instance.properties() {
            log::info!("OpenXR runtime: {} {}", p.runtime_name, p.runtime_version);
        }
        let system = instance
            .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
            .map_err(|e| XrError::NoHeadset(format!("is the headset connected? {e}")))?;
        let blend_modes = instance.enumerate_environment_blend_modes(system, VIEW_TYPE)?;
        let blend_mode = *blend_modes
            .first()
            .ok_or_else(|| XrError::Unsupported("a display blend mode".into()))?;

        let gpu = XrGpu::new(&instance, system, wanted_features)?;
        // SAFETY: the handles are the ones the runtime created in
        // `XrGpu::new`; `gpu` (last field) outlives the session.
        let (session, frame_waiter, frame_stream) = unsafe {
            instance.create_session::<xr::Vulkan>(
                system,
                &xr::vulkan::SessionCreateInfo {
                    instance: gpu.vk_instance.as_raw() as _,
                    physical_device: gpu.vk_physical_device.as_raw() as _,
                    device: gpu.vk_device.as_raw() as _,
                    queue_family_index: gpu.queue_family_index,
                    queue_index: 0,
                },
            )?
        };
        let space =
            session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;

        let formats = session.enumerate_swapchain_formats()?;
        let (vk_format, format) = [
            (
                vk::Format::R8G8B8A8_SRGB,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ),
            (
                vk::Format::B8G8R8A8_SRGB,
                wgpu::TextureFormat::Bgra8UnormSrgb,
            ),
        ]
        .into_iter()
        .find(|(f, _)| formats.contains(&(f.as_raw() as u32)))
        .ok_or_else(|| XrError::Unsupported("an 8-bit sRGB swapchain format".into()))?;
        let panel = create_panel(&session, &gpu.device, vk_format, format, panel.size_px)?;

        Ok(Self {
            panel,
            space,
            frame_stream,
            frame_waiter,
            session,
            instance,
            blend_mode,
            format,
            running: false,
            events: xr::EventDataBuffer::new(),
            gpu,
        })
    }

    /// The wgpu objects to render with.
    pub fn gpu(&self) -> &XrGpu {
        &self.gpu
    }

    /// The panel texture's format (what `draw` in [`Self::frame`] targets).
    pub fn panel_format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The runtime's name and version, for logs.
    pub fn runtime(&self) -> String {
        self.instance.properties().map_or_else(
            |_| "?".into(),
            |p| format!("{} {}", p.runtime_name, p.runtime_version),
        )
    }

    /// Session lifecycle: begin on READY, end on STOPPING. `false` = quit.
    fn poll_events(&mut self) -> Result<bool, XrError> {
        while let Some(event) = self.instance.poll_event(&mut self.events)? {
            use xr::Event::*;
            match event {
                SessionStateChanged(e) => {
                    log::debug!("OpenXR session state {:?}", e.state());
                    match e.state() {
                        xr::SessionState::READY => {
                            self.session.begin(VIEW_TYPE)?;
                            self.running = true;
                        }
                        xr::SessionState::STOPPING => {
                            self.session.end()?;
                            self.running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            return Ok(false);
                        }
                        _ => {}
                    }
                }
                InstanceLossPending(_) => return Ok(false),
                _ => {}
            }
        }
        Ok(true)
    }

    /// One headset frame: blocks until the runtime wants it (its display
    /// rate), then shows the panel. `draw` renders into the panel texture
    /// and must submit its commands to the queue before returning; it is
    /// only called for [`FrameOutcome::Rendered`].
    pub fn frame(
        &mut self,
        panel: &Panel,
        draw: impl FnOnce(&wgpu::TextureView),
    ) -> Result<FrameOutcome, XrError> {
        if !self.poll_events()? {
            return Ok(FrameOutcome::Quit);
        }
        if !self.running {
            return Ok(FrameOutcome::Idle);
        }
        let state = self.frame_waiter.wait()?;
        self.frame_stream.begin()?;
        if !state.should_render {
            self.frame_stream
                .end(state.predicted_display_time, self.blend_mode, &[])?;
            return Ok(FrameOutcome::Hidden);
        }
        let index = self.panel.handle.acquire_image()? as usize;
        self.panel.handle.wait_image(xr::Duration::INFINITE)?;
        draw(&self.panel.views[index]);
        self.panel.handle.release_image()?;

        let (w, h) = self.panel.size_px;
        let layer = xr::CompositionLayerQuad::new()
            .space(&self.space)
            .eye_visibility(xr::EyeVisibility::BOTH)
            .sub_image(
                xr::SwapchainSubImage::new()
                    .swapchain(&self.panel.handle)
                    .image_array_index(0)
                    .image_rect(xr::Rect2Di {
                        offset: xr::Offset2Di { x: 0, y: 0 },
                        extent: xr::Extent2Di {
                            width: w as i32,
                            height: h as i32,
                        },
                    }),
            )
            .pose(xr::Posef {
                orientation: xr::Quaternionf::IDENTITY,
                position: xr::Vector3f {
                    x: 0.0,
                    y: panel.height_m,
                    z: -panel.distance_m,
                },
            })
            .size(xr::Extent2Df {
                width: panel.width_m,
                height: panel.width_m * h as f32 / w as f32,
            });
        self.frame_stream
            .end(state.predicted_display_time, self.blend_mode, &[&layer])?;
        Ok(FrameOutcome::Rendered)
    }
}

/// The panel's swapchain, its images wrapped as wgpu texture views.
fn create_panel(
    session: &xr::Session<xr::Vulkan>,
    device: &wgpu::Device,
    vk_format: vk::Format,
    format: wgpu::TextureFormat,
    size_px: (u32, u32),
) -> Result<PanelSwapchain, XrError> {
    let handle = session.create_swapchain(&xr::SwapchainCreateInfo {
        create_flags: xr::SwapchainCreateFlags::EMPTY,
        usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | xr::SwapchainUsageFlags::SAMPLED,
        format: vk_format.as_raw() as u32,
        sample_count: 1,
        width: size_px.0,
        height: size_px.1,
        face_count: 1,
        array_size: 1,
        mip_count: 1,
    })?;
    let size = wgpu::Extent3d {
        width: size_px.0,
        height: size_px.1,
        depth_or_array_layers: 1,
    };
    let views = handle
        .enumerate_images()?
        .into_iter()
        .map(|image| {
            // SAFETY: the image belongs to the swapchain created with this
            // size, format and usage; the no-op drop callback leaves it to
            // the runtime, and `PanelSwapchain` keeps the swapchain alive
            // as long as the views.
            let texture = unsafe {
                let hal_device = device
                    .as_hal::<Vulkan>()
                    .ok_or_else(|| XrError::Graphics("not a Vulkan device".into()))?;
                let hal_texture = hal_device.texture_from_raw(
                    vk::Image::from_raw(image),
                    &hal::TextureDescriptor {
                        label: Some("openxr panel"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUses::COLOR_TARGET | wgpu::TextureUses::RESOURCE,
                        memory_flags: hal::MemoryFlags::empty(),
                        view_formats: vec![],
                    },
                    Some(Box::new(|| {})),
                );
                device.create_texture_from_hal::<Vulkan>(
                    hal_texture,
                    &wgpu::TextureDescriptor {
                        label: Some("openxr panel"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                )
            };
            Ok(texture.create_view(&wgpu::TextureViewDescriptor::default()))
        })
        .collect::<Result<Vec<_>, XrError>>()?;
    Ok(PanelSwapchain {
        handle,
        views,
        size_px,
    })
}
