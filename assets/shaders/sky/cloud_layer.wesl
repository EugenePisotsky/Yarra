// The cloud layer as the sky composite sees it: the cached panorama or this frame's clouds, whose
// texels hold the light scattered per unit of sun and moon light and where the clouds lie
// (clouds/render.wesl), lit here by this frame's lights.
import package::shaders::clouds::panorama::sky_panorama_uv;
import package::shaders::sky::composite_common::{
    environment, cloud_newer, cloud_older, cloud_sampler, composite_settings, LIGHTNING_COLOR,
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

// Radiance a cloud scatters per unit of light it scatters (clouds/render.wesl) and lux of the sun
// or moon on it.
const CLOUD_LIGHT: f32 = 0.065;
// Sunlit land and sea below light the clouds' bases (albedo about 0.1 over the island): at midday
// as much as the sky lights them from above.
const GROUND_BOUNCE: f32 = 0.07;

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

// Light scattered per unit of sky light by a cloud's base and by its top: the sky lights the top
// more. The images keep the clouds' opacity-weighted distance instead (clouds/render.wesl): the
// share follows height linearly, so the clouds' mean height gives the whole sum.
const SKY_LIGHT: vec2<f32> = vec2(0.15, 0.6);

// A cached image was traced from where the camera then stood in the drifting cloud field, row by
// row over a sweep: the uv of the ray that saw the clouds now `distance` metres along `ray`, from
// the image's shifts since its first and its last row.
@if(CACHED_CLOUDS)
fn traced_uv(ray: vec3<f32>, distance: f32, first: vec4<f32>, last: vec4<f32>, row: f32) -> vec2<f32> {
    return sky_panorama_uv(normalize(ray * distance + mix(first.xyz, last.xyz, row)));
}

// The cached images along `ray`, which crosses the layer's base `start` metres away, each carried
// with the wind and the camera's moves since it was traced, cross-faded.
@if(CACHED_CLOUDS)
fn cached_clouds(ray: vec3<f32>, start: f32, cubic: bool) -> vec4<f32> {
    let uv = sky_panorama_uv(ray);
    // How far off the clouds are; where the newest image holds little cloud, about the layer's
    // middle.
    let here = textureSampleLevel(cloud_newer, cloud_sampler, uv, 0.0);
    let middle = start + 0.5 * environment.layer.y / max(ray.y, 0.01);
    let distance = (here.b + 0.02 * middle) / (1.0 - here.a + 0.02);
    let shift = composite_settings.cloud_shift;
    let newer = traced_uv(ray, distance, shift[0], shift[1], uv.y);
    let older = traced_uv(ray, distance, shift[2], shift[3], uv.y);
    if cubic {
        return mix(panorama_cubic(cloud_older, older), panorama_cubic(cloud_newer, newer),
            composite_settings.flags.x);
    }
    return mix(textureSampleLevel(cloud_older, cloud_sampler, older, 0.0),
        textureSampleLevel(cloud_newer, cloud_sampler, newer, 0.0), composite_settings.flags.x);
}

// The cloud layer where the ray crosses its base, `start` metres away; `cubic` for the smooth
// lookup, which reflections in the waves have no need for.
fn cloud_layer(ray: vec3<f32>, start: f32, screen_uv: vec2<f32>, cubic: bool) -> Cloud {
    @if(CACHED_CLOUDS)
    let c = cached_clouds(ray, start, cubic);
    @if(!CACHED_CLOUDS)
    let c = textureSampleLevel(cloud_newer, cloud_sampler, screen_uv, 0.0);
    let haze = exp(-start * 3.912 / max(environment.haze.w, 50.0));
    // Haze in front of an opaque cloud must not reintroduce the sun/moon disk behind it. With
    // the atmosphere, a cloud lost in haze turns into the sky it stands in.
    var air = environment.haze.rgb * (environment.ambient.rgb * environment.ambient.w * 0.3
        + environment.near_sun.rgb * 0.025
        + environment.moon_color.rgb * environment.moon.w * 0.025) * view.exposure;
    // Under a closing deck far clouds keep the deck's grey.
    @if(ATMOSPHERE)
    air = mix(clear_sky(ray) * view.exposure, air, environment.weather.z);
    // The sky lights the clouds from above and the lit ground a little from below, by the clouds'
    // height in the layer: the ray rises from its base, or from the camera inside the layer.
    let opacity = 1.0 - c.a;
    let above = select(view.world_position.y - environment.layer.x, -start * ray.y, start > 0.0);
    let h = clamp((above + ray.y * c.b / max(opacity, 1e-4)) / environment.layer.y, 0.0, 1.0);
    let sky = environment.ambient.rgb * environment.ambient.w;
    let ground = environment.near_sun.rgb * max(environment.sun.y, 0.0)
        * (1.0 - environment.weather.z) + sky;
    var lit = c.r * environment.sun_color.rgb * environment.sun.w * CLOUD_LIGHT
        + c.g * environment.moon_color.rgb * environment.moon.w * CLOUD_LIGHT
        + opacity * mix(SKY_LIGHT.x, SKY_LIGHT.y, h) * (sky + ground * GROUND_BOUNCE);
    // A lightning flash lights the clouds, most near the strike.
    if environment.lightning.w > 0.0 {
        let to_strike = normalize(environment.lightning.xyz - view.world_position);
        let near = exp(-(1.0 - dot(ray, to_strike)) * 25.0) + 0.12;
        lit += LIGHTNING_COLOR * environment.lightning.w * near * (1.0 - c.a);
    }
    let color = mix(air * (1.0 - c.a), lit * view.exposure, haze);
    // Fade the finite ground-view tracing range into the horizon rather than exposing a
    // straight edge at the end of the cloud layer.
    // A closed deck stays to the horizon: fading it showed clear sky beneath.
    let coverage = 1.0 - smoothstep(20000.0, 40000.0, start) * (1.0 - environment.weather.z);
    return Cloud(color * coverage, 1.0 - coverage * (1.0 - c.a));
}
