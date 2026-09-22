//! Spine skeletons as character sheets (docs/PLAN.md §16).
//!
//! A `*.spine.ron` sheet names a Spine export (skeleton JSON, atlas) and how its animations map
//! onto the engine's `<action>_<facing>` clips. The host never runs Spine: `dark-cli
//! bake-spine` evaluates the skeleton once and writes a baked file (each clip's length in ticks,
//! its events, and any `hitbox`/`hurtbox` bounding boxes per tick), and loading the sheet builds
//! plain clips from that, one frame per tick. Only the game's view poses the real skeleton.

use std::collections::BTreeMap;
use std::path::Path;

use dark_sprite::{Circle, Clip, ClipTiming, Facing, Frame, Rect, SpriteSheet};
use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::{AssetError, Image, LoadedSheet, Project, invalid, read_ron};

/// A `*.spine.ron` file.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SpineDef {
    /// The skeleton's JSON export (Spine 4.1), relative to the project.
    pub skeleton: String,
    /// Its atlas (`.atlas` or `.atlas.txt`); pages are found beside it.
    pub atlas: String,
    /// The baked file `dark-cli bake-spine` writes.
    pub baked: String,
    /// Spine units to world pixels.
    pub scale: f32,
    /// How much smaller the atlas pages are drawn than their pixels (skeleton scale times any
    /// bone scale over the attachments); the view shrinks them to that once. Defaults to `scale`.
    #[serde(default)]
    pub texture_scale: Option<f32>,
    /// How an animation is named: `{dir}` and `{action}` are replaced.
    #[serde(default = "default_pattern")]
    pub pattern: String,
    /// Engine action (`idle`, `walk`, `run`, `attack`, `hurt`, …) to the Spine action name.
    pub actions: BTreeMap<String, String>,
    /// Engine facing name (`down`, `down_left`, …) to the Spine direction name; defaults to
    /// compass points (`S`, `SW`, `W`, `NW`, `N`, `NE`, `E`, `SE`).
    #[serde(default = "compass")]
    pub directions: BTreeMap<String, String>,
    /// Engine actions that loop.
    #[serde(default = "default_looping")]
    pub looping: Vec<String>,
    /// The Spine event marking when an attack lands; baked as `strike`.
    #[serde(default)]
    pub strike_event: Option<String>,
}

fn default_pattern() -> String {
    "{dir}_{action}".into()
}

fn default_looping() -> Vec<String> {
    ["idle", "walk", "run"].map(String::from).to_vec()
}

fn compass() -> BTreeMap<String, String> {
    [
        (Facing::Down, "S"),
        (Facing::DownLeft, "SW"),
        (Facing::Left, "W"),
        (Facing::UpLeft, "NW"),
        (Facing::Up, "N"),
        (Facing::UpRight, "NE"),
        (Facing::Right, "E"),
        (Facing::DownRight, "SE"),
    ]
    .into_iter()
    .map(|(f, d)| (f.name().to_owned(), d.to_owned()))
    .collect()
}

impl SpineDef {
    /// Every engine clip this skeleton plays, with the Spine animation behind it and whether it
    /// loops, in name order.
    pub fn clips(&self) -> Vec<(String, String, bool)> {
        let mut clips = Vec::new();
        for (action, spine_action) in &self.actions {
            for (dir, spine_dir) in &self.directions {
                let animation = self
                    .pattern
                    .replace("{dir}", spine_dir)
                    .replace("{action}", spine_action);
                let looping = self.looping.contains(action);
                clips.push((format!("{action}_{dir}"), animation, looping));
            }
        }
        clips.sort();
        clips
    }
}

/// What `dark-cli bake-spine` measured: all the host needs of a skeleton.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpineBake {
    /// The skeleton's `hash` when baked: a changed export must be baked again.
    pub skeleton_hash: String,
    /// Height of the skeleton's bounds in world pixels (for what shows over heads).
    pub height: f32,
    /// By engine clip name.
    pub clips: BTreeMap<String, BakedClip>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BakedClip {
    /// The Spine animation.
    pub animation: String,
    /// Length in simulation ticks (60 per second), at least 1.
    pub ticks: u32,
    pub looping: bool,
    /// Events by tick; the strike event is named `strike`.
    #[serde(default)]
    pub events: Vec<(String, u32)>,
    /// Per tick, where a `hitbox` bounding box is (ground plane, feet-relative); empty if the
    /// animation has none.
    #[serde(default)]
    pub hitboxes: Vec<Option<(f32, f32, f32)>>,
    #[serde(default)]
    pub hurtboxes: Vec<Option<(f32, f32, f32)>>,
}

/// A Spine sheet as loaded: its definition and bake, beside the plain clips built from them.
#[derive(Clone, Debug, PartialEq)]
pub struct SpineSheet {
    pub def: SpineDef,
    pub bake: SpineBake,
}

impl SpineBake {
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    /// Plain clips, one frame per tick, carrying the baked events and boxes.
    pub fn sheet(&self) -> SpriteSheet {
        let mut sheet = SpriteSheet::default();
        let circles = |boxes: &[Option<(f32, f32, f32)>]| {
            boxes
                .iter()
                .map(|b| {
                    b.map(|(x, y, radius)| Circle {
                        offset: Vec2::new(x, y),
                        radius,
                    })
                })
                .collect()
        };
        for (name, clip) in &self.clips {
            let first = sheet.frames.len() as u32;
            let ticks = clip.ticks.max(1);
            sheet.frames.extend((0..ticks).map(|_| Frame {
                rect: Rect::new(0, 0, 1, 1),
                pivot: Vec2::ZERO,
            }));
            sheet.clips.push(Clip {
                name: name.clone(),
                frames: (first..first + ticks).collect(),
                ticks_per_frame: 1,
                looping: clip.looping,
                flip_x: false,
            });
            sheet.timing.push(ClipTiming {
                events: clip.events.clone(),
                hitboxes: circles(&clip.hitboxes),
                hurtboxes: circles(&clip.hurtboxes),
            });
        }
        sheet
    }
}

impl Project {
    /// A `*.spine.ron` sheet's definition, checked.
    pub fn load_spine_def(&self, sheet: impl AsRef<Path>) -> Result<SpineDef, AssetError> {
        let def_path = self.path(sheet);
        let def: SpineDef = read_ron(&def_path)?;
        let positive = |v: f32| v.is_finite() && v > 0.0;
        if !positive(def.scale) || !def.texture_scale.is_none_or(positive) {
            return Err(invalid(
                &def_path,
                "scale and texture_scale must be positive".into(),
            ));
        }
        Ok(def)
    }

    /// A `*.spine.ron` sheet: its baked clips, and no pixels (the view loads the skeleton).
    pub(crate) fn load_spine_sheet(&self, def_path: &Path) -> Result<LoadedSheet, AssetError> {
        let def = self.load_spine_def(def_path)?;
        let baked_path = self.path(&def.baked);
        if !baked_path.exists() {
            return Err(invalid(
                def_path,
                format!(
                    "{} is not baked yet: run `dark-cli bake-spine <project> <this sheet>`",
                    def.baked
                ),
            ));
        }
        let bake: SpineBake = read_ron(&baked_path)?;
        let missing: Vec<String> = def
            .clips()
            .into_iter()
            .map(|(name, ..)| name)
            .filter(|name| !bake.clips.contains_key(name))
            .collect();
        if !missing.is_empty() {
            return Err(invalid(
                &baked_path,
                format!("bake again: no {}", missing.join(", ")),
            ));
        }
        Ok(LoadedSheet {
            image_path: self.path(&def.skeleton),
            image: Image {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            },
            sheet: bake.sheet(),
            spine: Some(SpineSheet { def, bake }),
        })
    }
}

/// The `hash` in a skeleton JSON export, read without parsing the whole file.
pub fn skeleton_hash(json: &str) -> Option<String> {
    let at = json.find("\"hash\"")?;
    let rest = &json[at + 6..];
    let start = rest.find('"')? + 1;
    let end = start + rest[start..].find('"')?;
    Some(rest[start..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_are_named_by_engine_action_and_facing_from_the_pattern() {
        let def: SpineDef = ron::from_str(
            r#"(skeleton: "s.json", atlas: "s.atlas", baked: "s.baked.ron", scale: 0.5,
                pattern: "{action}/{dir}_{action}",
                actions: {"idle": "idle", "attack": "atk"})"#,
        )
        .unwrap();
        let clips = def.clips();
        assert_eq!(clips.len(), 16);
        assert!(clips.contains(&("idle_down".into(), "idle/S_idle".into(), true)));
        assert!(clips.contains(&("attack_up_left".into(), "atk/NW_atk".into(), false)));
    }

    #[test]
    fn a_bake_becomes_one_frame_per_tick_with_its_events_and_boxes() {
        let bake = SpineBake {
            skeleton_hash: "h".into(),
            height: 40.0,
            clips: [(
                "attack_down".to_owned(),
                BakedClip {
                    animation: "S_atk".into(),
                    ticks: 3,
                    looping: false,
                    events: vec![("strike".into(), 2)],
                    hitboxes: vec![None, None, Some((0.0, 10.0, 6.0))],
                    hurtboxes: Vec::new(),
                },
            )]
            .into(),
        };
        let sheet = bake.sheet();
        let id = sheet.clip_id("attack_down").unwrap();
        assert_eq!(sheet.clips[0].frames, vec![0, 1, 2]);
        let timing = sheet.timing(id).unwrap();
        assert_eq!(timing.event("strike"), Some(2));
        assert_eq!(timing.hitboxes[2].unwrap().offset, Vec2::new(0.0, 10.0));
        assert!(timing.hurtboxes.is_empty());
    }

    #[test]
    fn the_hash_is_read_from_the_skeleton_header() {
        let json = r#"{"skeleton":{"hash":"PJNQGoo3GKU","spine":"4.1.24"},"bones":[]}"#;
        assert_eq!(skeleton_hash(json).as_deref(), Some("PJNQGoo3GKU"));
    }
}
