//! Posing a model: a clip and a time in, a bone palette out.
//!
//! The simulation decides which clip is at which tick; this turns that into matrices. Nothing
//! here is simulation state, and nothing here may become any — see the crate docs.

use glam::{Mat4, Vec3};

use crate::Model;

/// A model posed at a moment: one matrix per bone, ready for a shader's bone palette.
#[derive(Clone, Debug, Default)]
pub struct Pose {
    /// Each bone's transform in model space, before the bind pose is undone.
    world: Vec<Mat4>,
    /// What a vertex is actually multiplied by: `world × inverse_bind`.
    palette: Vec<Mat4>,
}

impl Pose {
    pub fn new(model: &Model) -> Self {
        let bones = model.skeleton.bones.len();
        Self {
            world: vec![Mat4::IDENTITY; bones],
            palette: vec![Mat4::IDENTITY; bones],
        }
    }

    /// One matrix per bone, in skeleton order.
    pub fn palette(&self) -> &[Mat4] {
        &self.palette
    }

    /// Where a bone sits in model space, posed.
    pub fn bone(&self, bone: usize) -> Option<Mat4> {
        self.world.get(bone).copied()
    }

    /// Poses `model` at `animation` and `seconds`. An animation that is not there leaves the
    /// model in its rest pose, which is wrong but visible — better than an empty screen while
    /// somebody works out why.
    pub fn pose(&mut self, model: &Model, animation: &str, looping: bool, seconds: f32) {
        let clip = model.animations.get(animation);
        if clip.is_none() && !animation.is_empty() {
            tracing::warn!("no animation {animation}");
        }
        let at = match clip {
            Some(clip) if looping && clip.seconds > 0.0 => seconds.rem_euclid(clip.seconds),
            Some(clip) => seconds.clamp(0.0, clip.seconds),
            None => 0.0,
        };

        self.world
            .resize(model.skeleton.bones.len(), Mat4::IDENTITY);
        self.palette
            .resize(model.skeleton.bones.len(), Mat4::IDENTITY);

        for (index, bone) in model.skeleton.bones.iter().enumerate() {
            let (mut t, mut r, mut s) = bone.rest;
            if let Some(track) = clip.and_then(|c| c.tracks.get(&index)) {
                // A held track keeps its key until the next one, rather than blending through.
                let hold = track.hold;
                let mix = move |a, b, f| if hold { a } else { Vec3::lerp(a, b, f) };
                let turn = move |a: glam::Quat, b, f| if hold { a } else { a.slerp(b, f) };
                t = sample(&track.translation, at, mix).unwrap_or(t);
                r = sample(&track.rotation, at, turn).unwrap_or(r);
                s = sample(&track.scale, at, mix).unwrap_or(s);
            }
            let local = Mat4::from_scale_rotation_translation(s, r, t);
            // Parents always precede their children, so the parent is already done.
            self.world[index] = match bone.parent {
                Some(parent) => self.world[parent] * local,
                None => local,
            };
            // The whole joint matrix glTF asks for; see `Model::skin_root`.
            self.palette[index] = model.skin_root * self.world[index] * bone.inverse_bind;
        }
    }
}

/// The value of a keyframe track at `at`, blended between the two keys around it.
///
/// glTF keys are in ascending time, so this walks to the first key past `at`. Tracks are short —
/// a few dozen keys — and this runs once per bone per frame, so a scan is cheaper than the
/// bookkeeping a binary search would need to beat it.
fn sample<T: Copy>(keys: &[(f32, T)], at: f32, blend: impl Fn(T, T, f32) -> T) -> Option<T> {
    match keys {
        [] => None,
        [(_, only)] => Some(*only),
        _ => {
            let first = keys[0];
            if at <= first.0 {
                return Some(first.1);
            }
            let last = keys[keys.len() - 1];
            if at >= last.0 {
                return Some(last.1);
            }
            let next = keys.iter().position(|(time, _)| *time > at)?;
            let (t0, v0) = keys[next - 1];
            let (t1, v1) = keys[next];
            let span = t1 - t0;
            // Two keys at the same time would divide by zero; take the later one.
            let f = if span > 1e-9 { (at - t0) / span } else { 1.0 };
            Some(blend(v0, v1, f))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bone, Skeleton, Track};
    use glam::Quat;
    use std::collections::HashMap;

    fn model(bones: Vec<Bone>) -> Model {
        Model {
            skeleton: Skeleton { bones },
            scale: 1.0,
            ..Default::default()
        }
    }

    fn bone(name: &str, parent: Option<usize>, rest: Vec3) -> Bone {
        Bone {
            name: name.to_owned(),
            parent,
            rest: (rest, Quat::IDENTITY, Vec3::ONE),
            inverse_bind: Mat4::IDENTITY,
        }
    }

    /// A child's transform is its parent's with its own on top. The hip is **turned**, so that
    /// composing the two the wrong way round moves the knee somewhere else: two translations
    /// commute and would not tell the difference.
    #[test]
    fn a_child_bone_is_carried_and_turned_by_its_parent() {
        let mut hip = bone("hip", None, Vec3::new(0.0, 0.0, 10.0));
        // A quarter turn about X takes the child's -Z offset onto +Y.
        hip.rest.1 = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let model = model(vec![hip, bone("knee", Some(0), Vec3::new(0.0, 0.0, -4.0))]);
        let mut pose = Pose::new(&model);
        pose.pose(&model, "", false, 0.0);

        let knee = pose.bone(1).expect("the knee").transform_point3(Vec3::ZERO);
        let want = Vec3::new(0.0, 4.0, 10.0);
        assert!(
            knee.abs_diff_eq(want, 1e-5),
            "the knee is carried by the hip and turned with it: {knee} is not {want}"
        );
    }

    /// `palette` is what a shader is actually handed: the bone's world transform with the bind
    /// pose undone. Every other test here leaves `inverse_bind` at identity, which cannot tell
    /// `world * inverse_bind` from `inverse_bind * world`. This one can.
    #[test]
    fn the_palette_undoes_the_bind_pose_in_the_right_order() {
        let mut root = bone("root", None, Vec3::new(0.0, 0.0, 5.0));
        // The bone is **turned** as well as moved. Two translations commute, so a bind pose made
        // only of them cannot tell the two orders apart — the trap this test exists to avoid.
        root.rest.1 = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        root.inverse_bind = Mat4::from_translation(Vec3::new(-2.0, 0.0, 0.0));
        let model = model(vec![root]);
        let mut pose = Pose::new(&model);
        pose.pose(&model, "", false, 0.0);

        let palette = pose.palette()[0];
        // world = T(0,0,5)·Rz(90). A vertex bound at (2,0,0) sits on the bone's origin once the
        // bind pose is undone, so `world * inverse_bind` puts it at (0,0,5). The other order
        // turns it first and leaves it at (-2,2,5).
        let at = palette.transform_point3(Vec3::new(2.0, 0.0, 0.0));
        assert!(
            at.abs_diff_eq(Vec3::new(0.0, 0.0, 5.0), 1e-5),
            "the bind pose is undone before the bone moves the vertex, not after: got {at}"
        );
    }

    /// Between two keys the value is blended; outside them it is held, not extrapolated.
    #[test]
    fn a_track_blends_between_its_keys_and_holds_at_the_ends() {
        let keys = [(1.0, Vec3::ZERO), (3.0, Vec3::new(0.0, 0.0, 8.0))];
        let at = |t| sample(&keys, t, Vec3::lerp).expect("a value");
        assert_eq!(at(0.0), Vec3::ZERO, "before the first key");
        assert_eq!(at(1.0), Vec3::ZERO);
        assert_eq!(at(2.0), Vec3::new(0.0, 0.0, 4.0), "halfway");
        assert_eq!(at(3.0), Vec3::new(0.0, 0.0, 8.0));
        assert_eq!(at(99.0), Vec3::new(0.0, 0.0, 8.0), "after the last key");
        // One key is a constant, and no keys is nothing to say.
        assert_eq!(sample(&[(5.0, 7.0f32)], 0.0, |a, _, _| a), Some(7.0));
        assert_eq!(sample::<f32>(&[], 0.0, |a, _, _| a), None);
    }

    /// A looping clip wraps; a one-shot clip stops on its last frame rather than restarting.
    #[test]
    fn looping_wraps_and_playing_once_holds_the_end() {
        let mut clip = crate::Animation {
            seconds: 2.0,
            tracks: HashMap::new(),
        };
        clip.tracks.insert(
            0,
            Track {
                translation: vec![(0.0, Vec3::ZERO), (2.0, Vec3::new(0.0, 0.0, 20.0))],
                ..Default::default()
            },
        );
        let mut model = model(vec![bone("root", None, Vec3::ZERO)]);
        model.animations.insert("walk".into(), clip);

        let mut pose = Pose::new(&model);
        pose.pose(&model, "walk", true, 2.5);
        let looped = pose.bone(0).expect("root").transform_point3(Vec3::ZERO);
        pose.pose(&model, "walk", true, 0.5);
        assert_eq!(
            looped,
            pose.bone(0).expect("root").transform_point3(Vec3::ZERO)
        );

        pose.pose(&model, "walk", false, 99.0);
        assert_eq!(
            pose.bone(0).expect("root").transform_point3(Vec3::ZERO),
            Vec3::new(0.0, 0.0, 20.0),
            "a clip that plays once holds its last pose"
        );
    }

    /// A held track keeps its key until the next one. Blender writes STEP for anything an
    /// animator snapped, and this very model's export carries both LINEAR and STEP tracks, so
    /// blending through them would smooth motion that was meant to jump.
    #[test]
    fn a_held_track_does_not_blend_between_its_keys() {
        let keys = vec![(0.0, Vec3::ZERO), (2.0, Vec3::new(0.0, 0.0, 20.0))];
        let mut model = model(vec![bone("root", None, Vec3::ZERO)]);
        let mut tracks = HashMap::new();
        tracks.insert(
            0,
            Track {
                translation: keys.clone(),
                hold: true,
                ..Default::default()
            },
        );
        model.animations.insert(
            "snap".into(),
            crate::Animation {
                seconds: 2.0,
                tracks,
            },
        );

        let mut pose = Pose::new(&model);
        pose.pose(&model, "snap", false, 1.0);
        let at = pose.bone(0).expect("root").transform_point3(Vec3::ZERO);
        assert_eq!(
            at,
            Vec3::ZERO,
            "halfway through a held track is still the first key, not the blend"
        );
        // And it does move once the next key is reached.
        pose.pose(&model, "snap", false, 2.0);
        assert_eq!(
            pose.bone(0).expect("root").transform_point3(Vec3::ZERO),
            Vec3::new(0.0, 0.0, 20.0)
        );
    }

    /// An animation nobody has is the rest pose, not a crash and not an empty screen.
    #[test]
    fn an_unknown_animation_leaves_the_model_at_rest() {
        let model = model(vec![bone("root", None, Vec3::new(1.0, 2.0, 3.0))]);
        let mut pose = Pose::new(&model);
        pose.pose(&model, "nothing_called_this", false, 1.0);
        assert_eq!(
            pose.bone(0).expect("root").transform_point3(Vec3::ZERO),
            Vec3::new(1.0, 2.0, 3.0)
        );
    }
}
