//! Skinned 3D meshes on the GPU (`journals/engine/04`, phase 2b).
//!
//! Sprites are drawn by painter's algorithm, sorted by layer and by feet. A mesh cannot be: it
//! is solid, it sees itself from every side, and a hand at the front of a body has to hide the
//! chest behind it. So meshes are drawn in a pass of their own, against a depth buffer, and the
//! camera is a matrix rather than an origin and a size.
//!
//! What a bone is and how a pose is worked out belongs to `dark_model`. This is only the part
//! that hands it to the GPU.

use glam::{Mat4, Vec3};

use crate::TextureId;

/// How many bones one draw may carry. A Mixamo rig arrives with 69, fingers and eyes included;
/// this leaves room to spare without the palette outgrowing a uniform buffer's guaranteed size
/// (128 × 64 bytes is 8 KiB, and 64 KiB is promised everywhere).
pub const MAX_BONES: usize = 128;

/// One vertex of a skinned mesh, laid out as the shader reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ModelVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Which bones move it, as indices into the draw's palette.
    pub joints: [u16; 4],
    /// How much each moves it. Normalised to sum to one before it gets here.
    pub weights: [f32; 4],
}

/// One model, posed, to be drawn.
pub struct ModelDraw<'a> {
    pub vertices: &'a [ModelVertex],
    /// Triangles, three indices each, into `vertices`.
    pub indices: &'a [u32],
    /// One matrix per bone: `placement × bone's world transform × its inverse bind`. Built on
    /// the CPU so the shader multiplies one matrix per vertex rather than three.
    pub palette: &'a [Mat4],
    pub texture: TextureId,
    /// Drawn from both faces. A closed body wants its inside culled; a hair card or a skirt
    /// modelled as one layer would lose half its triangles to that.
    pub double_sided: bool,
}

/// Where the light comes from and how dark it leaves the far side.
#[derive(Clone, Copy, Debug)]
pub struct Light {
    pub towards: Vec3,
    /// What a surface facing away still receives, 0 to 1.
    pub ambient: f32,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            // Over the viewer's left shoulder and down, which is where a top-down game's sun
            // sits if nobody has said otherwise.
            towards: Vec3::new(-0.4, 0.7, 0.6),
            ambient: 0.35,
        }
    }
}

/// What the model shader's group 0 holds.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ModelCamera {
    pub view_projection: [[f32; 4]; 4],
    /// `xyz` towards the light, `w` the ambient floor.
    pub light: [f32; 4],
}

impl ModelCamera {
    pub(crate) fn new(camera: Mat4, light: Light) -> Self {
        let towards = light.towards.normalize_or(Vec3::Z);
        Self {
            view_projection: camera.to_cols_array_2d(),
            light: [
                towards.x,
                towards.y,
                towards.z,
                light.ambient.clamp(0.0, 1.0),
            ],
        }
    }
}

/// A palette padded to the stride the bone buffer uses, ready to write.
///
/// Anything past the model's own bones is left as identity rather than as whatever the previous
/// draw put there: a vertex naming a bone the model does not have then stays where the artist
/// put it instead of being flung somewhere by another character's hip.
pub(crate) fn padded_palette(palette: &[Mat4]) -> Vec<Mat4> {
    let mut all = vec![Mat4::IDENTITY; MAX_BONES];
    // Zipping against the slots is what cuts a rig longer than the palette down to it.
    for (slot, bone) in all.iter_mut().zip(palette) {
        *slot = *bone;
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each bone lands in its own slot, and the slots past the rig are left as identity —
    /// whatever the last draw put there would otherwise move this model's vertices.
    ///
    /// The bones are told apart on purpose. Two of the same matrix cannot see a reversal or an
    /// off-by-one, which is the trap two earlier tests in this line of work fell into.
    #[test]
    fn every_bone_lands_in_its_own_slot_and_the_rest_are_identity() {
        let along = |n: f32| Mat4::from_translation(Vec3::new(n, 0.0, 0.0));
        let padded = padded_palette(&[along(1.0), along(2.0), along(3.0)]);
        assert_eq!(padded.len(), MAX_BONES);
        assert_eq!(padded[0], along(1.0), "in order, not reversed");
        assert_eq!(padded[1], along(2.0));
        assert_eq!(padded[2], along(3.0));
        assert_eq!(padded[3], Mat4::IDENTITY, "the rest is left alone");
        assert_eq!(padded[MAX_BONES - 1], Mat4::IDENTITY);
    }

    /// A rig larger than the palette is cut to what fits, keeping the bones that fit rather
    /// than running off the end of the buffer.
    #[test]
    fn a_palette_longer_than_the_buffer_keeps_the_bones_that_fit() {
        let along = |n: usize| Mat4::from_translation(Vec3::new(n as f32, 0.0, 0.0));
        let many: Vec<Mat4> = (0..MAX_BONES + 40).map(along).collect();
        let padded = padded_palette(&many);
        assert_eq!(padded.len(), MAX_BONES);
        assert_eq!(padded[0], along(0), "the first bone is still the first");
        assert_eq!(
            padded[MAX_BONES - 1],
            along(MAX_BONES - 1),
            "and the last that fits is the last that fits, not the last of the rig"
        );
    }

    /// The light is handed over normalised, so a shader can dot with it directly, and a light
    /// nobody set does not divide by zero.
    #[test]
    fn the_light_arrives_normalised() {
        let camera = ModelCamera::new(
            Mat4::IDENTITY,
            Light {
                towards: Vec3::new(0.0, 0.0, 5.0),
                ambient: 2.0,
            },
        );
        assert_eq!(
            [camera.light[0], camera.light[1], camera.light[2]],
            [0.0, 0.0, 1.0]
        );
        assert_eq!(camera.light[3], 1.0, "ambient is held to 0..1");

        let dark = ModelCamera::new(
            Mat4::IDENTITY,
            Light {
                towards: Vec3::ZERO,
                ambient: -1.0,
            },
        );
        assert!(
            dark.light[..3].iter().all(|v| v.is_finite()),
            "a light of no length does not divide by zero"
        );
        assert_eq!(dark.light[3], 0.0);
    }
}

/// Tests that put the pass on a real GPU. They ask the machine for an adapter and **skip** when
/// there is none, because a build machine may have no graphics at all; on a desktop they run.
#[cfg(test)]
mod gpu_tests {
    use super::*;
    use crate::Renderer;

    const SIZE: (u32, u32) = (64, 64);

    fn renderer() -> Option<Renderer> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(Renderer::offscreen(&adapter, device, queue, SIZE))
    }

    /// A quad in clip space. With an identity camera a position is already where it lands, which
    /// keeps these tests about the pass rather than about a projection.
    fn quad(z: f32) -> (Vec<ModelVertex>, Vec<u32>) {
        let at = |x: f32, y: f32| ModelVertex {
            position: [x, y, z],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            joints: [0, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        let vertices = vec![at(-0.5, -0.5), at(0.5, -0.5), at(0.5, 0.5), at(-0.5, 0.5)];
        (vertices, vec![0, 1, 2, 0, 2, 3])
    }

    /// Flat, so the colour read back is the texture's and not the lighting's.
    fn flat() -> Light {
        Light {
            towards: Vec3::Z,
            ambient: 1.0,
        }
    }

    fn middle(capture: &crate::Capture) -> [u8; 3] {
        let (x, y) = (capture.width / 2, capture.height / 2);
        let at = ((y * capture.width + x) * 4) as usize;
        [capture.rgba[at], capture.rgba[at + 1], capture.rgba[at + 2]]
    }

    /// The cull-back pipeline is what every closed character is drawn with, and until this test
    /// existed it had never drawn a triangle: the one model to hand is marked `doubleSided`, so
    /// only the other pipeline ever ran. A silhouette comparison cannot find this either — a
    /// closed mesh looks the same whichever face you cull.
    ///
    /// The winding here is the one `quad` uses, measured against the projection rather than
    /// reasoned about: it faces the camera, and reversed it is culled.
    #[test]
    fn a_back_face_is_culled_unless_the_material_is_double_sided() {
        let Some(mut renderer) = renderer() else {
            eprintln!("no GPU; skipping");
            return;
        };
        let white = renderer
            .create_texture("white", 1, 1, &[255; 4])
            .expect("a texture");
        let palette = [Mat4::IDENTITY];
        let (vertices, facing) = quad(0.5);
        // The same four corners, wound the other way about.
        let away = vec![0u32, 2, 1, 0, 3, 2];

        let mut shown = |indices: &Vec<u32>, double_sided: bool| {
            renderer.render_models(
                Mat4::IDENTITY,
                flat(),
                Some([0.0, 0.0, 0.0]),
                &[ModelDraw {
                    vertices: &vertices,
                    indices,
                    palette: &palette,
                    texture: white,
                    double_sided,
                }],
            );
            middle(&renderer.capture())[0] > 200
        };

        assert!(
            shown(&facing, false),
            "a front face is drawn when backs are culled"
        );
        assert!(!shown(&away, false), "a back face is not");
        // And a material marked double-sided keeps both, which is what hair and cloth need.
        assert!(shown(&facing, true), "double-sided keeps the front");
        assert!(shown(&away, true), "double-sided keeps the back too");
    }

    /// The nearer of two overlapping surfaces wins the pixel, whichever order they are drawn in.
    /// Without a depth buffer the last one drawn would simply paint over the first.
    #[test]
    fn the_nearer_surface_hides_the_further_one() {
        let Some(mut renderer) = renderer() else {
            eprintln!("no GPU; skipping");
            return;
        };
        let red = renderer
            .create_texture("red", 1, 1, &[255, 0, 0, 255])
            .expect("a texture");
        let blue = renderer
            .create_texture("blue", 1, 1, &[0, 0, 255, 255])
            .expect("a texture");
        let palette = [Mat4::IDENTITY];
        let (near_v, near_i) = quad(0.2);
        let (far_v, far_i) = quad(0.8);
        let draw = |v, i, t| ModelDraw {
            vertices: v,
            indices: i,
            palette: &palette,
            texture: t,
            double_sided: true,
        };

        // Far first, then near.
        renderer.render_models(
            Mat4::IDENTITY,
            flat(),
            Some([0.0, 0.0, 0.0]),
            &[draw(&far_v, &far_i, blue), draw(&near_v, &near_i, red)],
        );
        let [r, g, b] = middle(&renderer.capture());
        assert!(
            r > 200 && g < 60 && b < 60,
            "near drawn last: got {r},{g},{b}"
        );

        // And the other way round, which is the order a painter's algorithm gets wrong.
        renderer.render_models(
            Mat4::IDENTITY,
            flat(),
            Some([0.0, 0.0, 0.0]),
            &[draw(&near_v, &near_i, red), draw(&far_v, &far_i, blue)],
        );
        let [r, g, b] = middle(&renderer.capture());
        assert!(
            r > 200 && g < 60 && b < 60,
            "near drawn first: got {r},{g},{b}"
        );
    }

    /// The bone palette reaches the shader and moves the vertices that name it. Every other part
    /// of this pass could be right and a character would still stand in its bind pose.
    #[test]
    fn the_bone_palette_moves_the_mesh() {
        let Some(mut renderer) = renderer() else {
            eprintln!("no GPU; skipping");
            return;
        };
        let white = renderer
            .create_texture("white", 1, 1, &[255; 4])
            .expect("a texture");
        let (vertices, indices) = quad(0.5);

        let at_rest = [Mat4::IDENTITY];
        renderer.render_models(
            Mat4::IDENTITY,
            flat(),
            Some([0.0, 0.0, 0.0]),
            &[ModelDraw {
                vertices: &vertices,
                indices: &indices,
                palette: &at_rest,
                texture: white,
                double_sided: true,
            }],
        );
        let [r, ..] = middle(&renderer.capture());
        assert!(r > 200, "the quad covers the middle at rest: got {r}");

        // Now move the bone it hangs from, far enough to take it out of the picture.
        let moved = [Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0))];
        renderer.render_models(
            Mat4::IDENTITY,
            flat(),
            Some([0.0, 0.0, 0.0]),
            &[ModelDraw {
                vertices: &vertices,
                indices: &indices,
                palette: &moved,
                texture: white,
                double_sided: true,
            }],
        );
        let [r, ..] = middle(&renderer.capture());
        assert!(r < 60, "moving the bone takes the quad away: got {r}");
    }

    /// Two draws share one vertex buffer, one index buffer and one bone buffer, and each must
    /// read its own stretch of all three. A model of a single part cannot show this, because
    /// every offset is zero.
    #[test]
    fn a_second_draw_reads_its_own_vertices_and_its_own_palette() {
        let Some(mut renderer) = renderer() else {
            eprintln!("no GPU; skipping");
            return;
        };
        let white = renderer
            .create_texture("white", 1, 1, &[255; 4])
            .expect("a texture");
        let (away_v, away_i) = quad(0.5);
        let (here_v, here_i) = quad(0.5);
        // The first draw is taken out of the picture by its palette; the second is not. If the
        // second read the first's palette, or its vertices, the middle would stay empty.
        let away = [Mat4::from_translation(Vec3::new(-3.0, 0.0, 0.0))];
        let here = [Mat4::IDENTITY];

        renderer.render_models(
            Mat4::IDENTITY,
            flat(),
            Some([0.0, 0.0, 0.0]),
            &[
                ModelDraw {
                    vertices: &away_v,
                    indices: &away_i,
                    palette: &away,
                    texture: white,
                    double_sided: true,
                },
                ModelDraw {
                    vertices: &here_v,
                    indices: &here_i,
                    palette: &here,
                    texture: white,
                    double_sided: true,
                },
            ],
        );
        let [r, ..] = middle(&renderer.capture());
        assert!(
            r > 200,
            "the second draw lands where its own palette puts it: got {r}"
        );
    }
}
