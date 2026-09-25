// Standing water on soaked ground, with ripple rings while rain falls. Called by the terrain
// shaders before shared PBR lighting, which adds general surface wetness on top.
#import bevy_pbr::pbr_types::PbrInput
#import bevy_pbr::mesh_view_bindings::{globals, view}
#import "shaders/clouds/surface.wgsl"::{rain_shelter, surface_origin, surface_weather}

fn cell_hash(cell: vec2<f32>) -> vec2<f32> {
    let c = vec2<u32>(vec2<i32>(cell) + vec2(1 << 15));
    var h = c.x * 0x8da6b343u ^ c.y * 0xd8163841u;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    let g = (h ^ (h >> 13u)) * 0x846ca68bu;
    return vec2(f32(h & 0xffffu), f32(g >> 16u)) / 65535.0;
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let a = cell_hash(i).x;
    let b = cell_hash(i + vec2(1.0, 0.0)).x;
    let c = cell_hash(i + vec2(0.0, 1.0)).x;
    let d = cell_hash(i + vec2(1.0, 1.0)).x;
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Horizontal normal offset of expanding rings: one drop per cell per cycle.
fn ripples(p: vec2<f32>, time: f32) -> vec2<f32> {
    var offset = vec2(0.0);
    let base = floor(p - 0.5);
    for (var y = 0; y < 2; y += 1) {
        for (var x = 0; x < 2; x += 1) {
            let cell = base + vec2(f32(x), f32(y));
            let random = cell_hash(cell);
            let centre = cell + random;
            let age = fract(time * 1.3 + random.y);
            let to_point = p - centre;
            let distance = length(to_point);
            let ring = (distance - age * 0.9) * 22.0;
            if abs(ring) < 3.14159 && distance > 1e-4 {
                offset += to_point / distance * sin(ring) * (1.0 - age) * (1.0 - age);
            }
        }
    }
    return offset;
}

// Beyond this, puddles are too small on screen to justify their shading.
const PUDDLE_DISTANCE: f32 = 80.0;

// `hollow` is baked depth below the surrounding ground (0..1, saturating at 15 cm), or
// negative where the terrain path has no baked relief.
fn apply_rain_puddles(input: PbrInput, hollow: f32) -> PbrInput {
    var pbr = input;
    let weather = surface_weather();
    if weather.x <= 0.3 {
        return pbr;
    }
    let distance = length(pbr.world_position.xyz - view.world_position);
    if distance > PUDDLE_DISTANCE {
        return pbr;
    }
    let shelter = rain_shelter(pbr.world_position.xyz);
    let wetness = clamp(weather.x, 0.0, 1.0) * shelter;
    if wetness <= 0.3 {
        return pbr;
    }
    let p = pbr.world_position.xz + surface_origin();
    let fill = smoothstep(0.3, 1.0, wetness);
    let breakup = value_noise(p * 0.61);
    let pattern = value_noise(p * 0.18) * 0.65 + breakup * 0.35;
    var puddle: f32;
    var depth = 0.0;
    if hollow >= 0.0 {
        // Water rises into ruts and dips as the ground soaks, deepest first; when soaked,
        // hollows about 4 cm deep hold water. Water is level, so deep hollows accept their
        // sloped walls. A few sparse puddles remain on open flat ground.
        depth = hollow;
        let level = 1.0 - 0.8 * fill;
        let collected = smoothstep(level, level + 0.1, hollow + (breakup - 0.5) * 0.15);
        let open = smoothstep(0.84 - 0.08 * fill, 0.87 - 0.08 * fill, pattern);
        puddle = max(collected, open * 0.8);
    } else {
        // No baked relief: puddles from noise, about 2% of flat ground at first, 10% soaked.
        let threshold = 0.80 - 0.11 * fill;
        puddle = smoothstep(threshold, threshold + 0.03, pattern);
    }
    let flat = smoothstep(0.94 - 0.2 * depth, 0.99 - 0.1 * depth, pbr.world_normal.y);
    puddle *= flat * (1.0 - smoothstep(0.75 * PUDDLE_DISTANCE, PUDDLE_DISTANCE, distance));
    if puddle <= 0.0 {
        return pbr;
    }
    pbr.material.base_color = vec4(
        pbr.material.base_color.rgb * mix(1.0, 0.35, puddle),
        pbr.material.base_color.a,
    );
    pbr.material.perceptual_roughness = mix(pbr.material.perceptual_roughness, 0.05, puddle);
    // Water is level: replace the ground's normal, then add live ripples.
    var normal = mix(pbr.N, vec3(0.0, 1.0, 0.0), puddle);
    let rain = clamp(weather.y, 0.0, 1.0) * shelter;
    if rain > 0.0 {
        let ring = ripples(p * 3.0, globals.time) * 0.18 * rain * puddle;
        normal += vec3(ring.x, 0.0, ring.y);
    }
    pbr.N = normalize(normal);
    pbr.clearcoat_N = pbr.N;
    return pbr;
}
