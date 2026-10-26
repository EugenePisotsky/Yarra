// Blowing snow along the sky composite's view rays: snow the wind lifts off snow ground and carries
// along it, densest at the surface and thinning upwards. It blows where the air near the ground
// runs fast enough to lift it, thickest where the gusts carried downwind (weather/gust.wesl)
// pass, and flows in broad sheets carried with the wind, over the bare patches of snow country
// as well. It is the ground's snow lifted, lit by
// the sky above and by that snow below, and by the sun.
//
// Where and how thick it blows, and the light it takes from the sky, the ground and the sun,
// depend on the ground alone, so `fill` works them out once a frame over the ground map's texels
// (crates/atmosphere/src/drift.rs), and `drift_along` integrates each view ray through that
// field, exactly in its height over the ground.
import package::shaders::environment::maps::{
    FRESH_SNOW, FROST_MEAN, ground_layers, ground_relief, ground_texel, snow_country,
};
import package::shaders::sky::air::scattering;
import package::shaders::sky::composite_common::{
    environment, drift_field, ground_map, mist_sampler, noise, noise_sampler,
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
const DRIFT_CUT: array<f32, DRIFT_CUTS> = array(0.0, 3.0, 8.0, 20.0, 50.0, 120.0, 250.0, 500.0);

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

const PI: f32 = 3.14159265;

// The field `fill` writes: for each of the ground map's texels, the light blowing snow there
// scatters towards the eye from the sky, the ground and the sun past the ground, over
// `light_scale`, and its extinction per metre at the surface in its sheets. The composite reads
// it as `drift_field`. The other maps are bound at the composite's slots, so the functions here
// read them alike in both passes.
@group(1) @binding(19) var field_out: texture_storage_2d_array<rgba16float, write>;

// Blowing snow over part of a ray: its optical depth, and the unexposed light it scatters
// towards the eye, summed by optical depth.
struct Drift {
    light: vec3<f32>,
    depth: f32,
}

// Whether snow can blow at all this frame: snow ground, a map of it, and wind enough somewhere.
fn drifting() -> bool {
    return environment.drift.w > 0.0 && environment.ground_near.z > 0.0
        && environment.wind.x * (1.0 + environment.wind.y) > DRIFT_ONSET;
}

// Blowing snow over the first `distance` metres of the ray.
fn drift_along(ray: vec3<f32>, distance: f32) -> Drift {
    if !drifting() {
        return Drift(vec3(0.0), 0.0);
    }
    let origin = view.world_position;
    let length = min(distance, DRIFT_CUT[DRIFT_CUTS - 1u]);
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
    let scale = light_scale();
    let towards_eye = sunlight() * scattering(dot(ray, environment.sun.xyz));
    var start_ground = ground_at(origin.xz + ray.xz * t0).x;
    var drift = Drift(vec3(0.0), 0.0);
    for (var i = 0u; i + 1u < DRIFT_CUTS; i += 1u) {
        let start = max(DRIFT_CUT[i], t0);
        let end = min(DRIFT_CUT[i + 1u], t1);
        if start >= t1 {
            break;
        }
        if end <= start {
            continue;
        }
        let stretch = end - start;
        let end_ground = ground_at(origin.xz + ray.xz * end).x;
        let ha = max(origin.y + ray.y * start - start_ground, 0.0);
        let hb = max(origin.y + ray.y * end - end_ground, 0.0);
        start_ground = end_ground;
        let field = field_at((origin + ray * (0.5 * (start + end))).xz);
        if field.a <= 0.0 {
            continue;
        }
        let profile = layer_share(ha, hb, DRIFT_LAYER)
            + DRIFT_VEIL.x * layer_share(ha, hb, DRIFT_VEIL.y);
        let depth = field.a * profile * stretch;
        drift.light += (field.rgb * scale + towards_eye) * depth;
        drift.depth += depth;
    }
    return drift;
}

// The field at render XZ `xz`, its near layer over the far one as in the ground map.
fn field_at(xz: vec2<f32>) -> vec4<f32> {
    return ground_layers(drift_field, mist_sampler, environment.ground_near,
        environment.ground_far, xz, 0u, vec4(0.0));
}

// One texel of the field for each of the ground map's: its layer and texel.
@compute @workgroup_size(8, 8, 1)
fn fill(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(field_out)) {
        return;
    }
    let placement = select(environment.ground_far, environment.ground_near, id.z == 0u);
    let xz = placement.xy + (vec2<f32>(id.xy) + 0.5) * placement.z;
    textureStore(field_out, id.xy, id.z, blowing_at(xz));
}

// The field's value at render XZ `xz`. The light is what snow grains there scatter towards the
// eye over the ground below: half of what they see is the sky, half the ground reflecting the
// sky and the sun, so over snow they show as bright as the snow or a shade brighter, in its
// colour. The sky's light reaches them as lit materials take it past the terrain around, and the
// sun's ground as the ground map's cosine past the terrain's horizon. The sun's own light
// scattered towards the eye depends on the ray, so `drift_along` adds it.
fn blowing_at(xz: vec2<f32>) -> vec4<f32> {
    if !drifting() {
        return vec4(0.0);
    }
    let frame = wind_frame(xz, environment.gusts);
    let blowing = smoothstep(DRIFT_ONSET, DRIFT_FULL, environment.wind.x
        * gust(frame, environment.wind.y));
    if blowing <= 0.0 {
        return vec4(0.0);
    }
    let below = ground_texel(ground_map, mist_sampler, environment.ground_near,
        environment.ground_far, xz, vec4(0.0));
    let country = snow_country(ground_map, mist_sampler, environment.ground_near,
        environment.ground_far, xz);
    let cover = max(snow_cover(below.rgb), country);
    let extinction = DRIFT_EXTINCTION * blowing * cover
        * sheets(frame, environment.wind.z * SHEET_TURNS);
    let sky = environment.drift.rgb * AMBIENT_DIFFUSE * ground_at(xz).y;
    let albedo = mix(below.rgb, FRESH_SNOW, FROST_MEAN * environment.weather.w * country);
    let ground = albedo * (sky + sunlight() * below.a / PI);
    return vec4(0.5 * (sky + ground) / light_scale(), extinction);
}

// The most light the field can hold, per channel, so half floats keep it near 1 whatever the sun
// and sky: the whole sky's share and the sun's off the brightest ground.
fn light_scale() -> vec3<f32> {
    return max(environment.drift.rgb * AMBIENT_DIFFUSE + 0.5 * sunlight() / PI, vec3(1e-4));
}

// The sun's light on open ground past a closing cloud deck, as air_light lets it through.
fn sunlight() -> vec3<f32> {
    if environment.sun.y <= 0.0 {
        return vec3(0.0);
    }
    return environment.near_sun.rgb * (1.0 - environment.weather.z);
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
