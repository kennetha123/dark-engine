//! Character models as 3D meshes (`journals/engine/04`).
//!
//! A `*.model.ron` names what an artist exported, the glTF that `dark-cli bake-model` converts it
//! into, and the measurements the host needs. The host never loads geometry: baking writes each
//! clip's length in ticks and whether it loops, and that is all the simulation reads. Only the
//! game's view loads the mesh.
//!
//! This is the same split `spine.rs` already uses, for the same reason — the simulation says
//! which clip is at which tick, and presentation works out what that looks like. A skinned mesh
//! carries dozens of bones per character, and none of them belong in a networked tick.
//!
//! Unlike a Spine sheet there are no directions. A 2D skeleton needs a separate animation for
//! every way a character can face; a mesh is simply turned to face, so there is one clip per
//! action and the facing is a rotation the view applies.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{AssetError, Project, invalid, read_ron};

/// A `*.model.ron` file.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ModelDef {
    /// What the artist exported, relative to the project: `.fbx` or `.glb`/`.gltf`.
    pub source: String,
    /// The glTF binary `dark-cli bake-model` writes from `source`, which the view loads. FBX is
    /// never read at run time — it is Autodesk's format, and reading it in Rust is a poor bet.
    pub mesh: String,
    /// The measurements `dark-cli bake-model` writes.
    pub baked: String,
    /// Model units to world pixels. A Mixamo figure arrives about two units tall, and a
    /// character in this game stands a few dozen pixels, so this is normally in the tens.
    pub scale: f32,
    /// Engine action (`idle`, `walk`, `run`, `attack`, …) to the animation's name in the file.
    pub actions: BTreeMap<String, String>,
    /// Engine actions that loop.
    #[serde(default = "default_looping")]
    pub looping: Vec<String>,
}

fn default_looping() -> Vec<String> {
    ["idle", "walk", "run"].map(String::from).to_vec()
}

impl ModelDef {
    /// Every engine clip this model plays, with the animation behind it and whether it loops, in
    /// name order.
    pub fn clips(&self) -> Vec<(String, String, bool)> {
        let mut clips: Vec<(String, String, bool)> = self
            .actions
            .iter()
            .map(|(action, animation)| {
                (
                    action.clone(),
                    animation.clone(),
                    self.looping.contains(action),
                )
            })
            .collect();
        clips.sort();
        clips
    }
}

/// What `dark-cli bake-model` measured: all the host needs of a mesh.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelBake {
    /// The source file's contents when baked: an export changed since must be baked again.
    pub source_hash: String,
    /// Height of the model's bounds in world pixels (for what shows over heads).
    pub height: f32,
    /// How many bones the skeleton has, for the view's bone palette to size itself.
    pub bones: u32,
    /// By engine clip name.
    pub clips: BTreeMap<String, BakedModelClip>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BakedModelClip {
    /// The animation in the mesh file.
    pub animation: String,
    /// Length in simulation ticks (60 per second), at least 1.
    pub ticks: u32,
    pub looping: bool,
    /// Events by tick, named as the animator named the pose marker. An attack lands on the tick
    /// of the one called `strike` — unlike a Spine sheet, which renames an event through
    /// `SpineDef::strike_event`, nothing here maps names, so the marker must carry that name
    /// itself.
    #[serde(default)]
    pub events: Vec<(String, u32)>,
}

/// Simulation ticks per second, as `dark_core` runs.
const TICK_RATE: f32 = 60.0;

/// How long a clip lasts in ticks, given its length in seconds.
///
/// This is `dark_spine`'s rule, deliberately: a clip that plays once shows its last key too
/// (frames 0 to the duration), while one that loops comes back round to its first instead. A
/// model and a skeleton of the same real duration have to agree, or an attack would recover on
/// a different tick depending only on how the character was drawn.
pub fn ticks_for(seconds: f32, looping: bool) -> u32 {
    let span = (seconds * TICK_RATE).max(0.0);
    if looping {
        (span.round() as u32).max(1)
    } else {
        span.floor() as u32 + 1
    }
}

/// What `tools/bake_model.py` measures and hands back: times in seconds, because the rule that
/// turns seconds into ticks belongs in one place and that place is Rust.
///
/// Unknown fields are refused. Serde would otherwise read a field the script had renamed as a
/// default and bake a model quietly wrong, and a Python script and a Rust struct that must agree
/// have nothing else holding them together.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelMeasure {
    pub height: f32,
    pub bones: u32,
    /// By engine clip name.
    pub clips: BTreeMap<String, MeasuredClip>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasuredClip {
    pub animation: String,
    /// How long the animation runs.
    pub seconds: f32,
    /// Pose markers, by when they happen.
    #[serde(default)]
    pub events: Vec<(String, f32)>,
}

impl ModelMeasure {
    pub fn read(path: &Path) -> Result<Self, AssetError> {
        read_ron(path)
    }

    /// Turns a measurement into a bake: the engine's arithmetic, and the two things only the
    /// engine knows — which clips loop, and what the source was when it was read.
    pub fn bake(&self, def: &ModelDef, source_hash: String) -> ModelBake {
        let mut clips = BTreeMap::new();
        for (action, animation, looping) in def.clips() {
            let Some(measured) = self.clips.get(&action) else {
                continue;
            };
            let ticks = ticks_for(measured.seconds, looping);
            let mut events: Vec<(String, u32)> = measured
                .events
                .iter()
                // The first tick at or past the mark, with `dark_spine`'s own slack so that a
                // key sitting exactly on a tick does not get pushed to the next one by float
                // error. The two must agree tick for tick; see `ticks_for`.
                .map(|(name, at)| {
                    let tick = ((at * TICK_RATE) - 1e-3).max(0.0).ceil() as u32;
                    (name.clone(), tick.min(ticks - 1))
                })
                .collect();
            events.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
            // Two markers of the same name on the same tick are one event, as in `dark_spine`.
            events.dedup();
            clips.insert(
                action,
                BakedModelClip {
                    animation,
                    ticks,
                    looping,
                    events,
                },
            );
        }
        ModelBake {
            source_hash,
            height: self.height,
            bones: self.bones,
            clips,
        }
    }
}

/// A model as loaded: its definition and its bake. No geometry — the view loads that.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedModel {
    pub def: ModelDef,
    pub bake: ModelBake,
}

impl ModelBake {
    /// Reads a bake back. The bake tool writes this file from Blender, so reading it here is
    /// also the check that the script and this struct still agree on its shape.
    pub fn read(path: &Path) -> Result<Self, AssetError> {
        read_ron(path)
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }
}

/// A source export's contents, folded into a name a bake can be checked against (FNV-1a, 64
/// bits, hex). No dependency, and the same answer on every machine.
///
/// Unlike [`crate::Project::fingerprint`] this eats binary and folds one blob rather than many
/// fields, so it needs no separator between them: every byte is multiplied through, which is why
/// a file that only grew a trailing zero still lands somewhere else.
pub fn source_hash(bytes: &[u8]) -> String {
    let mut all: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in bytes {
        all ^= u64::from(*byte);
        all = all.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("{all:016x}")
}

impl Project {
    /// A `*.model.ron` definition, checked.
    pub fn load_model_def(&self, model: impl AsRef<Path>) -> Result<ModelDef, AssetError> {
        let def_path = self.path(model);
        let def: ModelDef = read_ron(&def_path)?;
        if !(def.scale.is_finite() && def.scale > 0.0) {
            return Err(invalid(&def_path, "scale must be positive".into()));
        }
        if def.actions.is_empty() {
            return Err(invalid(
                &def_path,
                "a model needs at least one action".into(),
            ));
        }
        Ok(def)
    }

    /// A model and its bake. The bake must exist and hold every clip the definition names; that
    /// the source has not changed underneath it is the project smoke test's business, because
    /// reading a whole export to check a hash is not something loading should do.
    pub fn load_model(&self, model: impl AsRef<Path>) -> Result<LoadedModel, AssetError> {
        let def_path = self.path(&model);
        let def = self.load_model_def(&model)?;
        let baked_path = self.path(&def.baked);
        if !baked_path.exists() {
            return Err(invalid(
                &def_path,
                format!(
                    "{} is not baked yet: run `dark-cli bake-model <project> <this model>`",
                    def.baked
                ),
            ));
        }
        let bake: ModelBake = read_ron(&baked_path)?;
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
        if !self.path(&def.mesh).exists() {
            return Err(invalid(
                &def_path,
                format!("{} is missing: bake again", def.mesh),
            ));
        }
        Ok(LoadedModel { def, bake })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> ModelDef {
        ron::from_str(
            r#"(source: "m.fbx", mesh: "m.glb", baked: "m.baked.ron", scale: 20.0,
                actions: {"idle": "Idle", "attack": "Slash"})"#,
        )
        .expect("a model def")
    }

    /// One clip per action, not one per action and facing: a mesh is turned to face.
    #[test]
    fn a_model_has_one_clip_for_each_action() {
        let clips = def().clips();
        assert_eq!(clips.len(), 2);
        assert!(clips.contains(&("idle".into(), "Idle".into(), true)));
        // `attack` is not in the default looping list, so it plays once.
        assert!(clips.contains(&("attack".into(), "Slash".into(), false)));
    }

    /// `tools/bake_model.py` writes this file, so its shape is a contract between a Python
    /// script and this struct, with nothing but a test to hold the two together. This is that
    /// script's output, verbatim, for a clip with an event and one without.
    #[test]
    fn what_the_bake_script_writes_becomes_a_bake() {
        let written = r#"(
    height: 41.241600,
    bones: 69,
    clips: {
        "attack": (
            animation: "Slash",
            seconds: 0.700000,
            events: [("strike", 0.283333)],
        ),
        "idle": (
            animation: "Armature|mixamo.com|Layer0",
            seconds: 2.166667,
        ),
    },
)
"#;
        let measure: ModelMeasure = ron::from_str(written).expect("the script's own output");
        assert_eq!(measure.bones, 69);
        assert!(
            measure.clips["idle"].events.is_empty(),
            "events may be absent"
        );

        let def: ModelDef = ron::from_str(
            r#"(source: "m.fbx", mesh: "m.glb", baked: "m.baked.ron", scale: 20.0,
                actions: {"idle": "Armature|mixamo.com|Layer0", "attack": "Slash"})"#,
        )
        .expect("a model def");
        let bake = measure.bake(&def, "abc".into());
        assert_eq!(bake.source_hash, "abc");
        assert_eq!(bake.bones, 69);
        // `idle` loops by default, so it is round(2.166667 * 60) = 130.
        assert!(bake.clips["idle"].looping);
        assert_eq!(bake.clips["idle"].ticks, 130);
        // `attack` plays once, so it is floor(0.7 * 60) + 1 = 43, and shows its last key.
        assert!(!bake.clips["attack"].looping);
        assert_eq!(bake.clips["attack"].ticks, 43);
        // An event is on the first tick at or past its mark: ceil(0.283333 * 60) = 17.
        assert_eq!(bake.clips["attack"].events, vec![("strike".to_owned(), 17)]);
    }

    /// A model and a Spine skeleton of the same duration must last the same number of ticks, or
    /// an attack would recover on a different tick depending only on how the character is drawn.
    /// This is `dark_spine`'s rule, written out again so that a change to either one fails here.
    #[test]
    fn a_clip_lasts_as_long_as_the_same_spine_clip_would() {
        for seconds in [0.0f32, 0.016_667, 0.5, 0.7, 1.0, 2.166_667, 9.99] {
            let span = seconds * 60.0;
            assert_eq!(ticks_for(seconds, true), (span.round() as u32).max(1));
            assert_eq!(ticks_for(seconds, false), span.floor() as u32 + 1);
        }
        // A clip that plays once shows its last key, so it is never shorter than a looping one.
        assert!(ticks_for(0.5, false) > ticks_for(0.5, true));
        assert_eq!(ticks_for(0.0, true), 1, "never no ticks at all");
        assert_eq!(ticks_for(-1.0, true), 1, "nor for a nonsense duration");
    }

    /// An event past the end of its clip would index off a track the view is walking.
    #[test]
    fn an_event_never_lands_past_the_end_of_its_clip() {
        let measure = ModelMeasure {
            height: 1.0,
            bones: 1,
            clips: [(
                "attack".to_owned(),
                MeasuredClip {
                    animation: "Slash".into(),
                    seconds: 0.5,
                    events: vec![("late".into(), 99.0), ("early".into(), -3.0)],
                },
            )]
            .into_iter()
            .collect(),
        };
        let def: ModelDef = ron::from_str(
            r#"(source: "m.fbx", mesh: "m.glb", baked: "m.baked.ron", scale: 1.0,
                actions: {"attack": "Slash"})"#,
        )
        .expect("a model def");
        let bake = measure.bake(&def, String::new());
        let clip = &bake.clips["attack"];
        assert_eq!(clip.ticks, 31);
        assert_eq!(clip.events[0], ("early".to_owned(), 0));
        assert_eq!(clip.events[1], ("late".to_owned(), clip.ticks - 1));
    }

    /// A re-export must force a re-bake, so the hash has to move when the bytes do — including
    /// when the only change is a byte appended at the end, which is where a careless fold would
    /// let two exports meet.
    #[test]
    fn the_source_hash_follows_the_bytes_and_their_length() {
        let a = source_hash(b"skeleton");
        assert_eq!(a, source_hash(b"skeleton"), "the same bytes, the same name");
        assert_ne!(a, source_hash(b"skeletoN"));
        assert_ne!(source_hash(b"ab\0"), source_hash(b"ab\0\0"));
        assert_ne!(source_hash(b""), source_hash(b"\0"));
        assert_eq!(a.len(), 16, "sixteen hex digits");
    }
}
