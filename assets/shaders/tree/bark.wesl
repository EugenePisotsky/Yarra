// Upper bark layer of two-layer tree bark (crates/engine/src/trees/bark.rs).
import bevy_pbr::render::pbr_types::PbrInput;
import bevy_pbr::render::pbr_bindings;

@group(constants::MATERIAL_BIND_GROUP) @binding(101) var upper_color: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var upper_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var upper_normal: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var upper_metallic_roughness: texture_2d<f32>;

// How far the blend reaches past the lower bark's relief: the upper bark shows first in the
// lower bark's dark furrows and closes over its plates last.
const RELIEF_SOFTNESS: f32 = 0.25;

// Blends the already-built lower bark towards the upper bark by `weight`. `occlusion` is
// the vertex colour the lower bark's base colour was multiplied by. The upper normal map
// is decoded in the mesh's own tangent frame. Samples are unconditional: texture
// sampling must stay in uniform control flow.
fn blend_upper_bark(
    pbr_input: ptr<function, PbrInput>,
    uv: vec2<f32>,
    occlusion: vec3<f32>,
    weight: f32,
    world_normal: vec3<f32>,
    world_tangent: vec4<f32>,
) {
    let color = textureSample(upper_color, upper_sampler, uv).rgb * occlusion;
    let roughness = textureSample(upper_metallic_roughness, upper_sampler, uv).g;
    let encoded = textureSample(upper_normal, upper_sampler, uv).rgb * 2.0 - 1.0;
    // The lower bark's relief: its brightness against the texture's mean (its last mip),
    // plates above 0.5 and furrows below.
    let lower = (*pbr_input).material.base_color.rgb;
    let mean = textureSampleLevel(pbr_bindings::base_color_texture, pbr_bindings::base_color_sampler, uv, 16.0).rgb * occlusion;
    let luminance = vec3(0.2126, 0.7152, 0.0722);
    let relief = clamp(0.5 * dot(lower, luminance) / max(dot(mean, luminance), 1e-4), 0.0, 1.0);
    let reach = clamp(weight, 0.0, 1.0) * (1.0 + 2.0 * RELIEF_SOFTNESS) - RELIEF_SOFTNESS;
    let w = smoothstep(relief - RELIEF_SOFTNESS, relief + RELIEF_SOFTNESS, reach);

    (*pbr_input).material.base_color = vec4(
        mix((*pbr_input).material.base_color.rgb, color, w),
        (*pbr_input).material.base_color.a,
    );
    (*pbr_input).material.perceptual_roughness =
        mix((*pbr_input).material.perceptual_roughness, roughness, w);

    let n = normalize(world_normal);
    let t = normalize(world_tangent.xyz - n * dot(world_tangent.xyz, n));
    let b = cross(n, t) * world_tangent.w;
    let upper_n = normalize(encoded.x * t + encoded.y * b + encoded.z * n);
    (*pbr_input).N = normalize(mix((*pbr_input).N, upper_n, w));
}
