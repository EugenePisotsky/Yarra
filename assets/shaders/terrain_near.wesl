// Detailed surface evaluation on any terrain mesh; inputs use canonical cell addresses.
import bevy_pbr::render::mesh_view_bindings as near_view;
import package::shaders::terrain_stochastic::{quarter_turn, stochastic_vertex_turn, stochastic_vertex_offset};
import package::shaders::terrain_blend::{BLEND_DEPTH, BLEND_HEIGHT, height_blend};
import package::shaders::lighting::surface::surface_weather;
import package::shaders::terrain_road::{RoadHit, near_roads, nearest_road, road_amount, road_reach, road_shares, road_slots, road_patch, road_ruts, road_tracks, rut_depth, rut_parallax};
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
// Ground fully covered by grass shows the grass's lower blades in their own shade, this bright,
// where blades stand over it: gaps in a meadow read as deep grass, not bare soil.
const UNDER_GRASS: f32 = 0.2;
// Over this range the ground between blades turns from their shade to the field's colour, as
// the distant composite bakes it (world_cook material_bake), while the composite takes over
// (near::NEAR_START..NEAR_END).
const FIELD_RANGE: vec2<f32> = vec2(44.0, 76.0);
// How far fully covered ground takes the field's colour. The composite bake uses the same.
const FIELD_STRENGTH: f32 = 0.85;
// Close up, covered ground keeps most of its own texture (soil, litter, moss between the
// blades), taking only this much of the field's colour before the blades' shade darkens it.
const UNDER_TINT: f32 = 0.5;
// `color.a` is the surface's blend height.
struct Surface { color: vec4<f32>, normal: vec3<f32>, ao: f32, roughness: f32, }
// `field` is how far grassy ground here shows the field's colour rather than the blades' shade,
// 0..1.
fn surface(index: u32, slot: u32, local: vec2<f32>, world_dx: vec2<f32>, world_dy: vec2<f32>, field: f32) -> Surface {
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
    let cover = clamp(s.grass.w, 0.0, 1.0);
    let tint = mix(UNDER_TINT, FIELD_STRENGTH, field) * cover;
    let ground = mix(color.rgb, s.grass.rgb, tint) * mix(1.0, mix(UNDER_GRASS, 1.0, field), cover);
    return Surface(vec4(ground, color.a),n,packed.b,rough);
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
// The page's weights with a road drawn at full detail. Its natural ground comes from beside it,
// beyond its baked drawing, blended across; the road's ground then replaces that ground as the
// compiler lays it (environment_compile RoadPlan::ground), `amount` of it here.
fn road_weights(index: u32, page: Weights, hit: RoadHit, reach: f32, texel: f32, cell_size: f32, amount: f32) -> Weights {
    let left = page_weights(index, clamp((hit.point-hit.normal*reach)/cell_size, vec2(0.0), vec2(1.0)));
    let right = page_weights(index, clamp((hit.point+hit.normal*reach)/cell_size, vec2(0.0), vec2(1.0)));
    let side = clamp(hit.lateral/reach*0.5+0.5, 0.0, 1.0);
    let natural = smoothstep(reach-texel, reach, hit.distance);
    let slots = road_slots(hit); let shares = road_shares(hit) * amount;
    let laid = dot(shares, vec4(1.0));
    let a = slot_weight(slots.x, shares.x); let b = slot_weight(slots.y, shares.y);
    let c = slot_weight(slots.z, shares.z); let d = slot_weight(slots.w, shares.w);
    return Weights(mix(mix(left.low,right.low,side),page.low,natural)*(1.0-laid) + a.low+b.low+c.low+d.low,
        mix(mix(left.high,right.high,side),page.high,natural)*(1.0-laid) + a.high+b.high+c.high+d.high);
}
// The page's ground: its three largest weights here, blended by surface height.
fn blended_surfaces(index: u32, page: Weights, local: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, field: f32) -> Surface {
    let count = u32(near_table[index].state.z+0.5);
    if count <= 1u { return surface(index,0u,local,dx,dy,field); }
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
    let a = surface(index,pick.x,local,dx,dy,field);
    var cut = share.x+a.color.a*BLEND_HEIGHT-BLEND_DEPTH;
    var b = a; var c = a; var heights = vec3(a.color.a,-1.0e3,-1.0e3);
    if best.y > 0.0 && share.y+BLEND_HEIGHT > cut {
        b = surface(index,pick.y,local,dx,dy,field); heights.y = b.color.a;
        cut = max(cut,share.y+b.color.a*BLEND_HEIGHT-BLEND_DEPTH);
    }
    if best.z > 0.0 && share.z+BLEND_HEIGHT > cut {
        c = surface(index,pick.z,local,dx,dy,field); heights.z = c.color.a;
    }
    return mix_surfaces(a,b,c,height_blend(best,heights));
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
    let field = smoothstep(FIELD_RANGE.x,FIELD_RANGE.y,distance_to_camera);
    var weights = page_weights(index,uv);
    // Roads: where the surface is sampled (sunk into ruts), the terrain normal the ruts tilt,
    // how much they occlude, and how worn edges darken.
    var sampled = local; var n = normalize(geometry_normal); var occlusion = 1.0; var rim = 0.0;
    var compacted = 0.0;
    let roads = near_table[index].roads;
    if roads.y > 0 {
        let hit = nearest_road(roads,local,cell_size);
        if hit.road >= 0 {
            // Bilinear weights carry the baked road a texel and a half beyond its drawing.
            let texel = cell_size/max(near_table[index].state.y-1.0,1.0);
            let reach = road_reach(hit,texel*1.5);
            if hit.distance < reach {
                let page_origin = vec2<f32>(cell)*cell_size;
                let centre = page_origin+hit.point;
                let amount = road_amount(hit,page_origin+local,centre);
                weights = road_weights(index,weights,hit,reach,texel,cell_size,amount);
                // Where wheels wear the ground down to the grass, its edge is compacted and dark.
                rim = amount*(1.0-amount)*4.0;
                compacted = amount*0.25;
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
                    compacted = max(compacted,bottom*amount);
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
    }
    let s = blended_surfaces(index,weights,sampled,dx,dy,field);
    // Worn soil is darker and browner than loose soil of the same kind.
    let worn = mix(vec3(1.0), vec3(0.76,0.7,0.64), compacted)*(1.0-0.12*rim);
    result.color = vec4(clamp(s.color.rgb*macro_response(index,local,dx,dy)*worn,vec3(0.0),vec3(1.0)),1.0);
    // World-X tangent with the same +Z bitangent as the original heightfield UVs.
    let t = normalize(vec3(n.y,-n.x,0.0)); let b = normalize(cross(t,n));
    result.normal = normalize(t*s.normal.x+b*s.normal.y+n*s.normal.z);
    result.ao = s.ao*occlusion; result.roughness = s.roughness; result.weight = range_weight*availability;
    return result;
}
