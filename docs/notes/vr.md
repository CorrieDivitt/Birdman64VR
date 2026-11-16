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
- **Input lag:** a tick latches the frame the game finished after the
  previous tick, then the window draws it into the panel and the next headset
  frame samples it. Ticking at the headset frame's start cost almost two
  periods before the sample (felt as bad input lag at 72 Hz, on top of the
  compositor and Link). The headset thread now schedules the tick
  `PW64_VR_LEAD_MS` (default 6) before the next headset frame (`cue_thread`),
  so the latched frame reaches the panel just before it is sampled: about
  one period + lead. Too short a lead shows as repeated frames.
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

## Stereo world (`PW64_VR_STEREO=0` turns it off)
The game's 3D world view drawn once per eye from the same display list;
menus and everything else stay on the panel.
- **Which draws:** in VR the C side gets `pw64_widescreen_aspect < 0`
  (`pw64_widescreen.c`): no widescreen, but the main world view is tagged
  (`WIDE_TAG_ON/OFF`) and culled with a frustum `vr::CULL_SCALE` (6) times the
  camera's both ways. Tagged perspective draws get `DrawCall::view`.
- **Camera space back from the vertices:** the interpreter keeps emitting
  `pos = v * modelview * P * S` (S = viewport mapping). Per projection +
  viewport it stores `(P*S)⁻¹` in `Frame::views` (`Interpreter::world_view`,
  signs normalised to x right, y up, -z forward), so `pos * views[i]` is the
  point in the game camera's space. No second interpreter pass, no new
  vertex data. Fog and lighting stay as the game computed them.
- **Eye pass** (`Renderer::render_stereo`): `u.eye = views[i] * view(eye) *
  projection_reversed(fov)`; the shader uses `u.eye * pos` when `u.vr.x != 0`
  (flat draws untouched; `DrawUniforms` grew, stride 512). The game camera is
  the seated origin: the head moves around it. A new render pass wherever the
  game cleared depth between world draws. Background = the game's last
  full-screen fill before the world.
- **HUD:** draws after the first world draw that are not world draws or
  full-screen fills, in a last pass over cleared depth, through a matrix
  that puts the N64 screen on the panel's plane (2.5 m). Scissors ignored.
  Not shown yet: fades and wipes (`Wide::Stretch`), 2D drawn before the world.
- **Threads:** the window thread draws both eyes when a new game frame is
  shown (`Vr::world`, window.rs `drain`), for the poses the headset thread
  predicted for its next frame (`Frame::next_eyes`), into one of three
  texture sets; the headset thread copies the published set into the
  swapchain and submits the projection layer with *those* poses
  (`Layers::eyes`), so the runtime reprojects when the game is late or
  paused. The panel (toasts, settings) is drawn over it by alpha.
- The headset thread picks the eye set after `xrWaitFrame` returns
  (`XrContext::frame`'s `layers` closure), so the game frame drawn during the
  wait is the one shown, with the pose it was drawn for. The 5 s report
  counts eye pictures shown new and shown again, and the longest
  "eye pictures" draw time (window thread, CPU).
- **Judder found on the Quest (2026-11-14):** game and headset both at 72 fps,
  yet 25-30% of headset frames showed the previous eye pictures: cue lead
  6 ms, but flat picture (2-3 ms) + eye pictures (4-5 ms CPU) took 7-8 ms,
  so the pictures were ready for some frames and late for others. Now the
  eyes are drawn before the flat picture and the lead follows the measured
  cue-to-published time (`STEREO_LEAD_US`: worst of the last 64 + 1 ms,
  6 ms to a frame less 1 ms).
- **Culling in VR is by distance only** for the main world view
  (`code_7150.c.patch`): side-plane normals zeroed, forward plane skipped,
  terrain footprint = a square around the camera out to the far plane.
  `CULL_SCALE` still widens the frustum corners but no longer decides
  anything.
- **Stereo only in flight** (`pw64_vr_flight`: `GAME_STATE_TEST_UPDATE`, pause
  included). Title fly-over, menus with 3D backgrounds, results and replays
  stay flat on the panel: their cameras swoop and cut on their own, which is
  disorienting as the player's head (reported on the title sequence).
- Second headset log: repeats down from 25-30% to 1-3% of frames, lead
  settling around 7 ms; the lead is now quantised to whole ms over 256
  frames so the cue (the game's time step) does not wander. The game itself
  runs 70-71 of 72 fps in flight (a 28 ms gap about once a second): open.
- **Fog by distance in VR** (`Interpreter::radial_fog`, set in hle.rs). F3D
  fog is a function of clip z/w, i.e. of depth along the game camera's
  view axis: equal fog lies on planes across that axis. On a monitor that
  is invisible; in the headset the player looks along other axes and saw
  the fog start as a flat layer across sea and land that kept its distance
  and orientation to the camera, and (the plane tilting with the chase
  camera's pitch) distant water seeming to rise when climbing. Now z/w is
  taken for the point straight ahead at the vertex's distance. Reported by
  the user as a white line "like a wake" at sea, "a layer of fog" over land.
  **Did not fix it** (headset test 2026-11-15): the line and the rising
  distant water are still there, so the cause is something else (the
  radial fog stays: it is right for VR anyway). Next: `describe` in vr.rs
  logs the world's draws every 10 s in flight (per run of one world view:
  draw count, depth state, distance and height range in camera space) to
  find which geometry it is, plus a headset screenshot.
  **Found (2026-11-15):** it is `env_802E0CF0`'s haze band (renderer.md
  "Level polish"): a fog-coloured card across the game camera's view at
  0.875..1 × far, where the far plane cuts the camera-following sea model,
  blending that cut into the sky on a monitor. The user confirmed it is in
  the flat game too, always at the sea/sky line. In the eye pass there is
  no far plane: the sea model (`[vr] world:` shows the environment draws
  out to 17.7 km) went on behind the card, so from altitude the player saw
  past it, and the sea's far edge read as water rising. In VR flight the
  band was replaced by a ring around the camera on the sea: first solid
  from far out to 30 km (looked like a wall from altitude), then thickening
  with distance to full at 17 km ("a little more natural"); the water still
  seemed to rise in both. **Set aside (2026-11-15, the user's call):** in
  VR flight the band is simply not drawn (environment.c.patch), which keeps
  the white line gone. Open: the rising water. The `[vr] world:` log puts
  the sea at true sea level out to about 17.7 km, so it is most likely the
  flat sea's perspective running up towards eye level with no far-plane
  cut or haze to hide the distance. Options for later: clip the sea at the
  game's far distance in the eye pass, radial haze on sea and terrain
  alike, or the dome/sea handling of other N64 VR ports.
- No border bars in VR (`pw64_fill_frame` set with the world tag): they
  floated in the world as black bars.
- **Rocket belt first person:** in VR the R button switches between the chase
  camera and camera mode 1, which the game already has for it (pilot matrix +
  per-pilot eye offset `camera->unk68`, head part hidden, 80% levelled to
  the horizon) instead of tilting the chase camera down
  (`code_AC1A0.c.patch`).
- Units: metres (physics notes use m and m/s); `PW64_VR_WORLD_SCALE` to try
  others. Near plane 0.1 m, no far plane (reversed depth).
- Test without a headset: `stereo_eye_matches_the_flat_picture` (pw64-gfx),
  `stereo_eye_matrices` (vr.rs).
- Open: fades, transitions between panel and stereo (one frame of the other
  picture), effects that assume the game camera (w9), cost of the extra
  passes on the Quest, chase-camera tuning.

## Menus, title and results: the island model (c10)

Outside a test's flight the game's camera swoops and cuts on its own,
which is dizzying as the player's head. Confirmed on the headset
(2026-11-15): "the opening scene looks awesome, the menus work".

- **Island model** (file and level select, results, replays): a full-width
  world view with terrain is drawn as a 2.2 m model of the whole island on
  a table below the menus (0.3 to 2.5 m ahead, 0.65 m down; the menus
  float 0.5 m higher meanwhile), game north away, fixed whatever the camera
  does. Its sea is kept, cut to a cylinder around the terrain and a little
  above its highest point, which also drops the far sea and the sky dome
  (a separate shader variant with a `discard`, so other draws keep early
  depth testing). Dim blue-grey room around it. Tries before: 3 m behind
  the panel (hidden by the level select's large menus), 1.6 m (too far).
- **Title**: no terrain; its island is one model at the origin
  (`MODEL_CRESCENT_ISLAND_LOW_POLY`, bounds measured from its vertices).
  In VR the camera does not drop (it starts at the drop's end) and the
  scene turns around the island at a quarter of the speed
  (title_screen.c.patch); the headset follows that camera at full size,
  level, 5 m behind and 3 m below it, so the logo floats above the menu.
  The view recentres as the title starts. Tries before: a view fixed at
  the drop's end (still turned with the scene), a slowed drop from 12 m
  behind (still a big motion, the logo behind the menu).
- **Data**: after the world tag, 21 G_NOOP words (`PW64_DIORAMA_TAG`):
  the camera's world matrix, the island's bounds, a title flag, the game
  state and the title's phase (logged: `[vr] game state N, title phase P`).
- **VR is single-player.** The title menu ignored the pad: the title's
  Players item had been switched to 2 Players and saved, and with
  split-screen on a lone pad is player 2's (player 1 is the keyboard on a
  PC), so nothing drove player 1. Found with `[input] game pad 1` lines (VR
  only). Split-screen now stays off in VR and a switch to it is undone.

## Cameras per vehicle (c1-c3, batches 1-4)

Tuned on the headset 2026-11-15/16. The rule that came out of it: never
let the game's camera fly the player around; snap, then hold still or
turn slowly and level. All C side, VR only (`pw64_widescreen_aspect < 0`).

- **After landing** the flight view lingers until the results menu
  (`pw64_vr_flight()`, `pw64_vr_results_menu` set in results.c.patch).
- **Badge celebrations**: full size, 1 m lower for rocket belt and
  gyrocopter.
- **Crash** (camera mode 6, `pw64_vr_crash_camera`): backs off 10 m once,
  then turns slowly (dt x 0.75) and level. **At rest** (mode 3, the
  game's sweep in and orbit): held still 1 s, then a snap to a fixed 3 m
  shot from the crash camera's side; raised only when occluded.
- **Hang glider**: first person from the eyes, (0, bar.y x 0.2 + 0.2,
  bar.z x 0.85 + 0.4) in the glider frame (0.1 m back and down from the
  first tuning, headset) (bar = the control bar corner
  `unk27C`), level; tries before: 1 m behind at sail height (the game's),
  0.5 m lower and further back. Chase 50% closer (offset, 0.35 s delay,
  0.9-5 m clamps).
- **Sky diving**: first person once the chute is open (`unk70 == 1`), from
  the eyes facing the heading, level.
- **Cannonball**: aiming view as the game (+5 m was tried and undone). In
  flight first person from the head, a level heading frame from the
  velocity (`pw64_vr_heading_frame`), offset (0, 0.6, 0.3). The cinematic
  view (R) is a fixed spot beside the line from the cannon to the target
  (`pw64_cannon_target`, pads.c.patch): 0.25 x their distance to the side
  (20-70 m), 0.2 x it short of the target, 5 m above the bullseye. Tries
  before: behind the cannon panning after the pilot (30 m, then 8 m back).
  While in that view the pilot is drawn up to 4x bigger (distance / 20 m),
  back to normal 120-30 m before the target (drawing only), and in flight
  a trail of near-black puffs (`pw64_vr_cannon_trail`: smoke.c's effect,
  4 m, every 0.05 s, 4.5 s life, opaque until the last 30%, up to 96 of
  the 120 effects). Lessons: smoke.c grows each puff from nothing and
  fades it all along (invisible from afar), and its draw distance is 50 x
  the puff size (prop 19, 200 m): the trail was culled from the cinematic
  view until it was raised to 2 km.
- **Gyrocopter aim** (batch 4): the missiles leave from the camera
  itself (`camera->unk108`), 6 degrees below its forward line
  (`func_802F7178`), and the game draws the reticle where that line
  crosses the flat screen (`func_802F6DC8`: 25.7 px below the view's
  centre, row 115, at the gyrocopter's zoom 0.8). The HUD sphere maps rows
  with its own scale, `(120 - y) x 0.0034 x size`, so the reticle showed
  about 3.5 degrees down (size 90%) and the missiles landed about 2.5
  degrees below it, more metres the further the target. Not parallax:
  the missile line and the sphere both start at the camera point. In VR
  the reticle row is now set from the sphere's scale
  (`pw64_vr_aim_reticle_y`, `pw64_vr_hud_rad_per_px` set by vr.rs each
  frame): its centre 6 degrees below the sphere's, at any HUD size.
- **Gyrocopter first person** shows the gyrocopter and its pilot in VR
  (asked for on the headset). The game hides them there
  (`gyrocopterMovementFrame`: `unk5D == 0` clears dobj state bit 2); in
  VR flight the bit stays set and `pw64_dobj_cockpit` stays 0, and the
  model goes on the near list (state bit 4, `uvDobj_8021771C`: own
  frustum after a z clear) like the hang glider's and birdman's first
  person. Drawn in the main pass only, it was invisible on the headset:
  the first-person near plane is 3 m (camera.c).
  Headset: "really helped with immersion and not feeling nauseous". The
  eye is 0.15 m further back than the game's (`PW64_VR_GYRO_EYE_BACK_M`,
  gyrocopterEnterLeave; the game's was too close to the windscreen, 0.25 m
  a little far) and centred across the cockpit. The
  windscreen is part 9 of all six pilots' models (the only translucent
  part, `PW64_GYRO_WINDSCREEN_PART`); VR HUD > Gyrocopter windscreen
  (`[vr] gyro_windscreen`, `pw64_vr_gyro_windscreen`) hides it in first
  person, never from outside.
- **Dashboard gauges** (gyrocopter first person, VR HUD > Throttle and map:
  on the dashboard / with the gauges, `[vr] dash_gauges`, default on). The
  game names a point on the dashboard's surface for each gauge every frame
  (`pw64_vr_dash_set`; gyrocopter: `pw64_vr_gyro_dash`, per pilot model,
  from the ROM's models: on the dashboard's middle line, 1 cm above the
  surface, the throttle on the middle line, its top about 2 degrees below
  the dashboard's tip (the windscreen's lower edge) as seen from that
  pilot's eye; the map a row lower on the left side, 15 cm from the middle,
  20 cm in the Hawk's and Robin's wider cockpits: in the middle the pilots'
  right hand covered it a lot; 8 cm further forward in the Goose's and
  Ibis's, whose knees came up there). At the end of the world view
  `pw64_vr_dash_tag` sends them in the camera's space with the model's
  right axis (`DASH_TAG`, nine floats; `Frame::dash`), so they stay on the
  dashboard however the camera turns. The renderer draws each element in
  `StereoEye::hud_dash` (`DashSlot`: point, offset, size; vr.rs
  `DASH_SLOTS`) on a flat panel at its point (`DashPanel`, shader
  `u.vr.w == 3`: facing the eye, level with the vehicle): the throttle at
  2 mm per N64 px (12 cm wide), the map (radar, 70 x 70 px) at 1.4 mm
  (10 cm). Drawn with the world's last pass (the cockpit's, after the
  near-list depth clear) and depth tested as if 20% nearer
  (`DashPanel::DEPTH_PULL`), so the pilot's hands pass in front and the
  dashboard's own surface never hides them; drawn over everything in the
  HUD pass, the hand went behind the gauge (headset). A first try offset
  the gauges along one panel through a single point (5 cm up, map below):
  up the panel is not along the sloping dashboard, so they sat off its
  surface and too low.
- **Rocket belt first person**: the eye 0.25 m further back than the
  game's (`PW64_VR_RB_EYE_BACK_M`, rocketBeltEnterLeave; headset).
- **Cannonball power bar** in VR: beside the heading strip at the bottom,
  bottoms level (`pw64_vr_cb_power_push/pop`: the HUD view matrix moved by
  `PW64_VR_CB_POWER_DX/DY`, 65, -52 px, placed for the default spread).
  The shots left, drawn with it before, are their own HUD element now
  ("Shots", `cb_shots`, id 13), so they stay where they were.
- **Gyrocopter fuel gauge** in VR: moved for good to between the altimeter
  and the altitude (sea level) readout on the HUD sphere
  (`PW64_VR_GYRO_FUEL_X/Y`, 207, 53 in HUD px, y up; the flat game's
  98, 37): its centre midway between the altimeter's lower end and the
  readout at the default spread (275%), horizontally between their
  centres. The pilot
  is part of the gyrocopter model (one model per pilot; head `unk572`,
  arms posed by the joint animation `unk590`).
- **Open**: the cannonball landing pull-back is the game's (fine on the
  headset).

## Quest (native, Android)
- **APK:** `packaging/android/build-apk.sh --quest` → `birdman64-quest.apk`.
  The manifest's `QUEST` blocks are switched on (`com.oculus.intent.category.VR`
  on the launcher intent, or the app starts as a 2D panel; head-tracking
  feature; `com.oculus.supportedDevices`; the Khronos loader's permissions
  and `<queries>`, which its AAR would merge in under Gradle) and the Khronos
  loader (`libopenxr_loader.so`, from the release AAR, sha256-pinned) is
  packed next to `libbirdman64.so`. Meta supports the Khronos loader from
  1.0.34 on (OS v62+). The loader needs only libc/liblog/libandroid.
- **Start:** no command line on Android, so `vr::requested()` is "the loader
  library is in the native lib dir". The phone flavour never asks.
- **Loader and instance:** `XrContext::new` takes the VM and activity
  (`pw64_xr::AndroidApp`, from android-activity): `xrInitializeLoaderKHR`
  right after loading (`XR_KHR_loader_init_android`), then the instance with
  `XR_KHR_android_create_instance`. Called from `App::start`, i.e. after the
  activity and its window exist.
- **Headless window:** in VR the activity's surface is not shown, so it is
  never configured, acquired or presented (`vr::headless()`); `redraw` draws
  the frame, toasts and overlay into the panel texture only. The panel's
  size is fixed (`vr::panel_size()`, 1440x1080) instead of the window's. No
  touch controls there.
- **Lifecycle:** headset off → the runtime stops the session (`Idle` until
  READY again) and Android suspends the activity, which pauses the game in
  the settings overlay as on a phone. The headset thread failing quits the
  app (nothing else to show the game on).
- `[vr]` lines all go to logcat (`adb logcat -s birdman64`).
- Not done yet: Touch controllers, refresh rate above the default, quad
  layer recheck, foveation.

## Test
`cargo run --release -p pw64-xr --example panel -- <quad|both|world>` with
Quest Link running: a three-band pattern panel straight ahead (fast bar on
the PC clock, fast and slow bars on the display time), timing printed every
5 s. `quad` = quad layer only, `both` = quad over an empty world layer,
`world` = the panel drawn inside the world layer.
