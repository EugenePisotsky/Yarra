// One blade's shape and wind pose, after Ghost of Tsushima's grass: a cubic Bézier whose tip
// leans by the blade's tilt and whose middle arches by its bend, with height, width, facing and
// colour varying per blade and per clump. The wind pushes the tip and the blade keeps its length.
import package::shaders::vegetation::types::{
    Instance, GrassType, Camera, PI, TYPE_INDEX_MASK, LOD_MORPH_MASK, LOD_MORPH_SHIFT,
};
import package::shaders::vegetation::random::{hash32, random01};

// Largest lean of the tip from upright, at tilt 1, in radians: nearly lying on the ground.
const MAX_LEAN: f32 = 1.45;
// How far grass grows along the ground's normal rather than straight up.
const SLOPE_FOLLOW: f32 = 0.35;

fn normalize3_or(value: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(value, value);
    if (length_squared <= 1e-10) {
        return fallback;
    }
    return value * inverseSqrt(length_squared);
}

fn cubic_bezier(p0: vec3<f32>, p1: vec3<f32>, p2: vec3<f32>, p3: vec3<f32>, t: f32) -> vec3<f32> {
    let u = 1.0 - t;
    return u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3;
}

fn cubic_bezier_derivative(
    p0: vec3<f32>, p1: vec3<f32>, p2: vec3<f32>, p3: vec3<f32>, t: f32,
) -> vec3<f32> {
    let u = 1.0 - t;
    return 3.0 * u * u * (p1 - p0) + 6.0 * u * t * (p2 - p1) + 3.0 * t * t * (p3 - p2);
}

// What one instance decodes to.
struct BladeInstance {
    root: vec3<f32>,
    surface_normal: vec3<f32>,
    // Unit horizontal facing.
    facing: vec2<f32>,
    // 0..1 key of the blade's clump, and the blade's distance from its centre (share).
    clump: f32,
    clump_distance: f32,
    type_index: u32,
    seed: u32,
    // 1 high-detail, morphing toward the low mesh as it falls to 0.
    lod_morph: f32,
    low: bool,
    // Share of the population kept here, and this blade's rank in it.
    density: f32,
    lod_rank: f32,
}

fn decode_instance(instance: Instance) -> BladeInstance {
    var blade: BladeInstance;
    blade.root = instance.root_clump.xyz;
    let clump_rank = unpack2x16unorm(bitcast<u32>(instance.root_clump.w));
    blade.clump = clump_rank.x;
    blade.lod_rank = clump_rank.y;
    let facing_clump = unpack2x16unorm(instance.geometry.x);
    let angle = facing_clump.x * 2.0 * PI;
    blade.facing = vec2<f32>(cos(angle), sin(angle));
    blade.clump_distance = facing_clump.y;
    blade.type_index = instance.geometry.y & TYPE_INDEX_MASK;
    blade.lod_morph = f32((instance.geometry.y >> LOD_MORPH_SHIFT) & LOD_MORPH_MASK)
        / f32(LOD_MORPH_MASK);
    blade.low = (instance.geometry.y >> 31u) != 0u;
    let normal_xz = unpack2x16snorm(instance.geometry.z);
    blade.surface_normal = normalize3_or(
        vec3<f32>(normal_xz.x, sqrt(max(0.0, 1.0 - dot(normal_xz, normal_xz))), normal_xz.y),
        vec3<f32>(0.0, 1.0, 0.0),
    );
    blade.seed = instance.geometry.w & 0x00ffffffu;
    blade.density = f32(instance.geometry.w >> 24u) / 255.0;
    return blade;
}

// A blade at rest: its frame, size and colours.
struct Blade {
    root: vec3<f32>,
    up: vec3<f32>,
    forward: vec3<f32>,
    side: vec3<f32>,
    height: f32,
    width: f32,
    // Tip lean in radians and arch as a share of height.
    lean: f32,
    bend: f32,
    side_curve: f32,
    root_color: vec3<f32>,
    tip_color: vec3<f32>,
    // Per-blade wind phase, 0..1.
    phase: f32,
}

fn rest_blade(b: BladeInstance, grass: GrassType) -> Blade {
    let seed = hash32(b.seed);
    let clump = hash32(u32(round(b.clump * 65535.0)) ^ bitcast<u32>(grass.placement.z));
    var blade: Blade;
    blade.root = b.root;
    blade.up = normalize3_or(mix(vec3(0.0, 1.0, 0.0), b.surface_normal, SLOPE_FOLLOW), b.surface_normal);
    let facing = vec3<f32>(b.facing.x, 0.0, b.facing.y);
    blade.forward = normalize3_or(facing - blade.up * dot(facing, blade.up), vec3(1.0, 0.0, 0.0));
    blade.side = normalize3_or(cross(blade.up, blade.forward), vec3(0.0, 0.0, 1.0));

    // Clumps differ in height; their edges are a little shorter, more so in tufted types.
    let clump_height = 1.0 - 0.5 * grass.clumps.z * random01(clump ^ 0x52dce729u);
    let edge = 1.0 - 0.35 * grass.clumps.y * b.clump_distance * b.clump_distance;
    blade.height = mix(grass.size.x, grass.size.y, random01(seed ^ 0xa511e9b3u))
        * clump_height * edge;
    blade.width = mix(grass.size.z, grass.size.w, random01(seed ^ 0x63d83595u));
    blade.lean = mix(grass.shape.x, grass.shape.y, random01(seed ^ 0x27d4eb2fu)) * MAX_LEAN;
    blade.bend = mix(grass.shape.z, grass.shape.w, random01(seed ^ 0x165667b1u));
    blade.side_curve = (random01(seed ^ 0x94d049bbu) * 2.0 - 1.0) * 0.18;
    blade.phase = random01(seed ^ 0x3c6ef372u);

    // Colour: a clump-wide brightness, dry tips in some clumps, and a little per blade.
    let dry = random01(clump ^ 0x7b7d159cu) < grass.root.w;
    let brightness = 1.0 + (random01(clump ^ 0x94d049bbu) * 2.0 - 1.0) * grass.tip.w
        + (random01(seed ^ 0x68e31da4u) - 0.5) * 0.12;
    blade.root_color = grass.root.xyz * brightness;
    blade.tip_color = select(grass.tip.xyz, grass.dry_tip.xyz, dry) * brightness;
    return blade;
}

// The curve's four control points.
struct Curve {
    p0: vec3<f32>,
    p1: vec3<f32>,
    p2: vec3<f32>,
    p3: vec3<f32>,
}

// Root handle rising steeply, tip at the lean, the tip handle raised by the bend: blades rise,
// arch and droop toward their tips, as tilt and bend grow.
fn rest_curve(blade: Blade) -> Curve {
    let h = blade.height;
    let chord = blade.up * cos(blade.lean) + blade.forward * sin(blade.lean);
    let arch = -blade.forward * cos(blade.lean) + blade.up * sin(blade.lean);
    let p0 = blade.root;
    let p3 = p0 + chord * h + blade.side * blade.side_curve * h * 0.5;
    // The root leaves the ground partway toward the lean, never as an upright stub.
    let rise = normalize3_or(mix(blade.up, chord, 0.45), blade.up);
    let p1 = p0 + rise * h * (0.3 + 0.3 * blade.bend);
    let p2 = p3 - chord * h * 0.3 + arch * h * 0.35 * blade.bend
        + blade.side * blade.side_curve * h * 0.4;
    return Curve(p0, p1, p2, p3);
}

fn polygon_length(c: Curve) -> f32 {
    return distance(c.p0, c.p1) + distance(c.p1, c.p2) + distance(c.p2, c.p3);
}

fn lattice_value(cell: vec2<i32>) -> f32 {
    return random01((bitcast<u32>(cell.x) * 0x8da6b343u) ^ (bitcast<u32>(cell.y) * 0xd8163841u));
}

fn value_noise(p: vec2<f32>) -> f32 {
    let cell = vec2<i32>(floor(p));
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(lattice_value(cell), lattice_value(cell + vec2(1, 0)), w.x),
        mix(lattice_value(cell + vec2(0, 1)), lattice_value(cell + vec2(1, 1)), w.x),
        w.y,
    );
}

// The broad field trees follow too (tree/wind.wesl `field_force`, VegetationWind::sample_force):
// a travelling wave with gust pulses, as a share of the wind's strength.
fn field_force(canonical_xz: vec2<f32>, camera: Camera) -> f32 {
    let direction = camera.wind.xy;
    let perpendicular = vec2(-direction.y, direction.x);
    let frequency = camera.wind_shape.x;
    let speed = camera.wind_shape.y;
    let broad = dot(canonical_xz, direction) * frequency - camera.wind.w * speed;
    let across = dot(canonical_xz, perpendicular) * frequency * 0.71 + camera.wind.w * speed * 0.37;
    let broad_amount = sin(broad + sin(across) * 0.85) * 0.5 + 0.5;
    let gust = clamp(sin(broad * 0.43 - across * 0.61) * 0.5 + 0.5, 0.0, 1.0);
    let rise = clamp((gust - 0.28) / 0.72, 0.0, 1.0);
    let pulse = rise * rise * (3.0 - 2.0 * rise);
    return clamp(0.25 + broad_amount * 0.18 + pulse * camera.wind_shape.z * 0.90, 0.12, 1.30);
}

// Metres per second at which grass-scale gusts roll downwind across a field.
const GUST_TRAVEL: f32 = 3.2;

// Gusts a few metres across rolling downwind over the grass, 0..1: the waves that sweep a field.
fn grass_gust(canonical_xz: vec2<f32>, camera: Camera) -> f32 {
    let p = canonical_xz - camera.wind.xy * camera.wind.w * GUST_TRAVEL;
    let n = value_noise(p / 7.5) * 0.65 + value_noise(p / 2.7 + vec2(17.3, 5.1)) * 0.35;
    return smoothstep(0.2, 0.85, n);
}

// The rest curve pushed downwind. The tip moves most; the root stays. Each blade also bobs at
// its own rate, neighbours along the wind in phase, and flutters across it. The control polygon
// then scales back to its rest length, so pushed blades bow over rather than stretch.
// Every oscillation's rate is constant: `time` grows without bound (to 4096 s), so a rate that
// varies over space or time would make the phase race and the blades vibrate. Gusts change
// amplitudes only.
fn wind_curve(rest: Curve, blade: Blade, grass: GrassType, camera: Camera) -> Curve {
    let strength = camera.wind.z;
    if (strength <= 1e-5) {
        return rest;
    }
    let canonical = blade.root.xz + camera.render_origin.xy;
    let force = field_force(canonical, camera);
    let gust = grass_gust(canonical, camera);
    let response = strength * (1.0 - 0.75 * grass.material.w);
    let wind = vec3<f32>(camera.wind.x, 0.0, camera.wind.y);
    let downwind = normalize3_or(wind - blade.up * dot(wind, blade.up), blade.forward);
    let across = cross(blade.up, downwind);
    let time = camera.wind.w;
    let bob = sin(time * (3.2 + blade.phase * 1.8) + blade.phase * 2.0 * PI
        + dot(canonical, camera.wind.xy) * 0.9);
    let flutter = sin(time * 7.3 + blade.phase * 11.0) * camera.wind_shape.w;
    let push = force * (0.35 + 0.65 * gust) + bob * (0.06 + 0.08 * gust);
    let offset = (downwind * push + across * flutter * 0.06) * response * blade.height;
    var c = rest;
    c.p1 += offset * 0.15;
    c.p2 += offset * 0.65;
    c.p3 += offset;
    let k = polygon_length(rest) / max(polygon_length(c), 1e-4);
    c.p1 = c.p0 + (c.p1 - c.p0) * k;
    c.p2 = c.p0 + (c.p2 - c.p0) * k;
    c.p3 = c.p0 + (c.p3 - c.p0) * k;
    return c;
}
