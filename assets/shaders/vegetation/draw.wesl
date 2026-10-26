// Grass draw: procedural blades write their surface into the G-buffer, lit afterwards as thin,
// glossy foliage (vegetation/deferred.wesl), and with TEMPORAL_GRASS the motion of the wind and
// the camera.
import package::shaders::vegetation::draw::bindings::{VertexOutput, camera};
@if(TEMPORAL_GRASS)
import package::shaders::vegetation::draw::bindings::temporal_grass;
import package::shaders::vegetation::draw::geometry::blade_vertex;
import package::shaders::vegetation::draw::{gbuffer, lighting::surface};

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
        output.previous_clip = (last_camera.clip_from_world * vec4(previous, 1.0)).xyw;
        let jitter = temporal_grass.raster_shift.xy * output.clip_position.w;
        output.clip_position += vec4(jitter, 0.0, 0.0);
    }
    return output;
}

struct GBufferFragment {
    @location(0) surface: vec4<u32>,
    @location(1) own: vec4<f32>,
    @if(TEMPORAL_GRASS) @location(2) motion: vec2<f32>,
}

@fragment fn fragment(input: VertexOutput) -> GBufferFragment {
    let texels = gbuffer::pack(surface(input), input.constants);
    var output: GBufferFragment;
    output.surface = texels.surface;
    output.own = texels.own;
    @if(TEMPORAL_GRASS) {
        // Where the fragment lies on screen this frame, unjittered.
        let size = camera.projection.yz;
        let raster = vec2(input.clip_position.x / size.x * 2.0 - 1.0,
            1.0 - input.clip_position.y / size.y * 2.0);
        let current = raster - temporal_grass.raster_shift.xy;
        output.motion = vec2(0.0);
        if input.previous_clip.z > 0.00001 {
            output.motion = (current - input.previous_clip.xy / input.previous_clip.z)
                * vec2(0.5, -0.5);
        }
    }
    return output;
}
