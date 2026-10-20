// Terrain v2's clipmap (crates/terrain_render/src/clipmap/gpu.rs), shared by its vertex and
// fragment shaders. Level L holds samples 2^L leaf spacings apart in a toroidal `side`² layer:
// world sample s of the level is texel s mod side. Positions travel as a leaf sample index
// (integer, world-wide) and a fraction of a leaf spacing, so they stay exact anywhere in the
// world while render coordinates stay small. Heights are the drawn ground, the top of the soft
// layer (snow) where there is one; the leaves' layer depths say how far it lies over the firm
// ground (world::soft_layer).
struct ClipmapSettings {
    // Texels per side, levels, sample intervals per node, metres between leaf samples.
    grid: vec4<f32>,
    // Leaf range, morph start and end as shares of a level's range, normal LOD bias.
    ranges: vec4<f32>,
    // Macro variation's three scales in metres and its albedo strength.
    macro_scales: vec4<f32>,
    // Macro contrast, whether macro variation is on, control texels per side.
    ground: vec4<f32>,
    // Range of the first detail level on soft ground (0: none).
    detail: vec4<f32>,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> settings: ClipmapSettings;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var heights: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var normals: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var normal_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var state: texture_2d<u32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var control: texture_2d_array<u32>;
// The leaves' soft layer depth, metres; filtered with the normals' sampler.
@group(constants::MATERIAL_BIND_GROUP) @binding(15) var soft_depths: texture_2d<f32>;

// State texels (gpu.rs, whose tests check these copies): eye, origin, a window per level, each
// level's residency rows, then the cell each wear atlas slot holds.
const STATE_WINDOWS: i32 = 2;
const STATE_RESIDENCY: i32 = 10;
const RESIDENCY_TEXELS: i32 = 8;
const STATE_WEAR: i32 = 74;
// gpu.rs WEAR_WINDOW: cells per side of the road wear window around the camera.
const WEAR_WINDOW: i32 = 5;
// select.rs DETAIL_LEVELS: levels below the leaves, on soft ground near the camera.
const DETAIL_LEVELS: i32 = 3;

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

// A packed control sample (world::pack_control): three layers of five bits, two shares of 255.
struct Sample { layers: vec3<u32>, shares: vec3<f32>, }

fn unpack(sample: u32) -> Sample {
    let a = f32((sample >> 16u) & 255u) / 255.0;
    let b = f32((sample >> 24u) & 255u) / 255.0;
    return Sample(vec3(sample & 31u, (sample >> 5u) & 31u, (sample >> 10u) & 31u),
        vec3(a, b, max(1.0 - a - b, 0.0)));
}

// The four packed control samples of `level` around a leaf sample plus a fraction of one (x, y,
// z, w: the cell's corners 00, 10, 01, 11), the fraction of the way between them, and metres
// between them: control samples lie half a height sample apart at every level.
struct ControlCell { packed: vec4<u32>, f: vec2<f32>, spacing: f32, }

fn control_cell(level: u32, leaf: vec2<i32>, fraction: vec2<f32>) -> ControlCell {
    let units = leaf * 2;
    let first = units >> vec2(level);
    let rest = units - (first << vec2(level));
    let q = (vec2<f32>(rest) + fraction * 2.0) / f32(1u << level);
    let whole = floor(q);
    let mask = vec2(i32(settings.ground.z) - 1);
    let t0 = (first + vec2<i32>(whole)) & mask;
    let t1 = (first + vec2<i32>(whole) + 1) & mask;
    let l = i32(level);
    let packed = vec4(textureLoad(control, t0, l, 0).r, textureLoad(control, vec2(t1.x, t0.y), l, 0).r,
        textureLoad(control, vec2(t0.x, t1.y), l, 0).r, textureLoad(control, t1, l, 0).r);
    return ControlCell(packed, q - whole, leaf_spacing() * f32(1u << level) * 0.5);
}

// How deep the leaves' soft layer lies over the firm ground at a leaf sample plus a fraction of
// one, bilinearly. Only resident leaves hold theirs.
fn soft_depth(leaf: vec2<i32>, fraction: vec2<f32>) -> f32 {
    let uv = (vec2<f32>(texel(leaf)) + fraction + 0.5) / f32(side());
    return textureSampleLevel(soft_depths, normal_sampler, uv, 0.0).r;
}

// The leaves' drawn ground at a leaf sample plus a fraction (0..1) of one: their triangles, split
// from each quad's (x, z) to (x + 1, z + 1) as patch_mesh splits them, so detail levels lie on
// the ground the leaves draw.
fn leaf_surface_height(leaf: vec2<i32>, f: vec2<f32>) -> f32 {
    let a = textureLoad(heights, texel(leaf), 0, 0).r;
    let b = textureLoad(heights, texel(leaf + vec2(0, 1)), 0, 0).r;
    let c = textureLoad(heights, texel(leaf + vec2(1, 1)), 0, 0).r;
    let d = textureLoad(heights, texel(leaf + vec2(1, 0)), 0, 0).r;
    if f.x >= f.y {
        return a + f.x * (d - a) + f.y * (c - d);
    }
    return a + f.y * (b - a) + f.x * (c - b);
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
