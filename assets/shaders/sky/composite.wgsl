// Everything between the camera and the scene, in one pass over the resolved HDR image: the
// physical sky and aerial perspective from Bevy's atmosphere tables, the cloud layer, ground
// haze, valley mist and weather fog. Each depth sample is classified, so a pixel on a silhouette gets the sky's light for its
// sky samples and the haze of its own distance for the geometry samples.
//
// The pass blends into the image: light is added, and the resolved scene is multiplied by the
// transmittance averaged over geometry samples. Sky samples hold the black clear colour, so they
// take no part in that average. A pixel with no geometry at all keeps whatever was drawn at
// infinity (editor gizmos, for example) behind the sky's own transmittance.
//
// Sky and aerial-perspective lookups follow Bevy 0.19.1 `render_sky.wgsl` (MIT; see
// third_party/BEVY-MIT.txt) and use its atmosphere shader functions directly.
enable dual_source_blending;

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import "shaders/clouds/types.wgsl"::{CloudParams, sky_panorama_uv}
#ifdef ATMOSPHERE
#import bevy_pbr::atmosphere::{
    bindings::{lights, view},
    bruneton_functions::ray_intersects_ground,
    functions::{
        direction_world_to_atmosphere, get_local_r, get_view_position, sample_aerial_view_lut,
        sample_density_lut, sample_sky_view_lut, sample_transmittance_lut,
    },
}
#else
#import bevy_render::view::View
@group(0) @binding(3) var<uniform> view: View;
#endif

#ifdef MULTISAMPLED
@group(1) @binding(0) var depth: texture_depth_multisampled_2d;
#else
@group(1) @binding(0) var depth: texture_depth_2d;
#endif
@group(1) @binding(1) var<storage, read> clouds: CloudParams;
// Cloud images: the newest and previous complete cache refreshes, and x = cross-fade progress;
// y = light shafts drawn, z = main-pass pixels per shaft texel.
@group(1) @binding(2) var cloud_newer: texture_2d<f32>;
@group(1) @binding(3) var cloud_older: texture_2d<f32>;
@group(1) @binding(4) var cloud_sampler: sampler;
@group(1) @binding(5) var<uniform> cloud_blend: vec4<f32>;
// Where mist pools (`atmosphere::valley_mist`): floor height, valley share, ground height, land
// share.
@group(1) @binding(6) var mist_map: texture_2d<f32>;
@group(1) @binding(7) var mist_sampler: sampler;
@group(1) @binding(8) var noise: texture_3d<f32>;
@group(1) @binding(9) var noise_sampler: sampler;
// Light shafts (sky/light_shafts.wgsl): light to add and the transmittance of the air under
// crowns, and the distance each texel marched to.
@group(1) @binding(10) var shaft_light: texture_2d<f32>;
@group(1) @binding(11) var shaft_distance: texture_2d<f32>;
// The sun as the camera sees it (sky/sun_occlusion.wgsl): x share seen, y share the cloud layer
// lets through, zw its main-pass uv.
@group(1) @binding(12) var<storage, read> sun_state: vec4<f32>;
// Share of the glare veil reaching each texel (sky/sun_rays.wgsl), at the light shafts' scale;
// drawn when cloud_blend.w is 1.
@group(1) @binding(13) var sun_rays: texture_2d<f32>;

struct Output {
#ifdef DUAL_SOURCE_BLENDING
    @location(0) @blend_src(0) light: vec4<f32>,
    @location(0) @blend_src(1) transmittance: vec4<f32>,
#else
    @location(0) light: vec4<f32>,
#endif
}

// Light added along part of a view ray, and how much of whatever lies beyond it shows through.
struct Path {
    light: vec3<f32>,
    transmittance: vec3<f32>,
}

fn in_front(light: vec3<f32>, transmittance: f32, behind: Path) -> Path {
    return Path(light + behind.light * transmittance, behind.transmittance * transmittance);
}

// Ground haze, valley mist and weather fog over the first `distance` metres of a ray, as one
// medium in front of everything beyond: each adds its optical depth, and the light scattered
// towards the eye is their mix by optical depth.
struct Fog {
    // Exposed light scattered towards the eye where the fog is opaque, and optical depth.
    light: vec3<f32>,
    depth: f32,
}

fn fog_along(ray: vec3<f32>, distance: f32) -> Fog {
    let weather = clouds.fog.w * distance;
    let haze = haze_depth(ray, distance);
    let mist = mist_depth(ray, distance);
    let depth = weather + haze + mist;
    if depth <= 0.0 {
        return Fog(vec3(0.0), 0.0);
    }
    let air = clouds.air_light.rgb + clouds.air_sun.rgb * scattering(dot(ray, clouds.sun.xyz));
    let light = (clouds.fog.rgb * weather + air * (clouds.haze.rgb * haze + vec3(mist))) / depth;
    return Fog(light * view.exposure, depth);
}

fn behind_fog(fog: Fog, behind: Path) -> Path {
    let transmittance = exp(-fog.depth);
    return in_front(fog.light * (1.0 - transmittance), transmittance, behind);
}

fn fogged(ray: vec3<f32>, distance: f32, behind: Path) -> Path {
    return behind_fog(fog_along(ray, distance), behind);
}

// Phase function of haze and mist droplets: an even share, a forward lobe and the narrow
// aureole of large droplets, so both glow towards the sun and brightest right around it.
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

// Ground haze is integrated this far at most: near-horizontal rays to the sky.
const HAZE_RANGE: f32 = 60000.0;

// Optical depth of the ground haze, exponential in height above its base (`haze_depth` in
// `atmosphere::valley_mist`, which the tests check).
fn haze_depth(ray: vec3<f32>, distance: f32) -> f32 {
    let density = clouds.low_haze.x;
    if density <= 0.0 {
        return 0.0;
    }
    let length = min(distance, HAZE_RANGE);
    let height = clouds.low_haze.z;
    let above = max(view.world_position.y - clouds.low_haze.y, 0.0);
    let start = density * exp(-above / height);
    // Climb in thinning heights, stopping where the ray would sink below the base.
    let climb = max(ray.y * length / height, -above / height);
    if abs(climb) < 1e-4 {
        return start * length * (1.0 - 0.5 * climb);
    }
    return start * length * (1.0 - exp(-climb)) / climb;
}

// Mist is integrated over this many stretches of the part of a ray low enough to hold it, and
// this far at most.
const MIST_STEPS: u32 = 4u;
const MIST_RANGE: f32 = 20000.0;
// Open ground holds these shares of a valley's mist depth and density.
const OPEN_DEPTH: f32 = 0.35;
const OPEN_DENSITY: f32 = 0.2;
// Noise lifts and lowers the mist top by up to this share of its depth.
const WISP_LIFT: f32 = 0.35;
// Horizontal size of the noise tile, metres; `MIST_NOISE_PERIOD` in `atmosphere::clouds`.
const MIST_NOISE_PERIOD: f32 = 2048.0;

struct MistColumn {
    top: f32,
    // Depth of mist above the floor, and the share of full density it holds.
    depth: f32,
    density: f32,
}

fn mist_column(xz: vec2<f32>) -> MistColumn {
    let extent = clouds.mist_map.z * vec2<f32>(textureDimensions(mist_map));
    let m = textureSampleLevel(mist_map, mist_sampler, (xz - clouds.mist_map.xy) / extent, 0.0);
    let depth = clouds.mist.y * mix(OPEN_DEPTH, 1.0, m.g) * m.a;
    return MistColumn(m.r + depth, depth, mix(OPEN_DENSITY, 1.0, m.g) * m.a);
}

// Slowly drifting, world-anchored variation of mist density and height, 0..1.
fn mist_wisps(p: vec3<f32>) -> f32 {
    let q = (p.xz + clouds.mist_drift.xy) / MIST_NOISE_PERIOD;
    let n = textureSampleLevel(noise, noise_sampler, vec3(q.x, p.y / 512.0, q.y), 0.0);
    return smoothstep(0.2, 0.8, n.r * 0.7 + n.b * 0.3);
}

// Mean share of full mist density along a straight stretch through the mist's soft top: `ua`
// and `ub` are its ends in fade widths below the top (`mist_share` in `atmosphere::valley_mist`).
fn mist_share(ua: f32, ub: f32) -> f32 {
    if abs(ua - ub) < 1e-4 {
        return clamp(0.5 * (ua + ub), 0.0, 1.0);
    }
    return (mist_integral(ua) - mist_integral(ub)) / (ua - ub);
}

fn mist_integral(u: f32) -> f32 {
    if u <= 0.0 {
        return 0.0;
    }
    if u < 1.0 {
        return 0.5 * u * u;
    }
    return u - 0.5;
}

// Optical depth of valley mist over the first `distance` metres of the ray. Mist fills the
// ground up to a level top that varies slowly across the map, so a straight ray only holds mist
// where it runs below the higher of the tops at its two ends: that part is integrated exactly in
// a few stretches, each with the top and wisps at its middle.
fn mist_depth(ray: vec3<f32>, distance: f32) -> f32 {
    if clouds.mist.x <= 0.0 || clouds.mist_map.w < 0.5 {
        return 0.0;
    }
    let origin = view.world_position;
    let length = min(distance, MIST_RANGE);
    let ceiling = max(mist_column(origin.xz).top, mist_column(origin.xz + ray.xz * length).top)
        + clouds.mist.y * WISP_LIFT;
    var t0 = 0.0;
    var t1 = length;
    if ray.y > 1e-5 {
        t1 = min(t1, (ceiling - origin.y) / ray.y);
    } else if ray.y < -1e-5 {
        t0 = max(t0, (origin.y - ceiling) / -ray.y);
    } else if origin.y >= ceiling {
        return 0.0;
    }
    if t1 <= t0 {
        return 0.0;
    }
    let stretch = (t1 - t0) / f32(MIST_STEPS);
    var depth = 0.0;
    for (var i = 0u; i < MIST_STEPS; i += 1u) {
        let start = t0 + stretch * f32(i);
        let middle = origin + ray * (start + 0.5 * stretch);
        let column = mist_column(middle.xz);
        let wisps = mist_wisps(middle);
        let top = column.top + (wisps - 0.5) * 2.0 * WISP_LIFT * column.depth;
        let fade = max(column.depth * 0.6, 3.0);
        let ua = (top - (origin.y + ray.y * start)) / fade;
        let ub = (top - (origin.y + ray.y * (start + stretch))) / fade;
        // Wisps thin the mist to gaps and thicken it into banks.
        depth += column.density * mix(0.1, 1.8, wisps) * mist_share(ua, ub) * stretch;
    }
    return depth * clouds.mist.x;
}

struct Cloud {
    light: vec3<f32>,
    transmittance: f32,
}

// The cloud layer where the ray crosses its base, `start` metres away.
fn cloud_layer(ray: vec3<f32>, start: f32, screen_uv: vec2<f32>) -> Cloud {
#ifdef CACHED_CLOUDS
    let uv = sky_panorama_uv(ray);
    let c = mix(textureSampleLevel(cloud_older, cloud_sampler, uv, 0.0),
        textureSampleLevel(cloud_newer, cloud_sampler, uv, 0.0), cloud_blend.x);
#else
    let c = textureSampleLevel(cloud_newer, cloud_sampler, screen_uv, 0.0);
#endif
    let haze = exp(-start * 3.912 / max(clouds.haze.w, 50.0));
    // Haze in front of an opaque cloud must not reintroduce the sun/moon disk behind it.
    let air = clouds.haze.rgb * (clouds.ambient.rgb * clouds.ambient.w * 0.3
        + clouds.near_sun.rgb * 0.025
        + clouds.moon_color.rgb * clouds.moon.w * 0.025) * view.exposure;
    let color = mix(air * (1.0 - c.a), c.rgb * (1024.0 * view.exposure), haze);
    // Fade the finite ground-view tracing range into the horizon rather than exposing a
    // straight edge at the end of the cloud layer.
    let coverage = 1.0 - smoothstep(20000.0, 40000.0, start);
    return Cloud(color * coverage, 1.0 - coverage * (1.0 - c.a));
}

// Air between the camera and a surface `distance` metres along the ray. The atmosphere entity
// has unit scale, so world distance is also atmosphere distance.
fn aerial_perspective(ray: vec3<f32>, uv: vec2<f32>, distance: f32) -> Path {
#ifdef ATMOSPHERE
    let position = get_view_position();
    let r = length(position);
    let mu = dot(ray, normalize(position));
    return Path(sample_aerial_view_lut(uv, distance) * view.exposure,
        segment_transmittance(r, mu, distance));
#else
    return Path(vec3(0.0), vec3(1.0));
#endif
}

#ifdef ATMOSPHERE
// The sun and moon discs with Bevy's antialiased edge and total light. The sun darkens towards
// its limb, where the photosphere is seen at a grazing angle.
const LIMB_DARKENING: f32 = 0.6;
fn is_sun(direction: vec3<f32>) -> bool {
    return dot(direction, clouds.sun.xyz) > 0.995;
}
fn discs(ray: vec3<f32>) -> vec3<f32> {
    let position = get_view_position();
    let below = ray_intersects_ground(length(position), dot(ray, normalize(position)));
    var radiance = vec3(0.0);
    for (var i = 0u; i < lights.n_directional_lights; i += 1u) {
        let light = &lights.directional_lights[i];
        let size = (*light).sun_disk_angular_size;
        let intensity = (*light).sun_disk_intensity;
        let angle = acos(clamp(dot((*light).direction_to_light, ray), -1.0, 1.0));
        let w = max(0.5 * fwidth(angle), 1e-6);
        if size <= 0.0 || intensity <= 0.0 {
            continue;
        }
        let radius = 0.5 * size;
        let edge = 1.0 - smoothstep(radius - w, radius + w, angle);
        let x = min(angle / radius, 1.0);
        let limb = select(1.0,
            (1.0 - LIMB_DARKENING * (1.0 - sqrt(1.0 - x * x))) / (1.0 - LIMB_DARKENING / 3.0),
            is_sun((*light).direction_to_light));
        radiance += (*light).color.rgb / (size * size * 0.25 * 3.14159265) * intensity * edge * limb;
    }
    return select(radiance, vec3(0.0), below);
}

// Exposed disc light is kept within half floats (65504); the glare carries the sun's light
// beyond its disc.
const MAX_DISC: f32 = 30000.0;

// Glare of the eye and lens around the sun: a core about a degree wide and a veil over tens of
// degrees, each holding a share of the sun's light. The core is scaled by the share of the sun
// seen, the veil by the share of it reaching the pixel past what lies between, so beams fan out
// from silhouettes in front of the sun.
// Kernels are normalised over the image plane: core (1 + u)^-2 / (pi w^2), veil
// (1 + u)^-1.25 / (4 pi w^2) and air glow (1 + u)^-1.5 / (2 pi w^2), u = (angle / w)^2; the
// veil's slow fall leaves no visible rim.
const GLARE_CORE_SHARE: f32 = 0.02;
const GLARE_CORE_WIDTH: f32 = 0.021;
const GLARE_VEIL_SHARE: f32 = 0.012;
const GLARE_VEIL_WIDTH: f32 = 0.12;
// Sunlit air in front of the scene scattering towards the eye around the sun, shaded like the
// veil by what lies between, so sunset beams fan out from silhouettes. Drawn with sun rays only,
// fading out as the sun leaves the image: off screen nothing shows what shades it.
const AIR_GLOW_SHARE: f32 = 0.08;
const AIR_GLOW_WIDTH: f32 = 0.15;
// Depth of air over which the glow builds up in front of a surface, metres, and how much faster
// it builds up in the air under crowns (as a power of that air's transmittance).
const AIR_GLOW_DEPTH: f32 = 30.0;
const AIR_GLOW_CANOPY: f32 = 4.0;

// 1 with the sun in the image, falling to 0 a fifth of the image outside it.
fn sun_in_view() -> f32 {
    let clip = view.clip_from_world * vec4(clouds.sun.xyz, 0.0);
    if clip.w <= 0.0 {
        return 0.0;
    }
    let uv = clip.xy / clip.w * vec2(0.5, -0.5) + 0.5;
    let outside = max(max(-uv.x, uv.x - 1.0), max(-uv.y, uv.y - 1.0));
    return 1.0 - smoothstep(0.0, 0.2, outside);
}
fn sun_glare(ray: vec3<f32>, pixel: vec2<f32>, air: f32) -> vec3<f32> {
    let visible = sun_state.x;
    let drawn = cloud_blend.w > 0.5;
    let rays = select(visible, sun_rays_at(pixel), drawn);
    let glow = select(0.0, AIR_GLOW_SHARE * sun_in_view() * air, drawn);
    if visible <= 0.0 && rays <= 0.0 {
        return vec3(0.0);
    }
    let position = get_view_position();
    for (var i = 0u; i < lights.n_directional_lights; i += 1u) {
        let light = &lights.directional_lights[i];
        if !is_sun((*light).direction_to_light) || (*light).sun_disk_intensity <= 0.0 {
            continue;
        }
        let transmittance = sample_transmittance_lut(length(position),
            dot((*light).direction_to_light, normalize(position)));
        let irradiance = (*light).color.rgb * (*light).sun_disk_intensity * transmittance;
        // Squared angle, close to 2 (1 - cos) well past the veil.
        let angle2 = 2.0 * (1.0 - dot(ray, (*light).direction_to_light));
        let core = 1.0 + angle2 / (GLARE_CORE_WIDTH * GLARE_CORE_WIDTH);
        let veil = 1.0 + angle2 / (GLARE_VEIL_WIDTH * GLARE_VEIL_WIDTH);
        let air = 1.0 + angle2 / (AIR_GLOW_WIDTH * AIR_GLOW_WIDTH);
        let kernel = GLARE_CORE_SHARE / (3.14159265 * GLARE_CORE_WIDTH * GLARE_CORE_WIDTH * core * core)
            * visible
            + (GLARE_VEIL_SHARE / (12.566371 * GLARE_VEIL_WIDTH * GLARE_VEIL_WIDTH * veil * sqrt(sqrt(veil)))
            + glow / (6.2831853 * AIR_GLOW_WIDTH * AIR_GLOW_WIDTH * air * sqrt(air)))
            * rays;
        return irradiance * kernel * view.exposure;
    }
    return vec3(0.0);
}

// Transmittance of the camera-to-surface segment alone. Bevy's
// `sample_transmittance_lut_segment` divides two whole-atmosphere transmittances. For
// near-horizontal rays towards the ground both paths cross hundreds of kilometres of low haze,
// underflow its half-float table, and the ratio turns into coloured horizontal streaks.
// Surfaces lie within the view distance, so integrate the extinction along the segment itself:
// Simpson's rule over the full-precision medium density table, whose haze falls off over about
// a kilometre of altitude.
fn segment_transmittance(r: f32, mu: f32, distance: f32) -> vec3<f32> {
    let depth = extinction(r)
        + 4.0 * extinction(get_local_r(r, mu, 0.5 * distance))
        + extinction(get_local_r(r, mu, distance));
    return exp(-depth * (distance / 6.0));
}

fn extinction(r: f32) -> vec3<f32> {
    return sample_density_lut(r, 0.0) + sample_density_lut(r, 1.0);
}
#endif

// Sun rays at a pixel, bilinear between the four nearest texels.
fn sun_rays_at(position: vec2<f32>) -> f32 {
    let size = vec2<i32>(textureDimensions(sun_rays));
    let q = (position - view.main_pass_viewport.xy) / cloud_blend.z - 0.5;
    let base = vec2<i32>(floor(q));
    let f = q - floor(q);
    let a = textureLoad(sun_rays, clamp(base, vec2(0), size - 1), 0).r;
    let b = textureLoad(sun_rays, clamp(base + vec2(1, 0), vec2(0), size - 1), 0).r;
    let c = textureLoad(sun_rays, clamp(base + vec2(0, 1), vec2(0), size - 1), 0).r;
    let d = textureLoad(sun_rays, clamp(base + vec2(1, 1), vec2(0), size - 1), 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Light shafts at a pixel `distance` metres deep: the four nearest texels, bilinear but skipping
// those at a different distance, so beams stop at silhouettes. Sky pixels use 1e6.
fn light_shafts(position: vec2<f32>, distance: f32) -> vec4<f32> {
    if cloud_blend.y < 0.5 {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    let size = vec2<i32>(textureDimensions(shaft_light));
    let q = (position - view.main_pass_viewport.xy) / cloud_blend.z - 0.5;
    let base = vec2<i32>(floor(q));
    let f = q - floor(q);
    var sum = vec4(0.0);
    var weight = 0.0;
    var nearest = vec4(0.0, 0.0, 0.0, 1.0);
    var nearest_gap = 1.0e30;
    for (var i = 0; i < 4; i += 1) {
        let offset = vec2(i & 1, i >> 1u);
        let at = clamp(base + offset, vec2(0), size - 1);
        let d = textureLoad(shaft_distance, at, 0).r;
        let s = textureLoad(shaft_light, at, 0);
        let gap = abs(d - distance);
        let w = select(1.0 - f.x, f.x, offset.x == 1) * select(1.0 - f.y, f.y, offset.y == 1)
            * exp(-gap / (0.05 * min(d, distance) + 0.3));
        sum += s * w;
        weight += w;
        if gap < nearest_gap {
            nearest_gap = gap;
            nearest = s;
        }
    }
    return select(nearest, sum / weight, weight > 1e-4);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> Output {
    let uv = (in.position.xy - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let ndc = uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
    // Direction from the near plane in view space, as Bevy's sky does: exact far from the origin.
    let near = view.view_from_clip * vec4(ndc, 1.0, 1.0);
    let ray = normalize((view.world_from_view * vec4(near.xyz / near.w, 0.0)).xyz);
#ifdef ATMOSPHERE
    // The sun and moon disks use derivatives, so evaluate them before any per-pixel branch.
    let disks = discs(ray);
#endif
    if any(uv < vec2(0.0)) || any(uv > vec2(1.0)) {
        discard;
    }
    let fog = clouds.fog.w > 0.0 || clouds.low_haze.x > 0.0 || clouds.mist.x > 0.0;
    // Clouds are traced only above this elevation; `start` is the distance to their base.
    let above = ray.y > 0.01;
    let start = select(1.0e9, max(0.0, (clouds.layer.x - view.world_position.y) / ray.y), above);
#ifndef ATMOSPHERE
    // Without the atmosphere, ground below the horizon only changes in fog.
    if !above && !fog {
        discard;
    }
#endif

    let pixel = vec2<i32>(in.position.xy);
#ifdef MULTISAMPLED
    let samples = min(textureNumSamples(depth), 8u);
#else
    let samples = 1u;
#endif
    // Negative distance marks a sky sample.
    var distances: array<f32, 8>;
    var sky_samples = 0u;
    var nearest = 1.0e30;
    var farthest = 0.0;
    var total = 0.0;
    for (var i = 0u; i < samples; i += 1u) {
        let z = textureLoad(depth, pixel, i32(i));
        if z == 0.0 {
            distances[i] = -1.0;
            sky_samples += 1u;
            continue;
        }
        let world = view.world_from_clip * vec4(ndc, z, 1.0);
        let distance = length(world.xyz / world.w - view.world_position);
        distances[i] = distance;
        nearest = min(nearest, distance);
        farthest = max(farthest, distance);
        total += distance;
    }
    let geometry_samples = samples - sky_samples;

    // One cloud lookup serves every sample that can see the cloud base.
    var cloud = Cloud(vec3(0.0), 1.0);
#ifdef CLOUDS
    if above && (sky_samples > 0u || farthest >= start) {
        cloud = cloud_layer(ray, start, in.uv);
    }
#endif

    var light = vec3(0.0);
    var sky_transmittance = vec3(1.0);
    if sky_samples > 0u {
#ifdef ATMOSPHERE
        let position = get_view_position();
        let r = length(position);
        let transmittance = sample_transmittance_lut(r, dot(ray, normalize(position)));
        let sky = sample_sky_view_lut(r, direction_world_to_atmosphere(ray));
        var path = Path(sky * view.exposure
            + min(disks * transmittance * view.exposure, vec3(MAX_DISC)), transmittance);
#else
        var path = Path(vec3(0.0), vec3(1.0));
#endif
        path = fogged(ray, start, in_front(cloud.light, cloud.transmittance, path));
        light += path.light * f32(sky_samples);
        sky_transmittance = path.transmittance;
    }

    var transmittance = vec3(0.0);
    if geometry_samples > 0u {
        if farthest - nearest <= 0.02 * farthest {
            // Usual case: all geometry samples lie at one distance, so shade it once.
            let distance = total / f32(geometry_samples);
            var path = aerial_perspective(ray, uv, distance);
            if distance >= start {
                path = in_front(cloud.light, cloud.transmittance, path);
            }
            path = fogged(ray, min(distance, start), path);
            light += path.light * f32(geometry_samples);
            transmittance = path.transmittance * f32(geometry_samples);
        } else {
            // A silhouette: fog changes smoothly between the nearest and farthest samples, so it
            // is integrated at those two and interpolated for the rest.
            let near = min(nearest, start);
            let far = min(farthest, start);
            let near_fog = fog_along(ray, near);
            let far_fog = fog_along(ray, far);
            for (var i = 0u; i < samples; i += 1u) {
                let distance = distances[i];
                if distance < 0.0 {
                    continue;
                }
                var path = aerial_perspective(ray, uv, distance);
                if distance >= start {
                    path = in_front(cloud.light, cloud.transmittance, path);
                }
                let t = clamp((min(distance, start) - near) / max(far - near, 1e-3), 0.0, 1.0);
                let fog = Fog(mix(near_fog.light, far_fog.light, t),
                    mix(near_fog.depth, far_fog.depth, t));
                path = behind_fog(fog, path);
                light += path.light;
                transmittance += path.transmittance;
            }
        }
        transmittance /= f32(geometry_samples);
    } else {
        transmittance = sky_transmittance;
    }
    light /= f32(samples);
    // Light shafts lie in front of everything else the pass adds.
    let shafts = light_shafts(in.position.xy,
        select(1.0e6, total / f32(max(geometry_samples, 1u)), geometry_samples > 0u));
    light = light * shafts.a + shafts.rgb;
    transmittance *= shafts.a;
#ifdef ATMOSPHERE
    // Share of the air glow in front of this pixel: little in front of near surfaces, unless
    // the humid air under crowns lies between.
    let near_air = 1.0 - exp(-total / f32(max(geometry_samples, 1u)) / AIR_GLOW_DEPTH)
        * pow(shafts.a, AIR_GLOW_CANOPY);
    let air = (f32(sky_samples) + f32(geometry_samples) * near_air) / f32(samples);
    light += sun_glare(ray, in.position.xy, air);
#endif

#ifdef DUAL_SOURCE_BLENDING
    return Output(vec4(light, 0.0), vec4(transmittance, 1.0));
#else
    return Output(vec4(light, dot(transmittance, vec3(1.0 / 3.0))));
#endif
}
