# Birdman64VR

**Pilotwings 64 in VR.** Experience your favorite flight sim in stereo 3D with head tracking, seated, on a **Meta Quest 3 with
no PC**, or on a PC VR headset through OpenXR. An unofficial fork of
[Birdman64](https://github.com/NoahVl/Birdman64), the native PC port of
Pilotwings 64.

[![Built on Birdman64](https://img.shields.io/badge/built%20on-Birdman64-blue)](https://github.com/NoahVl/Birdman64)
[![Platforms](https://img.shields.io/badge/platforms-Quest%203%20%7C%20Windows%20PC%20VR-blue)](#install)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)

- **Bring your own game.** You need your own US Pilotwings 64 ROM. No game
  code, assets or ROM data are included.
- **Separate from Birdman64.** Birdman64VR keeps its own saves and settings,
  so the two install side by side.
- **Bugs go here, not upstream.** Please report problems with this fork in
  [its issue tracker](https://github.com/CorrieDivitt/Birdman64VR/issues),
  never to the Birdman64 project.

## Thanks to Birdman64

This fork exists because of [Birdman64](https://github.com/NoahVl/Birdman64)
by [NoahVl](https://github.com/NoahVl) and its contributors. Everything that
makes the game run natively is their work: the replacement for the N64
operating system, the graphics and audio engines, the one-time build from
your own ROM, controls, settings and the Android port. Birdman64VR adds a VR
layer on top and changes as little of the original as it can.

Birdman64 in turn is built on the
[Pilotwings 64 decompilation](https://github.com/gcsmith/Pilotwings64Decomp)
by gcsmith and its contributors. If you enjoy this, go and star both
projects. To play Pilotwings 64 on a monitor, use
[Birdman64](https://github.com/NoahVl/Birdman64/releases/latest): it is the
better flat game, and this fork follows it.

## What's new compared to Birdman64

Everything Birdman64 does still works. On top of that:

- **Stereo world with head tracking.** The whole flight is in 3D around you;
- **Runs natively on the Quest 3.** A standalone Quest app (no PC, no
  Quest Link), with the Touch controllers working as a gamepad and full controller support.
- **VR cameras for every vehicle.** First-person views from the pilot's
  eyes and the gyrocopter's cockpit with its pilot and
  dashboard. Chase cameras tuned for VR. The game's own swooping cameras
  are replaced by still or slowly turning views so you don't get sick.
- **Horizon lock.** The world stays level and your vehicle banks around you.
- **A VR HUD.** The gauges float in front of you. Distance,
  spread and size are adjustable, and each gauge can be hidden. In the
  gyrocopter the throttle and map can sit on the dashboard.
- **Menus with the island as a model.** File select, level select and
  results show the whole island as a small model on a table below the menus.
- **Diorama mode.** Third-person flights as a living model of the island:
  watch your vehicle fly over a tabletop island, in a dim virtual room or in
  your own room through passthrough on the Quest.
- **Passthrough.** On the Quest, the menus can float in your real room.
- **Refresh rate**, **Animation distance**, **Infinite LOD** and
  **Infinite view distance** all apply instantly (see
  [Performance](#performance)).
- **Comfort details.** Short fades when the view changes, no black flashes
  between screens, a shutter blink when you take a photo, and a recentre
  button.

## Install

### Meta Quest 3 (standalone)

1. Turn on developer mode for your headset (in the Meta Horizon app on your
   phone, in the headset's settings).
2. Download `Birdman64VR-quest.apk` from the
   [Releases](https://github.com/CorrieDivitt/Birdman64VR/releases) page.
3. Sideload it, with [SideQuest](https://sidequestvr.com/) or from a PC
   with Android platform tools: `adb install -r Birdman64VR-quest.apk`.
4. Start **Birdman64VR** from the headset's library (under Unknown sources).
5. The first launch builds the game on the headset. This needs internet
   once and takes a little while; later launches start straight away.
6. Pick your ROM with the file picker when it asks. Or copy it over USB
   first: `adb push <your ROM>.z64 /sdcard/Android/data/app.birdman64vr/files/rom/`.

A newer APK installs over the old one and keeps your saves.

### PC VR (Windows)

1. Download `Birdman64VR-windows-x64.zip` from the
   [Releases](https://github.com/CorrieDivitt/Birdman64VR/releases) page and
   extract it (never run it from inside the zip; keep the `toolchain`
   folder next to the exe).
2. Start your headset's OpenXR runtime: Meta Quest Link with Meta set as
   the active OpenXR runtime (tested), or SteamVR (untested).
3. Run `Birdman64VR.exe --vr` (a shortcut with `--vr` added to its target
   works, or set `PW64_VR=1`). Without `--vr`, or without a running OpenXR
   runtime, it starts as the normal flat game.
4. First start: pick your ROM, and wait for the one-time build (about a
   minute, internet needed once), as in Birdman64.

Windows may show "Windows protected your PC" because the program is not code
signed: click **More info**, then **Run anyway**.

### Your ROM

Only the **US** version of Pilotwings 64 works (`.z64`, `.n64` or `.v64`,
or a `.zip` with one inside). The `.z64` file's SHA-1 is
`ec771aedf54ee1b214c25404fb4ec51cfd43191a`. A ROM is a copy of a cartridge
you own; we can't provide or link one.

## Controls

### Touch controllers

The Touch controllers act as an Xbox-style gamepad. A Bluetooth gamepad
works alongside them.

| Touch | N64 | In the game |
|---|---|---|
| Left stick | Control stick | Steer · move in menus |
| A (right) | A | Confirm · thrust, flap, jump, fire |
| B (right), X (left) | B | Back · gentle thrust, parachute |
| Y (left) | C up | |
| Right stick | C buttons in menus | See [The right stick](#the-right-stick) |
| Left trigger | Z | Hold to aim camera or missile, release to shoot |
| Right trigger, right grip | R | Change camera view (chase / first person) |
| Left grip | L | Not used in this game |
| Menu (left) | Start | Pause · confirm |
| Right stick press | | **Recentre** the view |
| Left stick press | | Open or close the **settings** |

### Gamepad

The same layout as on an Xbox pad: the buttons are as in Birdman64, the
right stick press recentres, and View (Select) opens the settings. Rebind
anything on the settings screen's Controls page.

### The right stick

Your head does the looking in VR, so the right stick does VR things:

- **In a chase view:** up and down move the camera closer or further back
  (remembered per vehicle).
- **In first person:** up and down move your eye forward or back.
- **In the menus:** turns and tilts the island model; L and R zoom it.
- **In Diorama mode:** up and down zoom the model, left and right turn it.

The C buttons no longer turn the camera in VR, and R changes the view as in
the original game.

## The VR features

### Cameras and comfort

- **R switches between chase and first person** for every vehicle that has
  both. In first person the gyrocopter shows its cockpit, pilot and
  dashboard.
- **Horizon lock** (Settings > VR Comfort): **bank** (the
  default) keeps the world level when you bank; **bank and pitch** also when
  you climb or dive; **off** follows the vehicle fully. It is always off for
  the cannonball and sky diving.
- **Recentre** any time with the right stick press.
- The game's moving cameras for crashes, landings, badge celebrations and
  the title screen are replaced by views that snap and then hold still or
  turn slowly.

### The VR HUD

Settings > VR HUD: HUD distance, gauge spread and gauge size, and a
show / hide switch for each gauge. In a flight the page only shows the
gauges your vehicle has. In the gyrocopter: the windscreen on or off in
first person, and the throttle and map on the dashboard or with the other
gauges.

### Diorama mode

Settings > Diorama mode turns third-person flights into a model of the
island in front of you. First person (R) stays full size.

| Choice | What you see |
|---|---|
| off | The normal full-size chase view |
| on | The model moves so your vehicle stays in front of you; dim virtual room |
| on static | The island stands still and your vehicle flies over it; dim virtual room |
| passthrough | As **on**, in your own room (Quest) |
| passthrough static | As **on static**, in your own room (Quest) |

In the dim room the model has the game's sea around it; there is no sky
around the model yet. The right stick zooms (down to 20 game metres from
your vehicle) and turns the island. The flight HUD is hidden while Diorama
mode is on, except for the cannonball.

### Passthrough (Quest)

Settings > Passthrough: on puts the menus and the island model in your own
room; off gives the dim virtual room. Flights, the title and badge
celebrations stay fully virtual.

### Display & graphics in the headset

- **Refresh rate** (Quest): headset default, 72, 80, 90 or 120 Hz, applied
  at once.
- **Animation distance:** **Original** animates the Ferris wheel, boats,
  ferry, ski lift, planes and the like only near the camera, as on the N64;
  **Infinite** keeps them moving at any distance, which matters when you
  look across the island in VR.
- **Infinite LOD** and **Infinite view distance** apply at once; see the
  next section before turning them on.
- Options that do nothing in a headset (window modes, widescreen, MSAA,
  render scale, OLED care, split-screen, touch controls, Move HUD) are
  hidden in VR.

## Performance

We measured on a Quest 3 running standalone. 

- **Normal flights run smoothly.** They hold 72 fps at the default 72 Hz
  with plenty of headroom, and 90 fps at 90 Hz. 72 or 90 Hz is the sweet
  spot.
- **The first launch can hitch.** The first time the game meets a new
  effect it has to prepare it, which causes short stutters in the first
  menus and the first flights. This is saved, so from the second launch on
  it is smooth.
- **Infinite view distance is fine.** On its own it holds 72 fps, in normal
  flights and in Diorama mode.
- **Infinite LOD will likely cause problems.** It draws every model at
  full detail at any distance: about 2,500 draws per eye instead of a few
  hundred. Together with Infinite view distance the game dropped to
  35-52 fps, which you feel as stutter. The headset keeps your head
  movement smooth, but the world doesn't keep up. We recommend leaving
  Infinite LOD off on the Quest.
- **Diorama mode can be taxing.** It shows the whole island at once, so
  there is more to draw than in a chase view. With the default settings it
  holds 72 fps; with Infinite LOD on as well it fell to 20-57 fps.
- **Higher refresh rates leave less time per frame.** If a flight stutters
  at 90 or 120 Hz, go back to 72 Hz first.

On PC VR the numbers depend on your PC. We haven't measured it in detail; it
ran at a steady 72 fps over Quest Link on a fast desktop graphics card.

## Settings file

Everything is set from inside the headset, and saved in `pw64.toml` like
Birdman64's settings. The VR keys, for reference:

```toml
[vr]
diorama_mode = "off"      # "off", "on", "on_static", "passthrough", "passthrough_static"
passthrough = true        # Quest: menus in your own room
horizon_lock = "bank"     # "off", "bank" or "full" (bank and pitch)
refresh_rate = 90         # Quest: Hz; leave out for the headset default
hud_distance = 2.0        # metres
hud_spread = 2.75         # how far apart the gauges are
hud_size = 1.0            # gauge size
hud_hidden = ["timer"]    # gauges hidden in the headset
gyro_windscreen = true    # gyrocopter windscreen in first person
dash_gauges = true        # gyrocopter throttle and map on the dashboard

[graphics]
infinite_animation_distance = false
infinite_lod = false
infinite_view_distance = false
```

The values above are examples. On Windows the file lives in
`%APPDATA%\Birdman64VR`; on the Quest in the app's own storage. Everything else about
settings, saves (including moving saves between devices and emulators),
texture packs and troubleshooting works as described in
[Birdman64's README](https://github.com/NoahVl/Birdman64#readme).

## Known issues

- No sky around the island model in Diorama mode yet (the sea is there).
- In the model view the game still fogs and picks detail as seen from its
  own chase camera.
- Infinite LOD is too heavy for the Quest (see [Performance](#performance)).
- Single player only in VR (split-screen stays in the flat game).
- Tested on a Meta Quest 3 standalone and over Quest Link. Other headsets,
  Air Link, Virtual Desktop and SteamVR have not been tested yet; reports
  are welcome.

## Building from source

As for Birdman64 (Rust stable, zig 0.16.0 fetched by one command; see
[its build instructions](https://github.com/NoahVl/Birdman64#building-from-source)):

```sh
git clone --recursive https://github.com/CorrieDivitt/Birdman64VR
cd Birdman64VR
cargo run -p pw64-cbuild --example get_zig --features fetch
cargo run --release -p birdman64 -- --vr
```

The Quest APK needs the Android SDK, NDK r27 and the `aarch64-linux-android`
Rust target ([docs/notes/android.md](docs/notes/android.md)), then:

```sh
packaging/android/build-apk.sh --quest
```

How the VR mode works, and what was tried and measured along the way, is in
[docs/notes/vr.md](docs/notes/vr.md). `cargo test --workspace` and
`cargo clippy --workspace --all-targets` must pass; tests check that the
flat game's picture is unchanged by the VR work.

## Legal

MIT licence, like Birdman64. Birdman64VR is an unofficial fan project, not
affiliated with or endorsed by Nintendo, Paradigm Simulation or the
Birdman64 project. Pilotwings 64 is a trademark of Nintendo. No game code,
assets or ROM data are distributed: the game is built on your own device
from your own ROM.
