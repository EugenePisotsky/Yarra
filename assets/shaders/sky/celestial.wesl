// The sky with Bevy's atmosphere, for the sky composite: its light, the sun and moon discs,
// stars, the sun's glare, the lightning channel and the transmittance of the air in front of a
// surface. Sky lookups follow Bevy 0.20.0 `atmosphere/render_sky.wesl` (MIT; see
// third_party/BEVY-MIT.txt).
import bevy_pbr::atmosphere::{
    bindings::{lights, view},
    bruneton_functions::ray_intersects_ground,
    functions::{
        direction_world_to_atmosphere, get_local_r, get_view_position, sample_density_lut,
        sample_sky_view_lut, sample_transmittance_lut,
    },
};
import package::shaders::sky::composite_common::{
    clouds, composite_settings, mist_sampler, noise, noise_sampler, sun_rays, sun_state,
    LIGHTNING_COLOR,
};
import package::shaders::sky::sun::{
    AIR_GLOW_SHARE, AIR_GLOW_WIDTH, GLARE_CORE_SHARE, GLARE_CORE_WIDTH, GLARE_VEIL_SHARE,
    GLARE_VEIL_WIDTH,
};

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
    let drawn = composite_settings.w > 0.5;
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

// Sun rays at a pixel, bilinear between the four nearest texels.
fn sun_rays_at(position: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(sun_rays));
    let uv = (position - view.main_pass_viewport.xy) / composite_settings.z / size;
    return textureSampleLevel(sun_rays, mist_sampler, uv, 0.0).r;
}

// The lightning channel's exposed brightness at its core, far past white so bloom spreads it.
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
