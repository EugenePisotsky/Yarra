// The draw's own bind groups and varyings. Group zero is Bevy's mesh-view bind group, including
// directional shadow cascades; group two holds the cloud and forest-shadow surface inputs.
import package::shaders::vegetation::types::{Instance, GrassType, Camera};

struct VertexOutput {
    @if(TEMPORAL_GRASS) @location(11) current_clip: vec4<f32>,
    @if(TEMPORAL_GRASS) @location(12) previous_clip: vec4<f32>,
    @builtin(position) clip_position: vec4<f32>,
    // Albedo at this point of the blade.
    @location(0) color: vec3<f32>,
    // The blade's physical normal, before facing the viewer.
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    // 0 at the root, 1 at the tip.
    @location(3) blade_t: f32,
    // x: roughness, y: translucency, z: gloss, w: ambient occlusion
    @location(4) material: vec4<f32>,
    // -1..1 across the blade's width.
    @location(5) blade_u: f32,
    // The clump's stable normal: unresolved blades take their highlight from it.
    @location(6) clump_normal: vec3<f32>,
    // The blade's width axis.
    @location(7) side: vec3<f32>,
    // x: how deep in the field's canopy this point sits (0 at its top, 1 on the ground),
    // y: the canopy's height in metres.
    @location(8) canopy: vec2<f32>,
}

@group(1) @binding(0) var<storage, read> instances: array<Instance>;
@group(1) @binding(1) var<storage, read> types: array<GrassType>;
@group(1) @binding(2) var<uniform> camera: Camera;

@if(TEMPORAL_GRASS)
struct TemporalGrass { previous: Camera, raster_clip: mat4x4<f32> }
@if(TEMPORAL_GRASS)
@group(3) @binding(0) var<uniform> temporal_grass: TemporalGrass;
