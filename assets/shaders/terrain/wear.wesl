// Worn ground near the camera in terrain v2: roads, paths and junctions as the cook draws them into
// each page's wear field (world terrain_wear.rs `TerrainWear`), at 12.5 cm. The control tiles hold
// the ground already worn as `TerrainWear::lay` lays it; here bare road ground is re-sharpened from
// the field, ruts sink by parallax over its rut depth, grooves of many wheels run along its traffic
// direction, and rain stands in dips of the rut floors.

// Wear fields come in a texture array (terrain v2's wear atlas), one page a layer: wear,
// rut depth (of RUT_DEPTH_SCALE), and the traffic direction as a doubled angle (cosine and sine
// mapped to 0..1), at texel centres over the page. The terrain material reads them across pages
// (terrain/clipmap.wesl wear_texel).

// world terrain_wear.rs RUT_DEPTH_SCALE, and WEAR_RESOLUTION: texels a side of a page, 2^8.
const RUT_DEPTH_SCALE: f32 = 0.25;
const WEAR_TEXELS: i32 = 256;
const WEAR_SHIFT: u32 = 8u;

fn wear_bits(x: i32, z: i32, seed: u32) -> u32 {
    var v = (bitcast<u32>(x) * 0x8da6b343u) ^ (bitcast<u32>(z) * 0xd8163841u) ^ seed;
    v ^= v >> 16u; v *= 0x7feb352du; v ^= v >> 15u; v *= 0x846ca68bu; v ^= v >> 16u;
    return v;
}
fn wear_hash(x: i32, z: i32, seed: u32) -> f32 { return f32(wear_bits(x, z, seed)) * (1.0 / 4294967296.0); }
fn wear_quintic(t: f32) -> f32 { return t*t*t*(t*(t*6.0-15.0)+10.0); }
fn wear_value(p: vec2<f32>, seed: u32) -> f32 {
    let c = floor(p); let i = vec2<i32>(c);
    let u = wear_quintic(p.x-c.x); let v = wear_quintic(p.y-c.y);
    let a = mix(wear_hash(i.x,i.y,seed), wear_hash(i.x+1,i.y,seed), u);
    let b = mix(wear_hash(i.x,i.y+1,seed), wear_hash(i.x+1,i.y+1,seed), u);
    return mix(a, b, v);
}
// Domain-warped value noise, 0..1, in patches about `size` metres across.
fn wear_patch(world: vec2<f32>, size: f32, seed: u32) -> f32 {
    let p = world/size;
    let warp = vec2(wear_value(p*0.43, seed ^ 0x731u), wear_value(p*0.43, seed ^ 0x891u)) - 0.5;
    let q = p + warp*0.6;
    return wear_value(q, seed)*0.75 + wear_value(q*2.0, seed ^ 0x463u)*0.25;
}

// The field in a filtered texel `t`: wear, rut depth in metres, and the traffic direction (unit;
// its sign is arbitrary).
struct Wear { wear: f32, rut: f32, direction: vec2<f32>, }
fn decode_wear(t: vec4<f32>) -> Wear {
    let doubled = t.zw*2.0 - 1.0;
    let angle = atan2(doubled.y, doubled.x)*0.5;
    return Wear(t.x, t.y*RUT_DEPTH_SCALE, vec2(cos(angle), sin(angle)));
}

// How much of the worn ground goes bare rather than trampled at `world` (absolute XZ), as world
// `bare_share`: light wear tramples, heavy wear bares, the line broken up in patches.
fn bare_share(wear: f32, world: vec2<f32>) -> f32 {
    let t = clamp((wear + 0.3*(wear_patch(world, 1.8, 0x7a1u) - 0.5) - 0.3)/0.45, 0.0, 1.0);
    return t*t*(3.0 - 2.0*t);
}

// Value noise along a line, 0..1, one value per unit.
fn wear_line(x: f32, row: i32, seed: u32) -> f32 {
    let c = floor(x); let i = i32(c);
    return mix(wear_hash(i, row, seed), wear_hash(i+1, row, seed), wear_quintic(x-c));
}
// Grooves of many wheels along the traffic `direction` at `world` (XZ), -1..1, a few centimetres
// apart at random, drifting slowly along the road and showing in stretches.
fn wheel_grooves(world: vec2<f32>, direction: vec2<f32>) -> f32 {
    let across = dot(world, vec2(-direction.y, direction.x));
    let along = dot(world, direction);
    let drift = wear_value(vec2(along/9.0, across*0.2), 0x77du)*6.0;
    let stretch = smoothstep(0.35, 0.85, wear_value(vec2(along/4.0, across*0.5), 0x2d9u));
    return (wear_line(across/0.055 + drift, 3, 0x9b1u)*2.0 - 1.0)*stretch;
}
