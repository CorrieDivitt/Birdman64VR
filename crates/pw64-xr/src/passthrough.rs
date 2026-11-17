//! Passthrough (`XR_FB_passthrough`, Quest): the headset's cameras shown
//! behind the app's picture. The runtime draws the room itself into a
//! composition layer; the app never sees camera images. It only says
//! where that layer goes (first: behind everything) and leaves its own
//! picture transparent where the room should show.
//!
//! Raw calls rather than the openxr crate's `Passthrough`: in 0.22 its
//! `start()` calls `xrPassthroughPauseFB`.

use crate::XrError;
use openxr as xr;
use xr::sys::{self, Handle as _};

/// A passthrough feature and one full-view (reconstruction) layer, both
/// created paused. Children of the session: must drop before it.
pub(crate) struct Passthrough {
    /// Keeps the extension's function table alive.
    instance: xr::Instance,
    feature: sys::PassthroughFB,
    layer: sys::PassthroughLayerFB,
    /// The cameras are running and the layer is to be submitted.
    on: bool,
}

fn check(r: sys::Result) -> Result<(), XrError> {
    if r.into_raw() >= 0 {
        Ok(())
    } else {
        Err(XrError::Xr(r))
    }
}

impl Passthrough {
    /// `instance` must have `XR_FB_passthrough` enabled.
    pub(crate) fn new(
        instance: &xr::Instance,
        session: &xr::Session<xr::Vulkan>,
    ) -> Result<Self, XrError> {
        let fp = instance
            .exts()
            .fb_passthrough
            .as_ref()
            .ok_or_else(|| XrError::Unsupported("passthrough (XR_FB_passthrough)".into()))?;
        let mut feature = sys::PassthroughFB::NULL;
        // SAFETY: a live session, a well-formed create info, an out handle.
        check(unsafe {
            (fp.create_passthrough)(
                session.as_raw(),
                &sys::PassthroughCreateInfoFB {
                    ty: sys::PassthroughCreateInfoFB::TYPE,
                    next: std::ptr::null(),
                    flags: sys::PassthroughFlagsFB::EMPTY,
                },
                &mut feature,
            )
        })?;
        let mut layer = sys::PassthroughLayerFB::NULL;
        // SAFETY: as above; `feature` was just created on this session.
        let made = check(unsafe {
            (fp.create_passthrough_layer)(
                session.as_raw(),
                &sys::PassthroughLayerCreateInfoFB {
                    ty: sys::PassthroughLayerCreateInfoFB::TYPE,
                    next: std::ptr::null(),
                    passthrough: feature,
                    flags: sys::PassthroughFlagsFB::EMPTY,
                    purpose: sys::PassthroughLayerPurposeFB::RECONSTRUCTION,
                },
                &mut layer,
            )
        });
        if let Err(e) = made {
            // SAFETY: created above, not used since.
            unsafe { (fp.destroy_passthrough)(feature) };
            return Err(e);
        }
        Ok(Self {
            instance: instance.clone(),
            feature,
            layer,
            on: false,
        })
    }

    fn fp(&self) -> &xr::raw::PassthroughFB {
        // `new` checked it; the instance's extensions do not change.
        self.instance
            .exts()
            .fb_passthrough
            .as_ref()
            .expect("XR_FB_passthrough is enabled")
    }

    pub(crate) fn on(&self) -> bool {
        self.on
    }

    /// Starts the cameras and the layer, or pauses both (no cost in
    /// flight).
    pub(crate) fn set(&mut self, on: bool) -> Result<(), XrError> {
        if on == self.on {
            return Ok(());
        }
        let fp = self.fp();
        // SAFETY: both handles are live (destroyed only in `drop`).
        unsafe {
            if on {
                check((fp.passthrough_start)(self.feature))?;
                check((fp.passthrough_layer_resume)(self.layer))?;
            } else {
                check((fp.passthrough_layer_pause)(self.layer))?;
                check((fp.passthrough_pause)(self.feature))?;
            }
        }
        self.on = on;
        Ok(())
    }

    /// Forgets that it was on without telling the runtime (the session is
    /// ending; the next [`Self::set`] starts it again).
    pub(crate) fn stopped(&mut self) {
        let _ = self.set(false);
        self.on = false;
    }

    /// The composition layer to submit first, behind the app's own layers
    /// (which need `BLEND_TEXTURE_SOURCE_ALPHA` to let it show).
    pub(crate) fn layer(&self) -> RoomLayer<'_> {
        RoomLayer(
            sys::CompositionLayerPassthroughFB {
                ty: sys::CompositionLayerPassthroughFB::TYPE,
                next: std::ptr::null(),
                flags: sys::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA,
                // A passthrough layer has no space.
                space: sys::Space::NULL,
                layer_handle: self.layer,
            },
            std::marker::PhantomData,
        )
    }
}

/// The passthrough layer for one frame; borrows the [`Passthrough`] whose
/// layer handle it names. (The openxr crate has a builder for this struct
/// but does not export it.)
pub(crate) struct RoomLayer<'a>(
    sys::CompositionLayerPassthroughFB,
    std::marker::PhantomData<&'a Passthrough>,
);

impl<'a> RoomLayer<'a> {
    /// As an entry of the frame's layer list.
    pub(crate) fn base(&self) -> &xr::CompositionLayerBase<'a, xr::Vulkan> {
        // SAFETY: `CompositionLayerBase` is a transparent wrapper of
        // `XrCompositionLayerBaseHeader`, which every composition layer
        // struct starts with (type, next, flags, space); the crate's own
        // layer builders deref the same way.
        unsafe { &*std::ptr::from_ref(&self.0).cast() }
    }
}

impl Drop for Passthrough {
    fn drop(&mut self) {
        let fp = self.fp();
        // SAFETY: created in `new`, destroyed once, layer before feature,
        // both before the session (`XrContext`'s field order).
        unsafe {
            (fp.destroy_passthrough_layer)(self.layer);
            (fp.destroy_passthrough)(self.feature);
        }
    }
}
