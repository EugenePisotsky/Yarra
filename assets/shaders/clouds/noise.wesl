// The clouds' noise volumes (crates/atmosphere/src/clouds), generated once on the GPU and tiling
// seamlessly, after Guerrilla's Nubis. The base shape is Perlin noise dilated by Worley noise,
// which rounds blobs into billowing masses, eroded by Worley noise over three octaves, and spread
// over 0..1 so that the share of the field above 1 - c is about c: coverage reads as the share of
// sky clouded. The detail is Worley noise over three octaves, spread over 0..1, which eats a
// cloud's edges into billows above and wisps below. Each keeps its value in r: rgba8unorm is the
// format every device both writes from a compute pass and filters.
@group(0) @binding(0) var shape: texture_storage_3d<rgba8unorm, write>;
@group(0) @binding(1) var detail: texture_storage_3d<rgba8unorm, write>;

// The base shape's billows and erosion blend, and the 5th and 95th percentiles of that blend and
// of the detail's Worley noise over their tiles, measured from this generator.
const BILLOWS: f32 = 0.65;
const SHAPE_RANGE: vec2<f32> = vec2(0.537, 0.763);
const DETAIL_RANGE: vec2<f32> = vec2(0.292, 0.676);

// Three numbers 0..1 from a lattice point (pcg3d).
fn hash3(c: vec3<u32>) -> vec3<f32> {
    var v = c * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec3<f32>(v) * (1.0 / 4294967295.0);
}

// A lattice point wrapped to `period` cells, so the tile repeats, with a seed apart.
fn lattice(c: vec3<i32>, period: i32, seed: u32) -> vec3<u32> {
    return vec3<u32>(((c % period) + period) % period) + vec3(seed);
}

// Worley noise of `cells` cells a side over the unit tile: 1 at a cell's point, 0 a cell away.
fn worley(p: vec3<f32>, cells: i32, seed: u32) -> f32 {
    let q = p * f32(cells);
    let base = vec3<i32>(floor(q));
    var nearest = 1.0e9;
    for (var z = -1; z <= 1; z += 1) {
        for (var y = -1; y <= 1; y += 1) {
            for (var x = -1; x <= 1; x += 1) {
                let c = base + vec3(x, y, z);
                let d = q - (vec3<f32>(c) + hash3(lattice(c, cells, seed)));
                nearest = min(nearest, dot(d, d));
            }
        }
    }
    return 1.0 - clamp(sqrt(nearest), 0.0, 1.0);
}

// Worley noise over three octaves from `cells`, 0..1.
fn worley_fbm(p: vec3<f32>, cells: i32, seed: u32) -> f32 {
    return worley(p, cells, seed) * 0.625 + worley(p, cells * 2, seed + 7u) * 0.25
        + worley(p, cells * 4, seed + 13u) * 0.125;
}

// Perlin noise of `cells` cells a side over the unit tile, about -0.7..0.7.
fn perlin(p: vec3<f32>, cells: i32, seed: u32) -> f32 {
    let q = p * f32(cells);
    let i = vec3<i32>(floor(q));
    let f = q - floor(q);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    var corners: array<f32, 8>;
    for (var k = 0; k < 8; k += 1) {
        let o = vec3(k & 1, (k >> 1) & 1, (k >> 2) & 1);
        let g = normalize(hash3(lattice(i + o, cells, seed)) * 2.0 - 1.0 + vec3(1e-4));
        corners[k] = dot(g, f - vec3<f32>(o));
    }
    let x0 = mix(mix(corners[0], corners[1], u.x), mix(corners[2], corners[3], u.x), u.y);
    let x1 = mix(mix(corners[4], corners[5], u.x), mix(corners[6], corners[7], u.x), u.y);
    return mix(x0, x1, u.z);
}

// Perlin noise over four octaves from `cells`, 0..1.
fn perlin_fbm(p: vec3<f32>, cells: i32, seed: u32) -> f32 {
    var sum = 0.0;
    var amplitude = 1.0;
    var total = 0.0;
    for (var k = 0; k < 4; k += 1) {
        sum += perlin(p, cells << u32(k), seed + u32(k) * 5u) * amplitude;
        total += amplitude;
        amplitude *= 0.5;
    }
    return clamp(sum / total * 0.7 + 0.5, 0.0, 1.0);
}

fn texel(id: vec3<u32>, size: vec3<u32>) -> vec3<f32> {
    return (vec3<f32>(id) + 0.5) / vec3<f32>(size);
}

@compute @workgroup_size(4, 4, 4)
fn generate_shape(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(shape);
    if any(id >= size) {
        return;
    }
    let p = texel(id, size);
    let octaves = vec3(worley_fbm(p, 4, 17u), worley_fbm(p, 8, 31u), worley_fbm(p, 16, 47u));
    // Perlin dilated by Worley: low Perlin values rise to the Worley noise.
    let billows = clamp(mix(octaves.x, 1.0, perlin_fbm(p, 4, 0u)), 0.0, 1.0);
    let erosion = dot(octaves, vec3(0.625, 0.25, 0.125));
    let blend = mix(erosion, billows, BILLOWS);
    let base = clamp((blend - SHAPE_RANGE.x) / (SHAPE_RANGE.y - SHAPE_RANGE.x), 0.0, 1.0);
    textureStore(shape, id, vec4(base, 0.0, 0.0, 1.0));
}

@compute @workgroup_size(4, 4, 4)
fn generate_detail(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(detail);
    if any(id >= size) {
        return;
    }
    let fine = worley_fbm(texel(id, size), 2, 61u);
    let spread = clamp((fine - DETAIL_RANGE.x) / (DETAIL_RANGE.y - DETAIL_RANGE.x), 0.0, 1.0);
    textureStore(detail, id, vec4(spread, 0.0, 0.0, 1.0));
}
