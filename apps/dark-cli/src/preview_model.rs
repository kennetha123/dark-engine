//! `dark-cli preview-model` — draws a posed model on the CPU (`journals/engine/04`, phase 2).
//!
//! The same idea as `preview-spine`: fill the triangles the game will draw, without a window or
//! a GPU, so that loading and skinning can be checked by looking. A depth buffer stands in for
//! the one the renderer will have, since a mesh seen from any angle covers itself.

use dark_assets::Project;
use dark_model::{Model, Pose};
use glam::camera::rh::proj::directx::orthographic;
use glam::camera::rh::view::look_at_mat4;
use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};

/// Where the camera stands: the angle a top-down action RPG looks from.
const ELEVATION: f32 = 50.0;

/// How large a preview is drawn. Both previews use it, so their pictures line up.
pub const SIZE: (u32, u32) = (360, 460);

/// Looks down at a posed model from [`ELEVATION`], framed on its own bounds so that any export
/// fills the picture whatever scale it was authored at. Orthographic, as a top-down game is.
///
/// Shared by the CPU preview and the GPU one, so the two can be compared: a difference between
/// them is then the drawing, not the camera.
///
/// Returns the world-to-clip matrix and how large the model is, in its own units.
pub fn game_camera(model: &Model, pose: &Pose, (w, h): (u32, u32)) -> (Mat4, f32) {
    let (lo, hi) = bounds(model, pose);
    let mid = (lo + hi) / 2.0;
    let size = (hi - lo).max_element().max(1e-3);
    // glTF is Y-up, so the camera climbs Y and the horizontal plane is XZ (`Model::UP`).
    let angle = ELEVATION.to_radians();
    let eye = mid + Vec3::new(0.0, angle.sin(), angle.cos()) * size * 3.0;
    let view = look_at_mat4(eye, mid, Model::UP);
    let half = size * 0.62;
    // The DirectX convention puts depth in 0..1, which is what wgpu wants.
    let projection = orthographic(
        -half,
        half,
        -half * h as f32 / w as f32,
        half * h as f32 / w as f32,
        0.01,
        size * 10.0,
    );
    (projection * view, size)
}

pub fn preview_model(
    project: &str,
    model_path: &str,
    clip: &str,
    tick: &str,
    out: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let project = Project::open(project)?;
    let loaded = project.load_model(model_path)?;
    let baked = loaded
        .bake
        .clips
        .get(clip)
        .ok_or_else(|| format!("no clip {clip}; it has {:?}", loaded.bake.clips.keys()))?;
    let tick: u32 = tick.parse()?;
    let model = Model::load(&project, &loaded.def)?;

    let mut pose = Pose::new(&model);
    pose.pose(&model, &baked.animation, baked.looping, tick as f32 / 60.0);

    let (w, h) = SIZE;
    let mut canvas = image::RgbaImage::from_pixel(w, h, image::Rgba([120, 132, 124, 255]));
    let mut depth = vec![f32::MAX; (w * h) as usize];
    let (camera, _) = game_camera(&model, &pose, SIZE);

    let mut drawn = 0usize;
    for part in &model.parts {
        let image = part.image.and_then(|i| model.images.get(i));
        let posed: Vec<(Vec3, Vec3, [f32; 2])> = part
            .vertices
            .iter()
            .map(|v| {
                let skin = skin_of(v, pose.palette());
                (
                    skin.transform_point3(v.position),
                    skin.transform_vector3(v.normal).normalize_or_zero(),
                    v.uv,
                )
            })
            .collect();

        for tri in part.indices.chunks_exact(3) {
            let corner: Vec<&(Vec3, Vec3, [f32; 2])> =
                tri.iter().filter_map(|&i| posed.get(i as usize)).collect();
            if corner.len() != 3 {
                continue;
            }
            // To the screen, keeping depth so the near surface wins.
            let clip: Vec<glam::Vec4> = corner
                .iter()
                .map(|(p, ..)| camera * p.extend(1.0))
                .collect();
            if clip.iter().any(|c| c.w.abs() < 1e-6) {
                continue;
            }
            let ndc: Vec<Vec3> = clip.iter().map(|c| c.xyz() / c.w).collect();
            // Every comparison below is false for a NaN, so a NaN triangle would skip the
            // rejections, skip the depth test, and write NaN into the buffer — after which that
            // pixel never wins a depth test again and takes whatever is drawn last.
            if !ndc.iter().all(|n| n.is_finite()) {
                continue;
            }
            let screen: Vec<Vec2> = ndc
                .iter()
                .map(|n| Vec2::new((n.x * 0.5 + 0.5) * w as f32, (0.5 - n.y * 0.5) * h as f32))
                .collect();

            let area = (screen[1] - screen[0]).perp_dot(screen[2] - screen[0]);
            if area.abs() < 1e-6 {
                continue;
            }
            // Flat shading from the face's own normal: enough to read the form, and it is the
            // banding the toon shader will replace in phase 3.
            let lit = {
                let normal = (corner[0].1 + corner[1].1 + corner[2].1).normalize_or_zero();
                // The renderer's own light, so that what differs between the two pictures is
                // the drawing. Lighting this from anywhere else made them incomparable on
                // everything but the silhouette.
                let light = dark_render::Light::default();
                let towards = light.towards.normalize();
                light.ambient + (1.0 - light.ambient) * normal.dot(towards).max(0.0)
            };
            drawn += 1;

            let (min, max) = (
                screen[0].min(screen[1]).min(screen[2]),
                screen[0].max(screen[1]).max(screen[2]),
            );
            let y0 = min.y.floor().max(0.0) as u32;
            let y1 = (max.y.ceil() as i64).clamp(0, h as i64) as u32;
            let x0 = min.x.floor().max(0.0) as u32;
            let x1 = (max.x.ceil() as i64).clamp(0, w as i64) as u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let b1 = (screen[2] - screen[1]).perp_dot(q - screen[1]) / area;
                    let b2 = (screen[0] - screen[2]).perp_dot(q - screen[2]) / area;
                    let b3 = 1.0 - b1 - b2;
                    if b1 < 0.0 || b2 < 0.0 || b3 < 0.0 {
                        continue;
                    }
                    let z = ndc[0].z * b1 + ndc[1].z * b2 + ndc[2].z * b3;
                    let at = (y * w + x) as usize;
                    if z >= depth[at] {
                        continue;
                    }
                    let uv = [
                        corner[0].2[0] * b1 + corner[1].2[0] * b2 + corner[2].2[0] * b3,
                        corner[0].2[1] * b1 + corner[1].2[1] * b2 + corner[2].2[1] * b3,
                    ];
                    let [r, g, b, a] = match image {
                        Some(picture) => sample(picture, uv),
                        None => [200, 190, 180, 255],
                    };
                    // Cut out, exactly as `model.wgsl` does. Hair and cloth are drawn on cards
                    // whose texture is mostly transparent; taking the colour and ignoring the
                    // alpha fills those cards in solid and puts a dark blob where the hair is.
                    if a < 128 {
                        continue;
                    }
                    depth[at] = z;
                    let pixel = canvas.get_pixel_mut(x, y);
                    for (c, v) in [r, g, b].into_iter().enumerate() {
                        pixel[c] = (f32::from(v) * lit).clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }

    canvas.save(out)?;
    // The loaded height against the one baking measured in Blender: they are the same model on
    // different axes, so a disagreement means a transform was dropped or the export is not Y-up.
    println!(
        "{out}: {clip} at tick {tick}, {drawn} triangles, {} bones ({w}x{h})",
        model.skeleton.bones.len()
    );
    println!(
        "  height: {:.2} px loaded, {:.2} px baked",
        model.height(),
        loaded.bake.height
    );
    Ok(())
}

/// A vertex's own matrix: its bones' palette entries, in the proportions it names.
pub fn skin_of(vertex: &dark_model::Vertex, palette: &[Mat4]) -> Mat4 {
    let weight: [f32; 4] = vertex.weights.into();
    let mut skin = Mat4::ZERO;
    let mut used = 0.0;
    for (joint, w) in vertex.joints.iter().zip(weight) {
        if w <= 0.0 {
            continue;
        }
        if let Some(bone) = palette.get(*joint as usize) {
            skin += *bone * w;
            used += w;
        }
    }
    // A vertex whose bones are all missing would collapse to the origin and drag its triangle
    // across the picture; leave it where the artist put it instead.
    if used > 1e-6 { skin } else { Mat4::IDENTITY }
}

/// The posed model's bounds, so the camera frames whatever it is given.
pub fn bounds(model: &Model, pose: &Pose) -> (Vec3, Vec3) {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for part in &model.parts {
        for vertex in &part.vertices {
            let at = skin_of(vertex, pose.palette()).transform_point3(vertex.position);
            lo = lo.min(at);
            hi = hi.max(at);
        }
    }
    if lo.x > hi.x {
        (Vec3::ZERO, Vec3::ONE)
    } else {
        (lo, hi)
    }
}

fn sample(image: &dark_assets::Image, uv: [f32; 2]) -> [u8; 4] {
    if image.width == 0 || image.height == 0 {
        return [200, 190, 180, 255];
    }
    let wrap = |v: f32, n: u32| ((v.rem_euclid(1.0) * n as f32) as u32).min(n - 1);
    let (x, y) = (wrap(uv[0], image.width), wrap(uv[1], image.height));
    let at = ((y * image.width + x) * 4) as usize;
    match image.rgba.get(at..at + 4) {
        Some(rgba) => [rgba[0], rgba[1], rgba[2], rgba[3]],
        None => [200, 190, 180, 255],
    }
}
