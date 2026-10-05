// Impostor LOD (crates/engine/src/tree_impostor.rs): each instance is four vertices at its
// root, spread here into a quad facing the camera. The baked views lie on a hemi-octahedral
// grid (YarraVegetation scripts/bake_impostor.py): the four views nearest the camera's
// direction are blended, then lit with the trees' crown shading.
#import bevy_pbr::{
    mesh_functions,
    mesh_view_bindings::view,
    view_transformations::position_world_to_clip,
}
#ifdef PREPASS_PIPELINE
#import bevy_pbr::prepass_io::FragmentOutput
#ifdef MOTION_VECTOR_PREPASS
#import bevy_pbr::prepass_bindings::previous_view_uniforms
#endif
#else
#import bevy_pbr::{
    forward_io::FragmentOutput,
    pbr_types,
    pbr_functions::{calculate_view, main_pass_post_lighting_processing},
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
}
#import "shaders/clouds/pbr_lighting.wgsl"::apply_pbr_lighting
#ifdef CROSSFADE_SAMPLE_MASK
#import "shaders/crossfade.wgsl"::crossfade_sample_mask

// The forward output with the crossfade's covered samples.
struct CrossfadeOutput {
    @location(0) color: vec4<f32>,
    @builtin(sample_mask) sample_mask: u32,
}
#endif
#endif

struct ImpostorParams {
    // Object-space centre of the baked views and their half size.
    centre_radius: vec4<f32>,
    // The part of every view any view covers: u0, v0, u1, v1 (v down); quads span only it.
    crop: vec4<f32>,
    // Views per side, pixels per metre of the LOD projection, orthographic flag, and the
    // farthest hand-off from the mesh LODs in metres.
    settings: vec4<f32>,
}
// object_lod.rs: every LOD switch crossfades over 10% either side of its distance.
const CROSSFADE_FRACTION: f32 = 0.1;
// Quads smaller than this many pixels across are not drawn.
const MINIMUM_PIXELS: f32 = 1.0;
@group(#{MATERIAL_BIND_GROUP}) @binding(130) var<uniform> impostor: ImpostorParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(131) var albedo_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(132) var impostor_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(133) var normal_texture: texture_2d<f32>;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    // Yaw, uniform scale, switch (height over threshold), quad corner 0-3.
    @location(10) instance: vec4<f32>,
}

struct Varyings {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    // Position across a view (0-1, v down).
    @location(1) corner: vec2<f32>,
    // Lowest of the four nearest views (column, row) and the blend toward the next ones.
    @location(2) @interpolate(flat) frame: vec4<f32>,
    @location(3) @interpolate(flat) right: vec3<f32>,
    @location(4) @interpolate(flat) up: vec3<f32>,
    @location(5) @interpolate(flat) toward_view: vec3<f32>,
    @location(6) @interpolate(flat) dither: i32,
}

// Bevy's Quat::from_rotation_y.
fn rotate_y(v: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

// Bevy's visibility-range dither level for a fade-in band [start, end] (object_lod.rs):
// -16 before it (hidden), rising to 0 after it, so it complements the mesh LOD fading out.
fn fade_in_level(distance: f32, start: f32, end: f32) -> i32 {
    let level = i32(round((distance - start) / max(end - start, 1e-4) * 16.0));
    return -16 + clamp(level, 0, 16);
}

@vertex
fn vertex(v: Vertex) -> Varyings {
    var out: Varyings;
    let world_from_local = mesh_functions::get_world_from_local(v.instance_index);
    let root = (world_from_local * vec4(v.position, 1.0)).xyz;
    let yaw = v.instance.x;
    let scale = v.instance.y;
    let centre = root + rotate_y(impostor.centre_radius.xyz * scale, yaw);
    let radius = impostor.centre_radius.w * scale;
    let orthographic = impostor.settings.z > 0.5;

    // Shown beyond the distance where the mesh LOD hands over, measured like Bevy's
    // visibility ranges: from the LOD view position to the object's origin.
    let switch_distance = v.instance.z * impostor.settings.y;
    if orthographic {
        out.dither = select(-16, 0, switch_distance <= 1.0);
    } else {
        let handoff = min(switch_distance, impostor.settings.w);
        let band = handoff * CROSSFADE_FRACTION;
        let distance = length(view.lod_view_world_position.xyz - root);
        out.dither = fade_in_level(distance, handoff - band, handoff + band);
        if 2.0 * radius * impostor.settings.y < MINIMUM_PIXELS * distance {
            out.dither = -16;
        }
    }
    if out.dither <= -16 {
        // All four corners coincide: nothing to rasterize.
        out.position = vec4(0.0, 0.0, 0.0, 1.0);
        out.world_position = vec4(root, 1.0);
        return out;
    }

    var toward_view: vec3<f32>;
    if orthographic {
        toward_view = normalize(view.world_from_view[2].xyz);
    } else {
        toward_view = normalize(view.world_position - centre);
    }
    // The bake's frame: right = up x view direction, image up = view direction x right.
    var right = cross(vec3(0.0, 1.0, 0.0), toward_view);
    if dot(right, right) < 1e-8 {
        right = rotate_y(vec3(1.0, 0.0, 0.0), yaw);
    }
    right = normalize(right);
    let up = cross(toward_view, right);
    let index = u32(v.instance.w);
    let quad = vec2(f32(index == 1u || index == 2u), f32(index >= 2u));
    let corner = mix(impostor.crop.xy, impostor.crop.zw, quad);
    let offset = right * ((corner.x * 2.0 - 1.0) * radius) + up * ((1.0 - corner.y * 2.0) * radius);
    out.world_position = vec4(centre + offset, 1.0);
    out.position = position_world_to_clip(out.world_position.xyz);

    // The view direction in object space, below the horizon clamped to it, on the grid.
    let d = rotate_y(toward_view, -yaw);
    let h = vec3(d.x, max(d.y, 0.0), d.z);
    let q = h / max(abs(h.x) + abs(h.y) + abs(h.z), 1e-6);
    let p = vec2(q.x + q.z, q.x - q.z);
    let views = impostor.settings.x;
    let grid = (p * 0.5 + 0.5) * views - 0.5;
    let cell = clamp(floor(grid), vec2(0.0), vec2(views - 2.0));
    out.frame = vec4(cell, clamp(grid - cell, vec2(0.0), vec2(1.0)));
    out.corner = corner;
    out.right = right;
    out.up = up;
    out.toward_view = toward_view;
    return out;
}

const DITHER_THRESHOLD_MAP: vec4<u32> = vec4(0x0a020800, 0x060e040c, 0x09010b03, 0x050d070f);

// Bevy's visibility_range_dither, so impostor and mesh crossfades share one pattern.
fn dither_discard(frag_coord: vec4<f32>, dither: i32) {
    if dither == 0 {
        return;
    }
    if dither <= -16 || dither >= 16 {
        discard;
    }
    let coords = vec2<u32>(floor(frag_coord.xy)) % 4u;
    let threshold = i32((DITHER_THRESHOLD_MAP[coords.y] >> (coords.x * 8u)) & 0xffu);
    if (dither >= 0 && dither + threshold >= 16) || (dither < 0 && 1 + dither + threshold <= 0) {
        discard;
    }
}

struct Surface {
    colour: vec3<f32>,
    normal: vec3<f32>,
    occlusion: f32,
}

// Coverage-weighted blend of the four nearest views; discards outside the crown before
// reading any normal, since most of a quad lies outside it. The quad's gradients pick each
// view's mip for its on-screen size.
fn surface(in: Varyings, ddx_uv: vec2<f32>, ddy_uv: vec2<f32>) -> Surface {
    let corner = clamp(in.corner, vec2(0.0), vec2(1.0));
    let count = impostor.settings.x;
    var uvs: array<vec2<f32>, 4>;
    var weights: vec4<f32>;
    var colour = vec3(0.0);
    var alpha = 0.0;
    for (var i = 0u; i < 4u; i += 1u) {
        let step = vec2(f32(i & 1u), f32(i >> 1u));
        let weight = mix(1.0 - in.frame.z, in.frame.z, step.x) * mix(1.0 - in.frame.w, in.frame.w, step.y);
        uvs[i] = (in.frame.xy + step + corner) / count;
        let a = textureSampleGrad(albedo_texture, impostor_sampler, uvs[i], ddx_uv, ddy_uv);
        weights[i] = a.a * weight;
        colour += a.rgb * weights[i];
        alpha += weights[i];
    }
    if alpha < 0.5 {
        discard;
    }
    var normal = vec3(0.0);
    var occlusion = 0.0;
    for (var i = 0u; i < 4u; i += 1u) {
        let n = textureSampleGrad(normal_texture, impostor_sampler, uvs[i], ddx_uv, ddy_uv);
        normal += (n.xyz * 2.0 - 1.0) * weights[i];
        occlusion += n.w * weights[i];
    }
    var out: Surface;
    out.colour = clamp(colour / alpha, vec3(0.0), vec3(1.0));
    // Opposed views can cancel; a zero normal must never reach lighting as NaN.
    let length_n = length(normal);
    let n = select(vec3(0.0, 0.0, 1.0), normal / length_n, length_n > 1e-4);
    out.normal = normalize(in.right * n.x + in.up * n.y + in.toward_view * n.z);
    out.occlusion = clamp(occlusion / alpha, 0.0, 1.0);
    return out;
}

#ifdef PREPASS_PIPELINE
@fragment
fn fragment(in: Varyings) -> FragmentOutput {
#else
#ifdef CROSSFADE_SAMPLE_MASK
@fragment
fn fragment(in: Varyings) -> CrossfadeOutput {
#else
@fragment
fn fragment(in: Varyings) -> FragmentOutput {
#endif
#endif
    // Derivatives before any discard, while every fragment of the quad is still running.
    let ddx_uv = dpdx(in.corner) / impostor.settings.x;
    let ddy_uv = dpdy(in.corner) / impostor.settings.x;
#ifndef PREPASS_PIPELINE
#ifdef CROSSFADE_SAMPLE_MASK
    let crossfade_mask = crossfade_sample_mask(in.position, in.dither);
    if crossfade_mask == 0u {
        discard;
    }
#else
    dither_discard(in.position, in.dither);
#endif
#else
    dither_discard(in.position, in.dither);
#endif
    let s = surface(in, ddx_uv, ddy_uv);
    var out: FragmentOutput;
#ifdef PREPASS_PIPELINE
#ifdef NORMAL_PREPASS
    out.normal = vec4(s.normal * 0.5 + 0.5, 1.0);
#endif
#ifdef MOTION_VECTOR_PREPASS
    // Objects are static; the quad turns with the camera, which reprojection ignores.
    let clip = view.unjittered_clip_from_world * in.world_position;
    let previous = previous_view_uniforms.clip_from_world * in.world_position;
    out.motion_vector = (clip.xy / clip.w - previous.xy / previous.w) * vec2(0.5, -0.5);
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.position.z;
#endif
    return out;
#else
    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.material.base_color = vec4(s.colour, 1.0);
    pbr_input.material.perceptual_roughness = 0.85;
    // Foliage keeps half the default reflectance, like the mesh crowns.
    pbr_input.material.reflectance *= 0.5;
    // Crown occlusion: indirect light, and the crown self-shadow past the shadow range.
    pbr_input.diffuse_occlusion = vec3(s.occlusion);
    pbr_input.specular_occlusion = s.occlusion;
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = s.normal;
    pbr_input.N = s.normal;
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.V = calculate_view(in.world_position, pbr_input.is_orthographic);
    pbr_input.flags = MESH_FLAGS_SHADOW_RECEIVER_BIT;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
#ifdef CROSSFADE_SAMPLE_MASK
    return CrossfadeOutput(out.color, crossfade_mask);
#else
    return out;
#endif
#endif
}
