# VR (Birdman64VR): `crates/pw64-xr`

Status: OpenXR session, Vulkan device through the runtime, a stereo world
layer and one quad-layer panel (`examples/panel.rs` test pattern). Not wired
into the game yet.

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
