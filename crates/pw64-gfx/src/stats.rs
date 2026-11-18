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
static EYE_SUBMIT_US: AtomicU64 = AtomicU64::new(0);
static GATE_US: AtomicU64 = AtomicU64::new(0);
static GATE_MAX_US: AtomicU64 = AtomicU64::new(0);
static MISSES: [AtomicU32; MISS_KINDS] = [const { AtomicU32::new(0) }; MISS_KINDS];
static EYE_PREP_US: AtomicU64 = AtomicU64::new(0);
static EYE_ENCODE_US: AtomicU64 = AtomicU64::new(0);
static EYE_CALLS: AtomicU32 = AtomicU32::new(0);
static MERGED: AtomicU64 = AtomicU64::new(0);
static EYES: AtomicU32 = AtomicU32::new(0);
static EYE_DRAWS: AtomicU64 = AtomicU64::new(0);
static EYE_DRAWS_MAX: AtomicU32 = AtomicU32::new(0);

/// Why a draw could not join the draw before it (`merge_runs`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Miss {
    Vertices,
    View,
    Pipeline,
    Steps,
    Textures,
    /// The uniforms differ, first in this 16-byte row of `DrawUniforms`.
    Uniform(usize),
}

/// `DrawUniforms` rows, 16 bytes each (frame.rs).
const UNIFORM_ROWS: [&str; 18] = [
    "screen", "prim", "env", "fog", "blend", "fill", "lod", "lodp", "filt", "vr", "eye", "eye",
    "eye", "eye", "clip", "clip", "clip", "clip",
];
const MISS_KINDS: usize = 5 + UNIFORM_ROWS.len() + 1;

fn miss_name(k: usize) -> &'static str {
    match k {
        0 => "vertices",
        1 => "view",
        2 => "pipeline",
        3 => "steps",
        4 => "textures",
        k if k - 5 < UNIFORM_ROWS.len() => UNIFORM_ROWS[k - 5],
        _ => "uniforms",
    }
}

pub(crate) fn miss(m: Miss) {
    let k = match m {
        Miss::Vertices => 0,
        Miss::View => 1,
        Miss::Pipeline => 2,
        Miss::Steps => 3,
        Miss::Textures => 4,
        Miss::Uniform(row) => 5 + row.min(UNIFORM_ROWS.len()),
    };
    MISSES[k].fetch_add(1, Relaxed);
}

pub(crate) fn gate_wait(d: Duration) {
    GATE_US.fetch_add(us(d), Relaxed);
    GATE_MAX_US.fetch_max(us(d), Relaxed);
}

/// What happened since the last [`take`].
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// Of the stereo calls, the average time submitting (waiting for the
    /// queue included).
    pub eye_submit_ms: f64,
    /// Every submit through `crate::submit`: total and longest wait for the
    /// queue (the headset thread holds it while it makes its frame).
    pub gate_ms: f64,
    pub gate_max_ms: f64,
    /// Why draws were not joined: the three most common reasons, with
    /// their count per eye.
    pub misses: String,
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

pub(crate) fn eye_cpu(prep: Duration, encode: Duration, submit: Duration) {
    EYE_CALLS.fetch_add(1, Relaxed);
    EYE_PREP_US.fetch_add(us(prep), Relaxed);
    EYE_ENCODE_US.fetch_add(us(encode), Relaxed);
    EYE_SUBMIT_US.fetch_add(us(submit), Relaxed);
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
        eye_submit_ms: per_call(EYE_SUBMIT_US.swap(0, Relaxed)),
        gate_ms: GATE_US.swap(0, Relaxed) as f64 / 1e3,
        gate_max_ms: GATE_MAX_US.swap(0, Relaxed) as f64 / 1e3,
        misses: {
            let mut counts: Vec<(usize, u32)> = MISSES
                .iter()
                .map(|a| a.swap(0, Relaxed))
                .enumerate()
                .filter(|&(_, n)| n > 0)
                .collect();
            counts.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
            counts
                .iter()
                .take(3)
                .map(|&(k, n)| format!("{} {:.0}", miss_name(k), n as f64 / eyes.max(1) as f64))
                .collect::<Vec<_>>()
                .join(", ")
        },
    }
}
