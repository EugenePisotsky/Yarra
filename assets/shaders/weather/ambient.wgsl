// Ambient particles: dust motes, seed fluff and falling leaves as instanced quads in
// camera-anchored boxes that wrap a world-fixed lattice (crates/atmosphere/src/ambient_particles.rs).
// Drawn over the resolved HDR scene with a soft depth test, like the rain.
#import bevy_render::view::View
#import bevy_pbr::mesh_view_types::{Lights, DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT}
#import "shaders/clouds/types.wgsl"::CloudParams

struct Ambient {
    time: vec4<f32>, // x: fraction of the motion period, y: the period in seconds
    drift: vec4<f32>, // xyz: wind drift velocity (m/s) at full share
    kinds: array<vec4<f32>, 3>, // box size, box height, first instance, instance count
    offsets: array<vec4<f32>, 3>, // accumulated drift (m), w: the kind's drift share
    amounts: vec4<f32>, // share of each kind present
}
@group(0) @binding(0) var<uniform> ambient: Ambient;
@group(0) @binding(1) var<storage, read> clouds: CloudParams;
@group(0) @binding(2) var<uniform> view: View;
#ifdef MULTISAMPLED
@group(0) @binding(3) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(3) var depth: texture_depth_2d;
#endif
@group(0) @binding(4) var<uniform> lights: Lights;
@group(0) @binding(5) var shadow_maps: texture_depth_2d_array;
@group(0) @binding(6) var shadow_sampler: sampler_comparison;
@group(0) @binding(7) var cloud_shadow: texture_2d<f32>;
@group(0) @binding(8) var repeat_sampler: sampler;
@group(0) @binding(9) var forest_map: texture_2d<f32>;
@group(0) @binding(10) var mist_map: texture_2d<f32>;
@group(0) @binding(11) var clamp_sampler: sampler;

const MOTES: u32 = 0u;
const FLUFF: u32 = 1u;
const LEAVES: u32 = 2u;
const TAU: f32 = 6.2831853;
const FOUR_PI: f32 = 12.566371;
// Dust is flakes and fibres, not spheres: as they turn they glint far brighter than a sphere
// of their size would scatter.
const MOTE_GLINT: f32 = 4.0;
// A disk's area over the integral of the dots' falloff exp(-3 r^2) on the unit disk.
const DOT_NORMAL: f32 = 3.157;
// Lowest seed height over the ground.
const FLUFF_LIFT: f32 = 0.3;

struct Particle {
    @builtin(position) position: vec4<f32>,
    // Position on the quad, -1..1; leaves run along x.
    @location(0) coords: vec2<f32>,
    @location(1) view_depth: f32,
    // Exposed radiance and opacity.
    @location(2) radiance: vec3<f32>,
    @location(3) alpha: f32,
    @location(4) @interpolate(flat) kind: u32,
    // Leaves: 0 a broad leaf, 1 a needle sprig.
    @location(5) @interpolate(flat) shape: f32,
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

// A periodic motion of `cycles` whole turns per motion period, so the wrapping clock never
// makes it jump.
fn turn(cycles: f32, phase: f32) -> f32 {
    return TAU * fract(ambient.time.x * round(cycles) + phase);
}
fn wave(cycles: f32, phase: f32) -> f32 {
    return sin(turn(cycles, phase));
}

fn rotate(v: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    return v * c + cross(axis, v) * sin(angle) + axis * dot(axis, v) * (1.0 - c);
}

// Henyey-Greenstein phase per steradian; `mu` is the cosine between the light's travel and
// the view ray.
fn phase(mu: f32, g: f32) -> f32 {
    let d = 1.0 + g * g - 2.0 * g * mu;
    return (1.0 - g * g) / (FOUR_PI * d * sqrt(d));
}

// Sunlight past everything the sun's shadow maps hold (terrain, trunks, crowns, the
// character); 1 beyond them or without a shadowed sun.
fn shadow_map_visibility(p: vec3<f32>) -> f32 {
    var index = -1;
    for (var i = 0u; i < lights.n_directional_lights; i += 1u) {
        let light = &lights.directional_lights[i];
        if ((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u
            && dot((*light).direction_to_light, clouds.sun.xyz) > 0.995 {
            index = i32(i);
            break;
        }
    }
    if index < 0 {
        return 1.0;
    }
    let light = &lights.directional_lights[index];
    let distance = -(view.view_from_world * vec4(p, 1.0)).z;
    for (var c = 0u; c < (*light).num_cascades; c += 1u) {
        let cascade = &(*light).cascades[c];
        if distance >= (*cascade).far_bound {
            continue;
        }
        // Particles cast no shadow; the offset only keeps clear of the depth's precision.
        let lifted = p + (*light).direction_to_light * ((*light).shadow_depth_bias + 2.0 * (*cascade).texel_size);
        let clip = (*cascade).clip_from_world * vec4(lifted, 1.0);
        let ndc = clip.xyz / clip.w;
        if any(abs(ndc.xy) > vec2(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
            return 1.0;
        }
        let uv = ndc.xy * vec2(0.5, -0.5) + 0.5;
        return textureSampleCompareLevel(
            shadow_maps, shadow_sampler, uv, i32((*light).depth_texture_base_index + c), ndc.z);
    }
    return 1.0;
}

// Sunlight past the cloud layer's shadow, as surfaces see it.
fn cloud_visibility(p: vec3<f32>) -> f32 {
    let sun = clouds.sun.xyz;
    if clouds.layer.w < 0.5 || sun.y <= 0.0 || p.y >= clouds.layer.x + clouds.layer.y {
        return 1.0;
    }
    let hit = p.xz + clouds.offset.xy + sun.xz * (clouds.layer.x - p.y) / max(sun.y, 0.04);
    let shadow = textureSampleLevel(cloud_shadow, repeat_sampler,
        (hit - clouds.offset.zw) / (clouds.layer.z * 4.0), 0.0).r;
    return max(shadow, 0.12);
}

// Forest map texel under `xz`: highest crown top, lowest crown base, foliage density and
// canopy top (atmosphere/src/forest_shadow.rs). Density 0 off the map.
fn crown_at(xz: vec2<f32>) -> vec4<f32> {
    let parameters = clouds.forest_shadow;
    if parameters.z <= 0.0 {
        return vec4(0.0);
    }
    let size = vec2<i32>(textureDimensions(forest_map));
    let texel = vec2<i32>(floor((xz - parameters.xy) / parameters.z));
    if any(texel < vec2(0)) || any(texel >= size) {
        return vec4(0.0);
    }
    return textureLoad(forest_map, texel, 0);
}

// Share of the sky's light reaching `p` past the crowns, as surfaces under them see it
// (forest_sky_visibility in clouds/forest_shadow.wgsl).
fn sky_visibility(p: vec3<f32>) -> f32 {
    let parameters = clouds.forest_shadow;
    let strength = clouds.forest_sky.x;
    if strength <= 0.0 || parameters.z <= 0.0 {
        return 1.0;
    }
    let uv = (p.xz - parameters.xy) / (parameters.z * vec2<f32>(textureDimensions(forest_map)));
    if any(uv <= vec2(0.0)) || any(uv >= vec2(1.0)) {
        return 1.0;
    }
    let around = textureSampleLevel(forest_map, clamp_sampler, uv, 1.5);
    if around.a <= p.y {
        return 1.0;
    }
    return mix(1.0, mix(0.15, 1.0, around.b), strength);
}

// Ground height of the mist map, or `fallback` off it.
fn ground_at(xz: vec2<f32>, fallback: f32) -> f32 {
    if clouds.mist_map.w < 0.5 {
        return fallback;
    }
    let uv = (xz - clouds.mist_map.xy) / (clouds.mist_map.z * vec2<f32>(textureDimensions(mist_map)));
    if any(uv <= vec2(0.0)) || any(uv >= vec2(1.0)) {
        return fallback;
    }
    return textureSampleLevel(mist_map, clamp_sampler, uv, 0.0).b;
}

// Late-summer leaves: mostly green, some yellowing and dry ones.
fn leaf_albedo(pick: f32, sprig: bool) -> vec3<f32> {
    if sprig {
        return vec3(0.28, 0.16, 0.07);
    }
    if pick < 0.45 {
        return vec3(0.10, 0.15, 0.04);
    }
    if pick < 0.7 {
        return vec3(0.22, 0.24, 0.06);
    }
    if pick < 0.85 {
        return vec3(0.42, 0.31, 0.07);
    }
    return vec3(0.25, 0.13, 0.05);
}

@vertex
fn vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Particle {
    var out: Particle;
    out.position = vec4(0.0, 0.0, 0.0, 1.0);
    out.coords = vec2(0.0);
    out.view_depth = 0.0;
    out.radiance = vec3(0.0);
    out.alpha = 0.0;
    out.kind = MOTES;
    out.shape = 0.0;
    var kind = MOTES;
    if f32(instance) >= ambient.kinds[1].z { kind = FLUFF; }
    if f32(instance) >= ambient.kinds[2].z { kind = LEAVES; }
    let spec = ambient.kinds[kind];
    // The last instances of the present share fade, so amounts change smoothly.
    let rank = (f32(instance) - spec.z + 0.5) / spec.w;
    let amount = ambient.amounts[kind];
    var alpha = 1.0 - smoothstep(amount - 0.05, amount, rank);
    if alpha <= 0.0 { return out; }

    let a = random3(instance * 3u + 17u);
    let b = random3(instance * 5u + 503u);
    let c = random3(instance * 7u + 1201u);
    let size = vec3(spec.x, spec.y, spec.x);
    // Most of each box lies ahead of the camera.
    let forward = normalize(-view.world_from_view[2].xyz);
    let anchor = view.world_position + forward * spec.x * 0.3;
    let lattice = a * size + ambient.offsets[kind].xyz;
    var p = anchor + (fract((lattice - anchor) / size + 0.5) - 0.5) * size;
    let edge = abs(p - anchor) / (0.5 * size);
    alpha *= 1.0 - smoothstep(0.7, 1.0, max(edge.x, edge.z));

    let sun = clouds.sun.xyz;
    let corner = vertex % 6u;
    let cx = select(-1.0, 1.0, corner == 1u || corner == 2u || corner == 4u);
    let cy = select(-1.0, 1.0, corner == 2u || corner == 4u || corner == 5u);
    var world_corner: vec3<f32>;
    var radiance: vec3<f32>;
    if kind == LEAVES {
        // A leaf falls from a random height in the crown it left, carried by the wind.
        let cycles = round(mix(170.0, 300.0, b.x));
        let age = fract(ambient.time.x * cycles + b.y);
        let seconds = ambient.time.y / cycles;
        let spawn = p.xz - ambient.drift.xz * ambient.offsets[LEAVES].w * age * seconds;
        let crown = crown_at(spawn);
        // Denser crowns shed more.
        if crown.b < mix(0.05, 0.6, b.z) { return out; }
        let top = mix(crown.g, crown.r, c.x);
        let ground = ground_at(p.xz, top - 25.0);
        p.y = top - age * (max(top - ground, 1.0) + 0.5);
        alpha *= smoothstep(0.0, 0.05, age) * (1.0 - smoothstep(0.92, 1.0, age));
        // Falling leaves swing from side to side and tilt with the swing, and tumble slowly.
        let swing = wave(mix(1300.0, 2600.0, c.y), c.z);
        let side = vec2(cos(TAU * a.y), sin(TAU * a.y));
        p += vec3(side.x, 0.0, side.y) * swing * mix(0.15, 0.4, a.z);
        let axis = normalize(vec3(a.x * 2.0 - 1.0, 0.6, c.y * 2.0 - 1.0));
        let spin = turn(mix(500.0, 1500.0, b.z), a.x);
        var along = rotate(vec3(1.0, 0.0, 0.0), axis, spin);
        var across = rotate(vec3(0.0, 0.0, 1.0), axis, spin);
        across = rotate(across, along, swing * 0.7);
        let normal = cross(along, across);
        let sprig = c.z < 0.35;
        let half_length = mix(0.03, 0.05, b.y);
        let half_width = half_length * select(0.55, 0.22, sprig);
        world_corner = p + along * cx * half_length + across * cy * half_width;
        out.shape = select(0.0, 1.0, sprig);
        // Lit from the side the light reaches; sunlight seen through the leaf is dimmer.
        let to_camera = normalize(view.world_position - p);
        let facing = dot(normal, sun) * dot(normal, to_camera);
        let sunlight = clouds.sun_color.rgb * clouds.sun.w
            * shadow_map_visibility(p) * cloud_visibility(p) * abs(dot(normal, sun))
            * select(0.4, 1.0, facing >= 0.0);
        let sky = clouds.ambient.rgb * clouds.ambient.w * sky_visibility(p) * 0.8;
        let moon = clouds.moon_color.rgb * clouds.moon.w * 0.5;
        radiance = leaf_albedo(c.x, sprig) / 3.14159265 * (sunlight + sky + moon);
    } else {
        if kind == FLUFF {
            // Seeds drift in a layer over the ground, sinking slowly through it.
            let height = fract(lattice.y / spec.y);
            p.y = ground_at(p.xz, view.world_position.y - 3.0) + FLUFF_LIFT + height * spec.y;
            alpha *= smoothstep(0.0, 0.15, height) * (1.0 - smoothstep(0.85, 1.0, height));
        } else {
            alpha *= 1.0 - smoothstep(0.7, 1.0, edge.y);
        }
        // Slow wander of a few tens of centimetres; fluff also bobs up and down.
        let reach = select(0.15, 0.6, kind == FLUFF);
        p += vec3(
            wave(mix(90.0, 200.0, b.x), b.y),
            wave(mix(60.0, 150.0, b.z), c.x) * 0.6,
            wave(mix(90.0, 200.0, c.y), c.z),
        ) * reach;
        let ray = normalize(p - view.world_position);
        let mu = dot(sun, ray);
        let lit = shadow_map_visibility(p) * cloud_visibility(p) * step(0.0, sun.y);
        let sunlight = clouds.sun_color.rgb * clouds.sun.w * lit;
        let open = sky_visibility(p);
        let sky = clouds.ambient.rgb * clouds.ambient.w * open;
        // Motes read only as sunbeams in shade, so they keep to the woods; seeds blow over
        // open ground.
        alpha *= select(1.0 - smoothstep(0.7, 0.95, open), mix(0.25, 1.0, open), kind == FLUFF);
        let moon = clouds.moon_color.rgb * clouds.moon.w;
        let moon_mu = dot(clouds.moon.xyz, ray);
        // Motes scatter strongly forwards, so they show most against the light.
        let g = select(0.65, 0.45, kind == FLUFF);
        let albedo = select(0.5, 0.85, kind == FLUFF);
        let forward = 0.75 * phase(mu, g) + 0.25 / FOUR_PI;
        let moon_forward = 0.75 * phase(moon_mu, g) + 0.25 / FOUR_PI;
        radiance = albedo * (sunlight * forward + moon * moon_forward + sky / (2.0 * 3.14159265));
        if kind == MOTES {
            let flash = 0.5 + 0.5 * wave(mix(1800.0, 5400.0, c.y), b.x);
            radiance *= MOTE_GLINT * (0.4 + 1.6 * pow(flash, 6.0));
        }
        // Radius: motes of one to three millimetres, and seed heads of one to two and a half
        // centimetres with an open, sparse pappus.
        let radius = select(mix(0.0006, 0.0015, c.x), mix(0.005, 0.012, c.x), kind == FLUFF);
        alpha *= select(1.0, 0.6, kind == FLUFF);
        let centre = view.unjittered_clip_from_world * vec4(p, 1.0);
        if centre.w < 0.2 { return out; }
        // Drawn at least three or four pixels across, as a soft dot holding the light of the
        // particle's true size; the fragment's falloff integrates to 1 / DOT_NORMAL.
        let half_size = view.viewport.zw * 0.5;
        let focal = view.clip_from_view[1][1] * half_size.y;
        let true_radius = radius * focal / centre.w;
        let drawn = max(true_radius, select(1.5, 2.0, kind == FLUFF));
        alpha *= DOT_NORMAL * (true_radius / drawn) * (true_radius / drawn);
        // Particles brushing past the lens would fill the screen.
        alpha *= smoothstep(0.2, 0.6, centre.w) * exp(-clouds.fog.w * centre.w);
        if alpha < 1e-5 { return out; }
        let pixel = centre.xy / centre.w * half_size + vec2(cx, cy) * drawn;
        out.position = vec4(pixel / half_size * centre.w, 0.5 * centre.w, centre.w);
        out.coords = vec2(cx, cy);
        out.view_depth = centre.w;
        out.radiance = radiance * view.exposure;
        out.alpha = alpha;
        out.kind = kind;
        return out;
    }
    let clip = view.unjittered_clip_from_world * vec4(world_corner, 1.0);
    if clip.w < 0.2 { return out; }
    alpha *= smoothstep(0.3, 1.0, clip.w) * exp(-clouds.fog.w * clip.w);
    out.position = clip;
    out.coords = vec2(cx, cy);
    out.view_depth = clip.w;
    out.radiance = radiance * view.exposure;
    out.alpha = alpha;
    out.kind = kind;
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

@fragment
fn fragment(in: Particle) -> @location(0) vec4<f32> {
    var shape: f32;
    var softness: f32;
    if in.kind == LEAVES {
        // Pointed at both ends; sprigs are narrow and blunt.
        let x = in.coords.x;
        let broad = pow(max(1.0 - x * x, 0.0), 0.7) * (1.0 - 0.2 * x);
        let sprig = 0.5 + 0.5 * (1.0 - pow(abs(x), 8.0));
        let half_width = mix(broad, sprig, in.shape);
        let d = abs(in.coords.y) - half_width;
        shape = clamp(0.5 - d / max(fwidth(d), 1e-3), 0.0, 1.0);
        softness = 0.03;
    } else {
        let r2 = dot(in.coords, in.coords);
        shape = exp(-3.0 * r2) * (1.0 - smoothstep(0.7, 1.0, r2));
        softness = select(0.05, 0.1, in.kind == FLUFF);
    }
    let soft = clamp((scene_depth(in.position.xy) - in.view_depth) / softness, 0.0, 1.0);
    let alpha = min(in.alpha * shape, 1.0) * soft;
    if alpha < 1e-5 { discard; }
    return vec4(in.radiance * alpha, alpha);
}
