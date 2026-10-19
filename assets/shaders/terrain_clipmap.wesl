// Terrain v2's surface (crates/terrain_render/src/clipmap): one material function at every
// distance. Per pixel, the control clipmap names the ground's three heaviest surfaces (dithered
// between two levels by the pixel's footprint, world::terrain_control); they blend by their
// heights as near detail blends them. Ground grass mostly covers takes one plain colour sample
// under its field's colour; other surfaces sample the pack's textures, with anti-tiling and their
// normal maps near the camera and one sample each further out. The normal under them comes per pixel
// from the height clipmap at the pixel's footprint.
import package::shaders::lighting::pbr_lighting::apply_pbr_lighting;
import package::shaders::terrain_clipmap_common::{clipmap_normal, leaf_spacing, levels, origin_samples, resident_level, settings};
import package::shaders::terrain_stochastic::{quarter_turn, stochastic_vertex_turn, stochastic_vertex_offset};
import package::shaders::terrain_blend::height_blend;
import bevy_pbr::render::mesh_view_bindings::{globals, view};
import bevy_pbr::{
    render::forward_io::{FragmentOutput, VertexOutput},
    render::pbr_fragment::pbr_input_from_vertex_output,
    render::pbr_functions::main_pass_post_lighting_processing,
    render::pbr_types,
};

// crates/terrain_render/src/clipmap/gpu.rs ClipmapSurface, by texture array layer.
struct ClipmapSurface { texture: vec4<f32>, material: vec4<f32>, grass: vec4<f32>, }
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var control: texture_2d_array<u32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var<storage, read> surfaces: array<ClipmapSurface>;
@group(constants::MATERIAL_BIND_GROUP) @binding(7) var base_color: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var repeat_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(9) var normal_material: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(10) var macro_variation: texture_2d<f32>;

// Past this distance each surface is sampled once and shades by the clipmap's normal alone: the
// scans' normals and anti-tiling blend are lost in their coarse mips there.
const FAR_GROUND: f32 = 120.0;
// Ground between grass blades (as near detail, terrain_near.wesl): close by it keeps most of its
// own texture, a little of the field's colour and the blades' shade; over UNDER_RANGE, as blades
// thin out, more of the field's colour and its shade; over FIELD_RANGE, past the blades, the
// field's colour unshaded. Pairs (close, at UNDER_RANGE's end).
const UNDER_RANGE: vec2<f32> = vec2(5.0, 30.0);
const UNDER_TINT: vec2<f32> = vec2(0.25, 0.5);
const UNDER_SHADE: vec2<f32> = vec2(0.8, 0.55);
const FIELD_RANGE: vec2<f32> = vec2(44.0, 76.0);
const FIELD_STRENGTH: f32 = 0.85;

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

// A surface at world XZ `p`. `under`: how much of the grass field's colour, and of the blades'
// shade, fully grassed ground takes here.
fn surface(layer: u32, p: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, far: bool, under: vec2<f32>) -> Surface {
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
    n = normalize(vec3(n.xy * clamp(s.texture.z, 0.0, 1.0), max(n.z, 0.001)));
    return Surface(color, n, packed.b, mix(s.material.x, s.material.y, packed.a));
}

fn rotate_medium(p: vec2<f32>) -> vec2<f32> { return vec2(p.x * 0.819 - p.y * 0.574, p.x * 0.574 + p.y * 0.819); }
fn rotate_large(p: vec2<f32>) -> vec2<f32> { return vec2(p.x * -0.342 - p.y * 0.940, p.x * 0.940 + p.y * -0.342); }

// Large-scale brightness variation over three scales, as near detail's.
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
    let top = ground_surfaces(leaf, fraction, lod + 1.0, in.position.xy);
    let a = surface(top.layers.x, p, world_dx, world_dy, far, under);
    var b = a;
    var c = a;
    var heights = vec3(a.color.a, -1.0e3, -1.0e3);
    if top.weights.y > 0.0 {
        b = surface(top.layers.y, p, world_dx, world_dy, far, under);
        heights.y = b.color.a;
    }
    if top.weights.z > 0.0 {
        c = surface(top.layers.z, p, world_dx, world_dy, far, under);
        heights.z = c.color.a;
    }
    let w = height_blend(top.weights, heights);
    let color = a.color.rgb * w.x + b.color.rgb * w.y + c.color.rgb * w.z;
    let local_normal = normalize(a.normal * w.x + b.normal * w.y + c.normal * w.z);
    // World-X tangent with the heightfield's +Z bitangent, as near detail's.
    let t = normalize(vec3(n.y, -n.x, 0.0));
    let bt = normalize(cross(t, n));
    let normal = normalize(t * local_normal.x + bt * local_normal.y + n * local_normal.z);

    var pbr = pbr_input_from_vertex_output(in, is_front, false);
    pbr.material.base_color = vec4(clamp(color * macro_response(p, world_dx, world_dy), vec3(0.0), vec3(1.0)), 1.0);
    pbr.material.perceptual_roughness = clamp(a.roughness * w.x + b.roughness * w.y + c.roughness * w.z, 0.08, 1.0);
    pbr.material.metallic = 0.0;
    pbr.material.reflectance = vec3(0.25);
    pbr.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    let ao = a.ao * w.x + b.ao * w.y + c.ao * w.z;
    pbr.diffuse_occlusion = vec3(ao);
    pbr.specular_occlusion = ao;
    pbr.N = normal;
    pbr.clearcoat_N = normal;
    let lit = apply_pbr_lighting(pbr);
    out.color = main_pass_post_lighting_processing(pbr, lit);
    return out;
}
