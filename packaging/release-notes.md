## Birdman64VR 1.0.0

**Pilotwings 64 in VR**, standalone on the Meta Quest 3: fly every vehicle in
stereo 3D with head tracking, in VR-tuned chase and first-person cameras, with
horizon lock, a VR HUD, menus with the island as a tabletop model, Diorama mode
and passthrough. The full list is in the [changelog](https://github.com/CorrieDivitt/Birdman64VR/blob/main/CHANGELOG.md).

Built on [Birdman64](https://github.com/NoahVl/Birdman64) by NoahVl and its
contributors, which does all the work of running the game natively. Thank you.
Please report problems with this fork
[here](https://github.com/CorrieDivitt/Birdman64VR/issues), not to Birdman64.

You need your own Pilotwings 64 (US) ROM.

### Meta Quest 3

1. Turn on developer mode for your headset (Meta Horizon phone app).
2. Sideload `Birdman64VR-quest.apk` with [SideQuest](https://sidequestvr.com/),
   or `adb install -r Birdman64VR-quest.apk`.
3. Start **Birdman64VR** from the library (under Unknown sources). The first
   launch builds the game on the headset (internet needed once), then pick
   your ROM.

Later releases install over this one and keep your saves.

### PC VR (Windows, experimental)

Download `Birdman64VR-windows-x64.zip`, extract it, start Meta Quest Link (or
another OpenXR runtime), then double-click **Start in VR.cmd**. PC VR is
experimental and unsupported in this release; over Virtual Desktop the
passthrough stays on.

### Good to know

- Normal flights hold 72 fps at 72 Hz and 90 fps at 90 Hz on the Quest 3.
- **Infinite LOD** is too heavy for the Quest (35-52 fps); leave it off.
  Infinite view distance is fine.
- Diorama mode has the game's sea but no sky around the island yet.
- The first launch stutters a little while the game prepares its effects;
  later launches are smooth.

The other files (debug symbols, checksums) are for bug reports and
verification.
