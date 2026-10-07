// Light shafts (crates/atmosphere/src/light_shafts.rs): each texel marches its view ray through
// the sun's shadow-map cascades over the range they cover.
//
// The sky composite lights ground haze and valley mist as if the sun reached all of it. Here
// the haze and mist in shadow give that sunlight back, and the humid air under crowns, which
// only this pass draws, scatters sunlight where it reaches and sky light everywhere. Each texel
// stores the light to add (exposed; negative where shadow darkens haze) and the transmittance of
// the air under crowns, and the distance it marched to, for the depth-aware blur and upsample.
#ifdef MARCH
#import bevy_render::view::View
#import bevy_pbr::mesh_view_types::{Lights, DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT}
#import "shaders/clouds/types.wgsl"::CloudParams

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> clouds: CloudParams;
#ifdef MULTISAMPLED
@group(0) @binding(2) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(2) var depth: texture_depth_2d;
#endif
@group(0) @binding(3) var<uniform> lights: Lights;
@group(0) @binding(4) var shadow_maps: texture_depth_2d_array;
@group(0) @binding(5) var shadow_sampler: sampler_comparison;
@group(0) @binding(6) var cloud_shadow: texture_2d<f32>;
@group(0) @binding(7) var forest_map: texture_2d<f32>;
@group(0) @binding(8) var mist_map: texture_2d<f32>;
@group(0) @binding(9) var noise: texture_3d<f32>;
@group(0) @binding(10) var repeat_sampler: sampler;
@group(0) @binding(11) var clamp_sampler: sampler;
@group(0) @binding(12) var light_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(13) var distance_out: texture_storage_2d<r32float, write>;

// Main-pass pixels per texel and steps per ray (`SCALE`, `STEPS` in light_shafts.rs).
const SCALE: f32 = f32(#{SHAFT_SCALE}u);
const STEPS: u32 = #{SHAFT_STEPS}u;
// Beyond the shadow maps nothing is shadowed and the composite's lighting is right.
const MAX_RANGE: f32 = 150.0;

// The composite's phase function for haze and mist (`scattering` in sky/composite.wgsl).
const FORWARD_SHARE: f32 = 0.4;
const FORWARD_G: f32 = 0.6;
const AUREOLE_SHARE: f32 = 0.05;
const AUREOLE_G: f32 = 0.92;
const INV_FOUR_PI: f32 = 0.07957747;
fn lobe(g: f32, cos_angle: f32) -> f32 {
    return (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * cos_angle, 1.5);
}
fn scattering(cos_angle: f32) -> f32 {
    return INV_FOUR_PI * ((1.0 - FORWARD_SHARE - AUREOLE_SHARE)
        + FORWARD_SHARE * lobe(FORWARD_G, cos_angle) + AUREOLE_SHARE * lobe(AUREOLE_G, cos_angle));
}

// Valley mist as the composite draws it (`mist_column`, `mist_wisps`, `mist_depth`), as a
// density at a point.
const OPEN_DEPTH: f32 = 0.35;
const OPEN_DENSITY: f32 = 0.2;
const WISP_LIFT: f32 = 0.35;
const MIST_NOISE_PERIOD: f32 = 2048.0;
fn mist_at(xz: vec2<f32>) -> vec4<f32> {
    let extent = clouds.mist_map.z * vec2<f32>(textureDimensions(mist_map));
    return textureSampleLevel(mist_map, clamp_sampler, (xz - clouds.mist_map.xy) / extent, 0.0);
}
// Highest the mist can reach in a column, wisps included.
fn mist_ceiling(xz: vec2<f32>) -> f32 {
    let m = mist_at(xz);
    return m.r + clouds.mist.y * mix(OPEN_DEPTH, 1.0, m.g) * m.a * (1.0 + WISP_LIFT);
}
fn mist_density(p: vec3<f32>) -> f32 {
    let m = mist_at(p.xz);
    let column_depth = clouds.mist.y * mix(OPEN_DEPTH, 1.0, m.g) * m.a;
    let q = (p.xz + clouds.mist_drift.xy) / MIST_NOISE_PERIOD;
    let n = textureSampleLevel(noise, repeat_sampler, vec3(q.x, p.y / 512.0, q.y), 0.0);
    let wisps = smoothstep(0.2, 0.8, n.r * 0.7 + n.b * 0.3);
    let top = m.r + column_depth + (wisps - 0.5) * 2.0 * WISP_LIFT * column_depth;
    let share = clamp((top - p.y) / max(column_depth * 0.6, 3.0), 0.0, 1.0);
    return clouds.mist.x * mix(OPEN_DENSITY, 1.0, m.g) * m.a * mix(0.1, 1.8, wisps) * share;
}

fn haze_density(y: f32) -> f32 {
    return clouds.low_haze.x * exp(-max(y - clouds.low_haze.y, 0.0) / clouds.low_haze.z);
}

// Air under crowns: the share of the sky the crowns around hold back (forest map levels 1-3),
// below their canopy top. Returns density and the sky light share reaching the point.
// Light under crowns has passed through and off leaves: dimmer and greener than the sky's.
const CANOPY_TINT: vec3<f32> = vec3(0.7, 1.0, 0.55);
const CANOPY_SKY: f32 = 0.3;
fn canopy_air(p: vec3<f32>) -> vec2<f32> {
    let parameters = clouds.forest_shadow;
    if clouds.shafts.x <= 0.0 || parameters.z <= 0.0 {
        return vec2(0.0, 1.0);
    }
    let uv = (p.xz - parameters.xy) / (parameters.z * vec2<f32>(textureDimensions(forest_map)));
    if any(uv <= vec2(0.0)) || any(uv >= vec2(1.0)) {
        return vec2(0.0, 1.0);
    }
    let around = textureSampleLevel(forest_map, clamp_sampler, uv, 2.0);
    if around.a <= p.y {
        return vec2(0.0, 1.0);
    }
    let cover = 1.0 - around.b;
    let sky = mix(1.0, mix(0.15, 1.0, around.b), clouds.forest_sky.x);
    return vec2(clouds.shafts.x * cover, sky);
}

struct Sun {
    index: i32,
    // Distance the cascades cover.
    range: f32,
}

const MAX_CASCADES: u32 = 4u;
// Shadow-map coordinates of a ray's two ends in each cascade. The cascades are orthographic, so
// coordinates along the ray are a straight blend of them.
struct ShadowRay {
    start: array<vec3<f32>, MAX_CASCADES>,
    end: array<vec3<f32>, MAX_CASCADES>,
}

fn shadow_ray(sun: Sun, origin: vec3<f32>, ray: vec3<f32>, reach: f32) -> ShadowRay {
    var out: ShadowRay;
    let light = &lights.directional_lights[sun.index];
    for (var c = 0u; c < min((*light).num_cascades, MAX_CASCADES); c += 1u) {
        let cascade = &(*light).cascades[c];
        // Particles of air cast no shadow; the lift only keeps clear of depth precision.
        let lift = (*light).direction_to_light * ((*light).shadow_depth_bias + (*cascade).texel_size);
        let a = (*cascade).clip_from_world * vec4(origin + lift, 1.0);
        let b = (*cascade).clip_from_world * vec4(origin + ray * reach + lift, 1.0);
        out.start[c] = a.xyz / a.w;
        out.end[c] = b.xyz / b.w;
    }
    return out;
}

fn shadowed_sun() -> Sun {
    for (var i = 0u; i < lights.n_directional_lights; i += 1u) {
        let light = &lights.directional_lights[i];
        if ((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u
            && (*light).num_cascades > 0u
            && dot((*light).direction_to_light, clouds.sun.xyz) > 0.995 {
            return Sun(i32(i), (*light).cascades[(*light).num_cascades - 1u].far_bound);
        }
    }
    return Sun(-1, 0.0);
}

// Sunlight past everything in the shadow maps, a share `along` of the way along the ray and
// `distance` from the camera along the view.
fn sun_visibility(sun: Sun, shadow: ShadowRay, along: f32, distance: f32) -> f32 {
    let light = &lights.directional_lights[sun.index];
    for (var c = 0u; c < min((*light).num_cascades, MAX_CASCADES); c += 1u) {
        if distance >= (*light).cascades[c].far_bound {
            continue;
        }
        let ndc = mix(shadow.start[c], shadow.end[c], along);
        if any(abs(ndc.xy) > vec2(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
            return 1.0;
        }
        let uv = ndc.xy * vec2(0.5, -0.5) + 0.5;
        return textureSampleCompareLevel(
            shadow_maps, shadow_sampler, uv, i32((*light).depth_texture_base_index + c), ndc.z);
    }
    return 1.0;
}

// Below this extinction per metre, shadow cannot visibly darken the air (thin ground haze).
const SHADOWED_AIR: f32 = 2e-4;

// Sunlight past the cloud layer's shadow at `p` (`cloud_visibility` in clouds/surface.wgsl).
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

// Interleaved gradient noise (Jimenez 2014): offsets each texel's steps so banding becomes
// fine noise that the blur removes.
fn gradient_noise(texel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(texel, vec2(0.06711056, 0.00583715))));
}

@compute @workgroup_size(8, 8)
fn march(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(light_out);
    if any(id.xy >= size) {
        return;
    }
    let texel = vec2<i32>(id.xy);
    let viewport = view.main_pass_viewport;
    let pixel = min((vec2<f32>(id.xy) + 0.5) * SCALE, viewport.zw - 0.5);
    let uv = pixel / viewport.zw;
    let ndc = uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
    let near = view.view_from_clip * vec4(ndc, 1.0, 1.0);
    let ray = normalize((view.world_from_view * vec4(near.xyz / near.w, 0.0)).xyz);
    let z = textureLoad(depth, vec2<i32>(pixel + viewport.xy), 0);
    var distance = 1.0e9;
    if z > 0.0 {
        let world = view.world_from_clip * vec4(ndc, z, 1.0);
        distance = length(world.xyz / world.w - view.world_position);
    }
    textureStore(distance_out, texel, vec4(min(distance, 1.0e6)));

    let sun = shadowed_sun();
    let reach = min(distance, min(sun.range, MAX_RANGE));
    let origin = view.world_position;
    let lit = sun.index >= 0 && clouds.sun.y > 0.0;
    let sunlight = select(vec3(0.0), clouds.air_sun.rgb * scattering(dot(ray, clouds.sun.xyz))
        * cloud_visibility(origin), lit);
    var mist_top = -1.0e9;
    if clouds.mist.x > 0.0 && clouds.mist_map.w > 0.5 {
        mist_top = max(mist_ceiling(origin.xz), mist_ceiling(origin.xz + ray.xz * reach));
    }
    var shadow: ShadowRay;
    if lit {
        shadow = shadow_ray(sun, origin, ray, reach);
    }
    let forward = dot(ray, normalize(-view.world_from_view[2].xyz));
    let luminance = vec3(0.2126, 0.7152, 0.0722);
    let skylight = dot(clouds.air_light.rgb, luminance) * CANOPY_SKY * CANOPY_TINT
        / dot(CANOPY_TINT, luminance);
    let jitter = gradient_noise(vec2<f32>(id.xy));
    var light = vec3(0.0);
    var seen = 1.0;
    var canopy_seen = 1.0;
    for (var i = 0u; i < STEPS; i += 1u) {
        // Steps widen with distance: t = length * u^2.
        let u = (f32(i) + jitter) / f32(STEPS);
        let t = reach * u * u;
        let dt = reach * f32(2u * i + 1u) / f32(STEPS * STEPS);
        let p = origin + ray * t;
        let haze = haze_density(p.y);
        var droplets = 0.0;
        if p.y < mist_top {
            droplets = mist_density(p);
        }
        let canopy = canopy_air(p);
        var visible = 1.0;
        if lit && droplets + canopy.x > SHADOWED_AIR {
            visible = sun_visibility(sun, shadow, u * u, t * forward);
        }
        // Haze and mist in shadow give back the composite's sunlight; the air under crowns adds
        // the sunlight that reaches it and its share of sky light.
        let lost = (clouds.haze.rgb * haze + vec3(droplets)) * (1.0 - visible);
        let added = canopy.x * (visible * sunlight + skylight * canopy.y);
        light += seen * dt * (added - lost * sunlight);
        let extinction = haze + droplets + canopy.x + clouds.fog.w;
        seen *= exp(-extinction * dt);
        canopy_seen *= exp(-canopy.x * dt);
    }
    textureStore(light_out, texel, vec4(light * view.exposure, canopy_seen));
}
#else

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var distances: texture_2d<f32>;
@group(0) @binding(2) var destination: texture_storage_2d<rgba16float, write>;

// A 9-tap Gaussian (sigma 2 texels) that skips texels at a different distance, so beams do not
// bleed across silhouettes.
const RADIUS: i32 = 4;
fn blur(id: vec2<u32>, step: vec2<i32>) {
    let size = vec2<i32>(textureDimensions(destination));
    let texel = vec2<i32>(id);
    if any(texel >= size) {
        return;
    }
    let centre = textureLoad(distances, texel, 0).r;
    var sum = vec4(0.0);
    var weight = 0.0;
    for (var i = -RADIUS; i <= RADIUS; i += 1) {
        let at = clamp(texel + step * i, vec2(0), size - 1);
        let d = textureLoad(distances, at, 0).r;
        let w = exp(-f32(i * i) / 8.0) * exp(-abs(d - centre) / (0.05 * min(centre, d) + 0.3));
        sum += textureLoad(source, at, 0) * w;
        weight += w;
    }
    textureStore(destination, texel, sum / weight);
}

@compute @workgroup_size(8, 8)
fn blur_x(@builtin(global_invocation_id) id: vec3<u32>) {
    blur(id.xy, vec2(1, 0));
}

@compute @workgroup_size(8, 8)
fn blur_y(@builtin(global_invocation_id) id: vec3<u32>) {
    blur(id.xy, vec2(0, 1));
}
#endif
