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

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);
/// The headset's refresh rate in mHz once frames flow (0: not known).
static HEADSET_MHZ: AtomicU32 = AtomicU32::new(0);

/// `--vr` on the command line (lib.rs `desktop_main`).
pub(crate) fn set_requested(on: bool) {
    REQUESTED.store(on, Ordering::Relaxed);
}

/// VR was asked for: `--vr`, or `PW64_VR` set to anything but `0`.
pub(crate) fn requested() -> bool {
    REQUESTED.load(Ordering::Relaxed) || std::env::var_os("PW64_VR").is_some_and(|v| v != "0")
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

#[cfg(any(windows, target_os = "linux"))]
pub(crate) use real::{Vr, gate};
#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) use stub::{Vr, gate};

#[cfg(not(any(windows, target_os = "linux")))]
mod stub {
    use crate::gpu_context::GpuContext;
    use std::sync::Arc;
    use winit::window::Window;

    pub(crate) struct Vr;

    pub(crate) fn gate() -> Option<std::sync::MutexGuard<'static, ()>> {
        None
    }

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
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::Duration;
    use winit::window::Window;

    /// Held by the headset thread while it draws and submits, and by the
    /// window thread around a surface reconfigure: wgpu wants no
    /// submissions in flight then (window.rs `refresh_present`).
    static GATE: Mutex<()> = Mutex::new(());

    /// See [`GATE`]. `None` when VR is not running.
    pub(crate) fn gate() -> Option<MutexGuard<'static, ()>> {
        super::headset_mhz()
            .is_some()
            .then(|| GATE.lock().unwrap_or_else(|e| e.into_inner()))
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
            eprintln!(
                "[vr] {} on {}, eye {:?}",
                xr.runtime(),
                gpu.adapter.get_info().name,
                xr.eye_size()
            );
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
                if let Err(e) = spawned {
                    eprintln!("[vr] could not start the headset thread: {e}");
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
        loop {
            let outcome = xr.frame(&layers, |frame| {
                let _gate = GATE.lock().unwrap_or_else(|e| e.into_inner());
                let mhz = (1e12 / frame.time.period_ns.max(1) as f64).round() as u32;
                HEADSET_MHZ.store(mhz.max(1), Ordering::Relaxed);
                let Some(eyes) = frame.eyes else { return };
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
                queue.submit([encoder.finish()]);
            });
            match outcome {
                Ok(FrameOutcome::Rendered) => {
                    if !announced {
                        announced = true;
                        eprintln!(
                            "[vr] showing in the headset at {:.0} Hz",
                            HEADSET_MHZ.load(Ordering::Relaxed) as f64 / 1000.0
                        );
                    }
                }
                Ok(FrameOutcome::Hidden) => {}
                Ok(FrameOutcome::Idle) => std::thread::sleep(Duration::from_millis(50)),
                Ok(FrameOutcome::Quit) => {
                    eprintln!("[vr] the VR runtime ended the session; quitting");
                    crate::window::request_quit();
                    break;
                }
                Err(e) => {
                    eprintln!("[vr] stopped: {e}");
                    crate::window::notify_toast(format!(
                        "VR stopped ({e}). The game continues on the monitor."
                    ));
                    break;
                }
            }
        }
        // The game's rate goes back to the monitor's.
        HEADSET_MHZ.store(0, Ordering::Relaxed);
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
