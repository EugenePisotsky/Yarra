// Terrain v2's surface (crates/terrain_render/src/clipmap): one material function at every
// distance. Per pixel, the control clipmap names the ground's three heaviest surfaces (dithered
// between two levels by the pixel's footprint, world::terrain_control); they blend by their
// heights (terrain/blend.wesl). Ground grass mostly covers takes one plain colour sample
// under its field's colour; other surfaces sample the pack's textures, with anti-tiling and their
// normal maps near the camera and one sample each further out. The normal under them comes per pixel
// from the height clipmap at the pixel's footprint. Soft ground takes the prints feet pressed into
// it (terrain/prints.wesl).
import package::shaders::lighting::pbr_lighting::apply_pbr_lighting;
import package::shaders::terrain::clipmap_common::{clipmap_normal, intervals, leaf_height, leaf_spacing, levels, node_of, origin_samples, resident_level, settings, wear_slot};
import package::shaders::terrain::wear::{WEAR_TEXEL, bare_share, rut_at, rut_parallax, wear_at, wear_patch, wheel_grooves};
import package::shaders::terrain::prints::{print_parallax, print_relief, print_relief_slope, print_view};
import package::shaders::weather::ground_wetness::apply_rain_puddles;
import package::shaders::lighting::surface::{ground_celestial_irradiance, surface_weather};
import package::shaders::terrain::stochastic::{quarter_turn, stochastic_vertex_turn, stochastic_vertex_offset};
import package::shaders::terrain::blend::{BLEND_DEPTH, BLEND_HEIGHT, height_blend};
import bevy_pbr::render::mesh_view_bindings::{globals, lights, view};
import bevy_render::maths::PI;
import bevy_pbr::{
    render::forward_io::{FragmentOutput, VertexOutput},
    render::pbr_fragment::pbr_input_from_vertex_output,
    render::pbr_functions::main_pass_post_lighting_processing,
    render::pbr_types,
};

// crates/terrain_render/src/clipmap/gpu.rs ClipmapSurface, by texture array layer.
struct ClipmapSurface { texture: vec4<f32>, material: vec4<f32>, grass: vec4<f32>, soft: vec4<f32>, }
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var control: texture_2d_array<u32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var<storage, read> surfaces: array<ClipmapSurface>;
@group(constants::MATERIAL_BIND_GROUP) @binding(7) var base_color: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var repeat_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(9) var normal_material: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(10) var macro_variation: texture_2d<f32>;
// Road wear of the cells around the camera, one layer per toroidal slot (terrain/wear.wesl).
@group(constants::MATERIAL_BIND_GROUP) @binding(11) var wear_pages: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(12) var wear_sampler: sampler;

// Past this distance each surface is sampled once and shades by the clipmap's normal alone: the
// scans' normals and anti-tiling blend are lost in their coarse mips there.
const FAR_GROUND: f32 = 120.0;
// Ground between grass blades: close by it keeps most of its own texture, a little of the field's
// colour and the blades' shade; over UNDER_RANGE, as blades thin out, more of the field's colour
// and its shade; over FIELD_RANGE, past the blades, the field's colour unshaded. Pairs (close, at UNDER_RANGE's end).
const UNDER_RANGE: vec2<f32> = vec2(5.0, 30.0);
const UNDER_TINT: vec2<f32> = vec2(0.25, 0.5);
const UNDER_SHADE: vec2<f32> = vec2(0.8, 0.55);
const FIELD_RANGE: vec2<f32> = vec2(44.0, 76.0);
const FIELD_STRENGTH: f32 = 0.85;
// Wheel ruts sink by parallax within this range, fading out over its second half; worn edges
// darken out to the wear window (two cells or more around the camera's).
const RUT_RANGE: vec2<f32> = vec2(20.0, 35.0);
const RUT_DEEPEST: f32 = 0.16;
const WEAR_FADE: vec2<f32> = vec2(40.0, 56.0);
// world::TERRAIN_HOLLOW_RADIUS_METRES and TERRAIN_HOLLOW_DEPTH_METRES: rain stands where the ground
// lies below its surroundings by this much over this radius.
const HOLLOW_RADIUS: f32 = 1.5;
const HOLLOW_DEPTH: f32 = 0.15;
// weather/ground_wetness.wesl PUDDLE_DISTANCE.
const PUDDLE_DISTANCE: f32 = 80.0;
// Prints: drawn within the coarse window's reach, sunk by parallax within this range (fading over
// its second half) and at most this deep; how deep a print packs the ground fully, and what packing
// does to it.
const PRINT_DISTANCE: f32 = 100.0;
const PRINT_SINK_RANGE: vec2<f32> = vec2(20.0, 35.0);
const PRINT_DEEPEST: f32 = 0.2;
const PRINT_PACKED: f32 = 0.06;
// Steepest a print's wall tilts the ground (tan 56°): snow scatters light into its own shade, so
// walls drawn as steep as the relief went near black facing away from the sun.
const PRINT_STEEPEST: f32 = 1.5;
const PACKED_TINT: vec3<f32> = vec3(0.8, 0.86, 0.95);
const PACKED_SMOOTHING: f32 = 0.7;
const PACKED_FLATTENING: f32 = 0.3;

// Where rain stands on the ground: how far it lies below two rings around it.
fn terrain_hollow(leaf: vec2<i32>, fraction: vec2<f32>) -> f32 {
    let centre = leaf_height(leaf, fraction);
    let radius = HOLLOW_RADIUS / leaf_spacing();
    var sum = 0.0;
    for (var i = 0u; i < 12u; i += 1u) {
        let inner = i < 4u;
        let angle = 6.2831855 * select((f32(i - 4u) + 0.5) / 8.0, f32(i) / 4.0, inner);
        let r = select(radius, radius * 0.5, inner);
        sum += leaf_height(leaf, fraction + vec2(cos(angle), sin(angle)) * r);
    }
    return clamp((sum / 12.0 - centre) / HOLLOW_DEPTH, 0.0, 1.0);
}

// A packed control sample (world::pack_control): three layers of five bits, two shares of 255.
struct Sample { layers: vec3<u32>, shares: vec3<f32>, }

fn unpack(sample: u32) -> Sample {
    let a = f32((sample >> 16u) & 255u) / 255.0;
    let b = f32((sample >> 24u) & 255u) / 255.0;
    return Sample(vec3(sample & 31u, (sample >> 5u) & 31u, (sample >> 10u) & 31u),
        vec3(a, b, max(1.0 - a - b, 0.0)));
}

// A layer's share of a sample.
fn share(s: Sample, layer: u32) -> f32 {
    return dot(select(vec3(0.0), s.shares, s.layers == vec3(layer)), vec3(1.0));
}

struct Top { layers: vec3<u32>, weights: vec3<f32>, }

fn soft_depths(top: Top) -> vec3<f32> {
    return vec3(surfaces[top.layers.x].soft.x, surfaces[top.layers.y].soft.x, surfaces[top.layers.z].soft.x);
}

// How far feet could press into the ground's three heaviest surfaces, by their shares.
fn soft_depth(top: Top) -> f32 {
    return dot(top.weights, soft_depths(top)) / max(dot(top.weights, vec3(1.0)), 0.000001);
}

// How far feet press into the surface that shows at world XZ `p`: the three blended by their
// heights, one plain sample each, as the ground draws them. By shares alone, grass growing over
// a little snow took whole prints.
fn shown_soft_depth(top: Top, p: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> f32 {
    var heights = vec3(0.0);
    for (var i = 0; i < 3; i += 1) {
        if top.weights[i] > 0.0 {
            let layer = top.layers[i];
            let size = surfaces[layer].texture.x;
            heights[i] = textureSampleGrad(base_color, repeat_sampler, p / size, i32(layer), dx / size, dy / size).a;
        }
    }
    return dot(height_blend(top.weights, heights), soft_depths(top));
}

// Ranks a candidate into the three heaviest; a layer already ranked is not ranked again.
fn rank(top: Top, layer: u32, weight: f32) -> Top {
    if any((top.layers == vec3(layer)) & (top.weights > vec3(0.0))) {
        return top;
    }
    if weight > top.weights.x {
        return Top(vec3(layer, top.layers.xy), vec3(weight, top.weights.xy));
    }
    if weight > top.weights.y {
        return Top(vec3(top.layers.x, layer, top.layers.y), vec3(top.weights.x, weight, top.weights.y));
    }
    if weight > top.weights.z {
        return Top(vec3(top.layers.xy, layer), vec3(top.weights.xy, weight));
    }
    return top;
}

// The three heaviest surfaces of a level's four control samples around a leaf sample plus a
// fraction of one, bilinearly. Candidates are each sample's heaviest surface and the nearest
// sample's other two: a surface none of them names weighs too little to show. Control samples
// lie half a height sample apart at every level.
fn level_surfaces(level: u32, leaf: vec2<i32>, fraction: vec2<f32>) -> Top {
    let units = leaf * 2;
    let first = units >> vec2(level);
    let rest = units - (first << vec2(level));
    let q = (vec2<f32>(rest) + fraction * 2.0) / f32(1u << level);
    let whole = floor(q);
    let f = q - whole;
    let mask = vec2(i32(settings.ground.z) - 1);
    let t0 = (first + vec2<i32>(whole)) & mask;
    let t1 = (first + vec2<i32>(whole) + 1) & mask;
    let l = i32(level);
    let p00 = textureLoad(control, t0, l, 0).r;
    let p10 = textureLoad(control, vec2(t1.x, t0.y), l, 0).r;
    let p01 = textureLoad(control, vec2(t0.x, t1.y), l, 0).r;
    let p11 = textureLoad(control, t1, l, 0).r;
    let s00 = unpack(p00);
    let s10 = unpack(p10);
    let s01 = unpack(p01);
    let s11 = unpack(p11);
    let w = vec4((1.0 - f.x) * (1.0 - f.y), f.x * (1.0 - f.y), (1.0 - f.x) * f.y, f.x * f.y);
    let near = unpack(select(select(p00, p10, w.y > w.x), select(p01, p11, w.w > w.z), max(w.z, w.w) > max(w.x, w.y)));
    let candidates = array<u32, 6>(s00.layers.x, s10.layers.x, s01.layers.x, s11.layers.x, near.layers.y, near.layers.z);
    var top = Top(vec3(s00.layers.x), vec3(0.0));
    for (var i = 0u; i < 6u; i += 1u) {
        let c = candidates[i];
        let weight = dot(w, vec4(share(s00, c), share(s10, c), share(s01, c), share(s11, c)));
        top = rank(top, c, weight);
    }
    return top;
}

// The ground's three heaviest surfaces for a footprint of 2^lod control samples: a level's
// samples, or over the last part of its octave the next coarser level's at a share of pixels
// that grows across it (the temporal upscaler resolves the dither into a blend).
fn ground_surfaces(leaf: vec2<i32>, fraction: vec2<f32>, lod: f32, pixel: vec2<f32>) -> Top {
    let top = levels() - 1u;
    let fine = resident_level(u32(clamp(floor(lod), 0.0, f32(top))), leaf);
    let coarse = resident_level(min(fine + 1u, top), leaf);
    let t = smoothstep(0.6, 1.0, lod - f32(fine));
    let frame = f32(globals.frame_count % 64u);
    let noise = fract(52.9829189 * fract(dot(pixel + 5.588238 * frame, vec2(0.06711056, 0.00583715))));
    return level_surfaces(select(fine, coarse, noise < t), leaf, fraction);
}

fn triangular(uv: vec2<f32>) -> vec2<f32> { return vec2(uv.x + uv.y * 0.5773502692, uv.y * 1.1547005384); }

// Anti-tiling from the lattice vertex with the largest weight alone: one sample instead of three.
fn stochastic_dominant(uv: vec2<f32>, lattice: vec2<f32>, layer: i32, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {
    let c = floor(lattice);
    let f = fract(lattice);
    var vertex = c;
    if f.x + f.y > 1.0 {
        vertex = c + select(select(vec2(1.0, 0.0), vec2(0.0, 1.0), 1.0 - f.x > 1.0 - f.y), vec2(1.0), f.x + f.y - 1.0 > max(1.0 - f.x, 1.0 - f.y));
    } else {
        vertex = c + select(select(vec2(0.0), vec2(1.0, 0.0), f.x > 1.0 - f.x - f.y), vec2(0.0, 1.0), f.y > max(f.x, 1.0 - f.x - f.y));
    }
    let turn = stochastic_vertex_turn(vertex, layer);
    return textureSampleGrad(base_color, repeat_sampler, quarter_turn(uv, turn) + stochastic_vertex_offset(vertex, layer), layer, quarter_turn(dx, turn), quarter_turn(dy, turn));
}

fn stochastic(uv: vec2<f32>, lattice: vec2<f32>, layer: i32, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {
    let c = floor(lattice);
    let f = fract(lattice);
    var vertices = array<vec2<f32>, 3>(c, c + vec2(1.0, 0.0), c + vec2(0.0, 1.0));
    var weights = vec3(1.0 - f.x - f.y, f.x, f.y);
    if f.x + f.y > 1.0 {
        vertices = array<vec2<f32>, 3>(c + vec2(1.0), c + vec2(0.0, 1.0), c + vec2(1.0, 0.0));
        weights = vec3(f.x + f.y - 1.0, 1.0 - f.x, 1.0 - f.y);
    }
    weights = weights * weights * weights * weights;
    weights /= max(dot(weights, vec3(1.0)), 0.000001);
    var result = vec4(0.0);
    for (var i = 0u; i < 3u; i += 1u) {
        let turn = stochastic_vertex_turn(vertices[i], layer);
        result += textureSampleGrad(base_color, repeat_sampler, quarter_turn(uv, turn) + stochastic_vertex_offset(vertices[i], layer), layer, quarter_turn(dx, turn), quarter_turn(dy, turn)) * weights[i];
    }
    return result;
}

// `color.a` is the surface's blend height; `normal` is in the ground's tangent space.
struct Surface { color: vec4<f32>, normal: vec3<f32>, ao: f32, roughness: f32, }

// The vertical plane a steep slope faces more: its XY or ZY coordinates and their derivatives.
struct Side { uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, }
fn side_projection(p: vec3<f32>, n: vec3<f32>, dx: vec3<f32>, dy: vec3<f32>) -> Side {
    if abs(n.x) > abs(n.z) {
        return Side(vec2(p.z, -p.y), vec2(dx.z, -dx.y), vec2(dy.z, -dy.y));
    }
    return Side(vec2(p.x, -p.y), vec2(dx.x, -dx.y), vec2(dy.x, -dy.y));
}

// Where the ground's view from above ends: the share of a slope's colour that comes from the
// side projection, sharpened so only steep ground takes it.
fn side_share(n: vec3<f32>) -> f32 {
    let w = pow(vec2(abs(n.y), max(abs(n.x), abs(n.z))), vec2(4.0));
    return w.y / (w.x + w.y);
}

// A surface at world XZ `p`, seen from above, and on steep ground also from the side it faces
// (biplanar: a texture projected from above alone stretches down cliffs); `side` is that share,
// `world` the point and its derivatives. `under`: how much of the grass field's colour, and of the
// blades' shade, fully grassed ground takes here.
fn surface(layer: u32, p: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, far: bool, under: vec2<f32>, side: f32, plane: Side) -> Surface {
    let s = surfaces[layer];
    let flat_normal = vec3(0.0, 0.0, 1.0);
    let rough = mix(s.material.x, s.material.y, 0.5);
    let size = s.texture.x;
    let l = i32(layer);
    let uv = p / size;
    let uv_dx = dx / size;
    let uv_dy = dy / size;
    let cover = clamp(s.grass.a, 0.0, 1.0);
    // Covered ground: grass hides most of it, so its colour alone, one plain sample, under the
    // field's colour and shade.
    if s.material.z > 0.5 {
        let color = textureSampleGrad(base_color, repeat_sampler, uv, l, uv_dx, uv_dy);
        let ground = mix(color.rgb, s.grass.rgb, under.x * cover) * mix(1.0, under.y, cover);
        return Surface(vec4(ground, color.a), flat_normal, 1.0, rough);
    }
    var color: vec4<f32>;
    if s.texture.y >= 0.5 && far {
        color = stochastic_dominant(uv, triangular(uv), l, uv_dx, uv_dy);
    } else if s.texture.y >= 0.5 {
        color = stochastic(uv, triangular(uv), l, uv_dx, uv_dy);
    } else {
        color = textureSampleGrad(base_color, repeat_sampler, uv, l, uv_dx, uv_dy);
    }
    // Steep ground: colour and blend height partly from the side, one plain sample.
    if side > 0.02 {
        let from_side = textureSampleGrad(base_color, repeat_sampler, plane.uv / size, l, plane.dx / size, plane.dy / size);
        color = mix(color, from_side, side);
    }
    // Ground partly grassed shows the field between its own texture.
    color = vec4(mix(color.rgb, s.grass.rgb, under.x * cover) * mix(1.0, under.y, cover), color.a);
    if far {
        return Surface(color, flat_normal, 1.0, rough);
    }
    let packed = textureSampleGrad(normal_material, repeat_sampler, uv, l, uv_dx, uv_dy);
    let oct = packed.rg * 2.0 - 1.0;
    var n = vec3(oct, 1.0 - abs(oct.x) - abs(oct.y));
    let fold = clamp(-n.z, 0.0, 1.0);
    n.x += select(fold, -fold, n.x >= 0.0);
    n.y += select(fold, -fold, n.y >= 0.0);
    n = normalize(n);
    n.y *= s.texture.w;
    n = normalize(vec3(n.xy * clamp(s.texture.z, 0.0, 1.0) * (1.0 - side), max(n.z, 0.001)));
    return Surface(color, n, packed.b, mix(s.material.x, s.material.y, packed.a));
}

fn rotate_medium(p: vec2<f32>) -> vec2<f32> { return vec2(p.x * 0.819 - p.y * 0.574, p.x * 0.574 + p.y * 0.819); }
fn rotate_large(p: vec2<f32>) -> vec2<f32> { return vec2(p.x * -0.342 - p.y * 0.940, p.x * 0.940 + p.y * -0.342); }

// Large-scale brightness variation over three scales.
fn macro_response(p: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> f32 {
    if settings.ground.y < 0.5 {
        return 1.0;
    }
    let s = settings.macro_scales;
    let a = textureSampleGrad(macro_variation, repeat_sampler, p / s.x + vec2(0.11, 0.73), dx / s.x, dy / s.x).r;
    let b = textureSampleGrad(macro_variation, repeat_sampler, rotate_medium(p) / s.y + vec2(0.37, 0.61), rotate_medium(dx) / s.y, rotate_medium(dy) / s.y).r;
    let c = textureSampleGrad(macro_variation, repeat_sampler, rotate_large(p) / s.z + vec2(0.83, 0.19), rotate_large(dx) / s.z, rotate_large(dy) / s.z).r;
    let signal = dot(vec3(a, b, c) * 2.0 - 1.0, vec3(0.28, 0.36, 0.36));
    return exp2(clamp(signal * settings.ground.x, -1.0, 1.0) * s.w);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var out: FragmentOutput;
    let world_dx = dpdx(in.world_position.xz);
    let world_dy = dpdy(in.world_position.xz);
    // Geometric mean of the footprint's axes: the long axis at grazing angles alone would
    // blur the whole distance.
    let footprint = sqrt(length(world_dx) * length(world_dy));
    let r = in.world_position.xz / leaf_spacing();
    let cell = floor(r);
    let leaf = origin_samples() + vec2<i32>(cell);
    let fraction = r - cell;
    let lod = log2(max(footprint / leaf_spacing(), 1e-6)) + settings.ranges.w;
    let n = clipmap_normal(leaf, fraction, lod);
    // World XZ; f32 holds it to a millimetre or better within 8 km of the world's origin.
    let p = (vec2<f32>(leaf) + fraction) * leaf_spacing();
    let distance_to_camera = distance(view.world_position, in.world_position.xyz);
    let far = distance_to_camera > FAR_GROUND;
    let field = smoothstep(FIELD_RANGE.x, FIELD_RANGE.y, distance_to_camera);
    let thinning = smoothstep(UNDER_RANGE.x, UNDER_RANGE.y, distance_to_camera);
    let under = vec2(mix(mix(UNDER_TINT.x, UNDER_TINT.y, thinning), FIELD_STRENGTH, field),
        mix(mix(UNDER_SHADE.x, UNDER_SHADE.y, thinning), 1.0, field));

    // Control samples are half a leaf apart: one octave finer than the heights.
    var top = ground_surfaces(leaf, fraction, lod + 1.0, in.position.xy);

    // Roads near the camera: the wear field sinks ruts by parallax
    // (surfaces are sampled where the eye meets the rut floor), tilts the ground with their
    // walls, shades their floors and scores them with grooves, and darkens worn edges.
    var sampled = p;
    var ground_normal = n;
    var occlusion = 1.0;
    var rim = 0.0;
    var compacted = 0.0;
    var hollow = 0.0;
    // Fine wear here, the wear as the half-metre control samples saw it, and how much to trust
    // the fine field (fading with the wear window).
    var refine = vec3(0.0);
    let wet = surface_weather().x > 0.3 && distance_to_camera < PUDDLE_DISTANCE;
    if distance_to_camera < WEAR_FADE.y {
        let cell = node_of(0u, leaf);
        let layer = wear_slot(cell);
        let size = leaf_spacing() * f32(intervals());
        if layer >= 0 {
            let uv = (vec2<f32>(leaf - cell * intervals()) + fraction) / f32(intervals());
            let field = wear_at(wear_pages, wear_sampler, layer, uv);
            // Over half a control sample each way: the wear the control samples averaged.
            let q = 0.25 / size;
            let blurred = (wear_at(wear_pages, wear_sampler, layer, uv + vec2(q, q)).wear
                + wear_at(wear_pages, wear_sampler, layer, uv + vec2(-q, q)).wear
                + wear_at(wear_pages, wear_sampler, layer, uv + vec2(q, -q)).wear
                + wear_at(wear_pages, wear_sampler, layer, uv + vec2(-q, -q)).wear) * 0.25;
            refine = vec3(field.wear, blurred, 1.0 - smoothstep(WEAR_FADE.x, WEAR_FADE.y, distance_to_camera));
            if field.wear > 0.002 || field.rut > 0.0005 {
                let fade = 1.0 - smoothstep(WEAR_FADE.x, WEAR_FADE.y, distance_to_camera);
                let worn_bare = field.wear * bare_share(field.wear, p);
                rim = worn_bare * (1.0 - worn_bare) * 4.0 * fade;
                compacted = worn_bare * 0.25 * fade;
                let sink = 1.0 - smoothstep(RUT_RANGE.x, RUT_RANGE.y, distance_to_camera);
                if sink > 0.0 && field.wear > 0.05 {
                    let to_eye = normalize(view.world_position - in.world_position.xyz);
                    let found = rut_parallax(wear_pages, wear_sampler, layer, uv, size, to_eye, n, RUT_DEEPEST * sink);
                    sampled = p + found.xy * sink;
                    let at = uv + found.xy * sink / size;
                    let e = WEAR_TEXEL;
                    let slope = vec2(
                        rut_at(wear_pages, wear_sampler, layer, at + vec2(e, 0.0)) - rut_at(wear_pages, wear_sampler, layer, at - vec2(e, 0.0)),
                        rut_at(wear_pages, wear_sampler, layer, at + vec2(0.0, e)) - rut_at(wear_pages, wear_sampler, layer, at - vec2(0.0, e))) / (2.0 * e * size) * sink;
                    ground_normal = normalize(n + vec3(slope.x, 0.0, slope.y));
                    let bottom = clamp(found.z / max(field.rut * 1.3, 0.02), 0.0, 1.0) * smoothstep(0.005, 0.03, found.z);
                    occlusion = 1.0 - 0.35 * bottom;
                    compacted = max(compacted, bottom * field.wear);
                    let fine = 1.0 - smoothstep(0.03, 0.07, max(length(world_dx), length(world_dy)));
                    if fine > 0.0 {
                        let g = wheel_grooves(sampled, field.direction);
                        occlusion *= 1.0 - 0.12 * g * abs(g) * fine * field.wear;
                    }
                    // Rain stands in dips of the rut floors a metre or three across.
                    if wet {
                        let dips = wear_patch(sampled, 2.0, 0x6e3u);
                        let wobble = wear_patch(sampled, 0.6, 0x51du) - 0.5;
                        hollow = clamp(bottom * bottom * dips * 1.5 - 0.55 + wobble * 0.35, 0.0, 1.0);
                    }
                }
            }
        }
    }
    if wet {
        hollow = max(hollow, terrain_hollow(leaf, fraction));
    }

    // Soft ground near the player: prints sink by parallax, tilt the ground with their walls,
    // and pack it, darker in their depths. Undisturbed ground shows itself: a view ray entering
    // it is already at the surface, so only ground with relief marches and takes slopes.
    var packed = 0.0;
    // The share of the ground's view a print's tilt turns from the sky to the snow around it.
    var turned = 0.0;
    if soft_depth(top) > 0.001 && distance_to_camera < PRINT_DISTANCE {
        let windows = print_view(in.world_position.xz);
        var soft = 0.0;
        if dot(windows.shares, vec2(1.0)) > 0.0 && abs(print_relief(windows, in.world_position.xz, PRINT_DEEPEST)) > 0.0001 {
            soft = shown_soft_depth(top, p, world_dx, world_dy);
        }
        if soft > 0.001 {
            var at = in.world_position.xz;
            let sink = 1.0 - smoothstep(PRINT_SINK_RANGE.x, PRINT_SINK_RANGE.y, distance_to_camera);
            if sink > 0.0 {
                let to_eye = normalize(view.world_position - in.world_position.xyz);
                let found = print_parallax(windows, at, soft, to_eye, ground_normal, min(soft, PRINT_DEEPEST) * sink);
                at += found.xy;
                sampled += found.xy;
            }
            let relief = print_relief_slope(windows, at, soft);
            let slope = relief.yz * min(1.0, PRINT_STEEPEST / max(length(relief.yz), 1e-6));
            let tilted = normalize(ground_normal - vec3(slope.x, 0.0, slope.y));
            turned = 0.5 - 0.5 * dot(tilted, ground_normal);
            ground_normal = tilted;
            let depth = max(-relief.x, 0.0);
            packed = smoothstep(0.0, PRINT_PACKED, depth);
            occlusion *= 1.0 - 0.25 * smoothstep(0.02, 0.15, depth);
        }
    }

    let side = side_share(n);
    let plane = side_projection(in.world_position.xyz, n, dpdx(in.world_position.xyz), dpdy(in.world_position.xyz));
    // Near the camera roads follow the fine wear field: the share of the ground worn bare
    // scales with how much more (or less) worn this point is than its half-metre neighbourhood,
    // so wheel tracks and the strip between them stay sharp.
    if refine.z > 0.0 && refine.y > 0.01 {
        let worn = vec3<f32>(vec3(surfaces[top.layers.x].material.w, surfaces[top.layers.y].material.w,
            surfaces[top.layers.z].material.w) > vec3(0.5));
        let total = dot(top.weights, vec3(1.0));
        let bare = dot(top.weights, worn);
        if bare > 0.001 && bare < total - 0.001 {
            let wanted = clamp(bare * mix(1.0, refine.x / refine.y, refine.z), 0.0, total);
            top.weights = mix(top.weights * (total - wanted) / (total - bare), top.weights * wanted / bare, worn);
        }
    }
    // A surface shows only where its score (weight share plus height) comes within the blend
    // depth of the best; one that cannot, even at full height, is not sampled.
    let share = top.weights / max(dot(top.weights, vec3(1.0)), 0.000001);
    let a = surface(top.layers.x, sampled, world_dx, world_dy, far, under, side, plane);
    var b = a;
    var c = a;
    var heights = vec3(a.color.a, -1.0e3, -1.0e3);
    var cut = share.x + a.color.a * BLEND_HEIGHT - BLEND_DEPTH;
    if top.weights.y > 0.0 && share.y + BLEND_HEIGHT > cut {
        b = surface(top.layers.y, sampled, world_dx, world_dy, far, under, side, plane);
        heights.y = b.color.a;
        cut = max(cut, share.y + b.color.a * BLEND_HEIGHT - BLEND_DEPTH);
    }
    if top.weights.z > 0.0 && share.z + BLEND_HEIGHT > cut {
        c = surface(top.layers.z, sampled, world_dx, world_dy, far, under, side, plane);
        heights.z = c.color.a;
    }
    let w = height_blend(top.weights, heights);
    // Worn soil is darker and browner than loose soil of the same kind.
    let worn = mix(vec3(1.0), vec3(0.76, 0.7, 0.64), compacted) * (1.0 - 0.12 * rim);
    // Packed snow is smoother, its grains pressed flat, and its depths take the sky's blue.
    let color = (a.color.rgb * w.x + b.color.rgb * w.y + c.color.rgb * w.z) * worn * mix(vec3(1.0), PACKED_TINT, packed);
    let local_normal = normalize(mix(a.normal * w.x + b.normal * w.y + c.normal * w.z, vec3(0.0, 0.0, 1.0), PACKED_FLATTENING * packed));
    // World-X tangent with the heightfield's +Z bitangent.
    let t = normalize(vec3(ground_normal.y, -ground_normal.x, 0.0));
    let bt = normalize(cross(t, ground_normal));
    let normal = normalize(t * local_normal.x + bt * local_normal.y + ground_normal * local_normal.z);

    var pbr = pbr_input_from_vertex_output(in, is_front, false);
    pbr.material.base_color = vec4(clamp(color * macro_response(p, world_dx, world_dy), vec3(0.0), vec3(1.0)), 1.0);
    pbr.material.perceptual_roughness = clamp((a.roughness * w.x + b.roughness * w.y + c.roughness * w.z) * mix(1.0, PACKED_SMOOTHING, packed), 0.08, 1.0);
    pbr.material.metallic = 0.0;
    pbr.material.reflectance = vec3(0.25);
    pbr.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    let ao = (a.ao * w.x + b.ao * w.y + c.ao * w.z) * occlusion;
    pbr.diffuse_occlusion = vec3(ao);
    pbr.specular_occlusion = ao;
    pbr.N = normal;
    pbr.clearcoat_N = normal;
    // A print's walls face its sunlit floor and the snow around it, far brighter than the grass
    // the shared ground light assumes: they take light reflected by snow as bright as their own.
    if turned > 0.001 {
        let irradiance = lights.ambient_color.rgb * PI + ground_celestial_irradiance(in.world_position.xyz);
        let albedo = pbr.material.base_color.rgb;
        pbr.material.emissive = vec4(albedo * albedo * irradiance / PI * turned, 1.0);
    }
    pbr = apply_rain_puddles(pbr, hollow);
    let lit = apply_pbr_lighting(pbr);
    out.color = main_pass_post_lighting_processing(pbr, lit);
    return out;
}
