// The draw's own bind groups and varyings. Group two holds the environment's surface inputs; the
// lighting kernel (vegetation/deferred.wesl) binds Bevy's mesh-view group at zero, with the
// directional shadow cascades, and the G-buffer pass none.
import package::shaders::vegetation::types::{Instance, GrassType, Camera, BladePose};
@if(TEMPORAL_GRASS)
import package::shaders::vegetation::types::TemporalGrass;

// Every vertex's outputs are written out for the tiler, which on Apple's GPUs costs more than a
// blade vertex's math (docs/DECISIONS.md), so a vertex carries only what varies along its blade
// and one word of what does not; the fragment takes the rest from the grass type.
struct VertexOutput {
    // Last frame's clip position x, y and w; this frame's is the fragment's own, less the jitter
    // (draw.wesl `fragment`).
    @if(TEMPORAL_GRASS) @location(6) previous_clip: vec3<f32>,
    @builtin(position) clip_position: vec4<f32>,
    // Albedo at this point of the blade.
    @location(0) color: vec3<f32>,
    // The blade's physical normal, before facing the viewer.
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    // x: 0 at the root, 1 at the tip; y: -1..1 across the width; z: how deep in the field's
    // canopy this point sits (0 at its top, 1 on the ground).
    @location(3) blade: vec3<f32>,
    // The blade's width axis.
    @location(4) side: vec3<f32>,
    // The same at every vertex of a blade: its clump's stable normal, from which unresolved
    // blades take their highlight, in the G-buffer's 16 bits (lighting/gbuffer.wesl
    // `pack_normal16`), then its grass type's index.
    @location(5) @interpolate(flat) constants: u32,
}

@group(1) @binding(0) var<storage, read> instances: array<Instance>;
@group(1) @binding(1) var<storage, read> types: array<GrassType>;
@group(1) @binding(2) var<uniform> camera: Camera;
// Each blade's curve this frame and last (pose.wesl), two records per instance.
@group(1) @binding(3) var<storage, read> poses: array<BladePose>;

@if(TEMPORAL_GRASS)
@group(3) @binding(0) var<uniform> temporal_grass: TemporalGrass;
