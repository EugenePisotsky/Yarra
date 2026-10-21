// The ground albedo map around the camera (crates/terrain_render/src/ground_albedo.rs, read by
// lighting/ground.wesl): per texel, the surfaces the clipmap's control samples name there, each
// by its texture's mean colour under its grass field's, blended by their shares, and how much of
// the sun's beam the ground there takes: the cosine of its slope to the sun, none behind the
// terrain's horizon. A slope turned from the sun or in a ridge's shadow reflects only the sky's
// light. Water lies where the ground is below the open sea. A texel takes four points across it
// from the level whose control samples lie half a texel apart, so each averages the ground it
// covers. Bound as the clipmap material binds the textures it shares (MATERIAL_BIND_GROUP=0).
import package::shaders::terrain::clipmap_common::{ControlCell, base_color, control_cell, heights, leaf_spacing, level_normal, levels, origin_samples, render_xz, repeat_sampler, resident_level, surfaces, texel, unpack};
import package::shaders::lighting::surface::environment;
import package::shaders::lighting::terrain_shadow::terrain_sunlight;

struct Placement {
    // Each layer's first corner in render XZ: near, far.
    corners: vec4<f32>,
    // Metres per texel of each layer, the open sea's level, 1 with a sea.
    texels: vec4<f32>,
}

@group(0) @binding(16) var albedo_map: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(17) var<uniform> placement: Placement;

// Surfaces a control sample can name: five bits a layer (world::pack_control).
const SURFACES: u32 = 32u;
// How much of a grass field's colour its ground takes past the blades (terrain/clipmap.wesl
// FIELD_STRENGTH): what the ground shows the light above it.
const FIELD_STRENGTH: f32 = 0.85;
// The open sea reflects little of the light on it back up, most of it the sky's.
const WATER_ALBEDO: vec3<f32> = vec3(0.04, 0.06, 0.07);

var<workgroup> albedos: array<vec3<f32>, SURFACES>;

// A surface's albedo as the ground shows it from above.
fn surface_albedo(layer: u32) -> vec3<f32> {
    if layer >= arrayLength(&surfaces) || layer >= textureNumLayers(base_color) {
        return vec3(0.0);
    }
    let s = surfaces[layer];
    let last = f32(textureNumLevels(base_color) - 1u);
    let mean = textureSampleLevel(base_color, repeat_sampler, vec2(0.5), layer, last).rgb;
    return mix(mean, s.grass.rgb, FIELD_STRENGTH * clamp(s.grass.a, 0.0, 1.0));
}

fn sample_albedo(packed: u32) -> vec3<f32> {
    let s = unpack(packed);
    return albedos[s.layers.x] * s.shares.x + albedos[s.layers.y] * s.shares.y
        + albedos[s.layers.z] * s.shares.z;
}

fn cell_albedo(cell: ControlCell) -> vec3<f32> {
    let low = mix(sample_albedo(cell.packed.x), sample_albedo(cell.packed.y), cell.f.x);
    let high = mix(sample_albedo(cell.packed.z), sample_albedo(cell.packed.w), cell.f.x);
    return mix(low, high, cell.f.y);
}

@compute @workgroup_size(8, 8, 1)
fn fill(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    if local < SURFACES {
        albedos[local] = surface_albedo(local);
    }
    workgroupBarrier();

    let layer = id.z;
    let corner = select(placement.corners.xy, placement.corners.zw, layer == 1u);
    let leaves = i32(round(placement.texels[layer] / leaf_spacing()));
    let first = origin_samples() + vec2<i32>(round(corner / leaf_spacing())) + vec2<i32>(id.xy) * leaves;
    let level = resident_level(min(firstTrailingBit(u32(leaves)), levels() - 1u), first + leaves / 2);
    let sun = environment.sun.xyz;
    let sea = placement.texels.w > 0.5;
    var albedo = vec3(0.0);
    var sunlit = 0.0;
    for (var i = 0u; i < 4u; i += 1u) {
        let at = (vec2(f32(i & 1u), f32(i >> 1u)) * 0.5 + 0.25) * f32(leaves);
        let leaf = first + vec2<i32>(floor(at));
        let fraction = fract(at);
        let height = textureLoad(heights, texel(leaf >> vec2(level)), level, 0).r;
        let xz = render_xz(leaf) + fraction * leaf_spacing();
        let past = terrain_sunlight(vec3(xz.x, height, xz.y), sun);
        if sea && height < placement.texels.z {
            albedo += WATER_ALBEDO;
            sunlit += max(sun.y, 0.0) * past;
        } else {
            albedo += cell_albedo(control_cell(level, leaf, fraction));
            sunlit += max(dot(level_normal(level, leaf, fraction), sun), 0.0) * past;
        }
    }
    textureStore(albedo_map, vec2<i32>(id.xy), layer, vec4(albedo, sunlit) * 0.25);
}
