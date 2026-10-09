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
#import "shaders/water/waves.wgsl"::sea_waves
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
@group(1) @binding(14) var cloud_shadow: texture_2d<f32>;
@group(1) @binding(15) var cloud_shadow_sampler: sampler;
// How waves come ashore (`atmosphere::shore`): depth below the sea level, seconds a crest takes
// to get here from deep water and that time's gradient along X and Z. Read with `mist_sampler`.
@group(1) @binding(16) var shore_map: texture_2d<f32>;

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

// Exposed light each medium scatters towards the eye along a ray where it is opaque: it depends
// on the ray's direction alone, so a pixel works it out once for all its samples.
struct FogLight {
    haze: vec3<f32>,
    mist: vec3<f32>,
}

fn fog_light(ray: vec3<f32>) -> FogLight {
    let air = clouds.air_light.rgb + clouds.air_sun.rgb * scattering(dot(ray, clouds.sun.xyz));
    return FogLight(haze_light(ray, air) * view.exposure, air * view.exposure);
}

fn fog_along(ray: vec3<f32>, distance: f32, lights: FogLight) -> Fog {
    let weather = clouds.fog.w * distance;
    let haze = haze_depth(ray, distance);
    let mist = mist_depth(ray, distance);
    let depth = weather + haze + mist;
    if depth <= 0.0 {
        return Fog(vec3(0.0), 0.0);
    }
    let light = clouds.fog.rgb * view.exposure * weather + lights.haze * haze + lights.mist * mist;
    return Fog(light / depth, depth);
}

// Light that ground haze scatters towards the eye where it is opaque. With the atmosphere it is
// at least the sky just above the horizon below the ray, from Bevy's tables: haze and sky are
// the same air, so distant haze meets the sky without a band at any time of day.
fn haze_light(ray: vec3<f32>, air: vec3<f32>) -> vec3<f32> {
#ifdef ATMOSPHERE
    let flat = select(vec2(1.0, 0.0), normalize(ray.xz), dot(ray.xz, ray.xz) > 1e-6);
    let horizon = normalize(vec3(flat.x, HORIZON_LIFT, flat.y));
    let clear = clear_sky(horizon);
    // Never below the haze's own sun and sky light: towards the sun that is its bright
    // aureole, which light shafts take back where the haze lies in shadow, and at night the
    // authored fill. Under a closed deck the horizon is the deck's grey, not the clear sky's.
    let own = clouds.haze.rgb * air;
    return mix(max(clear, own), own, clouds.weather.z);
#else
    return clouds.haze.rgb * air;
#endif
}
// Sine of the elevation the haze takes its colour from: just above the horizon.
const HORIZON_LIFT: f32 = 0.004;

fn behind_fog(fog: Fog, behind: Path) -> Path {
    let transmittance = exp(-fog.depth);
    return in_front(fog.light * (1.0 - transmittance), transmittance, behind);
}

fn fogged(ray: vec3<f32>, distance: f32, lights: FogLight, behind: Path) -> Path {
    return behind_fog(fog_along(ray, distance, lights), behind);
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

#ifdef CACHED_CLOUDS
// The cloud panorama as a cubic B-spline, in four bilinear taps: drawn bilinearly, sharp cloud
// edges showed the texels as staircases.
fn panorama_cubic(panorama: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(panorama));
    let p = uv * size - 0.5;
    let i = floor(p);
    let f = p - i;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    let w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w2 = (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    let lo = (i - 0.5 + w1 / g0) / size;
    let hi = (i + 1.5 + w3 / g1) / size;
    return g0.y * (g0.x * textureSampleLevel(panorama, cloud_sampler, lo, 0.0)
            + g1.x * textureSampleLevel(panorama, cloud_sampler, vec2(hi.x, lo.y), 0.0))
        + g1.y * (g0.x * textureSampleLevel(panorama, cloud_sampler, vec2(lo.x, hi.y), 0.0)
            + g1.x * textureSampleLevel(panorama, cloud_sampler, hi, 0.0));
}
#endif

// The cloud layer where the ray crosses its base, `start` metres away; `cubic` for the smooth
// lookup, which reflections in the waves have no need for.
fn cloud_layer(ray: vec3<f32>, start: f32, screen_uv: vec2<f32>, cubic: bool) -> Cloud {
#ifdef CACHED_CLOUDS
    let uv = sky_panorama_uv(ray);
    var c: vec4<f32>;
    if cubic {
        c = mix(panorama_cubic(cloud_older, uv), panorama_cubic(cloud_newer, uv), cloud_blend.x);
    } else {
        c = mix(textureSampleLevel(cloud_older, cloud_sampler, uv, 0.0),
            textureSampleLevel(cloud_newer, cloud_sampler, uv, 0.0), cloud_blend.x);
    }
#else
    let c = textureSampleLevel(cloud_newer, cloud_sampler, screen_uv, 0.0);
#endif
    let haze = exp(-start * 3.912 / max(clouds.haze.w, 50.0));
    // Haze in front of an opaque cloud must not reintroduce the sun/moon disk behind it. With
    // the atmosphere, a cloud lost in haze turns into the sky it stands in.
    var air = clouds.haze.rgb * (clouds.ambient.rgb * clouds.ambient.w * 0.3
        + clouds.near_sun.rgb * 0.025
        + clouds.moon_color.rgb * clouds.moon.w * 0.025) * view.exposure;
#ifdef ATMOSPHERE
    // Under a closing deck far clouds keep the deck's grey.
    air = mix(clear_sky(ray) * view.exposure, air, clouds.weather.z);
#endif
    var lit = c.rgb * 1024.0;
    // A lightning flash lights the clouds, most near the strike.
    if clouds.lightning.w > 0.0 {
        let to_strike = normalize(clouds.lightning.xyz - view.world_position);
        let near = exp(-(1.0 - dot(ray, to_strike)) * 25.0) + 0.12;
        lit += LIGHTNING_COLOR * clouds.lightning.w * near * (1.0 - c.a);
    }
    let color = mix(air * (1.0 - c.a), lit * view.exposure, haze);
    // Fade the finite ground-view tracing range into the horizon rather than exposing a
    // straight edge at the end of the cloud layer.
    // A closed deck stays to the horizon: fading it showed clear sky beneath.
    let coverage = 1.0 - smoothstep(20000.0, 40000.0, start) * (1.0 - clouds.weather.z);
    return Cloud(color * coverage, 1.0 - coverage * (1.0 - c.a));
}

// Lightning (`atmosphere::lightning`): the flash's colour, and the channel's exposed brightness
// at its core, far past white so bloom spreads it.
const LIGHTNING_COLOR: vec3<f32> = vec3(0.85, 0.9, 1.0);
const CHANNEL_BRIGHTNESS: f32 = 80.0;

// The lightning channel seen along `ray`, where it lies nearer than `limit` metres: a core about
// a pixel wide and a halo, dimmed by the haze and rain fog in front.
fn lightning_glow(ray: vec3<f32>, limit: f32) -> vec3<f32> {
    let channel = clouds.lightning_channel.x;
    if channel <= 0.0 {
        return vec3(0.0);
    }
    let pixel = 2.0 / (view.clip_from_view[1][1] * view.main_pass_viewport.w);
    let o = view.world_position;
    var glow = 0.0;
    for (var i = 0u; i < 16u; i += 1u) {
        let a = clouds.lightning_segments[2u * i];
        let b = clouds.lightning_segments[2u * i + 1u].xyz;
        let along = b - a.xyz;
        let w0 = o - a.xyz;
        let bd = dot(ray, along);
        let c = dot(along, along);
        let denom = max(c - bd * bd, 1e-6);
        let s = clamp((dot(along, w0) - bd * dot(ray, w0)) / denom, 0.0, 1.0);
        let q = a.xyz + along * s;
        let t = dot(q - o, ray);
        if t <= 0.0 || t > limit {
            continue;
        }
        let px = length(o + ray * t - q) / (t * pixel);
        let fade = exp(-t * (clouds.fog.w + 3.912 / max(clouds.haze.w, 50.0)));
        glow += a.w * fade * (exp(-px * px * 0.5) + 0.06 / (1.0 + px * px / 16.0));
    }
    return LIGHTNING_COLOR * glow * channel * CHANNEL_BRIGHTNESS;
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

// The clear sky's light from Bevy's tables, unexposed, at least the moonless night sky's glow:
// airglow and starlight, brightest towards the horizon, where the eye looks through more of the
// glowing air. Without it a night with the moon down would have a black sky over lit ground;
// under the moon its sky is brighter and the glow does not show.
fn clear_sky(ray: vec3<f32>) -> vec3<f32> {
    let glow = clouds.night_sky.rgb * mix(1.0, 0.45, sqrt(clamp(ray.y, 0.0, 1.0)));
    return max(sample_sky_view_lut(length(get_view_position()),
        direction_world_to_atmosphere(ray)), glow);
}

// The moon's near side, coarsely: its seas (maria) as overlapping round patches with ragged
// shores at their real places, and the brightest young craters, Tycho with its rays. Patches
// are a point in the moon's frame (x east, y north, z towards the earth) and a radius; normal
// albedo is about 0.13 on the highlands and half that in the darkest seas.
const MOON_SEAS: array<vec4<f32>, 21> = array<vec4<f32>, 21>(
    vec4(-0.7912, 0.2588, 0.5540, 0.2967), // Oceanus Procellarum
    vec4(-0.7233, 0.5736, 0.3846, 0.2094),
    vec4(-0.7044, -0.0872, 0.7044, 0.2094),
    vec4(-0.9237, 0.0872, 0.3732, 0.1745),
    vec4(-0.5523, 0.3420, 0.7602, 0.1396),
    vec4(-0.2312, 0.5446, 0.8062, 0.2967), // Imbrium
    vec4(0.2655, 0.4695, 0.8421, 0.1920), // Serenitatis
    vec4(0.4677, 0.0872, 0.8796, 0.1920), // Tranquillitatis
    vec4(0.6022, 0.2079, 0.7708, 0.1396),
    vec4(0.8197, 0.2924, 0.4925, 0.1484), // Crisium
    vec4(0.7729, -0.1045, 0.6259, 0.1745), // Fecunditatis
    vec4(0.5609, -0.2588, 0.7864, 0.0960), // Nectaris
    vec4(-0.2730, -0.3584, 0.8928, 0.1920), // Nubium
    vec4(-0.5793, -0.4131, 0.7027, 0.1134), // Humorum
    vec4(-0.2302, 0.8387, 0.4936, 0.0873), // Frigoris
    vec4(0.0000, 0.8387, 0.5446, 0.0873),
    vec4(0.2095, 0.8290, 0.5185, 0.0785),
    vec4(0.0611, 0.2300, 0.9713, 0.0698), // Vaporum
    vec4(-0.3848, -0.1736, 0.9065, 0.1047), // Cognitum
    vec4(-0.5106, 0.1305, 0.8498, 0.1222), // Insularum
    vec4(0.0296, 0.0419, 0.9987, 0.0524), // Sinus Medii
);
const MOON_SEA_DARKNESS: array<f32, 21> = array<f32, 21>(0.9, 0.85, 0.85, 0.85, 0.8, 1.0, 0.85,
    1.0, 0.95, 1.0, 0.85, 0.85, 0.8, 0.9, 0.75, 0.75, 0.7, 0.8, 0.8, 0.8, 0.7);
const MOON_CRATERS: array<vec4<f32>, 5> = array<vec4<f32>, 5>(
    vec4(-0.1438, -0.6858, 0.7134, 0.0279), // Tycho
    vec4(-0.3388, 0.1668, 0.9259, 0.0244), // Copernicus
    vec4(-0.6095, 0.1409, 0.7801, 0.0175), // Kepler
    vec4(-0.6740, 0.4019, 0.6198, 0.0157), // Aristarchus
    vec4(0.7015, 0.2773, 0.6565, 0.0140), // Proclus
);
fn moon_albedo(p: vec3<f32>) -> f32 {
    let broad = textureSampleLevel(noise, noise_sampler, p * 0.9 + 0.31, 0.0);
    let fine = textureSampleLevel(noise, noise_sampler, p * 3.1 + 0.57, 0.0);
    // Shores move by a few degrees; neighbouring patches merge into one sea.
    let ragged = (broad.r - 0.5) * 0.1 + (fine.g - 0.5) * 0.04;
    var sea = 0.0;
    for (var i = 0u; i < 21u; i += 1u) {
        let sea_patch = MOON_SEAS[i];
        let angle = acos(clamp(dot(p, sea_patch.xyz), -1.0, 1.0)) + ragged;
        let inside = 1.0 - smoothstep(0.65 * sea_patch.w, 1.25 * sea_patch.w, angle);
        sea = 1.0 - (1.0 - sea) * (1.0 - inside * MOON_SEA_DARKNESS[i]);
    }
    var albedo = mix(0.13, 0.07, sea) * (0.9 + 0.2 * fine.b);
    for (var i = 0u; i < 5u; i += 1u) {
        let crater = MOON_CRATERS[i];
        let angle = acos(clamp(dot(p, crater.xyz), -1.0, 1.0)) / crater.w;
        albedo += 0.12 * exp(-angle * angle);
    }
    let tycho = MOON_CRATERS[0].xyz;
    let across = normalize(cross(tycho, vec3(0.0, 1.0, 0.0)));
    let along = cross(tycho, across);
    let bearing = atan2(dot(p, across), dot(p, along));
    let distance = acos(clamp(dot(p, tycho), -1.0, 1.0));
    let rays = pow(max(sin(bearing * 7.0 + 1.3 * sin(bearing * 3.0)), 0.0), 12.0);
    return albedo + 0.035 * rays * exp(-distance / 0.7) * smoothstep(0.03, 0.08, distance);
}

// The moon: a sphere lit by the sun with Lommel-Seeliger reflection, as dusty ground reflects,
// so a full moon is evenly bright to its edge and a crescent dims towards the terminator; its
// dark side faintly lit by earthshine. rgb its unexposed light, a the share of the pixel it
// covers, which hides the stars behind it.
fn moon(ray: vec3<f32>) -> vec4<f32> {
    let radius = clouds.moon_disc.w;
    let centre = clouds.moon_disc.xyz;
    let c = dot(ray, centre);
    let angle = acos(clamp(c, -1.0, 1.0));
    let w = max(0.5 * fwidth(angle), 1e-6);
    let cover = 1.0 - smoothstep(radius - w, radius + w, angle);
    if radius <= 0.0 || cover <= 0.0 {
        return vec4(0.0);
    }
    let north = clouds.moon_frame.xyz;
    let east = normalize(cross(centre, north));
    // The ray's point on the moon: its offset across the disc in radii, then the sphere.
    let offset = (ray - centre * c) / sin(radius);
    let x = dot(offset, east);
    let y = dot(offset, north);
    let z = sqrt(max(1.0 - x * x - y * y, 0.0));
    let normal = east * x + north * y - centre * z;
    let sun = dot(normal, clouds.moon_sunward.xyz);
    let lit = select(0.0, 2.0 * sun / (sun + max(z, 1e-3)), sun > 0.0);
    let albedo = moon_albedo(vec3(x, y, z));
    return vec4(clouds.moon_face.rgb * albedo * (lit + clouds.moon_frame.w), cover);
}

// Stars: one star at most in each cell of a grid over the sky's directions, at a hashed spot
// away from the cell's edges, so a few thousand show above the horizon, mostly faint. They come
// out as twilight ends (sun 3 to 11 degrees down), drawn in exposed units about a pixel wide;
// clouds, haze and the atmosphere in front dim them as they dim the sky.
const STAR_CELLS: f32 = 160.0;
const STAR_SHARE: f32 = 0.03;
const STAR_PEAK: f32 = 6.0;
fn star_hash(p: vec3<f32>) -> vec3<f32> {
    var q = fract(p * vec3(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yxz + 33.33);
    return fract((q.xxy + q.yxx) * q.zyx);
}
fn stars(ray: vec3<f32>) -> vec3<f32> {
    let night = 1.0 - smoothstep(-0.2, -0.05, clouds.sun.y);
    if night <= 0.0 || ray.y < -0.01 {
        return vec3(0.0);
    }
    let cell = floor(ray * STAR_CELLS);
    let pick = star_hash(cell);
    if pick.x > STAR_SHARE {
        return vec3(0.0);
    }
    let star = normalize(cell + 0.2 + 0.6 * star_hash(cell + 17.0));
    // Angle to the star against the angle one pixel spans.
    let pixel = 2.0 / (view.clip_from_view[1][1] * view.main_pass_viewport.w);
    let offset = length(cross(ray, star)) / max(pixel, 1e-6);
    // Many faint stars and a few bright ones.
    let brightness = 0.02 + pow(pick.z, 5.0);
    let colour = mix(vec3(1.0, 0.82, 0.62), vec3(0.75, 0.85, 1.0), smoothstep(0.2, 0.8, pick.y));
    return colour * (STAR_PEAK * brightness * night * exp(-offset * offset * 1.4));
}

// Share of the sun and moon discs a closing deck leaves: their light is so much brighter than
// the sky that the thinnest gap in a rain deck would show them.
fn deck_open() -> f32 {
    let open = 1.0 - clouds.weather.z;
    return open * open;
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
const AIR_GLOW_SHARE: f32 = 0.05;
const AIR_GLOW_WIDTH: f32 = 0.15;
// Depth of air over which the glow builds up in front of a surface, metres, and how much faster
// it builds up in the air under crowns (as a power of that air's transmittance). Over 30 m it
// veiled the ground in front of a low sun; distant land and the air under crowns keep it.
const AIR_GLOW_DEPTH: f32 = 200.0;
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

// The open sea (`atmosphere::SeaSurface`) is drawn here, not by the main pass: each view ray is
// intersected with the sea's surface, flat out to SEA_FLAT_RADIUS around the view and then
// bending with the planet, and whatever the main pass drew beyond it (the seabed, the foot of a
// rock, or nothing far out) is seen through the water. Shallow water shows the sand below; deep
// water turns into the light its body scatters up; foam runs along the waterline.
const SEA_FLAT_RADIUS: f32 = 10000.0;
const SEA_PLANET_RADIUS: f32 = 6360000.0;
const NO_SEA: f32 = 1.0e30;
const WATER_F0: f32 = 0.02;
// Size of the noise tile the gust patches come from, metres.
const SEA_PATCH: f32 = 1200.0;
// Glitter is kept within half floats like the sun's disc.
const MAX_GLITTER: f32 = 30000.0;
// Clear coastal water: extinction per metre (red is absorbed within a few metres, blue and green
// carry), and the share of the daylight entering deep water that its body scatters back up.
const WATER_EXTINCTION: vec3<f32> = vec3(0.45, 0.10, 0.075);
const WATER_ALBEDO: vec3<f32> = vec3(0.0025, 0.012, 0.016);
// The seabed's own light comes down through the water, a longer path than straight down.
const WATER_LIGHT_PATH: f32 = 1.3;
// Swash: every SWASH_PERIOD seconds (whole cycles per wave period) the water runs up the beach
// and drains back over SWASH metres of depth, its phase varying along the shore. Foam rides the
// front and trails behind it; the sand the water has drained from is left wet and darker.
// Widths are along the ground, in metres, turned into depth by the seabed's slope, so a nearly
// flat beach gets a thin foam line rather than sheets.
const SWASH: f32 = 0.12;
const SWASH_RUN: f32 = 12.0;
const SWASH_PERIOD: f32 = 9.0;
const FOAM_FRONT: f32 = 0.35;
const FOAM_TRAIL: f32 = 2.0;
const FOAM_ALBEDO: f32 = 0.7;
const WET_SAND: f32 = 0.6;
const WET_RUN: f32 = 1.5;
// Run over which a film of water gains its full reflection and colour.
const THIN_WATER: f32 = 1.5;
// Water deeper than this is past the reach of the swash and its foam.
const SHORE_DEPTH: f32 = 0.3;
// Relief of the sand, metres: no band of the swash is thinner in depth than this, or on a nearly
// flat beach its centimetre bumps and hollows cut film, wet sand and foam into hard patches.
const SAND_RELIEF: f32 = 0.03;

// Depth over which a band of the swash `run` metres wide along the ground lies, on a seabed rising
// `slope` per metre.
fn swash_band(slope: f32, run: f32) -> f32 {
    return max(slope * run, SAND_RELIEF);
}

// Height of the main pass's surface at a pixel, from its first depth sample.
fn surface_height(pixel: vec2<i32>) -> vec3<f32> {
    let z = textureLoad(depth, pixel, 0);
    let uv = (vec2<f32>(pixel) + 0.5 - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let world = view.world_from_clip * vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), z, 1.0);
    return world.xyz / world.w;
}

// Rise of the seabed per metre along the ground at a pixel, averaged over the next pixels'
// surfaces two pixels away in each direction, at least that of a very flat beach.
fn seabed_slope(pixel: vec2<i32>, here: vec3<f32>) -> f32 {
    var rise = 0.0;
    var count = 0.0;
    for (var i = 0; i < 4; i += 1) {
        let offset = select(vec2(0, 2), vec2(2, 0), (i & 1) == 0) * select(1, -1, i >= 2);
        let other = surface_height(pixel + offset);
        let run = length(other.xz - here.xz);
        if run > 1e-3 && run < 20.0 {
            rise += abs(other.y - here.y) / run;
            count += 1.0;
        }
    }
    return clamp(rise / max(count, 1.0), 0.002, 1.0);
}

// Distance along `ray` to the sea's surface, or NO_SEA.
fn sea_distance(ray: vec3<f32>) -> f32 {
    if clouds.ocean_waves.x < 0.5 || ray.y >= 0.0 {
        return NO_SEA;
    }
    let height = view.world_position.y - clouds.ocean.x;
    if height <= 0.0 {
        return NO_SEA;
    }
    let flat = height / -ray.y;
    let h = length(ray.xz);
    if flat * h <= SEA_FLAT_RADIUS {
        return flat;
    }
    // Beyond it the surface sinks by (r - F)^2 / 2R; the nearer root of a t^2 + b t + c = 0,
    // in the form that keeps its precision.
    let a = h * h / (2.0 * SEA_PLANET_RADIUS);
    let b = ray.y - SEA_FLAT_RADIUS * h / SEA_PLANET_RADIUS;
    let c = SEA_FLAT_RADIUS * SEA_FLAT_RADIUS / (2.0 * SEA_PLANET_RADIUS) + height;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return NO_SEA;
    }
    return 2.0 * c / (-b + sqrt(disc));
}

// Breaking waves. Crests come in from deep water along the shore map's lines of equal arrival
// time, at its pace: they lie along the shore and close up as the water shallows. A wave grows
// as it slows (Green's law, the fourth root of the depth's fall) and breaks where its height
// reaches BREAKING of the depth; from there on its height is held to that share, a white roller
// runs on its front and it leaves foam behind that breaks up into lace. Before breaking its crest
// sharpens and its front steepens. SURF_HEIGHT is the swell's height in deep water, scaled by the
// square of the weather's wind strength.
const SURF_HEIGHT: f32 = 0.6;
const SURF_DEPTH: f32 = 12.0;
const BREAKING: f32 = 0.78;
// Seconds the foam behind a broken wave lasts, and how far apart along the shore crests arrive
// at different times, metres.
const SURF_FOAM_LIFE: f32 = 3.0;
const SURF_ALONG: f32 = 260.0;
// Shore map times past this are water no crest reaches (`atmosphere::shore::UNREACHED`).
const SURF_UNREACHED: f32 = 500.0;
// Crests stand up out of the water where the view ray falls steeply enough for the march to
// resolve them (a sine of its descent of SURF_STANDING, faded in from 60% of it): a grazing ray
// crosses the waves' band over hundreds of metres and caught or missed crests at random. Farther
// out their slopes and foam draw them on the flat surface, as they do the broad swell beyond
// SURF_STANDING_DEPTH, where crests are low and their standing up shows little.
const SURF_STANDING: f32 = 0.09;
const SURF_STANDING_DEPTH: f32 = 4.0;
// Steps of the march: about SURF_STEP metres along the view, within these bounds.
const SURF_STEP: f32 = 1.0;
const SURF_STEPS: f32 = 8.0;
// The band the surf fills at the weather's usual wind, metres above and below the sea level: the
// highest crests (the largest of a set at their breaking point) and the deepest troughs.
const SURF_CRESTS: f32 = 1.2;
const SURF_TROUGHS: f32 = 0.6;

// Foam as a network around bubbles: nearly whole when fresh, opening into holes as it ages
// (0..1) until thin threads are left, the holes at the Worley cells' cores half a metre across. Past a fifth of a cell per
// pixel it fades into the veil it averages to.
const LACE_SCALE: f32 = 6.0;
fn foam_lace(xz: vec2<f32>, age: f32, footprint: f32) -> f32 {
    let cells = textureSampleLevel(noise, noise_sampler, vec3(xz / LACE_SCALE, 0.41), 0.0).g;
    let open = mix(0.95, 0.35, age);
    let lace = 1.0 - smoothstep(open, open + 0.06, cells);
    let veil = clamp((open - 0.3) / 0.65, 0.0, 1.0);
    return mix(lace, veil, smoothstep(0.1, 0.5, footprint));
}

struct Surf {
    // Depth below the sea level by the shore map.
    depth: f32,
    // Height of the surface above the sea level, and its slope.
    height: f32,
    slope: vec2<f32>,
    foam: f32,
    // How far the waves here have broken, 0..1.
    broken: f32,
    // Where the waves are in their cycle: 0 as a crest passes, rising to 1 as the next comes.
    cycle: f32,
}

// Three fields varying smoothly along a shore, 0..1, a few hundred metres across: sums of sines
// in different directions, so they never repeat visibly along it.
fn along_shore(local: vec2<f32>) -> vec3<f32> {
    let q = local * (250.0 / SURF_ALONG);
    let a = sin(dot(q, vec2(0.0213, 0.0137)) + 1.3) + sin(dot(q, vec2(-0.0089, 0.0291)) + 4.1);
    let b = sin(dot(q, vec2(0.0171, -0.0233)) + 2.7) + sin(dot(q, vec2(0.0307, 0.0071)) + 0.4);
    let c = sin(dot(q, vec2(-0.0247, -0.0119)) + 5.2) + sin(dot(q, vec2(0.0063, -0.0311)) + 3.3);
    return 0.5 + 0.25 * vec3(a, b, c);
}

// `footprint` is the pixel's length on the water along the view, metres: each side of a crest is
// a Gaussian, blurred over the footprint by adding their variances (keeping its area), so far
// surf softens instead of breaking up into speckle; the foam's lace fades into an even veil.
// Without `shade` only the height is worked out, for the march to the surface.
fn surf_at(xz: vec2<f32>, footprint: f32, shade: bool) -> Surf {
    var surf = Surf(1.0e4, 0.0, vec2(0.0), 0.0, 0.0, 0.0);
    if clouds.shore_map.w <= 0.0 {
        return surf;
    }
    let size = vec2<f32>(textureDimensions(shore_map));
    // World-anchored: from the map's own corner.
    let local = xz - clouds.shore_map.xy;
    let map = textureSampleLevel(shore_map, mist_sampler, local / (clouds.shore_map.z * size), 0.0);
    let depth = map.x;
    surf.depth = depth;
    let period = clouds.shore_map.w;
    let along = along_shore(local);
    let theta = (clouds.ocean.w - map.y) / period + along.r * 1.5;
    let s = fract(theta);
    surf.cycle = s;
    let weight = 1.0 - smoothstep(SURF_DEPTH * 0.6, SURF_DEPTH, depth);
    if weight <= 0.0 || map.y >= SURF_UNREACHED || depth <= 0.0 {
        return surf;
    }
    // Sets: each crest its own height (golden-angle steps of a sine, so neighbours differ), which
    // shifts smoothly along the shore, and some stretches of shore get bigger ones. Between the
    // crest that has passed and the one coming the water takes after both, so the surface stays
    // whole across each crest.
    let passed = floor(theta);
    let luck = 0.5 + 0.5 * sin(vec2(passed, passed + 1.0) * 2.39996 + along.g * 6.2831853);
    let strength = clouds.ocean_waves.y;
    let shoaling = pow(SURF_DEPTH / max(depth, 0.3), 0.25);
    let shoaled = SURF_HEIGHT * strength * strength * (0.6 + 0.8 * luck) * (0.7 + 0.6 * along.b)
        * shoaling;
    let ratio = mix(shoaled.x, shoaled.y, s) / (BREAKING * depth);
    surf.broken = smoothstep(0.9, 1.2, ratio) * weight;
    // Heights of the crest behind and the one ahead.
    let crests = min(shoaled, vec2(BREAKING * depth)) * weight;
    // Distances to the crest ahead (the front) and behind, in periods; widths of each side, and
    // the footprint in periods (a period's length is the time's gradient's inverse).
    let steep = clamp(ratio, 0.0, 1.0);
    let front = mix(0.2, 0.05, steep * steep);
    let back = mix(0.2, 0.32, steep);
    let blur = footprint * length(map.zw) / period;
    let wide_front = sqrt(front * front + blur * blur);
    let wide_back = sqrt(back * back + blur * blur);
    let v = 1.0 - s;
    let ahead = front / wide_front * exp(-(v * v) / (wide_front * wide_front));
    let behind = back / wide_back * exp(-(s * s) / (wide_back * wide_back));
    surf.height = crests.y * ahead + crests.x * behind
        - 0.886 * (front + back) * mix(crests.x, crests.y, s);
    if !shade {
        return surf;
    }
    let rise = crests.y * 2.0 * v / (wide_front * wide_front) * ahead
        - crests.x * 2.0 * s / (wide_back * wide_back) * behind;
    surf.slope = rise * -map.zw / period;
    // The roller on a broken wave's front, and the foam it leaves, breaking up into lace.
    if surf.broken <= 0.0 {
        return surf;
    }
    let wide_roller = sqrt(0.035 * 0.035 + blur * blur);
    let roller = 0.035 / wide_roller * exp(-(v * v) / (wide_roller * wide_roller));
    let drift = clouds.ocean.yz * clouds.ocean.w * 0.2;
    // It thins as it opens, and the bubbles left grow clearer.
    let left = exp(-s * period / SURF_FOAM_LIFE);
    let trail = left * left * (0.5 + 0.5 * left) * foam_lace(local + drift, 1.0 - left, footprint);
    surf.foam = clamp(roller + 0.85 * trail, 0.0, 1.0) * surf.broken;
    return surf;
}

// Height of the surf as the march to the surface sees it.
fn standing_surf(xz: vec2<f32>, footprint: f32) -> f32 {
    let surf = surf_at(xz, footprint, false);
    return surf.height
        * (1.0 - smoothstep(0.6 * SURF_STANDING_DEPTH, SURF_STANDING_DEPTH, surf.depth));
}

// Distance along `ray` to the sea's surface with the surf standing up out of it, or NO_SEA.
// Near the shore the view ray is marched down through the band the waves fill, from their
// highest crests to below their troughs, then refined between the samples either side.
fn sea_surface(ray: vec3<f32>) -> f32 {
    let flat = sea_distance(ray);
    let standing = smoothstep(0.6 * SURF_STANDING, SURF_STANDING, -ray.y);
    if flat >= NO_SEA || standing <= 0.0 || clouds.shore_map.w <= 0.0 {
        return flat;
    }
    let p = view.world_position + ray * flat;
    let size = vec2<f32>(textureDimensions(shore_map));
    // The pixel's length on the water along the view, per metre of distance.
    let spread = 2.0 / (view.clip_from_view[1][1] * view.main_pass_viewport.w) / max(-ray.y, 0.02);
    let map = textureSampleLevel(shore_map, mist_sampler,
        (p.xz - clouds.shore_map.xy) / (clouds.shore_map.z * size), 0.0);
    if map.x >= SURF_STANDING_DEPTH + 1.0 || map.y >= SURF_UNREACHED {
        return flat;
    }
    let strength = clouds.ocean_waves.y;
    let scale = strength * strength * standing;
    let height = view.world_position.y - clouds.ocean.x;
    let first = max(height - SURF_CRESTS * scale, 0.0) / -ray.y;
    let last = (height + SURF_TROUGHS * scale) / -ray.y;
    var before = first;
    var above = height + ray.y * first
        - standing_surf((view.world_position + ray * first).xz, first * spread) * standing;
    if above <= 0.0 {
        return first;
    }
    let steps = clamp(ceil((last - first) / SURF_STEP), 4.0, SURF_STEPS);
    for (var i = 1.0; i <= steps; i += 1.0) {
        let t = mix(first, last, i / steps);
        let here = height + ray.y * t
            - standing_surf((view.world_position + ray * t).xz, t * spread) * standing;
        if here <= 0.0 {
            var a = before;
            var fa = above;
            var b = t;
            var fb = here;
            for (var k = 0u; k < 2u; k += 1u) {
                let m = a + (b - a) * fa / max(fa - fb, 1e-4);
                let fm = height + ray.y * m
                    - standing_surf((view.world_position + ray * m).xz, m * spread) * standing;
                if fm > 0.0 {
                    a = m;
                    fa = fm;
                } else {
                    b = m;
                    fb = fm;
                }
            }
            return a + (b - a) * fa / max(fa - fb, 1e-4);
        }
        before = t;
        above = here;
    }
    return flat;
}

// Share of the sun's direct light the cloud layer lets through to `p`.
fn sea_cloud_shadow(p: vec3<f32>) -> f32 {
    let sun = clouds.sun.xyz;
    if clouds.layer.w < 0.5 || sun.y <= 0.0 {
        return 1.0;
    }
    let hit = p.xz + clouds.offset.xy + sun.xz * (clouds.layer.x - p.y) / max(sun.y, 0.04);
    return textureSampleLevel(cloud_shadow, cloud_shadow_sampler,
        (hit - clouds.offset.zw) / (clouds.layer.z * 4.0), 0.0).r;
}

fn smith(n_dot_x: f32, alpha2: f32) -> f32 {
    return 2.0 * n_dot_x / (n_dot_x + sqrt(alpha2 + (1.0 - alpha2) * n_dot_x * n_dot_x));
}

// The sea in front of a sample `distance` metres away (NO_SEA for open sky), with its surface
// `sea_t` metres along the ray and `air` the path in front of the surface.
fn through_water(air: Path, ray: vec3<f32>, sea_t: f32, distance: f32, at: vec2<i32>) -> Path {
    let p = view.world_position + ray * sea_t;
    // The pixel's length on the water, stretched where the view grazes it.
    let pixel = 2.0 / (view.clip_from_view[1][1] * view.main_pass_viewport.w);
    let footprint = sea_t * pixel / max(-ray.y, 0.02);
    // Gusts roughen the water in patches a few hundred metres across, drifting with the wind
    // like the mist's wisps; calm patches between them read smoother and brighter.
    let q = (p.xz + clouds.mist_drift.xy) / SEA_PATCH;
    let gust = textureSampleLevel(noise, noise_sampler, vec3(q.x, 0.37, q.y), 0.0);
    let patches = mix(0.35, 1.65, smoothstep(0.25, 0.75, gust.r * 0.6 + gust.g * 0.4));
    // Near the shore the surf rides on them; broken and shallow water have little wind chop of
    // their own.
    let surf = surf_at(p.xz, footprint, true);
    let calm = (1.0 - 0.6 * surf.broken) * mix(0.15, 1.0, smoothstep(0.0, 2.0, surf.depth));
    let waves = sea_waves(p.xz, clouds.ocean.w, clouds.ocean.yz,
        clouds.ocean_waves.y * patches * calm, footprint);
    let tilt = waves.slope + surf.slope;
    let n = normalize(vec3(-tilt.x, 1.0, -tilt.y));
    let v = -ray;
    let n_dot_v = max(dot(n, v), 0.02);
    let fresnel = WATER_F0 + (1.0 - WATER_F0) * pow(1.0 - n_dot_v, 5.0);
    // Reflections of waves facing away from the eye would point into the sea: keep them just
    // above the horizon.
    let bounced = reflect(ray, n);
    let reflected = normalize(vec3(bounced.x, max(bounced.y, 0.004), bounced.z));
    let position = get_view_position();
    var sky = clear_sky(reflected) * view.exposure;
    // Under a closing deck the low sky is the deck's grey, as for the haze.
    let deck = clouds.haze.rgb * (clouds.air_light.rgb
        + clouds.air_sun.rgb * scattering(dot(reflected, clouds.sun.xyz))) * view.exposure;
    sky = mix(sky, deck, clouds.weather.z);
#ifdef CLOUDS
#ifdef CACHED_CLOUDS
    if reflected.y > 0.01 {
        let cloud = cloud_layer(reflected, max(0.0, (clouds.layer.x - p.y) / reflected.y),
            vec2(0.0), false);
        sky = cloud.light + cloud.transmittance * sky;
    }
#endif
#endif
    // Sun and moon glitter: GGX over the resolved waves, widened by the unresolved ones and
    // the disc's own size; clouds shade it.
    let direct = sea_cloud_shadow(p);
    // Diffuse daylight keeps a floor of light scattered through the clouds, as surfaces do.
    let shade = max(direct, 0.12);
    let alpha2 = waves.variance + 0.0006;
    var glitter = vec3(0.0);
    for (var i = 0u; i < lights.n_directional_lights; i += 1u) {
        let light = &lights.directional_lights[i];
        let l = (*light).direction_to_light;
        let n_dot_l = dot(n, l);
        if l.y <= 0.0 || n_dot_l <= 0.0 {
            continue;
        }
        let h = normalize(l + v);
        let n_dot_h = max(dot(n, h), 0.0);
        let d = alpha2 / (3.14159265 * pow(n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0, 2.0));
        let f = WATER_F0 + (1.0 - WATER_F0) * pow(1.0 - max(dot(v, h), 0.0), 5.0);
        let g = smith(n_dot_v, alpha2) * smith(n_dot_l, alpha2);
        let transmittance = sample_transmittance_lut(length(position), dot(l, normalize(position)));
        // The moon's image is its drawn face, much dimmer than its art-directed light.
        let image = select(1.0, clouds.moon_sunward.w,
            clouds.moon_disc.w > 0.0 && dot(l, clouds.moon_disc.xyz) > 0.9999);
        glitter += (*light).color.rgb * transmittance * image * (d * f * g / (4.0 * n_dot_v));
    }
    // The glitter is the sun's own image: a closing deck hides it entirely.
    let open = 1.0 - clouds.weather.z;
    glitter *= direct * open * open;
    // Daylight entering the water, as radiance a white diffuser would send back.
    let sun = clouds.near_sun.rgb * max(clouds.sun.y, 0.0) * shade
        + clouds.moon_color.rgb * clouds.moon.w * max(clouds.moon.y, 0.0);
    let daylight = (sun / 3.14159265 + clouds.ambient.rgb * clouds.ambient.w) * view.exposure;
    // The water between the surface and what lies beyond, and the seabed's light coming down.
    let column = max(distance - sea_t, 0.0);
    let seabed = view.world_position.y + ray.y * distance;
    let depth = select(1.0e4, max(clouds.ocean.x - seabed, 0.0), distance < NO_SEA);
    let seen = exp(-WATER_EXTINCTION * min(column, 1.0e4));
    let lit = exp(-WATER_EXTINCTION * min(depth * WATER_LIGHT_PATH, 1.0e4));
    // Broken water is full of bubbles and stirred-up sand, and scatters more light back.
    let body = WATER_ALBEDO * (1.0 + 3.0 * surf.broken) * daylight * (1.0 - seen);
    var open_water = fresnel * sky + min(glitter * view.exposure, vec3(MAX_GLITTER))
        + (1.0 - fresnel) * body;
    // The seabed under the water is wet sand, darker than dry.
    var open_transmittance = (1.0 - fresnel) * seen * lit * WET_SAND;
    open_water = mix(open_water, FOAM_ALBEDO * daylight, surf.foam);
    open_transmittance *= 1.0 - surf.foam;
    if depth >= SHORE_DEPTH {
        return Path(air.light + air.transmittance * open_water,
            air.transmittance * open_transmittance);
    }
    // The swash: a quick run-up as each broken wave comes ashore and a slow drain, its front at
    // `edge` metres of depth. Without a shore map it keeps its own time.
    var cycle = surf.cycle;
    if clouds.shore_map.w <= 0.0 {
        let along = textureSampleLevel(noise, noise_sampler, vec3(p.xz / 90.0, 0.61), 0.0).r;
        cycle = fract(clouds.ocean.w / SWASH_PERIOD + along * 3.0);
    }
    let surge = select(1.0 - smoothstep(0.25, 1.0, cycle), smoothstep(0.0, 0.25, cycle),
        cycle < 0.25);
    // Near the waterline, the seabed's slope turns runs along the ground into depths.
    var slope = 1.0;
    if distance < NO_SEA {
        slope = seabed_slope(at, view.world_position + ray * distance);
    }
    let edge = min(SWASH, slope * SWASH_RUN) * (1.0 - surge);
    let film = smoothstep(edge, edge + swash_band(slope, THIN_WATER), depth)
        * smoothstep(0.0, swash_band(slope, 1.0), depth);
    var light = film * open_water;
    var transmittance = mix(vec3(1.0), open_transmittance, film);
    // Sand the water has just drained from stays wet.
    let wet = 1.0 - smoothstep(0.0, swash_band(slope, WET_RUN), depth - edge);
    transmittance *= mix(1.0, WET_SAND, wet * smoothstep(0.0, 0.01, depth) * (1.0 - film));
    // Foam rides the front and trails behind it in lace, drifting with the waves.
    let drift = clouds.ocean.yz * clouds.ocean.w * 0.3;
    let front = exp(-pow((depth - edge) / swash_band(slope, FOAM_FRONT), 2.0))
        * foam_lace(p.xz + drift, 0.3, footprint);
    let trail = (1.0 - smoothstep(edge, edge + swash_band(slope, FOAM_TRAIL), depth))
        * foam_lace(p.xz - drift, 1.0, footprint)
        * smoothstep(edge, edge + swash_band(slope, FOAM_FRONT), depth);
    // Nothing starts hard at the dry edge: the terrain's contour there zigzags.
    let shore = smoothstep(0.0, swash_band(slope, 1.0), depth);
    let foam = clamp(front + 0.25 * trail, 0.0, 1.0) * (0.4 + 0.6 * surge) * shore;
    light = mix(light, FOAM_ALBEDO * daylight, foam);
    transmittance *= 1.0 - foam;
    return Path(air.light + air.transmittance * light, air.transmittance * transmittance);
}

// Air, then water where the sea lies in front, between the eye and a sample `distance` away.
fn scene_path(ray: vec3<f32>, uv: vec2<f32>, distance: f32, sea_t: f32, pixel: vec2<i32>)
    -> Path {
    var path = aerial_perspective(ray, uv, min(distance, sea_t));
#ifdef ATMOSPHERE
    if sea_t < distance {
        path = through_water(path, ray, sea_t, distance, pixel);
    }
#endif
    return path;
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
    let size = vec2<f32>(textureDimensions(sun_rays));
    let uv = (position - view.main_pass_viewport.xy) / cloud_blend.z / size;
    return textureSampleLevel(sun_rays, mist_sampler, uv, 0.0).r;
}

// Sky samples' distance as the light shafts store it, within half floats.
const SHAFT_SKY: f32 = 6.0e4;

// Light shafts at a pixel `distance` metres deep: the four nearest texels, bilinear but skipping
// those at a different distance, so beams stop at silhouettes. Where all four lie at the pixel's
// distance (nearly everywhere) the plain bilinear sample is the same; the four distances come
// in one gather. Sky pixels use 1e6.
fn light_shafts(position: vec2<f32>, distance: f32) -> vec4<f32> {
    if cloud_blend.y < 0.5 {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    let uv = (position - view.main_pass_viewport.xy) / cloud_blend.z
        / vec2<f32>(textureDimensions(shaft_light));
    let here = min(distance, SHAFT_SKY);
    // Texels (0, 0), (1, 0), (0, 1), (1, 1) of the footprint, as gather orders them w, z, x, y.
    let gathered = textureGather(0, shaft_distance, mist_sampler, uv);
    let around = vec4(gathered.w, gathered.z, gathered.x, gathered.y);
    let falloff = 0.05 * min(around, vec4(here)) + 0.3;
    let gap = abs(around - here);
    // All four within half the falloff: the weights differ by too little to show.
    if all(gap < 0.5 * falloff) {
        return textureSampleLevel(shaft_light, mist_sampler, uv, 0.0);
    }
    let size = vec2<i32>(textureDimensions(shaft_light));
    let q = (position - view.main_pass_viewport.xy) / cloud_blend.z - 0.5;
    let base = vec2<i32>(floor(q));
    let f = q - floor(q);
    let near_weight = exp(-gap / falloff);
    var sum = vec4(0.0);
    var weight = 0.0;
    var nearest = vec4(0.0, 0.0, 0.0, 1.0);
    var nearest_gap = 1.0e30;
    for (var i = 0; i < 4; i += 1) {
        let offset = vec2(i & 1, i >> 1u);
        let s = textureLoad(shaft_light, clamp(base + offset, vec2(0), size - 1), 0);
        let w = select(1.0 - f.x, f.x, offset.x == 1) * select(1.0 - f.y, f.y, offset.y == 1)
            * near_weight[i];
        sum += s * w;
        weight += w;
        if gap[i] < nearest_gap {
            nearest_gap = gap[i];
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
    let moon_disc = moon(ray);
#endif
    if any(uv < vec2(0.0)) || any(uv > vec2(1.0)) {
        discard;
    }
    let fog = clouds.fog.w > 0.0 || clouds.low_haze.x > 0.0 || clouds.mist.x > 0.0;
    // Clouds are traced only above this elevation; `start` is the distance to their base.
    // A closing deck continues down to the horizon at its lowest traced elevation, so no band of
    // clear sky shows beneath it.
    let horizon_dip = -sqrt(2.0 * max(view.world_position.y, 0.0) / 6360000.0);
    let above = ray.y > 0.01 || (clouds.weather.z > 0.0 && ray.y > horizon_dip);
    let cloud_ray = normalize(vec3(ray.x, max(ray.y, 0.01), ray.z));
    let start = select(1.0e9, max(0.0, (clouds.layer.x - view.world_position.y) / cloud_ray.y),
        above);
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
    // A sample's view-space depth from its depth value (the view-from-clip rows that give z and
    // w), and its distance along this pixel's ray.
    let to_view_z = vec4(view.view_from_clip[0].z, view.view_from_clip[1].z,
        view.view_from_clip[2].z, view.view_from_clip[3].z);
    let to_view_w = vec4(view.view_from_clip[0].w, view.view_from_clip[1].w,
        view.view_from_clip[2].w, view.view_from_clip[3].w);
    let per_depth = 1.0 / max(dot(ray, -normalize(view.world_from_view[2].xyz)), 1e-4);
    for (var i = 0u; i < samples; i += 1u) {
        let z = textureLoad(depth, pixel, i32(i));
        if z == 0.0 {
            distances[i] = -1.0;
            sky_samples += 1u;
            continue;
        }
        let clip = vec4(ndc, z, 1.0);
        let distance = -dot(to_view_z, clip) / dot(to_view_w, clip) * per_depth;
        distances[i] = distance;
        nearest = min(nearest, distance);
        farthest = max(farthest, distance);
        total += distance;
    }
    let geometry_samples = samples - sky_samples;
#ifdef ATMOSPHERE
    let sea_t = sea_surface(ray);
#else
    let sea_t = NO_SEA;
#endif

    // The fog's light, once for every sample.
    var fog_lights = FogLight(vec3(0.0), vec3(0.0));
    if fog {
        fog_lights = fog_light(ray);
    }

    // One cloud lookup serves every sample that can see the cloud base.
    var cloud = Cloud(vec3(0.0), 1.0);
#ifdef CLOUDS
    if above && (sky_samples > 0u || farthest >= start) {
        cloud = cloud_layer(cloud_ray, start, in.uv, true);
    }
#endif

    var light = vec3(0.0);
    var sky_transmittance = vec3(1.0);
    if sky_samples > 0u && sea_t < NO_SEA {
        // Open sea out to the horizon, beyond the world's ground.
        let path = fogged(ray, min(sea_t, start), fog_lights,
            scene_path(ray, uv, NO_SEA, sea_t, pixel));
        light += path.light * f32(sky_samples);
        sky_transmittance = path.transmittance;
    } else if sky_samples > 0u {
#ifdef ATMOSPHERE
        let position = get_view_position();
        let r = length(position);
        let transmittance = sample_transmittance_lut(r, dot(ray, normalize(position)));
        let sky = clear_sky(ray);
        var path = Path(sky * view.exposure
            + min((disks + moon_disc.rgb) * transmittance * view.exposure * deck_open(),
                vec3(MAX_DISC))
            + stars(ray) * transmittance * (1.0 - moon_disc.a), transmittance);
#else
        var path = Path(vec3(0.0), vec3(1.0));
#endif
        path = fogged(ray, start, fog_lights, in_front(cloud.light, cloud.transmittance, path));
        light += path.light * f32(sky_samples);
        sky_transmittance = path.transmittance;
    }

    var transmittance = vec3(0.0);
    if geometry_samples > 0u {
        if farthest - nearest <= 0.02 * farthest {
            // Usual case: all geometry samples lie at one distance, so shade it once.
            let distance = total / f32(geometry_samples);
            var path = scene_path(ray, uv, distance, sea_t, pixel);
            if distance >= start {
                path = in_front(cloud.light, cloud.transmittance, path);
            }
            path = fogged(ray, min(min(distance, sea_t), start), fog_lights, path);
            light += path.light * f32(geometry_samples);
            transmittance = path.transmittance * f32(geometry_samples);
        } else {
            // A silhouette: fog changes smoothly between the nearest and farthest samples, so it
            // is integrated at those two and interpolated for the rest. So is the air and water in
            // front, unless the sea's surface or the cloud base lies between them.
            let near = min(min(nearest, sea_t), start);
            let far = min(min(farthest, sea_t), start);
            let near_fog = fog_along(ray, near, fog_lights);
            let far_fog = fog_along(ray, far, fog_lights);
            let smooth_path = (sea_t <= nearest || sea_t >= farthest)
                && (start <= nearest || start > farthest);
            var near_path = Path(vec3(0.0), vec3(1.0));
            var far_path = Path(vec3(0.0), vec3(1.0));
            if smooth_path {
                near_path = scene_path(ray, uv, nearest, sea_t, pixel);
                far_path = scene_path(ray, uv, farthest, sea_t, pixel);
                if nearest >= start {
                    near_path = in_front(cloud.light, cloud.transmittance, near_path);
                    far_path = in_front(cloud.light, cloud.transmittance, far_path);
                }
            }
            for (var i = 0u; i < samples; i += 1u) {
                let distance = distances[i];
                if distance < 0.0 {
                    continue;
                }
                var path: Path;
                if smooth_path {
                    let k = (distance - nearest) / (farthest - nearest);
                    path = Path(mix(near_path.light, far_path.light, k),
                        mix(near_path.transmittance, far_path.transmittance, k));
                } else {
                    path = scene_path(ray, uv, distance, sea_t, pixel);
                    if distance >= start {
                        path = in_front(cloud.light, cloud.transmittance, path);
                    }
                }
                let t = clamp((min(min(distance, sea_t), start) - near) / max(far - near, 1e-3),
                    0.0, 1.0);
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
        min(select(1.0e6, total / f32(max(geometry_samples, 1u)), geometry_samples > 0u), sea_t));
    light = light * shafts.a + shafts.rgb;
    transmittance *= shafts.a;
#ifdef ATMOSPHERE
    // Share of the air glow in front of this pixel: little in front of near surfaces, unless
    // the humid air under crowns lies between.
    let near_air = 1.0 - exp(-total / f32(max(geometry_samples, 1u)) / AIR_GLOW_DEPTH)
        * pow(shafts.a, AIR_GLOW_CANOPY);
    let air = (f32(sky_samples) + f32(geometry_samples) * near_air) / f32(samples);
    light += sun_glare(ray, in.position.xy, air);
    light += lightning_glow(ray, select(1.0e30, farthest, sky_samples == 0u));
#endif

#ifdef DUAL_SOURCE_BLENDING
    return Output(vec4(light, 0.0), vec4(transmittance, 1.0));
#else
    return Output(vec4(light, dot(transmittance, vec3(1.0 / 3.0))));
#endif
}
