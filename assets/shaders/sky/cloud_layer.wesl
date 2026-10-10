// The cloud layer as the sky composite sees it: the cached panorama or this frame's clouds.
import package::shaders::clouds::types::sky_panorama_uv;
import package::shaders::sky::composite_common::{
    clouds, cloud_newer, cloud_older, cloud_sampler, composite_settings, LIGHTNING_COLOR,
};
@if(ATMOSPHERE)
import bevy_pbr::atmosphere::bindings::view;
@else
import package::shaders::sky::composite_common::view;
@if(ATMOSPHERE)
import package::shaders::sky::celestial::clear_sky;

struct Cloud {
    light: vec3<f32>,
    transmittance: f32,
}

// The cloud panorama as a cubic B-spline, in four bilinear taps: drawn bilinearly, sharp cloud
// edges showed the texels as staircases.
@if(CACHED_CLOUDS)
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

// The cloud layer where the ray crosses its base, `start` metres away; `cubic` for the smooth
// lookup, which reflections in the waves have no need for.
fn cloud_layer(ray: vec3<f32>, start: f32, screen_uv: vec2<f32>, cubic: bool) -> Cloud {
    @if(CACHED_CLOUDS)
    let uv = sky_panorama_uv(ray);
    @if(CACHED_CLOUDS)
    var c: vec4<f32>;
    @if(CACHED_CLOUDS)
    if cubic {
        c = mix(panorama_cubic(cloud_older, uv), panorama_cubic(cloud_newer, uv), composite_settings.x);
    } else {
        c = mix(textureSampleLevel(cloud_older, cloud_sampler, uv, 0.0),
            textureSampleLevel(cloud_newer, cloud_sampler, uv, 0.0), composite_settings.x);
    }
    @if(!CACHED_CLOUDS)
    let c = textureSampleLevel(cloud_newer, cloud_sampler, screen_uv, 0.0);
    let haze = exp(-start * 3.912 / max(clouds.haze.w, 50.0));
    // Haze in front of an opaque cloud must not reintroduce the sun/moon disk behind it. With
    // the atmosphere, a cloud lost in haze turns into the sky it stands in.
    var air = clouds.haze.rgb * (clouds.ambient.rgb * clouds.ambient.w * 0.3
        + clouds.near_sun.rgb * 0.025
        + clouds.moon_color.rgb * clouds.moon.w * 0.025) * view.exposure;
    // Under a closing deck far clouds keep the deck's grey.
    @if(ATMOSPHERE)
    air = mix(clear_sky(ray) * view.exposure, air, clouds.weather.z);
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
