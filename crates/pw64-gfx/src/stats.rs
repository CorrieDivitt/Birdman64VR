//! Birdman64VR: counters for the headset's performance log (birdman64
//! vr.rs, one line every 5 s): shader pipelines compiled and textures
//! uploaded (both on first use, in the middle of a frame: the suspects for
//! hitches), and how many draws each eye picture has.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

static PIPELINES: AtomicU32 = AtomicU32::new(0);
static PIPELINE_US: AtomicU64 = AtomicU64::new(0);
static PIPELINE_MAX_US: AtomicU64 = AtomicU64::new(0);
static SHADERS: AtomicU32 = AtomicU32::new(0);
static SHADER_US: AtomicU64 = AtomicU64::new(0);
static TEXTURES: AtomicU32 = AtomicU32::new(0);
static TEXTURE_US: AtomicU64 = AtomicU64::new(0);
static EYE_PREP_US: AtomicU64 = AtomicU64::new(0);
static EYE_ENCODE_US: AtomicU64 = AtomicU64::new(0);
static EYE_CALLS: AtomicU32 = AtomicU32::new(0);
static MERGED: AtomicU64 = AtomicU64::new(0);
static EYES: AtomicU32 = AtomicU32::new(0);
static EYE_DRAWS: AtomicU64 = AtomicU64::new(0);
static EYE_DRAWS_MAX: AtomicU32 = AtomicU32::new(0);

/// What happened since the last [`take`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Counts {
    /// Pipelines compiled (shader module included), their total and longest
    /// time.
    pub pipelines: u32,
    pub pipeline_ms: f64,
    pub pipeline_max_ms: f64,
    /// Of those, shader modules made (WGSL translated) and their time.
    pub shaders: u32,
    pub shader_ms: f64,
    /// Textures uploaded and their total time.
    pub textures: u32,
    pub texture_ms: f64,
    /// Eye pictures drawn, and their draws (average and most).
    pub eyes: u32,
    pub eye_draws_avg: f64,
    pub eye_draws_max: u32,
    /// Draws joined to the draw before (`merge_runs`), per eye on average.
    pub merged_avg: f64,
    /// Stereo calls (both eyes of a frame), and their average CPU time
    /// preparing (uniforms, uploads) and recording plus submitting.
    pub eye_calls: u32,
    pub eye_prep_ms: f64,
    pub eye_encode_ms: f64,
}

fn us(d: Duration) -> u64 {
    d.as_micros().min(u64::MAX as u128) as u64
}

pub(crate) fn pipeline(d: Duration) {
    PIPELINES.fetch_add(1, Relaxed);
    PIPELINE_US.fetch_add(us(d), Relaxed);
    PIPELINE_MAX_US.fetch_max(us(d), Relaxed);
}

pub(crate) fn shader(d: Duration) {
    SHADERS.fetch_add(1, Relaxed);
    SHADER_US.fetch_add(us(d), Relaxed);
}

pub(crate) fn texture(d: Duration) {
    TEXTURES.fetch_add(1, Relaxed);
    TEXTURE_US.fetch_add(us(d), Relaxed);
}

pub(crate) fn eye(draws: usize) {
    let draws = draws.min(u32::MAX as usize) as u32;
    EYES.fetch_add(1, Relaxed);
    EYE_DRAWS.fetch_add(draws as u64, Relaxed);
    EYE_DRAWS_MAX.fetch_max(draws, Relaxed);
}

pub(crate) fn merged(n: usize) {
    MERGED.fetch_add(n as u64, Relaxed);
}

pub(crate) fn eye_cpu(prep: Duration, encode: Duration) {
    EYE_CALLS.fetch_add(1, Relaxed);
    EYE_PREP_US.fetch_add(us(prep), Relaxed);
    EYE_ENCODE_US.fetch_add(us(encode), Relaxed);
}

/// The counts since the last call, which starts them again from zero.
pub fn take() -> Counts {
    let eyes = EYES.swap(0, Relaxed);
    let draws = EYE_DRAWS.swap(0, Relaxed);
    let calls = EYE_CALLS.swap(0, Relaxed);
    let per_call = |us: u64| {
        if calls == 0 {
            0.0
        } else {
            us as f64 / 1e3 / calls as f64
        }
    };
    Counts {
        pipelines: PIPELINES.swap(0, Relaxed),
        pipeline_ms: PIPELINE_US.swap(0, Relaxed) as f64 / 1e3,
        pipeline_max_ms: PIPELINE_MAX_US.swap(0, Relaxed) as f64 / 1e3,
        shaders: SHADERS.swap(0, Relaxed),
        shader_ms: SHADER_US.swap(0, Relaxed) as f64 / 1e3,
        textures: TEXTURES.swap(0, Relaxed),
        texture_ms: TEXTURE_US.swap(0, Relaxed) as f64 / 1e3,
        eyes,
        eye_draws_avg: if eyes == 0 {
            0.0
        } else {
            draws as f64 / eyes as f64
        },
        eye_draws_max: EYE_DRAWS_MAX.swap(0, Relaxed),
        merged_avg: {
            let m = MERGED.swap(0, Relaxed);
            if eyes == 0 {
                0.0
            } else {
                m as f64 / eyes as f64
            }
        },
        eye_calls: calls,
        eye_prep_ms: per_call(EYE_PREP_US.swap(0, Relaxed)),
        eye_encode_ms: per_call(EYE_ENCODE_US.swap(0, Relaxed)),
    }
}
