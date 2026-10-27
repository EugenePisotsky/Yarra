struct Params { previous_from_current: mat4x4<f32>, size: vec4<f32>, jitter_debug: vec4<f32> }
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var vectors: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_2d;
@group(0) @binding(3) var color: texture_2d<f32>;
@group(0) @binding(4) var linear_sampler: sampler;
@group(0) @binding(5) var history: texture_2d<f32>;
@vertex fn vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4(vec2(f32((i << 1u) & 2u),f32(i & 2u)) * 2.0 - 1.0,0.0,1.0);
}
@fragment fn motion(@builtin(position) p: vec4<f32>) -> @location(0) vec2<f32> {
    let xy = vec2<i32>(p.xy);
    if textureLoad(depth,xy,0) > 0.0 { return textureLoad(vectors,xy,0).xy; }
    // Background has no mesh prepass. Reproject directions, ignoring camera translation.
    let uv = (p.xy + params.jitter_debug.xy) / params.size.xy;
    let previous = params.previous_from_current * vec4(uv * vec2(2.0,-2.0) + vec2(-1.0,1.0),0.0,1.0);
    if previous.w <= 0.00001 { return vec2(0.0); }
    let previous_uv = previous.xy / previous.w * vec2(0.5,-0.5) + vec2(0.5);
    return uv - previous_uv;
}
@fragment fn display(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = p.xy / params.size.zw;
    let xy = vec2<i32>(clamp(uv * params.size.xy,vec2(0.0),params.size.xy - vec2(1.0)));
    if params.jitter_debug.z == 1.0 {
        let v = textureLoad(vectors,xy,0).xy * params.size.xy;
        return vec4(vec2(0.5) + v * 0.5,0.5,1.0);
    }
    if params.jitter_debug.z == 2.0 {
        let d = textureLoad(depth,xy,0);
        return vec4(vec3(pow(d,0.25)),1.0);
    }
    let sample_uv = clamp(uv * params.size.xy,vec2(0.5),params.size.xy-vec2(0.5)) / params.size.zw;
    return textureSampleLevel(color,linear_sampler,sample_uv,0.0);
}

// Temporal anti-aliasing at the rendered size (TemporalResolve::Taa): each pixel's jittered
// sample is blended into its history, reprojected by the complete scene motion, after the
// history is clipped to the spread of the pixel's neighbourhood so that stale colour (what was
// behind a moving edge) is dropped.
const TAA_BLEND: f32 = 0.1;
// How many standard deviations of the neighbourhood the history may stray before clipping.
const TAA_SPREAD: f32 = 1.25;

fn luma(c: vec3<f32>) -> f32 { return dot(c, vec3(0.2126, 0.7152, 0.0722)); }
// Blending tone-compressed colour keeps one bright sample from outweighing its neighbours.
fn compress(c: vec3<f32>) -> vec3<f32> { return c / (1.0 + luma(c)); }
fn expand(c: vec3<f32>) -> vec3<f32> { return c / max(1.0 - luma(c), 1.0e-4); }
fn ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3(dot(c, vec3(0.25, 0.5, 0.25)), dot(c, vec3(0.5, 0.0, -0.5)), dot(c, vec3(-0.25, 0.5, -0.25)));
}
fn rgb(c: vec3<f32>) -> vec3<f32> { return vec3(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z); }

fn history_at(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(history, linear_sampler, uv, 0.0).rgb;
}

// Catmull-Rom from five bilinear taps: bilinear history alone blurs a little more each frame.
fn sharp_history(uv: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    let at = uv * size;
    let centre = floor(at - 0.5) + 0.5;
    let f = at - centre;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let t0 = (centre - 1.0) / size;
    let t3 = (centre + 2.0) / size;
    let t12 = (centre + w2 / w12) / size;
    let c = history_at(vec2(t12.x, t0.y)) * (w12.x * w0.y)
        + history_at(vec2(t0.x, t12.y)) * (w0.x * w12.y)
        + history_at(t12) * (w12.x * w12.y)
        + history_at(vec2(t3.x, t12.y)) * (w3.x * w12.y)
        + history_at(vec2(t12.x, t3.y)) * (w12.x * w3.y);
    let weight = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return max(c / weight, vec3(0.0));
}

// Moves `c` toward `centre` until it lies within `extent` of it on every axis.
fn clip(c: vec3<f32>, centre: vec3<f32>, extent: vec3<f32>) -> vec3<f32> {
    let offset = c - centre;
    let units = abs(offset) / max(extent, vec3(1.0e-4));
    let largest = max(units.x, max(units.y, units.z));
    return select(c, centre + offset / largest, largest > 1.0);
}

struct Resolved { @location(0) image: vec4<f32>, @location(1) history: vec4<f32> }

@fragment fn taa(@builtin(position) p: vec4<f32>) -> Resolved {
    let size = params.size.xy;
    let xy = vec2<i32>(p.xy);
    let last = vec2<i32>(size) - vec2(1);
    var current = vec3(0.0);
    var mean = vec3(0.0);
    var square = vec3(0.0);
    // Motion from the nearest depth around the pixel (reverse Z: larger is nearer), so an edge
    // moves with the surface in front of it.
    var nearest = xy;
    var nearest_depth = -1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let at = clamp(xy + vec2(x, y), vec2(0), last);
            let c = ycocg(compress(textureLoad(color, at, 0).rgb));
            if x == 0 && y == 0 { current = c; }
            mean += c;
            square += c * c;
            let d = textureLoad(depth, at, 0);
            if d > nearest_depth { nearest_depth = d; nearest = at; }
        }
    }
    mean /= 9.0;
    let spread = sqrt(max(square / 9.0 - mean * mean, vec3(0.0))) * TAA_SPREAD;
    let previous = p.xy / size - textureLoad(vectors, nearest, 0).xy;
    var resolved = current;
    let reset = params.jitter_debug.w != 0.0;
    if !reset && all(previous >= vec2(0.0)) && all(previous <= vec2(1.0)) {
        let past = clip(ycocg(compress(sharp_history(previous, size))), mean, spread);
        resolved = mix(past, current, TAA_BLEND);
    }
    let image = vec4(expand(rgb(resolved)), 1.0);
    return Resolved(image, image);
}
