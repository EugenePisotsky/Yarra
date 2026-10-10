// Ground haze, valley mist and weather fog along the sky composite's view rays.
import package::shaders::sky::air::{
    scattering, OPEN_DEPTH, OPEN_DENSITY, WISP_LIFT, mist_texel, mist_wisps,
};
import package::shaders::sky::composite_common::{
    environment, mist_map, mist_sampler, noise, noise_sampler, Path, in_front,
};
@if(ATMOSPHERE)
import bevy_pbr::atmosphere::bindings::view;
@else
import package::shaders::sky::composite_common::view;
@if(ATMOSPHERE)
import package::shaders::sky::celestial::clear_sky;

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
    let air = environment.air_light.rgb + environment.air_sun.rgb * scattering(dot(ray, environment.sun.xyz));
    return FogLight(haze_light(ray, air) * view.exposure, air * view.exposure);
}

fn fog_along(ray: vec3<f32>, distance: f32, lights: FogLight) -> Fog {
    let weather = environment.fog.w * distance;
    let haze = haze_depth(ray, distance);
    let mist = mist_depth(ray, distance);
    let depth = weather + haze + mist;
    if depth <= 0.0 {
        return Fog(vec3(0.0), 0.0);
    }
    let light = environment.fog.rgb * view.exposure * weather + lights.haze * haze + lights.mist * mist;
    return Fog(light / depth, depth);
}

// Light that ground haze scatters towards the eye where it is opaque. With the atmosphere it is
// at least the sky just above the horizon below the ray, from Bevy's tables: haze and sky are
// the same air, so distant haze meets the sky without a band at any time of day.
fn haze_light(ray: vec3<f32>, air: vec3<f32>) -> vec3<f32> {
    @if(ATMOSPHERE)
    let flat = select(vec2(1.0, 0.0), normalize(ray.xz), dot(ray.xz, ray.xz) > 1e-6);
    @if(ATMOSPHERE)
    let horizon = normalize(vec3(flat.x, HORIZON_LIFT, flat.y));
    @if(ATMOSPHERE)
    let clear = clear_sky(horizon);
    // Never below the haze's own sun and sky light: towards the sun that is its bright
    // aureole, which light shafts take back where the haze lies in shadow, and at night the
    // authored fill. Under a closed deck the horizon is the deck's grey, not the clear sky's.
    @if(ATMOSPHERE)
    let own = environment.haze.rgb * air;
    @if(ATMOSPHERE)
    return mix(max(clear, own), own, environment.weather.z);
    @else
    return environment.haze.rgb * air;
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

// Ground haze is integrated this far at most: near-horizontal rays to the sky.
const HAZE_RANGE: f32 = 60000.0;

// Optical depth of the ground haze, exponential in height above its base (`haze_depth` in
// `atmosphere::valley_mist`, which the tests check).
fn haze_depth(ray: vec3<f32>, distance: f32) -> f32 {
    let density = environment.low_haze.x;
    if density <= 0.0 {
        return 0.0;
    }
    let length = min(distance, HAZE_RANGE);
    let height = environment.low_haze.z;
    let above = max(view.world_position.y - environment.low_haze.y, 0.0);
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

struct MistColumn {
    top: f32,
    // Depth of mist above the floor, and the share of full density it holds.
    depth: f32,
    density: f32,
}

fn mist_column(xz: vec2<f32>) -> MistColumn {
    let m = mist_texel(mist_map, mist_sampler, environment.mist_map, xz);
    let depth = environment.mist.y * mix(OPEN_DEPTH, 1.0, m.g) * m.a;
    return MistColumn(m.r + depth, depth, mix(OPEN_DENSITY, 1.0, m.g) * m.a);
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
    if environment.mist.x <= 0.0 || environment.mist_map.w < 0.5 {
        return 0.0;
    }
    let origin = view.world_position;
    let length = min(distance, MIST_RANGE);
    let ceiling = max(mist_column(origin.xz).top, mist_column(origin.xz + ray.xz * length).top)
        + environment.mist.y * WISP_LIFT;
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
        let wisps = mist_wisps(noise, noise_sampler, environment.mist_drift.xy, middle);
        let top = column.top + (wisps - 0.5) * 2.0 * WISP_LIFT * column.depth;
        let fade = max(column.depth * 0.6, 3.0);
        let ua = (top - (origin.y + ray.y * start)) / fade;
        let ub = (top - (origin.y + ray.y * (start + stretch))) / fade;
        // Wisps thin the mist to gaps and thicken it into banks.
        depth += column.density * mix(0.1, 1.8, wisps) * mist_share(ua, ub) * stretch;
    }
    return depth * environment.mist.x;
}
