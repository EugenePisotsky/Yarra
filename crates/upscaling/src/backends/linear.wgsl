struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return VertexOutput(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0), uv);
}
@group(0) @binding(0) var input_color: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@fragment fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSampleLevel(input_color, input_sampler, in.uv, 0.0);
}
