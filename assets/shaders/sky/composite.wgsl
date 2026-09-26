// Everything between the camera and the scene, in one pass over the resolved HDR image: the
// physical sky and aerial perspective from Bevy's atmosphere tables, the cloud layer and weather
// fog. Each depth sample is classified, so a pixel on a silhouette gets the sky's light for its
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
    bindings::view,
    functions::{
        direction_world_to_atmosphere, get_view_position, sample_aerial_view_lut,
        sample_sky_view_lut, sample_sun_radiance, sample_transmittance_lut,
        sample_transmittance_lut_segment,
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
// Cloud images: the newest and previous complete cache refreshes, and x = cross-fade progress.
@group(1) @binding(2) var cloud_newer: texture_2d<f32>;
@group(1) @binding(3) var cloud_older: texture_2d<f32>;
@group(1) @binding(4) var cloud_sampler: sampler;
@group(1) @binding(5) var<uniform> cloud_blend: vec4<f32>;

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

// Weather fog over the first `distance` metres of the ray.
fn fogged(distance: f32, behind: Path) -> Path {
    let transmittance = exp(-clouds.fog.w * distance);
    return in_front(clouds.fog.rgb * view.exposure * (1.0 - transmittance), transmittance, behind);
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
        + clouds.sun_color.rgb * clouds.sun.w * 0.025
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
        sample_transmittance_lut_segment(r, mu, distance));
#else
    return Path(vec3(0.0), vec3(1.0));
#endif
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
    let disks = sample_sun_radiance(ray);
#endif
    if any(uv < vec2(0.0)) || any(uv > vec2(1.0)) {
        discard;
    }
    let fog = clouds.fog.w > 0.0;
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
        var path = Path((sky + disks * transmittance) * view.exposure, transmittance);
#else
        var path = Path(vec3(0.0), vec3(1.0));
#endif
        path = fogged(start, in_front(cloud.light, cloud.transmittance, path));
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
            path = fogged(min(distance, start), path);
            light += path.light * f32(geometry_samples);
            transmittance = path.transmittance * f32(geometry_samples);
        } else {
            for (var i = 0u; i < samples; i += 1u) {
                let distance = distances[i];
                if distance < 0.0 {
                    continue;
                }
                var path = aerial_perspective(ray, uv, distance);
                if distance >= start {
                    path = in_front(cloud.light, cloud.transmittance, path);
                }
                path = fogged(min(distance, start), path);
                light += path.light;
                transmittance += path.transmittance;
            }
        }
        transmittance /= f32(geometry_samples);
    } else {
        transmittance = sky_transmittance;
    }
    light /= f32(samples);

#ifdef DUAL_SOURCE_BLENDING
    return Output(vec4(light, 0.0), vec4(transmittance, 1.0));
#else
    return Output(vec4(light, dot(transmittance, vec3(1.0 / 3.0))));
#endif
}
