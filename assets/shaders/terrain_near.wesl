// Detailed surface evaluation on any terrain mesh; inputs use canonical cell addresses.
import bevy_pbr::render::mesh_view_bindings as near_view;
import package::shaders::grass_canopy::canopy_visibility_at;
import package::shaders::terrain_stochastic::{quarter_turn, stochastic_vertex_turn, stochastic_vertex_offset};
import package::shaders::terrain_blend::height_blend;
struct NearCanopy { appearance: vec4<f32>, shape: vec4<f32>, distance_settings: vec4<f32>, origin: vec4<f32>, }
// One of a page's up to eight surfaces. `surface`: array layer, metres per repetition,
// anti-tiling (0/1), normal strength. `material`: normal Y sign, roughness minimum/maximum.
// `phase`/`lattice`: this page's UV and anti-tiling lattice offsets for the surface's tile size.
struct NearSlot { surface: vec4<f32>, material: vec4<f32>, phase: vec4<f32>, lattice: vec4<f32>, }
struct NearEntry {
    key: vec4<i32>, state: vec4<f32>, macro_scales: vec4<f32>, macro_settings: vec4<f32>,
    macro_a: vec4<f32>, macro_b: vec4<f32>, macro_c: vec4<f32>, slots: array<NearSlot, 8>, canopy: NearCanopy,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var near_weights: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(9) var near_canopy: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(10) var<storage, read> near_table: array<NearEntry>;
@group(constants::MATERIAL_BIND_GROUP) @binding(11) var<uniform> near_settings: vec4<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(12) var near_base: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(13) var near_normal: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(14) var near_macro: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(15) var repeat_sampler: sampler;
// This sampler belongs to the coarse maps and clamps rather than repeating controls.
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var map_sampler: sampler;

// Probe only keys. Returning the whole entry through a dynamic loop also
// carried every surface/canopy parameter through neighbour-only lookups.
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
// The page's ground: its three largest weights here, blended by surface height.
fn blended_surfaces(index: u32, uv: vec2<f32>, local: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> Surface {
    let count = u32(near_table[index].state.z+0.5);
    if count <= 1u { return surface(index,0u,local,dx,dy); }
    let wuv = (vec2(0.5)+uv*(near_table[index].state.y-1.0))/near_settings.w;
    let layer = (near_table[index].key.w-1)*2;
    let first = textureSampleLevel(near_weights,map_sampler,wuv,layer,0.0);
    var second = vec4(0.0);
    if count > 4u { second = textureSampleLevel(near_weights,map_sampler,wuv,layer+1,0.0); }
    var weights = array<f32,8>(first.x,first.y,first.z,first.w,second.x,second.y,second.z,second.w);
    // Largest first; ties keep the lower slot, as world::terrain_blend_candidates does.
    var pick = vec3<u32>(0u); var best = vec3(0.0);
    for (var i=0u; i<count; i+=1u) {
        let w = weights[i];
        if w > best.x { best = vec3(w,best.xy); pick = vec3(i,pick.xy); }
        else if w > best.y { best = vec3(best.x,w,best.y); pick = vec3(pick.x,i,pick.y); }
        else if w > best.z { best.z = w; pick.z = i; }
    }
    let a = surface(index,pick.x,local,dx,dy);
    var b = a; var c = a;
    if best.y > 0.0 { b = surface(index,pick.y,local,dx,dy); }
    if best.z > 0.0 { c = surface(index,pick.z,local,dx,dy); }
    return mix_surfaces(a,b,c,height_blend(best,vec3(a.color.a,b.color.a,c.color.a)));
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
struct NearResult { color: vec4<f32>, normal: vec3<f32>, ao: f32, roughness: f32, weight: f32, canopy: f32, }
fn close_ground(world: vec3<f32>, geometry_normal: vec3<f32>, dx: vec2<f32>, dy: vec2<f32>, cell_size: f32, origin: vec2<i32>) -> NearResult {
    var result = NearResult(vec4(0.0),geometry_normal,1.0,1.0,0.0,1.0);
    if near_settings.z < 0.5 { return result; }
    let distance_to_camera = distance(near_view::view.world_position,world);
    let range_weight = (1.0-smoothstep(near_settings.x,near_settings.y,distance_to_camera))
        * (1.0-smoothstep(0.06,0.18,max(length(dx),length(dy))));
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
    let s = blended_surfaces(index,uv,local,dx,dy);
    result.color = vec4(clamp(s.color.rgb*macro_response(index,local,dx,dy),vec3(0.0),vec3(1.0)),1.0);
    // World-X tangent with the same +Z bitangent as the original heightfield UVs.
    let n = normalize(geometry_normal);
    let t = normalize(vec3(n.y,-n.x,0.0)); let b = normalize(cross(t,n));
    result.normal = normalize(t*s.normal.x+b*s.normal.y+n*s.normal.z);
    result.ao = s.ao; result.roughness = s.roughness; result.weight = range_weight*availability;
    if near_table[index].canopy.appearance.x > 0.0 {
        let cover = textureSampleLevel(near_canopy,map_sampler,(uv*128.0+1.0)/130.0,near_table[index].key.w-1,0.0).rg;
        let shade = canopy_visibility_at(world.xz,0.0,distance_to_camera,near_table[index].canopy.appearance,near_table[index].canopy.shape,near_table[index].canopy.distance_settings,near_table[index].canopy.origin,near_table[index].canopy.appearance.y,cover.g*4.0,distance(near_view::view.world_position.xz,world.xz));
        result.canopy = mix(1.0,shade,cover.r);
    }
    return result;
}
