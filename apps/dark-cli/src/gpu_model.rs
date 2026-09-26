//! `dark-cli preview-model --gpu` — the same posed model, through the real renderer
//! (`journals/engine/04`, phase 2b).
//!
//! The CPU preview beside it proves that a model loads and poses. This proves that the mesh
//! pipeline draws what the posing worked out: same model, same clip, same tick, same camera, one
//! filled by hand and one by the GPU. Two pictures that agree mean the shader's skinning matches
//! the skinning `dark_model` does, and a depth buffer is sorting the surfaces.

use dark_assets::Project;
use dark_model::{Model, Pose};
use dark_render::{Light, MAX_BONES, ModelDraw, ModelVertex, Renderer};
use glam::Mat4;

/// Asks the machine for any adapter and builds a renderer with no window.
fn offscreen(size: (u32, u32)) -> Result<Renderer, String> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .map_err(|e| format!("no GPU to draw with: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("preview-model"),
        ..Default::default()
    }))
    .map_err(|e| format!("cannot open the GPU: {e}"))?;
    Ok(Renderer::offscreen(&adapter, device, queue, size))
}

pub fn preview_model_gpu(
    project: &str,
    model_path: &str,
    clip: &str,
    tick: &str,
    out: &str,
    size: (u32, u32),
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
    if model.skeleton.bones.len() > MAX_BONES {
        return Err(format!(
            "{} bones; the palette holds {MAX_BONES}",
            model.skeleton.bones.len()
        )
        .into());
    }

    let mut pose = Pose::new(&model);
    pose.pose(&model, &baked.animation, baked.looping, tick as f32 / 60.0);

    let mut renderer = offscreen(size)?;
    // The same framing the CPU preview uses, so the two pictures can be compared.
    let (camera, _) = crate::preview_model::game_camera(&model, &pose, size);

    // One texture per part, uploaded once. A part with no picture gets a plain white one so it
    // still draws rather than silently vanishing.
    let mut textures = Vec::new();
    for (nth, image) in model.images.iter().enumerate() {
        textures.push(renderer.create_texture_smooth(
            &format!("model image {nth}"),
            image.width,
            image.height,
            &image.rgba,
        )?);
    }
    let white = renderer.create_texture("model plain", 1, 1, &[255; 4])?;

    let palette: Vec<Mat4> = pose.palette().to_vec();
    let parts: Vec<(Vec<ModelVertex>, &[u32], dark_render::TextureId, bool)> = model
        .parts
        .iter()
        .map(|part| {
            let vertices = part
                .vertices
                .iter()
                .map(|v| ModelVertex {
                    position: v.position.to_array(),
                    normal: v.normal.to_array(),
                    uv: v.uv,
                    joints: v.joints,
                    weights: v.weights.to_array(),
                })
                .collect();
            let texture = part
                .image
                .and_then(|i| textures.get(i).copied())
                .unwrap_or(white);
            (
                vertices,
                part.indices.as_slice(),
                texture,
                part.double_sided,
            )
        })
        .collect();
    let draws: Vec<ModelDraw> = parts
        .iter()
        .map(|(vertices, indices, texture, double_sided)| ModelDraw {
            vertices,
            indices,
            palette: &palette,
            texture: *texture,
            double_sided: *double_sided,
        })
        .collect();

    renderer.render_models(camera, Light::default(), Some([0.22, 0.26, 0.24]), &draws);
    let capture = renderer.capture();
    let image = image::RgbaImage::from_raw(capture.width, capture.height, capture.rgba)
        .ok_or("the captured frame is the wrong size")?;
    image.save(out)?;

    let triangles: usize = model.parts.iter().map(|p| p.indices.len() / 3).sum();
    println!(
        "{out}: {clip} at tick {tick} on the GPU, {triangles} triangles, {} bones ({}x{})",
        model.skeleton.bones.len(),
        capture.width,
        capture.height
    );
    Ok(())
}
