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
//! On the Quest (Android, docs/notes/vr.md "Quest") there is no window to
//! mirror to: the activity's own surface is not shown while the app is in
//! VR, so the window thread draws the panel and nothing else
//! ([`headless`]).
//!
//! Only on Windows, Linux and Android; other targets get a stub that is
//! never requested.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};

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
/// Android has neither: there the Quest flavour of the APK
/// (`build-apk.sh --quest`) is the request, recognised by the OpenXR
/// loader it packs next to this library.
pub(crate) fn requested() -> bool {
    #[cfg(target_os = "android")]
    if crate::android::native_lib_dir()
        .join("libopenxr_loader.so")
        .is_file()
    {
        return true;
    }
    REQUESTED.load(Ordering::Relaxed) || std::env::var_os("PW64_VR").is_some_and(|v| v != "0")
}

/// The picture goes to the headset only (the Quest): the window thread
/// draws the VR panel and never touches the window's surface.
pub(crate) fn headless() -> bool {
    cfg!(target_os = "android") && active()
}

/// What the game's C side culls with in VR (`pw64_game::set_widescreen_aspect`
/// with the negated value, lib.rs `run_game`): the world view's culling
/// frustum is this many times the game camera's in both directions, so
/// what the headset shows beside, above and below the monitor picture is
/// there when the player looks at it (about 70 degrees to each side).
pub(crate) const CULL_SCALE: f32 = 6.0;

/// Flight-HUD elements not shown in the headset (bit per element id,
/// `hud_edit::ELEMENTS`; `[vr] hud_hidden`, the settings' VR HUD page).
static HUD_HIDDEN: AtomicU16 = AtomicU16::new(0);
static HUD_HIDDEN_LOADED: std::sync::Once = std::sync::Once::new();

/// The HUD elements hidden in the headset (bit per element id).
pub(crate) fn hud_hidden() -> u16 {
    HUD_HIDDEN_LOADED.call_once(|| {
        let mask = crate::config::get()
            .vr_hud_hidden
            .iter()
            .filter_map(|n| crate::hud_edit::element_id(n))
            .fold(0u16, |m, id| m | 1 << id);
        HUD_HIDDEN.store(mask, Ordering::Relaxed);
    });
    HUD_HIDDEN.load(Ordering::Relaxed)
}

/// A VR HUD setting changed: the window redraws the eye pictures from the
/// last game frame even while the game is paused (`take_hud_changed`).
static HUD_CHANGED: AtomicBool = AtomicBool::new(false);

/// Whether a VR HUD setting changed since the last call (window.rs).
pub(crate) fn take_hud_changed() -> bool {
    HUD_CHANGED.swap(false, Ordering::Relaxed)
}

/// Where the flight HUD sits in the headset (the settings' VR HUD page,
/// `[vr] hud_distance / hud_spread / hud_size`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HudLayout {
    /// The sphere's radius, metres.
    pub distance: f32,
    /// How far out from the centre the gauges sit, sideways, relative to
    /// the flat screen (up and down a little less).
    pub spread: f32,
    /// Gauge size relative to the flat panel's.
    pub size: f32,
}

impl HudLayout {
    /// Chosen by the user in the headset (2026-11-15).
    pub const DEFAULT: HudLayout = HudLayout {
        distance: 10.0,
        spread: 2.75,
        size: 0.9,
    };
    pub const DISTANCE_STEPS: [f32; 10] = [1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 7.0, 10.0, 15.0, 20.0];
    pub const SPREAD_STEPS: [f32; 17] = [
        1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0, 3.25, 3.5, 3.75, 4.0, 4.25, 4.5, 4.75, 5.0,
    ];
    pub const SIZE_STEPS: [f32; 14] = [
        0.6, 0.7, 0.8, 0.9, 1.0, 1.15, 1.3, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0,
    ];

    /// The sphere's spread factors: sideways `spread`, up and down 60% as
    /// much beyond 1.
    pub fn spread_xy(&self) -> [f32; 2] {
        [self.spread, 1.0 + (self.spread - 1.0) * 0.6]
    }
}

static HUD_LAYOUT: std::sync::Mutex<Option<HudLayout>> = std::sync::Mutex::new(None);

/// The HUD layout in force (loaded from the config the first time).
pub(crate) fn hud_layout() -> HudLayout {
    *HUD_LAYOUT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(|| {
            let c = &crate::config::get().vr_hud;
            let d = HudLayout::DEFAULT;
            HudLayout {
                distance: c.distance.unwrap_or(d.distance),
                spread: c.spread.unwrap_or(d.spread),
                size: c.size.unwrap_or(d.size),
            }
        })
}

/// A new HUD layout: in force at once (also while paused) and saved.
pub(crate) fn set_hud_layout(l: HudLayout) {
    *HUD_LAYOUT.lock().unwrap_or_else(|e| e.into_inner()) = Some(l);
    HUD_CHANGED.store(true, Ordering::Relaxed);
    if let Err(e) = crate::config::save_vr_hud_layout(l.distance, l.spread, l.size) {
        eprintln!("[vr] HUD layout not saved: {e}");
    }
}

/// Shows or hides one HUD element in the headset and saves the choice.
pub(crate) fn set_hud_shown(id: u8, shown: bool) {
    let mask = hud_hidden();
    let mask = if shown {
        mask & !(1 << id)
    } else {
        mask | 1 << id
    };
    HUD_HIDDEN.store(mask, Ordering::Relaxed);
    HUD_CHANGED.store(true, Ordering::Relaxed);
    let names: Vec<&str> = crate::hud_edit::ELEMENTS
        .iter()
        .filter(|e| mask & (1 << e.0) != 0)
        .map(|e| e.1)
        .collect();
    if let Err(e) = crate::config::save_vr_hud_hidden(&names) {
        eprintln!("[vr] HUD choice not saved: {e}");
    }
}

/// The gyrocopter's windscreen in first person (`[vr] gyro_windscreen`, the
/// settings' VR HUD page): 0 = not loaded yet, 1 = shown, 2 = hidden.
static GYRO_WINDSCREEN: AtomicU8 = AtomicU8::new(0);

/// Whether the gyrocopter's windscreen shows in VR first person.
pub(crate) fn gyro_windscreen() -> bool {
    match GYRO_WINDSCREEN.load(Ordering::Relaxed) {
        0 => {
            let on = crate::config::get().vr_gyro_windscreen.unwrap_or(true);
            GYRO_WINDSCREEN.store(if on { 1 } else { 2 }, Ordering::Relaxed);
            on
        }
        v => v == 1,
    }
}

/// Shows or hides the gyrocopter's windscreen in VR first person and saves
/// the choice (the C side takes it at the next headset frame).
pub(crate) fn set_gyro_windscreen(on: bool) {
    GYRO_WINDSCREEN.store(if on { 1 } else { 2 }, Ordering::Relaxed);
    HUD_CHANGED.store(true, Ordering::Relaxed);
    if let Err(e) = crate::config::save_vr_bool("gyro_windscreen", on) {
        eprintln!("[vr] windscreen choice not saved: {e}");
    }
}

/// The headset can show the player's room behind the picture
/// (`XrContext::passthrough_available`; set by the headset thread). Read
/// by the window thread, which then leaves the island model's background
/// clear, and by the settings, which then offer the Passthrough row.
static PASSTHROUGH_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// p12: whether the headset can show the player's room (a Quest, natively).
pub(crate) fn passthrough_available() -> bool {
    PASSTHROUGH_AVAILABLE.load(Ordering::Relaxed)
}

/// The menus in the player's room (`[vr] passthrough`, the settings' main
/// page): 0 = not loaded yet, 1 = on, 2 = off.
static PASSTHROUGH: AtomicU8 = AtomicU8::new(0);

/// Whether the menus are shown in the player's room where the headset can.
pub(crate) fn passthrough() -> bool {
    match PASSTHROUGH.load(Ordering::Relaxed) {
        0 => {
            let on = crate::config::get().vr_passthrough.unwrap_or(true);
            PASSTHROUGH.store(if on { 1 } else { 2 }, Ordering::Relaxed);
            on
        }
        v => v == 1,
    }
}

/// Turns the menus' passthrough on or off (from the next headset frame)
/// and saves the choice.
pub(crate) fn set_passthrough(on: bool) {
    PASSTHROUGH.store(if on { 1 } else { 2 }, Ordering::Relaxed);
    // A paused game's island model is redrawn with the other background.
    HUD_CHANGED.store(true, Ordering::Relaxed);
    if let Err(e) = crate::config::save_vr_bool("passthrough", on) {
        eprintln!("[vr] passthrough choice not saved: {e}");
    }
}

/// The display's refresh rates, whole Hz (set by the headset thread; empty:
/// no choice, e.g. on PC, where Quest Link sets it).
static REFRESH_RATES: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// q6: the refresh rates the player can pick (whole Hz, lowest first).
pub(crate) fn refresh_rates() -> Vec<u32> {
    let mut rates = REFRESH_RATES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    rates.sort_unstable();
    rates.dedup();
    rates
}

/// The player's refresh rate (`[vr] refresh_rate`): 0 = not loaded yet,
/// 1 = the headset's own, else Hz.
static REFRESH_RATE: AtomicU32 = AtomicU32::new(0);

/// The refresh rate the player picked (`None`: the headset's own, 72 Hz on
/// a Quest 3 by default).
pub(crate) fn refresh_rate() -> Option<u32> {
    match REFRESH_RATE.load(Ordering::Relaxed) {
        0 => {
            let hz = crate::config::get().vr_refresh_rate;
            REFRESH_RATE.store(hz.unwrap_or(1), Ordering::Relaxed);
            hz
        }
        1 => None,
        hz => Some(hz),
    }
}

/// Sets the refresh rate (from the next headset frame) and saves it.
pub(crate) fn set_refresh_rate(hz: Option<u32>) {
    REFRESH_RATE.store(hz.unwrap_or(1).max(1), Ordering::Relaxed);
    if let Err(e) = crate::config::save_vr_refresh_rate(hz) {
        eprintln!("[vr] refresh rate not saved: {e}");
    }
}

/// Diorama mode (p14): a flight in third person shown as a model of the
/// island (`[vr] diorama_mode`, the settings' main page). The game's
/// first-person view (R on the pad) stays full size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DioramaMode {
    Off,
    /// The model moves so the vehicle stays in front of the player, in a
    /// dim virtual room.
    On,
    /// The island stands still, as in the menus; dim virtual room.
    OnStatic,
    /// As `On`, in the player's own room (passthrough).
    Passthrough,
    /// As `OnStatic`, in the player's own room.
    PassthroughStatic,
}

impl DioramaMode {
    pub(crate) const ALL: [DioramaMode; 5] = [
        DioramaMode::Off,
        DioramaMode::On,
        DioramaMode::OnStatic,
        DioramaMode::Passthrough,
        DioramaMode::PassthroughStatic,
    ];

    /// The `[vr] diorama_mode` value (`config::DIORAMA_MODE_VALUES`).
    fn config(self) -> &'static str {
        match self {
            DioramaMode::Off => "off",
            DioramaMode::On => "on",
            DioramaMode::OnStatic => "on_static",
            DioramaMode::Passthrough => "passthrough",
            DioramaMode::PassthroughStatic => "passthrough_static",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            DioramaMode::Off => "off",
            DioramaMode::On => "on",
            DioramaMode::OnStatic => "on static",
            DioramaMode::Passthrough => "passthrough",
            DioramaMode::PassthroughStatic => "passthrough static",
        }
    }

    /// The model moves with the vehicle (else the island stands still).
    pub(crate) fn follows(self) -> bool {
        matches!(self, DioramaMode::On | DioramaMode::Passthrough)
    }

    /// The player's room behind the model (needs a headset that can).
    pub(crate) fn passthrough(self) -> bool {
        matches!(
            self,
            DioramaMode::Passthrough | DioramaMode::PassthroughStatic
        )
    }
}

/// 0 = not loaded yet, else 1 + the index in [`DioramaMode::ALL`].
static DIORAMA_MODE: AtomicU8 = AtomicU8::new(0);

pub(crate) fn diorama_mode() -> DioramaMode {
    match DIORAMA_MODE.load(Ordering::Relaxed) {
        0 => {
            let cfg = crate::config::get().vr_diorama_mode.clone();
            let mode = DioramaMode::ALL
                .into_iter()
                .find(|m| Some(m.config()) == cfg.as_deref())
                .unwrap_or(DioramaMode::Off);
            set_diorama_mode_live(mode);
            mode
        }
        v => DioramaMode::ALL[(v - 1) as usize % DioramaMode::ALL.len()],
    }
}

fn set_diorama_mode_live(mode: DioramaMode) {
    let i = DioramaMode::ALL
        .iter()
        .position(|m| *m == mode)
        .unwrap_or(0);
    DIORAMA_MODE.store(i as u8 + 1, Ordering::Relaxed);
}

/// Sets the diorama mode (from the next game frame) and saves the choice.
pub(crate) fn set_diorama_mode(mode: DioramaMode) {
    set_diorama_mode_live(mode);
    // A paused flight is redrawn the other way.
    HUD_CHANGED.store(true, Ordering::Relaxed);
    if let Err(e) = crate::config::save_vr_diorama_mode(mode.config()) {
        eprintln!("[vr] diorama mode not saved: {e}");
    }
}

/// The room shows this frame: the headset can and the player wants it.
fn passthrough_on() -> bool {
    passthrough_available() && passthrough()
}

/// p8: how much further back (metres; negative nearer) each vehicle's
/// camera sits in VR, set with the right stick, one per `VEHICLE_*`: chase
/// views (`[vr] chase_offsets`) and first person (`[vr]
/// first_person_offsets`). `None` until loaded.
static OFFSETS: std::sync::Mutex<Option<[[f32; 7]; 2]>> = std::sync::Mutex::new(None);

/// Which camera an offset is for ([`OFFSETS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewKind {
    Chase,
    FirstPerson,
}

impl ViewKind {
    /// The offset range, metres back: chase nearer than 2 m in front of the
    /// game's own spot runs into the vehicle, 30 m back is far enough; first
    /// person 1 m either way (the user, in the headset).
    pub(crate) fn range(self) -> (f32, f32) {
        match self {
            ViewKind::Chase => (-2.0, 30.0),
            ViewKind::FirstPerson => (-1.0, 1.0),
        }
    }
}

fn offset_table() -> std::sync::MutexGuard<'static, Option<[[f32; 7]; 2]>> {
    let mut t = OFFSETS.lock().unwrap_or_else(|e| e.into_inner());
    if t.is_none() {
        let c = crate::config::get();
        *t = Some([
            c.vr_chase_offsets.unwrap_or([0.0; 7]),
            c.vr_first_person_offsets.unwrap_or([0.0; 7]),
        ]);
    }
    t
}

/// The camera offset for `vehicle` in `kind` (metres back).
pub(crate) fn view_offset(kind: ViewKind, vehicle: u8) -> f32 {
    offset_table().map_or(0.0, |t| {
        t[kind as usize]
            .get(vehicle as usize)
            .copied()
            .unwrap_or(0.0)
    })
}

/// Moves `vehicle`'s `kind` camera by `d` metres (back), within its range;
/// [`save_offsets`] writes it.
pub(crate) fn nudge_offset(kind: ViewKind, vehicle: u8, d: f32) {
    let (lo, hi) = kind.range();
    if let Some(t) = offset_table().as_mut()
        && let Some(o) = t[kind as usize].get_mut(vehicle as usize)
    {
        *o = (*o + d).clamp(lo, hi);
    }
}

/// Saves the camera offsets (`[vr] chase_offsets`, `first_person_offsets`).
pub(crate) fn save_offsets() {
    let t = *offset_table();
    if let Some([chase, fp]) = t
        && let Err(e) = crate::config::save_vr_offsets(&chase, &fp)
    {
        eprintln!("[vr] camera distances not saved: {e}");
    }
}

/// Horizon lock (c5, comfort): in flight the world stays level and the
/// vehicle banks (and pitches) around the player, instead of the view
/// rolling with it (`[vr] horizon_lock`, the settings' VR HUD page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HorizonLock {
    Off,
    /// The horizon stays level; the view still pitches with the vehicle
    /// (where it is heading stays ahead). The default.
    Bank,
    /// Level in both: the view keeps only the vehicle's heading.
    Full,
}

impl HorizonLock {
    pub(crate) const ALL: [HorizonLock; 3] =
        [HorizonLock::Off, HorizonLock::Bank, HorizonLock::Full];

    fn config(self) -> &'static str {
        match self {
            HorizonLock::Off => "off",
            HorizonLock::Bank => "bank",
            HorizonLock::Full => "full",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            HorizonLock::Off => "off",
            HorizonLock::Bank => "bank",
            HorizonLock::Full => "bank and pitch",
        }
    }
}

/// 0 = not loaded yet, else 1 + the index in [`HorizonLock::ALL`].
static HORIZON_LOCK: AtomicU8 = AtomicU8::new(0);

/// The horizon lock setting.
pub(crate) fn horizon_lock() -> HorizonLock {
    match HORIZON_LOCK.load(Ordering::Relaxed) {
        0 => {
            let cfg = crate::config::get().vr_horizon_lock.clone();
            let mode = HorizonLock::ALL
                .into_iter()
                .find(|m| cfg.as_deref() == Some(m.config()))
                .unwrap_or(HorizonLock::Bank);
            set_horizon_lock_live(mode);
            mode
        }
        v => HorizonLock::ALL[(v - 1) as usize % 3],
    }
}

fn set_horizon_lock_live(mode: HorizonLock) {
    let i = HorizonLock::ALL
        .iter()
        .position(|m| *m == mode)
        .unwrap_or(1);
    HORIZON_LOCK.store(i as u8 + 1, Ordering::Relaxed);
}

/// Sets the horizon lock (live from the next game frame) and saves it.
pub(crate) fn set_horizon_lock(mode: HorizonLock) {
    set_horizon_lock_live(mode);
    HUD_CHANGED.store(true, Ordering::Relaxed);
    if let Err(e) = crate::config::save_vr_horizon_lock(mode.config()) {
        eprintln!("[vr] horizon lock choice not saved: {e}");
    }
}

/// The throttle and the map (radar) on the cockpit's dashboard in first
/// person, where the game gives one (`[vr] dash_gauges`, the settings' VR
/// HUD page; the gyrocopter): 0 = not loaded yet, 1 = on, 2 = off (on the
/// sphere).
static DASH_GAUGES: AtomicU8 = AtomicU8::new(0);

/// Whether the throttle and the map sit on the cockpit's dashboard.
pub(crate) fn dash_gauges() -> bool {
    match DASH_GAUGES.load(Ordering::Relaxed) {
        0 => {
            let on = crate::config::get().vr_dash_gauges.unwrap_or(true);
            DASH_GAUGES.store(if on { 1 } else { 2 }, Ordering::Relaxed);
            on
        }
        v => v == 1,
    }
}

/// Puts the throttle and the map on the dashboard or back on the sphere
/// and saves the choice.
pub(crate) fn set_dash_gauges(on: bool) {
    DASH_GAUGES.store(if on { 1 } else { 2 }, Ordering::Relaxed);
    HUD_CHANGED.store(true, Ordering::Relaxed);
    if let Err(e) = crate::config::save_vr_bool("dash_gauges", on) {
        eprintln!("[vr] dashboard choice not saved: {e}");
    }
}

/// A recentre was asked for (`request_recentre`); the headset thread does
/// it at its next frame.
static RECENTRE: AtomicBool = AtomicBool::new(false);

/// The player pressed the recentre button (R3, input.rs and
/// `input/mobile_pad.rs`): forward becomes the way they face now and the
/// game's camera moves back to their eyes. Nothing without a headset.
pub(crate) fn request_recentre() {
    if active() {
        RECENTRE.store(true, Ordering::Relaxed);
    }
}

/// The headset thread wants the game paused whatever the player's
/// "pause in background" setting says (headset taken off, system menu).
static PAUSE: AtomicBool = AtomicBool::new(false);

/// Whether a pause request (window.rs `UserEvent::Pause`) came from the
/// headset thread; clears it.
pub(crate) fn take_pause() -> bool {
    PAUSE.swap(false, Ordering::Relaxed)
}

/// The size of the window's picture when it is not the window's
/// ([`headless`]): a 4:3 panel about as sharp as the headset can show it at
/// the panel's distance.
pub(crate) fn panel_size() -> Option<(u32, u32)> {
    headless().then_some((1440, 1080))
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
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "android")),
    allow(dead_code)
)]
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
    /// Drawing both eyes' pictures of the stereo world.
    Eyes,
}

const STAGES: usize = 7;
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "android")),
    allow(dead_code)
)]
const STAGE_NAMES: [&str; STAGES] = [
    "game draw",
    "window image",
    "frame draw",
    "window present",
    "game held back",
    "gap between game frames",
    "eye pictures",
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
    #[cfg_attr(
        not(any(windows, target_os = "linux", target_os = "android")),
        allow(dead_code)
    )]
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
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "android")),
    allow(dead_code)
)]
static PANEL_AGE: Delay = Delay::new();
/// How far ahead the runtime predicts the display of the frame being drawn.
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "android")),
    allow(dead_code)
)]
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

#[cfg(any(windows, target_os = "linux", target_os = "android"))]
pub(crate) use real::{Vr, shutdown};
#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
pub(crate) use stub::{Vr, shutdown};

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
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

        pub(crate) fn probe(&mut self, _: &wgpu::Device, _: &wgpu::Queue, _: &wgpu::TextureView) {}

        pub(crate) fn publish(&mut self) {}

        pub(crate) fn world(
            &mut self,
            _: &mut pw64_gfx::Renderer,
            _: &wgpu::Device,
            _: wgpu::TextureFormat,
            _: Option<&pw64_gfx::Frame>,
        ) {
        }

        pub(crate) fn stereo(&self) -> bool {
            false
        }

        pub(crate) fn blank(&self) -> bool {
            false
        }

        pub(crate) fn clear_panel(&self, _: &wgpu::Device, _: &wgpu::Queue, _: &wgpu::TextureView) {
        }
    }
}

#[cfg(any(windows, target_os = "linux", target_os = "android"))]
mod real {
    use super::HEADSET_MHZ;
    use crate::gpu_context::GpuContext;
    use pw64_xr::math::{self, Mat4};
    use pw64_xr::{EyePose, FrameOutcome, Layers, Panel, Seat, XrContext};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Condvar, Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use winit::window::Window;

    /// `shutdown` asked the headset thread to end; it sets `DONE`.
    static STOP: AtomicBool = AtomicBool::new(false);
    static DONE: AtomicBool = AtomicBool::new(false);
    static STARTED: AtomicBool = AtomicBool::new(false);

    /// How long before the next headset frame the game is cued to show its
    /// newest frame. The cue latches the frame the game finished and the
    /// window then draws it (into the panel, or as the stereo world's eye
    /// pictures); that must be done when the headset frame starts. A
    /// longer lead is safer against a late (repeated) frame, a shorter one
    /// has less input lag.
    ///
    /// `PW64_VR_LEAD_MS` fixes it; otherwise 6 ms for the panel, and for
    /// the stereo world what the window thread measured it needs
    /// ([`STEREO_LEAD_US`]): drawing two eyes takes several ms, and a
    /// picture that is ready for some headset frames and late for others
    /// shows as judder (one shown twice, the next skipped).
    fn lead() -> Duration {
        static FIXED: OnceLock<Option<Duration>> = OnceLock::new();
        let fixed = *FIXED.get_or_init(|| {
            std::env::var("PW64_VR_LEAD_MS")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| (0.0..=50.0).contains(v))
                .map(|ms| Duration::from_secs_f64(ms / 1e3))
        });
        fixed.unwrap_or_else(|| match STEREO_LEAD_US.load(Ordering::Relaxed) {
            0 => Duration::from_millis(6),
            us => Duration::from_micros(us.into()),
        })
    }

    /// The lead the stereo world needs, in microseconds (0: no stereo
    /// world): the longest recent time from the cue to the eye pictures
    /// being published, plus a margin (`Vr::world`).
    static STEREO_LEAD_US: AtomicU32 = AtomicU32::new(0);
    /// When the cue thread last cued the game.
    static CUED_AT: Mutex<Option<Instant>> = Mutex::new(None);

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
            *CUED_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            pw64_platform::os::vi::VBLANK.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `[vr]` lines go to stderr and to `vr.log` in the data folder (a new
    /// file per run), so a headset session can be looked at afterwards.
    fn log(line: &str, console: bool) {
        use std::io::Write;
        static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
        // Android: everything, since logcat is the log that is easy to read
        // (`adb logcat -s birdman64`).
        if console || cfg!(target_os = "android") {
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

    /// The panel textures. The window thread draws a frame into one (many
    /// passes, each its own submit) while the headset thread shows the
    /// other, `front`: the last finished frame ([`Vr::publish`]). With one
    /// texture the headset caught frames between passes (a settings menu
    /// that flickers: frames without it). `generation` changes when the
    /// textures are replaced (window resize).
    ///
    /// The headset thread keeps this locked from choosing `front` until it
    /// has submitted the commands that read it, so a publish (and with it
    /// the window's next draw into that texture) cannot get in between.
    ///
    /// The stereo world's eye pictures work the same way with three sets
    /// (`eye_views[set][eye]`), so nothing waits: the headset thread marks
    /// the set it shows as `reading` when its frame starts, and the window
    /// draws into the set that is neither that one nor the published one.
    #[derive(Default)]
    struct Source {
        generation: u64,
        views: Option<[wgpu::TextureView; 2]>,
        front: usize,
        eye_generation: u64,
        eye_views: Option<[[wgpu::TextureView; 2]; 3]>,
        /// The published eye set, the eye poses it was drawn for and its
        /// number (one per game frame); `None`: no stereo world (menus),
        /// the panel is the picture.
        world: Option<(usize, [EyePose; 2], u64)>,
        reading: Option<usize>,
        /// A blank game frame with no stereo picture to keep (the game's
        /// start): the panel is clear and shown blended, i.e. nothing.
        blank: bool,
        /// What kind of view the headset shows (w8, the fade on a change):
        /// 0 flat screens, 1 flight, 2 island model, 3 title, 4 badge
        /// celebration. Blank frames keep the one before.
        scene: u8,
    }

    /// Where the eyes will be for the next headset frame (headset thread →
    /// window thread, which draws the next game frame's eye pictures for
    /// them).
    static NEXT_EYES: Mutex<Option<[EyePose; 2]>> = Mutex::new(None);
    /// Where the player sits (headset thread → window thread): the eye
    /// pictures are drawn for the eyes relative to it (`Seat::seated`).
    static SEAT: Mutex<Option<Seat>> = Mutex::new(None);

    /// The stereo world (`PW64_VR_STEREO=0` keeps everything on the panel).
    fn stereo_on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("PW64_VR_STEREO").is_none_or(|v| v != "0"))
    }

    /// Game units per metre: 1 (the game's physics is in metres and m/s).
    /// Larger makes the world smaller around the player.
    fn world_scale() -> f32 {
        static SCALE: OnceLock<f32> = OnceLock::new();
        *SCALE.get_or_init(|| {
            std::env::var("PW64_VR_WORLD_SCALE")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| (0.01..=100.0).contains(v))
                .unwrap_or(1.0)
        })
    }

    /// Nearest distance drawn, in metres (the game's own near plane is 1).
    const NEAR_M: f32 = 0.1;

    /// One line about a stereo frame's world: its draws in order, as runs
    /// of one world view (`v<n>`), with per run the draw count, the depth
    /// state (`t` test, `w` write) and, in the game camera's space (metres,
    /// y up, -z ahead), the nearest and farthest vertex and the lowest and
    /// highest one relative to the camera; `|` = a depth clear between.
    pub(super) fn describe(frame: &pw64_gfx::Frame) -> String {
        let mut out = String::from("world:");
        let mut i = 0;
        let draws = &frame.draws;
        while i < draws.len() {
            let d = &draws[i];
            if d.view == 0 {
                if d.pipeline.shader.mode == pw64_gfx::frame::ShaderMode::DepthClear {
                    out += " |";
                }
                i += 1;
                continue;
            }
            let (view, depth) = (d.view, d.pipeline.depth);
            let mut n = 0;
            let (mut near, mut far, mut low, mut high) = (f32::MAX, 0f32, f32::MAX, f32::MIN);
            while i < draws.len() && draws[i].view == view && draws[i].pipeline.depth == depth {
                let d = &draws[i];
                let m = &frame.views[view as usize - 1];
                let first = d.first_vertex as usize;
                for v in &frame.vertices[first..first + d.vertex_count as usize] {
                    let c: [f32; 4] =
                        [0, 1, 2, 3].map(|j| (0..4).map(|k| v.pos[k] * m[k][j]).sum());
                    if c[3].abs() < 1e-9 {
                        continue;
                    }
                    let p = [c[0] / c[3], c[1] / c[3], c[2] / c[3]];
                    let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
                    (near, far) = (near.min(r), far.max(r));
                    (low, high) = (low.min(p[1]), high.max(p[1]));
                }
                n += 1;
                i += 1;
            }
            out += &format!(
                " v{view}x{n}{}{} r {near:.0}..{far:.0} y {low:.0}..{high:.0};",
                if depth.test { "t" } else { "" },
                if depth.write { "w" } else { "" },
            );
        }
        out
    }

    /// The matrices one eye's picture is drawn with
    /// (`Renderer::render_stereo`) for `pose` in the seated space. The
    /// game's camera sits at the seated origin (where the head was when the
    /// session started or the player last recentred), looking the same
    /// way; the head moves around it.
    pub(super) fn stereo_eye(pose: &EyePose) -> pw64_gfx::StereoEye {
        stereo_eye_panel_at(pose, 0.0)
    }

    /// [`stereo_eye`] with the panel (the N64 screen's 2D draws) `up`
    /// metres higher than usual.
    fn stereo_eye_panel_at(pose: &EyePose, up: f32) -> pw64_gfx::StereoEye {
        // pw64-xr's column-vector matrices are stored column-major, which
        // is pw64-gfx's row-vector layout: the same arrays, multiplied in
        // the opposite order.
        use pw64_gfx::matrix::mul;
        let [l, r, u, d] = pose.fov;
        let scale = world_scale();
        let projection = |near| math::projection_reversed(l, r, u, d, near);
        let world = mul(
            &math::view(pose.orientation, pose.position.map(|v| v * scale)),
            &projection(NEAR_M * scale),
        );
        // The N64 screen (320x240, y down) on the panel's plane, in metres.
        let s = PANEL_HEIGHT_M / pw64_gfx::N64_HEIGHT;
        let on_panel: Mat4 = [
            [s, 0.0, 0.0, 0.0],
            [0.0, -s, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
            [
                -0.5 * pw64_gfx::N64_WIDTH * s,
                0.5 * pw64_gfx::N64_HEIGHT * s + up,
                -PANEL_DISTANCE_M,
                1.0,
            ],
        ];
        let mut hud = mul(
            &mul(&on_panel, &math::view(pose.orientation, pose.position)),
            &projection(NEAR_M),
        );
        // Everything on the panel has one depth; a little of each draw's
        // own depth on top keeps 3D HUD parts sorted among themselves.
        hud[2][2] += 0.05;
        let layout = super::hud_layout();
        // The gyrocopter's aim reticle is placed on this sphere by the C
        // side, in its pixels (pw64_vr_aim_reticle_y).
        pw64_game::set_vr_hud_rad_per_px(HUD_RAD_PER_PX * layout.size);
        pw64_game::set_vr_gyro_windscreen(super::gyro_windscreen());
        let hud_sphere = pw64_gfx::HudSphere {
            eye: mul(
                &math::view(pose.orientation, pose.position),
                &projection(NEAR_M),
            ),
            radius: layout.distance,
            rad_per_px: HUD_RAD_PER_PX * layout.size,
            spread: layout.spread_xy(),
        };
        pw64_gfx::StereoEye {
            world,
            hud,
            clip: None,
            background: None,
            hud_sphere: Some(hud_sphere),
            hud_hidden: super::hud_hidden(),
            hud_dash: if super::dash_gauges() {
                &DASH_SLOTS
            } else {
                &[]
            },
            sight: None,
            hide_env: false,
            hide_hud: false,
        }
    }

    /// The gauges on the cockpit's dashboard ([`pw64_gfx::DashSlot`]; the
    /// game sends a point on the dashboard's surface for each,
    /// `pw64_vr_gyro_dash`): the throttle (`hud_edit::ELEMENTS` id 3, about
    /// 62 x 32 N64 px, 12 x 6 cm) just below the dashboard's tip, the map
    /// (radar, id 6, 70 x 70 px, 10 cm across) lower, on the left side.
    const DASH_SLOTS: [pw64_gfx::DashSlot; 2] = [
        pw64_gfx::DashSlot {
            elem: 3,
            point: 0,
            offset: [0.0; 2],
            m_per_px: 0.002,
        },
        pw64_gfx::DashSlot {
            elem: 6,
            point: 1,
            offset: [0.0; 2],
            m_per_px: 0.0014,
        },
    ];

    /// The flight HUD on a sphere around the player ([`pw64_gfx::HudSphere`],
    /// placed by [`super::HudLayout`]): the size of an N64 pixel at size 1,
    /// radians (the flat panel's was about 0.0036: the gauges keep about
    /// their size). With the default spread a gauge at the screen's left
    /// edge ends up about 72 degrees to the left: a turn of the head.
    const HUD_RAD_PER_PX: f32 = 0.0034;

    /// The island model (outside flight, [`pw64_gfx::Diorama`]): how wide
    /// it is, in metres. 2.2, then 25% bigger (a zoom towards it; the
    /// centre stays), then 5% smaller again (the user, in the headset).
    const MODEL_DIAMETER_M: f32 = 2.6125;
    /// Where its centre (the island's middle at sea level) sits in the
    /// seated space, in metres: on a table below the menus, about where a
    /// desk would be (from about 0.3 to 2.5 m ahead). The first try, 3 m
    /// wide behind the panel, was hidden by the level select's big menus;
    /// the second, 1.6 m, was a little far away.
    const MODEL_CENTRE_M: [f32; 3] = [0.0, -0.65, -1.4];
    /// How much higher the menus float while the model is shown, so they
    /// clear it: their lower edge about 13 degrees below eye level.
    const MODEL_PANEL_UP_M: f32 = 0.5;
    /// Around the model: a dim, slightly blue room.
    const MODEL_BACKGROUND: [f64; 4] = [0.012, 0.016, 0.026, 1.0];
    /// ...or, where the headset can show it (passthrough, p12), the
    /// player's own room: nothing (transparent black, premultiplied).
    const MODEL_BACKGROUND_ROOM: [f64; 4] = [0.0; 4];
    /// p13: when the last photo was taken (window thread → headset
    /// thread, which blinks the whole view to black and back).
    static SHUTTER_AT: Mutex<Option<Instant>> = Mutex::new(None);
    /// The blink: closing, then opening again.
    const SHUTTER_CLOSE: Duration = Duration::from_millis(70);
    const SHUTTER_OPEN: Duration = Duration::from_millis(130);

    /// How black the view is `t` after a photo was taken (0 = clear).
    fn shutter_alpha(t: Duration) -> f32 {
        if t < SHUTTER_CLOSE {
            t.as_secs_f32() / SHUTTER_CLOSE.as_secs_f32()
        } else {
            1.0 - ((t - SHUTTER_CLOSE).as_secs_f32() / SHUTTER_OPEN.as_secs_f32()).min(1.0)
        }
    }

    /// The views shown in the player's room where the headset can
    /// (`Source::scene`): the flat screens and the island model if the
    /// player wants the menus there (`menus`: the Passthrough setting),
    /// and a flight shown as a model in the room (scene 5: Diorama mode
    /// passthrough or passthrough static; 6 is the model in the dim room).
    fn in_room(scene: u8, menus: bool) -> bool {
        match scene {
            0 | 2 => menus,
            5 => true,
            _ => false,
        }
    }

    /// Diorama mode, following: where the vehicle is held, in the seated
    /// space (metres): a metre ahead, a little below eye level.
    const TABLE_VEHICLE_M: [f32; 3] = [0.0, -0.25, -1.0];
    /// ...and the right stick's zoom range, as a factor of the menus'
    /// model at its default size (zoom 1, where a flight starts): back to
    /// 1.5 times the distance, and in to 7.5 times the size (round 1 went
    /// from the menus' full zoom, 2.5, to twice that, 5; the user: half as
    /// far in again).
    const TABLE_ZOOM_RANGE: (f32, f32) = (1.0 / 1.5, 7.5);

    /// [`stereo_eye`] for a flight shown as a model (Diorama mode,
    /// [`pw64_gfx::Table`]): the island at the scale of the menus' model
    /// times `zoom`, game north away, level. `follow`: moved so the vehicle
    /// stays at [`TABLE_VEHICLE_M`] and the land slides past below it;
    /// else the island stands still where the menus' model does
    /// ([`MODEL_CENTRE_M`], its middle at sea level). No sky and no sea;
    /// around it the player's room (`room`) or the dim one.
    pub(super) fn table_eye(
        pose: &EyePose,
        t: &pw64_gfx::Table,
        zoom: f32,
        follow: bool,
        room: bool,
    ) -> pw64_gfx::StereoEye {
        use pw64_gfx::matrix::mul;
        let mut eye = stereo_eye(pose);
        let [x0, y0, _, x1, y1, _] = t.bounds;
        let radius = 0.5 * (x1 - x0).max(y1 - y0).max(1.0);
        let s = MODEL_DIAMETER_M / (2.0 * radius) * zoom;
        // Game world → relative to the vehicle, or to the island's middle
        // at sea level.
        let (o, c) = if follow {
            (t.vehicle, TABLE_VEHICLE_M)
        } else {
            ([0.5 * (x0 + x1), 0.5 * (y0 + y1), 0.0], MODEL_CENTRE_M)
        };
        let centre: Mat4 = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-o[0], -o[1], -o[2], 1.0],
        ];
        // → the seated space (y up, -z away), scaled down and placed.
        let place: Mat4 = [
            [s, 0.0, 0.0, 0.0],
            [0.0, 0.0, -s, 0.0],
            [0.0, s, 0.0, 0.0],
            [c[0], c[1], c[2], 1.0],
        ];
        let [l, r, u, dn] = pose.fov;
        let view = mul(
            &math::view(pose.orientation, pose.position),
            &math::projection_reversed(l, r, u, dn, NEAR_M * 0.5),
        );
        let to_world = pw64_gfx::Diorama::to_world(&t.camera);
        eye.world = mul(&mul(&mul(&to_world, &centre), &place), &view);
        eye.hide_env = true;
        eye.background = Some(if room {
            MODEL_BACKGROUND_ROOM
        } else {
            MODEL_BACKGROUND
        });
        eye
    }

    /// The title view: how far behind and above (negative: below) the game camera, in game
    /// metres.
    /// 12 m behind hid the logo behind the menu; 5 m, 3 m below it, so
    /// the logo floats above the menu; then 2 m forward and 2 m up, then
    /// 2 m forward and 0.25 m down again (the user, in the headset).
    const TITLE_BACK_M: f32 = 1.0;
    const TITLE_UP_M: f32 = -1.25;

    /// Horizon lock (c5): the camera-space rotation (row vectors, as
    /// [`pw64_gfx::StereoEye::world`]) that levels the view, from `up`,
    /// the game world's up in camera space (x right, y up, -z forward;
    /// [`pw64_gfx::Frame::up`]). Bank: about the view's forward axis until
    /// up has no sideways part (the view still pitches). Full: the
    /// shortest turn of up onto straight up (only the heading is left).
    /// `None`: off, or no correction to make.
    pub(super) fn horizon_correction(up: [f32; 3], mode: super::HorizonLock) -> Option<Mat4> {
        use super::HorizonLock;
        let len = (up[0] * up[0] + up[1] * up[1] + up[2] * up[2]).sqrt();
        if len < 1e-6 || mode == HorizonLock::Off {
            return None;
        }
        let u = up.map(|c| c / len);
        let bank = || {
            // Looking (nearly) straight up or down: no bank to speak of.
            let h = u[0].hypot(u[1]);
            (h > 0.05).then(|| {
                let (s, c) = (u[0] / h, u[1] / h);
                [
                    [c, s, 0.0, 0.0],
                    [-s, c, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ]
            })
        };
        if mode == HorizonLock::Bank {
            return bank();
        }
        // Rodrigues: axis k = u x y (normalised), angle between u and y.
        let k = [-u[2], 0.0, u[0]];
        let sin = k[0].hypot(k[2]);
        let cos = u[1];
        if sin < 1e-4 {
            // Already level, or upside down (no unique turn): bank only.
            return if cos > 0.0 { None } else { bank() };
        }
        let k = k.map(|c| c / sin);
        let turn = |v: [f32; 3]| {
            let kv = k[0] * v[0] + k[1] * v[1] + k[2] * v[2];
            let kx = [
                k[1] * v[2] - k[2] * v[1],
                k[2] * v[0] - k[0] * v[2],
                k[0] * v[1] - k[1] * v[0],
            ];
            std::array::from_fn::<f32, 3, _>(|i| v[i] * cos + kx[i] * sin + k[i] * kv * (1.0 - cos))
        };
        let [x, y, z] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(turn);
        Some([
            [x[0], x[1], x[2], 0.0],
            [y[0], y[1], y[2], 0.0],
            [z[0], z[1], z[2], 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    /// A camera world matrix (rows right, forward, up, position; z up)
    /// turned level: facing the same way, no pitch or roll.
    fn level(camera: [[f32; 3]; 4]) -> [[f32; 3]; 4] {
        let f = camera[1];
        let len = f[0].hypot(f[1]);
        let forward = if len > 1e-4 {
            [f[0] / len, f[1] / len, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        [
            [forward[1], -forward[0], 0.0],
            forward,
            [0.0, 0.0, 1.0],
            camera[3],
        ]
    }

    /// [`stereo_eye`] for a world view shown as a small model of the island
    /// ([`pw64_gfx::Diorama`]): the model stays put in front of the player
    /// whatever the game's camera does (the title fly-over and menu cameras
    /// swoop and cut, which was dizzying as the player's own head). The
    /// island's middle at sea level is the model's centre, the terrain's
    /// larger side its diameter; game north (+y) is away from the player.
    /// Cut to that cylinder and a little above the highest terrain, which
    /// leaves out the far sea and the sky dome.
    /// `spin`: the island model's turn (radians about its vertical axis)
    /// and tilt (radians, far side up) from the right stick, and its zoom
    /// (L / R) (p8).
    pub(super) fn diorama_eye(
        pose: &EyePose,
        d: &pw64_gfx::Diorama,
        spin: [f32; 3],
    ) -> pw64_gfx::StereoEye {
        use pw64_gfx::matrix::mul;
        // The title: full size, following the game camera (which the
        // title's patch slows down in VR), level and a little behind it,
        // so the pilots flying by and the logo are seen from further back.
        // A badge celebration: the same at full size, level, with no
        // offset: its camera stands still while the pilots fly past.
        if d.title || d.celebration {
            let (back, up) = if d.title {
                (TITLE_BACK_M, TITLE_UP_M)
            } else if matches!(d.vehicle, 1 | 2) {
                // Rocket belt, gyrocopter: their celebration camera sits a
                // little high (the user, in the headset).
                (0.0, -1.0)
            } else {
                (0.0, 0.0)
            };
            let mut eye = stereo_eye(pose);
            eye.hud_sphere = None;
            let mut at = level(d.camera);
            let [fwd, pos] = [at[1], at[3]];
            at[3] = [pos[0] - fwd[0] * back, pos[1] - fwd[1] * back, pos[2] + up];
            if let Some(world_to_at) = pw64_gfx::matrix::inverse(&pw64_gfx::Diorama::to_world(&at))
            {
                eye.world = mul(&mul(&d.camera_to_world(), &world_to_at), &eye.world);
            }
            return eye;
        }
        let mut eye = stereo_eye_panel_at(pose, MODEL_PANEL_UP_M);
        eye.hud_sphere = None;
        let [x0, y0, _, x1, y1, z1] = d.bounds;
        let radius = 0.5 * (x1 - x0).max(y1 - y0).max(1.0);
        let top = z1.max(0.0) + 0.15 * radius;
        let s = MODEL_DIAMETER_M / (2.0 * radius) * spin[2];
        // Game world → relative to the island's middle at sea level.
        let centre: Mat4 = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-0.5 * (x0 + x1), -0.5 * (y0 + y1), 0.0, 1.0],
        ];
        let model = mul(&d.camera_to_world(), &centre);
        // → the seated space (y up, -z away), scaled down and placed.
        let c = MODEL_CENTRE_M;
        let place: Mat4 = [
            [s, 0.0, 0.0, 0.0],
            [0.0, 0.0, -s, 0.0],
            [0.0, s, 0.0, 0.0],
            [c[0], c[1], c[2], 1.0],
        ];
        let [l, r, u, dn] = pose.fov;
        let view = mul(
            &math::view(pose.orientation, pose.position),
            &math::projection_reversed(l, r, u, dn, NEAR_M * 0.5),
        );
        // The right stick's turn and tilt, about the island's middle (the
        // cut to its cylinder stays in the island's own frame, `model`).
        let (sy, cy) = spin[0].sin_cos();
        let (st, ct) = spin[1].sin_cos();
        let turn: Mat4 = [
            [cy, sy, 0.0, 0.0],
            [-sy, cy, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let tilt: Mat4 = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, ct, st, 0.0],
            [0.0, -st, ct, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        eye.world = mul(&mul(&mul(&mul(&model, &turn), &tilt), &place), &view);
        eye.clip = Some(pw64_gfx::StereoClip { model, radius, top });
        eye.background = Some(MODEL_BACKGROUND);
        eye
    }

    pub(crate) struct Vr {
        /// Until the first [`Vr::target`] call starts the headset thread.
        xr: Option<XrContext>,
        source: Arc<Mutex<Source>>,
        panel: Option<[(wgpu::Texture, wgpu::TextureView); 2]>,
        /// The texture of `panel` the window draws the next frame into.
        back: usize,
        /// One eye's picture size (the runtime's recommendation).
        eye_size: (u32, u32),
        /// The stereo world's eye pictures: `[set][eye]`, with their format.
        eyes: Option<(wgpu::TextureFormat, [[wgpu::TextureView; 2]; 3])>,
        /// The headset shows the stereo world (the last game frame had one).
        stereo: bool,
        /// Blank game frames ([`pw64_gfx::Frame::is_blank`], between
        /// screens) are being skipped: the last stereo picture stays.
        holding: bool,
        /// ...and with no stereo picture to keep: nothing is shown.
        void: bool,
        /// The last flat (no world) frame's makeup logged: draws, textured
        /// draws, blank ([`Self::world`], for tracking down transitions).
        flat_logged: Option<(usize, usize, bool)>,
        /// Lines of that logged since the last frame with a world (capped:
        /// 2D screens change their draw count often).
        flat_lines: u32,
        /// p8: the island model's turn and tilt from the right stick
        /// (radians) and its zoom (L / R), when the stick was last read,
        /// and a camera distance change not saved yet.
        spin: [f32; 3],
        stick_at: Instant,
        chase_dirty: bool,
        /// The last stereo frame was the island model ([`diorama_eye`]).
        model: bool,
        /// Diorama mode: the last stereo frame was a flight shown as a
        /// model ([`table_eye`]), and the right stick's zoom
        /// ([`TABLE_ZOOM_RANGE`]; 1 again after a menu).
        table: bool,
        table_zoom: f32,
        /// ...and the title screen (recentre when it starts).
        title: bool,
        /// The last game state and title phase logged.
        state: Option<(i32, i32)>,
        /// Eye picture sets drawn so far.
        eye_serial: u64,
        /// When the world was last described in the log ([`describe`]).
        described: Instant,
        /// How long the last eye picture sets took from the game's cue to
        /// being published, in microseconds (a ring, for [`lead`]).
        cue_to_eyes: [u32; 256],
        /// The window thread's probe bar (`probe_on`).
        probe: Option<Bar>,
    }

    impl Vr {
        /// Connects to the OpenXR runtime and builds the window's GPU
        /// context on the device it picks. `Err`: a one-line reason; the
        /// caller starts flat.
        pub(crate) fn connect(window: &Arc<Window>) -> Result<(GpuContext, Self), String> {
            // The optional features pw64-gfx asks for (`device_descriptor`).
            let features = wgpu::Features::DEPTH32FLOAT_STENCIL8
                | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
                | wgpu::Features::PIPELINE_CACHE;
            #[cfg(not(target_os = "android"))]
            let xr = XrContext::new(features, Panel::default());
            #[cfg(target_os = "android")]
            let xr = {
                let app = crate::android::app();
                // SAFETY: the process's VM and the activity's global
                // reference; the activity outlives the session (the process
                // exits with it).
                let app =
                    unsafe { pw64_xr::AndroidApp::new(app.vm_as_ptr(), app.activity_as_ptr()) };
                XrContext::new(features, Panel::default(), &app)
            };
            let xr = xr.map_err(|e| e.to_string())?;
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
            let eye_size = xr.eye_size();
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
                    back: 1,
                    eye_size,
                    eyes: None,
                    stereo: false,
                    holding: false,
                    void: false,
                    flat_logged: None,
                    flat_lines: 0,
                    spin: [0.0, 0.0, 1.0],
                    stick_at: Instant::now(),
                    chase_dirty: false,
                    model: false,
                    table: false,
                    table_zoom: 1.0,
                    title: false,
                    state: None,
                    eye_serial: 0,
                    described: Instant::now(),
                    cue_to_eyes: [0; 256],
                    probe: None,
                },
            ))
        }

        /// The texture the window draws its frame into instead of the
        /// surface (same format and size as the surface, so every pass
        /// works unchanged); [`Self::publish`] it when the frame is
        /// complete. Starts the headset thread on the first call.
        pub(crate) fn target(
            &mut self,
            device: &wgpu::Device,
            format: wgpu::TextureFormat,
            size: (u32, u32),
        ) -> Option<wgpu::TextureView> {
            let stale = self
                .panel
                .as_ref()
                .is_none_or(|[(t, _), _]| (t.width(), t.height()) != size || t.format() != format);
            if stale {
                let panel = [0, 1].map(|_| {
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
                    (texture, view)
                });
                let mut s = self.source.lock().unwrap_or_else(|e| e.into_inner());
                s.generation += 1;
                s.views = Some([panel[0].1.clone(), panel[1].1.clone()]);
                s.front = 0;
                drop(s);
                self.back = 1;
                self.panel = Some(panel);
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
            self.panel.as_ref().map(|p| p[self.back].1.clone())
        }

        /// The frame drawn into [`Self::target`]'s texture is complete:
        /// the headset shows it from now on, and the next frame is drawn
        /// into the other texture.
        pub(crate) fn publish(&mut self) {
            if self.panel.is_none() {
                return;
            }
            self.source.lock().unwrap_or_else(|e| e.into_inner()).front = self.back;
            self.back ^= 1;
        }

        /// A new game frame is shown (window.rs `drain`): `frame` is the
        /// display list behind it. With a 3D world view in it, draws both
        /// eyes' pictures of that world for where the head will be next
        /// frame and hands them to the headset thread; without, the
        /// headset goes back to the panel.
        pub(crate) fn world(
            &mut self,
            renderer: &mut pw64_gfx::Renderer,
            device: &wgpu::Device,
            format: wgpu::TextureFormat,
            frame: Option<&pw64_gfx::Frame>,
        ) {
            let poses = *NEXT_EYES.lock().unwrap_or_else(|e| e.into_inner());
            // p13: a photo was taken: the headset blinks.
            if frame.is_some_and(|f| f.shutter) {
                *SHUTTER_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                log("photo shutter: blink", true);
            }
            // Between screens the game shows blank frames (black fills,
            // loads): keep the last stereo picture (the runtime reprojects
            // it with the head) and the panel clear over it, rather than
            // the panel's black picture, until there is something to show.
            // Every flat frame's makeup, logged when it changes.
            if let Some(f) = frame.filter(|f| !f.has_world()) {
                let textured = f
                    .draws
                    .iter()
                    .filter(|d| d.textures.iter().any(Option::is_some))
                    .count();
                let makeup = (f.draws.len(), textured, f.is_blank());
                if self.flat_logged != Some(makeup) && self.flat_lines < 12 {
                    self.flat_logged = Some(makeup);
                    self.flat_lines += 1;
                    log(
                        &format!(
                            "flat game frame: {} draws, {} textured, blank {}",
                            makeup.0, makeup.1, makeup.2
                        ),
                        true,
                    );
                }
            } else {
                self.flat_logged = None;
                self.flat_lines = 0;
            }
            if frame.is_some_and(pw64_gfx::Frame::is_blank) {
                if self.stereo {
                    if !self.holding {
                        self.holding = true;
                        log("blank game frame: holding the last stereo picture", true);
                    }
                } else if !self.void {
                    // Nothing to hold (the game's start): show nothing
                    // rather than the panel's black picture.
                    self.void = true;
                    let mut src = self.source.lock().unwrap_or_else(|e| e.into_inner());
                    src.blank = true;
                    src.scene = 0;
                    log("blank game frame: showing nothing", true);
                }
                return;
            }
            self.holding = false;
            if self.void {
                self.void = false;
                self.source.lock().unwrap_or_else(|e| e.into_inner()).blank = false;
            }
            let (Some(frame), Some(poses)) =
                (frame.filter(|f| stereo_on() && f.has_world()), poses)
            else {
                if self.stereo {
                    self.stereo = false;
                    STEREO_LEAD_US.store(0, Ordering::Relaxed);
                    self.cue_to_eyes = [0; 256];
                    let mut src = self.source.lock().unwrap_or_else(|e| e.into_inner());
                    src.world = None;
                    src.scene = 0;
                    drop(src);
                    // What the panel shows instead, for tracking down
                    // transitions that still look wrong.
                    let (draws, textured) = frame.map_or((0, 0), |f| {
                        let t = f
                            .draws
                            .iter()
                            .filter(|d| d.textures.iter().any(Option::is_some));
                        (f.draws.len(), t.count())
                    });
                    log(
                        &format!(
                            "stereo world off: panel frame {draws} draws, {textured} textured"
                        ),
                        true,
                    );
                }
                return;
            };
            if self.eyes.as_ref().is_none_or(|(f, _)| *f != format) {
                let views = [0, 1, 2].map(|_| {
                    [0, 1].map(|_| {
                        device
                            .create_texture(&wgpu::TextureDescriptor {
                                label: Some("vr eye picture"),
                                size: wgpu::Extent3d {
                                    width: self.eye_size.0.max(1),
                                    height: self.eye_size.1.max(1),
                                    depth_or_array_layers: 1,
                                },
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: wgpu::TextureDimension::D2,
                                format,
                                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                    | wgpu::TextureUsages::TEXTURE_BINDING,
                                view_formats: &[],
                            })
                            .create_view(&Default::default())
                    })
                });
                let mut s = self.source.lock().unwrap_or_else(|e| e.into_inner());
                s.eye_generation += 1;
                s.eye_views = Some(views.clone());
                s.world = None;
                s.reading = None;
                drop(s);
                self.eyes = Some((format, views));
            }
            let Some((_, views)) = &self.eyes else { return };
            let back = {
                let s = self.source.lock().unwrap_or_else(|e| e.into_inner());
                let front = s.world.map(|w| w.0);
                (0..3)
                    .find(|i| Some(*i) != front && Some(*i) != s.reading)
                    .unwrap_or(0)
            };
            let seat = SEAT
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .unwrap_or_default();
            // p8, the right stick: in the menus it turns and tilts the
            // island model; in a chase view it moves the camera nearer or
            // further (saved per vehicle when the stick is let go).
            let now = Instant::now();
            let dt = now.duration_since(self.stick_at).as_secs_f32().min(0.1);
            self.stick_at = now;
            let stick = crate::input::right_stick().map(|v| {
                let m = (v.abs() - STICK_DEADZONE).max(0.0) / (1.0 - STICK_DEADZONE);
                m.copysign(v)
            });
            // Diorama mode: a flight as a model, unless the game's camera
            // is the first-person one (R on the pad). In the player's room
            // if the mode says so and the headset can.
            let mode = super::diorama_mode();
            let table = frame.table.filter(|_| {
                frame.diorama.is_none()
                    && mode != super::DioramaMode::Off
                    && frame.up.is_some_and(|u| u[4] as i32 != 1)
            });
            let table_room = mode.passthrough() && super::passthrough_available();
            if table.is_some() {
                // The right stick: up = closer.
                self.table_zoom = (self.table_zoom * (stick[1] * ZOOM_RATE * dt).exp())
                    .clamp(TABLE_ZOOM_RANGE.0, TABLE_ZOOM_RANGE.1);
            }
            let camera = frame
                .up
                .filter(|_| frame.diorama.is_none() && table.is_none())
                .and_then(|u| match u[4] as i32 {
                    m if CHASE_MODES.contains(&m) => Some(super::ViewKind::Chase),
                    1 => Some(super::ViewKind::FirstPerson),
                    _ => None,
                })
                .zip(pw64_game::vr_flight_vehicle());
            match &frame.diorama {
                Some(d) if !d.title && !d.celebration => {
                    self.spin[0] += stick[0] * SPIN_RAD_PER_S * dt;
                    self.spin[1] =
                        (self.spin[1] + stick[1] * TILT_RAD_PER_S * dt).clamp(-TILT_MAX, TILT_MAX);
                    // L / R: zoom out / in.
                    let b = crate::input::p1_buttons();
                    let dir = f32::from(u8::from(b & crate::input::CONT_R != 0))
                        - f32::from(u8::from(b & crate::input::CONT_L != 0));
                    self.spin[2] = (self.spin[2] * (dir * ZOOM_RATE * dt).exp())
                        .clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
                    // The next flight's model starts at its default size.
                    self.table_zoom = 1.0;
                }
                _ => {}
            }
            if let Some((kind, v)) = camera {
                if stick[1] != 0.0 {
                    // Up: nearer / forward.
                    let speed = match kind {
                        super::ViewKind::Chase => CHASE_M_PER_S,
                        super::ViewKind::FirstPerson => CHASE_M_PER_S * 0.5,
                    };
                    super::nudge_offset(kind, v, -stick[1] * speed * dt);
                    self.chase_dirty = true;
                } else if self.chase_dirty {
                    self.chase_dirty = false;
                    super::save_offsets();
                }
            }
            let chase_back = camera.map(|(kind, v)| super::view_offset(kind, v));
            // Horizon lock, in flight (one rotation for both eyes); the
            // fixed sights on the HUD sphere turn by it too.
            let horizon = (frame.diorama.is_none() && table.is_none())
                .then(|| {
                    frame.up.filter(|u| u[3] != 1.0).and_then(|u| {
                        // (The cannonball has no lock: aiming must pitch.)
                        horizon_correction([u[0], u[1], u[2]], super::horizon_lock())
                    })
                })
                .flatten();
            let mut eyes = Vec::with_capacity(poses.len());
            for pose in &poses {
                let seated = seat.seated(pose);
                let mut matrices = match (&frame.diorama, &table) {
                    (Some(d), _) => diorama_eye(&seated, d, self.spin),
                    (None, Some(t)) => {
                        table_eye(&seated, t, self.table_zoom, mode.follows(), table_room)
                    }
                    (None, None) => stereo_eye(&seated),
                };
                // p12: the island model in the player's room.
                if matrices.clip.is_some() && super::passthrough_on() {
                    matrices.background = Some(MODEL_BACKGROUND_ROOM);
                }
                // Diorama mode: no flight HUD while it is on, in the model
                // view and in the game's first-person view alike.
                if frame.diorama.is_none() && mode != super::DioramaMode::Off {
                    matrices.hide_hud = true;
                }
                // Horizon lock, in flight: the world level, the vehicle
                // (and its cockpit, in camera space) banking around the
                // player. The HUD sphere stays with the seat.
                if let Some(r) = &horizon {
                    matrices.world = pw64_gfx::matrix::mul(r, &matrices.world);
                    matrices.sight = Some(std::array::from_fn(|k| [r[k][0], r[k][1], r[k][2]]));
                }
                // p8: the camera that much further back (or forward) along
                // its view (camera space, -z forward; before the horizon
                // lock's turn, so the vehicle stays centred).
                if let Some(back) = chase_back.filter(|b| *b != 0.0) {
                    let mut t = pw64_gfx::matrix::IDENTITY;
                    t[3][2] = -back;
                    matrices.world = pw64_gfx::matrix::mul(&t, &matrices.world);
                }
                eyes.push(matrices);
            }
            // q6: both eyes in one call (one upload, one submit).
            let targets: Vec<_> = eyes
                .iter()
                .enumerate()
                .map(|(eye, m)| (&views[back][eye], m))
                .collect();
            renderer.render_stereo_eyes(frame, self.eye_size, &targets);
            self.eye_serial += 1;
            {
                let mut src = self.source.lock().unwrap_or_else(|e| e.into_inner());
                src.world = Some((back, poses, self.eye_serial));
                // w8: the kind of view, for the headset's fade on a change.
                src.scene = match &frame.diorama {
                    None if table.is_some() && table_room => 5,
                    None if table.is_some() => 6,
                    None => 1,
                    Some(d) if d.title => 3,
                    Some(d) if d.celebration => 4,
                    Some(_) => 2,
                };
            }
            // The lead for the next cues: the slowest of the last 256 sets
            // (a few seconds) plus a millisecond, in whole milliseconds, at
            // least the panel's 6 ms and at most a frame less a millisecond.
            // Whole steps over a long window, so it rarely changes: every
            // change moves the cue, i.e. makes one game frame's time step
            // differ from the time it is shown for. Hitches (level loads)
            // hit the cap and fall out of the ring again.
            // Not for a redraw while paused (a VR HUD change): no cue then.
            let cued = *CUED_AT.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cued) = cued.filter(|t| t.elapsed() < Duration::from_millis(100)) {
                let us = cued.elapsed().as_micros().min(u32::MAX as u128) as u32;
                self.cue_to_eyes[(self.eye_serial % 256) as usize] = us;
                let worst = self.cue_to_eyes.iter().copied().max().unwrap_or(0);
                let period_us = 1_000_000_000 / HEADSET_MHZ.load(Ordering::Relaxed).max(1);
                let lead = (worst + 1000)
                    .next_multiple_of(1000)
                    .clamp(6000, period_us.saturating_sub(1000).max(6000));
                STEREO_LEAD_US.store(lead, Ordering::Relaxed);
            }
            if !self.stereo {
                self.stereo = true;
                self.described = Instant::now() - Duration::from_secs(60);
                log(
                    &format!(
                        "stereo world on: {} draws, {} world views, eye {:?}, MSAA {}, flat \
                         picture {:?} (its world left out)",
                        frame.draws.len(),
                        frame.views.len(),
                        self.eye_size,
                        renderer.options.msaa,
                        renderer.fb_size()
                    ),
                    true,
                );
            }
            let title = frame.diorama.is_some_and(|d| d.title);
            if title && !self.title {
                // A fresh start for the intro: forward is where the player
                // faces as it begins.
                super::request_recentre();
            }
            self.title = title;
            if table.is_some() != self.table {
                self.table = table.is_some();
                log(
                    &match &table {
                        Some(t) => format!(
                            "diorama mode on ({}): vehicle {:?}, bounds {:?}",
                            mode.label(),
                            t.vehicle,
                            t.bounds
                        ),
                        None => "diorama mode off".into(),
                    },
                    true,
                );
            }
            if frame.diorama.is_some() != self.model {
                self.model = frame.diorama.is_some();
                let what = match &frame.diorama {
                    Some(d) if d.celebration => "badge celebration: full size, level".into(),
                    Some(d) if d.title => {
                        format!("title: following its camera, island bounds {:?}", d.bounds)
                    }
                    Some(d) => format!("island model on: bounds {:?}", d.bounds),
                    None => "island model off".into(),
                };
                log(&what, true);
            }
            let state = frame.diorama.map(|d| (d.state, d.title_phase));
            if let Some((game, phase)) = state
                && state != self.state
            {
                log(&format!("game state {game}, title phase {phase}"), true);
            }
            if state.is_some() {
                self.state = state;
            }
            // What the world is made of, every 10 s in flight (vr.log and
            // logcat): for tracking down what a strange shape in the
            // headset is.
            if self.described.elapsed() >= Duration::from_secs(10) {
                self.described = Instant::now();
                log(&describe(frame), true);
            }
        }

        /// The headset shows the stereo world: the panel then only carries
        /// what the window draws over the game (toasts, the settings menu).
        pub(crate) fn stereo(&self) -> bool {
            self.stereo
        }

        /// A blank game frame with no stereo picture to keep: the panel
        /// starts out clear too (and is shown blended), so the headset shows
        /// nothing but what the window draws over the game.
        pub(crate) fn blank(&self) -> bool {
            self.void
        }

        /// Clears the panel to fully transparent (the stereo world shows
        /// through it).
        pub(crate) fn clear_panel(
            &self,
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            panel: &wgpu::TextureView,
        ) {
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vr panel clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: panel,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pw64_gfx::submit(queue, [encoder.finish()]);
        }

        /// Latency probe ([`probe_on`]): the window thread's bar, drawn
        /// over the finished frame in the panel.
        pub(crate) fn probe(
            &mut self,
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            panel: &wgpu::TextureView,
        ) {
            if !probe_on() {
                return;
            }
            let format = panel.texture().format();
            let bar = self
                .probe
                .get_or_insert_with(|| Bar::new(device, format, 1, false));
            // The panel fills the target: its half-extents are 1 in clip space.
            let mvp = math::translate_scale(
                [(probe_phase() - 0.5) * 2.0, PROBE_Y_WINDOW * 2.0, 0.0],
                [PROBE_SIZE[0] * 2.0, PROBE_SIZE[1] * 2.0, 1.0],
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("vr probe"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: panel,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                bar.draw(queue, &mut pass, 0, &mvp, [1.0, 1.0, 1.0, 1.0]);
            }
            pw64_gfx::submit(queue, [encoder.finish()]);
        }
    }

    /// Latency probe (`PW64_VR_PROBE=1`): two bars sweeping across the top
    /// of the panel every two seconds, both placed from the same clock.
    /// The white one is drawn by the window thread into the panel texture,
    /// like the game frame; the orange one, just above it, by the headset
    /// thread straight into the eye images. Seen in the headset, a white
    /// bar trailing the orange one is delay on the way through the panel;
    /// both together but late is delay after the headset thread (runtime,
    /// video stream).
    fn probe_on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("PW64_VR_PROBE").is_some_and(|v| v != "0"))
    }

    /// Where the probe bars are now: 0..1 across the panel, 2 s per sweep.
    fn probe_phase() -> f32 {
        static START: OnceLock<Instant> = OnceLock::new();
        (START.get_or_init(Instant::now).elapsed().as_secs_f64() / 2.0).fract() as f32
    }

    /// A solid-colour rectangle: the unit rectangle through `mvp`.
    struct Bar {
        pipeline: wgpu::RenderPipeline,
        slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    }

    const BAR_SHADER: &str = r"
struct U { mvp: mat4x4<f32>, color: vec4<f32> }
@group(0) @binding(0) var<uniform> u: U;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0));
    let uv = corners[i];
    return u.mvp * vec4<f32>(uv.x - 0.5, 0.5 - uv.y, 0.0, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return u.color;
}
";

    impl Bar {
        /// `slots`: how many rectangles with their own placement per frame;
        /// `blend`: drawn by their alpha (the fade) or opaque (the probe).
        fn new(
            device: &wgpu::Device,
            format: wgpu::TextureFormat,
            slots: usize,
            blend: bool,
        ) -> Self {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("vr probe"),
                source: wgpu::ShaderSource::Wgsl(BAR_SHADER.into()),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vr probe"),
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
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });
            let slots = (0..slots)
                .map(|_| {
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("vr probe"),
                        size: 80,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("vr probe"),
                        layout: &pipeline.get_bind_group_layout(0),
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        }],
                    });
                    (buffer, group)
                })
                .collect();
            Self { pipeline, slots }
        }

        fn draw(
            &self,
            queue: &wgpu::Queue,
            pass: &mut wgpu::RenderPass<'_>,
            slot: usize,
            mvp: &Mat4,
            color: [f32; 4],
        ) {
            let mut bytes = [0u8; 80];
            bytes[..64].copy_from_slice(&mat_bytes(mvp));
            for (i, c) in color.iter().enumerate() {
                bytes[64 + i * 4..68 + i * 4].copy_from_slice(&c.to_ne_bytes());
            }
            queue.write_buffer(&self.slots[slot].0, 0, &bytes);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.slots[slot].1, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    /// p8, the right stick: dead zone; the island model's turn and tilt
    /// rates and tilt limit; the chase camera's speed. Camera modes that
    /// are chase views (camera.c: 0 chase, 4 and 7 variants, 9 jumble
    /// hopper).
    const STICK_DEADZONE: f32 = 0.2;
    const SPIN_RAD_PER_S: f32 = 1.6;
    const TILT_RAD_PER_S: f32 = 0.8;
    const TILT_MAX: f32 = 0.6;
    const CHASE_M_PER_S: f32 = 4.0;
    const CHASE_MODES: [i32; 4] = [0, 4, 7, 9];
    /// The island model's zoom with L / R: rate (e-folds per second) and
    /// range.
    const ZOOM_RATE: f32 = 0.7;
    const ZOOM_RANGE: (f32, f32) = (0.5, 2.5);

    /// w8: how long the fade in from black takes after the headset's view
    /// changes kind (flight, island model, title, flat screens).
    const FADE_IN: Duration = Duration::from_millis(300);

    /// The probe bars' size on the panel, in panel widths / heights, and
    /// their centre heights (panel centre 0, top edge 0.5).
    const PROBE_SIZE: [f32; 2] = [0.02, 0.05];
    const PROBE_Y_WINDOW: f32 = 0.425;
    const PROBE_Y_HEADSET: f32 = 0.475;

    /// The rectangle shader: `SRGB_DECODE` is replaced by a decode when the
    /// eye target is sRGB but the panel holds display-encoded values in a
    /// plain format (the window's surface format); `ALPHA` by 1, or by the
    /// texture's alpha for the panel over the stereo world.
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

// Birdman64VR: the game's black screen background keyed out (a flat
// screen floats in the room instead of on a black rectangle); pure black
// is transparent, anything brighter than a few 1/1000 opaque.
fn key(c: vec3<f32>) -> f32 {
    return smoothstep(0.0, 0.004, max(c.r, max(c.g, c.b)));
}

@fragment
fn fs(v: V) -> @location(0) vec4<f32> {
    let p = textureSample(t, s, v.uv);
    let c = PICTURE;
    let col = SRGB_DECODE;
    return OUT;
}
";

    /// How the panel is drawn ([`shader`]).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum PanelBlend {
        /// The whole picture (and the eye pictures of the stereo world).
        Opaque,
        /// Over the stereo world, by its alpha (premultiplied, as the UI
        /// layers leave it).
        Over,
        /// A flat game screen over the room: its black background keyed
        /// out (premultiplied).
        Keyed,
        /// An eye picture with a transparent background (the island model
        /// in the player's room, p12) over nothing: premultiplied in the
        /// picture's encoding, so colours are decoded unmultiplied (else
        /// soft edges come out dark).
        Cutout,
    }

    pub(super) fn shader(decode: bool, blend: PanelBlend) -> String {
        SHADER
            .replace(
                "PICTURE",
                match blend {
                    PanelBlend::Cutout => "p.rgb / max(p.a, 0.001)",
                    _ => "p.rgb",
                },
            )
            .replace("SRGB_DECODE", if decode { "decode(c)" } else { "c" })
            .replace(
                "OUT",
                match blend {
                    PanelBlend::Opaque => "vec4<f32>(col, 1.0)",
                    PanelBlend::Over => "vec4<f32>(col, p.a)",
                    PanelBlend::Keyed => "vec4<f32>(col * key(c), key(c))",
                    PanelBlend::Cutout => "vec4<f32>(col * p.a, p.a)",
                },
            )
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
        // One bind group layout for both pipelines, so the panel's bind
        // groups work with either.
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vr panel"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vr panel"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        // `pipeline`: opaque (the panel as the whole picture, and the eye
        // pictures of the stereo world). `over`: the panel over the stereo
        // world, by its alpha (premultiplied, as the UI layers leave it).
        let make = |blend: PanelBlend| {
            let alpha = blend != PanelBlend::Opaque;
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("vr panel"),
                source: wgpu::ShaderSource::Wgsl(shader(xr.format().is_srgb(), blend).into()),
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vr panel"),
                layout: Some(&layout),
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
                    targets: &[Some(wgpu::ColorTargetState {
                        format: xr.format(),
                        blend: alpha.then_some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        // `keyed`: a flat game screen (no stereo world) with its black
        // background keyed out, so it floats in the room.
        // `cutout`: an eye picture with a transparent background, in the
        // player's room (p12).
        let (pipeline, over, keyed, cutout) = (
            make(PanelBlend::Opaque),
            make(PanelBlend::Over),
            make(PanelBlend::Keyed),
            make(PanelBlend::Cutout),
        );
        super::PASSTHROUGH_AVAILABLE.store(xr.passthrough_available(), Ordering::Relaxed);
        // q6: the display's refresh rates, for the settings; the wanted one
        // is asked for below (each frame, when it changes).
        *super::REFRESH_RATES
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = xr
            .refresh_rates()
            .iter()
            .map(|r| r.round() as u32)
            .collect();
        let mut asked_rate: Option<Option<u32>> = None;
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
        // The eye pictures fill the eye: the unit rectangle over all of clip
        // space.
        let full = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vr eye picture"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &full,
            0,
            &mat_bytes(&math::translate_scale([0.0; 3], [2.0, 2.0, 1.0])),
        );
        // For the current eye pictures: (generation, bind groups per set
        // and eye).
        let eye_bound = std::cell::RefCell::new(None::<(u64, [[wgpu::BindGroup; 2]; 3])>);
        // Stereo report: headset frames that showed a new eye picture and
        // ones that showed the previous one again.
        let (mut eyes_new, mut eyes_repeated, mut last_serial) = (0u32, 0u32, 0u64);
        let probe = probe_on().then(|| Bar::new(&device, xr.format(), 2, false));
        // w8: a fade in from black when what the headset shows changes kind
        // (`Source::scene`), instead of a hard cut; one slot per eye.
        let fade = Bar::new(&device, xr.format(), 2, true);
        let mut shown_scene = None::<u8>;
        let mut scene_since = Instant::now();
        // For the current panel textures: (generation, aspect, bind groups
        // per texture and eye).
        let mut bound: Option<(u64, f32, [[wgpu::BindGroup; 2]; 2])> = None;
        // Session state, to pause the game when the player is away.
        let mut was_focused = false;
        let mut touch_active = false;
        let mut last_outcome = None;
        let mut announced = false;
        // Hitch report: the longest gap between shown headset frames and
        // the longest waits for the queue lock, per five seconds.
        let (mut last_frame, mut last_report) = (None::<Instant>, Instant::now());
        let (mut worst_gap, mut worst_wait, mut shown) = (Duration::ZERO, Duration::ZERO, 0u32);
        let started = Instant::now();
        // Headset frames the runtime did not want drawn (headset off the head).
        let mut not_shown = 0u32;
        while !STOP.load(Ordering::Relaxed) {
            // The stereo world's eye pictures for this frame, if the game
            // is showing one: chosen when the frame starts (after the wait
            // for it, so the game frame drawn meanwhile is the one shown)
            // and marked as being read, so the window thread leaves that
            // set alone.
            let world = std::cell::Cell::new(None::<(usize, [EyePose; 2], u64)>);
            let blank = std::cell::Cell::new(false);
            let scene = std::cell::Cell::new(0u8);
            let choose = || {
                let mut s = source.lock().unwrap_or_else(|e| e.into_inner());
                let mut eye_bound = eye_bound.borrow_mut();
                s.reading = s.world.map(|w| w.0);
                if eye_bound.as_ref().map(|b| b.0) != Some(s.eye_generation) {
                    *eye_bound = s.eye_views.as_ref().map(|views| {
                        let groups = [0, 1, 2].map(|set| {
                            [0, 1].map(|eye| {
                                device.create_bind_group(&wgpu::BindGroupDescriptor {
                                    label: Some("vr eye picture"),
                                    layout: &bgl,
                                    entries: &[
                                        wgpu::BindGroupEntry {
                                            binding: 0,
                                            resource: full.as_entire_binding(),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::TextureView(
                                                &views[set][eye],
                                            ),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 2,
                                            resource: wgpu::BindingResource::Sampler(&sampler),
                                        },
                                    ],
                                })
                            })
                        });
                        (s.eye_generation, groups)
                    });
                }
                world.set(s.world.filter(|_| eye_bound.is_some()));
                blank.set(s.blank);
                scene.set(s.scene);
                Layers {
                    world: true,
                    panel: None,
                    // The pictures were drawn for these poses: this frame's
                    // prediction made one frame ago, or older ones (game
                    // late, paused, loading); the runtime reprojects.
                    eyes: world.get().map(|w| w.1),
                    // p12: the menus in the player's room.
                    passthrough: in_room(s.scene, super::passthrough()),
                }
            };
            if super::RECENTRE.swap(false, Ordering::Relaxed) {
                xr.recentre();
            }
            // q6: the player's refresh rate (the settings), or the
            // headset's own; the game's rate follows the frame period.
            let want = super::refresh_rate();
            if asked_rate != Some(want) {
                asked_rate = Some(want);
                let exact = want.and_then(|hz| {
                    xr.refresh_rates()
                        .iter()
                        .copied()
                        .find(|r| r.round() as u32 == hz)
                });
                // A saved rate this headset lacks: its own choice.
                xr.request_refresh_rate(exact.unwrap_or(0.0));
            }
            let outcome = xr.frame(choose, Some(&pw64_gfx::QUEUE_GATE), |frame| {
                let world = world.get();
                // w8: the view's kind changed (not at the very start): fade
                // in from black over FADE_IN.
                if shown_scene != Some(scene.get()) {
                    if shown_scene.is_some() {
                        scene_since = Instant::now();
                    }
                    shown_scene = Some(scene.get());
                }
                let fade_alpha =
                    1.0 - (scene_since.elapsed().as_secs_f32() / FADE_IN.as_secs_f32()).min(1.0);
                // p13: ...or blinking after a photo.
                let fade_alpha = SHUTTER_AT
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .map_or(fade_alpha, |at| fade_alpha.max(shutter_alpha(at.elapsed())));
                let eye_bound = eye_bound.borrow();
                // Stereo report: a new picture, or the last one again?
                if let Some((_, _, serial)) = world {
                    if serial == last_serial {
                        eyes_repeated += 1;
                    } else {
                        eyes_new += 1;
                    }
                    last_serial = serial;
                }
                *NEXT_EYES.lock().unwrap_or_else(|e| e.into_inner()) = frame.next_eyes;
                *SEAT.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame.seat);
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
                // Locked until the submit below (see `Source`).
                let s = source.lock().unwrap_or_else(|e| e.into_inner());
                if bound.as_ref().map(|b| b.0) != Some(s.generation) {
                    bound = s.views.as_ref().map(|views| {
                        let t = views[0].texture();
                        let groups = [0, 1].map(|texture| {
                            [0, 1].map(|eye| {
                                device.create_bind_group(&wgpu::BindGroupDescriptor {
                                    label: Some("vr panel"),
                                    layout: &bgl,
                                    entries: &[
                                        wgpu::BindGroupEntry {
                                            binding: 0,
                                            resource: uniforms[eye].as_entire_binding(),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::TextureView(
                                                &views[texture],
                                            ),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 2,
                                            resource: wgpu::BindingResource::Sampler(&sampler),
                                        },
                                    ],
                                })
                            })
                        });
                        let aspect = t.width() as f32 / t.height().max(1) as f32;
                        (s.generation, aspect, groups)
                    });
                }
                let front = s.front;
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
                                // The dim room, or nothing: the player's
                                // own room shows where the picture is
                                // clear (p12).
                                load: wgpu::LoadOp::Clear(if frame.passthrough {
                                    wgpu::Color::TRANSPARENT
                                } else {
                                    wgpu::Color {
                                        r: 0.0,
                                        g: 0.002,
                                        b: 0.006,
                                        a: 1.0,
                                    }
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    // The stereo world: this eye's picture, then the panel
                    // over it where the window drew something.
                    if let (Some((set, _, _)), Some((_, groups))) = (world, &*eye_bound) {
                        pass.set_pipeline(if frame.passthrough {
                            &cutout
                        } else {
                            &pipeline
                        });
                        pass.set_bind_group(0, &groups[set][i], &[]);
                        pass.draw(0..6, 0..1);
                    }
                    if let Some((_, aspect, groups)) = &bound {
                        // Blended over the world, or over nothing for a
                        // blank game frame (the panel is clear then).
                        let blended = world.is_some() || blank.get();
                        pass.set_pipeline(if blended { &over } else { &keyed });
                        pass.set_bind_group(0, &groups[front][i], &[]);
                        pass.draw(0..6, 0..1);
                        // Latency probe: this thread's bar, on the panel's
                        // plane just above the window thread's.
                        if let Some(bar) = &probe {
                            let (w, h) = (PANEL_HEIGHT_M * aspect, PANEL_HEIGHT_M);
                            let model = math::translate_scale(
                                [
                                    (probe_phase() - 0.5) * w,
                                    PROBE_Y_HEADSET * h,
                                    -PANEL_DISTANCE_M,
                                ],
                                [PROBE_SIZE[0] * w, PROBE_SIZE[1] * h, 1.0],
                            );
                            let mvp = math::mul(&eye.projection, &math::mul(&eye.view, &model));
                            bar.draw(&queue, &mut pass, i, &mvp, [1.0, 0.45, 0.1, 1.0]);
                        }
                    }
                    // w8: fading in from black after a change of view.
                    if fade_alpha > 0.0 {
                        let all = math::translate_scale([0.0; 3], [2.0, 2.0, 1.0]);
                        fade.draw(&queue, &mut pass, i, &all, [0.0, 0.0, 0.0, fade_alpha]);
                    }
                }
                // Directly: this thread holds the queue lock for the frame.
                queue.submit([encoder.finish()]);
                drop(s);
            });
            for line in xr.take_log() {
                log(&line, true);
            }
            // (A failure to start it ends passthrough: the dim room again.)
            super::PASSTHROUGH_AVAILABLE.store(xr.passthrough_available(), Ordering::Relaxed);
            // The Touch controllers: a pad for player 1 (input/xr_pad.rs).
            let pad = xr.pad();
            if pad.active != touch_active {
                touch_active = pad.active;
                log(
                    if touch_active {
                        "Touch controllers on"
                    } else {
                        "Touch controllers off"
                    },
                    true,
                );
            }
            crate::input::set_xr_pad(pad);
            let kind = outcome.as_ref().ok().copied();
            if kind.is_some() && kind != last_outcome {
                last_outcome = kind;
                log(&format!("frames: {kind:?}"), true);
            }
            // The player is away (headset off, system menu in front): pause
            // the game. On the Quest nothing else does: the activity keeps
            // running.
            let focused = xr.focused();
            if was_focused && !focused && super::headless() {
                super::PAUSE.store(true, Ordering::Relaxed);
                crate::window::request_pause();
            }
            was_focused = focused;
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
                    if super::headless() {
                        // No monitor to continue on.
                        crate::window::request_quit();
                    } else {
                        crate::window::notify_toast(format!(
                            "VR stopped ({e}). The game continues on the monitor."
                        ));
                    }
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
                let gpu = pw64_gfx::stats::take();
                log(
                    &format!(
                        "{}t {:.0} s: headset {:.0} fps (frame {period_ms:.1} ms, longest gap \
                         {:.1}, waited {:.1} for the queue), game {game_fps:.0} fps; game \
                         frame: hand-over to panel avg {to_panel_avg:.1} max \
                         {to_panel_max:.1} ms, age when the headset draws it avg {age_avg:.1} \
                         max {age_max:.1} ms, then shown {ahead_avg:.1} (max {ahead_max:.1}) ms \
                         later; headset frames not shown {idle}; stereo: {} new eye pictures, \
                         {} shown again, lead {:.1} ms; longest, ms: {}; compiled {} \
                         pipelines ({:.0} ms, longest {:.0}; {} new shaders, {:.0} ms), {} \
                         new textures ({:.0} ms); eye draws avg {:.0} max {} ({:.0} more joined into \
                         them); {} stereo draws, \
                         CPU avg {:.1} ms preparing, {:.1} ms recording, {:.1} ms submitting; \
                         all submits waited {:.0} ms for the queue (longest {:.1}); not joined: {}",
                        if hitch { "hitch, " } else { "" },
                        started.elapsed().as_secs_f64(),
                        shown as f64 / secs,
                        ms(worst_gap),
                        ms(worst_wait),
                        std::mem::take(&mut eyes_new),
                        std::mem::take(&mut eyes_repeated),
                        lead().as_secs_f64() * 1e3,
                        detail.join(", "),
                        gpu.pipelines,
                        gpu.pipeline_ms,
                        gpu.pipeline_max_ms,
                        gpu.shaders,
                        gpu.shader_ms,
                        gpu.textures,
                        gpu.texture_ms,
                        gpu.eye_draws_avg,
                        gpu.eye_draws_max,
                        gpu.merged_avg,
                        gpu.eye_calls,
                        gpu.eye_prep_ms,
                        gpu.eye_encode_ms,
                        gpu.eye_submit_ms,
                        gpu.gate_ms,
                        gpu.gate_max_ms,
                        gpu.misses
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
        /// The eye matrices: the world straight ahead lands in the middle
        /// of a symmetric eye with reversed depth; the N64 screen lands on
        /// the panel, its top-left corner up and to the left.
        /// The dashboard slots name the throttle and the map by their
        /// `hud_edit::ELEMENTS` ids.
        #[test]
        fn dash_slots_are_throttle_and_map() {
            let ids: Vec<u8> = super::DASH_SLOTS.iter().map(|s| s.elem).collect();
            assert_eq!(
                ids,
                [
                    crate::hud_edit::element_id("throttle").unwrap(),
                    crate::hud_edit::element_id("radar").unwrap()
                ]
            );
        }

        #[test]
        fn stereo_eye_matrices() {
            let fov = std::f32::consts::FRAC_PI_4;
            let eye = super::stereo_eye(&pw64_xr::EyePose {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: [0.0; 3],
                fov: [-fov, fov, fov, -fov],
            });
            let apply = |m: &super::Mat4, p: [f32; 4]| -> [f32; 4] {
                [0, 1, 2, 3].map(|j| (0..4).map(|k| p[k] * m[k][j]).sum())
            };
            let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
            // 10 m ahead, 1 m to the right and up.
            let c = apply(&eye.world, [1.0, 1.0, -10.0, 1.0]);
            assert!(near(c[3], 10.0) && near(c[0] / c[3], 0.1) && near(c[1] / c[3], 0.1));
            assert!(near(c[2] / c[3], super::NEAR_M / 10.0), "reversed depth");
            // The middle of the N64 screen: the panel's centre.
            let c = apply(&eye.hud, [160.0, 120.0, 0.5, 1.0]);
            assert!(near(c[0], 0.0) && near(c[1], 0.0) && near(c[3], super::PANEL_DISTANCE_M));
            // Its top-left corner.
            let c = apply(&eye.hud, [0.0, 0.0, 0.5, 1.0]);
            let (x, y) = (c[0] / c[3], c[1] / c[3]);
            assert!(near(
                y,
                0.5 * super::PANEL_HEIGHT_M / super::PANEL_DISTANCE_M
            ));
            assert!(near(x, -y * 4.0 / 3.0));
            // A nearer HUD draw (larger N64 depth) wins the depth test.
            let far = apply(&eye.hud, [160.0, 120.0, 0.1, 1.0]);
            let nearer = apply(&eye.hud, [160.0, 120.0, 0.9, 1.0]);
            assert!(nearer[2] / nearer[3] > far[2] / far[3]);
        }

        /// The island model: what the game camera sees lands on the model
        /// in front of the player, the island's middle at the model's
        /// centre, north away from the player, east to the right, up up.
        #[test]
        fn diorama_places_the_island_in_front() {
            let fov = std::f32::consts::FRAC_PI_4;
            let pose = pw64_xr::EyePose {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: [0.0; 3],
                fov: [-fov, fov, fov, -fov],
            };
            // Island 0..2000 m both ways, up to 300 m; the camera at
            // (100, 200, 50) looking east (+x), level.
            let d = pw64_gfx::Diorama {
                camera: [
                    [0.0, -1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [100.0, 200.0, 50.0],
                ],
                bounds: [0.0, 0.0, -10.0, 2000.0, 2000.0, 300.0],
                title: false,
                celebration: false,
                state: 0,
                title_phase: -1,
                vehicle: -1,
            };
            let eye = super::diorama_eye(&pose, &d, [0.0, 0.0, 1.0]);
            let apply = |m: &super::Mat4, p: [f32; 4]| -> [f32; 4] {
                [0, 1, 2, 3].map(|j| (0..4).map(|k| p[k] * m[k][j]).sum())
            };
            let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
            // World point → camera space (x right = south, y up, -z east).
            let cam = |w: [f32; 3]| {
                let r = [w[0] - 100.0, w[1] - 200.0, w[2] - 50.0];
                [-r[1], r[2], -r[0], 1.0]
            };
            // Through the clip matrix: relative to the middle at sea level.
            let clip = eye.clip.unwrap();
            let m = apply(&clip.model, cam([1000.0, 1000.0, 0.0]));
            assert!(
                near(m[0], 0.0) && near(m[1], 0.0) && near(m[2], 0.0),
                "{m:?}"
            );
            assert!(near(clip.radius, 1000.0) && clip.top > 300.0);
            // On screen: the middle is straight below ahead (at the model
            // centre), north of it farther away, east of it to the right.
            let ndc = |w: [f32; 3]| {
                let c = apply(&eye.world, cam(w));
                [c[0] / c[3], c[1] / c[3], c[3]]
            };
            let c = super::MODEL_CENTRE_M;
            let mid = ndc([1000.0, 1000.0, 0.0]);
            assert!(near(mid[0], 0.0) && near(mid[1], c[1] / -c[2]) && near(mid[2], -c[2]));
            assert!(ndc([1000.0, 2000.0, 0.0])[2] > mid[2], "north is away");
            assert!(ndc([2000.0, 1000.0, 0.0])[0] > 0.0, "east is right");
            assert!(ndc([1000.0, 1000.0, 300.0])[1] > mid[1], "up is up");
        }

        /// The title view: the flight view from a level point a little
        /// behind the game camera, offset vertically by TITLE_UP_M.
        #[test]
        fn title_view_is_behind_the_camera() {
            let fov = std::f32::consts::FRAC_PI_4;
            let pose = pw64_xr::EyePose {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: [0.0; 3],
                fov: [-fov, fov, fov, -fov],
            };
            let d = pw64_gfx::Diorama {
                camera: [
                    [0.0, -1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [100.0, 200.0, 50.0],
                ],
                bounds: [0.0, 0.0, 0.0, 2000.0, 2000.0, 300.0],
                title: true,
                celebration: false,
                state: 0,
                title_phase: 1,
                vehicle: -1,
            };
            let eye = super::diorama_eye(&pose, &d, [0.0, 0.0, 1.0]);
            let flight = super::stereo_eye(&pose).world;
            let apply = |m: &super::Mat4, p: [f32; 4]| -> [f32; 4] {
                [0, 1, 2, 3].map(|j| (0..4).map(|k| p[k] * m[k][j]).sum())
            };
            // 10 m ahead of the game camera is TITLE_BACK_M further ahead
            // and TITLE_UP_M lower for the player.
            let a = apply(&eye.world, [0.0, 0.0, -10.0, 1.0]);
            let b = apply(
                &flight,
                [0.0, -super::TITLE_UP_M, -10.0 - super::TITLE_BACK_M, 1.0],
            );
            assert!((0..4).all(|i| (a[i] - b[i]).abs() < 1e-3), "{a:?} {b:?}");
            assert!(eye.clip.is_none() && eye.background.is_none());
        }

        /// A badge celebration: the flight view from the game camera's own
        /// (level) spot, no offset, flat HUD panel.
        #[test]
        fn celebration_view_is_the_cameras_own() {
            let fov = std::f32::consts::FRAC_PI_4;
            let pose = pw64_xr::EyePose {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: [0.0; 3],
                fov: [-fov, fov, fov, -fov],
            };
            let d = pw64_gfx::Diorama {
                camera: [
                    [0.0, -1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [100.0, 200.0, 50.0],
                ],
                bounds: [0.0, 0.0, 0.0, 2000.0, 2000.0, 300.0],
                title: false,
                celebration: true,
                state: 14,
                title_phase: -1,
                vehicle: 0,
            };
            let eye = super::diorama_eye(&pose, &d, [0.0, 0.0, 1.0]);
            let flight = super::stereo_eye(&pose).world;
            let near = |a: &super::Mat4, b: &super::Mat4| {
                (0..4).all(|i| (0..4).all(|j| (a[i][j] - b[i][j]).abs() < 1e-4))
            };
            assert!(near(&eye.world, &flight));
            assert!(eye.clip.is_none() && eye.hud_sphere.is_none());
        }

        /// Horizon lock: the correction turns the world's up onto the
        /// view's up: bank removes only the sideways tilt (pitch stays),
        /// full removes both; off or already level changes nothing; upside
        /// down falls back to bank.
        #[test]
        fn horizon_correction_levels_the_world() {
            use super::super::HorizonLock;
            let apply = |m: &super::Mat4, v: [f32; 3]| -> [f32; 3] {
                std::array::from_fn(|i| v[0] * m[0][i] + v[1] * m[1][i] + v[2] * m[2][i])
            };
            let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4);
            let (s30, c30) = (0.5f32, 0.75f32.sqrt());
            // Banked 30 degrees, level pitch.
            let up = [s30, c30, 0.0];
            let r = super::horizon_correction(up, HorizonLock::Bank).unwrap();
            assert!(close(apply(&r, up), [0.0, 1.0, 0.0]));
            assert!(
                close(apply(&r, [0.0, 0.0, -1.0]), [0.0, 0.0, -1.0]),
                "forward kept"
            );
            // Banked and pitched: bank keeps the pitch, full removes it.
            let up = [s30 * c30, c30 * c30, s30];
            let r = super::horizon_correction(up, HorizonLock::Bank).unwrap();
            let b = apply(&r, up);
            assert!(b[0].abs() < 1e-4 && (b[2] - s30).abs() < 1e-4, "{b:?}");
            let r = super::horizon_correction(up, HorizonLock::Full).unwrap();
            assert!(close(apply(&r, up), [0.0, 1.0, 0.0]));
            // A rotation: lengths kept.
            let v = apply(&r, [1.0, 0.0, 0.0]);
            assert!((v[0] * v[0] + v[1] * v[1] + v[2] * v[2] - 1.0).abs() < 1e-4);
            assert!(super::horizon_correction(up, HorizonLock::Off).is_none());
            assert!(super::horizon_correction([0.0, 1.0, 0.0], HorizonLock::Full).is_none());
            let r = super::horizon_correction([0.1, -1.0, 0.0], HorizonLock::Full).unwrap();
            assert!(apply(&r, [0.1, -1.0, 0.0])[1] > 0.99);
        }

        /// Diorama mode: following, the vehicle is held at its point in
        /// the seated space whatever the game camera does; static, the
        /// island's middle is where the menus' model has it. The model is
        /// the menus' size at zoom 1; no sky or sea either way.
        #[test]
        fn the_table_model_follows_the_vehicle_or_stands_still() {
            use super::{MODEL_CENTRE_M, Mat4, NEAR_M, TABLE_VEHICLE_M, math};
            use pw64_gfx::matrix::mul;
            let pose = pw64_xr::EyePose {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: [0.0; 3],
                fov: [-0.8, 0.8, 0.8, -0.8],
            };
            let t = pw64_gfx::Table {
                // Looking along +y (north), level, from somewhere else.
                camera: [
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [300.0, -200.0, 80.0],
                ],
                vehicle: [310.0, -150.0, 60.0],
                bounds: [-1000.0, -1000.0, 0.0, 1000.0, 1000.0, 300.0],
            };
            let apply = |m: &Mat4, p: [f32; 3]| -> [f32; 4] {
                std::array::from_fn(|k| p[0] * m[0][k] + p[1] * m[1][k] + p[2] * m[2][k] + m[3][k])
            };
            let ndc = |c: [f32; 4]| [c[0] / c[3], c[1] / c[3]];
            let same =
                |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
            let [l, r, u, dn] = pose.fov;
            let view = mul(
                &math::view(pose.orientation, pose.position),
                &math::projection_reversed(l, r, u, dn, NEAR_M * 0.5),
            );
            // In camera space (x right, y up, -z forward): the vehicle,
            // and the island's middle at sea level (world 0, 0, 0).
            let vehicle = [10.0, -20.0, -50.0];
            let middle = [-300.0, -80.0, -200.0];
            // The menus' scale: a 2000 m island is 2.6125 m.
            let s = 2.6125 / 2000.0;
            for zoom in [1.0f32 / 1.5, 1.0, 7.5] {
                let c = TABLE_VEHICLE_M;
                let eye = super::table_eye(&pose, &t, zoom, true, true);
                assert!(same(ndc(apply(&eye.world, vehicle)), ndc(apply(&view, c))));
                // 100 m east of the vehicle.
                let east = [c[0] + 100.0 * s * zoom, c[1], c[2]];
                assert!(same(
                    ndc(apply(&eye.world, [110.0, -20.0, -50.0])),
                    ndc(apply(&view, east))
                ));
                assert!(eye.hide_env && eye.clip.is_none());
                assert_eq!(eye.background, Some([0.0; 4]));

                let eye = super::table_eye(&pose, &t, zoom, false, false);
                assert!(same(
                    ndc(apply(&eye.world, middle)),
                    ndc(apply(&view, MODEL_CENTRE_M))
                ));
                assert!(eye.hide_env && eye.clip.is_none());
                assert_eq!(eye.background.map(|b| b[3]), Some(1.0));
            }
        }

        /// p13: the blink closes fully, then opens again and stays open.
        #[test]
        fn the_shutter_blinks_once() {
            use std::time::Duration;
            let a = |ms| super::shutter_alpha(Duration::from_millis(ms));
            assert_eq!(a(0), 0.0);
            assert!(a(35) > 0.4 && a(35) < 0.6);
            assert!((a(70) - 1.0).abs() < 1e-4);
            assert!(a(135) > 0.4 && a(135) < 0.6);
            assert_eq!(a(200), 0.0);
            assert_eq!(a(5000), 0.0);
        }

        /// p12: the room shows behind the flat screens and the island
        /// model, never in flight, the title or a celebration; and a
        /// cutout picture is decoded unmultiplied.
        #[test]
        fn the_room_shows_behind_menus_only() {
            assert!(super::in_room(0, true) && super::in_room(2, true));
            assert!(!super::in_room(0, false) && !super::in_room(2, false));
            for menus in [false, true] {
                assert!(!super::in_room(1, menus) && !super::in_room(3, menus));
                assert!(!super::in_room(4, menus));
                // A flight shown as a model: in the room, or the dim one.
                assert!(super::in_room(5, menus) && !super::in_room(6, menus));
            }
            let cut = super::shader(true, super::PanelBlend::Cutout);
            assert!(cut.contains("p.rgb / max(p.a, 0.001)") && cut.contains("col * p.a, p.a"));
            assert!(!super::shader(true, super::PanelBlend::Opaque).contains("max(p.a"));
            assert_eq!(super::MODEL_BACKGROUND_ROOM[3], 0.0);
        }

        /// Every shader parses and validates (no GPU needed).
        #[test]
        fn shaders_are_valid_wgsl() {
            use wgpu::naga;
            let sources = [
                super::shader(false, super::PanelBlend::Opaque),
                super::shader(true, super::PanelBlend::Opaque),
                super::shader(true, super::PanelBlend::Over),
                super::shader(true, super::PanelBlend::Keyed),
                super::shader(true, super::PanelBlend::Cutout),
                super::shader(false, super::PanelBlend::Cutout),
                super::BAR_SHADER.to_string(),
            ];
            for source in sources {
                let m = naga::front::wgsl::parse_str(&source).unwrap();
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
