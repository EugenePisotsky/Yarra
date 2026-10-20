// Terrain v2's clipmap (crates/terrain_render/src/clipmap/gpu.rs), shared by its vertex and
// fragment shaders. Level L holds samples 2^L leaf spacings apart in a toroidal `side`² layer:
// world sample s of the level is texel s mod side. Positions travel as a leaf sample index
// (integer, world-wide) and a fraction of a leaf spacing, so they stay exact anywhere in the
// world while render coordinates stay small.
struct ClipmapSettings {
    // Texels per side, levels, sample intervals per node, metres between leaf samples.
    grid: vec4<f32>,
    // Leaf range, morph start and end as shares of a level's range, normal LOD bias.
    ranges: vec4<f32>,
    // Macro variation's three scales in metres and its albedo strength.
    macro_scales: vec4<f32>,
    // Macro contrast, whether macro variation is on, control texels per side.
    ground: vec4<f32>,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> settings: ClipmapSettings;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var heights: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var normals: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var normal_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var state: texture_2d<u32>;

// State texels (gpu.rs, whose tests check these copies): eye, origin, a window per level, each
// level's residency rows, then the cell each wear atlas slot holds.
const STATE_WINDOWS: i32 = 2;
const STATE_RESIDENCY: i32 = 10;
const RESIDENCY_TEXELS: i32 = 8;
const STATE_WEAR: i32 = 74;
// gpu.rs WEAR_WINDOW: cells per side of the road wear window around the camera.
const WEAR_WINDOW: i32 = 5;

fn side() -> i32 { return i32(settings.grid.x); }
fn levels() -> u32 { return u32(settings.grid.y); }
fn intervals() -> i32 { return i32(settings.grid.z); }
fn leaf_spacing() -> f32 { return settings.grid.w; }

// The main camera in render space, for every pass: shadow views morph as the camera sees.
fn eye() -> vec3<f32> {
    return bitcast<vec3<f32>>(textureLoad(state, vec2(0, 0), 0).xyz);
}
// The render origin as a leaf sample index.
fn origin_samples() -> vec2<i32> {
    return bitcast<vec2<i32>>(textureLoad(state, vec2(1, 0), 0).xy);
}

// Render-space XZ of a leaf sample index.
fn render_xz(leaf: vec2<i32>) -> vec2<f32> {
    return vec2<f32>(leaf - origin_samples()) * leaf_spacing();
}

// Texel of a level's sample.
fn texel(sample: vec2<i32>) -> vec2<i32> {
    return sample & vec2(side() - 1);
}

// The node of `level` holding a leaf sample.
fn node_of(level: u32, leaf: vec2<i32>) -> vec2<i32> {
    return (leaf >> vec2(level)) >> vec2(countTrailingZeros(u32(intervals())));
}

// Whether a node is inside its level's window and resident.
fn resident(level: u32, node: vec2<i32>) -> bool {
    let window = bitcast<vec4<i32>>(textureLoad(state, vec2(STATE_WINDOWS + i32(level), 0), 0));
    let local = node - window.xy;
    if any(local < vec2(0)) || any(local >= vec2(window.z)) {
        return false;
    }
    let n = side() / intervals();
    let slot = ((node % n) + n) % n;
    let word = textureLoad(state, vec2(STATE_RESIDENCY + i32(level) * RESIDENCY_TEXELS + slot.y / 4, 0), 0)[slot.y % 4];
    return ((word >> u32(slot.x)) & 1u) != 0u;
}

fn unpack_normal(xz: vec2<f32>) -> vec3<f32> {
    return vec3(xz.x, sqrt(max(1.0 - dot(xz, xz), 0.0)), xz.y);
}

// The filtered normal of `level` at a leaf sample plus a fraction of one.
fn level_normal(level: u32, leaf: vec2<i32>, fraction: vec2<f32>) -> vec3<f32> {
    let sample = leaf >> vec2(level);
    let rest = leaf - (sample << vec2(level));
    let position = vec2<f32>(texel(sample)) + (vec2<f32>(rest) + fraction) / f32(1u << level);
    let uv = (position + 0.5) / f32(side());
    return unpack_normal(textureSampleLevel(normals, normal_sampler, uv, level, 0.0).xy);
}

// The first resident level at or above `level`; the top level always covers the world.
fn resident_level(level: u32, leaf: vec2<i32>) -> u32 {
    let top = levels() - 1u;
    var l = min(level, top);
    while l < top && !resident(l, node_of(l, leaf)) {
        l += 1u;
    }
    return l;
}

// The ground normal for a footprint of 2^lod leaf spacings, blended between the two resident
// levels around it as a mip chain would be.
fn clipmap_normal(leaf: vec2<i32>, fraction: vec2<f32>, lod: f32) -> vec3<f32> {
    let top = levels() - 1u;
    let fine = resident_level(u32(clamp(floor(lod), 0.0, f32(top))), leaf);
    let coarse = resident_level(min(fine + 1u, top), leaf);
    let t = clamp(lod - f32(fine), 0.0, 1.0);
    let n = level_normal(fine, leaf, fraction);
    if t <= 0.0 || coarse == fine {
        return n;
    }
    return normalize(mix(n, level_normal(coarse, leaf, fraction), t));
}

// The wear atlas layer holding a cell's road wear, or -1.
fn wear_slot(cell: vec2<i32>) -> i32 {
    let n = WEAR_WINDOW;
    let slot = ((cell.x % n) + n) % n + n * (((cell.y % n) + n) % n);
    let held = textureLoad(state, vec2(STATE_WEAR + slot, 0), 0);
    if held.z == 0u || any(bitcast<vec2<i32>>(held.xy) != cell) {
        return -1;
    }
    return slot;
}

// Leaf-level ground height at a leaf sample plus a fraction of one (which may exceed one),
// bilinearly over the triangles' corners.
fn leaf_height(leaf: vec2<i32>, fraction: vec2<f32>) -> f32 {
    let whole = floor(fraction);
    let s = leaf + vec2<i32>(whole);
    let f = fraction - whole;
    let a = textureLoad(heights, texel(s), 0, 0).r;
    let b = textureLoad(heights, texel(s + vec2(1, 0)), 0, 0).r;
    let c = textureLoad(heights, texel(s + vec2(0, 1)), 0, 0).r;
    let d = textureLoad(heights, texel(s + vec2(1, 1)), 0, 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}
