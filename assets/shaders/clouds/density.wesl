// Cloud density, after Guerrilla's Nubis (Horizon Zero Dawn): weather sets the coverage region by
// region, and a height profile by cloud type shapes the layer, rounded cumulus under broken
// cloud and a low flat deck as the sky closes. The base shape noise (clouds/noise.wesl) is cut
// down to that profile and coverage, so each cloud is a billowing mass with one flat base; the
// detail noise moves its outline in and out, into billows above and wisps underneath. The cheap
// form skips the detail, for marching through empty air and for light paths, where the edges
// hardly matter.
import package::shaders::environment::params::EnvironmentParams;

// Body periods (the layer's z) the noise tiles span: the regional weather, the base shape and the
// detail. Whole numbers of each fit the cloud field's wrapping period (16 body periods).
const WEATHER_TILE: f32 = 4.0;
const SHAPE_TILE: f32 = 0.5;
const DETAIL_TILE: f32 = 0.0625;
// The detail moves a cloud's outline in or out by up to half this much of the base noise's 0..1,
// more as the weather erodes. The base outline is drawn half of it wider and the detail only
// erodes, so the march finds every cloud from the cheap form and coverage keeps its share.
const DETAIL_DEPTH: vec2<f32> = vec2(0.25, 0.4);
// Cores carry more than the edges' density, reached this far (in the base noise's 0..1) past the
// coverage's threshold: clouds are solid bodies with a defined outline, which the detail breaks.
const CORE_DENSITY: f32 = 2.5;
const EDGE_RAMP: f32 = 0.06;

// What the weather makes of the field at a point: coverage (0 clear, 1 closed), how far it is a
// closed deck rather than separate cumulus, how much the detail erodes, and the local share of
// the target extinction.
struct Weather {
    coverage: f32,
    deck: f32,
    erosion: f32,
    extinction: f32,
}

// `q` is the field position in body periods.
fn cloud_weather(q: vec2<f32>, c: EnvironmentParams, noise: texture_3d<f32>, repeat: sampler) -> Weather {
    let seed = c.shape.w * 0.37;
    let region = textureSampleLevel(noise, repeat, vec3(q.x, seed * WEATHER_TILE, q.y) / WEATHER_TILE, 0.0);
    // Weather arrives region by region rather than as one global threshold: each part of the
    // field blends from the previous shape at its own time, over ~40% of the change.
    let arrival = smoothstep(0.25, 0.75, mix(region.g, region.b, 0.5));
    let local = smoothstep(0.0, 1.0, (c.transition.w * 1.6 - arrival) / 0.6);
    let coverage = mix(c.transition.x, c.shape.x, local);
    var w: Weather;
    w.deck = smoothstep(0.65, 0.95, coverage);
    // Broken cloud gathers over some regions and clears over others; a closing deck less so.
    w.coverage = clamp(coverage + (region.b - 0.5) * mix(0.85, 0.3, w.deck), 0.0, 1.0);
    w.erosion = mix(c.transition.z, c.shape.z, local);
    w.extinction = select(1.0, mix(c.transition.y, c.shape.y, local) / c.shape.y, c.shape.y > 1e-6);
    return w;
}

// How much of the layer's density a cloud type allows at height share `h`: cumulus rise steeply
// from a flat base and round off towards the top; a deck lies low and flat.
fn height_profile(h: f32, deck: f32) -> f32 {
    let cumulus = smoothstep(0.0, 0.08, h) * (1.0 - smoothstep(0.35, 1.0, h));
    let stratus = smoothstep(0.0, 0.05, h) * (1.0 - smoothstep(0.25, 0.6, h));
    return mix(cumulus, stratus, deck);
}

// Density at render-space point `p` (with the field offset applied: see `field_point`), as a share
// of the layer's extinction; `detailed` erodes the edges.
fn density_at(p: vec3<f32>, c: EnvironmentParams, noise: texture_3d<f32>, shape: texture_3d<f32>,
    detail: texture_3d<f32>, repeat: sampler, detailed: bool) -> f32 {
    let h = (p.y - c.layer.x) / c.layer.y;
    if h <= 0.0 || h >= 1.0 || c.layer.w < 0.5 || max(c.shape.x, c.transition.x) <= 0.0 {
        return 0.0;
    }
    let seed = vec3(c.shape.w, c.shape.w * 0.37, c.shape.w * 0.73);
    let q = (p.xz - c.offset.zw) / c.layer.z + seed.xz;
    let weather = cloud_weather(q, c, noise, repeat);
    let profile = height_profile(h, weather.deck);
    if profile * weather.coverage <= 0.0 {
        return 0.0;
    }
    let up = (p.y - c.layer.x) / c.layer.z + seed.y;
    let base = textureSampleLevel(shape, repeat, vec3(q.x, up, q.y) / SHAPE_TILE, 0.0).r;
    // How far inside the outline, in the base noise's units.
    let depth = mix(DETAIL_DEPTH.x, DETAIL_DEPTH.y, weather.erosion);
    var inside = base * profile - (1.0 - weather.coverage) + 0.5 * depth;
    // Only near the outline can the detail matter: deep inside a cloud is solid either way.
    if detailed && inside > 0.0 && inside < depth + EDGE_RAMP {
        let fine = textureSampleLevel(detail, repeat, vec3(q.x, up, q.y) / DETAIL_TILE, 0.0).r;
        // Worley noise peaks at its cells' points: eroding where it is low leaves round billows
        // above, and where it is high leaves the threads between cells as wisps under the base.
        inside -= mix(fine, 1.0 - fine, smoothstep(0.0, 0.15, h)) * depth;
    }
    return clamp(inside / EDGE_RAMP, 0.0, 1.0) * CORE_DENSITY * weather.extinction;
}

// The first step towards a light, at most, metres.
const LIGHT_STEP: f32 = 25.0;

// Optical depth from `p` towards a light in `direction` through the layer: out through its top
// towards a light above, or its base towards one just below the horizontal (a sun that has set
// for the ground but not for the clouds). Each step is twice the last, so near shadowing is sharp
// and the whole way counts: the last stretches to the layer's edge, for clouds shading clouds.
// Only the first, within a billow's size of the sample, sees the detail.
fn light_depth(p: vec3<f32>, direction: vec3<f32>, c: EnvironmentParams, noise: texture_3d<f32>,
    shape: texture_3d<f32>, detail: texture_3d<f32>, repeat: sampler, steps: u32) -> f32 {
    let exit = select(p.y - c.layer.x, c.layer.x + c.layer.y - p.y, direction.y >= 0.0);
    let distance = min(max(exit, 0.0) / max(abs(direction.y), 0.04), 30000.0);
    var optical = 0.0;
    var start = 0.0;
    var length = min(distance / (exp2(f32(steps)) - 1.0), LIGHT_STEP);
    for (var i = 0u; i < steps; i += 1u) {
        if i + 1u == steps {
            length = max(distance - start, 0.0);
        }
        let at = p + direction * (start + 0.5 * length);
        optical += density_at(at, c, noise, shape, detail, repeat, i == 0u) * length;
        start += length;
        length *= 2.0;
    }
    return optical * c.shape.y;
}
