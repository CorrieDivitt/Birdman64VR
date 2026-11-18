//! Birdman64VR (q6): the renderer's driver pipeline cache on disk, so the
//! shader pipelines compiled in one run load quickly in the next instead of
//! compiling again (25-40 ms each on the Quest 3, in the middle of a frame:
//! the hitches at the first menus, at each flight's start and in flight).
//! Vulkan only (`wgpu::util::pipeline_cache_key`); elsewhere nothing
//! happens. One file per GPU in `<data dir>/pipeline-cache/`; wgpu and the
//! driver ignore a file that no longer matches (driver update).

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The cache file for this GPU, if its backend has a pipeline cache.
fn path(info: &wgpu::AdapterInfo) -> Option<PathBuf> {
    let key = wgpu::util::pipeline_cache_key(info)?;
    Some(crate::paths::data_dir().join("pipeline-cache").join(key))
}

/// Starts `renderer`'s pipeline cache from the saved file, if any.
pub(crate) fn load(renderer: &mut pw64_gfx::Renderer, info: &wgpu::AdapterInfo) {
    let Some(path) = path(info) else { return };
    let data = std::fs::read(&path).ok();
    if renderer.use_pipeline_cache(data.as_deref()) {
        eprintln!(
            "[gpu] pipeline cache: {}",
            match &data {
                Some(d) => format!("{} KB loaded from {}", d.len() / 1024, path.display()),
                None => "new (nothing saved yet)".into(),
            }
        );
    }
}

/// When the cache was last saved.
static SAVED: Mutex<Option<Instant>> = Mutex::new(None);

/// Saves the cache when pipelines were compiled since the last save, at
/// most every 5 s (`force`: now, e.g. when the app is closing).
pub(crate) fn save(renderer: &mut pw64_gfx::Renderer, adapter: &wgpu::Adapter, force: bool) {
    let mut saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    if !force && saved.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
        return;
    }
    let Some(data) = renderer.take_pipeline_cache_data() else {
        return;
    };
    let Some(path) = path(&adapter.get_info()) else {
        return;
    };
    *saved = Some(Instant::now());
    // Written next to it, then renamed over it: a run that ends mid-write
    // leaves the old file whole.
    let tmp = path.with_extension("tmp");
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| std::fs::write(&tmp, &data))
        .and_then(|_| std::fs::rename(&tmp, &path));
    match result {
        Ok(()) => eprintln!("[gpu] pipeline cache: {} KB saved", data.len() / 1024),
        Err(e) => eprintln!("[gpu] pipeline cache: not saved ({e})"),
    }
}
