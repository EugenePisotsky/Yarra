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
    let blade = blade_vertex(vertex_index, instance_index);
    var output = blade.output;
    @if(TEMPORAL_GRASS) {
        // Last frame's render space lies apart from this frame's by the render origin's move.
        let last_camera = temporal_grass.previous;
        let shift = camera.render_origin.xy - last_camera.render_origin.xy;
        let previous = blade.previous + vec3(shift.x, 0.0, shift.y);
        output.current_clip = output.clip_position;
        output.previous_clip = last_camera.clip_from_world * vec4(previous, 1.0);
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
