//! 3D character models: the geometry behind a `*.model.ron` (`journals/engine/04`, phase 2).
//!
//! This is presentation. The simulation never comes here — it reads a model's **bake**
//! (`dark_assets::ModelBake`: clip names, lengths in ticks, whether they loop) and says which
//! clip is at which tick. This crate turns that into a pose. A skinned character carries dozens
//! of bone matrices and none of them belong in a networked tick, which is the same division
//! `dark_spine` draws for 2D skeletons.
//!
//! Only glTF is read. The artist's FBX is converted by `dark-cli bake-model` long before the
//! game runs.

use std::collections::HashMap;

use dark_assets::{Image, ModelDef, Project};
use glam::{Mat4, Quat, Vec3, Vec4};

mod pose;
pub use pose::Pose;

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("{path}: {source}")]
    Gltf {
        path: String,
        #[source]
        source: Box<gltf::Error>,
    },
    #[error("{path}: {what}")]
    Invalid { path: String, what: String },
}

fn invalid(path: &str, what: impl Into<String>) -> ModelError {
    ModelError::Invalid {
        path: path.to_owned(),
        what: what.into(),
    }
}

/// One vertex of a skinned mesh. Laid out as the shader will want it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub uv: [f32; 2],
    /// Which bones move it, as indices into [`Skeleton::bones`].
    pub joints: [u16; 4],
    /// How much each of those bones moves it. Normalised to sum to one.
    pub weights: Vec4,
}

/// A drawable piece of the model: triangles that share one picture.
#[derive(Clone, Debug, Default)]
pub struct Part {
    pub vertices: Vec<Vertex>,
    /// Triangles, three indices each.
    pub indices: Vec<u32>,
    /// Which of [`Model::images`] paints it, if any.
    pub image: Option<usize>,
}

/// A bone. `parent` is an index into [`Skeleton::bones`], earlier than this one.
#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    /// Where the bone rests, relative to its parent.
    pub rest: (Vec3, Quat, Vec3),
    /// Undoes the bind pose, so a vertex can be moved by the bone's animated transform.
    pub inverse_bind: Mat4,
}

/// One animation's keyframes, by the bone they move.
#[derive(Clone, Debug, Default)]
pub struct Animation {
    pub seconds: f32,
    /// By bone index.
    pub tracks: HashMap<usize, Track>,
}

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub translation: Vec<(f32, Vec3)>,
    pub rotation: Vec<(f32, Quat)>,
    pub scale: Vec<(f32, Vec3)>,
    /// How to read between the keys. Blender writes STEP for anything an animator snapped, and
    /// blending through those would smooth motion that was meant to jump.
    pub hold: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    /// Parents always come before their children, so one pass forward resolves the whole tree.
    pub bones: Vec<Bone>,
}

/// A model as loaded: geometry, pictures, skeleton and animations.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub parts: Vec<Part>,
    pub images: Vec<Image>,
    pub skeleton: Skeleton,
    pub animations: HashMap<String, Animation>,
    /// Model units to world pixels, from the definition.
    pub scale: f32,
}

impl Model {
    /// Loads the glTF a `*.model.ron` names. The **mesh**, not the source: the artist's export
    /// is never read at run time.
    pub fn load(project: &Project, def: &ModelDef) -> Result<Self, ModelError> {
        let path = project.path(&def.mesh);
        let shown = def.mesh.clone();
        let (document, buffers, images) =
            gltf::import(&path).map_err(|source| ModelError::Gltf {
                path: shown.clone(),
                source: Box::new(source),
            })?;

        let rigging = read_skeleton(&document, &buffers, &shown)?;
        let mut model = Model {
            skeleton: rigging.skeleton,
            scale: def.scale,
            images: images.into_iter().map(convert_image).collect(),
            ..Default::default()
        };

        for mesh in document.meshes() {
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                model
                    .parts
                    .push(read_part(&primitive, &buffers, &rigging.by_joint, &shown)?);
            }
        }
        if model.parts.is_empty() {
            return Err(invalid(&shown, "holds no triangles"));
        }
        model.animations = read_animations(&document, &buffers, &rigging.by_node);
        Ok(model)
    }

    /// How tall the model stands in world pixels, in its rest pose.
    ///
    /// Along **Y**: glTF is Y-up by specification, whatever the authoring tool used. Blender is
    /// Z-up and its exporter converts on the way out, so a model measured in Blender (as
    /// `dark-cli bake-model` measures it) and the same model read back here are the same height
    /// on different axes.
    pub fn height(&self) -> f32 {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for part in &self.parts {
            for vertex in &part.vertices {
                let up = vertex.position.dot(Self::UP);
                lo = lo.min(up);
                hi = hi.max(up);
            }
        }
        if lo > hi { 0.0 } else { (hi - lo) * self.scale }
    }

    /// Which way is up in a loaded model. glTF says Y; nothing here should guess otherwise.
    pub const UP: Vec3 = Vec3::Y;
}

/// A skeleton and the two ways glTF names its bones, which are not the same way.
///
/// A **vertex** names a bone by its place in the skin's joint list. An **animation** names one
/// by the glTF node it targets. Confusing the two skins a mesh to whichever bone happens to sit
/// at a node's number, and the character comes apart into spikes.
struct Rigging {
    skeleton: Skeleton,
    /// Place in the skin's joint list, to bone index.
    by_joint: Vec<usize>,
    /// glTF node index, to bone index.
    by_node: HashMap<usize, usize>,
}

fn read_skeleton(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    shown: &str,
) -> Result<Rigging, ModelError> {
    let Some(skin) = document.skins().next() else {
        return Ok(Rigging {
            skeleton: Skeleton::default(),
            by_joint: Vec::new(),
            by_node: HashMap::new(),
        });
    };
    if document.skins().count() > 1 {
        tracing::warn!("{shown}: more than one skin; only the first is read");
    }
    let joints: Vec<gltf::Node> = skin.joints().collect();
    let order: HashMap<usize, usize> = joints
        .iter()
        .enumerate()
        .map(|(bone, node)| (node.index(), bone))
        .collect();

    let reader = skin.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
    let inverse: Vec<Mat4> = reader
        .read_inverse_bind_matrices()
        .map(|m| m.map(|m| Mat4::from_cols_array_2d(&m)).collect())
        .unwrap_or_else(|| vec![Mat4::IDENTITY; joints.len()]);
    if inverse.len() < joints.len() {
        return Err(invalid(shown, "fewer inverse bind matrices than joints"));
    }

    // Who each joint's parent is. glTF says it the other way round, so walk the children once.
    let mut parent_of: HashMap<usize, usize> = HashMap::new();
    for node in document.nodes() {
        for child in node.children() {
            parent_of.insert(child.index(), node.index());
        }
    }

    // Everything above a joint still moves it. glTF puts a joint's matrix on the chain from the
    // scene root, and an exporter routinely leaves a transform up there: Blender's armature
    // object carries the whole Z-up to Y-up turn and its own scale. A joint whose parent is not
    // itself a joint therefore inherits that chain, folded into where it rests.
    let above = |node: &gltf::Node| {
        let mut carried = Mat4::IDENTITY;
        let mut at = parent_of.get(&node.index()).copied();
        while let Some(index) = at {
            if order.contains_key(&index) {
                break; // A joint: `Pose` walks the rest of the way itself.
            }
            let Some(ancestor) = document.nodes().nth(index) else {
                break;
            };
            carried = Mat4::from_cols_array_2d(&ancestor.transform().matrix()) * carried;
            at = parent_of.get(&index).copied();
        }
        carried
    };

    let mut bones: Vec<Bone> = joints
        .iter()
        .enumerate()
        .map(|(bone, node)| {
            let parent = parent_of
                .get(&node.index())
                .and_then(|p| order.get(p))
                .copied();
            let local = Mat4::from_cols_array_2d(&node.transform().matrix());
            // A root joint carries what sits above it; a joint with a joint parent does not,
            // because its parent already did.
            let rest = if parent.is_none() {
                above(node) * local
            } else {
                local
            };
            let (s, r, t) = rest.to_scale_rotation_translation();
            Bone {
                name: node.name().unwrap_or("").to_owned(),
                parent,
                rest: (t, r, s),
                inverse_bind: inverse[bone],
            }
        })
        .collect();

    // Bones arrive in the skin's own order, so a vertex's joint index is already its bone index.
    let mut by_joint: Vec<usize> = (0..bones.len()).collect();

    // A parent must come before its child, or one forward pass cannot resolve the tree. glTF
    // does not promise that order, so check it and say so rather than posing nonsense.
    let tangled = bones
        .iter()
        .enumerate()
        .any(|(bone, entry)| entry.parent.is_some_and(|p| p >= bone));
    if tangled {
        tracing::warn!("{shown}: joints are not in parent order; sorting them");
        let (sorted, moved) = sort_into_parent_order(bones);
        bones = sorted;
        // `moved` says where the bone that was at each index went, which is exactly what a
        // joint index now has to be read through.
        by_joint = moved;
    }
    let by_node = joints
        .iter()
        .enumerate()
        .map(|(joint, node)| (node.index(), by_joint[joint]))
        .collect();
    Ok(Rigging {
        skeleton: Skeleton { bones },
        by_joint,
        by_node,
    })
}

/// Re-orders bones so every parent precedes its children, and says where each one went, so the
/// maps that name them can follow. Only reached when a file breaks the usual order; written so
/// that it cannot loop forever on one that also has a cycle.
fn sort_into_parent_order(bones: Vec<Bone>) -> (Vec<Bone>, Vec<usize>) {
    let mut placed: Vec<Option<usize>> = vec![None; bones.len()];
    let mut order: Vec<usize> = Vec::with_capacity(bones.len());
    let mut left: Vec<usize> = (0..bones.len()).collect();
    while !left.is_empty() {
        let ready: Vec<usize> = left
            .iter()
            .copied()
            .filter(|&b| bones[b].parent.is_none_or(|p| placed[p].is_some()))
            .collect();
        if ready.is_empty() {
            // A cycle. Take what is left in the order it came, rather than hanging.
            order.extend(left.iter().copied());
            for (at, &b) in order.iter().enumerate() {
                placed[b] = Some(at);
            }
            break;
        }
        for b in ready {
            placed[b] = Some(order.len());
            order.push(b);
            left.retain(|&x| x != b);
        }
    }
    let sorted = order
        .iter()
        .map(|&b| Bone {
            parent: bones[b].parent.and_then(|p| placed[p]),
            ..bones[b].clone()
        })
        .collect();
    (sorted, placed.into_iter().map(|p| p.unwrap_or(0)).collect())
}

fn read_part(
    primitive: &gltf::Primitive,
    buffers: &[gltf::buffer::Data],
    by_joint: &[usize],
    shown: &str,
) -> Result<Part, ModelError> {
    let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
    let positions: Vec<Vec3> = reader
        .read_positions()
        .ok_or_else(|| invalid(shown, "a primitive has no positions"))?
        .map(Vec3::from)
        .collect();
    let normals: Vec<Vec3> = reader
        .read_normals()
        .map(|n| n.map(Vec3::from).collect())
        .unwrap_or_else(|| vec![Vec3::Z; positions.len()]);
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|t| t.into_f32().collect())
        .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);
    let joints: Vec<[u16; 4]> = reader
        .read_joints(0)
        .map(|j| j.into_u16().collect())
        .unwrap_or_else(|| vec![[0; 4]; positions.len()]);
    let weights: Vec<[f32; 4]> = reader
        .read_weights(0)
        .map(|w| w.into_f32().collect())
        .unwrap_or_else(|| vec![[1.0, 0.0, 0.0, 0.0]; positions.len()]);

    // glTF requires every attribute of a primitive to share one count. A short one would
    // silently pin the tail of the mesh to bone zero and drag it to the origin, which is exactly
    // the tearing this loader has already been debugged for once.
    for (what, len) in [
        ("normals", normals.len()),
        ("texture coordinates", uvs.len()),
        ("joints", joints.len()),
        ("weights", weights.len()),
    ] {
        if len != positions.len() {
            return Err(invalid(
                shown,
                format!(
                    "a primitive has {} positions but {len} {what}",
                    positions.len()
                ),
            ));
        }
    }

    let vertices = (0..positions.len())
        .map(|i| {
            // A vertex names a joint by its place in the skin's list, which is the bone's index
            // unless the skeleton had to be re-ordered. Never a node index.
            let mut joint = joints.get(i).copied().unwrap_or([0; 4]);
            for slot in &mut joint {
                *slot = by_joint.get(*slot as usize).copied().unwrap_or(0) as u16;
            }
            let w = Vec4::from(weights.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]));
            let sum = w.x + w.y + w.z + w.w;
            Vertex {
                position: positions[i],
                normal: normals.get(i).copied().unwrap_or(Vec3::Z),
                uv: uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                joints: joint,
                // A weightless vertex would vanish to the origin; pin it to its first bone.
                weights: if sum > 1e-6 { w / sum } else { Vec4::X },
            }
        })
        .collect();

    let indices: Vec<u32> = match reader.read_indices() {
        Some(read) => read.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };
    let image = primitive
        .material()
        .pbr_metallic_roughness()
        .base_color_texture()
        .map(|t| t.texture().source().index());
    Ok(Part {
        vertices,
        indices,
        image,
    })
}

fn read_animations(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    bone_of: &HashMap<usize, usize>,
) -> HashMap<String, Animation> {
    let mut animations = HashMap::new();
    for (nth, animation) in document.animations().enumerate() {
        let name = animation
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("animation{nth}"));
        let mut built = Animation::default();
        let (mut start, mut latest) = (f32::MAX, 0.0f32);
        for channel in animation.channels() {
            let Some(&bone) = bone_of.get(&channel.target().node().index()) else {
                continue;
            };
            let reader = channel.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(times) = reader.read_inputs() else {
                continue;
            };
            let times: Vec<f32> = times.collect();
            let interpolation = channel.sampler().interpolation();
            if interpolation == gltf::animation::Interpolation::CubicSpline {
                // Cubic keys hold three values each — in-tangent, value, out-tangent — so
                // pairing them with times one for one would read tangents as poses. Refusing is
                // honest; silently drawing it wrong is not.
                tracing::warn!(
                    "{name}: a track is cubic, which is not read yet; it will hold its first key"
                );
                continue;
            }
            // Where the clip begins, over all its channels. Blender writes keys at the action's
            // own frame numbers, so a Mixamo action starts at 1/30s, not 0. Playback counts
            // ticks from zero, so the whole clip is shifted back to start there.
            for time in &times {
                start = start.min(*time);
            }
            latest = latest.max(times.last().copied().unwrap_or(0.0));
            let track = built.tracks.entry(bone).or_default();
            track.hold = interpolation == gltf::animation::Interpolation::Step;
            let expected = times.len();
            match reader.read_outputs() {
                Some(gltf::animation::util::ReadOutputs::Translations(values)) => {
                    let values: Vec<Vec3> = values.map(Vec3::from).collect();
                    if values.len() == expected {
                        track.translation = times.iter().copied().zip(values).collect();
                    }
                }
                Some(gltf::animation::util::ReadOutputs::Rotations(values)) => {
                    let values: Vec<Quat> = values.into_f32().map(Quat::from_array).collect();
                    if values.len() == expected {
                        track.rotation = times.iter().copied().zip(values).collect();
                    }
                }
                Some(gltf::animation::util::ReadOutputs::Scales(values)) => {
                    let values: Vec<Vec3> = values.map(Vec3::from).collect();
                    if values.len() == expected {
                        track.scale = times.iter().copied().zip(values).collect();
                    }
                }
                _ => {}
            }
        }
        // Shift the clip to begin at zero and record how long it runs, so that its length is
        // the span Blender measured when it baked and a tick can be played as `tick / 60`.
        if start.is_finite() && start != f32::MAX {
            for track in built.tracks.values_mut() {
                for (at, _) in &mut track.translation {
                    *at -= start;
                }
                for (at, _) in &mut track.rotation {
                    *at -= start;
                }
                for (at, _) in &mut track.scale {
                    *at -= start;
                }
            }
            built.seconds = (latest - start).max(0.0);
        }
        animations.insert(name, built);
    }
    animations
}

fn convert_image(data: gltf::image::Data) -> Image {
    let (w, h) = (data.width, data.height);
    let pixels = (w * h) as usize;
    let rgba = match data.format {
        gltf::image::Format::R8G8B8A8 => data.pixels,
        gltf::image::Format::R8G8B8 => data
            .pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        gltf::image::Format::R8 => data.pixels.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        gltf::image::Format::R8G8 => data
            .pixels
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        // Everything else is a depth or float format no character texture uses.
        _ => vec![255; pixels * 4],
    };
    Image {
        width: w,
        height: h,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(name: &str, parent: Option<usize>) -> Bone {
        Bone {
            name: name.to_owned(),
            parent,
            rest: (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
            inverse_bind: Mat4::IDENTITY,
        }
    }

    /// `Pose` resolves the tree in one forward pass, so a parent must precede its child. glTF
    /// does not promise that. The sort has to put them in order **and** say where each bone
    /// went, because a vertex names its bone by the place it used to be in.
    #[test]
    fn sorting_puts_parents_first_and_says_where_every_bone_went() {
        // Branching on purpose. A straight reversal would make the map and its inverse the same
        // array, and returning either would pass — which is no test of the direction at all.
        //
        //   root -> chest -> arm -> hand
        //             \-> tail
        let bones = vec![
            bone("hand", Some(1)),
            bone("arm", Some(2)),
            bone("chest", Some(3)),
            bone("root", None),
            bone("tail", Some(2)),
        ];
        let (sorted, moved) = sort_into_parent_order(bones);

        assert_eq!(sorted.len(), 5);
        assert_eq!(moved.len(), 5, "every bone is accounted for");
        for (at, entry) in sorted.iter().enumerate() {
            if let Some(parent) = entry.parent {
                assert!(
                    parent < at,
                    "{} is placed before its own parent",
                    entry.name
                );
            }
        }
        // Old index to new index, which is the direction a joint index is read through. The
        // inverse would send 0 to "tail" here, so this pins which way round it is.
        assert_eq!(sorted[moved[0]].name, "hand");
        assert_eq!(sorted[moved[4]].name, "tail");
        assert_eq!(sorted[moved[3]].name, "root");
        assert_eq!(sorted[0].name, "root", "the root is placed first");
        // And the tree still hangs together after the move.
        let named = |at: usize| sorted[at].name.as_str();
        let parent_of = |name: &str| {
            let at = sorted.iter().position(|b| b.name == name).expect("a bone");
            sorted[at].parent.map(named)
        };
        assert_eq!(parent_of("hand"), Some("arm"));
        assert_eq!(parent_of("arm"), Some("chest"));
        assert_eq!(parent_of("chest"), Some("root"));
        assert_eq!(parent_of("tail"), Some("chest"));
        assert_eq!(parent_of("root"), None);
    }

    /// A file can name a bone its own ancestor. That must not hang the loader.
    #[test]
    fn a_skeleton_that_loops_back_on_itself_still_finishes() {
        let bones = vec![bone("a", Some(1)), bone("b", Some(0)), bone("free", None)];
        let (sorted, moved) = sort_into_parent_order(bones);
        assert_eq!(sorted.len(), 3);
        assert_eq!(moved.len(), 3);
        assert!(
            moved.iter().all(|&m| m < 3),
            "every bone lands somewhere real"
        );
        assert_eq!(sorted[0].name, "free", "what can be placed still is");
    }

    /// A skeleton already in order must come back untouched, or a well-formed file would be
    /// shuffled for nothing and every joint index would have to move with it.
    #[test]
    fn a_skeleton_already_in_order_keeps_its_places() {
        let bones = vec![
            bone("root", None),
            bone("chest", Some(0)),
            bone("arm", Some(1)),
        ];
        let (sorted, moved) = sort_into_parent_order(bones);
        assert_eq!(moved, vec![0, 1, 2]);
        let names: Vec<&str> = sorted.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["root", "chest", "arm"]);
    }
}
