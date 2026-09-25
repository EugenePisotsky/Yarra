#import "shaders/clouds/pbr_lighting.wgsl"::apply_pbr_lighting
#import "shaders/weather/ground_wetness.wgsl"::apply_rain_puddles
// World-projected composites with an independently resident close-up surface cache.
#import "shaders/terrain_near.wgsl"::{close_ground, map_sampler}
#import bevy_pbr::{
    decal::clustered::apply_decals,
    forward_io::{FragmentOutput, VertexOutput},
    pbr_fragment::pbr_input_from_vertex_output,
    pbr_functions::{ main_pass_post_lighting_processing},
    pbr_types,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> projection: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var color_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var response_map: texture_2d<f32>;

fn decode_normal(encoded: vec2<f32>) -> vec3<f32> {
    let oct = encoded * 2.0 - 1.0;
    var n = vec3(oct.x, 1.0 - abs(oct.x) - abs(oct.y), oct.y);
    if n.y < 0.0 {
        n = vec3((1.0 - abs(n.zx)) * select(vec2(-1.0), vec2(1.0), n.xz >= vec2(0.0)), n.y).xzy;
    }
    return normalize(n);
}

struct DetailProjection { origin: vec4<i32>, world: vec4<f32>, }
struct DetailEntry { key: vec4<i32>, state: vec4<f32>, }
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> detail: DetailProjection;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var detail_color: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var detail_response: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var<storage, read> detail_table: array<DetailEntry>;

fn lookup(key: vec3<i32>) -> DetailEntry {
    var h = u32(key.x) * 0x9e3779b9u ^ u32(key.y) * 0x85ebca6bu ^ u32(key.z) * 0xc2b2ae35u;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    let mask = u32(detail.origin.z);
    for (var probe = 0u; probe <= mask; probe += 1u) {
        let entry = detail_table[(h + probe) & mask];
        if entry.key.w == 0 || all(entry.key.xyz == key) { return entry; }
    }
    return DetailEntry(vec4<i32>(0), vec4(0.0));
}
struct GroundSample { color: vec4<f32>, normal: vec3<f32>, roughness: f32, ao: f32, }
fn ground_sample(color: vec4<f32>, response: vec4<f32>) -> GroundSample {
    return GroundSample(color, decode_normal(response.rg), response.b, response.a);
}
fn local_uv(cell: vec2<i32>, fraction: vec2<f32>, entry: DetailEntry) -> vec2<f32> {
    let level = u32(entry.key.z);
    let minimum = vec2(entry.key.x << level, entry.key.y << level);
    return (vec2<f32>(cell - minimum) + fraction) / f32(1u << level);
}
fn sample_tile(cell: vec2<i32>, fraction: vec2<f32>, entry: DetailEntry, footprint: f32) -> GroundSample {
    let uv = (local_uv(cell, fraction, entry) * 64.0 + 4.0) / 72.0;
    let mip = clamp(log2(max(footprint * 64.0 / (detail.world.x * f32(1u << u32(entry.key.z))), 0.000001)), 0.0, 2.0);
    return ground_sample(
        textureSampleLevel(detail_color, map_sampler, uv, entry.key.w - 1, mip),
        textureSampleLevel(detail_response, map_sampler, uv, entry.key.w - 1, mip));
}
fn detail_weight(entry: DetailEntry, uv: vec2<f32>, lod: f32) -> f32 {
    var availability = entry.state.x;
    let offsets = array<vec2<i32>,4>(vec2(-1,0), vec2(1,0), vec2(0,-1), vec2(0,1));
    let edges = vec4(uv.x, 1.0-uv.x, uv.y, 1.0-uv.y) * 64.0;
    for (var i = 0u; i < 4u; i += 1u) {
        if edges[i] < 2.0 {
            let neighbour = lookup(vec3(entry.key.xy + offsets[i], entry.key.z));
            availability = min(availability, mix(neighbour.state.x, 1.0, smoothstep(0.0, 2.0, edges[i])));
        }
    }
    return availability * (1.0 - smoothstep(0.0, 1.0, lod - f32(entry.key.z)));
}
fn detailed_ground(base: GroundSample, world: vec2<f32>, footprint: f32) -> GroundSample {
    if detail.origin.w == 0 { return base; }
    let render_cell = world / detail.world.x;
    let cell = vec2<i32>(floor(render_cell)) + detail.origin.xy;
    let fraction = fract(render_cell);
    let lod = max(log2(max(footprint * 64.0 / detail.world.x, 0.000001)), 0.0);
    var result = GroundSample(vec4(0.0), vec3(0.0), 0.0, 0.0);
    var remaining = 1.0;
    // Usually one fully available tile ends the loop. At a loading boundary,
    // accumulate ancestors too: skipping their edge fades would reveal seams
    // wherever adjacent regions differ by more than one material level.
    for (var level = u32(min(floor(lod), 30.0)); level < u32(detail.world.y); level += 1u) {
        let entry = lookup(vec3(cell.x >> level, cell.y >> level, i32(level)));
        if entry.key.w == 0 { continue; }
        let weight = detail_weight(entry, local_uv(cell, fraction, entry), lod);
        if weight <= 0.0 { continue; }
        let sample = sample_tile(cell, fraction, entry, footprint);
        let contribution = remaining * weight;
        result.color += sample.color * contribution;
        result.normal += sample.normal * contribution;
        result.roughness += sample.roughness * contribution;
        result.ao += sample.ao * contribution;
        remaining *= 1.0 - weight;
        if remaining == 0.0 { break; }
    }
    result.color += base.color * remaining;
    result.normal = normalize(result.normal + base.normal * remaining);
    result.roughness += base.roughness * remaining;
    result.ao += base.ao * remaining;
    return result;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var out: FragmentOutput;
#ifdef TERRAIN_FLAT
    out.color = vec4(0.05, 0.12, 0.025, 1.0);
#else ifdef TERRAIN_SINGLE_TEXTURE
    let uv = ((in.world_position.xz - projection.xy) / projection.z * 64.0 + 4.0) / 72.0;
    out.color = textureSample(color_map, map_sampler, uv);
#else
    let world_dx = dpdx(in.world_position.xz);
    let world_dy = dpdy(in.world_position.xz);
    let footprint = max(length(world_dx), length(world_dy));
    var canopy = 1.0;
    var pbr = pbr_input_from_vertex_output(in, is_front, false);
    // Same placeholder as the geometry diagnostic when the publication has no bake.
    pbr.material.base_color = vec4(0.1128048, 0.1548725, 0.0684782, 1.0);
    pbr.material.perceptual_roughness = 1.0;
    if projection.w > 0.5 {
        var ground: GroundSample;
#ifndef TERRAIN_NEAR_DISABLED
        // Close material normals use the actual mesh, as the original detailed
        // renderer does. This also makes fully resident close ground independent
        // of the distant material cache and its lower-resolution normal field.
        let close = close_ground(in.world_position.xyz, in.world_normal, world_dx, world_dy, detail.world.x, detail.origin.xy);
        ground = GroundSample(close.color, close.normal, close.roughness, close.ao);
        canopy = mix(1.0, close.canopy, close.weight);
        if close.weight < 1.0 {
#endif
        let tile_uv = (in.world_position.xz - projection.xy) / projection.z;
        let uv = (tile_uv * 64.0 + 4.0) / 72.0;
        // Derivatives were taken before the per-pixel availability branch.
        let uv_dx = world_dx / projection.z * (64.0 / 72.0);
        let uv_dy = world_dy / projection.z * (64.0 / 72.0);
        let response = textureSampleGrad(response_map, map_sampler, uv, uv_dx, uv_dy);
        let base = ground_sample(textureSampleGrad(color_map, map_sampler, uv, uv_dx, uv_dy), response);
        ground = detailed_ground(base, in.world_position.xz, footprint);
#ifndef TERRAIN_NEAR_DISABLED
        ground.color = mix(ground.color, close.color, close.weight);
        ground.normal = normalize(mix(ground.normal, close.normal, close.weight));
        ground.roughness = mix(ground.roughness, close.roughness, close.weight);
        ground.ao = mix(ground.ao, close.ao, close.weight);
        }
#endif
        pbr.material.base_color = ground.color;
        pbr.N = ground.normal;
        pbr.clearcoat_N = pbr.N;
        pbr.material.perceptual_roughness = clamp(ground.roughness, 0.08, 1.0);
        pbr.diffuse_occlusion = vec3(ground.ao);
        pbr.specular_occlusion = ground.ao;
    }
    pbr.material.metallic = 0.0;
    pbr.material.reflectance = vec3(0.25);
    pbr.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    apply_decals(&pbr);
#ifdef TERRAIN_SURFACE_UNLIT
    out.color = pbr.material.base_color;
#else
    pbr = apply_rain_puddles(pbr);
    let lit = apply_pbr_lighting(pbr);
    out.color = main_pass_post_lighting_processing(pbr, vec4(lit.rgb * canopy, lit.a));
#endif
#endif
    return out;
}
