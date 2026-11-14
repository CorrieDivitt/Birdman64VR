//! Headset smoke test, no game involved: a floating test-pattern panel.
//!
//! Start the VR runtime first (Quest Link), then:
//! `cargo run --release -p pw64-xr --example panel -- <mode>`
//!
//! Modes (how the same pattern reaches the headset):
//! - `quad` (default): an OpenXR quad layer only, nothing behind it.
//! - `both`: the quad layer over an empty dark stereo world layer.
//! - `world`: no quad layer; the panel is drawn as a rectangle inside the
//!   stereo world layer, once per eye.
//!
//! The pattern is a 4:3 panel straight ahead with three bands:
//! - top: a grid and a fast orange bar timed from the PC clock at draw time;
//! - middle: plain, a fast green bar timed from the predicted display time;
//! - bottom: plain, a slow white bar (20 s per sweep, about one pixel per
//!   frame at 72 Hz) timed from the predicted display time.
//!
//! Ctrl+C or quitting from the headset's menu ends it.

use pw64_xr::math::{self, Mat4};
use pw64_xr::{FrameOutcome, Layers, Panel, XrContext};
use std::time::{Duration, Instant};

const SHADER: &str = r"
struct U { mvp: mat4x4<f32>, wall: f32, display: f32 }
@group(0) @binding(0) var<uniform> u: U;

struct V { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }

@vertex
fn vs(@builtin(vertex_index) i: u32) -> V {
    // A unit rectangle (two triangles), uv (0,0) at the top left.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0));
    let uv = corners[i];
    var o: V;
    o.pos = u.mvp * vec4<f32>(uv.x - 0.5, 0.5 - uv.y, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@fragment
fn fs(v: V) -> @location(0) vec4<f32> {
    let size = vec2<f32>(1440.0, 1080.0);
    let uv = v.uv;
    let pos = uv * size;
    var c = mix(vec3<f32>(0.10, 0.35, 0.80), vec3<f32>(0.02, 0.10, 0.30), uv.y);
    if (uv.y < 0.34) {
        let cell = fract(pos / 120.0) * 120.0;
        if (cell.x < 2.0 || cell.y < 2.0) { c = vec3<f32>(0.9, 0.9, 0.9); }
        if (abs(uv.x - fract(u.wall * 0.5)) < 0.01) { c = vec3<f32>(1.0, 0.45, 0.1); }
    } else if (uv.y < 0.67) {
        c = vec3<f32>(0.03, 0.08, 0.20);
        if (abs(uv.x - fract(u.display * 0.5)) < 0.01) { c = vec3<f32>(0.2, 1.0, 0.3); }
    } else {
        c = vec3<f32>(0.02, 0.05, 0.12);
        if (abs(uv.x - fract(u.display * 0.05)) < 0.01) { c = vec3<f32>(1.0, 1.0, 1.0); }
    }
    let edge = min(min(pos.x, pos.y), min(size.x - pos.x, size.y - pos.y));
    if (edge < 12.0) { c = vec3<f32>(1.0, 1.0, 1.0); }
    return vec4<f32>(c, 1.0);
}
";

/// The `U` uniform block: mvp (64 bytes), wall, display, padding.
fn uniform_bytes(mvp: &Mat4, wall: f32, display: f32) -> [u8; 80] {
    let mut b = [0u8; 80];
    for (i, v) in mvp.iter().flatten().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
    }
    b[64..68].copy_from_slice(&wall.to_ne_bytes());
    b[68..72].copy_from_slice(&display.to_ne_bytes());
    b
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "quad".into());
    let panel = Panel::default();
    let layers = match mode.as_str() {
        "quad" => Layers {
            world: false,
            panel: Some(panel),
        },
        "both" => Layers {
            world: true,
            panel: Some(panel),
        },
        "world" => Layers {
            world: true,
            panel: None,
        },
        other => {
            eprintln!("unknown mode {other:?}: use quad, both or world");
            std::process::exit(2);
        }
    };
    let mut xr = match XrContext::new(wgpu::Features::empty(), panel) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("VR is not available: {e}");
            std::process::exit(1);
        }
    };
    let info = xr.gpu().adapter.get_info();
    println!("mode: {mode}");
    println!("runtime: {}", xr.runtime());
    println!(
        "GPU: {} ({:?}), format {:?}, eye size {:?}",
        info.name,
        info.backend,
        xr.format(),
        xr.eye_size()
    );

    let device = xr.gpu().device.clone();
    let queue = xr.gpu().queue.clone();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
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
    // One uniform block per target: the panel texture, left eye, right eye.
    let slots: Vec<(wgpu::Buffer, wgpu::BindGroup)> = (0..3)
        .map(|_| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 80,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            (buffer, group)
        })
        .collect();
    // The panel texture is filled edge to edge; in the world it is a
    // rectangle where the quad layer would be.
    let fill = math::translate_scale([0.0; 3], [2.0, 2.0, 1.0]);
    let (w, h) = panel.size_px;
    let in_world = math::translate_scale(
        [0.0, panel.height_m, -panel.distance_m],
        [panel.width_m, panel.width_m * h as f32 / w as f32, 1.0],
    );

    let start = Instant::now();
    let (mut frames, mut last_report, mut shown) = (0u32, Instant::now(), false);
    // Frame-to-frame steps of both clocks, in ms: (min, max) per report.
    let mut first_display: Option<i64> = None;
    let mut prev: Option<(f64, i64)> = None;
    let mut wall_step = (f64::MAX, 0f64);
    let mut display_step = (f64::MAX, 0f64);
    let mut period_ms = 0.0;
    loop {
        let outcome = xr.frame(&layers, None, |frame| {
            let time = frame.time;
            let wall = start.elapsed().as_secs_f64();
            let t0 = *first_display.get_or_insert(time.display_ns);
            let display = (time.display_ns - t0) as f64 / 1e9;
            if let Some((w, d)) = prev {
                let (ws, ds) = ((wall - w) * 1e3, (time.display_ns - d) as f64 / 1e6);
                wall_step = (wall_step.0.min(ws), wall_step.1.max(ws));
                display_step = (display_step.0.min(ds), display_step.1.max(ds));
            }
            prev = Some((wall, time.display_ns));
            period_ms = time.period_ns as f64 / 1e6;

            // (target, uniform slot, mvp or None = clear only, clear colour)
            let dark = wgpu::Color {
                r: 0.0,
                g: 0.004,
                b: 0.012,
                a: 1.0,
            };
            let mut passes: Vec<(&wgpu::TextureView, usize, Option<Mat4>)> = Vec::new();
            if let Some(view) = frame.panel {
                passes.push((view, 0, Some(fill)));
            }
            if let Some(eyes) = &frame.eyes {
                for (i, eye) in eyes.iter().enumerate() {
                    let mvp = layers
                        .panel
                        .is_none()
                        .then(|| math::mul(&eye.projection, &math::mul(&eye.view, &in_world)));
                    passes.push((eye.target, 1 + i, mvp));
                }
            }
            let mut encoder = device.create_command_encoder(&Default::default());
            for (target, slot, mvp) in passes {
                if let Some(mvp) = &mvp {
                    let bytes = uniform_bytes(mvp, wall as f32, display as f32);
                    queue.write_buffer(&slots[slot].0, 0, &bytes);
                }
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(dark),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                if mvp.is_some() {
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &slots[slot].1, &[]);
                    pass.draw(0..6, 0..1);
                }
            }
            queue.submit([encoder.finish()]);
        });
        match outcome {
            Ok(FrameOutcome::Rendered) => {
                frames += 1;
                if !shown {
                    println!("the panel is showing in the headset");
                    shown = true;
                }
            }
            Ok(FrameOutcome::Hidden) => {}
            Ok(FrameOutcome::Idle) => std::thread::sleep(Duration::from_millis(50)),
            Ok(FrameOutcome::Quit) => {
                println!("the VR runtime asked to quit");
                break;
            }
            Err(e) => {
                eprintln!("VR stopped: {e}");
                std::process::exit(1);
            }
        }
        if last_report.elapsed() >= Duration::from_secs(5) {
            println!(
                "{:.0} fps | period {period_ms:.1} ms | step between frames, min..max: \
                 PC clock {:.1}..{:.1} ms, display time {:.1}..{:.1} ms",
                frames as f32 / last_report.elapsed().as_secs_f32(),
                wall_step.0,
                wall_step.1,
                display_step.0,
                display_step.1
            );
            (frames, last_report) = (0, Instant::now());
            (wall_step, display_step) = ((f64::MAX, 0.0), (f64::MAX, 0.0));
        }
    }
}
