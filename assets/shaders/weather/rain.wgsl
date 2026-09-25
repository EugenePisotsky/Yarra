// Rain streaks: instanced camera-facing quads in camera-anchored boxes that wrap around a
// world-fixed drop lattice. Drawn over the resolved HDR scene with a soft depth test.
#import bevy_render::view::View
#import "shaders/clouds/types.wgsl"::CloudParams

struct Rain {
    velocity: vec4<f32>, // xyz m/s, w streak seconds
    shape: vec4<f32>, // drop width m, intensity alpha, soft depth m, forward shift
    offsets: array<vec4<f32>, 3>, // accumulated fall offset, w horizontal box size
    layers: array<vec4<f32>, 3>, // box height, alpha, first instance
}
@group(0) @binding(0) var<uniform> rain: Rain;
@group(0) @binding(1) var<storage, read> clouds: CloudParams;
@group(0) @binding(2) var<uniform> view: View;
#ifdef MULTISAMPLED
@group(0) @binding(3) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(3) var depth: texture_depth_2d;
#endif

struct Streak {
    @builtin(position) position: vec4<f32>,
    // x: tail (0) to head (1); y: across, -1..1.
    @location(0) coords: vec2<f32>,
    @location(1) view_depth: f32,
    @location(2) alpha: f32,
}

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}
fn random3(seed: u32) -> vec3<f32> {
    let a = pcg(seed);
    let b = pcg(a);
    let c = pcg(b);
    return vec3(f32(a), f32(b), f32(c)) / 4294967295.0;
}

@vertex
fn vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Streak {
    var out: Streak;
    out.position = vec4(0.0, 0.0, 0.0, 1.0);
    out.coords = vec2(0.0);
    out.view_depth = 0.0;
    out.alpha = 0.0;
    var layer = 0u;
    if f32(instance) >= rain.layers[1].z { layer = 1u; }
    if f32(instance) >= rain.layers[2].z { layer = 2u; }
    let size = vec3(rain.offsets[layer].w, rain.layers[layer].x, rain.offsets[layer].w);

    // Most of each box sits ahead of the camera, where drops are visible.
    let forward = -view.world_from_view[2].xyz;
    let ahead = select(vec2(0.0), normalize(forward.xz), dot(forward.xz, forward.xz) > 1e-6);
    let anchor = view.world_position
        + vec3(ahead.x * size.x, 0.1 * size.y, ahead.y * size.z) * vec3(rain.shape.w, 1.0, rain.shape.w);
    // Drops are fixed in the world and move with the shared fall offset; the box wraps them.
    let lattice = random3(instance * 3u + 11u) * size + rain.offsets[layer].xyz;
    let head = anchor + (fract((lattice - anchor) / size + 0.5) - 0.5) * size;
    let variation = random3(instance * 7u + 5u);
    let tail = head - rain.velocity.xyz * rain.velocity.w * mix(0.75, 1.25, variation.x);

    // Unjittered: streaks are drawn after temporal reconstruction.
    let head_clip = view.unjittered_clip_from_world * vec4(head, 1.0);
    let tail_clip = view.unjittered_clip_from_world * vec4(tail, 1.0);
    if head_clip.w < 0.1 || tail_clip.w < 0.1 { return out; }

    let corner = vertex % 6u;
    let along = select(0.0, 1.0, corner == 1u || corner == 2u || corner == 4u);
    let side = select(-1.0, 1.0, corner == 2u || corner == 4u || corner == 5u);
    let half_size = view.viewport.zw * 0.5;
    let head_px = head_clip.xy / head_clip.w * half_size;
    let tail_px = tail_clip.xy / tail_clip.w * half_size;
    let span = head_px - tail_px;
    let span_length = length(span);
    let axis = select(vec2(0.0, 1.0), span / span_length, span_length > 1e-4);
    let normal = vec2(-axis.y, axis.x);
    let w = mix(tail_clip.w, head_clip.w, along);
    // At least one pixel wide and two long; thinner drops fade to keep their coverage.
    let focal = view.clip_from_view[1][1] * half_size.y;
    let true_width = rain.shape.x * focal / w;
    let width = max(true_width, 1.0);
    let extend = max(0.0, 2.0 - span_length) * 0.5;
    let pixel = mix(tail_px - axis * extend, head_px + axis * extend, along)
        + axis * (along * 2.0 - 1.0) * width * 0.5
        + normal * side * width * 0.5;
    out.position = vec4(pixel / half_size * w, 0.5 * w, w);
    out.coords = vec2(along, side);
    out.view_depth = w;
    let fog = exp(-clouds.fog.w * w);
    out.alpha = rain.layers[layer].y * rain.shape.y * (true_width / width) * fog
        * mix(0.6, 1.0, variation.y);
    return out;
}

@fragment
fn fragment(in: Streak) -> @location(0) vec4<f32> {
    let across = 1.0 - smoothstep(0.3, 1.0, abs(in.coords.y));
    let ends = smoothstep(0.0, 0.3, in.coords.x) * (1.0 - smoothstep(0.85, 1.0, in.coords.x));
    // Scene depth may be at a lower resolution after temporal reconstruction.
    let scale = view.main_pass_viewport.zw / view.viewport.zw;
    let texel = vec2<i32>((in.position.xy - view.viewport.xy) * scale + view.main_pass_viewport.xy);
    let z = textureLoad(depth, texel, 0);
    let near = view.clip_from_view[3][2];
    let scene_depth = select(1.0e9, near / z, z > 0.0);
    let soft = clamp((scene_depth - in.view_depth) / rain.shape.z, 0.0, 1.0);
    let alpha = in.alpha * across * ends * soft;
    if alpha < 1e-4 { discard; }
    // Drops scatter the surrounding sky and sun light, like the haze in front of clouds.
    let light = clouds.haze.rgb * (clouds.ambient.rgb * clouds.ambient.w * 0.3
        + clouds.sun_color.rgb * clouds.sun.w * 0.025
        + clouds.moon_color.rgb * clouds.moon.w * 0.025) * view.exposure * 1.4;
    return vec4(light * alpha, alpha);
}
