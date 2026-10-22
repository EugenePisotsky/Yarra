// Blowing snow along the sky composite's view rays: snow the wind lifts off snow ground and carries
// along it, densest at the surface and thinning upwards. It blows where the air near the ground
// runs fast enough to lift it, which the gusts carried downwind (weather/gust.wesl) make it do in
// passing sheets, and lies in streaks drawn out along the wind and carried with it. Its light is
// the environment's `drift` (crates/atmosphere/src/environment/sync.rs).
import package::shaders::environment::maps::ground_texel;
import package::shaders::sky::air::mist_texel;
import package::shaders::sky::composite_common::{
    environment, ground_map, mist_map, mist_sampler, noise, noise_sampler,
};
import package::shaders::weather::gust::{gust, wind_frame};
@if(ATMOSPHERE)
import bevy_pbr::atmosphere::bindings::view;
@else
import package::shaders::sky::composite_common::view;

// Air speeds near the ground, metres a second, at which snow starts to blow and blows in full.
const DRIFT_ONSET: f32 = 3.5;
const DRIFT_FULL: f32 = 6.0;

// Extinction per metre at the surface in full drift. Most of it is a drifting layer of this
// e-folding height, metres; over it lies a veil the wind lifts higher (its share at the surface,
// its e-folding height).
const DRIFT_EXTINCTION: f32 = 0.6;
const DRIFT_LAYER: f32 = 0.3;
const DRIFT_VEIL: vec2<f32> = vec2(0.02, 2.0);

// Blowing snow lies within this height of the ground. A ray is integrated over the part of it
// that low, this far at most, in stretches that shorten towards its end nearer the ground.
const DRIFT_TOP: f32 = 10.0;
const DRIFT_RANGE: f32 = 1500.0;
const DRIFT_STEPS: u32 = 6u;

// The streaks: the noise volume's tile spans these metres along the wind, across it and up (whole
// tiles in the gusts' period) for single streaks and for the bundles they run in, and turns over
// this many times while the air travels one period, so streaks form and fade as they go.
const STREAK_TILE: vec3<f32> = vec3(80.0, 8.0, 4.0);
const BUNDLE_TILE: vec3<f32> = vec3(240.0, 24.0, 8.0);
const STREAK_TURNS: f32 = 6.0;

// Snow ground reflects most of the light on it: what tells it from grass, rock and water in the
// ground map.
const SNOW_ALBEDO: vec2<f32> = vec2(0.4, 0.65);

// Optical depth of blowing snow over the first `distance` metres of the ray.
fn drift_depth(ray: vec3<f32>, distance: f32) -> f32 {
    let speed = environment.wind.x;
    let swell = environment.wind.y;
    if environment.drift.w <= 0.0 || environment.mist_map.w < 0.5
        || speed * (1.0 + swell) <= DRIFT_ONSET {
        return 0.0;
    }
    let origin = view.world_position;
    let length = min(distance, DRIFT_RANGE);
    let ceiling = max(ground_at(origin.xz), ground_at(origin.xz + ray.xz * length)) + DRIFT_TOP;
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
    let turn = environment.wind.z * STREAK_TURNS;
    var depth = 0.0;
    for (var i = 0u; i < DRIFT_STEPS; i += 1u) {
        let start = mix(t0, t1, stretch_end(i, ray.y));
        let end = mix(t0, t1, stretch_end(i + 1u, ray.y));
        let middle = origin + ray * (0.5 * (start + end));
        let frame = wind_frame(middle.xz, environment.gusts);
        let blowing = smoothstep(DRIFT_ONSET, DRIFT_FULL, speed * gust(frame, swell));
        if blowing <= 0.0 {
            continue;
        }
        let cover = snow_cover(middle.xz);
        if cover <= 0.0 {
            continue;
        }
        let ground = ground_at(middle.xz);
        let ha = max(origin.y + ray.y * start - ground, 0.0);
        let hb = max(origin.y + ray.y * end - ground, 0.0);
        let streaks = streaks_at(vec3(frame.x, 0.5 * (ha + hb), frame.y), turn);
        let profile = layer_share(ha, hb, DRIFT_LAYER)
            + DRIFT_VEIL.x * layer_share(ha, hb, DRIFT_VEIL.y);
        depth += blowing * cover * streaks * profile * (end - start);
    }
    return depth * DRIFT_EXTINCTION;
}

// How much of the full drift blows at `p` (along the wind, height, across it): thin streaks, the
// noise's fine cells drawn out along the wind, running in bundles of its broad value noise, with
// gaps between them.
fn streaks_at(p: vec3<f32>, turn: f32) -> f32 {
    let lift = vec3(0.0, turn, 0.0);
    let single = textureSampleLevel(noise, noise_sampler, p / STREAK_TILE + lift, 0.0).g;
    let bundle = textureSampleLevel(noise, noise_sampler, p / BUNDLE_TILE + lift, 0.0).r;
    let streak = smoothstep(0.5, 0.85, single) * smoothstep(0.38, 0.6, bundle);
    return 0.05 + 3.0 * streak;
}

// Where stretch `i` of a ray's low part ends, as a share of it: stretches shorten towards its
// start when it climbs and towards its end when it descends, where it runs nearest the ground.
fn stretch_end(i: u32, climb: f32) -> f32 {
    let u = f32(i) / f32(DRIFT_STEPS);
    return select(1.0 - (1.0 - u) * (1.0 - u), u * u, climb >= 0.0);
}

fn ground_at(xz: vec2<f32>) -> f32 {
    return mist_texel(mist_map, mist_sampler, environment.mist_map, xz).b;
}

// 1 on snow ground at render XZ `p`, 0 off it.
fn snow_cover(p: vec2<f32>) -> f32 {
    let albedo = ground_texel(ground_map, mist_sampler, environment.ground_near,
        environment.ground_far, p, vec4(0.0)).rgb;
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
