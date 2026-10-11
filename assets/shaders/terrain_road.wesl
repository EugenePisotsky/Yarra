// Roads in near terrain detail: the wheel tracks, grassy centre and verges of each road crossing
// a page, drawn per pixel from its centre line. The page's weights carry the same roads at half a
// metre, too coarse for a 1.1 m track; this reproduces the compiler's road influence
// (environment_compile roads/mod.rs `influence`, noise.rs) so the tracks fall exactly where the
// compiled weights and grass put them.

// Records written by terrain_render near/gpu.rs `road_records`, in vec4s. A road: track spacing,
// track width, edge softness, edge variation; edge patch size, breakup patch size, seed low and
// high 16 bits; breakup, centre ground, shoulder ground, strength; track depth, rut roundness;
// the page slots of up to four ground surfaces (-1 none); their shares. A tile of the page's grid:
// two runs of segments (first, count), together every segment reaching the tile. A segment: ends
// A and B; their unit normals; their corridor widths and its road.
@group(constants::MATERIAL_BIND_GROUP) @binding(16) var<storage, read> near_roads: array<vec4<f32>>;

fn road_hash(x: i32, z: i32, seed: u32) -> f32 {
    var v = (bitcast<u32>(x) * 0x8da6b343u) ^ (bitcast<u32>(z) * 0xd8163841u) ^ seed;
    v ^= v >> 16u; v *= 0x7feb352du; v ^= v >> 15u; v *= 0x846ca68bu; v ^= v >> 16u;
    return f32(v) * (1.0 / 4294967296.0);
}
fn road_quintic(t: f32) -> f32 { return t*t*t*(t*(t*6.0-15.0)+10.0); }
fn road_value(p: vec2<f32>, seed: u32) -> f32 {
    let c = floor(p); let i = vec2<i32>(c);
    let u = road_quintic(p.x-c.x); let v = road_quintic(p.y-c.y);
    let a = mix(road_hash(i.x,i.y,seed), road_hash(i.x+1,i.y,seed), u);
    let b = mix(road_hash(i.x,i.y+1,seed), road_hash(i.x+1,i.y+1,seed), u);
    return mix(a, b, v);
}
// Domain-warped value noise, 0..1, in patches about `size` metres across.
fn road_patch(world: vec2<f32>, size: f32, seed: u32) -> f32 {
    let p = world/size;
    let warp = vec2(road_value(p*0.43, seed ^ 0x731u), road_value(p*0.43, seed ^ 0x891u)) - 0.5;
    let q = p + warp*0.6;
    return road_value(q, seed)*0.75 + road_value(q*2.0, seed ^ 0x463u)*0.25;
}
fn road_smooth(t: f32) -> f32 { let s = clamp(t, 0.0, 1.0); return s*s*(3.0-2.0*s); }
fn road_falloff(distance: f32, radius: f32, softness: f32) -> f32 {
    return road_smooth((radius+softness-distance)/(2.0*softness));
}

// The nearest point of a road's centre line (page-local), its unit normal there, the distance to
// it, the signed offset along the normal, the distance past the line's end, the corridor width,
// and the road's record (-1: no road within reach).
struct RoadHit { point: vec2<f32>, normal: vec2<f32>, distance: f32, lateral: f32, longitudinal: f32, width: f32, road: i32, }
// `roads`: the page entry's first tile and tiles across; `p` page-local on a page `cell_size` across.
fn nearest_road(roads: vec4<i32>, p: vec2<f32>, cell_size: f32) -> RoadHit {
    let tile = clamp(vec2<i32>(floor(p/cell_size*f32(roads.y))), vec2(0), vec2(roads.y-1));
    let runs = near_roads[u32(roads.x + tile.y*roads.y + tile.x)];
    // The search keeps only the nearest segment and where along it; the rest is read once.
    var best = 3.4e38; var nearest = 0u; var along = 0.0;
    for (var r = 0u; r < 2u; r += 1u) {
        let run = select(runs.xy, runs.zw, r == 1u);
        for (var i = 0u; i < u32(run.y); i += 1u) {
            let s = u32(run.x) + i*3u;
            let ends = near_roads[s];
            let ab = ends.zw - ends.xy;
            let denominator = dot(ab, ab);
            let t = select(0.0, clamp(dot(p-ends.xy, ab)/denominator, 0.0, 1.0), denominator > 1e-12);
            let d = p - ends.xy - ab*t;
            let d2 = dot(d, d);
            if d2 < best { best = d2; nearest = s; along = t; }
        }
    }
    if best == 3.4e38 { return RoadHit(vec2(0.0), vec2(0.0), 0.0, 0.0, 0.0, 0.0, -1); }
    let ends = near_roads[nearest];
    let normals = near_roads[nearest+1u];
    let widths = near_roads[nearest+2u];
    let q = mix(ends.xy, ends.zw, along);
    let m = mix(normals.xy, normals.zw, along);
    let n = select(normals.xy, normalize(m), dot(m, m) > 1e-12);
    let lateral = dot(p-q, n);
    return RoadHit(q, n, sqrt(best), lateral, sqrt(max(best-lateral*lateral, 0.0)), mix(widths.x, widths.y, along), i32(widths.z));
}
// How far from the centre line the road shows, plus `margin`.
fn road_reach(hit: RoadHit, margin: f32) -> f32 {
    let shape = near_roads[u32(hit.road)];
    return hit.width*0.5 + shape.w + shape.z + margin;
}
// How much the road's ground replaces the natural ground at `world` (absolute world XZ, metres).
fn road_amount(hit: RoadHit, world: vec2<f32>) -> f32 {
    let base = u32(hit.road);
    let shape = near_roads[base];
    let noise = near_roads[base+1u];
    let look = near_roads[base+2u];
    let seed = u32(noise.z) | (u32(noise.w) << 16u);
    let irregular = (road_patch(world, noise.x, seed ^ 0x153u)*2.0-1.0)*shape.w;
    let feather = max(shape.z, 0.0001);
    let lateral = abs(hit.lateral);
    let track_outer = (shape.x+shape.y)*0.5;
    let center_radius = (shape.x-shape.y)*0.5;
    let corridor = road_falloff(hit.distance, hit.width*0.5+irregular, feather);
    let tracks = road_falloff(length(vec2(lateral-shape.x*0.5, hit.longitudinal)), shape.y*0.5+irregular, feather)*corridor;
    let center = road_falloff(hit.distance, center_radius, feather)*corridor;
    let shoulder = road_smooth((lateral-center_radius)/feather)
        * road_smooth((hit.width*0.5-lateral)/max(hit.width*0.5-track_outer, feather))*corridor;
    let wear = 1.0 - look.x*road_smooth((road_patch(world, noise.y, seed ^ 0x815u)-0.4)/0.3);
    return max(max(tracks, center*look.y), shoulder*look.z)*wear*look.w;
}
// The page slots of the road's ground surfaces (-1 none) and their shares.
fn road_slots(hit: RoadHit) -> vec4<f32> { return near_roads[u32(hit.road)+4u]; }
fn road_shares(hit: RoadHit) -> vec4<f32> { return near_roads[u32(hit.road)+5u]; }
