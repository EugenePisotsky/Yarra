// The draw's own bind groups and varyings. Group zero is Bevy's mesh-view bind group, including
// directional shadow cascades; group two holds the cloud and forest-shadow surface inputs.
import package::shaders::vegetation::types::{
    ProceduralInstance, DiagnosticInstance, Species, Camera, Config,
};
import package::shaders::vegetation::blade::PreparedArena;

struct VertexOutput {
    @if(TEMPORAL_GRASS) @location(11) current_clip: vec4<f32>,
    @if(TEMPORAL_GRASS) @location(12) previous_clip: vec4<f32>,
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) blade_t: f32,
    // x: roughness, y: transmission, z: AO, w: geometry/diagnostic flag
    @location(4) material: vec4<f32>,
    @location(5) blade_u: f32,
    // xyz: terrain surface normal, w: stable clump variant
    @location(6) surface_normal_clump: vec4<f32>,
    // xyz: physical width axis before view opening, w: authored normal-rounding strength
    @location(7) ribbon_side_rounding: vec4<f32>,
    // Height above root plane, camera distance, grass-boundary depth, horizontal camera distance.
    @location(10) canopy_coordinates: vec4<f32>,
}

@group(1) @binding(0) var<storage, read> procedural_instances: array<ProceduralInstance>;
@group(1) @binding(1) var<storage, read> diagnostic_instances: array<DiagnosticInstance>;
@group(1) @binding(2) var<storage, read> species: array<Species>;
@group(1) @binding(3) var<uniform> camera: Camera;
@group(1) @binding(4) var<uniform> config: Config;
@group(1) @binding(5) var<storage, read> prepared_arena: PreparedArena;
// Independently uploaded, asynchronous canopy boundary; never part of placement coverage.
@group(1) @binding(6) var<storage, read> canopy_field: array<f32>;

@if(TEMPORAL_GRASS)
struct TemporalGrass { previous: Camera, raster_clip: mat4x4<f32> }
@if(TEMPORAL_GRASS)
@group(3) @binding(0) var<uniform> temporal_grass: TemporalGrass;
@if(TEMPORAL_GRASS)
@group(3) @binding(1) var<storage,read> previous_prepared_arena: PreparedArena;
