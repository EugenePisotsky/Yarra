// Worn ground in near terrain detail: roads, paths and junctions as the cook draws them into each
// page's wear field (world terrain_wear.rs `TerrainWear`), at 12.5 cm. The page's weights hold the
// natural ground; where the field wears it, each surface gives way to what it wears down to, as
// `TerrainWear::lay` lays it. Ruts sink by parallax over the field's rut depth; grooves of many
// wheels run along its traffic direction, and rain stands in dips of the rut floors.

// Per page: wear, rut depth (of RUT_DEPTH_SCALE), and the traffic direction as a doubled angle
// (cosine and sine mapped to 0..1), at texel centres over the page.
@group(constants::MATERIAL_BIND_GROUP) @binding(16) var near_wear: texture_2d_array<f32>;
// Near terrain's coarse maps (weights, grass cover, wear) clamp rather than repeat.
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var map_sampler: sampler;

// world terrain_wear.rs RUT_DEPTH_SCALE, and one texel of WEAR_RESOLUTION as a share of a page.
const RUT_DEPTH_SCALE: f32 = 0.25;
const WEAR_TEXEL: f32 = 1.0/256.0;

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

// The field at `uv` (0..1 over a page) of page `layer`: wear, rut depth in metres, and the
// traffic direction (unit; its sign is arbitrary).
struct Wear { wear: f32, rut: f32, direction: vec2<f32>, }
fn wear_at(layer: i32, uv: vec2<f32>) -> Wear {
    let t = textureSampleLevel(near_wear, map_sampler, uv, layer, 0.0);
    let doubled = t.zw*2.0 - 1.0;
    let angle = atan2(doubled.y, doubled.x)*0.5;
    return Wear(t.x, t.y*RUT_DEPTH_SCALE, vec2(cos(angle), sin(angle)));
}
fn rut_at(layer: i32, uv: vec2<f32>) -> f32 {
    return textureSampleLevel(near_wear, map_sampler, uv, layer, 0.0).y*RUT_DEPTH_SCALE;
}

// A page's eight surface weights worn by `wear`: each surface gives that share of itself to what
// it wears down to (`worn[i]`: two slots, -1 none, and their shares).
fn lay_worn(weights: array<f32, 8>, worn: array<vec4<f32>, 8>, count: u32, wear: f32) -> array<f32, 8> {
    var out = weights;
    for (var i = 0u; i < count; i += 1u) {
        let to = worn[i];
        if to.x < 0.0 || weights[i] <= 0.0 { continue; }
        let given = weights[i]*wear;
        out[i] -= given;
        out[u32(to.x)] += given*to.y;
        if to.z >= 0.0 { out[u32(to.z)] += given*to.w; }
    }
    return out;
}

// Where the view ray (`view`: toward the eye) entering the terrain at page uv `uv` meets the
// rutted ground below it: the shift along the terrain (world XZ, metres) and the depth there.
// `normal` is the terrain's; `size` the page's side in metres.
fn rut_parallax(layer: i32, uv: vec2<f32>, size: f32, view: vec3<f32>, normal: vec3<f32>, deepest: f32) -> vec3<f32> {
    let vn = max(dot(view, normal), 0.15);
    // Along the terrain per metre down.
    let shift = -(view.xz - normal.xz*dot(view, normal))/vn;
    var previous = vec2(0.0, rut_at(layer, uv));
    for (var i = 1u; i <= 6u; i += 1u) {
        let z = deepest*f32(i)/6.0;
        let h = rut_at(layer, uv + shift*z/size);
        if z >= h {
            // Between the last step above the ground and this one below it.
            let above = previous.y - previous.x; let below = z - h;
            let at = mix(previous.x, z, above/max(above+below, 1e-6));
            return vec3(shift*at, rut_at(layer, uv + shift*at/size));
        }
        previous = vec2(z, h);
    }
    return vec3(shift*deepest, rut_at(layer, uv + shift*deepest/size));
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
