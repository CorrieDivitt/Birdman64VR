//! Headset smoke test, no game involved: a floating test-pattern panel.
//!
//! Start the VR runtime first (Quest Link), then:
//! `cargo run --release -p pw64-xr --example panel`
//!
//! Expect a 4:3 panel straight ahead with a grid and two bars sweeping left
//! to right every two seconds: the top one (orange) follows the PC clock at
//! draw time, the bottom one (green) the display time the runtime predicts.
//! The green one is the correct way and should be smooth. Ctrl+C or quitting from the headset's
//! menu ends it.

use pw64_xr::{FrameOutcome, FrameTime, Panel, XrContext};
use std::time::{Duration, Instant};

const SHADER: &str = r"
struct U { wall: f32, display: f32 }
@group(0) @binding(0) var<uniform> u: U;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the whole target.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(1440.0, 1080.0);
    let uv = pos.xy / size;
    // Sky-to-sea gradient, a 12 x 9 grid, a white frame and a sweeping bar.
    var c = mix(vec3<f32>(0.10, 0.35, 0.80), vec3<f32>(0.02, 0.10, 0.30), uv.y);
    let cell = fract(pos.xy / 120.0) * 120.0;
    if (cell.x < 2.0 || cell.y < 2.0) { c = vec3<f32>(0.9, 0.9, 0.9); }
    let edge = min(min(pos.x, pos.y), min(size.x - pos.x, size.y - pos.y));
    if (edge < 12.0) { c = vec3<f32>(1.0, 1.0, 1.0); }
    if (uv.y < 0.5) {
        if (abs(uv.x - fract(u.wall * 0.5)) < 0.01) { c = vec3<f32>(1.0, 0.45, 0.1); }
    } else {
        if (abs(uv.x - fract(u.display * 0.5)) < 0.01) { c = vec3<f32>(0.2, 1.0, 0.3); }
    }
    return vec4<f32>(c, 1.0);
}
";

fn main() {
    let panel = Panel::default();
    let mut xr = match XrContext::new(wgpu::Features::empty(), panel) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("VR is not available: {e}");
            std::process::exit(1);
        }
    };
    let info = xr.gpu().adapter.get_info();
    println!("runtime: {}", xr.runtime());
    println!(
        "GPU: {} ({:?}), panel format {:?}, {} swapchain images",
        info.name,
        info.backend,
        xr.panel_format(),
        xr.panel_image_count()
    );

    let device = xr.gpu().device.clone();
    let queue = xr.gpu().queue.clone();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
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
            targets: &[Some(xr.panel_format().into())],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniforms.as_entire_binding(),
        }],
    });

    let start = Instant::now();
    let (mut frames, mut last_report, mut shown) = (0u32, Instant::now(), false);
    // Frame-to-frame steps of both clocks, in ms: (min, max) per report.
    let mut first_display: Option<i64> = None;
    let mut prev: Option<(f64, i64)> = None;
    let mut wall_step = (f64::MAX, 0f64);
    let mut display_step = (f64::MAX, 0f64);
    let mut period_ms = 0.0;
    loop {
        let outcome = xr.frame(&panel, |view, time: FrameTime| {
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
            let mut data = [0u8; 16];
            data[..4].copy_from_slice(&(wall as f32).to_ne_bytes());
            data[4..8].copy_from_slice(&(display as f32).to_ne_bytes());
            queue.write_buffer(&uniforms, 0, &data);
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..3, 0..1);
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
