#import bevy_pbr::{
    mesh_view_bindings as view_bindings,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    shadows,
}

struct GroundShadowSlice {
    // xy: minimum world x/z, z: square extent, w: this slice's world height
    origin_extent: vec4<f32>,
    // x: world-space filter radius
    filter_params: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@group(1) @binding(0) var<uniform> shadow_slice: GroundShadowSlice;

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32(vertex_index >> 1u), f32(vertex_index & 1u)) * 2.0;
    var output: VertexOutput;
    output.clip_position = vec4<f32>(
        uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0),
        0.0,
        1.0,
    );
    output.uv = uv;
    return output;
}

fn exact_visibility(world_position: vec4<f32>, pixel_position: vec2<f32>) -> f32 {
    let view_z = dot(vec4<f32>(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z
    ), world_position);
    return shadows::fetch_directional_shadow(
        0u,
        world_position,
        vec3<f32>(0.0, 1.0, 0.0),
        view_z,
        pixel_position,
    );
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let light = &view_bindings::lights.directional_lights[0u];
    if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u) {
        return vec4<f32>(1.0);
    }

    let world_xz = shadow_slice.origin_extent.xy
        + input.uv * shadow_slice.origin_extent.z;
    let center = vec4<f32>(
        world_xz.x,
        shadow_slice.origin_extent.w,
        world_xz.y,
        1.0,
    );
    let radius = shadow_slice.filter_params.x;
    let x_offset = vec4<f32>(radius, 0.0, 0.0, 0.0);
    let z_offset = vec4<f32>(0.0, 0.0, radius, 0.0);

    // Grass cards need a shared low-frequency illumination field, not another crisp copy of
    // the CSM. A fixed world-space cross removes card-scale binary changes while preserving
    // the broad animated silhouette of characters, trees, and a moving sun.
    let visibility = exact_visibility(center, input.clip_position.xy) * 0.36
        + exact_visibility(center + x_offset, input.clip_position.xy) * 0.16
        + exact_visibility(center - x_offset, input.clip_position.xy) * 0.16
        + exact_visibility(center + z_offset, input.clip_position.xy) * 0.16
        + exact_visibility(center - z_offset, input.clip_position.xy) * 0.16;
    return vec4<f32>(visibility, visibility, visibility, 1.0);
}
