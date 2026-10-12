// Detailed surface evaluation on any terrain mesh; inputs use canonical cell addresses.
import bevy_pbr::render::mesh_view_bindings as near_view;
import package::shaders::terrain_stochastic::{quarter_turn, stochastic_vertex_turn, stochastic_vertex_offset};
import package::shaders::terrain_blend::{BLEND_DEPTH, BLEND_HEIGHT, height_blend};
import package::shaders::lighting::surface::surface_weather;
import package::shaders::terrain_road::{RoadHit, near_roads, nearest_roads, road_amount, road_reach, road_patch, road_ruts, road_tracks, rut_depth, rut_parallax, junction_ground, junctions_over};
// One of a page's up to eight surfaces. `surface`: array layer, metres per repetition,
// anti-tiling (0/1), normal strength. `material`: normal Y sign, roughness minimum/maximum.
// `phase`/`lattice`: this page's UV and anti-tiling lattice offsets for the surface's tile size.
// `grass`: the grass field's linear colour and how fully grass covers the surface.
struct NearSlot { surface: vec4<f32>, material: vec4<f32>, phase: vec4<f32>, lattice: vec4<f32>, grass: vec4<f32>, }
// `roads`: the page's first road tile and tiles across in `near_roads` (terrain_road.wesl).
struct NearEntry {
    key: vec4<i32>, roads: vec4<i32>, state: vec4<f32>, macro_scales: vec4<f32>, macro_settings: vec4<f32>,
    macro_a: vec4<f32>, macro_b: vec4<f32>, macro_c: vec4<f32>, slots: array<NearSlot, 8>,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var near_weights: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(10) var<storage, read> near_table: array<NearEntry>;
@group(constants::MATERIAL_BIND_GROUP) @binding(11) var<uniform> near_settings: vec4<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(12) var near_base: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(13) var near_normal: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(14) var near_macro: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(15) var repeat_sampler: sampler;
// Per page: how grass covers its ground (near::NearGrassCover), the field colour's square root
// and the cover, at the grass fields' texel centres.
@group(constants::MATERIAL_BIND_GROUP) @binding(17) var near_grass: texture_2d_array<f32>;
// This sampler belongs to the coarse maps and clamps rather than repeating controls.
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var map_sampler: sampler;

// Probe only keys. Returning the whole entry through a dynamic loop also
// carried every surface parameter through neighbour-only lookups.
fn near_lookup(key: vec2<i32>) -> u32 {
    var h = (u32(key.x) * 0x9e3779b9u) ^ (u32(key.y) * 0x85ebca6bu);
    h ^= h >> 16u; h *= 0x7feb352du; h ^= h >> 15u;
    h &= 255u;
    // Most pixels hit the first slot. Keep that case outside the collision loop.
    let first = near_table[h].key;
    if first.w == 0 { return 256u; }
    if all(first.xy == key) { return h; }
    for (var i=1u; i<256u; i+=1u) {
        let index = (h+i)&255u;
        let stored = near_table[index].key;
        if stored.w == 0 { return 256u; }
        if all(stored.xy == key) { return index; }
    }
    return 256u;
}
fn near_availability(key: vec2<i32>) -> f32 {
    let index = near_lookup(key);
    if index == 256u { return 0.0; }
    return near_table[index].state.x;
}
fn triangular(uv: vec2<f32>) -> vec2<f32> { return vec2(uv.x + uv.y*0.5773502692, uv.y*1.1547005384); }
fn stochastic(uv: vec2<f32>, lattice: vec2<f32>, origin: vec2<f32>, layer: i32, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {
    let c = floor(lattice); let f = fract(lattice);
    var vertices = array<vec2<f32>,3>(c, c+vec2(1.0,0.0), c+vec2(0.0,1.0));
    var weights = vec3(1.0-f.x-f.y, f.x, f.y);
    if f.x+f.y > 1.0 {
        vertices = array<vec2<f32>,3>(c+vec2(1.0), c+vec2(0.0,1.0), c+vec2(1.0,0.0));
        weights = vec3(f.x+f.y-1.0, 1.0-f.x, 1.0-f.y);
    }
    weights = weights*weights*weights*weights;
    weights /= max(dot(weights,vec3(1.0)),0.000001);
    var result = vec4(0.0);
    for(var i=0u;i<3u;i+=1u) {
        let vertex = origin + vertices[i];
        let turn = stochastic_vertex_turn(vertex,layer);
        result += textureSampleGrad(near_base, repeat_sampler, quarter_turn(uv,turn)+stochastic_vertex_offset(vertex,layer), layer, quarter_turn(dx,turn), quarter_turn(dy,turn))*weights[i];
    }
    return result;
}
// Ground between grass blades, as far as grass covers it. Close by it keeps most of its own
// texture (soil, litter, moss between the blades), a little of the field's colour and shaded as
// the blades' roots are by the sky (vegetation lighting: about half); over UNDER_RANGE, as blades
// thin out with distance, it takes more of the field's colour and its shade, so gaps read as deep
// grass, not bare soil. Pairs (close, at UNDER_RANGE's end).
const UNDER_RANGE: vec2<f32> = vec2(5.0, 30.0);
const UNDER_TINT: vec2<f32> = vec2(0.25, 0.5);
const UNDER_SHADE: vec2<f32> = vec2(0.8, 0.55);
// Over this range the ground between blades turns from their shade to the field's colour, as
// the distant composite bakes it (world_cook material_bake), while the composite takes over
// (near::NEAR_START..NEAR_END).
const FIELD_RANGE: vec2<f32> = vec2(44.0, 76.0);
// How far fully covered ground takes the field's colour. The composite bake uses the same.
const FIELD_STRENGTH: f32 = 0.85;
// `color.a` is the surface's blend height.
struct Surface { color: vec4<f32>, normal: vec3<f32>, ao: f32, roughness: f32, }
fn surface(index: u32, slot: u32, local: vec2<f32>, world_dx: vec2<f32>, world_dy: vec2<f32>) -> Surface {
    let s = near_table[index].slots[slot];
    let size = max(s.surface.y, 0.001);
    let layer = i32(s.surface.x+0.5);
    let uv = local/size + s.phase.xy;
    let dx = world_dx/size; let dy = world_dy/size;
    var color: vec4<f32>;
    if s.surface.z >= 0.5 {
        color = stochastic(uv, triangular(local/size)+s.lattice.zw, s.lattice.xy, layer, dx, dy);
    } else { color = textureSampleGrad(near_base, repeat_sampler, uv, layer, dx, dy); }
    let packed = textureSampleGrad(near_normal, repeat_sampler, uv, layer, dx, dy);
    let oct = packed.rg*2.0-1.0;
    var n = vec3(oct, 1.0-abs(oct.x)-abs(oct.y));
    let fold = clamp(-n.z,0.0,1.0);
    n.x += select(fold,-fold,n.x>=0.0); n.y += select(fold,-fold,n.y>=0.0);
    n = normalize(n); n.y *= s.material.x;
    n = normalize(vec3(n.xy*clamp(s.surface.w,0.0,1.0), max(n.z,0.001)));
    let rough = mix(s.material.y,s.material.z,packed.a);
    return Surface(color,n,packed.b,rough);
}
// Weighted sum of up to three shaded surfaces.
fn mix_surfaces(a: Surface, b: Surface, c: Surface, w: vec3<f32>) -> Surface {
    return Surface(a.color*w.x+b.color*w.y+c.color*w.z,
        normalize(a.normal*w.x+b.normal*w.y+c.normal*w.z),
        a.ao*w.x+b.ao*w.y+c.ao*w.z, a.roughness*w.x+b.roughness*w.y+c.roughness*w.z);
}
// A page's surface weights, slots 0-3 and 4-7.
struct Weights { low: vec4<f32>, high: vec4<f32>, }
// The page's surface weights at `uv` (0..1 across the page).
fn page_weights(index: u32, uv: vec2<f32>) -> Weights {
    let count = u32(near_table[index].state.z+0.5);
    if count <= 1u { return Weights(vec4(1.0,0.0,0.0,0.0), vec4(0.0)); }
    let wuv = (vec2(0.5)+uv*(near_table[index].state.y-1.0))/near_settings.w;
    let layer = (near_table[index].key.w-1)*2;
    let first = textureSampleLevel(near_weights,map_sampler,wuv,layer,0.0);
    var second = vec4(0.0);
    if count > 4u { second = textureSampleLevel(near_weights,map_sampler,wuv,layer+1,0.0); }
    return Weights(first, second);
}
// Weight `share` on page slot `slot` (none when negative).
fn slot_weight(slot: f32, share: f32) -> Weights {
    return Weights(select(vec4(0.0), vec4(share), vec4(0.0,1.0,2.0,3.0) == vec4(slot)),
        select(vec4(0.0), vec4(share), vec4(4.0,5.0,6.0,7.0) == vec4(slot)));
}
// The natural ground under a road `hit`, drawn at full detail where it reaches `reach`: its
// page weights from beside it, beyond its baked drawing, blended across, and the page's own
// weights at the edge of its reach.
fn natural_weights(index: u32, page: Weights, hit: RoadHit, reach: f32, other: RoadHit, other_reach: f32, texel: f32, cell_size: f32) -> Weights {
    let left_uv = clamp((hit.point-hit.normal*reach)/cell_size, vec2(0.0), vec2(1.0));
    let right_uv = clamp((hit.point+hit.normal*reach)/cell_size, vec2(0.0), vec2(1.0));
    let left = page_weights(index, left_uv);
    let right = page_weights(index, right_uv);
    var side = clamp(hit.lateral/reach*0.5+0.5, 0.0, 1.0);
    // Near the page's edge a side's sample clamps onto the page, maybe onto the road itself, and
    // where roads meet it may fall on the other road: either's ground would then fill the grassy
    // centre. A side stands in as far as its sample stays clear of both, else the side whose
    // sample lies farther from the road.
    let clear = abs(vec2(dot(left_uv*cell_size-hit.point, hit.normal), dot(right_uv*cell_size-hit.point, hit.normal)));
    var usable = smoothstep(vec2(reach-2.0*texel), vec2(reach-texel), clear);
    if other.road >= 0 {
        let apart = abs(vec2(dot(left_uv*cell_size-other.point, other.normal), dot(right_uv*cell_size-other.point, other.normal)));
        usable *= smoothstep(vec2(other_reach-2.0*texel), vec2(other_reach-texel), apart);
    }
    let shares = vec2(1.0-side, side)*usable;
    if shares.x+shares.y > 1.0e-3 { side = shares.y/(shares.x+shares.y); }
    else { side = select(0.0, 1.0, clear.y > clear.x); }
    let natural = smoothstep(reach-texel, reach, hit.distance);
    return Weights(mix(mix(left.low,right.low,side),page.low,natural), mix(mix(left.high,right.high,side),page.high,natural));
}
// Weights `w` with `amount` of road record `road`'s ground laid over them, as the compiler lays
// it (environment_compile RoadPlan::ground): that share of all ground beneath gives way to the
// road's. Road ground the page does not carry leaves the ground beneath.
fn lay_road(w: Weights, road: u32, amount: f32) -> Weights {
    let slots = near_roads[road+4u]; let shares = near_roads[road+5u]*amount;
    let laid = dot(shares, vec4(1.0));
    let a = slot_weight(slots.x, shares.x); let b = slot_weight(slots.y, shares.y);
    let c = slot_weight(slots.z, shares.z); let d = slot_weight(slots.w, shares.w);
    return Weights(w.low*(1.0-laid) + a.low+b.low+c.low+d.low, w.high*(1.0-laid) + a.high+b.high+c.high+d.high);
}
// The page's ground: its three largest weights here, blended by surface height, and the grass
// its surfaces carry (field colour, cover) in the same shares.
struct Ground { surface: Surface, grass: vec4<f32>, }
fn blended_surfaces(index: u32, page: Weights, local: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> Ground {
    let count = u32(near_table[index].state.z+0.5);
    if count <= 1u { return Ground(surface(index,0u,local,dx,dy), near_table[index].slots[0].grass); }
    var weights = array<f32,8>(page.low.x,page.low.y,page.low.z,page.low.w,page.high.x,page.high.y,page.high.z,page.high.w);
    // Largest first; ties keep the lower slot, as world::terrain_blend_candidates does.
    var pick = vec3<u32>(0u); var best = vec3(0.0);
    for (var i=0u; i<count; i+=1u) {
        let w = weights[i];
        if w > best.x { best = vec3(w,best.xy); pick = vec3(i,pick.xy); }
        else if w > best.y { best = vec3(best.x,w,best.y); pick = vec3(pick.x,i,pick.y); }
        else if w > best.z { best.z = w; pick.z = i; }
    }
    // A surface shows only where its score (weight share plus height) comes within the blend
    // depth of the best; one that cannot, even at full height, is not sampled.
    let share = best/max(best.x+best.y+best.z,0.000001);
    let a = surface(index,pick.x,local,dx,dy);
    var cut = share.x+a.color.a*BLEND_HEIGHT-BLEND_DEPTH;
    var b = a; var c = a; var heights = vec3(a.color.a,-1.0e3,-1.0e3);
    if best.y > 0.0 && share.y+BLEND_HEIGHT > cut {
        b = surface(index,pick.y,local,dx,dy); heights.y = b.color.a;
        cut = max(cut,share.y+b.color.a*BLEND_HEIGHT-BLEND_DEPTH);
    }
    if best.z > 0.0 && share.z+BLEND_HEIGHT > cut {
        c = surface(index,pick.z,local,dx,dy); heights.z = c.color.a;
    }
    let w = height_blend(best,heights);
    let grass = near_table[index].slots[pick.x].grass*w.x + near_table[index].slots[pick.y].grass*w.y
        + near_table[index].slots[pick.z].grass*w.z;
    return Ground(mix_surfaces(a,b,c,w), grass);
}
fn rotate_medium(p: vec2<f32>) -> vec2<f32> { return vec2(p.x*0.819-p.y*0.574, p.x*0.574+p.y*0.819); }
fn rotate_large(p: vec2<f32>) -> vec2<f32> { return vec2(p.x*-0.342-p.y*0.940, p.x*0.940+p.y*-0.342); }
fn macro_response(index: u32, local: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> f32 {
    if near_table[index].macro_settings.y < 0.5 { return 1.0; }
    let a = textureSampleGrad(near_macro,repeat_sampler,local/near_table[index].macro_scales.x+near_table[index].macro_a.xy+vec2(0.11,0.73),dx/near_table[index].macro_scales.x,dy/near_table[index].macro_scales.x).r;
    let b = textureSampleGrad(near_macro,repeat_sampler,rotate_medium(local)/near_table[index].macro_scales.y+near_table[index].macro_b.xy+vec2(0.37,0.61),rotate_medium(dx)/near_table[index].macro_scales.y,rotate_medium(dy)/near_table[index].macro_scales.y).r;
    let c = textureSampleGrad(near_macro,repeat_sampler,rotate_large(local)/near_table[index].macro_scales.z+near_table[index].macro_c.xy+vec2(0.83,0.19),rotate_large(dx)/near_table[index].macro_scales.z,rotate_large(dy)/near_table[index].macro_scales.z).r;
    let signal = dot(vec3(a,b,c)*2.0-1.0,vec3(0.28,0.36,0.36));
    return exp2(clamp(signal*near_table[index].macro_settings.x,-1.0,1.0)*near_table[index].macro_scales.w);
}
// Wheel ruts sink by parallax within this range, fading out over its second half.
const RUT_RANGE: vec2<f32> = vec2(20.0, 35.0);
// `hollow`: where rain stands in a rut here, as rain hollowness 0..1 (-1: no rut).
struct NearResult { color: vec4<f32>, normal: vec3<f32>, ao: f32, roughness: f32, weight: f32, hollow: f32, }
fn close_ground(world: vec3<f32>, geometry_normal: vec3<f32>, dx: vec2<f32>, dy: vec2<f32>, cell_size: f32, origin: vec2<i32>) -> NearResult {
    var result = NearResult(vec4(0.0),geometry_normal,1.0,1.0,0.0,-1.0);
    if near_settings.z < 0.5 { return result; }
    let distance_to_camera = distance(near_view::view.world_position,world);
    // By distance only: at a glancing view a pixel spans metres along the ground but centimetres
    // across it, where the composite's half-metre bake would blur roads a few metres off.
    let range_weight = 1.0-smoothstep(near_settings.x,near_settings.y,distance_to_camera);
    if range_weight <= 0.0 { return result; }
    let c = world.xz/cell_size;
    let cell = vec2<i32>(floor(c))+origin;
    let uv = fract(c); let local = uv*cell_size;
    let index = near_lookup(cell);
    if index == 256u { return result; }
    if near_table[index].state.x <= 0.0 { return result; }
    var availability = near_table[index].state.x;
    let edges = vec4(uv.x,1.0-uv.x,uv.y,1.0-uv.y)*cell_size;
    let offsets = array<vec2<i32>,4>(vec2(-1,0),vec2(1,0),vec2(0,-1),vec2(0,1));
    for(var i=0u;i<4u;i+=1u) {
        if edges[i] < 1.0 { availability = min(availability,mix(near_availability(cell+offsets[i]),1.0,smoothstep(0.0,1.0,edges[i]))); }
    }
    // How much of the field's colour and of the blades' shade fully grassed ground takes here.
    let field = smoothstep(FIELD_RANGE.x,FIELD_RANGE.y,distance_to_camera);
    let thinning = smoothstep(UNDER_RANGE.x,UNDER_RANGE.y,distance_to_camera);
    let under = vec2(mix(mix(UNDER_TINT.x,UNDER_TINT.y,thinning), FIELD_STRENGTH, field),
        mix(mix(UNDER_SHADE.x,UNDER_SHADE.y,thinning), 1.0, field));
    var weights = page_weights(index,uv);
    // Roads: where the surface is sampled (sunk into ruts), the terrain normal the ruts tilt,
    // how much they occlude, and how worn edges darken.
    var sampled = local; var n = normalize(geometry_normal); var occlusion = 1.0; var rim = 0.0;
    var compacted = 0.0;
    let roads = near_table[index].roads;
    if roads.y > 0 {
        // Up to two roads draw here (where roads meet or branch), each within its reach: bilinear
        // weights carry the baked road a texel and a half beyond its drawing.
        let pair = nearest_roads(roads,local,cell_size);
        let texel = cell_size/max(near_table[index].state.y-1.0,1.0);
        var reach = vec2(0.0);
        if pair.first.road >= 0 { reach.x = road_reach(pair.first,texel*1.5); }
        if pair.second.road >= 0 { reach.y = road_reach(pair.second,texel*1.5); }
        let within = vec2((pair.first.road >= 0) && (pair.first.distance < reach.x),
            (pair.second.road >= 0) && (pair.second.distance < reach.y));
        if within.x || within.y {
            let page_origin = vec2<f32>(cell)*cell_size;
            let at = page_origin+local;
            var amount = vec2(0.0);
            if within.x { amount.x = road_amount(pair.first,at,page_origin+pair.first.point)*(1.0-junctions_over(roads,pair.first,local)); }
            if within.y { amount.y = road_amount(pair.second,at,page_origin+pair.second.point)*(1.0-junctions_over(roads,pair.second,local)); }
            // The road this pixel belongs to: the nearest within reach. Its natural ground comes
            // from beside it; the roads lay theirs over it in the compiler's order, then junctions.
            var hit = pair.first; var other = pair.second; var reaches = reach; var drawn = amount.x;
            if !within.x { hit = pair.second; other = pair.first; reaches = reach.yx; drawn = amount.y; }
            weights = natural_weights(index,weights,hit,reaches.x,other,reaches.y,texel,cell_size);
            let order = select(vec2(1u,0u), vec2(0u,1u), (pair.second.road < 0) || (pair.first.road < pair.second.road));
            for (var k = 0u; k < 2u; k += 1u) {
                let i = order[k];
                if amount[i] > 0.0 { weights = lay_road(weights,u32(select(pair.second.road,pair.first.road,i == 0u)),amount[i]); }
            }
            for (var j = 0; j < roads.w; j += 1) {
                let record = u32(roads.z+j*2);
                let hub = junction_ground(record,local,at);
                if hub > 0.0 { weights = lay_road(weights,u32(near_roads[record+1u].y),hub); }
            }
            let centre = page_origin+hit.point;
            // Where wheels wear the ground down to the grass, its edge is compacted and dark.
            rim = drawn*(1.0-drawn)*4.0;
            compacted = drawn*0.25;
            let sink = 1.0-smoothstep(RUT_RANGE.x,RUT_RANGE.y,distance_to_camera);
            let rut = near_roads[u32(hit.road)+3u];
            // A rut can be seen into from no further than its depth over the view's slope.
            let tracks = road_tracks(u32(hit.road),centre);
            let apart = abs(vec2(hit.lateral)-tracks.offset)-tracks.half;
            if rut.x > 0.0 && sink > 0.0 && min(apart.x,apart.y) < rut.x*8.0 {
                // Grooves finer than about two pixels fade rather than shimmer.
                let grooves = 1.0-smoothstep(0.03,0.07,max(length(dx),length(dy)));
                let ruts = road_ruts(hit,centre,sink,grooves);
                let found = rut_parallax(ruts,hit,normalize(near_view::view.world_position-world),n);
                sampled = local+found.xy;
                let across = hit.lateral+dot(found.xy,hit.normal);
                let slope = (rut_depth(ruts,across+0.01,hit.longitudinal,true)-rut_depth(ruts,across-0.01,hit.longitudinal,true))/0.02;
                n = normalize(n+vec3(hit.normal.x,0.0,hit.normal.y)*slope);
                // Rut bottoms: shaded by their walls, the soil compacted and damp.
                let bottom = found.z/max(max(ruts.depth.x,ruts.depth.y),0.0001);
                occlusion = 1.0-0.35*bottom;
                compacted = max(compacted,bottom*drawn);
                // Rain stands in dips of a rut's floor a metre or three across, deepest
                // first, spreading as the ground soaks (weather/ground_wetness.wesl); fine
                // noise breaks the water line. Puddles form only on wet ground.
                if surface_weather().x > 0.3 {
                    let floor = page_origin+sampled;
                    let dips = road_patch(floor,2.0,ruts.seed^0x6e3u);
                    let fine = road_patch(floor,0.6,ruts.seed^0x51du)-0.5;
                    result.hollow = clamp(bottom*bottom*dips*1.5-0.55+fine*0.35,0.0,1.0);
                }
            }
        }
    }
    let blend = blended_surfaces(index,weights,sampled,dx,dy);
    let s = blend.surface;
    // Grass over the ground here: where blades actually grow, from the page's cover layer,
    // else as its surfaces carry grass.
    var grass = clamp(blend.grass, vec4(0.0), vec4(1.0));
    let cover_side = near_table[index].state.w;
    if cover_side > 0.5 {
        let texel = clamp(uv*cover_side, vec2(0.5), vec2(cover_side-0.5))/f32(textureDimensions(near_grass).x);
        let packed = textureSampleLevel(near_grass,map_sampler,texel,near_table[index].key.w-1,0.0);
        grass = vec4(packed.rgb*packed.rgb, packed.a);
    }
    let ground = mix(s.color.rgb, grass.rgb, under.x*grass.a) * mix(1.0, under.y, grass.a);
    // Worn soil is darker and browner than loose soil of the same kind.
    let worn = mix(vec3(1.0), vec3(0.76,0.7,0.64), compacted)*(1.0-0.12*rim);
    result.color = vec4(clamp(ground*macro_response(index,local,dx,dy)*worn,vec3(0.0),vec3(1.0)),1.0);
    // World-X tangent with the same +Z bitangent as the original heightfield UVs.
    let t = normalize(vec3(n.y,-n.x,0.0)); let b = normalize(cross(t,n));
    result.normal = normalize(t*s.normal.x+b*s.normal.y+n*s.normal.z);
    result.ao = s.ao*occlusion; result.roughness = s.roughness; result.weight = range_weight*availability;
    return result;
}
