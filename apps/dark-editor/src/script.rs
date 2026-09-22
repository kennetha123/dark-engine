//! Scripted input for checking the editor without a person at the screen (`--script`): clicks,
//! drags, keys and typing, in window points, one step after another, as if a person did them.
//!
//! `click 700,400; drag 600,300 700,350; rdrag …; key ctrl+s; key delete; type Hello; wait 10`

use std::collections::VecDeque;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};

/// Input for the frames to come, one entry per frame.
#[derive(Default)]
pub struct Script {
    frames: VecDeque<Vec<Event>>,
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut script = Self::default();
        for step in text.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (verb, rest) = step.split_once(' ').unwrap_or((step, ""));
            let rest = rest.trim();
            match verb {
                "click" | "rclick" => {
                    let at = point(rest)?;
                    let button = if verb == "click" {
                        PointerButton::Primary
                    } else {
                        PointerButton::Secondary
                    };
                    script.push(vec![Event::PointerMoved(at)]);
                    script.push(vec![press(at, button, true)]);
                    script.push(vec![press(at, button, false)]);
                    script.wait(2);
                }
                "drag" | "rdrag" => {
                    let (a, b) = rest.split_once(' ').ok_or(format!("{step}: two points"))?;
                    let (a, b) = (point(a)?, point(b.trim())?);
                    let button = if verb == "drag" {
                        PointerButton::Primary
                    } else {
                        PointerButton::Secondary
                    };
                    script.push(vec![Event::PointerMoved(a)]);
                    script.push(vec![press(a, button, true)]);
                    const STEPS: usize = 12;
                    for i in 1..=STEPS {
                        let t = i as f32 / STEPS as f32;
                        script.push(vec![Event::PointerMoved(a + (b - a) * t)]);
                    }
                    script.push(vec![press(b, button, false)]);
                    script.wait(2);
                }
                "key" => {
                    let (modifiers, key) = chord(rest)?;
                    let event = |pressed| Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    };
                    script.push(vec![event(true)]);
                    script.push(vec![event(false)]);
                    script.wait(2);
                }
                "type" => {
                    script.push(vec![Event::Text(rest.to_owned())]);
                    script.wait(1);
                }
                "wait" => {
                    let n = rest.parse().map_err(|_| format!("{step}: a number"))?;
                    script.wait(n);
                }
                _ => return Err(format!("unknown step `{step}`")),
            }
        }
        Ok(script)
    }

    fn push(&mut self, events: Vec<Event>) {
        self.frames.push_back(events);
    }

    fn wait(&mut self, frames: usize) {
        for _ in 0..frames {
            self.push(Vec::new());
        }
    }

    /// This frame's input, if the script is still running.
    pub fn next(&mut self) -> Option<Vec<Event>> {
        self.frames.pop_front()
    }

    pub fn is_done(&self) -> bool {
        self.frames.is_empty()
    }
}

fn press(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn point(text: &str) -> Result<Pos2, String> {
    let (x, y) = text.split_once(',').ok_or(format!("`{text}`: x,y"))?;
    let n = |v: &str| {
        v.trim()
            .parse::<f32>()
            .map_err(|_| format!("`{text}`: x,y"))
    };
    Ok(pos2(n(x)?, n(y)?))
}

fn chord(text: &str) -> Result<(Modifiers, Key), String> {
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for part in text.split('+') {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" => modifiers |= Modifiers::COMMAND,
            "shift" => modifiers |= Modifiers::SHIFT,
            name => {
                let named = match name {
                    "delete" => Some(Key::Delete),
                    "escape" => Some(Key::Escape),
                    "enter" => Some(Key::Enter),
                    _ => Key::from_name(&name.to_ascii_uppercase()),
                };
                key = Some(named.ok_or(format!("unknown key `{part}`"))?);
            }
        }
    }
    Ok((modifiers, key.ok_or(format!("`{text}`: no key"))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_become_frames_of_input() {
        let mut s = Script::parse("click 10,20; key ctrl+z; drag 0,0 12,12").unwrap();
        assert!(matches!(s.next().unwrap()[0], Event::PointerMoved(p) if p == pos2(10.0, 20.0)));
        assert!(matches!(
            s.next().unwrap()[0],
            Event::PointerButton { pressed: true, .. }
        ));
        let mut frames = 2;
        while s.next().is_some() {
            frames += 1;
        }
        assert_eq!(frames, 5 + 4 + 17, "click, key, drag");
        assert!(s.is_done());
        assert!(Script::parse("jump 1,2").is_err());
        assert!(Script::parse("key ctrl+nothing").is_err());
    }
}
