#import "shaders/terrain_stochastic.wgsl"::{
    stochastic_vertex_turn, stochastic_vertex_offset,
}

struct CacheSettings {
    origins: vec4<f32>,
    layers: vec4<f32>,
    size: vec4<u32>,
}
@group(0) @binding(0) var<uniform> settings: CacheSettings;
@group(0) @binding(1) var<storage, read_write> output: array<vec4<f32>>;

@compute @workgroup_size(8, 8, 1)
fn build_cache(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= settings.size.xy) || id.z >= 2u {
        return;
    }
    let origin = select(settings.origins.xy, settings.origins.zw, id.z == 1u);
    let layer = i32(select(settings.layers.x, settings.layers.y, id.z == 1u) + 0.5);
    let vertex = origin + vec2<f32>(id.xy);
    output[id.x + settings.size.x * (id.y + id.z * settings.size.y)] = vec4(
        stochastic_vertex_offset(vertex, layer),
        stochastic_vertex_turn(vertex, layer), 0.0,
    );
}
