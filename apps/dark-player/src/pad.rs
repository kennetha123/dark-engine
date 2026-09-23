//! Gamepads (docs/PLAN.md §20), through `gilrs`: one more way to fill the same [`TickInput`]
//! the keyboard fills, so the simulation never learns where input came from.
//!
//! Pads are plugged and unplugged while the game runs. A pad plays once a button on it is
//! pressed, and the one pressed last is the one playing: a device nobody has touched — a wheel,
//! a flight stick, a pad resting off-centre — never steers. The keyboard works alongside it.

use dark_world::TickInput;
use gilrs::{Axis, Button, Event, EventType, GamepadId, Gilrs};
use glam::Vec2;

/// Stick movement smaller than this is the stick resting, not the player's wish. Past it the
/// tilt is spread over the whole range again, so the slowest walk is a slow walk.
const DEADZONE: f32 = 0.25;

/// What is held on the pad this frame, and what the frame should do besides filling presses.
#[derive(Clone, Debug, Default)]
pub struct Held {
    pub movement: Vec2,
    pub run: bool,
    /// Start was pressed: opens or closes the menu, as Esc does.
    pub menu: bool,
    /// Hotbar slots (1–8) pressed this frame, in order; they answer a conversation while it
    /// offers choices, as the number keys do.
    pub numbers: Vec<u8>,
}

/// The gamepads, or nothing at all when the library has no way to read them.
pub struct Pads {
    gilrs: Option<Gilrs>,
    /// The pad last pressed: with several plugged in, that is the one playing.
    playing: Option<GamepadId>,
}

impl Pads {
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(gilrs) => {
                for (id, pad) in gilrs.gamepads() {
                    found(id, &pad);
                }
                Some(gilrs)
            }
            Err(err) => {
                tracing::warn!("gamepads cannot be read: {err}");
                None
            }
        };
        Self {
            gilrs,
            playing: None,
        }
    }

    /// Reads what happened since the last frame: presses go into `presses`, beside the
    /// keyboard's, and what is held comes back. Unfocused (`playing` false), the pad is read
    /// but does nothing: it belongs to no window, and the game is not the one being played.
    pub fn poll(&mut self, presses: &mut TickInput, focused: bool) -> Held {
        let Some(gilrs) = &mut self.gilrs else {
            return Held::default();
        };
        let mut held = Held::default();
        // Whether the shoulder above the d-pad is down, kept in step with the events as they
        // are read: a press in this same frame counts, however the two arrived.
        let mut shoulder = self
            .playing
            .and_then(|id| gilrs.connected_gamepad(id))
            .is_some_and(|pad| pad.is_pressed(Button::LeftTrigger));
        while let Some(Event { id, event, .. }) = gilrs.next_event() {
            match event {
                EventType::Connected => {
                    if let Some(pad) = gilrs.connected_gamepad(id) {
                        found(id, &pad);
                    }
                }
                EventType::Disconnected => {
                    tracing::info!("gamepad {id} unplugged");
                    if self.playing == Some(id) {
                        self.playing = None;
                    }
                }
                EventType::ButtonPressed(button, _) if focused => {
                    // Whoever presses something is playing; another pad takes over by being used.
                    self.playing = Some(id);
                    if button == Button::LeftTrigger {
                        shoulder = true;
                    }
                    press(button, presses, &mut held, shoulder);
                }
                EventType::ButtonReleased(Button::LeftTrigger, _) if self.playing == Some(id) => {
                    shoulder = false;
                }
                _ => {}
            }
        }
        let Some(pad) = self
            .playing
            .filter(|_| focused)
            .and_then(|id| gilrs.connected_gamepad(id))
        else {
            return held;
        };
        let stick = Vec2::new(
            pad.value(Axis::LeftStickX),
            // Sticks point up as +1; the world's +y is down the screen.
            -pad.value(Axis::LeftStickY),
        );
        let tilt = stick.length();
        if tilt > DEADZONE {
            held.movement = stick / tilt * ((tilt - DEADZONE) / (1.0 - DEADZONE)).min(1.0);
        }
        // Run on the left trigger, or on the stick pressed in, whichever the player reaches for.
        held.run = pad.is_pressed(Button::LeftTrigger2) || pad.is_pressed(Button::LeftThumb);
        held
    }
}

/// Says what was plugged in. A pad the library has no mapping for reports nothing this
/// understands, so it does nothing at all until a mapping for it exists.
fn found(id: GamepadId, pad: &gilrs::Gamepad) {
    tracing::info!("gamepad {id}: {} ({:?})", pad.name(), pad.mapping_source());
    if matches!(pad.mapping_source(), gilrs::MappingSource::None) {
        tracing::warn!("gamepad {id} is not one this machine knows; it will not play");
    }
}

/// One press: the buttons that act, and the d-pad's hotbar slots (the shoulder above it picks
/// the second four). The d-pad does not walk — it would turn the character on the spot as the
/// slot it chose was used, planting a tent behind them or walking them out of a conversation.
fn press(button: Button, presses: &mut TickInput, held: &mut Held, shoulder: bool) {
    let mut slot = |n: u8| held.numbers.push(if shoulder { n + 4 } else { n });
    match button {
        Button::South => presses.jump = true,
        Button::West => presses.attack = true,
        Button::East => presses.dodge = true,
        Button::North => presses.interact = true,
        Button::RightTrigger => presses.recruit = true,
        Button::RightThumb => presses.relieve = true,
        Button::Select => presses.sleep = true,
        Button::Start => held.menu = !held.menu,
        Button::DPadUp => slot(1),
        Button::DPadRight => slot(2),
        Button::DPadDown => slot(3),
        Button::DPadLeft => slot(4),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed(button: Button, shoulder: bool) -> (TickInput, Held) {
        let (mut presses, mut held) = (TickInput::default(), Held::default());
        press(button, &mut presses, &mut held, shoulder);
        (presses, held)
    }

    #[test]
    fn the_buttons_do_what_the_keys_do() {
        assert!(pressed(Button::South, false).0.jump);
        assert!(pressed(Button::West, false).0.attack);
        assert!(pressed(Button::East, false).0.dodge);
        assert!(pressed(Button::North, false).0.interact);
        assert!(pressed(Button::RightTrigger, false).0.recruit);
        assert!(pressed(Button::Select, false).0.sleep);
        assert!(pressed(Button::RightThumb, false).0.relieve);
        assert!(pressed(Button::Start, false).1.menu);
        // Nothing is pressed by a button the game does not use.
        assert_eq!(pressed(Button::C, false).0, TickInput::default());
        assert_eq!(pressed(Button::Mode, false).1.numbers, Vec::new());
    }

    #[test]
    fn the_d_pad_reaches_all_eight_hotbar_slots_and_keeps_every_press() {
        let slots = |shoulder| {
            [
                Button::DPadUp,
                Button::DPadRight,
                Button::DPadDown,
                Button::DPadLeft,
            ]
            .map(|b| pressed(b, shoulder).1.numbers[0])
        };
        assert_eq!(slots(false), [1, 2, 3, 4]);
        assert_eq!(slots(true), [5, 6, 7, 8]);
        // Two slots in one frame are both used, as two number keys would be.
        let (mut presses, mut held) = (TickInput::default(), Held::default());
        press(Button::DPadUp, &mut presses, &mut held, false);
        press(Button::DPadDown, &mut presses, &mut held, true);
        assert_eq!(held.numbers, vec![1, 7]);
    }

    /// Twice in one frame leaves the menu as it was, as pressing Esc twice does.
    #[test]
    fn the_menu_button_toggles_each_press() {
        let (mut presses, mut held) = (TickInput::default(), Held::default());
        press(Button::Start, &mut presses, &mut held, false);
        assert!(held.menu);
        press(Button::Start, &mut presses, &mut held, false);
        assert!(!held.menu);
    }
}
