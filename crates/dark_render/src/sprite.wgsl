// Sprite passes: instanced quads into the low-resolution target.
//   fs_sprite      main pass (plain sprites, character bodies, blob shadows, debug outlines)
//   fs_mask        occluders write their draw order into the occlusion mask (max blend)
//   fs_silhouette  character bodies, only where the mask holds a later draw order

struct Globals {
    // World position of the target's top-left pixel.
    origin: vec2<f32>,
    // Target size in pixels.
    size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;
@group(2) @binding(0) var occlusion_mask: texture_2d<f32>;

// Draw modes; must match `sprite.rs`.
const MODE_PLAIN: u32 = 0u;
const MODE_BLOB: u32 = 1u;
const MODE_BODY: u32 = 2u;
const MODE_CIRCLE: u32 = 3u;
const MODE_RECT: u32 = 4u;

// Alpha at or above which a pixel is the object itself rather than its baked soft shadow.
const OPAQUE: f32 = 0.6;
// Silhouette tint; the sprite's own brightness shades it so the figure stays readable.
const SILHOUETTE: vec3<f32> = vec3(0.55, 0.8, 1.0);
const SILHOUETTE_ALPHA: f32 = 0.6;

struct Instance {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_size: vec2<f32>,
    @location(4) tiles: vec2<f32>,
    @location(5) color: vec4<f32>,
    @location(6) order: f32,
    @location(7) mode: u32,
};

struct SpriteOut {
    @builtin(position) clip: vec4<f32>,
    // Position inside the quad in source tiles, so repeated textures wrap per tile. For 1×1
    // white sprites the tile is one pixel, so this is also the pixel position in the quad.
    @location(0) local: vec2<f32>,
    @location(1) uv_min: vec2<f32>,
    @location(2) uv_size: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) tiles: vec2<f32>,
    @location(5) @interpolate(flat) order: f32,
    @location(6) @interpolate(flat) mode: u32,
};

const CORNERS = array<vec2<f32>, 6>(
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

@vertex
fn vs_sprite(@builtin(vertex_index) vertex: u32, instance: Instance) -> SpriteOut {
    let corner = CORNERS[vertex];
    let world = instance.pos + corner * instance.size;
    let ndc = (world - globals.origin) / globals.size * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
    var out: SpriteOut;
    out.clip = vec4(ndc, 0.0, 1.0);
    out.local = corner * instance.tiles;
    out.uv_min = instance.uv_min;
    out.uv_size = instance.uv_size;
    out.color = instance.color;
    out.tiles = instance.tiles;
    out.order = instance.order;
    out.mode = instance.mode;
    return out;
}

fn texel(in: SpriteOut) -> vec4<f32> {
    let uv = in.uv_min + fract(in.local) * in.uv_size;
    return textureSample(sprite_texture, sprite_sampler, uv);
}

@fragment
fn fs_sprite(in: SpriteOut) -> @location(0) vec4<f32> {
    // Sample before any branch: derivatives need uniform control flow.
    let t = texel(in);
    switch in.mode {
        case MODE_BLOB: {
            let p = in.local / in.tiles * 2.0 - 1.0;
            if dot(p, p) > 1.0 { discard; }
            return in.color;
        }
        case MODE_BODY: {
            if t.a < OPAQUE { discard; }
        }
        case MODE_CIRCLE: {
            let radius = min(in.tiles.x, in.tiles.y) * 0.5;
            let d = length(in.local - in.tiles * 0.5);
            if d > radius || d < radius - 1.0 { discard; }
            return in.color;
        }
        case MODE_RECT: {
            let p = in.local;
            if p.x >= 1.0 && p.y >= 1.0 && p.x <= in.tiles.x - 1.0 && p.y <= in.tiles.y - 1.0 { discard; }
            return in.color;
        }
        default: {}
    }
    return t * in.color;
}

@fragment
fn fs_mask(in: SpriteOut) -> @location(0) vec4<f32> {
    let t = texel(in);
    // Soft shadows do not hide anything.
    if t.a * in.color.a < OPAQUE { discard; }
    return vec4(in.order, 0.0, 0.0, 1.0);
}

@fragment
fn fs_silhouette(in: SpriteOut) -> @location(0) vec4<f32> {
    let t = texel(in);
    if t.a < OPAQUE { discard; }
    let covered_by = textureLoad(occlusion_mask, vec2<i32>(in.clip.xy), 0).r;
    if covered_by <= in.order + 0.5 { discard; }
    let brightness = dot(t.rgb, vec3(0.3, 0.59, 0.11));
    return vec4(SILHOUETTE * (0.45 + 0.75 * brightness), SILHOUETTE_ALPHA);
}
