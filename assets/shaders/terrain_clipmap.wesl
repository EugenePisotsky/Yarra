// Terrain v2's surface (crates/terrain_render/src/clipmap): per-pixel normals from the
// clipmap at the pixel's footprint, and for now a flat colour by slope and altitude standing
// in for the surfaces (the control maps of the roadmap's second step).
import package::shaders::lighting::pbr_lighting::apply_pbr_lighting;
import package::shaders::terrain_clipmap_common::{clipmap_normal, leaf_spacing, origin_samples, settings};
import bevy_pbr::{
    render::forward_io::{FragmentOutput, VertexOutput},
    render::pbr_fragment::pbr_input_from_vertex_output,
    render::pbr_functions::main_pass_post_lighting_processing,
    render::pbr_types,
};

struct Ground { color: vec3<f32>, roughness: f32, }

// Debug ground: sand at the sea, grass, dry meadow higher up, rock on steep slopes, snow on
// the high ground that holds it.
fn debug_ground(height: f32, n: vec3<f32>) -> Ground {
    let grass = mix(vec3(0.075, 0.11, 0.035), vec3(0.20, 0.18, 0.09), smoothstep(150.0, 320.0, height));
    var color = mix(vec3(0.42, 0.37, 0.27), grass, smoothstep(1.5, 4.0, height));
    var roughness = 0.95;
    color = mix(color, vec3(0.19, 0.18, 0.17), smoothstep(0.82, 0.72, n.y));
    let snow = smoothstep(390.0, 430.0, height) * smoothstep(0.55, 0.7, n.y);
    color = mix(color, vec3(0.78, 0.79, 0.82), snow);
    roughness = mix(roughness, 0.6, snow);
    return Ground(color, roughness);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var out: FragmentOutput;
    let world_dx = dpdx(in.world_position.xz);
    let world_dy = dpdy(in.world_position.xz);
    // Geometric mean of the footprint's axes: the long axis at grazing angles alone would
    // blur the whole distance.
    let footprint = sqrt(length(world_dx) * length(world_dy));
    let r = in.world_position.xz / leaf_spacing();
    let cell = floor(r);
    let leaf = origin_samples() + vec2<i32>(cell);
    let lod = log2(max(footprint / leaf_spacing(), 1e-6)) + settings.ranges.w;
    let n = clipmap_normal(leaf, r - cell, lod);
    var pbr = pbr_input_from_vertex_output(in, is_front, false);
    let ground = debug_ground(in.world_position.y, n);
    pbr.material.base_color = vec4(ground.color, 1.0);
    pbr.material.perceptual_roughness = ground.roughness;
    pbr.material.metallic = 0.0;
    pbr.material.reflectance = vec3(0.25);
    pbr.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr.N = n;
    pbr.clearcoat_N = n;
    let lit = apply_pbr_lighting(pbr);
    out.color = main_pass_post_lighting_processing(pbr, lit);
    return out;
}
