//! Birdman64VR: counters for the headset's performance log (birdman64
//! vr.rs, one line every 5 s): shader pipelines compiled and textures
//! uploaded (both on first use, in the middle of a frame: the suspects for
//! hitches), and how many draws each eye picture has.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

static PIPELINES: AtomicU32 = AtomicU32::new(0);
static PIPELINE_US: AtomicU64 = AtomicU64::new(0);
static PIPELINE_MAX_US: AtomicU64 = AtomicU64::new(0);
static TEXTURES: AtomicU32 = AtomicU32::new(0);
static TEXTURE_US: AtomicU64 = AtomicU64::new(0);
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
    /// Textures uploaded and their total time.
    pub textures: u32,
    pub texture_ms: f64,
    /// Eye pictures drawn, and their draws (average and most).
    pub eyes: u32,
    pub eye_draws_avg: f64,
    pub eye_draws_max: u32,
}

fn us(d: Duration) -> u64 {
    d.as_micros().min(u64::MAX as u128) as u64
}

pub(crate) fn pipeline(d: Duration) {
    PIPELINES.fetch_add(1, Relaxed);
    PIPELINE_US.fetch_add(us(d), Relaxed);
    PIPELINE_MAX_US.fetch_max(us(d), Relaxed);
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

/// The counts since the last call, which starts them again from zero.
pub fn take() -> Counts {
    let eyes = EYES.swap(0, Relaxed);
    let draws = EYE_DRAWS.swap(0, Relaxed);
    Counts {
        pipelines: PIPELINES.swap(0, Relaxed),
        pipeline_ms: PIPELINE_US.swap(0, Relaxed) as f64 / 1e3,
        pipeline_max_ms: PIPELINE_MAX_US.swap(0, Relaxed) as f64 / 1e3,
        textures: TEXTURES.swap(0, Relaxed),
        texture_ms: TEXTURE_US.swap(0, Relaxed) as f64 / 1e3,
        eyes,
        eye_draws_avg: if eyes == 0 {
            0.0
        } else {
            draws as f64 / eyes as f64
        },
        eye_draws_max: EYE_DRAWS_MAX.swap(0, Relaxed),
    }
}
