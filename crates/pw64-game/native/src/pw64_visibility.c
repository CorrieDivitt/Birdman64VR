/* Optional model detail and whole-island view distance. Model LOD only:
 * texture mipmapping stays, it prevents shimmer and is separate. */
#include <uv_graphics.h>
#include <uv_math.h>
#include <math_macros.h>
#include <macros.h>

/* Set before boot by pw64_game::set_visibility. LOD selection reads both
 * (dobj.c.patch, sobj.c.patch), drawing the view distance (chan.c.patch). */
int pw64_infinite_lod = 0;
int pw64_infinite_view_distance = 0;

/* Far plane for drawing channel `chan` (uvChan_80204FE4): with infinite
 * view distance, far enough to reach every corner of its terrain from the
 * camera, plus a margin for models standing on the edge; else `far`. */
float pw64_visibility_far(const void* chan, float far) {
    const UnkStruct_80204D94* c = chan;
    const ParsedUVTR* t;
    float dx, dy, dz;

    /* Birdman64VR: the island model (pw64_vr_diorama) shows all of it. */
    if (!(pw64_infinite_view_distance || pw64_vr_diorama(chan)) || !(c->unk0 & 2) ||
        c->unk4 >= LEVEL_TERRA_COUNT) {
        return far;
    }
    t = gGfxUnkPtrs->terras[c->unk4];
    if (t == NULL) {
        return far;
    }
    dx = MAX(FABS(t->unk0.unk0 - c->unk1D0), FABS(t->unk0.unkC - c->unk1D0));
    dy = MAX(FABS(t->unk0.unk4 - c->unk1D4), FABS(t->unk0.unk10 - c->unk1D4));
    dz = MAX(FABS(t->unk0.unk8 - c->unk1D8), FABS(t->unk0.unk14 - c->unk1D8));
    return MAX(far, uvSqrtF(SQ(dx) + SQ(dy) + SQ(dz)) + t->unk24 + 1000.0f);
}

/* Animation distance (Graphics page): the game animates objects through
 * proximity slots (proxanim.c) only while the camera is within each one's
 * range: 100 m for the toys (Ferris wheel, merry-go-round, water wheel,
 * pump jacks), 1.2-4 km for boats, the ferry, the ski lift, gliders and
 * planes. Infinite: those ambient ones (marked by their files when they
 * register, proxAnimAddCallback) count as always in range
 * (proxAnimGetRange). Triggered ones stay as they are: the whales, Missi,
 * the shuttle's launch, the fountain, the oil gusher, the oil plant flame.
 * Set any time by pw64_game::set_anim_distance; read every frame. */
int pw64_anim_infinite = 0;

#define PW64_PROX_SLOTS 40 /* sProxAnimSlots (proxanim.c) */
static unsigned char pw64_prox_ambient[PW64_PROX_SLOTS];

void pw64_prox_ambient_set(int prox_id, int ambient) {
    if (prox_id >= 0 && prox_id < PW64_PROX_SLOTS) {
        pw64_prox_ambient[prox_id] = ambient != 0;
    }
}

int pw64_prox_always(int prox_id) {
    return pw64_anim_infinite && prox_id >= 0 && prox_id < PW64_PROX_SLOTS &&
           pw64_prox_ambient[prox_id];
}
