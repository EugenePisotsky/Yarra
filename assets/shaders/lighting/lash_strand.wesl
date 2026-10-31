// Lash strands' vertex shader (crates/engine/src/character_skin.rs): Bevy 0.20's mesh vertex path
// (bevy_pbr render/mesh.wesl) for a skinned, uncompressed mesh, each strand held to a pixel's width.
// A strand thinner than a pixel falls between the samples and breaks into specks; widened, it fades
// by as much, so it covers what it should.
import bevy_pbr::render::{
    mesh_functions,
    mesh_view_bindings::view,
    skinning,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
};

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(5) color: vec4<f32>,
    @location(6) joint_indices: vec4<u32>,
    @location(7) joint_weights: vec4<f32>,
    // From the strand's middle to this edge, in the mesh's space.
    @location(10) across: vec3<f32>,
}

// The narrowest a strand draws, in pixels across.
const MIN_PIXELS: f32 = 1.25;

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = skinning::skin_model(
        vertex.joint_indices,
        vertex.joint_weights,
        vertex.instance_index,
    );
    let at = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(vertex.position, 1.0));
    let across = mat3x3(world_from_local[0].xyz, world_from_local[1].xyz, world_from_local[2].xyz) * vertex.across;
    let half_width = length(across);
    let pixel = position_world_to_clip(at.xyz).w * 2.0 / (view.clip_from_view[1][1] * view.main_pass_viewport.w);
    let widened = max(pixel * MIN_PIXELS * 0.5 / max(half_width, 1e-7), 1.0);

    out.world_position = vec4(at.xyz + across * (widened - 1.0), 1.0);
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = skinning::skin_normals(world_from_local, vertex.normal);
    out.uv = vertex.uv;
    out.color = vec4(vertex.color.rgb, vertex.color.a / widened);
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex.instance_index;
    return out;
}
