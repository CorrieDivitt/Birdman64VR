//! Birdman64VR: the VR mode (`--vr` or `PW64_VR=1`; docs/notes/vr.md).
//!
//! For now a virtual screen: the window keeps working exactly as in the
//! flat game, but everything it would draw to its surface (game frame,
//! toasts, settings overlay) goes into a "panel" texture first. The window
//! shows a copy of it (the desktop mirror); a headset thread draws it as a
//! rectangle floating in front of the player, in both eyes, at the
//! headset's own rate (`pw64-xr`). The headset loop never waits for the
//! game or the window: it shows the latest panel content with the head
//! pose predicted for each frame.
//!
//! Only on Windows and Linux; other targets get a stub that is never
//! requested.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);
/// The headset's refresh rate in mHz once frames flow (0: not known).
static HEADSET_MHZ: AtomicU32 = AtomicU32::new(0);

/// A VR session exists on the game's device (set once, by `Vr::connect`).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// VR is running: the GPU queue is shared with the OpenXR runtime.
pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// `--vr` on the command line (lib.rs `desktop_main`).
pub(crate) fn set_requested(on: bool) {
    REQUESTED.store(on, Ordering::Relaxed);
}

/// VR was asked for: `--vr`, or `PW64_VR` set to anything but `0`.
pub(crate) fn requested() -> bool {
    REQUESTED.load(Ordering::Relaxed) || std::env::var_os("PW64_VR").is_some_and(|v| v != "0")
}

/// The headset paces the game's frames (window.rs `App::start`). Off with
/// `PW64_VR_PACING=fixed` (a test switch): the game then runs on its own
/// timer at the headset's rate, as it does on a monitor.
pub(crate) fn headset_paced() -> bool {
    static FIXED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    active()
        && !*FIXED.get_or_init(|| std::env::var_os("PW64_VR_PACING").is_some_and(|v| v == "fixed"))
}

/// The headset's refresh rate (mHz) while VR runs: the game's "monitor"
/// rate then (window.rs `monitor_mhz`), so it produces one frame per
/// headset frame.
pub(crate) fn headset_mhz() -> Option<u32> {
    match HEADSET_MHZ.load(Ordering::Relaxed) {
        0 => None,
        mhz => Some(mhz),
    }
}

/// Where the window and game threads spend time, for the headset thread's
/// hitch line: the longest duration of each stage since the last report.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
#[derive(Clone, Copy)]
pub(crate) enum Stage {
    /// Replaying the game's framebuffer ops (the actual game rendering).
    Drain,
    /// Getting the window surface's next image.
    Acquire,
    /// Showing the frame, toasts and overlay, and the mirror copy.
    Draw,
    /// Presenting to the window.
    Present,
    /// The game thread held back because the window had not caught up.
    GameWait,
    /// The time between two new game frames reaching the window.
    GameGap,
}

const STAGES: usize = 6;
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
const STAGE_NAMES: [&str; STAGES] = [
    "game draw",
    "window image",
    "frame draw",
    "window present",
    "game held back",
    "gap between game frames",
];
static STAGE_MAX_US: [AtomicU32; STAGES] = [const { AtomicU32::new(0) }; STAGES];

/// Records `d` for `stage` (only while VR runs).
pub(crate) fn note(stage: Stage, d: std::time::Duration) {
    if active() {
        let us = d.as_micros().min(u32::MAX as u128) as u32;
        STAGE_MAX_US[stage as usize].fetch_max(us, Ordering::Relaxed);
    }
}

/// New game frames that reached the window since the headset thread's
/// last report, and when the last one did.
static GAME_FRAMES: AtomicU32 = AtomicU32::new(0);
static LAST_GAME_FRAME: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// `n` new game frames reached the window (window.rs `drain`).
pub(crate) fn count_frames(n: u32) {
    if !active() {
        return;
    }
    GAME_FRAMES.fetch_add(n, Ordering::Relaxed);
    let now = std::time::Instant::now();
    let last = LAST_GAME_FRAME
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(now);
    if let Some(t) = last {
        note(Stage::GameGap, now - t);
    }
}

/// Latency log: a running average and maximum of a delay, in microseconds.
struct Delay {
    sum_us: AtomicU64,
    count: AtomicU32,
    max_us: AtomicU32,
}

impl Delay {
    const fn new() -> Self {
        Self {
            sum_us: AtomicU64::new(0),
            count: AtomicU32::new(0),
            max_us: AtomicU32::new(0),
        }
    }

    fn add(&self, d: std::time::Duration) {
        let us = d.as_micros().min(u32::MAX as u128) as u32;
        self.sum_us.fetch_add(us.into(), Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
    }

    /// (samples, average ms, max ms) since the last call.
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    fn take(&self) -> (u32, f64, f64) {
        let n = self.count.swap(0, Ordering::Relaxed);
        let sum = self.sum_us.swap(0, Ordering::Relaxed);
        let max = self.max_us.swap(0, Ordering::Relaxed);
        (n, sum as f64 / 1e3 / n.max(1) as f64, max as f64 / 1e3)
    }
}

/// From the game thread handing a frame over to it being in the panel.
static HANDOVER_TO_PANEL: Delay = Delay::new();
/// How old the panel's content is when the headset thread samples it.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
static PANEL_AGE: Delay = Delay::new();
/// How far ahead the runtime predicts the display of the frame being drawn.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
static AHEAD: Delay = Delay::new();
/// When the panel was last drawn.
static PANEL_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// The window drew a frame into the panel (window.rs `redraw`);
/// `handed_over`: when the game thread handed it over, for a new frame.
pub(crate) fn panel_updated(handed_over: Option<std::time::Instant>) {
    let now = std::time::Instant::now();
    if let Some(t) = handed_over {
        HANDOVER_TO_PANEL.add(now - t);
        // Only a new game frame resets the panel's age.
        *PANEL_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
    }
}

#[cfg(any(windows, target_os = "linux"))]
pub(crate) use real::{Vr, shutdown};
#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) use stub::{Vr, shutdown};

#[cfg(not(any(windows, target_os = "linux")))]
mod stub {
    use crate::gpu_context::GpuContext;
    use std::sync::Arc;
    use winit::window::Window;

    pub(crate) struct Vr;

    pub(crate) fn shutdown() {}

    impl Vr {
        pub(crate) fn connect(_: &Arc<Window>) -> Result<(GpuContext, Self), String> {
            Err("VR is not supported on this platform".into())
        }

        pub(crate) fn target(
            &mut self,
            _: &wgpu::Device,
            _: wgpu::TextureFormat,
            _: (u32, u32),
        ) -> Option<wgpu::TextureView> {
            None
        }
    }
}

#[cfg(any(windows, target_os = "linux"))]
mod real {
    use super::HEADSET_MHZ;
    use crate::gpu_context::GpuContext;
    use pw64_xr::math::{self, Mat4};
    use pw64_xr::{FrameOutcome, Layers, Panel, XrContext};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use winit::window::Window;

    /// `shutdown` asked the headset thread to end; it sets `DONE`.
    static STOP: AtomicBool = AtomicBool::new(false);
    static DONE: AtomicBool = AtomicBool::new(false);
    static STARTED: AtomicBool = AtomicBool::new(false);

    /// How long before the next headset frame the game is cued to show its
    /// newest frame (`PW64_VR_LEAD_MS`, default 6). The cue latches the
    /// frame the game finished and the window then draws it into the panel;
    /// that must be done when the headset thread samples the panel. A
    /// longer lead is safer against a late (repeated) frame, a shorter one
    /// has less input lag.
    fn lead() -> Duration {
        static LEAD: OnceLock<Duration> = OnceLock::new();
        *LEAD.get_or_init(|| {
            let ms = std::env::var("PW64_VR_LEAD_MS")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| (0.0..=50.0).contains(v))
                .unwrap_or(6.0);
            Duration::from_secs_f64(ms / 1e3)
        })
    }

    /// When the game's next cue is due (set per headset frame).
    static CUE_AT: Mutex<Option<Instant>> = Mutex::new(None);
    static CUE_SET: Condvar = Condvar::new();

    /// Ticks `os::vi::VBLANK` (the game's display-pacing cue, window.rs
    /// `App::start`) at the times the headset thread sets.
    fn cue_thread() {
        loop {
            let at = {
                let mut cue = CUE_AT.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(at) = cue.take() {
                        break at;
                    }
                    cue = CUE_SET.wait(cue).unwrap_or_else(|e| e.into_inner());
                }
            };
            std::thread::sleep(at.saturating_duration_since(Instant::now()));
            pw64_platform::os::vi::VBLANK.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `[vr]` lines go to stderr and to `vr.log` in the data folder (a new
    /// file per run), so a headset session can be looked at afterwards.
    fn log(line: &str, console: bool) {
        use std::io::Write;
        static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
        if console {
            eprintln!("[vr] {line}");
        }
        let file = FILE.get_or_init(|| {
            std::fs::File::create(crate::paths::data_dir().join("vr.log"))
                .ok()
                .map(Mutex::new)
        });
        if let Some(f) = file {
            let _ = writeln!(f.lock().unwrap_or_else(|e| e.into_inner()), "{line}");
        }
    }

    /// Before the process exits (window.rs `quit`): ends the VR session
    /// properly. Asks the headset thread to finish and waits for it, at
    /// most three seconds.
    pub(crate) fn shutdown() {
        if !STARTED.load(Ordering::Relaxed) {
            return;
        }
        STOP.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !DONE.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if !DONE.load(Ordering::Relaxed) {
            log("the headset thread did not stop in time", true);
        }
    }

    /// The panel's height in metres and its distance in front of the
    /// player (seated space); the width follows the picture's aspect. A
    /// 4:3 picture fills about 60 degrees.
    const PANEL_HEIGHT_M: f32 = 2.175;
    const PANEL_DISTANCE_M: f32 = 2.5;

    /// The panel texture the window draws into; `generation` changes when
    /// it is replaced (window resize).
    #[derive(Default)]
    struct Source {
        generation: u64,
        view: Option<wgpu::TextureView>,
    }

    pub(crate) struct Vr {
        /// Until the first [`Vr::target`] call starts the headset thread.
        xr: Option<XrContext>,
        source: Arc<Mutex<Source>>,
        panel: Option<(wgpu::Texture, wgpu::TextureView)>,
    }

    impl Vr {
        /// Connects to the OpenXR runtime and builds the window's GPU
        /// context on the device it picks. `Err`: a one-line reason; the
        /// caller starts flat.
        pub(crate) fn connect(window: &Arc<Window>) -> Result<(GpuContext, Self), String> {
            // The optional features pw64-gfx asks for (`device_descriptor`).
            let features = wgpu::Features::DEPTH32FLOAT_STENCIL8
                | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
            let xr = XrContext::new(features, Panel::default()).map_err(|e| e.to_string())?;
            let gpu = xr.gpu();
            let surface = gpu
                .instance
                .create_surface(window.clone())
                .map_err(|e| format!("no window surface on the headset's GPU: {e}"))?;
            if surface.get_capabilities(&gpu.adapter).formats.is_empty() {
                return Err("the headset's GPU cannot draw to this window".into());
            }
            log(
                &format!(
                    "{} on {}, eye {:?}",
                    xr.runtime(),
                    gpu.adapter.get_info().name,
                    xr.eye_size()
                ),
                true,
            );
            super::ACTIVE.store(true, Ordering::Relaxed);
            let context = GpuContext {
                instance: gpu.instance.clone(),
                surface,
                adapter: gpu.adapter.clone(),
                device: gpu.device.clone(),
                queue: gpu.queue.clone(),
            };
            Ok((
                context,
                Self {
                    xr: Some(xr),
                    source: Arc::default(),
                    panel: None,
                },
            ))
        }

        /// The texture the window draws its frame into instead of the
        /// surface (same format and size as the surface, so every pass
        /// works unchanged). Starts the headset thread on the first call.
        pub(crate) fn target(
            &mut self,
            device: &wgpu::Device,
            format: wgpu::TextureFormat,
            size: (u32, u32),
        ) -> Option<wgpu::TextureView> {
            let stale = self
                .panel
                .as_ref()
                .is_none_or(|(t, _)| (t.width(), t.height()) != size || t.format() != format);
            if stale {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("vr panel"),
                    size: wgpu::Extent3d {
                        width: size.0.max(1),
                        height: size.1.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                let mut s = self.source.lock().unwrap();
                s.generation += 1;
                s.view = Some(view.clone());
                drop(s);
                self.panel = Some((texture, view));
            }
            if let Some(xr) = self.xr.take() {
                let source = self.source.clone();
                let spawned = std::thread::Builder::new()
                    .name("pw64-xr".into())
                    .spawn(move || headset_thread(xr, source));
                STARTED.store(spawned.is_ok(), Ordering::Relaxed);
                if let Err(e) = spawned {
                    log(&format!("could not start the headset thread: {e}"), true);
                }
            }
            self.panel.as_ref().map(|(_, v)| v.clone())
        }
    }

    /// The rectangle shader: `SRGB_DECODE` is replaced by a decode when the
    /// eye target is sRGB but the panel holds display-encoded values in a
    /// plain format (the window's surface format).
    const SHADER: &str = r"
struct U { mvp: mat4x4<f32> }
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var t: texture_2d<f32>;
@group(0) @binding(2) var s: sampler;

struct V { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }

@vertex
fn vs(@builtin(vertex_index) i: u32) -> V {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0));
    let uv = corners[i];
    var o: V;
    o.pos = u.mvp * vec4<f32>(uv.x - 0.5, 0.5 - uv.y, 0.0, 1.0);
    o.uv = uv;
    return o;
}

fn decode(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(v: V) -> @location(0) vec4<f32> {
    let c = textureSample(t, s, v.uv).rgb;
    return vec4<f32>(SRGB_DECODE, 1.0);
}
";

    pub(super) fn shader(decode: bool) -> String {
        SHADER.replace("SRGB_DECODE", if decode { "decode(c)" } else { "c" })
    }

    fn mat_bytes(m: &Mat4) -> [u8; 64] {
        let mut b = [0u8; 64];
        for (i, v) in m.iter().flatten().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
        }
        b
    }

    /// Runs until the runtime ends the session or fails.
    fn headset_thread(mut xr: XrContext, source: Arc<Mutex<Source>>) {
        let _ = std::thread::Builder::new()
            .name("pw64-xr-cue".into())
            .spawn(cue_thread);
        let device = xr.gpu().device.clone();
        let queue = xr.gpu().queue.clone();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vr panel"),
            source: wgpu::ShaderSource::Wgsl(shader(xr.format().is_srgb()).into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vr panel"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(xr.format().into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vr panel"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = [0, 1].map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vr eye"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        // Per eye, for the current panel texture: (generation, aspect, groups).
        let mut bound: Option<(u64, f32, [wgpu::BindGroup; 2])> = None;
        let layers = Layers {
            world: true,
            panel: None,
        };
        let mut announced = false;
        // Hitch report: the longest gap between shown headset frames and
        // the longest waits for the queue lock, per five seconds.
        let (mut last_frame, mut last_report) = (None::<Instant>, Instant::now());
        let (mut worst_gap, mut worst_wait, mut shown) = (Duration::ZERO, Duration::ZERO, 0u32);
        let started = Instant::now();
        // Headset frames the runtime did not want drawn (headset off the head).
        let mut not_shown = 0u32;
        while !STOP.load(Ordering::Relaxed) {
            let outcome = xr.frame(&layers, Some(&pw64_gfx::QUEUE_GATE), |frame| {
                // A headset frame starts. Cue the game `lead()` before the
                // next one starts (display pacing, window.rs `App::start`):
                // its newest frame then reaches the panel just in time to
                // be sampled, and it starts the frame after.
                let period = Duration::from_nanos(frame.time.period_ns.max(0) as u64);
                if super::headset_paced() {
                    *CUE_AT.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(Instant::now() + period.saturating_sub(lead()));
                    CUE_SET.notify_one();
                }
                if let Some(ns) = frame.time.ahead_ns {
                    super::AHEAD.add(Duration::from_nanos(ns.max(0) as u64));
                }
                let mhz = (1e12 / frame.time.period_ns.max(1) as f64).round() as u32;
                HEADSET_MHZ.store(mhz.max(1), Ordering::Relaxed);
                let Some(eyes) = frame.eyes else { return };
                // Latency log: how old is the game frame we are about to show?
                if let Some(t) = *super::PANEL_AT.lock().unwrap_or_else(|e| e.into_inner()) {
                    super::PANEL_AGE.add(t.elapsed());
                }
                {
                    let s = source.lock().unwrap();
                    if bound.as_ref().map(|b| b.0) != Some(s.generation) {
                        bound = s.view.as_ref().map(|view| {
                            let t = view.texture();
                            let groups = [0, 1].map(|i| {
                                device.create_bind_group(&wgpu::BindGroupDescriptor {
                                    label: Some("vr panel"),
                                    layout: &pipeline.get_bind_group_layout(0),
                                    entries: &[
                                        wgpu::BindGroupEntry {
                                            binding: 0,
                                            resource: uniforms[i].as_entire_binding(),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::TextureView(view),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 2,
                                            resource: wgpu::BindingResource::Sampler(&sampler),
                                        },
                                    ],
                                })
                            });
                            let aspect = t.width() as f32 / t.height().max(1) as f32;
                            (s.generation, aspect, groups)
                        });
                    }
                }
                let mut encoder = device.create_command_encoder(&Default::default());
                for (i, eye) in eyes.iter().enumerate() {
                    if let Some((_, aspect, _)) = &bound {
                        let model = math::translate_scale(
                            [0.0, 0.0, -PANEL_DISTANCE_M],
                            [PANEL_HEIGHT_M * aspect, PANEL_HEIGHT_M, 1.0],
                        );
                        let mvp = math::mul(&eye.projection, &math::mul(&eye.view, &model));
                        queue.write_buffer(&uniforms[i], 0, &mat_bytes(&mvp));
                    }
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("vr eye"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: eye.target,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.0,
                                    g: 0.002,
                                    b: 0.006,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    if let Some((_, _, groups)) = &bound {
                        pass.set_pipeline(&pipeline);
                        pass.set_bind_group(0, &groups[i], &[]);
                        pass.draw(0..6, 0..1);
                    }
                }
                // Directly: this thread holds the queue lock for the frame.
                queue.submit([encoder.finish()]);
            });
            match outcome {
                Ok(FrameOutcome::Rendered) => {
                    let now = Instant::now();
                    if let Some(t) = last_frame.replace(now) {
                        worst_gap = worst_gap.max(now - t);
                    }
                    worst_wait = worst_wait.max(xr.last_queue_wait());
                    shown += 1;
                    if !announced {
                        announced = true;
                        log(
                            &format!(
                                "showing in the headset at {:.0} Hz, game paced by {}, lead {:.1} ms",
                                HEADSET_MHZ.load(Ordering::Relaxed) as f64 / 1000.0,
                                if super::headset_paced() {
                                    "the headset"
                                } else {
                                    "its own timer"
                                },
                                lead().as_secs_f64() * 1e3
                            ),
                            true,
                        );
                    }
                }
                Ok(FrameOutcome::Hidden) => {
                    last_frame = None;
                    not_shown += 1;
                }
                Ok(FrameOutcome::Idle) => {
                    last_frame = None;
                    not_shown += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(FrameOutcome::Quit) => {
                    log("the VR runtime ended the session; quitting", true);
                    crate::window::request_quit();
                    break;
                }
                Err(e) => {
                    log(&format!("stopped: {e}"), true);
                    crate::window::notify_toast(format!(
                        "VR stopped ({e}). The game continues on the monitor."
                    ));
                    break;
                }
            }
            // Every five seconds, one line to vr.log (and to the console
            // when something was off): rates, the longest gap between
            // headset frames, the longest wait for the queue lock and the
            // longest window / game stages (`Stage`).
            let since = last_report.elapsed();
            if since >= Duration::from_secs(5) {
                let secs = since.as_secs_f64();
                let period_ms = 1e6 / HEADSET_MHZ.load(Ordering::Relaxed).max(1) as f64;
                let ms = |d: Duration| d.as_secs_f64() * 1e3;
                let stages: Vec<f64> = super::STAGE_MAX_US
                    .iter()
                    .map(|a| a.swap(0, Ordering::Relaxed) as f64 / 1e3)
                    .collect();
                let detail: Vec<String> = super::STAGE_NAMES
                    .iter()
                    .zip(&stages)
                    .map(|(name, v)| format!("{name} {v:.1}"))
                    .collect();
                let game_fps = super::GAME_FRAMES.swap(0, Ordering::Relaxed) as f64 / secs;
                let hitch = ms(worst_gap) > period_ms * 1.8
                    || ms(worst_wait) > 4.0
                    || stages[super::Stage::GameWait as usize] > 4.0
                    || stages[super::Stage::GameGap as usize] > period_ms * 2.5;
                let (_, to_panel_avg, to_panel_max) = super::HANDOVER_TO_PANEL.take();
                let (_, age_avg, age_max) = super::PANEL_AGE.take();
                let (_, ahead_avg, ahead_max) = super::AHEAD.take();
                let idle = std::mem::take(&mut not_shown);
                log(
                    &format!(
                        "{}t {:.0} s: headset {:.0} fps (frame {period_ms:.1} ms, longest gap \
                         {:.1}, waited {:.1} for the queue), game {game_fps:.0} fps; game \
                         frame: hand-over to panel avg {to_panel_avg:.1} max \
                         {to_panel_max:.1} ms, age when the headset draws it avg {age_avg:.1} \
                         max {age_max:.1} ms, then shown {ahead_avg:.1} (max {ahead_max:.1}) ms \
                         later; headset frames not shown {idle}; longest, ms: {}",
                        if hitch { "hitch, " } else { "" },
                        started.elapsed().as_secs_f64(),
                        shown as f64 / secs,
                        ms(worst_gap),
                        ms(worst_wait),
                        detail.join(", ")
                    ),
                    hitch,
                );
                (worst_gap, worst_wait, shown, last_report) =
                    (Duration::ZERO, Duration::ZERO, 0u32, Instant::now());
            }
        }
        // The game's rate goes back to the monitor's.
        HEADSET_MHZ.store(0, Ordering::Relaxed);
        xr.shutdown();
        DONE.store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    mod tests {
        /// Both shader variants parse and validate (no GPU needed).
        #[test]
        fn shader_variants_are_valid_wgsl() {
            use wgpu::naga;
            for decode in [false, true] {
                let m = naga::front::wgsl::parse_str(&super::shader(decode)).unwrap();
                naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::empty(),
                )
                .validate(&m)
                .unwrap();
            }
        }
    }
}
