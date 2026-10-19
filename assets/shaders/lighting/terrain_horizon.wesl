// The terrain shadow map's searches (crates/atmosphere/src/terrain_shadow/gpu.rs), a band of rows
// a dispatch. `sky`: the share of the sky the terrain around each texel leaves open, once per
// relief, beside its height. `horizon`: the highest horizon towards the map's light, its tangent
// and distance, with the height and sky share, into the map lit materials sample
// (lighting/terrain_shadow.wesl). The searches match the CPU reference in terrain_shadow.rs.
import package::shaders::environment::params::EnvironmentParams;

@group(0) @binding(0) var<storage, read> environment: EnvironmentParams;
// First row and row count of this dispatch.
@group(0) @binding(1) var<uniform> band: vec4<u32>;
@group(0) @binding(2) var heights: texture_2d<f32>;
@group(0) @binding(3) var relief_out: texture_storage_2d<rg32float, write>;
@group(0) @binding(4) var relief: texture_2d<f32>;
@group(0) @binding(5) var map: texture_storage_2d<rgba16float, write>;

const HORIZON_GROWTH: f32 = 1.08;
const HORIZON_REACH: f32 = 6000.0;
const SKY_DIRECTIONS: u32 = 8u;
const SKY_GROWTH: f32 = 1.15;
const SKY_REACH: f32 = 600.0;

// Bilinear height at texel coordinates (texel centres at whole numbers), clamped to the grid.
fn height_at(field: texture_2d<f32>, p: vec2<f32>) -> f32 {
    let last = vec2<i32>(textureDimensions(field)) - 1;
    let q = clamp(p, vec2(0.0), vec2<f32>(last));
    let p0 = vec2<i32>(q);
    let p1 = min(p0 + 1, last);
    let f = q - vec2<f32>(p0);
    let low = mix(textureLoad(field, p0, 0).x, textureLoad(field, vec2(p1.x, p0.y), 0).x, f.x);
    let high = mix(textureLoad(field, vec2(p0.x, p1.y), 0).x, textureLoad(field, p1, 0).x, f.x);
    return mix(low, high, f.y);
}

// The highest horizon from texel `at` towards `direction` (texel units, unit length): the
// tangent of its elevation and how far off it lies in metres.
fn search(field: texture_2d<f32>, at: vec2<u32>, direction: vec2<f32>, growth: f32, reach: f32) -> vec2<f32> {
    let spacing = environment.terrain_shadow.z;
    let last = vec2<f32>(textureDimensions(field)) - 1.0;
    let here = textureLoad(field, vec2<i32>(at), 0).x;
    var best = -3.0e38;
    var distance = reach;
    var r = 1.0;
    while r * spacing <= reach {
        let s = vec2<f32>(at) + direction * r;
        // As the reference: a thousandth of a texel of slack at the grid's edges.
        if any(s < vec2(-1e-3)) || any(s > last + 1e-3) {
            break;
        }
        let tangent = (height_at(field, s) - here) / (r * spacing);
        if tangent > best {
            best = tangent;
            distance = r * spacing;
        }
        r = max(r * growth, r + 1.0);
    }
    if best < -1.0e38 {
        return vec2(0.0, reach);
    }
    return vec2(best, distance);
}

@compute @workgroup_size(8, 8)
fn sky(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2(id.x, id.y + band.x);
    if at.x >= textureDimensions(heights).x || id.y >= band.y {
        return;
    }
    var hidden = 0.0;
    for (var i = 0u; i < SKY_DIRECTIONS; i += 1u) {
        let angle = 6.2831855 * f32(i) / f32(SKY_DIRECTIONS);
        let t = max(search(heights, at, vec2(cos(angle), sin(angle)), SKY_GROWTH, SKY_REACH).x, 0.0);
        hidden += t * t / (1.0 + t * t);
    }
    let height = textureLoad(heights, vec2<i32>(at), 0).x;
    textureStore(relief_out, at, vec4(height, 1.0 - hidden / f32(SKY_DIRECTIONS), 0.0, 0.0));
}

@compute @workgroup_size(8, 8)
fn horizon(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2(id.x, id.y + band.x);
    if at.x >= textureDimensions(relief).x || id.y >= band.y {
        return;
    }
    let h = search(relief, at, normalize(environment.terrain_light.xy), HORIZON_GROWTH, HORIZON_REACH);
    let here = textureLoad(relief, vec2<i32>(at), 0);
    textureStore(map, at, vec4(clamp(h.x, -4.0, 16.0), here.x, here.y, h.y));
}
