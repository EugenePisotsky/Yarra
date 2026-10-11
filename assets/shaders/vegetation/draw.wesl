// Grass draw: procedural blades shaded as thin, glossy foliage. With TEMPORAL_GRASS it also
// writes the motion of the wind and the camera.
import package::shaders::vegetation::draw::bindings::{VertexOutput, camera};
@if(TEMPORAL_GRASS)
import package::shaders::vegetation::draw::bindings::temporal_grass;
import package::shaders::vegetation::draw::geometry::blade_vertex;
import package::shaders::vegetation::draw::lighting::shade;

@vertex
fn vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    var output = blade_vertex(vertex_index, instance_index, camera, false);
    @if(TEMPORAL_GRASS) {
        let previous = blade_vertex(vertex_index, instance_index, temporal_grass.previous, true);
        output.current_clip = output.clip_position;
        output.previous_clip = previous.clip_position;
        output.clip_position = temporal_grass.raster_clip * vec4(output.world_position, 1.0);
    }
    return output;
}

@if(TEMPORAL_GRASS)
struct TemporalFragment { @location(0) color: vec4<f32>, @location(1) motion: vec2<f32> }
@if(TEMPORAL_GRASS)
@fragment fn fragment(input: VertexOutput) -> TemporalFragment {
    var output: TemporalFragment;
    output.color = shade(input);
    output.motion = vec2(0.0);
    if input.current_clip.w > 0.00001 && input.previous_clip.w > 0.00001 {
        output.motion = (input.current_clip.xy / input.current_clip.w
            - input.previous_clip.xy / input.previous_clip.w) * vec2(0.5, -0.5);
    }
    return output;
}
@else
@fragment fn fragment(input: VertexOutput) -> @location(0) vec4<f32> { return shade(input); }
