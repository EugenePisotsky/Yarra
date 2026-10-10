// The open sea (`atmosphere::SeaSurface`), drawn by the sky composite rather than the main pass:
// each view ray is intersected with the sea's surface, flat out to SEA_FLAT_RADIUS around the
// view and then bending with the planet, and whatever the main pass drew beyond it (the seabed,
// the foot of a rock, or nothing far out) is seen through the water. Shallow water shows the sand
// below; deep water turns into the light its body scatters up; foam runs along the waterline.
import bevy_pbr::atmosphere::{
    bindings::{lights, view},
    functions::{get_view_position, sample_transmittance_lut},
};
import package::shaders::clouds::maps::cloud_shadow_texel;
import package::shaders::sky::air::scattering;
import package::shaders::sky::celestial::clear_sky;
import package::shaders::sky::cloud_layer::cloud_layer;
import package::shaders::sky::composite_common::{
    clouds, cloud_shadow, cloud_shadow_sampler, depth, mist_sampler, noise, noise_sampler,
    shore_map, Path, NO_SEA,
};
import package::shaders::water::waves::sea_waves;

const SEA_FLAT_RADIUS: f32 = 10000.0;
const SEA_PLANET_RADIUS: f32 = 6360000.0;
const WATER_F0: f32 = 0.02;
// Size of the noise tile the gust patches come from, metres.
const SEA_PATCH: f32 = 1200.0;
// Glitter is kept within half floats like the sun's disc.
const MAX_GLITTER: f32 = 30000.0;
// Clear coastal water: extinction per metre (red is absorbed within a few metres, blue and green
// carry), and the share of the daylight entering deep water that its body scatters back up.
const WATER_EXTINCTION: vec3<f32> = vec3(0.45, 0.10, 0.075);
const WATER_ALBEDO: vec3<f32> = vec3(0.0025, 0.012, 0.016);
// The seabed's own light comes down through the water, a longer path than straight down.
const WATER_LIGHT_PATH: f32 = 1.3;
// Swash: every SWASH_PERIOD seconds (whole cycles per wave period) the water runs up the beach
// and drains back over SWASH metres of depth, its phase varying along the shore. Foam rides the
// front and trails behind it; the sand the water has drained from is left wet and darker.
// Widths are along the ground, in metres, turned into depth by the seabed's slope, so a nearly
// flat beach gets a thin foam line rather than sheets.
const SWASH: f32 = 0.12;
const SWASH_RUN: f32 = 12.0;
const SWASH_PERIOD: f32 = 9.0;
const FOAM_FRONT: f32 = 0.35;
const FOAM_TRAIL: f32 = 2.0;
const FOAM_ALBEDO: f32 = 0.7;
const WET_SAND: f32 = 0.6;
const WET_RUN: f32 = 1.5;
// Run over which a film of water gains its full reflection and colour.
const THIN_WATER: f32 = 1.5;
// Water deeper than this is past the reach of the swash and its foam.
const SHORE_DEPTH: f32 = 0.3;
// Relief of the sand, metres: no band of the swash is thinner in depth than this, or on a nearly
// flat beach its centimetre bumps and hollows cut film, wet sand and foam into hard patches.
const SAND_RELIEF: f32 = 0.03;

// Depth over which a band of the swash `run` metres wide along the ground lies, on a seabed rising
// `slope` per metre.
fn swash_band(slope: f32, run: f32) -> f32 {
    return max(slope * run, SAND_RELIEF);
}

// Height of the main pass's surface at a pixel, from its first depth sample.
fn surface_height(pixel: vec2<i32>) -> vec3<f32> {
    let z = textureLoad(depth, pixel, 0);
    let uv = (vec2<f32>(pixel) + 0.5 - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let world = view.world_from_clip * vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), z, 1.0);
    return world.xyz / world.w;
}

// Rise of the seabed per metre along the ground at a pixel, averaged over the next pixels'
// surfaces two pixels away in each direction, at least that of a very flat beach.
fn seabed_slope(pixel: vec2<i32>, here: vec3<f32>) -> f32 {
    var rise = 0.0;
    var count = 0.0;
    for (var i = 0; i < 4; i += 1) {
        let offset = select(vec2(0, 2), vec2(2, 0), (i & 1) == 0) * select(1, -1, i >= 2);
        let other = surface_height(pixel + offset);
        let run = length(other.xz - here.xz);
        if run > 1e-3 && run < 20.0 {
            rise += abs(other.y - here.y) / run;
            count += 1.0;
        }
    }
    return clamp(rise / max(count, 1.0), 0.002, 1.0);
}

// Distance along `ray` to the sea's surface, or NO_SEA.
fn sea_distance(ray: vec3<f32>) -> f32 {
    if clouds.ocean_waves.x < 0.5 || ray.y >= 0.0 {
        return NO_SEA;
    }
    let height = view.world_position.y - clouds.ocean.x;
    if height <= 0.0 {
        return NO_SEA;
    }
    let flat = height / -ray.y;
    let h = length(ray.xz);
    if flat * h <= SEA_FLAT_RADIUS {
        return flat;
    }
    // Beyond it the surface sinks by (r - F)^2 / 2R; the nearer root of a t^2 + b t + c = 0,
    // in the form that keeps its precision.
    let a = h * h / (2.0 * SEA_PLANET_RADIUS);
    let b = ray.y - SEA_FLAT_RADIUS * h / SEA_PLANET_RADIUS;
    let c = SEA_FLAT_RADIUS * SEA_FLAT_RADIUS / (2.0 * SEA_PLANET_RADIUS) + height;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return NO_SEA;
    }
    return 2.0 * c / (-b + sqrt(disc));
}

// Breaking waves. Crests come in from deep water along the shore map's lines of equal arrival
// time, at its pace: they lie along the shore and close up as the water shallows. A wave grows
// as it slows (Green's law, the fourth root of the depth's fall) and breaks where its height
// reaches BREAKING of the depth; from there on its height is held to that share, a white roller
// runs on its front and it leaves foam behind that breaks up into lace. Before breaking its crest
// sharpens and its front steepens. SURF_HEIGHT is the swell's height in deep water, scaled by the
// square of the weather's wind strength.
const SURF_HEIGHT: f32 = 0.6;
const SURF_DEPTH: f32 = 12.0;
const BREAKING: f32 = 0.78;
// Seconds the foam behind a broken wave lasts, and how far apart along the shore crests arrive
// at different times, metres.
const SURF_FOAM_LIFE: f32 = 3.0;
const SURF_ALONG: f32 = 260.0;
// Shore map times past this are water no crest reaches (`atmosphere::shore::UNREACHED`).
const SURF_UNREACHED: f32 = 500.0;
// Crests stand up out of the water where the view ray falls steeply enough for the march to
// resolve them (a sine of its descent of SURF_STANDING, faded in from 60% of it): a grazing ray
// crosses the waves' band over hundreds of metres and caught or missed crests at random. Farther
// out their slopes and foam draw them on the flat surface, as they do the broad swell beyond
// SURF_STANDING_DEPTH, where crests are low and their standing up shows little.
const SURF_STANDING: f32 = 0.09;
const SURF_STANDING_DEPTH: f32 = 4.0;
// Steps of the march: about SURF_STEP metres along the view, within these bounds.
const SURF_STEP: f32 = 1.0;
const SURF_STEPS: f32 = 8.0;
// The band the surf fills at the weather's usual wind, metres above and below the sea level: the
// highest crests (the largest of a set at their breaking point) and the deepest troughs.
const SURF_CRESTS: f32 = 1.2;
const SURF_TROUGHS: f32 = 0.6;

// Foam as a network around bubbles: nearly whole when fresh, opening into holes as it ages
// (0..1) until thin threads are left, the holes at the Worley cells' cores half a metre across. Past a fifth of a cell per
// pixel it fades into the veil it averages to.
const LACE_SCALE: f32 = 6.0;
fn foam_lace(xz: vec2<f32>, age: f32, footprint: f32) -> f32 {
    let cells = textureSampleLevel(noise, noise_sampler, vec3(xz / LACE_SCALE, 0.41), 0.0).g;
    let open = mix(0.95, 0.35, age);
    let lace = 1.0 - smoothstep(open, open + 0.06, cells);
    let veil = clamp((open - 0.3) / 0.65, 0.0, 1.0);
    return mix(lace, veil, smoothstep(0.1, 0.5, footprint));
}

struct Surf {
    // Depth below the sea level by the shore map.
    depth: f32,
    // Height of the surface above the sea level, and its slope.
    height: f32,
    slope: vec2<f32>,
    foam: f32,
    // How far the waves here have broken, 0..1.
    broken: f32,
    // Where the waves are in their cycle: 0 as a crest passes, rising to 1 as the next comes.
    cycle: f32,
}

// Three fields varying smoothly along a shore, 0..1, a few hundred metres across: sums of sines
// in different directions, so they never repeat visibly along it.
fn along_shore(local: vec2<f32>) -> vec3<f32> {
    let q = local * (250.0 / SURF_ALONG);
    let a = sin(dot(q, vec2(0.0213, 0.0137)) + 1.3) + sin(dot(q, vec2(-0.0089, 0.0291)) + 4.1);
    let b = sin(dot(q, vec2(0.0171, -0.0233)) + 2.7) + sin(dot(q, vec2(0.0307, 0.0071)) + 0.4);
    let c = sin(dot(q, vec2(-0.0247, -0.0119)) + 5.2) + sin(dot(q, vec2(0.0063, -0.0311)) + 3.3);
    return 0.5 + 0.25 * vec3(a, b, c);
}

// `footprint` is the pixel's length on the water along the view, metres: each side of a crest is
// a Gaussian, blurred over the footprint by adding their variances (keeping its area), so far
// surf softens instead of breaking up into speckle; the foam's lace fades into an even veil.
// Without `shade` only the height is worked out, for the march to the surface.
fn surf_at(xz: vec2<f32>, footprint: f32, shade: bool) -> Surf {
    var surf = Surf(1.0e4, 0.0, vec2(0.0), 0.0, 0.0, 0.0);
    if clouds.shore_map.w <= 0.0 {
        return surf;
    }
    let size = vec2<f32>(textureDimensions(shore_map));
    // World-anchored: from the map's own corner.
    let local = xz - clouds.shore_map.xy;
    let map = textureSampleLevel(shore_map, mist_sampler, local / (clouds.shore_map.z * size), 0.0);
    let depth = map.x;
    surf.depth = depth;
    let period = clouds.shore_map.w;
    let along = along_shore(local);
    let theta = (clouds.ocean.w - map.y) / period + along.r * 1.5;
    let s = fract(theta);
    surf.cycle = s;
    let weight = 1.0 - smoothstep(SURF_DEPTH * 0.6, SURF_DEPTH, depth);
    if weight <= 0.0 || map.y >= SURF_UNREACHED || depth <= 0.0 {
        return surf;
    }
    // Sets: each crest its own height (golden-angle steps of a sine, so neighbours differ), which
    // shifts smoothly along the shore, and some stretches of shore get bigger ones. Between the
    // crest that has passed and the one coming the water takes after both, so the surface stays
    // whole across each crest.
    let passed = floor(theta);
    let luck = 0.5 + 0.5 * sin(vec2(passed, passed + 1.0) * 2.39996 + along.g * 6.2831853);
    let strength = clouds.ocean_waves.y;
    let shoaling = pow(SURF_DEPTH / max(depth, 0.3), 0.25);
    let shoaled = SURF_HEIGHT * strength * strength * (0.6 + 0.8 * luck) * (0.7 + 0.6 * along.b)
        * shoaling;
    let ratio = mix(shoaled.x, shoaled.y, s) / (BREAKING * depth);
    surf.broken = smoothstep(0.9, 1.2, ratio) * weight;
    // Heights of the crest behind and the one ahead.
    let crests = min(shoaled, vec2(BREAKING * depth)) * weight;
    // Distances to the crest ahead (the front) and behind, in periods; widths of each side, and
    // the footprint in periods (a period's length is the time's gradient's inverse).
    let steep = clamp(ratio, 0.0, 1.0);
    let front = mix(0.2, 0.05, steep * steep);
    let back = mix(0.2, 0.32, steep);
    let blur = footprint * length(map.zw) / period;
    let wide_front = sqrt(front * front + blur * blur);
    let wide_back = sqrt(back * back + blur * blur);
    let v = 1.0 - s;
    let ahead = front / wide_front * exp(-(v * v) / (wide_front * wide_front));
    let behind = back / wide_back * exp(-(s * s) / (wide_back * wide_back));
    surf.height = crests.y * ahead + crests.x * behind
        - 0.886 * (front + back) * mix(crests.x, crests.y, s);
    if !shade {
        return surf;
    }
    let rise = crests.y * 2.0 * v / (wide_front * wide_front) * ahead
        - crests.x * 2.0 * s / (wide_back * wide_back) * behind;
    surf.slope = rise * -map.zw / period;
    // The roller on a broken wave's front, and the foam it leaves, breaking up into lace.
    if surf.broken <= 0.0 {
        return surf;
    }
    let wide_roller = sqrt(0.035 * 0.035 + blur * blur);
    let roller = 0.035 / wide_roller * exp(-(v * v) / (wide_roller * wide_roller));
    let drift = clouds.ocean.yz * clouds.ocean.w * 0.2;
    // It thins as it opens, and the bubbles left grow clearer.
    let left = exp(-s * period / SURF_FOAM_LIFE);
    let trail = left * left * (0.5 + 0.5 * left) * foam_lace(local + drift, 1.0 - left, footprint);
    surf.foam = clamp(roller + 0.85 * trail, 0.0, 1.0) * surf.broken;
    return surf;
}

// Height of the surf as the march to the surface sees it.
fn standing_surf(xz: vec2<f32>, footprint: f32) -> f32 {
    let surf = surf_at(xz, footprint, false);
    return surf.height
        * (1.0 - smoothstep(0.6 * SURF_STANDING_DEPTH, SURF_STANDING_DEPTH, surf.depth));
}

// Distance along `ray` to the sea's surface with the surf standing up out of it, or NO_SEA.
// Near the shore the view ray is marched down through the band the waves fill, from their
// highest crests to below their troughs, then refined between the samples either side.
fn sea_surface(ray: vec3<f32>) -> f32 {
    let flat = sea_distance(ray);
    let standing = smoothstep(0.6 * SURF_STANDING, SURF_STANDING, -ray.y);
    if flat >= NO_SEA || standing <= 0.0 || clouds.shore_map.w <= 0.0 {
        return flat;
    }
    let p = view.world_position + ray * flat;
    let size = vec2<f32>(textureDimensions(shore_map));
    // The pixel's length on the water along the view, per metre of distance.
    let spread = 2.0 / (view.clip_from_view[1][1] * view.main_pass_viewport.w) / max(-ray.y, 0.02);
    let map = textureSampleLevel(shore_map, mist_sampler,
        (p.xz - clouds.shore_map.xy) / (clouds.shore_map.z * size), 0.0);
    if map.x >= SURF_STANDING_DEPTH + 1.0 || map.y >= SURF_UNREACHED {
        return flat;
    }
    let strength = clouds.ocean_waves.y;
    let scale = strength * strength * standing;
    let height = view.world_position.y - clouds.ocean.x;
    let first = max(height - SURF_CRESTS * scale, 0.0) / -ray.y;
    let last = (height + SURF_TROUGHS * scale) / -ray.y;
    var before = first;
    var above = height + ray.y * first
        - standing_surf((view.world_position + ray * first).xz, first * spread) * standing;
    if above <= 0.0 {
        return first;
    }
    let steps = clamp(ceil((last - first) / SURF_STEP), 4.0, SURF_STEPS);
    for (var i = 1.0; i <= steps; i += 1.0) {
        let t = mix(first, last, i / steps);
        let here = height + ray.y * t
            - standing_surf((view.world_position + ray * t).xz, t * spread) * standing;
        if here <= 0.0 {
            var a = before;
            var fa = above;
            var b = t;
            var fb = here;
            for (var k = 0u; k < 2u; k += 1u) {
                let m = a + (b - a) * fa / max(fa - fb, 1e-4);
                let fm = height + ray.y * m
                    - standing_surf((view.world_position + ray * m).xz, m * spread) * standing;
                if fm > 0.0 {
                    a = m;
                    fa = fm;
                } else {
                    b = m;
                    fb = fm;
                }
            }
            return a + (b - a) * fa / max(fa - fb, 1e-4);
        }
        before = t;
        above = here;
    }
    return flat;
}

// Share of the sun's direct light the cloud layer lets through to `p`.
fn sea_cloud_shadow(p: vec3<f32>) -> f32 {
    let sun = clouds.sun.xyz;
    if clouds.layer.w < 0.5 || sun.y <= 0.0 {
        return 1.0;
    }
    return cloud_shadow_texel(cloud_shadow, cloud_shadow_sampler, clouds.layer, clouds.offset, sun,
        p).r;
}

fn smith(n_dot_x: f32, alpha2: f32) -> f32 {
    return 2.0 * n_dot_x / (n_dot_x + sqrt(alpha2 + (1.0 - alpha2) * n_dot_x * n_dot_x));
}

// The sea in front of a sample `distance` metres away (NO_SEA for open sky), with its surface
// `sea_t` metres along the ray and `air` the path in front of the surface.
fn through_water(air: Path, ray: vec3<f32>, sea_t: f32, distance: f32, at: vec2<i32>) -> Path {
    let p = view.world_position + ray * sea_t;
    // The pixel's length on the water, stretched where the view grazes it.
    let pixel = 2.0 / (view.clip_from_view[1][1] * view.main_pass_viewport.w);
    let footprint = sea_t * pixel / max(-ray.y, 0.02);
    // Gusts roughen the water in patches a few hundred metres across, drifting with the wind
    // like the mist's wisps; calm patches between them read smoother and brighter.
    let q = (p.xz + clouds.mist_drift.xy) / SEA_PATCH;
    let gust = textureSampleLevel(noise, noise_sampler, vec3(q.x, 0.37, q.y), 0.0);
    let patches = mix(0.35, 1.65, smoothstep(0.25, 0.75, gust.r * 0.6 + gust.g * 0.4));
    // Near the shore the surf rides on them; broken and shallow water have little wind chop of
    // their own.
    let surf = surf_at(p.xz, footprint, true);
    let calm = (1.0 - 0.6 * surf.broken) * mix(0.15, 1.0, smoothstep(0.0, 2.0, surf.depth));
    let waves = sea_waves(p.xz, clouds.ocean.w, clouds.ocean.yz,
        clouds.ocean_waves.y * patches * calm, footprint);
    let tilt = waves.slope + surf.slope;
    let n = normalize(vec3(-tilt.x, 1.0, -tilt.y));
    let v = -ray;
    let n_dot_v = max(dot(n, v), 0.02);
    let fresnel = WATER_F0 + (1.0 - WATER_F0) * pow(1.0 - n_dot_v, 5.0);
    // Reflections of waves facing away from the eye would point into the sea: keep them just
    // above the horizon.
    let bounced = reflect(ray, n);
    let reflected = normalize(vec3(bounced.x, max(bounced.y, 0.004), bounced.z));
    let position = get_view_position();
    var sky = clear_sky(reflected) * view.exposure;
    // Under a closing deck the low sky is the deck's grey, as for the haze.
    let deck = clouds.haze.rgb * (clouds.air_light.rgb
        + clouds.air_sun.rgb * scattering(dot(reflected, clouds.sun.xyz))) * view.exposure;
    sky = mix(sky, deck, clouds.weather.z);
    @if(CLOUDS && CACHED_CLOUDS)
    if reflected.y > 0.01 {
        let cloud = cloud_layer(reflected, max(0.0, (clouds.layer.x - p.y) / reflected.y),
            vec2(0.0), false);
        sky = cloud.light + cloud.transmittance * sky;
    }
    // Sun and moon glitter: GGX over the resolved waves, widened by the unresolved ones and
    // the disc's own size; clouds shade it.
    let direct = sea_cloud_shadow(p);
    // Diffuse daylight keeps a floor of light scattered through the clouds, as surfaces do.
    let shade = max(direct, 0.12);
    let alpha2 = waves.variance + 0.0006;
    var glitter = vec3(0.0);
    for (var i = 0u; i < lights.n_directional_lights; i += 1u) {
        let light = &lights.directional_lights[i];
        let l = (*light).direction_to_light;
        let n_dot_l = dot(n, l);
        if l.y <= 0.0 || n_dot_l <= 0.0 {
            continue;
        }
        let h = normalize(l + v);
        let n_dot_h = max(dot(n, h), 0.0);
        let d = alpha2 / (3.14159265 * pow(n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0, 2.0));
        let f = WATER_F0 + (1.0 - WATER_F0) * pow(1.0 - max(dot(v, h), 0.0), 5.0);
        let g = smith(n_dot_v, alpha2) * smith(n_dot_l, alpha2);
        let transmittance = sample_transmittance_lut(length(position), dot(l, normalize(position)));
        // The moon's image is its drawn face, much dimmer than its art-directed light.
        let image = select(1.0, clouds.moon_sunward.w,
            clouds.moon_disc.w > 0.0 && dot(l, clouds.moon_disc.xyz) > 0.9999);
        glitter += (*light).color.rgb * transmittance * image * (d * f * g / (4.0 * n_dot_v));
    }
    // The glitter is the sun's own image: a closing deck hides it entirely.
    let open = 1.0 - clouds.weather.z;
    glitter *= direct * open * open;
    // Daylight entering the water, as radiance a white diffuser would send back.
    let sun = clouds.near_sun.rgb * max(clouds.sun.y, 0.0) * shade
        + clouds.moon_color.rgb * clouds.moon.w * max(clouds.moon.y, 0.0);
    let daylight = (sun / 3.14159265 + clouds.ambient.rgb * clouds.ambient.w) * view.exposure;
    // The water between the surface and what lies beyond, and the seabed's light coming down.
    let column = max(distance - sea_t, 0.0);
    let seabed = view.world_position.y + ray.y * distance;
    let depth = select(1.0e4, max(clouds.ocean.x - seabed, 0.0), distance < NO_SEA);
    let seen = exp(-WATER_EXTINCTION * min(column, 1.0e4));
    let lit = exp(-WATER_EXTINCTION * min(depth * WATER_LIGHT_PATH, 1.0e4));
    // Broken water is full of bubbles and stirred-up sand, and scatters more light back.
    let body = WATER_ALBEDO * (1.0 + 3.0 * surf.broken) * daylight * (1.0 - seen);
    var open_water = fresnel * sky + min(glitter * view.exposure, vec3(MAX_GLITTER))
        + (1.0 - fresnel) * body;
    // The seabed under the water is wet sand, darker than dry.
    var open_transmittance = (1.0 - fresnel) * seen * lit * WET_SAND;
    open_water = mix(open_water, FOAM_ALBEDO * daylight, surf.foam);
    open_transmittance *= 1.0 - surf.foam;
    if depth >= SHORE_DEPTH {
        return Path(air.light + air.transmittance * open_water,
            air.transmittance * open_transmittance);
    }
    // The swash: a quick run-up as each broken wave comes ashore and a slow drain, its front at
    // `edge` metres of depth. Without a shore map it keeps its own time.
    var cycle = surf.cycle;
    if clouds.shore_map.w <= 0.0 {
        let along = textureSampleLevel(noise, noise_sampler, vec3(p.xz / 90.0, 0.61), 0.0).r;
        cycle = fract(clouds.ocean.w / SWASH_PERIOD + along * 3.0);
    }
    let surge = select(1.0 - smoothstep(0.25, 1.0, cycle), smoothstep(0.0, 0.25, cycle),
        cycle < 0.25);
    // Near the waterline, the seabed's slope turns runs along the ground into depths.
    var slope = 1.0;
    if distance < NO_SEA {
        slope = seabed_slope(at, view.world_position + ray * distance);
    }
    let edge = min(SWASH, slope * SWASH_RUN) * (1.0 - surge);
    let film = smoothstep(edge, edge + swash_band(slope, THIN_WATER), depth)
        * smoothstep(0.0, swash_band(slope, 1.0), depth);
    var light = film * open_water;
    var transmittance = mix(vec3(1.0), open_transmittance, film);
    // Sand the water has just drained from stays wet.
    let wet = 1.0 - smoothstep(0.0, swash_band(slope, WET_RUN), depth - edge);
    transmittance *= mix(1.0, WET_SAND, wet * smoothstep(0.0, 0.01, depth) * (1.0 - film));
    // Foam rides the front and trails behind it in lace, drifting with the waves.
    let drift = clouds.ocean.yz * clouds.ocean.w * 0.3;
    let front = exp(-pow((depth - edge) / swash_band(slope, FOAM_FRONT), 2.0))
        * foam_lace(p.xz + drift, 0.3, footprint);
    let trail = (1.0 - smoothstep(edge, edge + swash_band(slope, FOAM_TRAIL), depth))
        * foam_lace(p.xz - drift, 1.0, footprint)
        * smoothstep(edge, edge + swash_band(slope, FOAM_FRONT), depth);
    // Nothing starts hard at the dry edge: the terrain's contour there zigzags.
    let shore = smoothstep(0.0, swash_band(slope, 1.0), depth);
    let foam = clamp(front + 0.25 * trail, 0.0, 1.0) * (0.4 + 0.6 * surge) * shore;
    light = mix(light, FOAM_ALBEDO * daylight, foam);
    transmittance *= 1.0 - foam;
    return Path(air.light + air.transmittance * light, air.transmittance * transmittance);
}
