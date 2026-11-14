//! The OpenXR session: loader, lifecycle events, the panel swapchain and
//! the per-frame wait / begin / end.

use crate::math::{self, Mat4};
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

/// When the frame being drawn will be shown. Animate from this, not from
/// the clock at draw time: the runtime wakes the app at uneven moments but
/// shows frames at even ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTime {
    /// Predicted display time on the runtime's clock, in nanoseconds. Only
    /// differences between frames mean anything.
    pub display_ns: i64,
    /// The display's frame period in nanoseconds (72 Hz: 13.9 ms).
    pub period_ns: i64,
}

/// One eye of the stereo world layer for the frame being drawn.
pub struct Eye<'a> {
    /// Render target (one layer of the eye swapchain).
    pub target: &'a wgpu::TextureView,
    /// World → eye for the predicted head pose (`math::view`), in the
    /// seated space.
    pub view: Mat4,
    /// Eye → clip for this eye's field of view (`math::projection`).
    pub projection: Mat4,
}

/// What to draw into this frame; handed to the `draw` callback.
pub struct Frame<'a> {
    pub time: FrameTime,
    /// The flat panel's texture, when [`Layers::panel`] is set.
    pub panel: Option<&'a wgpu::TextureView>,
    /// Left and right eye, when [`Layers::world`] is set.
    pub eyes: Option<[Eye<'a>; 2]>,
}

/// Which layers a frame shows: the stereo world (drawn first, behind) and
/// the flat panel in front of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layers {
    pub world: bool,
    pub panel: Option<Panel>,
}

/// Near and far planes of the eye projections, in metres.
const NEAR: f32 = 0.05;
const FAR: f32 = 1000.0;

/// What [`XrContext::frame`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    /// The layers were drawn and shown.
    Rendered,
    /// The session is running but nothing is visible (headset off the
    /// head, system menu): no draw this frame.
    Hidden,
    /// The session is not running yet or any more; call again later.
    Idle,
    /// The runtime asked us to quit (or the session was lost).
    Quit,
}

/// A swapchain and its images as wgpu views: `views[image][layer]`.
struct Swapchain {
    handle: xr::Swapchain<xr::Vulkan>,
    views: Vec<Vec<wgpu::TextureView>>,
    size_px: (u32, u32),
}

/// One OpenXR session and its wgpu device.
///
/// Field order matters: the OpenXR objects drop before `gpu`, whose device
/// they were created on.
pub struct XrContext {
    panel: Swapchain,
    /// Both eyes: one image with two array layers.
    eyes: Swapchain,
    space: xr::Space,
    frame_stream: xr::FrameStream<xr::Vulkan>,
    frame_waiter: xr::FrameWaiter,
    session: xr::Session<xr::Vulkan>,
    instance: xr::Instance,
    blend_mode: xr::EnvironmentBlendMode,
    format: wgpu::TextureFormat,
    running: bool,
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
        let panel = create_swapchain(&session, &gpu.device, vk_format, format, panel.size_px, 1)?;
        // The size the runtime recommends for one eye.
        let eye = instance
            .enumerate_view_configuration_views(system, VIEW_TYPE)?
            .first()
            .map(|v| {
                (
                    v.recommended_image_rect_width,
                    v.recommended_image_rect_height,
                )
            })
            .ok_or_else(|| XrError::Unsupported("a stereo view configuration".into()))?;
        let eyes = create_swapchain(&session, &gpu.device, vk_format, format, eye, 2)?;

        Ok(Self {
            panel,
            eyes,
            space,
            frame_stream,
            frame_waiter,
            session,
            instance,
            blend_mode,
            format,
            running: false,
            gpu,
        })
    }

    /// The wgpu objects to render with.
    pub fn gpu(&self) -> &XrGpu {
        &self.gpu
    }

    /// The format of the panel and eye textures (what `draw` in
    /// [`Self::frame`] targets).
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// One eye's render size in pixels (the runtime's recommendation).
    pub fn eye_size(&self) -> (u32, u32) {
        self.eyes.size_px
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
        // Local: the buffer holds raw pointers, which would make the
        // context impossible to move to the headset thread.
        let mut events = xr::EventDataBuffer::new();
        while let Some(event) = self.instance.poll_event(&mut events)? {
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
    /// rate), then shows `layers`. `draw` renders into the targets in its
    /// [`Frame`] and must submit its commands to the queue before
    /// returning; it is only called for [`FrameOutcome::Rendered`].
    ///
    /// `queue_lock`: `XR_KHR_vulkan_enable2` lets the runtime use our
    /// Vulkan queue inside begin/end frame and swapchain acquire/release,
    /// and requires that nothing else touches the queue meanwhile. wgpu
    /// only serialises its own use. So when another thread also submits or
    /// presents on this device, both sides must hold this lock: it is taken
    /// here after the wait, until the frame has ended. Racing the runtime
    /// loses the device (seen with Quest Link).
    pub fn frame(
        &mut self,
        layers: &Layers,
        queue_lock: Option<&std::sync::Mutex<()>>,
        draw: impl FnOnce(Frame<'_>),
    ) -> Result<FrameOutcome, XrError> {
        if !self.poll_events()? {
            return Ok(FrameOutcome::Quit);
        }
        if !self.running {
            return Ok(FrameOutcome::Idle);
        }
        let state = self.frame_waiter.wait()?;
        let _queue = queue_lock.map(|m| m.lock().unwrap_or_else(|e| e.into_inner()));
        self.frame_stream.begin()?;
        if !state.should_render {
            self.frame_stream
                .end(state.predicted_display_time, self.blend_mode, &[])?;
            return Ok(FrameOutcome::Hidden);
        }
        // Where the eyes will be when this frame is shown.
        let located = if layers.world {
            let (_, views) =
                self.session
                    .locate_views(VIEW_TYPE, state.predicted_display_time, &self.space)?;
            (views.len() == 2).then_some(views)
        } else {
            None
        };
        let eye_image = match &located {
            Some(_) => {
                let i = self.eyes.handle.acquire_image()? as usize;
                self.eyes.handle.wait_image(xr::Duration::INFINITE)?;
                Some(i)
            }
            None => None,
        };
        let panel_image = match layers.panel {
            Some(_) => {
                let i = self.panel.handle.acquire_image()? as usize;
                self.panel.handle.wait_image(xr::Duration::INFINITE)?;
                Some(i)
            }
            None => None,
        };
        let eyes = located.as_ref().zip(eye_image).map(|(views, image)| {
            [0, 1].map(|i| {
                let (p, o, f) = (
                    views[i].pose.position,
                    views[i].pose.orientation,
                    views[i].fov,
                );
                Eye {
                    target: &self.eyes.views[image][i],
                    view: math::view([o.x, o.y, o.z, o.w], [p.x, p.y, p.z]),
                    projection: math::projection(
                        f.angle_left,
                        f.angle_right,
                        f.angle_up,
                        f.angle_down,
                        NEAR,
                        FAR,
                    ),
                }
            })
        });
        draw(Frame {
            time: FrameTime {
                display_ns: state.predicted_display_time.as_nanos(),
                period_ns: state.predicted_display_period.as_nanos(),
            },
            panel: panel_image.map(|i| &self.panel.views[i][0]),
            eyes,
        });
        if eye_image.is_some() {
            self.eyes.handle.release_image()?;
        }
        if panel_image.is_some() {
            self.panel.handle.release_image()?;
        }

        let rect = |size: (u32, u32)| xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di {
                width: size.0 as i32,
                height: size.1 as i32,
            },
        };
        let projection_views = located.as_ref().map(|views| {
            [0usize, 1].map(|i| {
                xr::CompositionLayerProjectionView::new()
                    .pose(views[i].pose)
                    .fov(views[i].fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&self.eyes.handle)
                            .image_array_index(i as u32)
                            .image_rect(rect(self.eyes.size_px)),
                    )
            })
        });
        let world_layer = projection_views.as_ref().map(|views| {
            xr::CompositionLayerProjection::new()
                .space(&self.space)
                .views(views)
        });
        let panel_layer = layers.panel.filter(|_| panel_image.is_some()).map(|panel| {
            let (w, h) = self.panel.size_px;
            xr::CompositionLayerQuad::new()
                .space(&self.space)
                .eye_visibility(xr::EyeVisibility::BOTH)
                .sub_image(
                    xr::SwapchainSubImage::new()
                        .swapchain(&self.panel.handle)
                        .image_array_index(0)
                        .image_rect(rect(self.panel.size_px)),
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
                })
        });
        // Back to front: the world, then the panel over it.
        let mut submit: Vec<&xr::CompositionLayerBase<'_, xr::Vulkan>> = Vec::new();
        if let Some(l) = &world_layer {
            submit.push(l);
        }
        if let Some(l) = &panel_layer {
            submit.push(l);
        }
        self.frame_stream
            .end(state.predicted_display_time, self.blend_mode, &submit)?;
        Ok(FrameOutcome::Rendered)
    }
}

/// A colour swapchain with `layers` array layers, its images wrapped as
/// wgpu texture views (one per image and layer).
fn create_swapchain(
    session: &xr::Session<xr::Vulkan>,
    device: &wgpu::Device,
    vk_format: vk::Format,
    format: wgpu::TextureFormat,
    size_px: (u32, u32),
    layers: u32,
) -> Result<Swapchain, XrError> {
    let handle = session.create_swapchain(&xr::SwapchainCreateInfo {
        create_flags: xr::SwapchainCreateFlags::EMPTY,
        usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | xr::SwapchainUsageFlags::SAMPLED,
        format: vk_format.as_raw() as u32,
        sample_count: 1,
        width: size_px.0,
        height: size_px.1,
        face_count: 1,
        array_size: layers,
        mip_count: 1,
    })?;
    let size = wgpu::Extent3d {
        width: size_px.0,
        height: size_px.1,
        depth_or_array_layers: layers,
    };
    let views = handle
        .enumerate_images()?
        .into_iter()
        .map(|image| {
            // SAFETY: the image belongs to the swapchain created with this
            // size, format and usage; the no-op drop callback leaves it to
            // the runtime, and `Swapchain` keeps the swapchain alive as
            // long as the views.
            let texture = unsafe {
                let hal_device = device
                    .as_hal::<Vulkan>()
                    .ok_or_else(|| XrError::Graphics("not a Vulkan device".into()))?;
                let hal_texture = hal_device.texture_from_raw(
                    vk::Image::from_raw(image),
                    &hal::TextureDescriptor {
                        label: Some("openxr swapchain"),
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
                        label: Some("openxr swapchain"),
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
            Ok((0..layers)
                .map(|layer| {
                    texture.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_array_layer: layer,
                        array_layer_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect())
        })
        .collect::<Result<Vec<_>, XrError>>()?;
    Ok(Swapchain {
        handle,
        views,
        size_px,
    })
}
