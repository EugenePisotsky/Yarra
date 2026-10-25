// Grass's deferred lighting (crates/vegetation_render/src/renderer/deferred.rs): every pixel the
// blades left in the G-buffer (draw/gbuffer.wesl) is lit once by the grass lighting
// (draw/lighting.wesl), instead of on every 2×2 quad of every blade drawn over it.
import package::shaders::lighting::{deferred_pixel, gbuffer::{MODEL_GRASS, model}};
import package::shaders::vegetation::draw::{gbuffer, lighting::light};

// The grass's own G-buffer texel.
@group(3) @binding(0) var grass_texture: texture_2d<f32>;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return deferred_pixel::cover(index);
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = deferred_pixel::texel(position);
    if model(texel) != MODEL_GRASS {
        discard;
    }
    let own = textureLoad(grass_texture, vec2<i32>(position.xy), 0);
    let pixel = deferred_pixel::pixel(position);
    return vec4(light(gbuffer::unpack(texel, own), pixel.world_position, position.xy), 1.0);
}
