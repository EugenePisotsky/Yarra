// The trample map (vegetation_render renderer/trample.rs): each texel of a world-aligned texture
// wrapping around the camera holds how far the grass rooted there is pressed down (z) and the
// way it lies (xy, that direction times z). Characters press a disc under them, laying blades
// along their motion and outward from their feet; pressed grass rises again over seconds.

struct Trample {
    // xy: canonical centre this frame, zw: last frame's
    centres: vec4<f32>,
    // x: metres per texel, y: texels per side, z: seconds since last frame, w: benders
    params: vec4<f32>,
    // Per bender: (canonical x, z, radius, 0), (velocity x, z, 0, 0).
    benders: array<vec4<f32>, 32>,
}

@group(0) @binding(0) var<uniform> trample: Trample;
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var current: texture_storage_2d<rgba16float, write>;

// Fresh presses start above 1 and read as fully flat until they decay to it (~2.4 s), then the
// blades rise with this time constant.
const PRESS: f32 = 1.5;
const RECOVERY_SECONDS: f32 = 6.0;
// Share of the disc's radius pressed fully flat; blades toward its rim are pushed partway.
const FLAT_SHARE: f32 = 0.45;

// The canonical position, along one axis, of the texel's ground nearest a centre.
fn nearest(cell: f32, centre: f32) -> f32 {
    let texel = trample.params.x;
    let side = trample.params.y;
    let k = round((centre / texel - cell - 0.5) / side);
    return (cell + 0.5 + k * side) * texel;
}

@compute @workgroup_size(8, 8, 1)
fn stamp(@builtin(global_invocation_id) id: vec3<u32>) {
    let side = u32(trample.params.y);
    if (id.x >= side || id.y >= side) {
        return;
    }
    let cell = vec2<f32>(id.xy);
    let here = vec2(nearest(cell.x, trample.centres.x), nearest(cell.y, trample.centres.y));
    let before = vec2(nearest(cell.x, trample.centres.z), nearest(cell.y, trample.centres.w));
    var value = vec4<f32>(0.0);
    // A texel the map's movement wrapped onto new ground starts unpressed.
    if (all(here == before)) {
        value = textureLoad(previous, vec2<i32>(id.xy), 0)
            * exp(-trample.params.z / RECOVERY_SECONDS);
    }
    for (var i = 0u; i < u32(trample.params.w); i += 1u) {
        let disc = trample.benders[2u * i];
        let velocity = trample.benders[2u * i + 1u].xy;
        let offset = here - disc.xy;
        let distance = length(offset);
        let press = (1.0 - smoothstep(disc.z * FLAT_SHARE, disc.z, distance)) * PRESS;
        if (press > value.z) {
            let radial = select(vec2<f32>(0.0), offset / distance, distance > 1e-4);
            let speed = length(velocity);
            let along = select(vec2<f32>(0.0), velocity / speed, speed > 0.2);
            // Laid along the motion, and outward more toward the rim: no parting down the middle.
            var lay = along * min(speed, 1.0) + radial * 0.6 * distance / disc.z;
            let length_squared = dot(lay, lay);
            lay = select(radial, lay * inverseSqrt(length_squared), length_squared > 1e-6);
            value = vec4<f32>(lay * press, press, 0.0);
        }
    }
    textureStore(current, vec2<i32>(id.xy), value);
}
