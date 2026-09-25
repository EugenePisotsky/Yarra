// Rain streaks: instanced camera-facing quads in camera-anchored boxes that wrap around a
// world-fixed drop lattice. Drawn over the resolved HDR scene with a soft depth test.
#import bevy_render::view::View
#import "shaders/clouds/types.wgsl"::{CloudParams, shelter_exposure}

struct Rain {
    velocity: vec4<f32>, // xyz m/s, w streak seconds
    shape: vec4<f32>, // drop width m, intensity alpha, soft depth m, forward shift
    offsets: array<vec4<f32>, 3>, // accumulated fall offset, w horizontal box size
    layers: array<vec4<f32>, 3>, // box height, alpha, first instance
    streak: vec4<f32>, // xyz drop velocity relative to the camera
    splash: vec4<f32>, // current time, lifetime s, size m
}
@group(0) @binding(0) var<uniform> rain: Rain;
@group(0) @binding(1) var<storage, read> clouds: CloudParams;
@group(0) @binding(2) var<uniform> view: View;
#ifdef MULTISAMPLED
@group(0) @binding(3) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(3) var depth: texture_depth_2d;
#endif
@group(0) @binding(4) var shelter_map: texture_2d<f32>;
// Recent impacts, two entries each: position and spawn time; surface normal and seed.
@group(0) @binding(5) var<storage, read> splashes: array<vec4<f32>>;

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

    // Most of each box sits ahead of the camera along the view direction, so a high camera
    // looking down still sees dense rain reaching the ground.
    let forward = normalize(-view.world_from_view[2].xyz);
    let anchor = view.world_position + forward * size.x * rain.shape.w + vec3(0.0, 0.1 * size.y, 0.0);
    // Drops are fixed in the world and move with the shared fall offset; the box wraps them.
    let lattice = random3(instance * 3u + 11u) * size + rain.offsets[layer].xyz;
    let head = anchor + (fract((lattice - anchor) / size + 0.5) - 0.5) * size;
    let variation = random3(instance * 7u + 5u);
    // Like a camera shutter: streaks follow the drop's motion relative to the camera.
    let tail = head - rain.streak.xyz * rain.velocity.w * mix(0.75, 1.25, variation.x);
    // No rain below a canopy: drops above the tree top still fall.
    let exposure = shelter_exposure(shelter_map, clouds.shelter, head);
    if exposure < 0.02 { return out; }

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
    // Drops brushing past the lens would become huge streaks; fade them out.
    let near_fade = smoothstep(0.4, 1.5, min(tail_clip.w, head_clip.w));
    out.alpha = rain.layers[layer].y * rain.shape.y * (true_width / width) * fog * exposure
        * near_fade * mix(0.6, 1.0, variation.y);
    return out;
}

// Linear scene depth under a fragment. Scene depth may be at a lower resolution after
// temporal reconstruction.
fn scene_depth(fragment: vec2<f32>) -> f32 {
    let scale = view.main_pass_viewport.zw / view.viewport.zw;
    let texel = vec2<i32>((fragment - view.viewport.xy) * scale + view.main_pass_viewport.xy);
    let z = textureLoad(depth, texel, 0);
    return select(1.0e9, view.clip_from_view[3][2] / z, z > 0.0);
}

// Drops scatter the surrounding sky and sun light, like the haze in front of clouds.
fn rain_light() -> vec3<f32> {
    return clouds.haze.rgb * (clouds.ambient.rgb * clouds.ambient.w * 0.3
        + clouds.sun_color.rgb * clouds.sun.w * 0.025
        + clouds.moon_color.rgb * clouds.moon.w * 0.025) * view.exposure * 1.4;
}

@fragment
fn fragment(in: Streak) -> @location(0) vec4<f32> {
    let across = 1.0 - smoothstep(0.3, 1.0, abs(in.coords.y));
    let ends = smoothstep(0.0, 0.3, in.coords.x) * (1.0 - smoothstep(0.85, 1.0, in.coords.x));
    let soft = clamp((scene_depth(in.position.xy) - in.view_depth) / rain.shape.z, 0.0, 1.0);
    let alpha = in.alpha * across * ends * soft;
    if alpha < 1e-4 { discard; }
    return vec4(rain_light() * alpha, alpha);
}

struct Splash {
    @builtin(position) position: vec4<f32>,
    // Ripple: position on the ring quad, -1..1. Droplet: position on the dot, -1..1.
    @location(0) coords: vec2<f32>,
    @location(1) view_depth: f32,
    @location(2) age: f32,
    @location(3) alpha: f32,
    // 0: ripple lying on the surface; 1: thrown droplet.
    @location(4) @interpolate(flat) kind: u32,
}

const SPLASH_GRAVITY: f32 = 9.8;

// Each impact draws a ripple lying on the surface and four droplets thrown from it.
@vertex
fn vertex_splash(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Splash {
    var out: Splash;
    out.position = vec4(0.0, 0.0, 0.0, 1.0);
    out.coords = vec2(0.0);
    out.view_depth = 0.0;
    out.age = 1.0;
    out.alpha = 0.0;
    out.kind = 0u;
    let origin = splashes[instance * 2u];
    let surface = splashes[instance * 2u + 1u];
    let age = (rain.splash.x - origin.w) / rain.splash.y;
    if age < 0.0 || age >= 1.0 { return out; }
    let normal = surface.xyz;
    let corner = vertex % 6u;
    let quad = vertex / 6u;
    let corner_x = select(-1.0, 1.0, corner == 1u || corner == 2u || corner == 4u);
    let corner_y = select(-1.0, 1.0, corner == 2u || corner == 4u || corner == 5u);
    let seed = u32(surface.w * 65535.0) * 16u + quad;
    var world: vec3<f32>;
    if quad == 0u {
        // Ripple on the sampled surface, slightly lifted to stay in front of it.
        let reference = select(vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), abs(normal.x) > 0.9);
        let tangent = normalize(cross(normal, reference));
        let bitangent = cross(normal, tangent);
        world = origin.xyz + normal * 0.01
            + (tangent * corner_x + bitangent * corner_y) * rain.splash.z;
    } else {
        // Droplets leave the surface in a cone and fall back under gravity.
        let random = random3(seed);
        let reference = select(vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), abs(normal.x) > 0.9);
        let tangent = normalize(cross(normal, reference));
        let bitangent = cross(normal, tangent);
        let angle = random.x * 6.2831853;
        let outward = (tangent * cos(angle) + bitangent * sin(angle)) * mix(0.3, 0.7, random.y);
        let velocity = outward + normal * mix(0.9, 1.5, random.z);
        let t = age * rain.splash.y;
        let centre = origin.xyz + velocity * t + vec3(0.0, -0.5 * SPLASH_GRAVITY * t * t, 0.0);
        let to_camera = view.world_position - centre;
        let side = cross(vec3(0.0, 1.0, 0.0), to_camera);
        let right = select(vec3(1.0, 0.0, 0.0), normalize(side), dot(side, side) > 1e-6);
        let up = normalize(cross(to_camera, right));
        world = centre + (right * corner_x + up * corner_y) * 0.008;
        out.kind = 1u;
    }
    let clip = view.unjittered_clip_from_world * vec4(world, 1.0);
    if clip.w < 0.1 { return out; }
    out.position = vec4(clip.xy, 0.5 * clip.w, clip.w);
    out.coords = vec2(corner_x, corner_y);
    out.view_depth = clip.w;
    out.age = age;
    out.alpha = rain.shape.y * exp(-clouds.fog.w * clip.w);
    return out;
}

@fragment
fn fragment_splash(in: Splash) -> @location(0) vec4<f32> {
    var shape: f32;
    if in.kind == 0u {
        // An expanding ring, thinning and fading as it spreads. Rings show on wet ground and
        // water; dry soil mostly shows the thrown droplets.
        let radius = mix(0.15, 1.0, sqrt(in.age));
        let ring = abs(length(in.coords) - radius);
        let wet = mix(0.15, 1.0, clamp(clouds.weather.x, 0.0, 1.0));
        shape = (1.0 - smoothstep(0.0, mix(0.12, 0.05, in.age), ring)) * (1.0 - in.age) * 0.2 * wet;
    } else {
        shape = (1.0 - smoothstep(0.3, 1.0, length(in.coords))) * (1.0 - in.age * in.age) * 0.4;
    }
    // The impact sits on the terrain. Allow a few centimetres for the rendered terrain LOD to
    // differ from the sampled height; anything nearer, such as grass blades, hides it.
    let soft = clamp((scene_depth(in.position.xy) + 0.08 - in.view_depth) / 0.06, 0.0, 1.0);
    let alpha = in.alpha * shape * soft;
    if alpha < 1e-4 { discard; }
    return vec4(rain_light() * 0.9 * alpha, alpha);
}
