// Roads in near terrain detail: the wheel tracks, grassy centre and verges of each road crossing
// a page, drawn per pixel from its centre line. The page's weights carry the same roads at half a
// metre, too coarse for a 1.1 m track; this reproduces the compiler's road drawing (world
// terrain_road.rs `RoadShape::zones`) so the tracks fall exactly where the compiled weights and
// grass put them. Wheel ruts narrower than the heights resolve are drawn here alone: sunk into
// the ground by parallax, grooved by wheels, and holding rain.

// Records written by terrain_render near/gpu.rs `road_records`, in vec4s. A road: track spacing,
// track width, edge softness, edge variation; edge patch size, breakup patch size, seed low and
// high 16 bits; breakup, centre ground, shoulder ground, strength; track depth, rut roundness,
// track wander, wander length; the page slots of up to four ground surfaces (-1 none); their shares. A tile of the page's grid:
// two runs of segments (first, count), together every segment reaching the tile. A segment: ends
// A and B; their unit normals; their corridor widths and its road.
@group(constants::MATERIAL_BIND_GROUP) @binding(16) var<storage, read> near_roads: array<vec4<f32>>;

fn road_bits(x: i32, z: i32, seed: u32) -> u32 {
    var v = (bitcast<u32>(x) * 0x8da6b343u) ^ (bitcast<u32>(z) * 0xd8163841u) ^ seed;
    v ^= v >> 16u; v *= 0x7feb352du; v ^= v >> 15u; v *= 0x846ca68bu; v ^= v >> 16u;
    return v;
}
fn road_hash(x: i32, z: i32, seed: u32) -> f32 { return f32(road_bits(x, z, seed)) * (1.0 / 4294967296.0); }
fn road_quintic(t: f32) -> f32 { return t*t*t*(t*(t*6.0-15.0)+10.0); }
fn road_value(p: vec2<f32>, seed: u32) -> f32 {
    let c = floor(p); let i = vec2<i32>(c);
    let u = road_quintic(p.x-c.x); let v = road_quintic(p.y-c.y);
    let a = mix(road_hash(i.x,i.y,seed), road_hash(i.x+1,i.y,seed), u);
    let b = mix(road_hash(i.x,i.y+1,seed), road_hash(i.x+1,i.y+1,seed), u);
    return mix(a, b, v);
}
// Four value noises at once, 0..1: each lattice corner's hash gives a byte to each.
fn road_value4(p: vec2<f32>, seed: u32) -> vec4<f32> {
    let c = floor(p); let i = vec2<i32>(c);
    let u = road_quintic(p.x-c.x); let v = road_quintic(p.y-c.y);
    let a = mix(unpack4x8unorm(road_bits(i.x,i.y,seed)), unpack4x8unorm(road_bits(i.x+1,i.y,seed)), u);
    let b = mix(unpack4x8unorm(road_bits(i.x,i.y+1,seed)), unpack4x8unorm(road_bits(i.x+1,i.y+1,seed)), u);
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
fn road_seed(road: u32) -> u32 { let noise = near_roads[road+1u]; return u32(noise.z) | (u32(noise.w) << 16u); }
// The wheel tracks where the centre line passes `centre` (world XZ): their offsets from the line
// (left, right) and half-widths, wandering along the road.
struct RoadTracks { offset: vec2<f32>, half: vec2<f32>, }
fn road_tracks(road: u32, centre: vec2<f32>) -> RoadTracks {
    let shape = near_roads[road]; let rut = near_roads[road+3u];
    var tracks = RoadTracks(vec2(-0.5, 0.5)*shape.x, vec2(0.5*shape.y));
    if rut.z > 0.0 {
        let n = road_value4(centre/rut.w, road_seed(road) ^ 0x5a1u)*2.0-1.0;
        tracks.offset += rut.z*n.xy;
        tracks.half += rut.z*0.5*n.zw;
    }
    return tracks;
}
// How far the road's edges stand out at `world` (negative: in): broad patches and fine bites.
fn road_ragged(road: u32, world: vec2<f32>) -> f32 {
    let size = near_roads[road+1u].x; let seed = road_seed(road);
    let broad = road_patch(world, size, seed ^ 0x153u)*2.0-1.0;
    let bites = road_value(world*5.0/size, seed ^ 0x2e1u)*2.0-1.0;
    return (broad*0.7 + bites*0.3)*near_roads[road].w;
}
// How much the road's ground replaces the natural ground at `world` (absolute world XZ, metres),
// whose nearest centre-line point is `centre` (world XZ).
fn road_amount(hit: RoadHit, world: vec2<f32>, centre: vec2<f32>) -> f32 {
    let base = u32(hit.road);
    let shape = near_roads[base];
    let noise = near_roads[base+1u];
    let look = near_roads[base+2u];
    let irregular = road_ragged(base, world);
    let feather = max(shape.z, 0.0001);
    let tracks = road_tracks(base, centre);
    let corridor = road_falloff(hit.distance, hit.width*0.5+irregular, feather);
    let wheels = max(
        road_falloff(length(vec2(hit.lateral-tracks.offset.x, hit.longitudinal)), tracks.half.x+irregular, feather),
        road_falloff(length(vec2(hit.lateral-tracks.offset.y, hit.longitudinal)), tracks.half.y+irregular, feather))*corridor;
    // The strip between the tracks' inner edges, and outside each track its shoulder.
    let inner = vec2(tracks.offset.x+tracks.half.x, tracks.offset.y-tracks.half.y);
    let middle = (inner.x+inner.y)*0.5;
    let center = road_falloff(length(vec2(hit.lateral-middle, hit.longitudinal)), (inner.y-inner.x)*0.5, feather)*corridor;
    var out = hit.lateral; var near = inner.y; var far = tracks.offset.y+tracks.half.y;
    if hit.lateral < middle { out = -hit.lateral; near = -inner.x; far = tracks.half.x-tracks.offset.x; }
    let half_width = hit.width*0.5;
    let shoulder = road_smooth((out-near)/feather)*road_smooth((half_width-out)/max(half_width-far, feather))*corridor;
    let wear = 1.0 - look.x*road_smooth((road_patch(world, noise.y, road_seed(base) ^ 0x815u)-0.4)/0.3);
    return max(max(wheels, center*look.y), shoulder*look.z)*wear*look.w;
}

// The wheel ruts of a road near `centre` (world XZ): the tracks, each rut's depth (metres), how
// far its grooves have drifted across it and how strongly they show, the ruts' roundness and
// the road's seed.
struct RoadRuts { tracks: RoadTracks, depth: vec2<f32>, drift: vec2<f32>, grooves: vec2<f32>, roundness: f32, seed: u32, }
fn road_ruts(hit: RoadHit, centre: vec2<f32>, scale: f32, grooves: f32) -> RoadRuts {
    let base = u32(hit.road); let rut = near_roads[base+3u]; let seed = road_seed(base);
    // Over twice the wander length each rut deepens or shallows by 30% and its grooves drift;
    // every metre or two it dips, and grooves show in stretches a few metres long.
    let slow = road_value4(centre/(2.0*max(rut.w, 0.5)), seed ^ 0x77du);
    let fast = road_value4(centre/1.4, seed ^ 0x3c7u);
    let stretch = road_value4(centre/4.0, seed ^ 0x2d9u);
    let depth = rut.x*scale*(0.7+0.6*slow.xy)*(0.8+0.4*fast.zw);
    let streaks = smoothstep(vec2(0.3), vec2(0.9), stretch.xy)*grooves;
    return RoadRuts(road_tracks(base, centre), depth, slow.zw*8.0, streaks, rut.y, seed);
}
// Value noise along a line, 0..1, one value per unit.
fn road_line(x: f32, row: i32, seed: u32) -> f32 {
    let c = floor(x); let i = i32(c);
    return mix(road_hash(i, row, seed), road_hash(i+1, row, seed), road_quintic(x-c));
}
// How deep the ground lies below the terrain surface `lateral` from the centre line: the deeper
// rut there, rounded across its width and, in stretches and when `grooved`, scored along by the
// grooves of many wheels, a few centimetres apart at random.
fn rut_depth(r: RoadRuts, lateral: f32, longitudinal: f32, grooved: bool) -> f32 {
    let x = lateral - r.tracks.offset;
    let t = min(vec2(length(vec2(x.x, longitudinal)), length(vec2(x.y, longitudinal)))/max(r.tracks.half, vec2(0.01)), vec2(1.0));
    let profile = r.depth*smoothstep(vec2(0.0), vec2(1.0), 1.0 - pow(t, vec2(2.0 + 6.0*(1.0-r.roundness))));
    // Only the deeper rut here is grooved.
    let side = select(0u, 1u, profile.y > profile.x);
    if !grooved || r.grooves[side] <= 0.0 { return profile[side]; }
    let g = road_line(x[side]/0.055 + r.drift[side], i32(side)*7 + 3, r.seed ^ 0x9b1u)*2.0 - 1.0;
    return profile[side]*(1.0 + 0.3*g*abs(g)*r.grooves[side]);
}
// Where the view ray (`view`: toward the eye) entering the terrain at `hit`'s point meets the
// rutted ground below it: the shift along the terrain (world XZ) and the depth there. `normal`
// is the terrain's. The march follows the ruts' shape; grooves only shade their floors.
fn rut_parallax(r: RoadRuts, hit: RoadHit, view: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let deepest = max(r.depth.x, r.depth.y)*1.3;
    let vn = max(dot(view, normal), 0.15);
    // Along the terrain per metre down, and across the road per metre down.
    let shift = -(view.xz - normal.xz*dot(view, normal))/vn;
    let across = dot(shift, hit.normal);
    var previous = vec2(0.0, rut_depth(r, hit.lateral, hit.longitudinal, false));
    for (var i = 1u; i <= 6u; i += 1u) {
        let z = deepest*f32(i)/6.0;
        let h = rut_depth(r, hit.lateral+across*z, hit.longitudinal, false);
        if z >= h {
            // Between the last step above the ground and this one below it.
            let above = previous.y - previous.x; let below = z - h;
            let at = mix(previous.x, z, above/max(above+below, 1e-6));
            return vec3(shift*at, rut_depth(r, hit.lateral+across*at, hit.longitudinal, false));
        }
        previous = vec2(z, h);
    }
    return vec3(shift*deepest, deepest);
}
// The page slots of the road's ground surfaces (-1 none) and their shares.
fn road_slots(hit: RoadHit) -> vec4<f32> { return near_roads[u32(hit.road)+4u]; }
fn road_shares(hit: RoadHit) -> vec4<f32> { return near_roads[u32(hit.road)+5u]; }
