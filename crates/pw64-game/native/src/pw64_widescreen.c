/*
 * Hor+ widescreen (PW64_WIDESCREEN, renderer.md "Widescreen") and fill
 * screen (PW64_FILL_SCREEN, renderer.md "Fill screen"): the projection is
 * unchanged, the HLE renderer draws the world view's perspective geometry
 * past the 4:3 edges and its letterbox rows. What the C side must do:
 *  - cull with a frustum as wide (and, on a fill view, as tall) as the
 *    shown view (code_7150.c func_802061A0: object planes + terrain
 *    footprint), else objects and terrain pop at the new edges;
 *  - tag the world view in the display list (chan.c uvChan_80204FE4), so
 *    the renderer extends only that: a model shown over a 2D menu (pilot
 *    select) must stay inside its 4:3 background;
 *  - on a fill view, tell the border bars (drawScreenBorder, level_select)
 *    not to draw this frame (pw64_fill_frame, reset per uvGfxBegin).
 */
#include <ultra64.h>
#include <uv_graphics.h>
#include <uv_math.h>
#include <uv_matrix.h>
#include "app/game.h"
#include "app/smoke.h"

/* Output aspect w/h, 0 = 4:3 (off). Set by Rust before boot
 * (pw64_game::set_widescreen_aspect).
 * Birdman64VR: negative = VR. The picture stays 4:3 with its bars (every
 * `> 0` / `<= 0` test here and in the patches reads it as off), but the
 * main world view is tagged like a widescreen one, so the renderer can
 * draw it again for each eye, and culled with a frustum -aspect times the
 * camera's in both directions: a headset sees far more than the monitor
 * picture, and the player looks around in it. */
float pw64_widescreen_aspect = 0.0f;

/* VR: the world goes to the headset in stereo only while the player flies
 * a test (pause included). Everywhere else (title fly-over, menus with a
 * 3D background, results, replays) the camera moves on its own, swooping
 * and cutting, which is disorienting when it is the player's head: those
 * stay a flat picture on the VR panel, untagged and culled as usual. */
int pw64_vr_results_menu = 0;

int pw64_vr_flight(void) {
    if (pw64_widescreen_aspect >= 0.0f || D_80362690 == NULL) {
        return 0;
    }
    if (D_80362690->state == GAME_STATE_TEST_UPDATE) {
        pw64_vr_results_menu = 0;
        return 1;
    }
    /* Birdman64VR: after landing the view stays full size around the
     * player until the results menu appears (the player sees how they
     * did; the cannonball waits there between shots). */
    return D_80362690->state == GAME_STATE_RESULTS && !pw64_vr_results_menu;
}

void pw64_vr_crash_camera(void* cur_, const void* target_, const void* pilot_, float dt, float now) {
    static float last = -1.0f;
    Mtx4F* cur = cur_;
    const Mtx4F* target = target_;
    const Mtx4F* pilot = pilot_;
    float k = dt * 0.75f; /* about half of the way in a second */
    float f[3], r[3], d[3], len;
    int i;

    /* A new crash (this camera was not running last frame): back off 10 m
     * along the line from the pilot, never below them, and look at them
     * straight away. */
    if (last < 0.0f || now - last > 0.25f || now < last) {
        for (i = 0; i < 3; i++) {
            d[i] = cur->m[3][i] - pilot->m[3][i];
        }
        len = uvSqrtF(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
        if (len > 1e-3f) {
            for (i = 0; i < 3; i++) {
                cur->m[3][i] += d[i] / len * 10.0f;
            }
            if (cur->m[3][2] < pilot->m[3][2]) {
                cur->m[3][2] = pilot->m[3][2];
            }
        }
        k = 1.0f;
    }
    last = now;

    if (k > 1.0f) {
        k = 1.0f;
    }
    for (i = 0; i < 3; i++) {
        f[i] = cur->m[1][i] + (target->m[1][i] - cur->m[1][i]) * k;
    }
    len = uvSqrtF(f[0] * f[0] + f[1] * f[1] + f[2] * f[2]);
    if (len < 1e-4f) {
        return;
    }
    for (i = 0; i < 3; i++) {
        f[i] /= len;
    }
    /* Level: right is horizontal (up = world z), no roll. Straight up or
     * down keeps the last right. */
    r[0] = f[1];
    r[1] = -f[0];
    r[2] = 0.0f;
    len = uvSqrtF(r[0] * r[0] + r[1] * r[1]);
    if (len < 1e-4f) {
        return;
    }
    r[0] /= len;
    r[1] /= len;
    for (i = 0; i < 3; i++) {
        cur->m[0][i] = r[i];
        cur->m[1][i] = f[i];
    }
    cur->m[2][0] = r[1] * f[2] - r[2] * f[1];
    cur->m[2][1] = r[2] * f[0] - r[0] * f[2];
    cur->m[2][2] = r[0] * f[1] - r[1] * f[0];
}

/* Camera matrix `cur` (position kept) looking along `f` (normalised in
 * place), level: right horizontal, no roll. 0 when `f` is too short or
 * straight up or down (nothing changed). */
static int pw64_vr_look_level(Mtx4F* cur, float f[3]) {
    float r[3], len;
    int i;

    len = uvSqrtF(f[0] * f[0] + f[1] * f[1] + f[2] * f[2]);
    if (len < 1e-4f) {
        return 0;
    }
    for (i = 0; i < 3; i++) {
        f[i] /= len;
    }
    r[0] = f[1];
    r[1] = -f[0];
    r[2] = 0.0f;
    len = uvSqrtF(r[0] * r[0] + r[1] * r[1]);
    if (len < 1e-4f) {
        return 0;
    }
    r[0] /= len;
    r[1] /= len;
    for (i = 0; i < 3; i++) {
        cur->m[0][i] = r[i];
        cur->m[1][i] = f[i];
        cur->m[i][3] = 0.0f;
    }
    cur->m[2][0] = r[1] * f[2] - r[2] * f[1];
    cur->m[2][1] = r[2] * f[0] - r[0] * f[2];
    cur->m[2][2] = r[0] * f[1] - r[1] * f[0];
    cur->m[3][3] = 1.0f;
    return 1;
}

void pw64_vr_heading_frame(void* out_, const void* at_, float vx, float vy) {
    Mtx4F* out = out_;
    const Mtx4F* at = at_;
    float f[3];
    int i;

    f[0] = vx;
    f[1] = vy;
    f[2] = 0.0f;
    if (!pw64_vr_look_level(out, f)) {
        f[0] = at->m[1][0];
        f[1] = at->m[1][1];
        f[2] = 0.0f;
        if (!pw64_vr_look_level(out, f)) {
            uvMat4SetIdentity(out);
        }
    }
    for (i = 0; i < 3; i++) {
        out->m[3][i] = at->m[3][i];
    }
}

void pw64_vr_cannon_cinematic(void* cur_, const void* cannon_, const void* pilot_, float dt, float now) {
    static float last = -1.0f;
    static Mtx4F view;
    Mtx4F* cur = cur_;
    const Mtx4F* cannon = cannon_;
    const Mtx4F* pilot = pilot_;
    float t[3], f[3], d[2], len, side, back;
    int i;

    (void)dt;
    /* A new shot or the view turned back on: place it once. Off to the
     * right of the line from the cannon to the target and a little short of
     * the target, at the bullseye's height, looking at the point that far
     * along the line: the target on one side of the view, the last part of
     * the flight towards it on the other. */
    if (last < 0.0f || now - last > 0.25f || now < last) {
        if (!pw64_cannon_target(t)) {
            for (i = 0; i < 3; i++) {
                t[i] = pilot->m[3][i] + pilot->m[1][i] * 100.0f;
            }
        }
        d[0] = t[0] - cannon->m[3][0];
        d[1] = t[1] - cannon->m[3][1];
        len = uvSqrtF(d[0] * d[0] + d[1] * d[1]);
        if (len < 1.0f) {
            d[0] = 0.0f;
            d[1] = 1.0f;
            len = 100.0f;
        } else {
            d[0] /= len;
            d[1] /= len;
        }
        side = len * PW64_VR_CINE_SIDE;
        if (side < PW64_VR_CINE_SIDE_MIN_M) {
            side = PW64_VR_CINE_SIDE_MIN_M;
        } else if (side > PW64_VR_CINE_SIDE_MAX_M) {
            side = PW64_VR_CINE_SIDE_MAX_M;
        }
        back = side * (PW64_VR_CINE_BACK / PW64_VR_CINE_SIDE);
        uvMat4SetIdentity(&view);
        view.m[3][0] = t[0] - d[0] * back + d[1] * side;
        view.m[3][1] = t[1] - d[1] * back - d[0] * side;
        view.m[3][2] = t[2] + PW64_VR_CINE_UP_M;
        f[0] = (t[0] - d[0] * back) - view.m[3][0];
        f[1] = (t[1] - d[1] * back) - view.m[3][1];
        f[2] = t[2] - view.m[3][2];
        if (!pw64_vr_look_level(&view, f)) {
            uvMat4Copy(&view, cur);
        }
    }
    last = now;
    uvMat4Copy(cur, &view);
}

float pw64_vr_cannon_scale(const void* pilot_, const void* cam_) {
    const Mtx4F* pilot = pilot_;
    const Mtx4F* cam = cam_;
    float t[3], d[3], s, w;
    int i;

    for (i = 0; i < 3; i++) {
        d[i] = pilot->m[3][i] - cam->m[3][i];
    }
    s = uvSqrtF(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) / PW64_VR_CINE_SCALE_REF_M;
    if (s < 1.0f) {
        s = 1.0f;
    } else if (s > PW64_VR_CINE_SCALE_MAX) {
        s = PW64_VR_CINE_SCALE_MAX;
    }
    if (pw64_cannon_target(t)) {
        for (i = 0; i < 3; i++) {
            d[i] = pilot->m[3][i] - t[i];
        }
        /* Back to normal over the last 120 m to 30 m before the target. */
        w = (uvSqrtF(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) - 30.0f) / 90.0f;
        if (w < 0.0f) {
            w = 0.0f;
        } else if (w > 1.0f) {
            w = 1.0f;
        }
        s = 1.0f + (s - 1.0f) * w;
    }
    return s;
}

void pw64_vr_cannon_trail(int on, const void* pilot_, float dt) {
    /* A smoke has at most 10 puffs, one every tenth of its life: a few of
     * them started a little apart make a denser, longer trail. */
    static s32 trail[PW64_VR_TRAIL_N] = { -1, -1, -1 };
    static float flown = 0.0f;
    const Mtx4F* pilot = pilot_;
    int i;

    for (i = 0; i < PW64_VR_TRAIL_N; i++) {
        if (on) {
            if ((trail[i] < 0) && (flown >= i * (PW64_VR_TRAIL_LIFE_S / 10.0f / PW64_VR_TRAIL_N))) {
                trail[i] = smokeCreate();
                if (trail[i] < 0) {
                    continue;
                }
                smokeProps(trail[i], SMOKE_PROP_1(PW64_VR_TRAIL_GREY, PW64_VR_TRAIL_GREY, PW64_VR_TRAIL_GREY),
                           SMOKE_PROP_2(PW64_VR_TRAIL_SIZE_M), SMOKE_PROP_3(PW64_VR_TRAIL_LIFE_S),
                           SMOKE_PROP_5(0.0f, 0.0f, 0.0f), SMOKE_PROP_END);
            }
            if (trail[i] >= 0) {
                smokeProps(trail[i], SMOKE_PROP_6(pilot->m[3][0], pilot->m[3][1], pilot->m[3][2]), SMOKE_PROP_END);
            }
        } else if (trail[i] >= 0) {
            /* Landed: no more puffs; the ones out there fade on their own
             * and the smoke frees itself after the last. */
            smokeProps(trail[i], SMOKE_PROP_4(0.0f), SMOKE_PROP_END);
            trail[i] = -1;
        }
    }
    flown = on ? flown + dt : 0.0f;
}

/* VR outside a test's flight: a full-width world view of an island is
 * shown as a small model of it in front of the player (pw64-gfx
 * `Diorama`, birdman64 vr.rs): the game's camera can swoop and cut as it
 * likes, the model stays put. The island is the view's terrain or, on the
 * title screen (no terrain, the island is one model at the origin), that
 * model. Culled by distance only like flight; with terrain the far plane
 * reaches all of it (pw64_visibility_far), so the whole island is there
 * whatever the camera looks at. */

/* The title's island (title_screen.c func_803427FC). */
#define PW64_TITLE_ISLAND 0x028 /* MODEL_CRESCENT_ISLAND_LOW_POLY */

/* Bounds (min xyz, max xyz) of a model's vertices, measured once per
 * loaded model. 0 if it is not loaded. */
static int pw64_model_bounds(int id, float out[6]) {
    static const ParsedUVMD* measured = NULL;
    static float bounds[6];
    const ParsedUVMD* m;
    int i, k;

    if (gGfxUnkPtrs == NULL || (m = gGfxUnkPtrs->models[id]) == NULL || m->vtxCount == 0) {
        return 0;
    }
    if (m != measured) {
        for (k = 0; k < 3; k++) {
            bounds[k] = bounds[k + 3] = m->vtxTable[0].v.ob[k];
        }
        for (i = 1; i < m->vtxCount; i++) {
            for (k = 0; k < 3; k++) {
                float v = m->vtxTable[i].v.ob[k];
                if (v < bounds[k]) {
                    bounds[k] = v;
                }
                if (v > bounds[k + 3]) {
                    bounds[k + 3] = v;
                }
            }
        }
        measured = m;
    }
    for (k = 0; k < 6; k++) {
        out[k] = bounds[k];
    }
    return 1;
}

/* The island model's bounds for channel `c` (see above); 0: none. */
static int pw64_diorama_bounds(const UnkStruct_80204D94* c, float out[6]) {
    const ParsedUVTR* t;

    if (gGfxUnkPtrs == NULL) {
        return 0;
    }
    if ((c->unk0 & 2) && c->unk4 < LEVEL_TERRA_COUNT && (t = gGfxUnkPtrs->terras[c->unk4]) != NULL) {
        out[0] = t->unk0.unk0;
        out[1] = t->unk0.unk4;
        out[2] = t->unk0.unk8;
        out[3] = t->unk0.unkC;
        out[4] = t->unk0.unk10;
        out[5] = t->unk0.unk14;
        return 1;
    }
    if (D_80362690 != NULL && D_80362690->state == GAME_STATE_TITLE && c->unk2 != 0xFFFF) {
        return pw64_model_bounds(PW64_TITLE_ISLAND, out);
    }
    return 0;
}

int pw64_vr_diorama(const void* chan) {
    const UnkStruct_80204D94* c = chan;
    float b[6];
    return pw64_widescreen_aspect < 0.0f && !pw64_vr_flight() && c->viewX0 <= 16 &&
           c->viewX1 >= SCREEN_WIDTH - 16 && pw64_diorama_bounds(c, b);
}

int pw64_vr_stereo_view = 0;
int pw64_vr_title_phase = -1;

/* Must match pw64-gfx `interp::DIORAMA_TAG`: G_NOOP with this in w0's low
 * 24 bits (+ the value's index in the low byte) and an f32's bits in w1. */
#define PW64_DIORAMA_TAG 0x445200u /* "DR" */

/* The diorama's data for the renderer, right after the world tag, 22
 * values (the last: the vehicle): the game camera's world matrix (rows right, forward, up,
 * position; xyz), the island's bounds (min xyz, max xyz), 1 on the title
 * screen (the headset follows its camera, slowed down by the title's
 * patch, instead of showing a model), the game state and the title's
 * phase (for the log). */
static void pw64_diorama_tag(const UnkStruct_80204D94* c) {
    float v[22];
    int i, k, title;
    for (i = 0; i < 4; i++) {
        for (k = 0; k < 3; k++) {
            v[i * 3 + k] = c->unk110.m[i][k];
        }
    }
    if (!pw64_diorama_bounds(c, &v[12])) {
        return;
    }
    /* 1: the title (followed, behind and below); 2: a badge celebration
     * (its camera stands still: followed, full size, level). */
    title = D_80362690 != NULL && D_80362690->state == GAME_STATE_TITLE;
    v[18] = title ? 1.0f
            : (D_80362690 != NULL && D_80362690->state == GAME_STATE_CONGRATULATIONS) ? 2.0f
                                                                                    : 0.0f;
    v[19] = D_80362690 != NULL ? (float)D_80362690->state : -1.0f;
    v[20] = title ? (float)pw64_vr_title_phase : -1.0f;
    v[21] = D_80362690 != NULL ? (float)D_80362690->unkC[D_80362690->unk9C].veh : -1.0f;
    for (i = 0; i < 22; i++) {
        union {
            float f;
            u32 u;
        } bits;
        bits.f = v[i];
        gGfxDisplayListHead->words.w0 = ((u32)G_NOOP << 24) | PW64_DIORAMA_TAG | (u32)i;
        gGfxDisplayListHead->words.w1 = bits.u;
        gGfxDisplayListHead++;
    }
}

/* Fill screen, 0 = off. Set by Rust before boot
 * (pw64_game::set_fill_screen). */
int pw64_fill_screen = 0;

/* 1 once a filled world view was drawn this frame; reset at the top of
 * uvGfxBegin (graphics.c.patch), read by the border bars. */
int pw64_fill_frame = 0;

/* Must match pw64-gfx `interp::WIDE_TAG_ON/OFF` (G_NOOP w1). */
#define PW64_WIDE_TAG_ON 0x50575731u  /* "PWW1" */
#define PW64_WIDE_TAG_OFF 0x50575730u /* "PWW0" */

/* A channel that draws the 3D world: terrain (unk0 bit 2) or an
 * environment (unk2), over almost the full screen width. */
static int pw64_world_view(const UnkStruct_80204D94* c) {
    return (c->unk0 & 2 || c->unk2 != 0xFFFF) && c->viewX0 <= 16 &&
           c->viewX1 >= SCREEN_WIDTH - 16;
}

/* A fill view: fill on, a world view and a viewport covering the
 * letterboxed rows (bottom-up; the flight view is 18..232). 2D screens'
 * channels never match, so their bars stay. */
static int pw64_fill_view(const UnkStruct_80204D94* c) {
    return pw64_fill_screen && pw64_world_view(c) && c->viewY0 <= 20 &&
           c->viewY1 >= 220;
}

/* Horizontal culling-frustum scale for channel `chan`. Widened: a world
 * view (terrain or environment) whose viewport is the main view (full width
 * or the 10..310 flight subscreen; same bounds as pw64-gfx `Wide::MAIN_VIEW`),
 * shown 240*aspect N64 pixels wide. Other channels stay 4:3.
 * Fill screen without widescreen still shows the world past the subscreen
 * columns (the renderer extends its world draws to the full output), so the
 * effective aspect is 4:3 there.
 * +2% margin for the rounding of the output area. */
float pw64_widescreen_xscale(const void* chan) {
    const UnkStruct_80204D94* c = chan;
    float aspect = pw64_widescreen_aspect;
    float k;
    if (aspect < 0.0f) {
        /* VR: the main world view (the same test as below), in flight. */
        if (!(pw64_vr_flight() || pw64_vr_diorama(c)) || c->viewX0 > 16 ||
            c->viewX1 < SCREEN_WIDTH - 16 ||
            c->viewX1 <= c->viewX0 || (!(c->unk0 & 2) && c->unk2 == 0xFFFF)) {
            return 1.0f;
        }
        return -aspect;
    }
    if (aspect <= 0.0f) {
        if (!pw64_fill_view(c)) {
            return 1.0f;
        }
        aspect = 4.0f / 3.0f;
    }
    if (c->viewX0 > 16 || c->viewX1 < SCREEN_WIDTH - 16 ||
        c->viewX1 <= c->viewX0 || (!(c->unk0 & 2) && c->unk2 == 0xFFFF)) {
        return 1.0f;
    }
    k = 240.0f * aspect / (float)(c->viewX1 - c->viewX0) * 1.02f;
    return k > 1.0f ? k : 1.0f;
}

/* Vertical culling-frustum scale for channel `chan` on a fill view, 1
 * otherwise. The guard viewport (uvGfxClipRect: the view +/- 5 px, clamped
 * to the screen) is centred at top-down row SCREEN_HEIGHT - g0 - a/2 with
 * NDC -1/+1 = a/2 rows (a = g1 - g0); the frustum must reach the guard row
 * farthest from that centre. Without fill the old view is symmetric about
 * row 115 and the game's frustum already reaches both ends.
 * +2% margin like the x scale. */
float pw64_fill_yscale(const void* chan) {
    const UnkStruct_80204D94* c = chan;
    int g0, g1, half, centre, hi;
    float k;
    if (pw64_widescreen_aspect < 0.0f) {
        /* VR: as tall as it is wide (pw64_widescreen_xscale). */
        return pw64_widescreen_xscale(chan);
    }
    if (!pw64_fill_view(c)) {
        return 1.0f;
    }
    g0 = c->viewY0 - 5;
    if (g0 < 0) {
        g0 = 0;
    }
    g1 = c->viewY1 + 5;
    if (g1 > SCREEN_HEIGHT - 1) {
        g1 = SCREEN_HEIGHT - 1;
    }
    half = (g1 - g0) / 2;
    centre = SCREEN_HEIGHT - g0 - half;
    hi = centre > SCREEN_HEIGHT - centre ? centre : SCREEN_HEIGHT - centre;
    k = (float)hi / (float)half * 1.02f;
    return k > 1.0f ? k : 1.0f;
}

/* Output aspect a left/right split view extends into: the widescreen
 * aspect, 4:3 with fill screen alone (the views reach the 4:3 edges), 0
 * when neither (the views stay inside the subscreen). Must match the
 * renderer's split world placement and pw64_split_viewport's frustum. */
float pw64_split_out_aspect(void) {
    if (pw64_widescreen_aspect > 0.0f) {
        return pw64_widescreen_aspect;
    }
    return pw64_fill_screen ? 4.0f / 3.0f : 0.0f;
}

/* Must match pw64-gfx `interp::SPLIT_TAG` ("PWS" + 'X'/'Y'/'0'). */
#define PW64_SPLIT_TAG 0x50575300u

/* The split-view tag pair for channel `c`'s viewport (top-down rows). */
static void pw64_split_rect_tag(const UnkStruct_80204D94* c) {
    gGfxDisplayListHead->words.w0 = ((u32)G_NOOP << 24) | ((u32)c->viewX0 << 12) | (u32)c->viewX1;
    gGfxDisplayListHead->words.w1 = PW64_SPLIT_TAG | 'X';
    gGfxDisplayListHead++;
    gGfxDisplayListHead->words.w0 = ((u32)G_NOOP << 24) | ((u32)(SCREEN_HEIGHT - c->viewY1) << 12) |
                                    (u32)(SCREEN_HEIGHT - c->viewY0);
    gGfxDisplayListHead->words.w1 = PW64_SPLIT_TAG | 'Y';
    gGfxDisplayListHead++;
}

static void pw64_split_off_tag(void) {
    gGfxDisplayListHead->words.w0 = (u32)G_NOOP << 24;
    gGfxDisplayListHead->words.w1 = PW64_SPLIT_TAG | '0';
    gGfxDisplayListHead++;
}

/* A left/right split world view (not full width) on a wider output: the
 * renderer stretches its draws from the outer subscreen edge to the
 * output's edge (pw64-gfx split world placement; the frustum is that wide,
 * pw64_split_viewport). The full-screen results view is a plain world
 * view. Culling follows the camera's own (already wide) frustum. */
static int pw64_split_world_view(const UnkStruct_80204D94* c) {
    return pw64_level_players > 1 && pw64_split_lr && pw64_split_out_aspect() > 0.0f &&
           (c->unk0 & 2 || c->unk2 != 0xFFFF) &&
           !(c->viewX0 <= 16 && c->viewX1 >= SCREEN_WIDTH - 16);
}

/* Brackets a channel's draw (on = 1 at the start, 0 at the end) with
 * G_NOOP markers when it is a widened world view or a fill view. RDP NOOPs
 * are harmless on hardware. A fill view marks the frame: the border bars
 * stay off until the next uvGfxBegin. A left/right split world view gets
 * its view rect too (the split tag outside a HUD bracket). */
void pw64_widescreen_tag(const void* chan, int on) {
    if (pw64_split_world_view(chan)) {
        if (!on) {
            pw64_split_off_tag();
        }
        gGfxDisplayListHead->words.w0 = (u32)G_NOOP << 24;
        gGfxDisplayListHead->words.w1 = on ? PW64_WIDE_TAG_ON : PW64_WIDE_TAG_OFF;
        gGfxDisplayListHead++;
        if (on) {
            pw64_split_rect_tag(chan);
        }
        return;
    }
    if (pw64_widescreen_xscale(chan) <= 1.0f && !pw64_fill_view(chan)) {
        return;
    }
    /* VR: no border bars either: the eye pictures have no screen edge for
     * them to sit on (they showed as black bars floating in the world). */
    if (on && (pw64_fill_view(chan) || pw64_widescreen_aspect < 0.0f)) {
        pw64_fill_frame = 1;
    }
    gGfxDisplayListHead->words.w0 = (u32)G_NOOP << 24;
    gGfxDisplayListHead->words.w1 = on ? PW64_WIDE_TAG_ON : PW64_WIDE_TAG_OFF;
    gGfxDisplayListHead++;
    if (pw64_widescreen_aspect < 0.0f) {
        pw64_vr_stereo_view = on;
        if (on && pw64_vr_diorama(chan)) {
            pw64_diorama_tag(chan);
        }
    }
}

/*
 * Flight HUD edge anchoring. hud.c calls pw64_hud_anchor_x(x) at the top of
 * each gauge helper; the renderer moves the following 2D draws by the side
 * margin (pw64-gfx `Anchor`). Text is queued (uvFontPrintStr) and drawn at
 * uvFontGenDlist, so font.c stores the anchor current at print time per
 * message and re-emits it (pw64_hud_emit_anchor) when drawing.
 */

/* Must match pw64-gfx `interp::ANCHOR_TAG` ("PWA" + 'L'/'C'/'R'). */
#define PW64_ANCHOR_TAG 0x50574100u

/* Anchor for text printed now ('C' outside the flight HUD). */
int pw64_hud_anchor = 'C';

/* Vertical anchor for the following draws, used by the renderer on Fixed
 * HUD draws of a fill view ('M' elsewhere). */
int pw64_hud_vanchor = 'M';

/* Must match pw64-gfx `interp::VANCHOR_TAG` ("PWV" + 'T'/'M'/'B'). */
#define PW64_VANCHOR_TAG 0x50575600u

void pw64_hud_emit_anchor(int anchor) {
    if (pw64_widescreen_aspect <= 0.0f && pw64_level_players <= 1) {
        return;
    }
    gGfxDisplayListHead->words.w0 = (u32)G_NOOP << 24;
    gGfxDisplayListHead->words.w1 = PW64_ANCHOR_TAG | (u32)(anchor & 0xFF);
    gGfxDisplayListHead++;
}

/* The vertical anchor matters on a fill view (with or without
 * widescreen: 4:3 + fill shifts HUD rows too) and in split-screen views
 * (both anchors: pw64_hud_split_tag). */
void pw64_hud_emit_vanchor(int anchor) {
    if (pw64_widescreen_aspect <= 0.0f && !pw64_fill_screen && pw64_level_players <= 1) {
        return;
    }
    gGfxDisplayListHead->words.w0 = (u32)G_NOOP << 24;
    gGfxDisplayListHead->words.w1 = PW64_VANCHOR_TAG | (u32)(anchor & 0xFF);
    gGfxDisplayListHead++;
}

void pw64_hud_set_anchor(int anchor) {
    pw64_hud_anchor = anchor;
    /* Every HUD element without an explicit vertical anchor keeps its old
     * row: resetting to 'M' makes each set_anchor a complete anchor pair. */
    pw64_hud_vanchor = 'M';
    pw64_hud_emit_anchor(anchor);
    pw64_hud_emit_vanchor('M');
}

void pw64_hud_set_anchor2(int anchor, int vanchor) {
    pw64_hud_anchor = anchor;
    pw64_hud_vanchor = vanchor;
    pw64_hud_emit_anchor(anchor);
    pw64_hud_emit_vanchor(vanchor);
}

/* Anchor for a HUD element at N64 x `x` (bottom-up) `y`: the left gauge
 * column (x 27) and the right one (radar 215, sea level 235, altimeter 250)
 * go to the horizontal edges; y 160+ is the top row of the screen (timer,
 * radar), y < 100 the bottom (speed, fuel, throttle), else middle
 * (altimeter). */
void pw64_hud_anchor_xy(int x, int y) {
    pw64_hud_set_anchor2(x < 80 ? 'L' : x >= 200 ? 'R' : 'C',
                         y >= 160 ? 'T' : y < 100 ? 'B' : 'M');
}

/*
 * Flight-HUD on/off bracket. hud.c.patch calls pw64_hud_tag(1) at the top
 * of hudMainRender's non-disabled branch and pw64_hud_tag(0) at its end,
 * so the renderer knows which draws are the flight HUD (the only ones its
 * OLED care options drift + dim). Not gated on the widescreen/fill
 * settings: the OLED care setting is live in the renderer, which ignores
 * the tag unless it is on.
 */

/* Must match pw64-gfx `interp::HUD_TAG_ON/OFF` (G_NOOP w1). */
#define PW64_HUD_TAG_ON 0x50574831u  /* "PWH1" */
#define PW64_HUD_TAG_OFF 0x50574830u /* "PWH0" */

void pw64_hud_tag(int on) {
    gGfxDisplayListHead->words.w0 = (u32)G_NOOP << 24;
    gGfxDisplayListHead->words.w1 = on ? PW64_HUD_TAG_ON : PW64_HUD_TAG_OFF;
    gGfxDisplayListHead++;
}

/*
 * Movable HUD elements (renderer.md "Movable HUD"). hud.c.patch calls
 * pw64_hud_element(id) where each gauge starts; the renderer moves the
 * following flight-HUD draws by the player's offset for that element.
 * Text is queued, so font.c.patch stores the element per message like the
 * anchors and re-emits it (pw64_hud_emit_element) when drawing. Not gated
 * on any setting: offsets are live in the renderer, which ignores the tag
 * while they are zero.
 */

/* Must match pw64-gfx `interp::HUD_ELEM_TAG` ("PWE" + element id). */
#define PW64_HUD_ELEM_TAG 0x50574500u

/* Element for the following draws and text printed now (0 = none). */
int pw64_hud_elem = 0;

void pw64_hud_emit_element(int id) {
    gGfxDisplayListHead->words.w0 = (u32)G_NOOP << 24;
    gGfxDisplayListHead->words.w1 = PW64_HUD_ELEM_TAG | (u32)(id & 0xFF);
    gGfxDisplayListHead++;
}

void pw64_hud_element(int id) {
    if (id == pw64_hud_elem) {
        return;
    }
    pw64_hud_elem = id;
    pw64_hud_emit_element(id);
}

/*
 * Split-screen HUD (splitscreen.md "HUD per view"). Each player's HUD is
 * drawn in their channel's callback, still for the full 320x240 screen;
 * hudMainRender brackets it with this tag carrying the view (the current
 * player's channel viewport, top-down rows) and the renderer lays the
 * HUD's subscreen frame out inside it (pw64-gfx `split_placement`: one
 * uniform scale, each element kept at its L/C/R + T/M/B anchor, which
 * pw64_hud_emit_anchor/vanchor also emit in split-screen). One tag covers
 * every draw path (sprites, uvVtx gauges, queued text); the HUD scissor
 * stays full screen in the display list. Only with 2+ players in the test.
 */

void pw64_hud_split_tag(int on) {
    if (pw64_level_players <= 1) {
        return;
    }
    if (!on) {
        pw64_split_off_tag();
        return;
    }
    pw64_split_rect_tag(&D_80261730[pw64_player_cur]);
}
