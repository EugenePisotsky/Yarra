// Everything between the camera and the scene, in three draws over the resolved HDR image, each
// in front of the one before:
// - `sea`: the sea where it lies in front (water/sea.wesl), the open sea out to the horizon and
//   the water over the seabed and the foot of rocks;
// - `air`: the physical sky and aerial perspective from Bevy's atmosphere tables, and the cloud
//   layer;
// - `fog`: ground haze, valley mist, weather fog and blowing snow, then the light shafts and the
//   sun's glare.
// Each draw needs only its own registers: one shader for all of it ran few pixels at a time, and
// the water and fog held back pixels that show neither.
//
// Each draw blends into the image: light is added, and the resolved image is multiplied by the
// transmittance averaged over the samples that hold something. Sky samples hold the black clear
// colour, so they take no part in that average until the sea draws over them. A pixel with no
// geometry at all keeps whatever was drawn at infinity (editor gizmos, for example) behind the
// sky's own transmittance. Each depth sample is classified, so a pixel on a silhouette gets the
// sky's light for its sky samples and the air and fog of their own distance for the others.
//
// Sky and aerial-perspective lookups follow Bevy 0.20.0 `atmosphere/render_sky.wesl` (MIT; see
// third_party/BEVY-MIT.txt) and use its atmosphere shader functions directly.
//
// Its modules: sky/composite_common.wesl (bindings), sky/fog.wesl, sky/cloud_layer.wesl, and with
// the atmosphere sky/celestial.wesl and water/sea.wesl.
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import package::shaders::sky::cloud_layer::{Cloud, cloud_layer};
import package::shaders::sky::composite_common::{
    environment, composite_settings, depth, mist_sampler, shaft_distance, shaft_light, Path, in_front,
    NO_SEA,
};
import package::shaders::sky::fog::{FogLight, fog_light, fogged};
@if(ATMOSPHERE)
import bevy_pbr::atmosphere::{
    bindings::view,
    functions::{get_view_position, sample_aerial_view_lut, sample_transmittance_lut},
};
@else
import package::shaders::sky::composite_common::view;
@if(ATMOSPHERE)
import package::shaders::sky::celestial::{
    AIR_GLOW_CANOPY, AIR_GLOW_DEPTH, MAX_DISC, clear_sky, deck_open, discs, lightning_glow, moon,
    segment_transmittance, stars, sun_glare,
};
@if(ATMOSPHERE)
import package::shaders::water::sea::{sea_surface, through_water};

@if(DUAL_SOURCE_BLENDING)
enable dual_source_blending;

struct Output {
    @if(DUAL_SOURCE_BLENDING)
    @location(0) @blend_src(0) light: vec4<f32>,
    @if(DUAL_SOURCE_BLENDING)
    @location(0) @blend_src(1) transmittance: vec4<f32>,
    @else
    @location(0) light: vec4<f32>,
}

fn output(path: Path) -> Output {
    @if(DUAL_SOURCE_BLENDING)
    return Output(vec4(path.light, 0.0), vec4(path.transmittance, 1.0));
    @else
    return Output(vec4(path.light, dot(path.transmittance, vec3(1.0 / 3.0))));
}

const CLEAR: Path = Path(vec3(0.0), vec3(1.0));

// A main-pass pixel's view ray: its uv in the main pass, NDC and world direction.
struct PixelRay {
    uv: vec2<f32>,
    ndc: vec2<f32>,
    ray: vec3<f32>,
    // Distance along the ray per metre of view depth.
    per_depth: f32,
}

fn pixel_ray(position: vec2<f32>) -> PixelRay {
    let uv = (position - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let ndc = uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
    // Direction from the near plane in view space, as Bevy's sky does: exact far from the origin.
    let near = view.view_from_clip * vec4(ndc, 1.0, 1.0);
    let ray = normalize((view.world_from_view * vec4(near.xyz / near.w, 0.0)).xyz);
    let per_depth = 1.0 / max(dot(ray, -normalize(view.world_from_view[2].xyz)), 1e-4);
    return PixelRay(uv, ndc, ray, per_depth);
}

fn outside_viewport(at: PixelRay) -> bool {
    return any(at.uv < vec2(0.0)) || any(at.uv > vec2(1.0));
}

// The cloud layer along a ray. Clouds are traced only above this elevation; `start` is the
// distance to their base. A closing deck continues down to the horizon at its lowest traced
// elevation, so no band of clear sky shows beneath it.
struct CloudBase {
    above: bool,
    ray: vec3<f32>,
    start: f32,
}

fn cloud_base(ray: vec3<f32>) -> CloudBase {
    let horizon_dip = -sqrt(2.0 * max(view.world_position.y, 0.0) / 6360000.0);
    let above = ray.y > 0.01 || (environment.weather.z > 0.0 && ray.y > horizon_dip);
    let cloud_ray = normalize(vec3(ray.x, max(ray.y, 0.01), ray.z));
    let start = select(1.0e9, max(0.0, (environment.layer.x - view.world_position.y) / cloud_ray.y),
        above);
    return CloudBase(above, cloud_ray, start);
}

// Distance along the ray to the sea's surface, NO_SEA without one.
fn sea_along(ray: vec3<f32>) -> f32 {
    @if(ATMOSPHERE)
    return sea_surface(ray);
    @else
    return NO_SEA;
}

fn depth_samples() -> u32 {
    @if(MULTISAMPLED)
    return min(textureNumSamples(depth), 8u);
    @else
    return 1u;
}

// Distance along the pixel's ray to depth sample `i`, NO_SEA for a sky sample: its view-space
// depth from the rows of view_from_clip that give z and w.
fn sample_distance(at: PixelRay, pixel: vec2<i32>, i: u32) -> f32 {
    let z = textureLoad(depth, pixel, i32(i));
    if z == 0.0 {
        return NO_SEA;
    }
    let m = view.view_from_clip;
    let clip = vec4(at.ndc, z, 1.0);
    return -dot(vec4(m[0].z, m[1].z, m[2].z, m[3].z), clip)
        / dot(vec4(m[0].w, m[1].w, m[2].w, m[3].w), clip) * at.per_depth;
}

// A sample as a draw sees it: the distance it shades the sample at, NO_SEA for one beyond
// everything, or below zero for one it leaves as it is. The sea draw shades the samples below its
// surface and the open sea beyond the land; the others shade every sample that holds something at
// its own distance or the sea's surface in front, and the sky beyond.
fn classified(distance: f32, sea_t: f32, seabed: bool) -> f32 {
    if !seabed {
        return min(distance, sea_t);
    }
    return select(-1.0, distance, distance >= NO_SEA || distance > sea_t);
}

// The nearest, farthest and summed distances of the samples a draw shades by distance, and how
// many there are.
struct Spread {
    nearest: f32,
    farthest: f32,
    total: f32,
    count: u32,
}

fn mean(spread: Spread) -> f32 {
    return spread.total / f32(max(spread.count, 1u));
}

// All of a spread's samples lie at one distance, near enough to shade once at their mean.
fn one_distance(spread: Spread) -> bool {
    return spread.farthest - spread.nearest <= 0.02 * spread.farthest;
}

// A pixel's samples as a draw sees them: those it shades by distance, how many lie beyond them all,
// and how many hold geometry.
struct Samples {
    held: Spread,
    beyond: u32,
    geometry: u32,
    all: u32,
}

fn samples_of(at: PixelRay, pixel: vec2<i32>, sea_t: f32, seabed: bool) -> Samples {
    var s = Samples(Spread(1.0e30, 0.0, 0.0, 0u), 0u, 0u, depth_samples());
    for (var i = 0u; i < s.all; i += 1u) {
        let found = sample_distance(at, pixel, i);
        s.geometry += u32(found < NO_SEA);
        let distance = classified(found, sea_t, seabed);
        if distance >= NO_SEA {
            s.beyond += 1u;
        } else if distance >= 0.0 {
            s.held = Spread(min(s.held.nearest, distance), max(s.held.farthest, distance),
                s.held.total + distance, s.held.count + 1u);
        }
    }
    return s;
}

// What a draw adds in front of a pixel's samples, worked out at three distances at most: the
// nearest and farthest of those it shades by distance (once at their mean when they agree), and
// beyond them all. Each draw works the paths out with one call site in a loop over `end`: the
// air, water and fog are most of their code, and each inlined copy took registers.
struct Ends {
    near: Path,
    far: Path,
    beyond: Path,
}

// The distance to work out `end` at, below zero when no sample needs it.
fn end_distance(end: u32, s: Samples) -> f32 {
    let held = s.held.count > 0u;
    if end == 0u {
        return select(-1.0, select(s.held.nearest, mean(s.held), one_distance(s.held)), held);
    }
    if end == 1u {
        return select(-1.0, s.held.farthest, held && !one_distance(s.held));
    }
    return select(-1.0, NO_SEA, s.beyond > 0u);
}

fn with_end(ends: Ends, end: u32, path: Path) -> Ends {
    var next = ends;
    if end == 0u {
        next.near = path;
    } else if end == 1u {
        next.far = path;
    } else {
        next.beyond = path;
    }
    return next;
}

// The share of the farthest sample's path a sample `distance` metres away takes on a silhouette:
// by its distance between the nearest and farthest (each within `clip`, past which the path no
// longer changes), as air and fog change smoothly with it; where `step` lies between them (where
// the path changes at once), by its side of it.
fn far_share(distance: f32, spread: Spread, step: f32, clip: f32) -> f32 {
    let near = min(spread.nearest, clip);
    let far = min(spread.farthest, clip);
    if step > near && step <= far {
        return select(0.0, 1.0, distance >= step);
    }
    return clamp((min(distance, clip) - near) / max(far - near, 1e-3), 0.0, 1.0);
}

// The light a draw adds to a pixel and the transmittance of what lies behind, from its ends:
// light over all samples, transmittance over those that hold something (1 for those it leaves).
// Samples beyond everything hold something once an earlier draw lit them (`lit_beyond`): the fog
// lies in front of the sky the air draw added.
fn resolved(ends: Ends, s: Samples, at: PixelRay, pixel: vec2<i32>, sea_t: f32, seabed: bool,
    step: f32, clip: f32, lit_beyond: bool) -> Path {
    var light = ends.beyond.light * f32(s.beyond);
    var transmittance = vec3(f32(s.geometry - min(s.held.count, s.geometry)));
    var covered = max(s.geometry, s.held.count);
    if lit_beyond {
        transmittance += ends.beyond.transmittance * f32(s.beyond);
        covered += s.beyond;
    }
    if s.held.count > 0u && one_distance(s.held) {
        light += ends.near.light * f32(s.held.count);
        transmittance += ends.near.transmittance * f32(s.held.count);
    } else if s.held.count > 0u {
        for (var i = 0u; i < s.all; i += 1u) {
            let distance = classified(sample_distance(at, pixel, i), sea_t, seabed);
            if distance < 0.0 || distance >= NO_SEA {
                continue;
            }
            let k = far_share(distance, s.held, step, clip);
            light += mix(ends.near.light, ends.far.light, k);
            transmittance += mix(ends.near.transmittance, ends.far.transmittance, k);
        }
    }
    return Path(light / f32(s.all),
        select(ends.beyond.transmittance, transmittance / f32(max(covered, 1u)), covered > 0u));
}

// The sea in front of a pixel's samples: its water's light, and the water's transmittance over
// what the main pass drew below its surface.
@if(ATMOSPHERE)
@fragment
fn sea(in: FullscreenVertexOutput) -> Output {
    let at = pixel_ray(in.position.xy);
    if outside_viewport(at) {
        discard;
    }
    let sea_t = sea_surface(at.ray);
    if sea_t >= NO_SEA {
        discard;
    }
    let pixel = vec2<i32>(in.position.xy);
    let s = samples_of(at, pixel, sea_t, true);
    if s.held.count == 0u && s.beyond == 0u {
        discard;
    }

    var ends = Ends(CLEAR, CLEAR, CLEAR);
    for (var end = 0u; end < 3u; end += 1u) {
        let distance = end_distance(end, s);
        if distance < 0.0 {
            continue;
        }
        ends = with_end(ends, end, through_water(CLEAR, at.ray, sea_t, distance, pixel));
    }
    return output(resolved(ends, s, at, pixel, sea_t, true, NO_SEA, NO_SEA, false));
}

// Air between the camera and a surface `distance` metres along the ray. The atmosphere entity
// has unit scale, so world distance is also atmosphere distance.
fn aerial_perspective(ray: vec3<f32>, uv: vec2<f32>, distance: f32) -> Path {
    @if(ATMOSPHERE)
    let position = get_view_position();
    @if(ATMOSPHERE)
    let r = length(position);
    @if(ATMOSPHERE)
    let mu = dot(ray, normalize(position));
    @if(ATMOSPHERE)
    return Path(sample_aerial_view_lut(uv, distance) * view.exposure,
        segment_transmittance(r, mu, distance));
    @else
    return CLEAR;
}

// The sky and the air in front of the sea's surface or whatever the main pass drew, and the cloud
// layer where it lies in front.
@fragment
fn air(in: FullscreenVertexOutput) -> Output {
    let at = pixel_ray(in.position.xy);
    let ray = at.ray;
    // The sun and moon disks use derivatives, so evaluate them before any per-pixel branch.
    @if(ATMOSPHERE)
    let disks = discs(ray);
    @if(ATMOSPHERE)
    let moon_disc = moon(ray);
    if outside_viewport(at) {
        discard;
    }
    let base = cloud_base(ray);
    // Without the atmosphere only the clouds are drawn here.
    @if(!ATMOSPHERE)
    if !base.above {
        discard;
    }
    let sea_t = sea_along(ray);
    let pixel = vec2<i32>(in.position.xy);
    let s = samples_of(at, pixel, sea_t, false);

    // One cloud lookup serves every sample that can see the cloud base.
    var cloud = Cloud(vec3(0.0), 1.0);
    @if(CLOUDS)
    if base.above && (s.beyond > 0u || s.held.farthest >= base.start) {
        cloud = cloud_layer(base.ray, base.start, in.uv, true);
    }

    // The sky behind sky samples, its clouds in front.
    var sky = CLEAR;
    if s.beyond > 0u {
        @if(ATMOSPHERE)
        let position = get_view_position();
        @if(ATMOSPHERE)
        let r = length(position);
        @if(ATMOSPHERE)
        let transmittance = sample_transmittance_lut(r, dot(ray, normalize(position)));
        @if(ATMOSPHERE)
        let clear = clear_sky(ray);
        @if(ATMOSPHERE)
        let behind = Path(clear * view.exposure
            + min((disks + moon_disc.rgb) * transmittance * view.exposure * deck_open(),
                vec3(MAX_DISC))
            + stars(ray) * transmittance * (1.0 - moon_disc.a), transmittance);
        @else
        let behind = CLEAR;
        sky = in_front(cloud.light, cloud.transmittance, behind);
    }

    var ends = Ends(sky, sky, sky);
    for (var end = 0u; end < 2u; end += 1u) {
        let distance = end_distance(end, s);
        if distance < 0.0 {
            continue;
        }
        var path = aerial_perspective(ray, at.uv, distance);
        if distance >= base.start {
            path = in_front(cloud.light, cloud.transmittance, path);
        }
        ends = with_end(ends, end, path);
    }
    return output(resolved(ends, s, at, pixel, sea_t, false, base.start, NO_SEA, false));
}

// Sky samples' distance as the light shafts store it, within half floats.
const SHAFT_SKY: f32 = 6.0e4;

// Light shafts at a pixel `distance` metres deep: the four nearest texels, bilinear but skipping
// those at a different distance, so beams stop at silhouettes. Where all four lie at the pixel's
// distance (nearly everywhere) the plain bilinear sample is the same; the four distances come
// in one gather. Sky pixels use 1e6.
fn light_shafts(position: vec2<f32>, distance: f32) -> vec4<f32> {
    if composite_settings.flags.y < 0.5 {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    let uv = (position - view.main_pass_viewport.xy) / composite_settings.flags.z
        / vec2<f32>(textureDimensions(shaft_light));
    let here = min(distance, SHAFT_SKY);
    // Texels (0, 0), (1, 0), (0, 1), (1, 1) of the footprint, as gather orders them w, z, x, y.
    let gathered = textureGather(0, shaft_distance, mist_sampler, uv);
    // Fetched beside the distances rather than after them: the pass waits for one fetch, not two.
    let bilinear = textureSampleLevel(shaft_light, mist_sampler, uv, 0.0);
    let around = vec4(gathered.w, gathered.z, gathered.x, gathered.y);
    let falloff = 0.05 * min(around, vec4(here)) + 0.3;
    let gap = abs(around - here);
    // All four within half the falloff: the weights differ by too little to show.
    if all(gap < 0.5 * falloff) {
        return bilinear;
    }
    let size = vec2<i32>(textureDimensions(shaft_light));
    let q = (position - view.main_pass_viewport.xy) / composite_settings.flags.z - 0.5;
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

// The fog in front of everything else, then the light shafts and the sun's glare in front of it.
@fragment
fn fog(in: FullscreenVertexOutput) -> Output {
    let at = pixel_ray(in.position.xy);
    let ray = at.ray;
    if outside_viewport(at) {
        discard;
    }
    let fog = environment.fog.w > 0.0 || environment.low_haze.x > 0.0 || environment.mist.x > 0.0
        || environment.drift.w > 0.0;
    let base = cloud_base(ray);
    // Without the atmosphere, ground below the horizon only changes in fog.
    @if(!ATMOSPHERE)
    if !base.above && !fog {
        discard;
    }
    let sea_t = sea_along(ray);
    let pixel = vec2<i32>(in.position.xy);
    let s = samples_of(at, pixel, sea_t, false);

    // Fog lies over the ray up to the cloud base at most; its light depends on the ray's direction
    // alone.
    var ends = Ends(CLEAR, CLEAR, CLEAR);
    if fog {
        let lights = fog_light(ray);
        for (var end = 0u; end < 3u; end += 1u) {
            let distance = end_distance(end, s);
            if distance < 0.0 {
                continue;
            }
            ends = with_end(ends, end, fogged(ray, min(distance, base.start), lights, CLEAR));
        }
    }
    var path = resolved(ends, s, at, pixel, sea_t, false, NO_SEA, base.start, true);

    // Light shafts lie in front of everything else the composite adds.
    let held = s.held.count > 0u;
    let shafts = light_shafts(in.position.xy, select(1.0e6, mean(s.held), held));
    path = Path(path.light * shafts.a + shafts.rgb, path.transmittance * shafts.a);
    // Share of the air glow in front of this pixel: little in front of near surfaces, unless
    // the humid air under crowns lies between.
    @if(ATMOSPHERE)
    let near_air = 1.0 - exp(-mean(s.held) / AIR_GLOW_DEPTH) * pow(shafts.a, AIR_GLOW_CANOPY);
    @if(ATMOSPHERE)
    let air = (f32(s.beyond) + f32(s.held.count) * near_air) / f32(s.all);
    @if(ATMOSPHERE)
    path.light += sun_glare(ray, in.position.xy, air);
    @if(ATMOSPHERE)
    path.light += lightning_glow(ray, select(1.0e30, s.held.farthest, s.beyond == 0u));
    return output(path);
}
