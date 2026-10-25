// The frame's clock and count for shaders that run both in the main pass and in the prepass,
// whose view group holds Bevy's globals at another binding.
import bevy_render::globals::Globals;
@if(!PREPASS_PIPELINE) import bevy_pbr::render::mesh_view_bindings::globals;

@if(PREPASS_PIPELINE)
@group(0) @binding(1) var<uniform> globals: Globals;

fn time() -> f32 {
    return globals.time;
}

fn frame_count() -> u32 {
    return globals.frame_count;
}
