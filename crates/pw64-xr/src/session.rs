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
    /// How far ahead of now the predicted display time is, when the
    /// runtime exposes its clock: the delay between drawing and showing.
    pub ahead_ns: Option<i64>,
}

/// Where one eye is and what it sees, in the runtime's `LOCAL` space
/// ([`Seat::seated`] gives it in the seated space): plain data, so it can
/// be handed to another thread and back ([`Layers::eyes`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyePose {
    /// Unit quaternion x, y, z, w.
    pub orientation: [f32; 4],
    /// Metres.
    pub position: [f32; 3],
    /// Field of view angles in radians: left, right, up, down (left and
    /// down are negative).
    pub fov: [f32; 4],
}

impl EyePose {
    fn from_view(v: &xr::View) -> Self {
        let (o, p, f) = (v.pose.orientation, v.pose.position, v.fov);
        Self {
            orientation: [o.x, o.y, o.z, o.w],
            position: [p.x, p.y, p.z],
            fov: [f.angle_left, f.angle_right, f.angle_up, f.angle_down],
        }
    }

    fn pose(&self) -> xr::Posef {
        let (o, p) = (self.orientation, self.position);
        xr::Posef {
            orientation: xr::Quaternionf {
                x: o[0],
                y: o[1],
                z: o[2],
                w: o[3],
            },
            position: xr::Vector3f {
                x: p[0],
                y: p[1],
                z: p[2],
            },
        }
    }

    fn fov(&self) -> xr::Fovf {
        xr::Fovf {
            angle_left: self.fov[0],
            angle_right: self.fov[1],
            angle_up: self.fov[2],
            angle_down: self.fov[3],
        }
    }
}

/// Where the player sits: the seated space's origin and forward (-z) in
/// the runtime's `LOCAL` space. Everything the app draws is placed in the
/// seated space; [`XrContext::recentre`] moves it to where the head is.
/// Level by construction: only a turn about the vertical axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seat {
    /// Unit quaternion x, y, z, w (a turn about y).
    pub orientation: [f32; 4],
    /// Metres.
    pub position: [f32; 3],
}

impl Default for Seat {
    /// `LOCAL` itself: the head when the session started, or when the
    /// system last recentred (long press of the Quest's Meta button).
    fn default() -> Self {
        Self {
            orientation: [0.0, 0.0, 0.0, 1.0],
            position: [0.0; 3],
        }
    }
}

impl Seat {
    /// A seat at a head pose: there, facing the way it faces, level.
    pub fn at_head(orientation: [f32; 4], position: [f32; 3]) -> Self {
        Self {
            orientation: math::yaw_only(orientation),
            position,
        }
    }

    /// `pose` (in `LOCAL`) relative to the seat: in the seated space.
    pub fn seated(&self, pose: &EyePose) -> EyePose {
        let inv = math::quat_conj(self.orientation);
        let d = [0, 1, 2].map(|i| pose.position[i] - self.position[i]);
        EyePose {
            orientation: math::quat_mul(inv, pose.orientation),
            position: math::rotate(inv, d),
            fov: pose.fov,
        }
    }

    /// A pose in the seated space, in `LOCAL`.
    fn place(&self, orientation: [f32; 4], position: [f32; 3]) -> xr::Posef {
        let o = math::quat_mul(self.orientation, orientation);
        let r = math::rotate(self.orientation, position);
        xr::Posef {
            orientation: xr::Quaternionf {
                x: o[0],
                y: o[1],
                z: o[2],
                w: o[3],
            },
            position: xr::Vector3f {
                x: self.position[0] + r[0],
                y: self.position[1] + r[1],
                z: self.position[2] + r[2],
            },
        }
    }
}

/// One eye of the stereo world layer for the frame being drawn.
pub struct Eye<'a> {
    /// Render target (one layer of the eye swapchain).
    pub target: &'a wgpu::TextureView,
    /// Seated space → eye for the predicted head pose (`math::view` of
    /// [`Seat::seated`]).
    pub view: Mat4,
    /// Eye → clip for this eye's field of view (`math::projection`).
    pub projection: Mat4,
    /// The pose (in `LOCAL`, not seated) and field of view both were made
    /// from.
    pub pose: EyePose,
}

/// What to draw into this frame; handed to the `draw` callback.
pub struct Frame<'a> {
    pub time: FrameTime,
    /// The flat panel's texture, when [`Layers::panel`] is set.
    pub panel: Option<&'a wgpu::TextureView>,
    /// Left and right eye, when [`Layers::world`] is set.
    pub eyes: Option<[Eye<'a>; 2]>,
    /// Where the eyes are predicted to be one frame after this one: what
    /// to draw the next picture for when it takes a frame to make
    /// ([`Layers::eyes`]).
    pub next_eyes: Option<[EyePose; 2]>,
    /// Where the player sits this frame: draw the world for
    /// `seat.seated(pose)`, submit it for `pose`.
    pub seat: Seat,
    /// The player's room is shown behind this frame's picture
    /// ([`Layers::passthrough`], granted): leave the eye targets
    /// transparent (premultiplied alpha) where it should show.
    pub passthrough: bool,
}

/// Which layers a frame shows: the stereo world (drawn first, behind) and
/// the flat panel in front of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layers {
    pub world: bool,
    pub panel: Option<Panel>,
    /// The eye poses the world layer's pictures were drawn for, when they
    /// were not drawn for this frame's own prediction (pictures made ahead
    /// of time, [`Frame::next_eyes`], or reused while their source
    /// stalls). The runtime shifts the picture to where the head really is
    /// (reprojection). `None`: this frame's prediction.
    pub eyes: Option<[EyePose; 2]>,
    /// Show the player's room behind the picture, where the headset can
    /// ([`XrContext::passthrough_available`]); [`Frame::passthrough`]
    /// tells whether this frame does.
    pub passthrough: bool,
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
    /// The room behind the picture (Quest; `None`: not available). First:
    /// a child of the session.
    passthrough: Option<crate::passthrough::Passthrough>,
    /// The display's refresh rates, Hz (`XR_FB_display_refresh_rate`;
    /// empty: not offered, the runtime keeps its own).
    refresh_rates: Vec<f32>,
    panel: Swapchain,
    /// Both eyes: one image with two array layers.
    eyes: Swapchain,
    space: xr::Space,
    /// The head (`VIEW`), located in `space` for [`Self::recentre`].
    head: xr::Space,
    seat: Seat,
    /// [`Self::recentre`] was asked for: done at the next frame.
    recentre: bool,
    frame_stream: xr::FrameStream<xr::Vulkan>,
    frame_waiter: xr::FrameWaiter,
    session: xr::Session<xr::Vulkan>,
    instance: xr::Instance,
    blend_mode: xr::EnvironmentBlendMode,
    format: wgpu::TextureFormat,
    running: bool,
    /// The session has input focus (state FOCUSED): no system menu is in
    /// front. On a Quest this outlasts taking the headset off by about ten
    /// seconds, hence `present`.
    focused: bool,
    /// The headset is worn (`XR_EXT_user_presence`; true where the runtime
    /// does not report it).
    present: bool,
    /// Session state changes since the last [`Self::take_log`].
    log: Vec<String>,
    /// The runtime's clock can be read (`Instance::now`).
    has_clock: bool,
    /// How long the last frame waited for the queue lock.
    queue_wait: std::time::Duration,
    /// The Touch controllers as a gamepad (`None`: the runtime refused the
    /// actions; logged, the game still runs on a gamepad).
    controllers: Option<crate::controllers::Controllers>,
    /// Their state at the last frame ([`Self::pad`]).
    pad: crate::PadState,
    /// A read failed and was logged (once per session).
    pad_error: bool,
    gpu: XrGpu,
}

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

/// The loader library: `PW64_OPENXR_LOADER`, next to the exe, the dev
/// tree's `tools/openxr/`, then the system search path (Android: the APK's
/// `lib/arm64-v8a/libopenxr_loader.so`). `platform`: `()`, or on Android
/// the VM and activity the loader is initialised with
/// (`XR_KHR_loader_init_android`) as soon as it is loaded.
fn load_entry(platform: &impl xr::PlatformInfo) -> Result<xr::Entry, XrError> {
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
        match unsafe { xr::Entry::load_from(p, platform) } {
            Ok(e) => return Ok(e),
            Err(e) => last = format!("{}: {e}", p.display()),
        }
    }
    // SAFETY: as above, found by the OS loader.
    unsafe { xr::Entry::load(platform) }.map_err(|e| {
        if last == "not found" {
            last = e.to_string();
        }
        XrError::Loader(last)
    })
}

impl XrContext {
    /// Connects to the active OpenXR runtime and creates the wgpu device on
    /// the GPU it picks. `wanted_features`: see [`XrGpu`]. Android: `app`
    /// is the activity the session belongs to; call this once it exists.
    pub fn new(
        wanted_features: wgpu::Features,
        panel: Panel,
        #[cfg(target_os = "android")] app: &crate::AndroidApp,
    ) -> Result<Self, XrError> {
        // SAFETY: `AndroidApp::new`'s contract is this one's.
        #[cfg(target_os = "android")]
        let platform = unsafe { xr::AndroidPlatformInfo::new(app.java_vm, app.activity) };
        #[cfg(not(target_os = "android"))]
        let platform = ();
        let entry = load_entry(&platform)?;
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
        // Optional: tells us when the headset is taken off and put on.
        enabled.ext_user_presence = available.ext_user_presence;
        // Android: the instance is created for an activity.
        #[cfg(target_os = "android")]
        {
            if !available.khr_android_create_instance {
                return Err(XrError::Unsupported(
                    "Android support (XR_KHR_android_create_instance)".into(),
                ));
            }
            enabled.khr_android_create_instance = true;
        }
        // Optional, standalone headsets only (Quest Link is left as it
        // is): the room behind the menus. Quest offers it only to apps
        // whose manifest has the PASSTHROUGH feature.
        enabled.fb_passthrough = cfg!(target_os = "android") && available.fb_passthrough;
        // Optional, standalone headsets only: the CPU's clock level (q6).
        enabled.ext_performance_settings =
            cfg!(target_os = "android") && available.ext_performance_settings;
        // Optional, standalone headsets only (Quest Link sets its own): the
        // display's refresh rate.
        enabled.fb_display_refresh_rate =
            cfg!(target_os = "android") && available.fb_display_refresh_rate;
        // Optional: the runtime's clock, to see how far ahead it predicts
        // (`FrameTime::ahead_ns`).
        #[cfg(windows)]
        let has_clock = {
            enabled.khr_win32_convert_performance_counter_time =
                available.khr_win32_convert_performance_counter_time;
            available.khr_win32_convert_performance_counter_time
        };
        #[cfg(not(windows))]
        let has_clock = {
            enabled.khr_convert_timespec_time = available.khr_convert_timespec_time;
            available.khr_convert_timespec_time
        };
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
                &platform,
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
        let head =
            session.create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)?;
        let mut log = Vec::new();
        let passthrough = if enabled.fb_passthrough {
            match crate::passthrough::Passthrough::new(&instance, &session) {
                Ok(p) => {
                    log.push("passthrough: XR_FB_passthrough available, ready".into());
                    Some(p)
                }
                Err(e) => {
                    log.push(format!(
                        "passthrough: XR_FB_passthrough available, but not created: {e}"
                    ));
                    None
                }
            }
        } else {
            log.push(
                if available.fb_passthrough {
                    "passthrough: XR_FB_passthrough available, not used on PC"
                } else {
                    "passthrough: XR_FB_passthrough not available"
                }
                .into(),
            );
            None
        };
        // q6: the Quest picks the CPU clock by how busy the cores are; the
        // window thread prepares both eye pictures in one go and stays
        // under the bar, so the CPU often ran at 1.4 GHz (level 2) and the
        // game fell to 35-50 fps with Infinite LOD. Sustained high asks for
        // a higher clock it can hold without overheating; the GPU stays on
        // the runtime's own choice.
        if enabled.ext_performance_settings {
            let level = xr::sys::PerfSettingsLevelEXT::SUSTAINED_HIGH;
            log.push(
                match instance.exts().ext_performance_settings.map(|p| {
                    // SAFETY: the extension is enabled; a live session handle.
                    unsafe {
                        (p.perf_settings_set_performance_level)(
                            session.as_raw(),
                            xr::sys::PerfSettingsDomainEXT::CPU,
                            level,
                        )
                    }
                }) {
                    Some(r) if r.into_raw() >= 0 => "performance: CPU level sustained high".into(),
                    Some(r) => format!("performance: CPU level not set ({r:?})"),
                    None => "performance: XR_EXT_performance_settings not loaded".into(),
                },
            );
        }
        // The refresh rates the display offers (none: the runtime's own).
        let refresh_rates = if enabled.fb_display_refresh_rate {
            let rates = session
                .enumerate_display_refresh_rates()
                .unwrap_or_default();
            let now = session.get_display_refresh_rate().unwrap_or(0.0);
            log.push(format!(
                "refresh rates: {} Hz (now {now:.0} Hz)",
                rates
                    .iter()
                    .map(|r| format!("{r:.0}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            rates
        } else {
            Vec::new()
        };
        let controllers = match crate::controllers::Controllers::new(&instance, &session) {
            Ok(c) => Some(c),
            Err(e) => {
                log.push(format!("Touch controllers unavailable: {e}"));
                None
            }
        };

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
            passthrough,
            panel,
            eyes,
            space,
            head,
            seat: Seat::default(),
            recentre: false,
            frame_stream,
            frame_waiter,
            session,
            instance,
            blend_mode,
            format,
            running: false,
            focused: false,
            present: true,
            log,
            has_clock,
            queue_wait: std::time::Duration::ZERO,
            controllers,
            pad: crate::PadState::default(),
            pad_error: false,
            refresh_rates,
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

    /// How long the last [`Self::frame`] waited for its `queue_lock`.
    pub fn last_queue_wait(&self) -> std::time::Duration {
        self.queue_wait
    }

    /// Ends the session the way the runtime expects before the process
    /// exits: ask to exit, wait (at most a second) for the runtime to stop
    /// the session, then destroy everything. Exiting with a live session
    /// made the Quest Link runtime time out in its own shutdown and abort
    /// the process.
    pub fn shutdown(mut self) {
        if self.running && self.session.request_exit().is_ok() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while self.running && std::time::Instant::now() < deadline {
                // `poll_events` ends the session on STOPPING.
                if !matches!(self.poll_events(), Ok(true)) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        // Drops in field order: swapchains, space, session, instance, then
        // our references to the wgpu device.
    }

    /// The refresh rates the display offers, Hz (empty: no choice here).
    pub fn refresh_rates(&self) -> &[f32] {
        &self.refresh_rates
    }

    /// Asks the display for `hz` (one of [`Self::refresh_rates`]; 0: the
    /// runtime's own choice). The frame period follows within a few frames
    /// ([`FrameTime::period_ns`]). Logged.
    pub fn request_refresh_rate(&mut self, hz: f32) {
        if self.refresh_rates.is_empty() {
            return;
        }
        let line = match self.session.request_display_refresh_rate(hz) {
            Ok(()) if hz == 0.0 => "refresh rate: the headset's own".to_string(),
            Ok(()) => format!("refresh rate: {hz:.0} Hz"),
            Err(e) => format!("refresh rate {hz:.0} Hz not set: {e}"),
        };
        self.log.push(line);
    }

    /// The headset can show the player's room behind the picture
    /// ([`Layers::passthrough`]).
    pub fn passthrough_available(&self) -> bool {
        self.passthrough.is_some()
    }

    /// Turns passthrough on or off for the frame being made; `true`: on.
    /// A failure is logged and ends passthrough for this session.
    fn set_passthrough(&mut self, on: bool) -> bool {
        let Some(p) = &mut self.passthrough else {
            return false;
        };
        if p.on() == on {
            return on;
        }
        match p.set(on) {
            Ok(()) => {
                self.log
                    .push(format!("passthrough {}", if on { "on" } else { "off" }));
                on
            }
            Err(e) => {
                self.log.push(format!(
                    "passthrough: turning it {} failed ({e}); not used any more",
                    if on { "on" } else { "off" }
                ));
                self.passthrough = None;
                false
            }
        }
    }

    /// One eye's render size in pixels (the runtime's recommendation).
    pub fn eye_size(&self) -> (u32, u32) {
        self.eyes.size_px
    }

    /// The session has the player's attention: the headset is worn and no
    /// system menu is in front of the app.
    pub fn focused(&self) -> bool {
        self.focused && self.present
    }

    /// Moves the seat to where the head is at the next frame: forward
    /// becomes the way the player faces (level), and the seated origin the
    /// head's position, so the game's camera is at the player's eyes again.
    pub fn recentre(&mut self) {
        self.recentre = true;
    }

    /// The Touch controllers' state at the last frame (idle without input
    /// focus or controllers).
    pub fn pad(&self) -> crate::PadState {
        self.pad
    }

    /// The seat now ([`Seat`]).
    pub fn seat(&self) -> Seat {
        self.seat
    }

    /// One line per session state change since the last call, for logs.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
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
                    self.log.push(format!("session state {:?}", e.state()));
                    self.focused = e.state() == xr::SessionState::FOCUSED;
                    match e.state() {
                        xr::SessionState::READY => {
                            self.session.begin(VIEW_TYPE)?;
                            self.running = true;
                        }
                        xr::SessionState::STOPPING => {
                            // Started again by the first frame that
                            // wants it after the session resumes.
                            if let Some(p) = self.passthrough.as_mut().filter(|p| p.on()) {
                                p.stopped();
                                self.log.push("passthrough off (session stopping)".into());
                            }
                            self.session.end()?;
                            self.running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            return Ok(false);
                        }
                        _ => {}
                    }
                }
                UserPresenceChangedEXT(e) => {
                    self.present = e.is_user_present();
                    self.log.push(format!(
                        "headset {}",
                        if self.present { "put on" } else { "taken off" }
                    ));
                }
                // The system recentred (long press of the Meta button):
                // `LOCAL` itself now starts at the head, so our own offset
                // from it goes.
                ReferenceSpaceChangePending(e)
                    if e.reference_space_type() == xr::ReferenceSpaceType::LOCAL =>
                {
                    self.seat = Seat::default();
                    self.log.push("the system recentred the view".into());
                }
                InstanceLossPending(_) => {
                    self.log.push("instance loss pending".into());
                    return Ok(false);
                }
                _ => {}
            }
        }
        Ok(true)
    }

    /// One headset frame: blocks until the runtime wants it (its display
    /// rate), then shows `layers()`, asked for only then so it can name the
    /// newest pictures ([`Layers::eyes`]). `draw` renders into the targets in its
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
        layers: impl FnOnce() -> Layers,
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
        // The Touch controllers: once per headset frame, only with input
        // focus (the runtime delivers none otherwise: idle).
        self.pad = match &self.controllers {
            Some(c) if self.focused => c.read(&self.session).unwrap_or_else(|e| {
                if !std::mem::replace(&mut self.pad_error, true) {
                    self.log.push(format!("Touch controllers: {e}"));
                }
                crate::PadState::default()
            }),
            _ => crate::PadState::default(),
        };
        if std::mem::take(&mut self.recentre) {
            let flags =
                xr::SpaceLocationFlags::ORIENTATION_VALID | xr::SpaceLocationFlags::POSITION_VALID;
            match self.head.locate(&self.space, state.predicted_display_time) {
                Ok(l) if l.location_flags.contains(flags) => {
                    let (o, p) = (l.pose.orientation, l.pose.position);
                    self.seat = Seat::at_head([o.x, o.y, o.z, o.w], [p.x, p.y, p.z]);
                    self.log.push("recentred".into());
                }
                _ => self.log.push("recentre: the head is not tracked".into()),
            }
        }
        let layers = layers();
        let passthrough = self.set_passthrough(layers.passthrough);
        let waiting = std::time::Instant::now();
        let _queue = queue_lock.map(|m| m.lock().unwrap_or_else(|e| e.into_inner()));
        self.queue_wait = waiting.elapsed();
        self.frame_stream.begin()?;
        if !state.should_render {
            self.frame_stream
                .end(state.predicted_display_time, self.blend_mode, &[])?;
            return Ok(FrameOutcome::Hidden);
        }
        // Where the eyes will be when this frame is shown.
        let (located, next_eyes) = if layers.world {
            let locate = |time| {
                self.session
                    .locate_views(VIEW_TYPE, time, &self.space)
                    .ok()
                    .filter(|(_, views)| views.len() == 2)
                    .map(|(_, views)| [0, 1].map(|i| EyePose::from_view(&views[i])))
            };
            let now = match layers.eyes {
                Some(eyes) => Some(eyes),
                None => locate(state.predicted_display_time),
            };
            let next = xr::Time::from_nanos(
                state.predicted_display_time.as_nanos() + state.predicted_display_period.as_nanos(),
            );
            (now, locate(next))
        } else {
            (None, None)
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
                let e = views[i];
                let s = self.seat.seated(&e);
                Eye {
                    target: &self.eyes.views[image][i],
                    view: math::view(s.orientation, s.position),
                    projection: math::projection(e.fov[0], e.fov[1], e.fov[2], e.fov[3], NEAR, FAR),
                    pose: e,
                }
            })
        });
        draw(Frame {
            time: FrameTime {
                display_ns: state.predicted_display_time.as_nanos(),
                period_ns: state.predicted_display_period.as_nanos(),
                ahead_ns: self
                    .has_clock
                    .then(|| self.instance.now().ok())
                    .flatten()
                    .map(|now| state.predicted_display_time.as_nanos() - now.as_nanos()),
            },
            panel: panel_image.map(|i| &self.panel.views[i][0]),
            eyes,
            next_eyes,
            seat: self.seat,
            passthrough,
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
                    .pose(views[i].pose())
                    .fov(views[i].fov())
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
                // Over the room: by the picture's alpha (premultiplied).
                .layer_flags(if passthrough {
                    xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA
                } else {
                    xr::CompositionLayerFlags::EMPTY
                })
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
                .pose(self.seat.place(
                    [0.0, 0.0, 0.0, 1.0],
                    [0.0, panel.height_m, -panel.distance_m],
                ))
                .size(xr::Extent2Df {
                    width: panel.width_m,
                    height: panel.width_m * h as f32 / w as f32,
                })
        });
        // Back to front: the room (passthrough), the world, then the
        // panel over it.
        let room_layer = self
            .passthrough
            .as_ref()
            .filter(|_| passthrough)
            .map(|p| p.layer());
        let mut submit: Vec<&xr::CompositionLayerBase<'_, xr::Vulkan>> = Vec::new();
        if let Some(l) = &room_layer {
            submit.push(l.base());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_seat_at_the_head_puts_the_head_at_the_seated_origin_facing_forward() {
        // Turned 90 degrees left and looking down a little, 0.3 m right
        // and 0.2 m lower than where the session started.
        let yaw = std::f32::consts::FRAC_PI_4;
        let turn = [0.0, yaw.sin(), 0.0, yaw.cos()];
        let pitch = [(-0.2f32).sin(), 0.0, 0.0, (-0.2f32).cos()];
        let head = EyePose {
            orientation: math::quat_mul(turn, pitch),
            position: [0.3, -0.2, 0.0],
            fov: [-0.8, 0.8, 0.8, -0.8],
        };
        let seat = Seat::at_head(head.orientation, head.position);
        let s = seat.seated(&head);
        assert!(s.position.iter().all(|&v| close(v, 0.0)));
        // Only the look down is left: ahead is -z, tilted down.
        let ahead = math::rotate(s.orientation, [0.0, 0.0, -1.0]);
        assert!(close(ahead[0], 0.0) && ahead[1] < -0.3 && ahead[2] < 0.0);
        assert_eq!(s.fov, head.fov);
        // Something ahead in the seated space is ahead of the head in LOCAL.
        let p = seat.place([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, -2.5]).position;
        assert!(close(p.x, 0.3 - 2.5) && close(p.y, -0.2) && close(p.z, 0.0));
    }
}
