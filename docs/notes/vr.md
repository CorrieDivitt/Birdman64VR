# VR (Birdman64VR): `crates/pw64-xr`

Status: `--vr` (or `PW64_VR=1`) shows the flat game on a panel in the headset
(virtual screen); the world is not in stereo yet. `pw64-xr` = OpenXR session,
Vulkan device through the runtime, a stereo world layer and an optional
quad-layer panel.

## Game hookup (`crates/birdman64/src/vr.rs`, window.rs)
- `App::start`: `vr::requested()` → `Vr::connect` builds the `GpuContext` on
  the runtime's device (the window surface is created from the same wgpu
  instance; wgpu-hal's `desired_extensions` include the surface ones). Any
  error → log + toast + the flat path.
- **Panel texture:** `App::redraw` draws the frame, toasts and the settings
  overlay into `Vr::target` (surface format and size, so every pass is
  unchanged) instead of the surface, then blits it to the surface (desktop
  mirror). ROM/setup screens still draw to the surface only.
- **Headset thread** (`pw64-xr`, started by the first `target` call): every
  headset frame it draws the latest panel texture as a rectangle in both eyes
  (2.175 m high, 2.5 m ahead, width from the aspect) with the predicted head
  pose. It never waits for the game or window. Panel values are
  display-encoded in a non-sRGB format, the eye target is sRGB: the shader
  decodes.
- **Pacing:** display pacing with the headset as the display: in VR
  `opts::present_rate` gets `vsync = true` (`PresentRate::Display`) and the
  headset thread ticks `os::vi::VBLANK` at the start of each headset frame
  (the window's present doesn't in VR), so the game makes one frame per
  headset frame, in step, shown one headset frame later. The surface's
  present mode stays non-blocking (V-Sync off). `vr::headset_mhz()` (from
  the frame period) overrides `monitor_mhz` for the fallback cadence (headset
  off the head). First version: `Fixed` at the headset's rate, free-running.
  With every stage on time (`vr.log`: headset 72 fps, gaps ≤ 14.4 ms, game 72
  fps, nothing waiting) play still stuttered for 1 to 2 s about every 15 s:
  the two 72 Hz clocks drifting through each other.
- **Queue lock (`pw64_gfx::QUEUE_GATE`, `XrContext::frame`'s `queue_lock`):**
  `XR_KHR_vulkan_enable2` lets the runtime use our VkQueue in begin/end frame
  and swapchain acquire/release and forbids any other use meanwhile; wgpu
  only serialises itself. Without a lock the device was lost (`Parent device
  is lost` in present / poll) as soon as the settings overlay opened. The
  headset thread holds the lock from begin to end of a frame; every other
  queue use takes it per call: all submits go through `pw64_gfx::submit`,
  presents through `window::present`. (First version: the window thread held
  it for each whole event. The headset thread then waited 5 to 28 ms for it,
  missed frames, and play had slowdowns with lagging audio.)
- **Reconfigure:** `Gpu::configure_surface` takes the lock and polls the
  device idle first.
- **Exit:** `window::quit` calls `vr::shutdown`: stops the headset thread,
  which runs `XrContext::shutdown` (`request_exit`, wait for STOPPING, `end`,
  destroy). Exiting with a live session made Quest Link's runtime time out
  in its own shutdown and abort the process (0xc0000409).
- **`vr.log`** (data folder, new per run): every `[vr]` line, plus one line
  every 5 s: headset fps, frame period, longest gap between headset frames,
  longest wait for the queue lock, game fps (window presents) and the
  longest game draw (fb op replay), surface acquire, frame draw, present and
  game-thread backpressure wait. Lines with a hitch also go to the console.
- A minimised window stops redraws, so the headset picture freezes.

## Bring-up
- **Loader:** `openxr` crate 0.22, feature `loaded` only: `openxr_loader` is
  opened at run time (`session.rs load_entry`: `PW64_OPENXR_LOADER`, next to
  the exe, `tools/openxr/`, then the OS search path). No loader or no running
  runtime → `XrError`, the caller starts flat.
- **Dev loader:** `tools/openxr/openxr_loader.dll` (gitignored) = `x64/bin/`
  of Khronos' `openxr_loader_windows-1.1.49.zip` (OpenXR-SDK-Source release
  1.1.49; zip sha256 `21f0d7f2…a4e4c4`). Apache-2.0; the release zip will
  ship it next to the exe with its licence.
- **Device (`gpu.rs`):** `XR_KHR_vulkan_enable2`: the runtime creates the
  Vulkan instance and device from our create-infos and picks the physical
  device. The create-infos mirror wgpu-hal 27's own
  (`vulkan::Instance::desired_extensions`, `Adapter::required_device_extensions`
  + `physical_device_features().add_to_device_create`), then
  `Instance::from_raw` → `expose_adapter` → `device_from_raw` →
  `wgpu::Instance::from_hal` / `create_adapter_from_hal` /
  `create_device_from_hal`. wgpu-hal owns instance and device
  (`drop_callback` None). Vulkan only: it is wgpu's raw-handle path and the
  Quest's only API.
- **Swapchain images:** `vulkan::Device::texture_from_raw` with a no-op drop
  callback (the runtime owns the image) → `create_texture_from_hal`.
  Format: `R8G8B8A8_SRGB`, else `B8G8R8A8_SRGB`.
- **Frame (`XrContext::frame(&Layers, draw)`):** poll events (begin on
  READY, end on STOPPING, quit on EXITING / LOSS_PENDING) → `wait` →
  `begin` → `locate_views` (if the world layer is on) → `draw(Frame)` into
  the acquired images → `end` with, back to front, a
  `CompositionLayerProjection` (one swapchain, 2 array layers, the runtime's
  recommended eye size) and a `CompositionLayerQuad` panel, both in a `LOCAL`
  (seated) space. `should_render` false → `end` with no layers.
- **`Frame`:** `FrameTime` (predicted display time + period: animate from
  this, not the clock at draw time), the panel view, and per eye the target
  plus `math::view(pose)` / `math::projection(fov)` (column-major,
  right-handed, -z forward, depth 0..1; unit-tested).
- `XrContext` field order: OpenXR objects drop before the wgpu device.

## Headset findings (Quest 3, Quest Link, RTX 4090, 72 Hz)
- First run worked: panel visible, world-locked, 72 fps, 3 swapchain images.
- Moving content on a **quad-only** frame looks choppy although frames are
  drawn every 13.9 ms on both the PC clock and the predicted display time
  (so not draw timing), and a `device.poll(wait)` before
  `xrReleaseSwapchainImage` changed nothing (removed again). Khronos
  `hello_xr -g Vulkan2` (projection layer) is smooth on the same setup.
  Open: does a projection layer under the quad, or drawing the panel inside
  the projection layer, fix it? (`panel` example modes `both` / `world`.)
- The predicted display time re-syncs now and then (steps of 0 or hundreds
  of ms, e.g. around putting the headset on): clamp dt when the game uses it.

## Test
`cargo run --release -p pw64-xr --example panel -- <quad|both|world>` with
Quest Link running: a three-band pattern panel straight ahead (fast bar on
the PC clock, fast and slow bars on the display time), timing printed every
5 s. `quad` = quad layer only, `both` = quad over an empty world layer,
`world` = the panel drawn inside the world layer.
