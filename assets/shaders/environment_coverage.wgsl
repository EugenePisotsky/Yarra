#import bevy_pbr::forward_io::{VertexOutput, FragmentOutput}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> bounds: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> grid: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var coverage: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var coverage_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    // Source masks include both endpoints; sample the centers of the edge texels exactly.
    let local = (in.world_position.xz - bounds.xy) * bounds.zw;
    let uv = local * grid.x + vec2(grid.y);
    let value = textureSample(coverage, coverage_sampler, uv).r;
    var out: FragmentOutput;
    out.color = vec4(mix(vec3(0.015, 0.025, 0.045), vec3(0.02, 0.9, 0.8), value), 0.86);
    return out;
}
