//! Birdman64VR: OpenXR on wgpu's Vulkan backend.
//!
//! [`XrContext::new`] connects to the OpenXR runtime (Quest Link, SteamVR,
//! ...), lets it pick the Vulkan device, and wraps that device in the
//! ordinary wgpu types the renderer already uses. [`XrContext::frame`] runs
//! one headset frame; for now it shows one flat panel (an OpenXR quad
//! layer) that the caller draws into.
//!
//! The OpenXR loader is opened at run time, so a machine without it (or
//! without a running runtime) gets an [`XrError`] and can start flat.

mod gpu;
mod session;

pub use gpu::XrGpu;
pub use session::{FrameOutcome, Panel, XrContext};

/// Why VR could not start or continue; `Display` is a one-line reason.
#[derive(Debug)]
pub enum XrError {
    /// `openxr_loader` was not found or failed to load.
    Loader(String),
    /// No OpenXR runtime is active, or no headset is connected to it.
    NoHeadset(String),
    /// The runtime lacks something we need (named in the message).
    Unsupported(String),
    /// Vulkan or wgpu set-up failed.
    Graphics(String),
    /// Any other OpenXR call failed.
    Xr(openxr::sys::Result),
}

impl std::fmt::Display for XrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loader(m) => write!(f, "the OpenXR loader could not be loaded ({m})"),
            Self::NoHeadset(m) => write!(f, "no VR headset is available ({m})"),
            Self::Unsupported(m) => write!(f, "the VR runtime is missing {m}"),
            Self::Graphics(m) => write!(f, "VR graphics set-up failed ({m})"),
            Self::Xr(r) => write!(f, "the VR runtime reported {r}"),
        }
    }
}

impl std::error::Error for XrError {}

impl From<openxr::sys::Result> for XrError {
    fn from(r: openxr::sys::Result) -> Self {
        Self::Xr(r)
    }
}
