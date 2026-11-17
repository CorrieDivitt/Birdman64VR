//! Birdman64VR (q3): the Touch controllers as a modern (Xbox / PlayStation
//! style) gamepad. The headset thread reads them once per headset frame
//! (`pw64_xr::XrContext::pad`) and hands the state here; it becomes the
//! gilrs [`Button`]s at the same place on an Xbox pad, so the
//! `[input.gamepad]` bindings, the settings trigger, R3 recentre, pad capture
//! (rebinding) and the overlay navigation behave as for a real pad. The
//! result is its own source, merged into player 1 (VR is single-player), so
//! a Bluetooth pad keeps working alongside.
//!
//! | Touch                    | Xbox pad (gilrs)        | N64 by default  |
//! |--------------------------|-------------------------|-----------------|
//! | A, B (right)             | A, B (South, East)      | A, B            |
//! | X, Y (left)              | X, Y (West, North)      | B, C-up         |
//! | Left / right stick       | Left / right stick      | Stick / C       |
//! | Left / right trigger     | LT, RT                  | Z, R            |
//! | Left / right grip        | LB, RB                  | L, R            |
//! | Menu (left)              | Menu (Start)            | Start           |
//! | Right stick press        | R3                      | Recentre (VR)   |
//! | Left stick press         | View (Select)           | Settings        |
//!
//! The Touch controllers have one menu button (the right controller's is
//! the system's), so the left stick press stands in for the View button
//! that opens the settings; the d-pad has no Touch counterpart (by default
//! it only doubles the stick).

use super::{
    CAPTURED_BUTTON, CONT_A, CONT_B, CONT_START, FOCUSED, IDLE, PAD_CAPTURE, Pad, SOURCES,
    TRIGGER_THRESHOLD, c_stick_buttons, gamepad_bindings, publish, slot_bits_pad, stick_to_n64,
};
use gilrs::Button;
use pw64_xr::PadState;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

/// A grip counts as held past this (the grips are analog, the bumpers they
/// stand in for are not).
const GRIP_THRESHOLD: f32 = 0.5;

/// The last state handed in and the buttons it held (edge detection).
static LAST: Mutex<Option<(PadState, Vec<Button>)>> = Mutex::new(None);

/// The Xbox pad buttons the Touch state holds (the analog triggers count
/// past [`TRIGGER_THRESHOLD`], as a pad's do).
pub(crate) fn held(p: &PadState) -> Vec<Button> {
    let mut held = Vec::new();
    for (on, b) in [
        (p.a, Button::South),
        (p.b, Button::East),
        (p.x, Button::West),
        (p.y, Button::North),
        (p.menu, Button::Start),
        (p.left_stick_click, Button::Select),
        (p.right_stick_click, Button::RightThumb),
        (p.left_grip >= GRIP_THRESHOLD, Button::LeftTrigger),
        (p.right_grip >= GRIP_THRESHOLD, Button::RightTrigger),
        (p.left_trigger >= TRIGGER_THRESHOLD, Button::LeftTrigger2),
        (p.right_trigger >= TRIGGER_THRESHOLD, Button::RightTrigger2),
    ] {
        if on {
            held.push(b);
        }
    }
    held
}

/// The N64 pad from the resolved gamepad bindings (as `gamepad_pad` does
/// for a gilrs pad): sticks y up, right stick → C buttons.
pub(crate) fn game_pad(p: &PadState, held: &[Button]) -> Pad {
    let mut bits = 0u32;
    for binding in gamepad_bindings().iter() {
        if binding.inputs.iter().any(|b| held.contains(b)) {
            bits |= binding.bit;
        }
    }
    let (stick_x, stick_y) = stick_to_n64(p.left_stick[0], p.left_stick[1]);
    slot_bits_pad(bits).merge(Pad {
        button: c_stick_buttons(p.right_stick[0], p.right_stick[1]),
        stick_x,
        stick_y,
    })
}

/// The overlay's navigation pad (fixed, as `gamepad_nav_pad`): A, B, Start
/// and the left stick.
pub(crate) fn nav_pad(p: &PadState, held: &[Button]) -> Pad {
    let mut button = 0;
    for (b, bit) in [
        (Button::South, CONT_A),
        (Button::East, CONT_B),
        (Button::Start, CONT_START),
    ] {
        if held.contains(&b) {
            button |= bit;
        }
    }
    let (stick_x, stick_y) = stick_to_n64(p.left_stick[0], p.left_stick[1]);
    Pad {
        button,
        stick_x,
        stick_y,
    }
}

/// The Touch controllers' state (headset thread, once per headset frame).
/// Publishes only when it changed.
pub fn set_xr_pad(p: PadState) {
    let settings_btn = crate::settings::pad_button();
    let now = held(&p);
    let before = {
        let mut last = LAST.lock().unwrap();
        if last.as_ref().is_some_and(|(s, _)| *s == p) {
            return;
        }
        last.replace((p, now.clone()))
            .map(|(_, b)| b)
            .unwrap_or_default()
    };
    let pressed: Vec<Button> = now
        .iter()
        .copied()
        .filter(|b| !before.contains(b))
        .collect();
    // Pad capture (the settings screen's rebinding): the first new press is
    // stored for the overlay (the game is paused under it).
    if PAD_CAPTURE.load(Ordering::Relaxed)
        && let Some(&b) = pressed.iter().find(|&&b| b != settings_btn)
    {
        *CAPTURED_BUTTON.lock().unwrap() = Some(b);
        PAD_CAPTURE.store(false, Ordering::Relaxed);
    }
    if pressed.contains(&Button::RightThumb) {
        crate::vr::request_recentre();
    }
    if FOCUSED.load(Ordering::Relaxed) && pressed.contains(&settings_btn) {
        crate::window::notify_settings();
    }
    let (game, nav) = if p.active {
        (game_pad(&p, &now), nav_pad(&p, &now))
    } else {
        (IDLE, IDLE)
    };
    let mut s = SOURCES.lock().unwrap();
    s.xr = game;
    s.xr_nav = nav;
    publish(&mut s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{CONT_C_UP, CONT_L, CONT_R, CONT_Z};

    fn touch() -> PadState {
        PadState {
            active: true,
            ..PadState::default()
        }
    }

    /// The default layout: each Touch input lands where the same input of
    /// an Xbox pad does (input.rs `GAMEPAD_SLOTS`).
    #[test]
    fn touch_buttons_map_like_an_xbox_pad() {
        let p = PadState {
            a: true,
            y: true,
            menu: true,
            left_trigger: 1.0,
            left_grip: 1.0,
            right_grip: 0.2,
            ..touch()
        };
        let down = held(&p);
        assert_eq!(
            down,
            [
                Button::South,
                Button::North,
                Button::Start,
                Button::LeftTrigger,
                Button::LeftTrigger2
            ]
        );
        let pad = game_pad(&p, &down);
        assert_eq!(
            pad.button,
            CONT_A | CONT_C_UP | CONT_START | CONT_L | CONT_Z
        );
        let r = PadState {
            right_trigger: 0.9,
            x: true,
            ..touch()
        };
        assert_eq!(game_pad(&r, &held(&r)).button, CONT_R | CONT_B);
    }

    /// The left stick is the N64 stick (y up), the right one the C buttons.
    #[test]
    fn sticks_drive_the_stick_and_the_c_buttons() {
        let p = PadState {
            left_stick: [0.0, 1.0],
            right_stick: [0.0, 1.0],
            ..touch()
        };
        let pad = game_pad(&p, &[]);
        assert!(pad.stick_y > 60, "stick up: {}", pad.stick_y);
        assert_eq!(pad.stick_x, 0);
        assert_eq!(pad.button & CONT_C_UP, CONT_C_UP);
    }

    /// The stick presses are R3 (recentre) and View (the settings).
    #[test]
    fn stick_presses_are_r3_and_view() {
        let p = PadState {
            left_stick_click: true,
            right_stick_click: true,
            ..touch()
        };
        assert_eq!(held(&p), [Button::Select, Button::RightThumb]);
    }
}
