// Everything between the camera and the scene, in one pass over the resolved HDR image: the
// physical sky and aerial perspective from Bevy's atmosphere tables, the cloud layer, ground
// haze, valley mist and weather fog. Each depth sample is classified, so a pixel on a silhouette
// gets the sky's light for its sky samples and the haze of its own distance for the geometry
// samples.
//
// The pass blends into the image: light is added, and the resolved scene is multiplied by the
// transmittance averaged over geometry samples. Sky samples hold the black clear colour, so they
// take no part in that average. A pixel with no geometry at all keeps whatever was drawn at
// infinity (editor gizmos, for example) behind the sky's own transmittance.
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
import package::shaders::sky::fog::{Fog, FogLight, behind_fog, fog_along, fog_light, fogged};
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
    return Path(vec3(0.0), vec3(1.0));
}

// Air, then water where the sea lies in front, between the eye and a sample `distance` away.
fn scene_path(ray: vec3<f32>, uv: vec2<f32>, distance: f32, sea_t: f32, pixel: vec2<i32>)
    -> Path {
    var path = aerial_perspective(ray, uv, min(distance, sea_t));
    @if(ATMOSPHERE)
    if sea_t < distance {
        path = through_water(path, ray, sea_t, distance, pixel);
    }
    return path;
}

// Sky samples' distance as the light shafts store it, within half floats.
const SHAFT_SKY: f32 = 6.0e4;

// Light shafts at a pixel `distance` metres deep: the four nearest texels, bilinear but skipping
// those at a different distance, so beams stop at silhouettes. Where all four lie at the pixel's
// distance (nearly everywhere) the plain bilinear sample is the same; the four distances come
// in one gather. Sky pixels use 1e6.
fn light_shafts(position: vec2<f32>, distance: f32) -> vec4<f32> {
    if composite_settings.y < 0.5 {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    let uv = (position - view.main_pass_viewport.xy) / composite_settings.z
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
    let q = (position - view.main_pass_viewport.xy) / composite_settings.z - 0.5;
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
    // The sun and moon disks use derivatives, so evaluate them before any per-pixel branch.
    @if(ATMOSPHERE)
    let disks = discs(ray);
    @if(ATMOSPHERE)
    let moon_disc = moon(ray);
    if any(uv < vec2(0.0)) || any(uv > vec2(1.0)) {
        discard;
    }
    let fog = environment.fog.w > 0.0 || environment.low_haze.x > 0.0 || environment.mist.x > 0.0;
    // Clouds are traced only above this elevation; `start` is the distance to their base.
    // A closing deck continues down to the horizon at its lowest traced elevation, so no band of
    // clear sky shows beneath it.
    let horizon_dip = -sqrt(2.0 * max(view.world_position.y, 0.0) / 6360000.0);
    let above = ray.y > 0.01 || (environment.weather.z > 0.0 && ray.y > horizon_dip);
    let cloud_ray = normalize(vec3(ray.x, max(ray.y, 0.01), ray.z));
    let start = select(1.0e9, max(0.0, (environment.layer.x - view.world_position.y) / cloud_ray.y),
        above);
    // Without the atmosphere, ground below the horizon only changes in fog.
    @if(!ATMOSPHERE)
    if !above && !fog {
        discard;
    }

    let pixel = vec2<i32>(in.position.xy);
    @if(MULTISAMPLED)
    let samples = min(textureNumSamples(depth), 8u);
    @else
    let samples = 1u;
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
    @if(ATMOSPHERE)
    let sea_t = sea_surface(ray);
    @else
    let sea_t = NO_SEA;

    // The fog's light, once for every sample.
    var fog_lights = FogLight(vec3(0.0), vec3(0.0));
    if fog {
        fog_lights = fog_light(ray);
    }

    // One cloud lookup serves every sample that can see the cloud base.
    var cloud = Cloud(vec3(0.0), 1.0);
    @if(CLOUDS)
    if above && (sky_samples > 0u || farthest >= start) {
        cloud = cloud_layer(cloud_ray, start, in.uv, true);
    }

    var light = vec3(0.0);
    var sky_transmittance = vec3(1.0);
    if sky_samples > 0u && sea_t < NO_SEA {
        // Open sea out to the horizon, beyond the world's ground.
        let path = fogged(ray, min(sea_t, start), fog_lights,
            scene_path(ray, uv, NO_SEA, sea_t, pixel));
        light += path.light * f32(sky_samples);
        sky_transmittance = path.transmittance;
    } else if sky_samples > 0u {
        @if(ATMOSPHERE)
        let position = get_view_position();
        @if(ATMOSPHERE)
        let r = length(position);
        @if(ATMOSPHERE)
        let transmittance = sample_transmittance_lut(r, dot(ray, normalize(position)));
        @if(ATMOSPHERE)
        let sky = clear_sky(ray);
        @if(ATMOSPHERE)
        var path = Path(sky * view.exposure
            + min((disks + moon_disc.rgb) * transmittance * view.exposure * deck_open(),
                vec3(MAX_DISC))
            + stars(ray) * transmittance * (1.0 - moon_disc.a), transmittance);
        @else
        var path = Path(vec3(0.0), vec3(1.0));
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
    // Share of the air glow in front of this pixel: little in front of near surfaces, unless
    // the humid air under crowns lies between.
    @if(ATMOSPHERE)
    let near_air = 1.0 - exp(-total / f32(max(geometry_samples, 1u)) / AIR_GLOW_DEPTH)
        * pow(shafts.a, AIR_GLOW_CANOPY);
    @if(ATMOSPHERE)
    let air = (f32(sky_samples) + f32(geometry_samples) * near_air) / f32(samples);
    @if(ATMOSPHERE)
    light += sun_glare(ray, in.position.xy, air);
    @if(ATMOSPHERE)
    light += lightning_glow(ray, select(1.0e30, farthest, sky_samples == 0u));

    @if(DUAL_SOURCE_BLENDING)
    return Output(vec4(light, 0.0), vec4(transmittance, 1.0));
    @else
    return Output(vec4(light, dot(transmittance, vec3(1.0 / 3.0))));
}
