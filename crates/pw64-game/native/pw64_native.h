/*
 * Force-included (-FI) before every decomp C file in the native build.
 * Keep this tiny: it runs ahead of <ultra64.h>, so it may only set up
 * host-ABI types and feature macros. See docs/notes/native-build.md.
 */
#ifndef PW64_NATIVE_H
#define PW64_NATIVE_H

/* Host size_t (64-bit; `unsigned long long` on Windows, `unsigned long` on
 * Linux). ultratypes.h and the IDO libc headers would otherwise typedef a
 * 32-bit one, which breaks calls into the host CRT (memcpy, ...). */
#ifndef _SIZE_T_DEF
#define _SIZE_T_DEF
#define _SIZE_T
typedef __SIZE_TYPE__ size_t;
#endif

/* The decomp's `long` is 32-bit (IDO; also MSVC/LLP64), but 64-bit on LP64
 * (Linux). The patched headers spell every ABI-relevant `long` (u32/s32 in
 * ultratypes.h, Mtx_t and friends in gbi.h) as PW64_LONG32: `long` where
 * that is 32-bit (Windows keeps its exact types), else `int`. */
#if __SIZEOF_LONG__ == 4
#define PW64_LONG32 long
#else
#define PW64_LONG32 int
#endif

#define PW64_NATIVE 1

/* Pointer-width helpers for the pointer patch set (native-build.md §2,
 * option-a.md). The C keeps addresses in 32-bit slots ("vaddr": display-list
 * words, allocator, filesystem, 0x80xxxxxx literals); every conversion goes
 * through these, which map registered host blocks to their vaddr range
 * (pw64-platform vaddr.rs). Values in no region (ROM offsets, physical
 * addresses, small ints in OSMesg) pass through; with no regions registered
 * (fixed low-4 GB window) both are the identity.
 *   PW64_U32(p): pointer (or 32-bit int) -> 32-bit vaddr.
 *   PW64_PTR(a): 32-bit vaddr (s32 or u32) -> host pointer; also for literal
 *                RDRAM addresses: (u8*)PW64_PTR(0x800DA800).
 * Inline fast path: one region (`pw64_vmap`, Rust vaddr.rs `VMap`; the
 * dylib shim defines the module's copy) is tried first; a miss calls Rust.
 * No regions: the map covers all 2^32 values at host 0 (identity, always
 * hits, so the fixed-window build never calls out). Not usable in static
 * initializers. */
unsigned int pw64_to_vaddr(const void* p);
void* pw64_to_host(unsigned int a);
struct pw64_vmap {
    __UINTPTR_TYPE__ host;
    unsigned long long size;
    unsigned int vbase;
    unsigned int pad;
};
extern struct pw64_vmap pw64_vmap;
static __inline__ unsigned int pw64_u32_fast(__UINTPTR_TYPE__ p) {
    __UINTPTR_TYPE__ off = p - pw64_vmap.host;
    if (__builtin_expect(off < pw64_vmap.size, 1)) {
        return pw64_vmap.vbase + (unsigned int)off;
    }
    return pw64_to_vaddr((const void*)p);
}
static __inline__ void* pw64_ptr_fast(unsigned int a) {
    unsigned int off = a - pw64_vmap.vbase;
    if (__builtin_expect(off < pw64_vmap.size, 1)) {
        return (void*)(pw64_vmap.host + off);
    }
    return pw64_to_host(a);
}
#define PW64_U32(p) pw64_u32_fast((__UINTPTR_TYPE__)(p))
#define PW64_PTR(a) pw64_ptr_fast((unsigned int)(a))

/* Endianness (native-build.md §3, task 6). ROM data is big-endian; scalars
 * read through uvMemRead/uvConsumeBytes(1/2/4) are already converted. Structs
 * copied raw get swapped in place right after the copy:
 *   PW64_SWAP(p, count, T, layout): `count` elements of sizeof(T) at p, each
 *   described by `layout` (`[N]c`, c = b keep / h 2-byte / w 4-byte /
 *   d 8-byte swap). pw64_swap (Rust, pw64-platform swap.rs) panics if the
 *   layout doesn't add up to sizeof(T). */
void pw64_swap(void* p, unsigned int count, unsigned int stride, const char* layout);
#define PW64_SWAP(p, count, T, layout) \
    pw64_swap((p), (unsigned int)(count), (unsigned int)sizeof(T), (layout))
/* Panics (Rust) with a message; for native helpers and patched checks. */
_Noreturn void pw64_fatal(const char* msg, unsigned int value);
/* One-line trace from patched C code (only prints with PW64_TRACE_OS).
 * `value` carries the packed state: fields OR'd / shifted into 32 bits. */
void pw64_log(const char* msg, unsigned int value);
#define PW64_LOGF(msg, v) pw64_log(msg, (unsigned int)(v))
/* PW64_START: boot straight into one test (pw64-platform start.rs); 1 once. */
int pw64_direct_start(int* file, int* cls, int* veh, int* test, int* pilot);
unsigned int pw64_rsp(void);
#define PW64_SWAP16(p, count) pw64_swap((p), (unsigned int)(count), 2, "h")
#define PW64_SWAP32(p, count) pw64_swap((p), (unsigned int)(count), 4, "w")
/* Hor+ widescreen (native/src/pw64_widescreen.c): output aspect (0 = 4:3)
 * culling-frustum x scale and display-list marker for a channel
 * (UnkStruct_80204D94*). */
extern float pw64_widescreen_aspect;
float pw64_widescreen_xscale(const void* chan);
void pw64_widescreen_tag(const void* chan, int on);
/* Birdman64VR (same file; VR = pw64_widescreen_aspect < 0): 1 while the
 * world is shown in stereo, i.e. in VR during a test's flight. */
int pw64_vr_flight(void);
/* Birdman64VR, same file: the HUD sphere's radians per N64 pixel (set by
 * Rust from the VR HUD size; 0 = not known). */
extern float pw64_vr_hud_rad_per_px;
/* Birdman64VR, same file: the gyrocopter's aim reticle row offset
 * (code_7CF30.c.patch func_802F6DC8). In VR it is placed on the HUD sphere
 * `down` radians below the view's centre, the direction the missiles fly;
 * otherwise `flat`, the game's value. */
float pw64_vr_aim_reticle_y(float flat, float down);
/* Birdman64VR, same file: 1 = the gyrocopter's windscreen shows in VR
 * first person (Rust: VR HUD page, `[vr] gyro_windscreen`). Always shown
 * from outside. */
extern int pw64_vr_gyro_windscreen;
/* The windscreen: part 9 of all six pilots' gyrocopter models (the only
 * translucent part; pw64-extract --models, models 0x127-0x133). */
#define PW64_GYRO_WINDSCREEN_PART 9
/* Birdman64VR: the gyrocopter's first-person eye this much further back
 * than the game's (code_7CF30.c.patch gyrocopterEnterLeave), metres: the
 * game's was too close to the windscreen, 0.25 m back was a little far, so
 * 0.1 m forward again (headset). Centred across the cockpit too (the
 * game's is up to 1.3 cm to the right). */
#define PW64_VR_GYRO_EYE_BACK_M 0.15f
/* Birdman64VR: the gyrocopter's fuel gauge in VR (hud.c.patch
 * hudDrawGyrocopter; the flat game's is at 98, 37), N64 HUD px, y up: on
 * the HUD sphere at the default spread (275%) its centre sits midway
 * between the altimeter's lower end and the altitude (sea level) readout
 * below it, horizontally between their centres. */
#define PW64_VR_GYRO_FUEL_X 207
#define PW64_VR_GYRO_FUEL_Y 53
/* Birdman64VR, same file: the cockpit dashboard's points of the vehicle in
 * first person, one per dashboard gauge (2: throttle, map; model units,
 * model frame), sent to the renderer each frame (VR HUD > Throttle and
 * map); objId < 0 or points NULL: none. */
void pw64_vr_dash_set(int objId, const float (*points)[3]);
/* The gyrocopter's dashboard on (`on`) or off for dobj `objId` with model
 * `modelId` (code_7CF30.c.patch, every frame). */
void pw64_vr_gyro_dash(int objId, int modelId, int on);
/* Birdman64VR, same file: outside a test's flight, a world view with
 * terrain is shown as a small model of the island (the "diorama": menus,
 * title fly-over, results, replays). 1 for such a channel
 * (UnkStruct_80204D94*). */
int pw64_vr_diorama(const void* chan);
/* Birdman64VR, same file: 1 between the world tags of a world view shown
 * in stereo (flight or diorama). */
extern int pw64_vr_stereo_view;
/* Birdman64VR, same file: the title screen's phase (its unk28, 1..8;
 * title_screen.c.patch), sent to the renderer for the log. */
extern int pw64_vr_title_phase;
/* Birdman64VR, same file: the results menu has appeared (results.c.patch
 * resultInit); until then the view after landing stays full size. */
extern int pw64_vr_results_menu;
/* Birdman64VR, same file: the crash camera in VR (camera.c.patch): on
 * its first frame it moves 10 m further from the pilot (`pilot`, Mtx4F*)
 * along its line to them; then it stays there and turns camera matrix
 * `cur` towards `target`'s direction slowly and level. `now`: the
 * camera's running time (a gap = a new crash). */
void pw64_vr_crash_camera(void* cur, const void* target, const void* pilot, float dt, float now);
/* Birdman64VR, same file: `out` (Mtx4F*) = a level frame at `at`'s
 * position facing heading (vx, vy) (`at`'s own forward when that is 0). */
void pw64_vr_heading_frame(void* out, const void* at, float vx, float vy);
/* Birdman64VR, same file: the cannonball's cinematic view in VR
 * (cannonball.c.patch): a fixed spot beside the line from the cannon
 * (Mtx4F* `cannon`) to the target (pw64_cannon_target), a little short of
 * the target, showing the target and the pilot (`pilot`) coming in. Side
 * offset = PW64_VR_CINE_SIDE x the cannon-target distance (clamped), short
 * of the target by PW64_VR_CINE_BACK x it. A gap in `now` = a new shot
 * (or the view switched back on). */
#define PW64_VR_CINE_SIDE 0.25f
#define PW64_VR_CINE_BACK 0.2f
#define PW64_VR_CINE_SIDE_MIN_M 20.0f
#define PW64_VR_CINE_SIDE_MAX_M 70.0f
#define PW64_VR_CINE_UP_M 5.0f
/* pads.c.patch: the cannon target's bullseye (world xyz) in `out`; 0 when
 * the level has none. */
int pw64_cannon_target(float* out);
/* Birdman64VR, same file: the pilot's drawing scale in the cinematic
 * view: their distance from the camera / PW64_VR_CINE_SCALE_REF_M (at
 * least 1, at most PW64_VR_CINE_SCALE_MAX), back to 1 over the last 120 m
 * to 30 m before the target. `pilot`, `cam`: Mtx4F*. */
#define PW64_VR_CINE_SCALE_REF_M 20.0f
#define PW64_VR_CINE_SCALE_MAX 4.0f
float pw64_vr_cannon_scale(const void* pilot, const void* cam);
/* Birdman64VR, same file: the cannonball's smoke trail in VR: while `on`
 * (in flight), a dark grey puff left at the pilot (Mtx4F*) every
 * PW64_VR_TRAIL_EVERY_S, PW64_VR_TRAIL_SIZE_M across and opaque from the
 * start, fading out over the last 30% of PW64_VR_TRAIL_LIFE_S. At most
 * PW64_VR_TRAIL_N puffs (of the 120 effects, fx.c), so EVERY x N bounds
 * LIFE; several a frame are spread along the way. Call every frame. */
#define PW64_VR_TRAIL_N 96
/* Drawn up to this far from the camera (fx.c prop 19; smoke.c uses 50 x
 * the puff size, 200 m here, short of the cinematic view). */
#define PW64_VR_TRAIL_DRAW_M 2000.0f
#define PW64_VR_TRAIL_GREY 40
#define PW64_VR_TRAIL_SIZE_M 4.0f
#define PW64_VR_TRAIL_EVERY_S 0.05f
#define PW64_VR_TRAIL_LIFE_S 4.5f
void pw64_vr_cannon_trail(int on, const void* pilot, float dt);
void pw64_vr_cannon_cinematic(void* cur, const void* cannon, const void* pilot, float dt, float now);
/* Fill screen (renderer.md "Fill screen"), same file: 0 = off (set by Rust
 * before boot), the per-frame "a filled world view was drawn" flag (reset
 * at the top of uvGfxBegin, graphics.c.patch) and the vertical
 * culling-frustum scale for a fill view (1 otherwise). */
extern int pw64_fill_screen;
extern int pw64_fill_frame;
float pw64_fill_yscale(const void* chan);
/* Flight-HUD anchors ('L'/'C'/'R' horizontal; 'T'/'M'/'B' vertical, fill
 * screen), same file. Text keeps both anchors of its print per message
 * (font.c.patch) and re-emits them at uvFontGenDlist. */
extern int pw64_hud_anchor;
void pw64_hud_emit_anchor(int anchor);
void pw64_hud_set_anchor(int anchor);
extern int pw64_hud_vanchor;
void pw64_hud_emit_vanchor(int anchor);
void pw64_hud_set_anchor2(int anchor, int vanchor);
void pw64_hud_anchor_xy(int x, int y);
/* Flight-HUD on/off ("PWH", same file, pw64-gfx interp::HUD_TAG_ON/OFF):
 * hud.c.patch brackets the HUD branch of hudMainRender with it; the
 * renderer only uses the tag when OLED care is on. */
void pw64_hud_tag(int on);
/* Movable HUD elements ("PWE", same file, pw64-gfx interp::HUD_ELEM_TAG):
 * the element of the following draws / text printed now (0 = none; ids
 * must match the names in birdman64 hud_edit.rs). Text keeps the element
 * of its print (font.c.patch). */
#define PW64_HUD_SPEED 1
#define PW64_HUD_TIMER 2
#define PW64_HUD_THROTTLE 3
#define PW64_HUD_FUEL 4
#define PW64_HUD_ALTIMETER 5
#define PW64_HUD_RADAR 6
#define PW64_HUD_SEA_LEVEL 7
#define PW64_HUD_PHOTOS 8
#define PW64_HUD_MESSAGES 9
#define PW64_HUD_CB_POWER 10
#define PW64_HUD_CB_HEADING 11
#define PW64_HUD_CB_ELEVATION 12
extern int pw64_hud_elem;
void pw64_hud_element(int id);
void pw64_hud_emit_element(int id);
/* Split-screen HUD view ("PWS", same file, pw64-gfx interp::SPLIT_TAG):
 * hudMainRender brackets the HUD with the current player's view. */
void pw64_hud_split_tag(int on);
/* uvCopyFrameBuf (graphics.c.patch): copy the HLE renderer's framebuffer
 * target `src` into `dst` (N64 fb addresses; pw64-platform headless.rs). */
void pw64_fb_copy(unsigned int dst, unsigned int src);
/* snowDraw (snow.c.patch): the `count` RDRAM pixel indexes (y*320+x) the
 * CPU wrote with `color` into the previous, finished framebuffer `fb`
 * (N64 fb address; pw64-platform headless.rs). `u32` is
 * `unsigned PW64_LONG32` (ultratypes.h), so `idx` matches it exactly. */
void pw64_fb_pixels(unsigned int fb, const unsigned PW64_LONG32* idx, int count, unsigned int color);
/* Present tick (sched.c.patch; pw64-platform os/vi.rs, framerate.md):
 * register the scheduler's present message (`OSMesgQueue*`, `OSMesg`), and
 * whether present ticks (not retraces) latch swaps and start gfx. */
void pw64_present_set_event(void* mq, void* msg);
int pw64_present_active(void);
/* Split-screen (native/src/pw64_split.c): 0 = off, 1 = on (set by Rust
 * before boot, pw64_game::set_split). pw64_split_layout: 0 top/bottom, 1
 * left/right (Rust, any time); pw64_split_lr: the layout latched at test
 * start. The helpers live in camera.c.patch; they take the game's Camera
 * as void* because this header is force-included before camera.h exists.
 * pw64_split_out_aspect (pw64_widescreen.c): the output aspect a
 * left/right view extends into (0 = none: plain 4:3 halves). */
extern int pw64_split;
/* Live switch (splitscreen.md "Live switch"): pw64_split_apply(on) sets
 * pw64_split/pw64_players (+ player 2's one-time setup and kernel pad 2) —
 * only where no test is loaded; pw64_split_poll applies a pending Rust
 * request (pw64_split_req, settings row). pw64_cont_enable: controller.c.patch. */
extern int pw64_split_req;
extern int pw64_split_hint;
void pw64_split_apply(int on);
void pw64_split_poll(void);
void pw64_cont_enable(int idx, int on);
extern int pw64_split_layout;
extern int pw64_split_lr;
void pw64_split_viewport(void* camera, int i);
float pw64_split_out_aspect(void);
/* Gyro aim reticle (code_7CF30.c.patch func_802F6DC8): the HUD px per unit
 * of projected NDC y for `camera`'s view; 107 (the N64 constant) unless the
 * camera has a split viewport. */
float pw64_split_reticle_half(void* camera);
/* Pilot collisions (native/src/pw64_collide.c, splitscreen.md "Interactions
 * toggle"): 1 = the two pilots can crash into each other. Read at each test
 * start; Rust may set it any time (pw64_game::set_split_collide, also live
 * from the settings screen). */
extern int pw64_split_collide;
/* Share best score (native/src/pw64_split.c): 1 = the better run of the
 * players is saved to player 1's file. Rust, any time. */
extern int pw64_split_share;
/* Split-screen players (native/src/pw64_players.c, splitscreen.md "M2"):
 * per-player state lives in the game's ordinary globals; only the current
 * player's copy is live. Registered regions are swapped on a switch
 * (live -> slot[cur], slot[i] -> live). Registration is explicit (no
 * constructors: the game module also builds as a dylib with our CRT
 * replacement) and runs once, on the first switch to 2 players
 * (pw64_split_apply: gameInit or the live switch). */
#define PW64_MAX_PLAYERS 2
extern int pw64_players;    /* 1; 2 with split-screen (gameInit: pw64_split ? 2 : 1) */
extern int pw64_player_cur; /* whose state is live in the globals */
/* Players in the current test (game.c.patch test setup): pw64_players for
 * a split-capable test, else 1. Gates every split path (1 = original). */
extern int pw64_level_players;
void pw64_player_register(void* addr, unsigned int size); /* current bytes -> every slot */
void pw64_player_switch(int i);                           /* no-op if i == cur */
void pw64_players_sync(void);                             /* live bytes -> every other slot */
int pw64_region_count(void);                              /* regions registered so far */
void pw64_players_sync_regions(int first, int end); /* pw64_players_sync for regions [first, end) */
void* pw64_player_peek(int i, void* addr); /* player i's copy of a registered region (or NULL) */
#define PW64_REG(sym) pw64_player_register(&(sym), (unsigned int)sizeof(sym))
/* One per patched file that owns per-player statics; pw64_players_init
 * calls them all (gameInit, only if pw64_players > 1). */
void pw64_reg_game(void);
void pw64_reg_camera(void);
void pw64_reg_hud(void);
void pw64_reg_shadow(void);
void pw64_reg_hang_glider0(void);
void pw64_reg_hang_glider1(void);
void pw64_reg_hang_glider2(void);
void pw64_reg_rocket_belt(void);
void pw64_reg_rocket_belt_sound(void);
void pw64_reg_rocket_belt_poses(void); /* code_AE460.c */
void pw64_reg_rocket_belt_move(void);  /* code_AC1A0.c */
void pw64_reg_gyrocopter(void);        /* gyrocopter.c: rate trim */
void pw64_reg_gyrocopter_sound(void);  /* gyrocopter_sound.c */
void pw64_reg_gyrocopter_poses(void);  /* code_7C4C0.c */
void pw64_reg_gyrocopter_move(void);   /* code_7CF30.c */
void pw64_reg_jumble_hopper(void);     /* code_82B90.c */
void pw64_reg_birdman(void);        /* birdman0.c: camera-look toggle + rate state */
void pw64_reg_birdman_sound(void);  /* bmsound.c */
void pw64_reg_birdman_flutter(void); /* birdman1.c */
void pw64_reg_birdman_move(void);   /* birdman2.c */
void pw64_reg_cannonball(void);       /* cannonball.c: aim, scoring, round state */
void pw64_reg_cannonball_pilot(void); /* code_60020.c: pilot wobble */
void pw64_reg_skydiving(void);        /* skydiving.c: formation, scoring, countdown */
void pw64_reg_skydiving_drift(void);  /* code_BA190.c: rate drift hold */
void pw64_reg_skydiving_sound(void);  /* sdsound.c */
void pw64_players_init(void);
void pw64_reg_task(void); /* task.c: objective hit state (+ its modules) */
void pw64_reg_snap(void); /* snap.c: shutter + this test's photos */
/* Per-player objective dobjs (kernel dobj.c.patch): owning player + 1 per
 * dobj, 0 = shared. Draw and collision loops skip another player's dobj. */
extern unsigned char pw64_dobj_owner[100];
#define PW64_DOBJ_OTHER(id) \
    (pw64_dobj_owner[id] != 0 &&pw64_dobj_owner[id] != pw64_player_cur + 1)
/* Cockpit hide (dobj.c.patch): player + 1 whose first-person view hid the
 * dobj (its state's visible bit off); the other views still draw it. */
extern unsigned char pw64_dobj_cockpit[100];
#define PW64_DOBJ_COCKPIT_OTHER(id) \
    (pw64_dobj_cockpit[id] != 0 && pw64_dobj_cockpit[id] != pw64_player_cur + 1)
/* First-person near draw (dobj.c.patch): player + 1 whose view set state
 * bit 4 (near list) and hid pw64_dobj_near_parts; the other views draw it
 * in the normal pass with those parts shown. Set by the vehicle each frame. */
extern unsigned char pw64_dobj_near[100];
extern unsigned PW64_LONG32 pw64_dobj_near_parts[100];
#define PW64_DOBJ_NEAR_OTHER(id) \
    (pw64_dobj_near[id] != 0 && pw64_dobj_near[id] != pw64_player_cur + 1)
#define PW64_DOBJ_NEAR_PARTS_SHOWN(id) \
    (PW64_DOBJ_NEAR_OTHER(id) ? pw64_dobj_near_parts[id] : 0u)
void pw64_dobj_near_set(PW64_LONG32 objId, PW64_LONG32 on, unsigned PW64_LONG32 parts);
void pw64_dobj_claim(PW64_LONG32 objId);          /* owned by the current player */
PW64_LONG32 pw64_dobj_clone(PW64_LONG32 src);     /* copy for the current player */
/* Kernel controller reads (controller.c.patch): with 2+ players, pad 0
 * means "the current player's pad", so hardcoded-0 sites follow the swap. */
#define PW64_CONT(i) ((pw64_players > 1 && (i) == 0) ? pw64_player_cur : (i))
/* Split-screen sound events (kernel audio_event.c.patch): vehicle sound
 * handlers are owned by the player live when they registered. A post made
 * while pw64_evt_scope = i (>= 0) reaches only player i's handlers; an
 * unscoped one (-1) reaches all but muted players' (pw64_evt_mute bit i:
 * finished or respawning). pw64_evt_post_player posts in player i's scope. */
extern int pw64_evt_scope;
extern unsigned int pw64_evt_mute;
void pw64_evt_post_player(int i, int event);
/* Listener pose of player i > 0 (kernel audio_emitter.c.patch, a Mtx4F*):
 * positional emitters are heard from the nearest player's listener. */
void pw64_listener_set(int i, void* mat);
float pw64_player_dist(int i, float x, float y, float z); /* from player i > 0 */
/* Split-screen free flight (game.c.patch, test_menu.c.patch): 1 while a
 * flight started from the test menu's Free flight item runs. */
extern int pw64_free_flight;
/* Plane variant of the gyrocopter (native/src/pw64_plane.c, experimental):
 * pw64_plane (Rust, live) = which plane flies from the next test (values
 * below; pw64_game::set_plane mirrors them); pw64_plane_run = 1
 * while the current test does (latched by gyrocopterLoadLevel, cleared by
 * pw64_plane_end at test setup and leave). Each hook does nothing / returns
 * 0 without a plane run, so the stock gyrocopter path runs unchanged. */
struct GyrocopterData;
#define PW64_PLANE_OFF 0   /* the gyrocopter */
#define PW64_PLANE_BIG 1   /* model 39, or 27 where 39 isn't loaded */
#define PW64_PLANE_SMALL 2 /* model 27 */
extern int pw64_plane;
extern int pw64_plane_run;
void pw64_plane_load(struct GyrocopterData* gc);
void pw64_plane_enter(struct GyrocopterData* gc);
void pw64_plane_posm(struct GyrocopterData* gc);
void pw64_plane_end(void);
int pw64_plane_forces(struct GyrocopterData* gc);
int pw64_plane_control(struct GyrocopterData* gc);
int pw64_plane_accel(struct GyrocopterData* gc);
int pw64_plane_props(struct GyrocopterData* gc);
int pw64_plane_pose(struct GyrocopterData* gc);
int pw64_plane_rest(struct GyrocopterData* gc);
/* Split-screen: player 2's pilot (native/src/pw64_split.c), -1 = player
 * 1's. Set by the pilot select's second pass (game.c.patch) or, on a
 * PW64_START boot, by PW64_P2_PILOT (pw64-platform start.rs). */
extern int pw64_p2_pilot;
int pw64_p2_pilot_env(void);
/* Our ASCII text in the game's 16-bit glyph strings (native/src/pw64_text.c):
 * glyphs + 0xFFE + -1 into out[max]; returns the glyph count. */
int pw64_text_encode(const char* ascii, short* out, int max);
int pw64_text_encode_style(const char* ascii, short* out, int max, int bold);
/* Per-frame draw budgets, raised from the ROM's (4200 commands, 0x15E
 * matrices, 0x1E look-ats, 100 + 60 sort entries) for split-screen (two
 * world passes) and infinite view distance (every model on the island).
 * Measured worst case, both on, split-screen rocket belt hovering high over
 * an island (PW64_START=a:rb:1): 25045 commands,
 * 1247 matrices, 0 look-ats (0x60 kept), 73 + 16 sort entries. Display
 * list: kernel/graphics.c.patch; matrices: uv_graphics.h.patch (indices are
 * s16); sort lists: chan.c.patch. */
#define PW64_DL_LEN 65536
#define PW64_MATRIX_LEN 4096
#define PW64_SORT_LEN 512
#define PW64_SORT_DOBJ_LEN 128
/* Infinite LOD / infinite view distance (native/src/pw64_visibility.c),
 * set before boot by pw64_game::set_visibility. */
extern int pw64_infinite_lod;
extern int pw64_infinite_view_distance;
float pw64_visibility_far(const void* chan, float far);

#endif
