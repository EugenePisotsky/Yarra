// Detailed surface evaluation on any terrain mesh; inputs use canonical cell addresses.
import bevy_pbr::render::mesh_view_bindings as near_view;
import package::shaders::terrain_stochastic::{quarter_turn, stochastic_vertex_turn, stochastic_vertex_offset};
import package::shaders::terrain_blend::{BLEND_DEPTH, BLEND_HEIGHT, height_blend};
import package::shaders::lighting::surface::surface_weather;
import package::shaders::terrain_wear::{WEAR_TEXEL, map_sampler, wear_at, rut_at, lay_worn, bare_share, rut_parallax, wear_patch, wheel_grooves};
// One of a page's up to eight surfaces. `surface`: array layer, metres per repetition,
// anti-tiling (0/1), normal strength. `material`: normal Y sign, roughness minimum/maximum.
// `phase`/`lattice`: this page's UV and anti-tiling lattice offsets for the surface's tile size.
// `grass`: the grass field's linear colour and how fully grass covers the surface.
// `worn`: the page slots the surface wears down to (-1 none) and their shares.
struct NearSlot { surface: vec4<f32>, material: vec4<f32>, phase: vec4<f32>, lattice: vec4<f32>, grass: vec4<f32>, worn: vec4<f32>, }
// `wear.x`: whether the page has a wear field (terrain_wear.wesl); `wear.y`: its layer.
struct NearEntry {
    key: vec4<i32>, wear: vec4<f32>, state: vec4<f32>, macro_scales: vec4<f32>, macro_settings: vec4<f32>,
    macro_a: vec4<f32>, macro_b: vec4<f32>, macro_c: vec4<f32>, slots: array<NearSlot, 12>,
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

// Returned by near_lookup for a page near detail does not hold.
const NEAR_NONE: u32 = 0xffffffffu;
// Probe only keys. Returning the whole entry through a dynamic loop also
// carried every surface parameter through neighbour-only lookups. The table's size is a power
// of two (near::NEAR_TABLE).
fn near_lookup(key: vec2<i32>) -> u32 {
    let mask = arrayLength(&near_table) - 1u;
    var h = (u32(key.x) * 0x9e3779b9u) ^ (u32(key.y) * 0x85ebca6bu);
    h ^= h >> 16u; h *= 0x7feb352du; h ^= h >> 15u;
    h &= mask;
    // Most pixels hit the first slot. Keep that case outside the collision loop.
    let first = near_table[h].key;
    if first.w == 0 { return NEAR_NONE; }
    if all(first.xy == key) { return h; }
    for (var i=1u; i<=mask; i+=1u) {
        let index = (h+i)&mask;
        let stored = near_table[index].key;
        if stored.w == 0 { return NEAR_NONE; }
        if all(stored.xy == key) { return index; }
    }
    return NEAR_NONE;
}
fn near_availability(key: vec2<i32>) -> f32 {
    let index = near_lookup(key);
    if index == NEAR_NONE { return 0.0; }
    return near_table[index].state.x;
}
fn triangular(uv: vec2<f32>) -> vec2<f32> { return vec2(uv.x + uv.y*0.5773502692, uv.y*1.1547005384); }
// Anti-tiling from the lattice vertex with the largest weight alone: one sample instead of three.
// Seams between vertices are faint at the coarse mips far ground samples.
fn stochastic_dominant(uv: vec2<f32>, lattice: vec2<f32>, origin: vec2<f32>, layer: i32, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {
    let c = floor(lattice); let f = fract(lattice);
    var vertex = c;
    if f.x+f.y > 1.0 {
        vertex = c+select(select(vec2(1.0,0.0), vec2(0.0,1.0), 1.0-f.x > 1.0-f.y), vec2(1.0), f.x+f.y-1.0 > max(1.0-f.x,1.0-f.y));
    } else {
        vertex = c+select(select(vec2(0.0), vec2(1.0,0.0), f.x > 1.0-f.x-f.y), vec2(0.0,1.0), f.y > max(f.x,1.0-f.x-f.y));
    }
    let at = origin + vertex;
    let turn = stochastic_vertex_turn(at,layer);
    return textureSampleGrad(near_base, repeat_sampler, quarter_turn(uv,turn)+stochastic_vertex_offset(at,layer), layer, quarter_turn(dx,turn), quarter_turn(dy,turn));
}
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
// `far`: ground past FAR_GROUND, where the surface's colour comes from its coarse mips. Its
// normal, occlusion and roughness are then the composite's (the caller's), not the scan's.
fn surface(index: u32, slot: u32, local: vec2<f32>, world_dx: vec2<f32>, world_dy: vec2<f32>, far: bool) -> Surface {
    let s = near_table[index].slots[slot];
    let size = max(s.surface.y, 0.001);
    let layer = i32(s.surface.x+0.5);
    let uv = local/size + s.phase.xy;
    let dx = world_dx/size; let dy = world_dy/size;
    var color: vec4<f32>;
    if s.surface.z >= 0.5 && far {
        color = stochastic_dominant(uv, triangular(local/size)+s.lattice.zw, s.lattice.xy, layer, dx, dy);
    } else if s.surface.z >= 0.5 {
        color = stochastic(uv, triangular(local/size)+s.lattice.zw, s.lattice.xy, layer, dx, dy);
    } else { color = textureSampleGrad(near_base, repeat_sampler, uv, layer, dx, dy); }
    if far {
        return Surface(color, vec3(0.0, 0.0, 1.0), 1.0, 1.0);
    }
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
// A page's surface weights are one per slot: array<f32, 12> (world::MAX_TERRAIN_SURFACES_PER_CELL).
// Spelled out, not aliased: Metal translation treats an alias as a distinct type.
// The page's surface weights at `uv` (0..1 across the page): four per weight layer, a layer read
// only where the page has surfaces in it.
fn page_weights(index: u32, uv: vec2<f32>) -> array<f32, 12> {
    var w: array<f32, 12>;
    let count = u32(near_table[index].state.z+0.5);
    if count <= 1u { w[0] = 1.0; return w; }
    let wuv = (vec2(0.5)+uv*(near_table[index].state.y-1.0))/near_settings.w;
    let layer = (near_table[index].key.w-1)*3;
    for (var page = 0u; page*4u < count; page += 1u) {
        let v = textureSampleLevel(near_weights,map_sampler,wuv,layer+i32(page),0.0);
        w[page*4u] = v.x; w[page*4u+1u] = v.y; w[page*4u+2u] = v.z; w[page*4u+3u] = v.w;
    }
    return w;
}
// The page's weights worn by `wear`, `bare` of it bare, as `world::TerrainWear::lay` wears them.
fn worn_weights(index: u32, page: array<f32, 12>, wear: f32, bare: f32) -> array<f32, 12> {
    let count = u32(near_table[index].state.z+0.5);
    var worn: array<vec4<f32>, 12>;
    var trampled: array<f32, 12>;
    for (var i = 0u; i < count; i += 1u) {
        worn[i] = near_table[index].slots[i].worn;
        trampled[i] = near_table[index].slots[i].material.w;
    }
    return lay_worn(page, worn, trampled, count, wear, bare);
}
// The page's ground: its three largest weights here, blended by surface height, and the grass
// its surfaces carry (field colour, cover) in the same shares.
struct Ground { surface: Surface, grass: vec4<f32>, }
fn blended_surfaces(index: u32, page: array<f32, 12>, local: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, far: bool) -> Ground {
    let count = u32(near_table[index].state.z+0.5);
    if count <= 1u { return Ground(surface(index,0u,local,dx,dy,far), near_table[index].slots[0].grass); }
    var weights = page;
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
    let a = surface(index,pick.x,local,dx,dy,far);
    var cut = share.x+a.color.a*BLEND_HEIGHT-BLEND_DEPTH;
    var b = a; var c = a; var heights = vec3(a.color.a,-1.0e3,-1.0e3);
    if best.y > 0.0 && share.y+BLEND_HEIGHT > cut {
        b = surface(index,pick.y,local,dx,dy,far); heights.y = b.color.a;
        cut = max(cut,share.y+b.color.a*BLEND_HEIGHT-BLEND_DEPTH);
    }
    if best.z > 0.0 && share.z+BLEND_HEIGHT > cut {
        c = surface(index,pick.z,local,dx,dy,far); heights.z = c.color.a;
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
// Past this distance near ground keeps its crisp half-metre weights and the surfaces' colours,
// but samples each colour once (stochastic_dominant) and shades by the composite's normal,
// occlusion and roughness (NearResult.far): the scans' normals and anti-tiling blend are lost in
// their coarse mips there, and they made near ground to 240 m cost more than the frame had.
const FAR_GROUND: f32 = 120.0;
// Wheel ruts sink by parallax within this range, fading out over its second half.
const RUT_RANGE: vec2<f32> = vec2(20.0, 35.0);
// How deep the rut march looks below the terrain, metres.
const RUT_DEEPEST: f32 = 0.16;
// `hollow`: where rain stands in a rut here, as rain hollowness 0..1 (-1: no rut).
// `far`: the ground is past FAR_GROUND; take occlusion and roughness from the composite.
struct NearResult { color: vec4<f32>, normal: vec3<f32>, ao: f32, roughness: f32, weight: f32, hollow: f32, far: bool, }
fn close_ground(world: vec3<f32>, geometry_normal: vec3<f32>, dx: vec2<f32>, dy: vec2<f32>, cell_size: f32, origin: vec2<i32>) -> NearResult {
    var result = NearResult(vec4(0.0),geometry_normal,1.0,1.0,0.0,-1.0,false);
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
    if index == NEAR_NONE { return result; }
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
    if near_table[index].wear.x > 0.5 {
        let layer = i32(near_table[index].wear.y+0.5);
        let field = wear_at(layer,uv);
        if field.wear > 0.002 || field.rut > 0.0005 {
            let bare = bare_share(field.wear,vec2<f32>(cell)*cell_size+local);
            weights = worn_weights(index,weights,field.wear,bare);
            // Where wheels wear the ground bare, its edge is compacted and dark.
            let worn_bare = field.wear*bare;
            rim = worn_bare*(1.0-worn_bare)*4.0;
            compacted = worn_bare*0.25;
            let sink = 1.0-smoothstep(RUT_RANGE.x,RUT_RANGE.y,distance_to_camera);
            if sink > 0.0 && field.wear > 0.05 {
                let deepest = RUT_DEEPEST*sink;
                let view = normalize(near_view::view.world_position-world);
                let found = rut_parallax(layer,uv,cell_size,view,n,deepest);
                sampled = local+found.xy*sink;
                let at = uv+found.xy*sink/cell_size;
                // Rut walls tilt the ground; grooves of many wheels score the floors along the
                // traffic, fading before they would shimmer.
                let e = WEAR_TEXEL;
                let slope = vec2(rut_at(layer,at+vec2(e,0.0))-rut_at(layer,at-vec2(e,0.0)),
                    rut_at(layer,at+vec2(0.0,e))-rut_at(layer,at-vec2(0.0,e)))/(2.0*e*cell_size)*sink;
                n = normalize(n+vec3(slope.x,0.0,slope.y));
                let bottom = clamp(found.z/max(field.rut*1.3,0.02),0.0,1.0)*smoothstep(0.005,0.03,found.z);
                occlusion = 1.0-0.35*bottom;
                compacted = max(compacted,bottom*field.wear);
                let fine = 1.0-smoothstep(0.03,0.07,max(length(dx),length(dy)));
                if fine > 0.0 {
                    let g = wheel_grooves(vec2<f32>(cell)*cell_size+sampled, field.direction);
                    occlusion *= 1.0-0.12*g*abs(g)*fine*field.wear;
                }
                // Rain stands in dips of the rut floors a metre or three across, deepest first,
                // spreading as the ground soaks (weather/ground_wetness.wesl); fine noise breaks
                // the water line. Puddles form only on wet ground.
                if surface_weather().x > 0.3 {
                    let floor = vec2<f32>(cell)*cell_size+sampled;
                    let dips = wear_patch(floor,2.0,0x6e3u);
                    let wobble = wear_patch(floor,0.6,0x51du)-0.5;
                    result.hollow = clamp(bottom*bottom*dips*1.5-0.55+wobble*0.35,0.0,1.0);
                }
            }
        }
    }
    let far = distance_to_camera > FAR_GROUND;
    let blend = blended_surfaces(index,weights,sampled,dx,dy,far);
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
    result.far = far;
    return result;
}
