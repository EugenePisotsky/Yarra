// Blowing snow along the sky composite's view rays: snow the wind lifts off snow ground and carries
// along it, densest at the surface and thinning upwards. It blows where the air near the ground
// runs fast enough to lift it, thickest where the gusts carried downwind (weather/gust.wesl)
// pass, and flows in broad sheets carried with the wind. It is the ground's snow lifted, lit by
// the sky above and by that snow below, and by the sun.
import package::shaders::environment::maps::{ground_relief, ground_texel};
import package::shaders::sky::air::scattering;
import package::shaders::sky::composite_common::{
    environment, ground_map, mist_sampler, noise, noise_sampler,
};
import package::shaders::weather::gust::{gust, wind_frame};
@if(ATMOSPHERE)
import bevy_pbr::atmosphere::bindings::view;
@else
import package::shaders::sky::composite_common::view;

// Air speeds near the ground, metres a second, at which snow starts to blow and blows in full.
const DRIFT_ONSET: f32 = 3.0;
const DRIFT_FULL: f32 = 6.5;

// Extinction per metre at the surface in full drift. Most of it is a drifting layer of this
// e-folding height, metres; over it lies a veil the wind lifts higher (its share at the surface,
// its e-folding height).
const DRIFT_EXTINCTION: f32 = 0.5;
const DRIFT_LAYER: f32 = 0.4;
const DRIFT_VEIL: vec2<f32> = vec2(0.03, 2.0);

// Blowing snow lies within this height of the ground. A ray is integrated over the part of it
// that low in stretches cut at the same distances from the camera on every ray, so pixels at
// different depths agree up to the nearer one: the density varies smoothly, and the height over
// the ground is exact between the ground at each stretch's ends. Past the last cut the weather
// that blows snow hides the land anyway.
const DRIFT_TOP: f32 = 10.0;
const DRIFT_CUTS: u32 = 8u;

// The sheets: the noise volume's tile spans these metres along the wind and across it (whole
// tiles in the gusts' period) and turns over this many times while the air travels one period,
// so the sheets change as they go. Between them the drift thins to this share, never clearing.
const SHEET_TILE: vec2<f32> = vec2(120.0, 40.0);
const SHEET_TURNS: f32 = 6.0;
const SHEET_THIN: f32 = 0.3;

// Lit materials reflect about this share of the ambient light at full roughness, at the grazing
// angles open ground is seen from (Bevy's ambient_light: EnvBRDFApprox with the DFG table's
// F_AB(1, NdotV)), so the snow ground does; blowing snow takes the sky's light as the ground
// does, to match it.
const AMBIENT_DIFFUSE: f32 = 0.6;

// Snow ground reflects most of the light on it: what tells it from grass, rock and water in the
// ground map.
const SNOW_ALBEDO: vec2<f32> = vec2(0.4, 0.65);

// Blowing snow over part of a ray: its optical depth, and the unexposed light it scatters
// towards the eye, summed by optical depth.
struct Drift {
    light: vec3<f32>,
    depth: f32,
}

// Blowing snow over the first `distance` metres of the ray.
fn drift_along(ray: vec3<f32>, distance: f32) -> Drift {
    let speed = environment.wind.x;
    let swell = environment.wind.y;
    if environment.drift.w <= 0.0 || environment.ground_near.z <= 0.0
        || speed * (1.0 + swell) <= DRIFT_ONSET {
        return Drift(vec3(0.0), 0.0);
    }
    var cuts = array<f32, DRIFT_CUTS>(0.0, 3.0, 8.0, 20.0, 50.0, 120.0, 250.0, 500.0);
    let origin = view.world_position;
    let length = min(distance, cuts[DRIFT_CUTS - 1u]);
    let ceiling = max(ground_at(origin.xz).x, ground_at(origin.xz + ray.xz * length).x) + DRIFT_TOP;
    var t0 = 0.0;
    var t1 = length;
    if ray.y > 1e-5 {
        t1 = min(t1, (ceiling - origin.y) / ray.y);
    } else if ray.y < -1e-5 {
        t0 = max(t0, (origin.y - ceiling) / -ray.y);
    } else if origin.y >= ceiling {
        return Drift(vec3(0.0), 0.0);
    }
    if t1 <= t0 {
        return Drift(vec3(0.0), 0.0);
    }
    let sun = sunlight();
    let towards_eye = sun * scattering(dot(ray, environment.sun.xyz));
    let turn = environment.wind.z * SHEET_TURNS;
    var start_ground = ground_at(origin.xz + ray.xz * t0).x;
    var drift = Drift(vec3(0.0), 0.0);
    for (var i = 0u; i + 1u < DRIFT_CUTS; i += 1u) {
        let start = max(cuts[i], t0);
        let end = min(cuts[i + 1u], t1);
        if start >= t1 {
            break;
        }
        if end <= start {
            continue;
        }
        let stretch = end - start;
        let relief = ground_at(origin.xz + ray.xz * end);
        let ha = max(origin.y + ray.y * start - start_ground, 0.0);
        let hb = max(origin.y + ray.y * end - relief.x, 0.0);
        start_ground = relief.x;
        let middle = (origin + ray * (0.5 * (start + end))).xz;
        let frame = wind_frame(middle, environment.gusts);
        let blowing = smoothstep(DRIFT_ONSET, DRIFT_FULL, speed * gust(frame, swell));
        if blowing <= 0.0 {
            continue;
        }
        let below = ground_texel(ground_map, mist_sampler, environment.ground_near,
            environment.ground_far, middle, vec4(0.0));
        let profile = layer_share(ha, hb, DRIFT_LAYER)
            + DRIFT_VEIL.x * layer_share(ha, hb, DRIFT_VEIL.y);
        let depth = DRIFT_EXTINCTION * blowing * snow_cover(below.rgb) * sheets(frame, turn)
            * profile * stretch;
        let lights = SnowLights(environment.drift.rgb * AMBIENT_DIFFUSE * relief.y, sun * below.a,
            towards_eye);
        drift.light += snow_light(lights, below.rgb) * depth;
        drift.depth += depth;
    }
    return drift;
}

// The light reaching blowing snow: the sky's as lit materials take it past the terrain around,
// the sun's on the ground below past the terrain's horizon (the ground map's cosine), and the
// sun's scattered towards the eye along the ray.
struct SnowLights {
    sky: vec3<f32>,
    sun_on_ground: vec3<f32>,
    sun_towards_eye: vec3<f32>,
}

// The sun's light on open ground past a closing cloud deck, as air_light lets it through.
fn sunlight() -> vec3<f32> {
    if environment.sun.y <= 0.0 {
        return vec3(0.0);
    }
    return environment.near_sun.rgb * (1.0 - environment.weather.z);
}

// Light grains of snow scatter towards the eye over ground of `albedo`: half of what they see is
// the sky, half the ground below reflecting the sky and sun, so over snow they show as bright as
// the snow or a shade brighter, in its colour; and the sun's own light, scattered as by haze.
fn snow_light(lights: SnowLights, albedo: vec3<f32>) -> vec3<f32> {
    let ground = albedo * (lights.sky + lights.sun_on_ground / 3.14159265);
    return 0.5 * (lights.sky + ground) + lights.sun_towards_eye;
}

// How much of the full drift blows at `frame` (along the wind, across it): broad sheets drawn out
// along the wind, thinning between them.
fn sheets(frame: vec2<f32>, turn: f32) -> f32 {
    let q = vec3(frame.x / SHEET_TILE.x, turn, frame.y / SHEET_TILE.y);
    let n = textureSampleLevel(noise, noise_sampler, q, 0.0).r;
    return mix(SHEET_THIN, 1.0, smoothstep(0.3, 0.7, n));
}

// The ground at render XZ `xz`: the top of its snow, and the share of the sky the terrain around
// leaves it. Beyond the map no snow blows.
fn ground_at(xz: vec2<f32>) -> vec2<f32> {
    return ground_relief(ground_map, mist_sampler, environment.ground_near, environment.ground_far,
        xz, environment.ground_near.w);
}

// 1 on snow ground of `albedo`, 0 off it.
fn snow_cover(albedo: vec3<f32>) -> f32 {
    return smoothstep(SNOW_ALBEDO.x, SNOW_ALBEDO.y, dot(albedo, vec3(0.2126, 0.7152, 0.0722)));
}

// Mean of exp(-h / height) along a straight stretch from height `ha` to `hb`.
fn layer_share(ha: f32, hb: f32, height: f32) -> f32 {
    let rise = hb - ha;
    if abs(rise) < 1e-3 * height {
        return exp(-0.5 * (ha + hb) / height);
    }
    return height * (exp(-ha / height) - exp(-hb / height)) / rise;
}
