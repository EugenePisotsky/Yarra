struct Params { previous_from_current: mat4x4<f32>, size: vec4<f32>, jitter_debug: vec4<f32> }
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var vectors: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_2d;
@group(0) @binding(3) var color: texture_2d<f32>;
@group(0) @binding(4) var linear_sampler: sampler;
@vertex fn vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4(vec2(f32((i << 1u) & 2u),f32(i & 2u)) * 2.0 - 1.0,0.0,1.0);
}
@fragment fn motion(@builtin(position) p: vec4<f32>) -> @location(0) vec2<f32> {
    let xy = vec2<i32>(p.xy);
    if textureLoad(depth,xy,0) > 0.0 { return textureLoad(vectors,xy,0).xy; }
    // Background has no mesh prepass. Reproject directions, ignoring camera translation.
    let uv = (p.xy + params.jitter_debug.xy) / params.size.xy;
    let previous = params.previous_from_current * vec4(uv * vec2(2.0,-2.0) + vec2(-1.0,1.0),0.0,1.0);
    if previous.w <= 0.00001 { return vec2(0.0); }
    let previous_uv = previous.xy / previous.w * vec2(0.5,-0.5) + vec2(0.5);
    return uv - previous_uv;
}
@fragment fn display(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = p.xy / params.size.zw;
    let xy = vec2<i32>(clamp(uv * params.size.xy,vec2(0.0),params.size.xy - vec2(1.0)));
    if params.jitter_debug.z == 1.0 {
        let v = textureLoad(vectors,xy,0).xy * params.size.xy;
        return vec4(vec2(0.5) + v * 0.5,0.5,1.0);
    }
    if params.jitter_debug.z == 2.0 {
        let d = textureLoad(depth,xy,0);
        return vec4(vec3(pow(d,0.25)),1.0);
    }
    let sample_uv = clamp(uv * params.size.xy,vec2(0.5),params.size.xy-vec2(0.5)) / params.size.zw;
    return textureSampleLevel(color,linear_sampler,sample_uv,0.0);
}
