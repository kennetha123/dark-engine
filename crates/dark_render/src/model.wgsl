// Skinned 3D meshes (docs/PLAN.md §16.3). Sprites are drawn by sprite.wgsl in their own pass,
// by painter's algorithm; a mesh is solid and sees itself from every side, so it is drawn here
// against a depth buffer instead.

struct Camera {
    // World to clip. The view and the projection are multiplied on the CPU, once a frame.
    view_projection: mat4x4<f32>,
    // Where the light comes from, in world space, and how dark the shadowed side is left.
    light: vec4<f32>,
};

// `bones` already carries the model's own placement in the world: the palette is built as
// `model * world * inverse_bind`, so the shader multiplies one matrix per vertex and not three.
struct Bones {
    matrices: array<mat4x4<f32>, 128>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var model_texture: texture_2d<f32>;
@group(1) @binding(1) var model_sampler: sampler;
@group(2) @binding(0) var<uniform> bones: Bones;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
};

fn skin_of(in: VertexIn) -> mat4x4<f32> {
    // The weights are normalised when the model is loaded, so they sum to one and this is an
    // average rather than a sum that could grow the mesh.
    var skin = bones.matrices[in.joints.x] * in.weights.x;
    skin += bones.matrices[in.joints.y] * in.weights.y;
    skin += bones.matrices[in.joints.z] * in.weights.z;
    skin += bones.matrices[in.joints.w] * in.weights.w;
    return skin;
}

@vertex
fn vs_model(in: VertexIn) -> VertexOut {
    let skin = skin_of(in);
    let world = skin * vec4<f32>(in.position, 1.0);

    var out: VertexOut;
    out.clip_position = camera.view_projection * world;
    out.uv = in.uv;
    // Good enough while every bone is rigid: a skinned normal wants the inverse transpose, and
    // a bone that only turns and moves is the same either way. Scaling a bone would need more.
    out.normal = normalize((skin * vec4<f32>(in.normal, 0.0)).xyz);
    return out;
}

@fragment
fn fs_model(in: VertexOut) -> @location(0) vec4<f32> {
    let colour = textureSample(model_texture, model_sampler, in.uv);
    // Cut out rather than blended: a depth-tested mesh cannot sort its own transparency, and a
    // half-lit edge written to the depth buffer would hide whatever came after it.
    if colour.a < 0.5 {
        discard;
    }
    let towards = normalize(camera.light.xyz);
    let lit = camera.light.w + (1.0 - camera.light.w) * max(dot(in.normal, towards), 0.0);
    return vec4<f32>(colour.rgb * lit, 1.0);
}
