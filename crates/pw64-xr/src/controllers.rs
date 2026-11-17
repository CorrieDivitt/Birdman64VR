//! Birdman64VR: the Touch controllers as one gamepad (q3). One OpenXR
//! action per physical input, bound for the Oculus/Meta Touch profile
//! (`/interaction_profiles/oculus/touch_controller`, which the Quest's
//! runtime also maps Touch Pro and Touch Plus onto). The caller turns the
//! state into an Xbox-style pad (birdman64 `input::xr_pad`).

use openxr as xr;

/// The Touch controllers' state at the last sync.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PadState {
    /// Some controller is tracked and bound (the runtime reports it
    /// active): false while both are off, asleep or put down too long.
    pub active: bool,
    /// Right controller: A (lower) and B (upper).
    pub a: bool,
    pub b: bool,
    /// Left controller: X (lower) and Y (upper).
    pub x: bool,
    pub y: bool,
    /// Left controller's menu button (the right one is the system's).
    pub menu: bool,
    /// Thumbstick presses.
    pub left_stick_click: bool,
    pub right_stick_click: bool,
    /// Index triggers and grips, 0..1.
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_grip: f32,
    pub right_grip: f32,
    /// Thumbsticks, x right and y up, -1..1.
    pub left_stick: [f32; 2],
    pub right_stick: [f32; 2],
}

pub(crate) struct Controllers {
    set: xr::ActionSet,
    a: xr::Action<bool>,
    b: xr::Action<bool>,
    x: xr::Action<bool>,
    y: xr::Action<bool>,
    menu: xr::Action<bool>,
    left_stick_click: xr::Action<bool>,
    right_stick_click: xr::Action<bool>,
    left_trigger: xr::Action<f32>,
    right_trigger: xr::Action<f32>,
    left_grip: xr::Action<f32>,
    right_grip: xr::Action<f32>,
    left_stick: xr::Action<xr::Vector2f>,
    right_stick: xr::Action<xr::Vector2f>,
}

const PROFILE: &str = "/interaction_profiles/oculus/touch_controller";

impl Controllers {
    /// Creates the actions, suggests their Touch bindings and attaches them
    /// to `session` (once per session, before the first sync).
    pub(crate) fn new<G: xr::Graphics>(
        instance: &xr::Instance,
        session: &xr::Session<G>,
    ) -> xr::Result<Self> {
        let set = instance.create_action_set("gamepad", "Gamepad", 0)?;
        let c = Self {
            a: set.create_action("a", "A", &[])?,
            b: set.create_action("b", "B", &[])?,
            x: set.create_action("x", "X", &[])?,
            y: set.create_action("y", "Y", &[])?,
            menu: set.create_action("menu", "Menu", &[])?,
            left_stick_click: set.create_action("left_stick_click", "Left stick press", &[])?,
            right_stick_click: set.create_action("right_stick_click", "Right stick press", &[])?,
            left_trigger: set.create_action("left_trigger", "Left trigger", &[])?,
            right_trigger: set.create_action("right_trigger", "Right trigger", &[])?,
            left_grip: set.create_action("left_grip", "Left grip", &[])?,
            right_grip: set.create_action("right_grip", "Right grip", &[])?,
            left_stick: set.create_action("left_stick", "Left stick", &[])?,
            right_stick: set.create_action("right_stick", "Right stick", &[])?,
            set,
        };
        let p = |s: &str| instance.string_to_path(s);
        instance.suggest_interaction_profile_bindings(
            p(PROFILE)?,
            &[
                xr::Binding::new(&c.a, p("/user/hand/right/input/a/click")?),
                xr::Binding::new(&c.b, p("/user/hand/right/input/b/click")?),
                xr::Binding::new(&c.x, p("/user/hand/left/input/x/click")?),
                xr::Binding::new(&c.y, p("/user/hand/left/input/y/click")?),
                xr::Binding::new(&c.menu, p("/user/hand/left/input/menu/click")?),
                xr::Binding::new(
                    &c.left_stick_click,
                    p("/user/hand/left/input/thumbstick/click")?,
                ),
                xr::Binding::new(
                    &c.right_stick_click,
                    p("/user/hand/right/input/thumbstick/click")?,
                ),
                xr::Binding::new(&c.left_trigger, p("/user/hand/left/input/trigger/value")?),
                xr::Binding::new(&c.right_trigger, p("/user/hand/right/input/trigger/value")?),
                xr::Binding::new(&c.left_grip, p("/user/hand/left/input/squeeze/value")?),
                xr::Binding::new(&c.right_grip, p("/user/hand/right/input/squeeze/value")?),
                xr::Binding::new(&c.left_stick, p("/user/hand/left/input/thumbstick")?),
                xr::Binding::new(&c.right_stick, p("/user/hand/right/input/thumbstick")?),
            ],
        )?;
        session.attach_action_sets(&[&c.set])?;
        Ok(c)
    }

    /// Syncs the actions and reads them (the session must be focused for
    /// the runtime to deliver input; otherwise everything reads idle).
    pub(crate) fn read<G: xr::Graphics>(&self, session: &xr::Session<G>) -> xr::Result<PadState> {
        session.sync_actions(&[xr::ActiveActionSet::new(&self.set)])?;
        let mut active = false;
        let mut on = |a: &xr::Action<bool>| -> xr::Result<bool> {
            let s = a.state(session, xr::Path::NULL)?;
            active |= s.is_active;
            Ok(s.is_active && s.current_state)
        };
        let (a, b, x, y, menu) = (
            on(&self.a)?,
            on(&self.b)?,
            on(&self.x)?,
            on(&self.y)?,
            on(&self.menu)?,
        );
        let (left_stick_click, right_stick_click) =
            (on(&self.left_stick_click)?, on(&self.right_stick_click)?);
        let value = |a: &xr::Action<f32>| -> xr::Result<f32> {
            let s = a.state(session, xr::Path::NULL)?;
            Ok(if s.is_active {
                s.current_state.clamp(0.0, 1.0)
            } else {
                0.0
            })
        };
        let stick = |a: &xr::Action<xr::Vector2f>| -> xr::Result<[f32; 2]> {
            let s = a.state(session, xr::Path::NULL)?;
            let v = s.current_state;
            Ok(if s.is_active {
                [v.x.clamp(-1.0, 1.0), v.y.clamp(-1.0, 1.0)]
            } else {
                [0.0; 2]
            })
        };
        Ok(PadState {
            active,
            a,
            b,
            x,
            y,
            menu,
            left_stick_click,
            right_stick_click,
            left_trigger: value(&self.left_trigger)?,
            right_trigger: value(&self.right_trigger)?,
            left_grip: value(&self.left_grip)?,
            right_grip: value(&self.right_grip)?,
            left_stick: stick(&self.left_stick)?,
            right_stick: stick(&self.right_stick)?,
        })
    }
}
