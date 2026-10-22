// The cloud layer traced along view rays (crates/atmosphere/src/clouds): into the cached sky's
// panorama (CACHED_SKY) or into a screen image. A texel keeps the light its clouds scatter towards
// the eye per unit of sun, moon and sky light (rgb), and the share of the sky behind they let
// through (a). The sky composite colours them with this frame's lights (sky/cloud_layer.wesl), so
// dawn and dusk colour the clouds as fast as the sky, not as fast as the cache refreshes.
import bevy_render::view::View;
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import package::shaders::environment::params::EnvironmentParams;
import package::shaders::clouds::panorama::sky_panorama_ray;
import package::shaders::clouds::density::{density_at, light_depth};
@group(0) @binding(0) var<storage, read> environment: EnvironmentParams;
@group(0) @binding(1) var noise: texture_3d<f32>;
@group(0) @binding(2) var repeat: sampler;
@group(0) @binding(3) var<uniform> view: View;
@group(0) @binding(4) var shape: texture_3d<f32>;
@group(0) @binding(5) var detail: texture_3d<f32>;

// The moon lights the clouds only while the sun's light on them is less than this many times its
// own: by day it adds nothing a sample could hold, and tracing its beam would double the work.
const MOON_SEEN: f32 = 1000.0;
// Steps towards a light from each sample inside a cloud.
const LIGHT_STEPS: u32 = 6u;
// Through empty air the march takes steps this many times longer, and returns to them after this
// many empty samples.
const CHEAP_STRIDE: f32 = 3.0;
const EMPTY_SAMPLES: u32 = 6u;
// Light scattered many times, against the beam's phase: most of what a thick cloud returns, which
// whitens its sunlit side from every direction and keeps its shaded side grey, not black.
const DIFFUSION: f32 = 2.0;

// Henyey-Greenstein phase, scaled to a mean of 1 over directions; `mu` the cosine between the
// light's travel and the view ray.
fn hg(mu: f32, g: f32) -> f32 {
    return (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * mu, 1.5);
}

// Water droplets scatter mostly forwards, with a weaker lobe back towards the light, which keeps
// clouds lit with the sun behind the viewer; `spread` evens both out for light scattered again.
fn phase(mu: f32, spread: f32) -> f32 {
    return mix(hg(mu, 0.8 * spread), hg(mu, -0.25 * spread), 0.3);
}

// Light a sample scatters towards the eye per unit of a light whose beam reaches it through
// `optical` depth of cloud: once (the beam), again (Wrenninge's octave: half as strong, reaching
// four times as deep, spread more evenly), and many times (diffusion).
fn scattered(optical: f32, mu: f32) -> f32 {
    let diffuse = 1.0 / (1.0 + 0.11 * optical);
    return exp(-optical) * phase(mu, 1.0) + 0.5 * exp(-0.25 * optical) * phase(mu, 0.5)
        + DIFFUSION * diffuse;
}

// A static per-texel offset of the march's first step, so its stepping shows as fine grain, not
// as bands.
fn jitter(position: vec2<f32>) -> f32 {
    let p = vec2<u32>(position);
    var h = (p.x * 0x8da6b343u) ^ (p.y * 0xd8163841u);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = h ^ (h >> 12u);
    return f32(h & 0xffffu) / 65535.0;
}

@fragment fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // Hemisphere panorama. It covers every camera orientation; turns only change
    // the lookup in the full-resolution composite, never the cache.
    @if(CACHED_SKY)
    let ray = sky_panorama_ray(in.uv);
    @if(!CACHED_SKY)
    let uv = in.uv;
    @if(!CACHED_SKY)
    if any(uv < vec2(0.0)) || any(uv > vec2(1.0)) { return vec4(0.0, 0.0, 0.0, 1.0); }
    @if(!CACHED_SKY)
    let ndc = vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 1.0, 1.0);
    @if(!CACHED_SKY)
    let point = view.world_from_clip * ndc;
    @if(!CACHED_SKY)
    let ray = normalize(point.xyz / point.w - view.world_position);
    let camera = view.world_position + vec3(environment.offset.x, 0.0, environment.offset.y);
    if ray.y <= 0.01 || camera.y >= environment.layer.x + environment.layer.y {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    let start = max(0.0, (environment.layer.x - camera.y) / ray.y);
    let end = min((environment.layer.x + environment.layer.y - camera.y) / ray.y, 40000.0);
    if end <= start { return vec4(0.0, 0.0, 0.0, 1.0); }
    // About 25 m steps, at least 48: a long grazing path through the layer needs more of them,
    // or sharp-edged clouds break into slices. Traced every frame (High), at most 80.
    @if(CACHED_SKY)
    let most = 160u;
    @else
    let most = 80u;
    let steps = clamp(u32((end - start) / 25.0), 48u, most);
    let ds = (end - start) / f32(steps);
    let sun = environment.sun;
    let moon = select(vec4(0.0), environment.moon, environment.moon.w * MOON_SEEN > sun.w);
    let sun_mu = dot(ray, sun.xyz);
    let moon_mu = dot(ray, moon.xyz);
    var transmittance = 1.0;
    var light = vec3(0.0);
    var t = start + jitter(in.position.xy) * ds;
    var cheap = true;
    var empty = 0u;
    for (var i = 0u; i < steps * 2u && t < end; i += 1u) {
        let p = camera + ray * t;
        if cheap {
            // Through empty air in long strides; on meeting cloud, back one stride.
            if density_at(p, environment, noise, shape, detail, repeat, false) > 0.0 {
                cheap = false;
                empty = 0u;
                t = max(t - CHEAP_STRIDE * ds, start);
            } else {
                t += CHEAP_STRIDE * ds;
            }
            continue;
        }
        let density = density_at(p, environment, noise, shape, detail, repeat, true);
        if density > 0.001 {
            let a = 1.0 - exp(-density * environment.shape.y * ds);
            let h = clamp((p.y - environment.layer.x) / environment.layer.y, 0.0, 1.0);
            var sunlit = 0.0;
            var moonlit = 0.0;
            if sun.w > 0.0 {
                sunlit = scattered(light_depth(p, sun.xyz, environment, noise, shape, detail,
                    repeat, LIGHT_STEPS), sun_mu);
            }
            if moon.w > 0.0 {
                moonlit = scattered(light_depth(p, moon.xyz, environment, noise, shape, detail,
                    repeat, LIGHT_STEPS), moon_mu);
            }
            // The sky lights a cloud's top more than its base.
            light += transmittance * a * vec3(sunlit, moonlit, mix(0.15, 0.6, h));
            transmittance *= 1.0 - a;
            empty = 0u;
            // Once opaque, discard the tiny residual transmission too. Leaving a
            // 1% floor leaks extremely bright HDR sun/moon disks through solid clouds.
            if transmittance < 0.003 {
                transmittance = 0.0;
                break;
            }
        } else {
            empty += 1u;
            cheap = empty >= EMPTY_SAMPLES;
        }
        t += ds;
    }
    return vec4(light, transmittance);
}
