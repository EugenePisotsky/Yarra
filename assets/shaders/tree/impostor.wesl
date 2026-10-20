// Impostor LOD (crates/engine/src/trees/impostor.rs): each instance is six vertices at its
// root, spread here into a quad facing the camera. The GPU trees (trees/gpu.rs) draw one quad
// mesh per instance record. The baked views lie on a hemi-octahedral
// grid (YarraVegetation scripts/bake_impostor.py), each stored as the tile of its crop: the
// four views nearest the camera's direction are blended, each read where the fragment's ray
// meets that view's plane, then lit with the trees' crown
// shading where the baked depth puts the crown's surface, not on the quad. Near the camera
// the quad bends in the wind as its mesh LODs' trunk does.
import bevy_pbr::{render::mesh_functions, render::mesh_view_bindings::view, render::view_transformations::position_world_to_clip};
// Impostors draw in the main pass only: no prepass (trees/impostor.rs `enable_prepass`) and
// no shadows.
import bevy_pbr::{render::pbr_types, render::pbr_functions::{calculate_view, main_pass_post_lighting_processing}, render::mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT};
import package::shaders::lighting::pbr_lighting::apply_pbr_lighting;
import package::shaders::crossfade::{crossfade_sample_mask, dither_discard};
import package::shaders::tree::wind::{WindFrames, sway_point_of_trunk};

// The forward pass's output (Bevy's forward_io::FragmentOutput), with the crossfade's covered
// samples where the view crossfades by sample mask.
struct ForwardOutput {
    @location(0) color: vec4<f32>,
    @if(CROSSFADE_SAMPLE_MASK)
    @builtin(sample_mask) sample_mask: u32,
}

struct ImpostorParams {
    // Object-space centre of the baked views and their half size.
    centre_radius: vec4<f32>,
    // The part of every view any view covers: u0, v0, u1, v1 (v down); quads span only it.
    crop: vec4<f32>,
    // Views per side, pixels per metre of the LOD projection, orthographic flag, unused.
    settings: vec4<f32>,
    // The far mesh LOD's wind profile (tree/material.wesl tree_profile); its stem height in
    // object metres (x), the views' texels per metre (y) and the LOD projection's output
    // pixels per pixel (z): z over y is the hand-off at unit scale (trees/impostor.rs).
    wind_profile: vec4<f32>,
    wind_height: vec4<f32>,
}
// Quads smaller than this many pixels across are not drawn.
const MINIMUM_PIXELS: f32 = 1.0;
@group(constants::MATERIAL_BIND_GROUP) @binding(130) var<uniform> impostor: ImpostorParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(131) var albedo_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(132) var impostor_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(133) var normal_texture: texture_2d<f32>;

// The GPU trees' records, instances and origins (trees/gpu.rs; tree/material.wesl has the same).
struct TreeInstance {
    position_yaw: vec4<f32>,
    scale: f32,
    form: u32,
    origin: u32,
    flags: u32,
}
@group(4) @binding(0) var<storage, read> tree_records: array<u32>;
@group(4) @binding(1) var<storage, read> tree_instances: array<TreeInstance>;
@group(4) @binding(2) var<storage, read> tree_origins: array<vec4<f32>>;

// The wind every tree shares (trees/wind.rs WindBuffer).
@group(constants::MATERIAL_BIND_GROUP) @binding(135) var<storage, read> wind: WindFrames;
// Impostors sway as fully as their mesh LODs where they take over from them, so the
// dissolve matches, then less with distance: not at all by WIND_SPAN times that distance,
// nor past WIND_END metres. A whole crown swinging as one reads as a card far away.
const WIND_SPAN: f32 = 2.5;
const WIND_END: f32 = 450.0;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // The quad corner (0-5: bottom, middle and top rows, left then right) as x.
    @location(0) position: vec3<f32>,
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
    // The views' half size in metres, which the baked depth is a share of.
    @location(7) @interpolate(flat) radius: f32,
    // The views' centre and the instance's yaw, and this point of the quad before it sways:
    // each view's image is found where the ray through it meets that view's plane.
    @location(8) @interpolate(flat) centre: vec3<f32>,
    @location(9) @interpolate(flat) yaw: f32,
    @location(10) rest: vec3<f32>,
}

// Bevy's Quat::from_rotation_y.
fn rotate_y(v: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

// Where an instance stands, which corner of its quad a vertex is, and its dither level.
struct Placement {
    root: vec3<f32>,
    yaw: f32,
    scale: f32,
    // Height over the hand-off threshold: times pixels per metre, the hand-off distance.
    switch_distance: f32,
    corner: u32,
    dither: i32,
}

fn placement(v: Vertex) -> Placement {
    // The GPU trees chose this instance and its dither level.
    let record = tree_records[v.instance_index];
    let instance = tree_instances[record & 0xffffffu];
    return Placement(
        tree_origins[instance.origin].xyz + instance.position_yaw.xyz,
        instance.position_yaw.w,
        instance.scale,
        impostor.wind_height.z / impostor.wind_height.y * instance.scale,
        u32(v.position.x),
        i32((record >> 24u) & 63u) - 16,
    );
}

@vertex
fn vertex(v: Vertex) -> Varyings {
    var out: Varyings;
    let placed = placement(v);
    let root = placed.root;
    let yaw = placed.yaw;
    let scale = placed.scale;
    let switch_distance = placed.switch_distance;
    let index = placed.corner;
    out.dither = placed.dither;
    let centre = root + rotate_y(impostor.centre_radius.xyz * scale, yaw);
    let radius = impostor.centre_radius.w * scale;
    let orthographic = impostor.settings.z > 0.5;
    let distance = length(view.lod_view_world_position.xyz - root);
    if !orthographic && 2.0 * radius * impostor.settings.y < MINIMUM_PIXELS * distance {
        out.dither = -16;
    }
    if out.dither <= -16 {
        // All corners coincide: nothing to rasterize.
        out.position = vec4(0.0, 0.0, 0.0, 1.0);
        out.world_position = vec4(root, 1.0);
        return out;
    }

    // An orthographic view's quads all face along it.
    var toward_view: vec3<f32>;
    if view.clip_from_view[3].w == 1.0 {
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
    let quad = vec2(f32(index & 1u), 1.0 - 0.5 * f32(index >> 1u));
    let corner = mix(impostor.crop.xy, impostor.crop.zw, quad);
    let offset = right * ((corner.x * 2.0 - 1.0) * radius) + up * ((1.0 - corner.y * 2.0) * radius);
    let rest = centre + offset;
    out.world_position = vec4(rest, 1.0);
    out.centre = centre;
    out.yaw = yaw;
    out.rest = rest;
    // Leaning as the mesh LODs' trunk does where it takes over from them (the LOD
    // projection's switch distance), less beyond.
    let handoff = switch_distance * impostor.settings.y;
    let calm = max(min(handoff * WIND_SPAN, WIND_END), handoff + 1.0);
    let sway = select(1.0 - smoothstep(handoff, calm, distance), 1.0, orthographic);
    let height = impostor.wind_height.x * scale;
    if sway > 0.0 {
        out.world_position = vec4(sway_point_of_trunk(rest, root, height, impostor.wind_profile, wind.current, sway), 1.0);
    }
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
    out.radius = radius;
    out.right = right;
    out.up = up;
    out.toward_view = toward_view;
    return out;
}

struct Surface {
    colour: vec3<f32>,
    normal: vec3<f32>,
    occlusion: f32,
    // Metres from the quad toward the viewer.
    depth: f32,
}

// The bake's octahedral encoding of a unit vector (z toward the viewer).
fn decode_normal(e: vec2<f32>) -> vec3<f32> {
    var n = vec3(e, 1.0 - abs(e.x) - abs(e.y));
    let t = max(-n.z, 0.0);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return n;
}

// The view a cell of the grid was baked from: its direction toward the viewer and its
// image's right and up, in world space (bake_impostor.py `view_direction`, `basis`).
fn view_frame(cell: vec2<f32>, yaw: f32) -> mat3x3<f32> {
    let p = (cell + 0.5) / impostor.settings.x * 2.0 - 1.0;
    let x = (p.x + p.y) * 0.5;
    let z = (p.x - p.y) * 0.5;
    let d = rotate_y(normalize(vec3(x, max(1.0 - abs(x) - abs(z), 0.0), z)), yaw);
    var right = cross(vec3(0.0, 1.0, 0.0), d);
    if dot(right, right) < 1e-6 {
        right = rotate_y(vec3(1.0, 0.0, 0.0), yaw);
    }
    right = normalize(right);
    return mat3x3(right, cross(d, right), d);
}

// Where the ray through this fragment meets a view's image, across the view (0-1, v down):
// on the plane through the centre facing that view. Blending views at the quad's own
// position instead lays four different projections of the crown over each other.
fn view_position(in: Varyings, frame: mat3x3<f32>, ray: vec3<f32>) -> vec2<f32> {
    let d = frame[2];
    let t = dot(in.centre - in.rest, d) / min(dot(ray, d), -0.1);
    let local = in.rest + ray * t - in.centre;
    return vec2(dot(local, frame[0]), -dot(local, frame[1])) / (2.0 * in.radius) + 0.5;
}

// Coverage-weighted blend of the four nearest views, each read where the fragment's ray
// meets it; discards outside the crown before reading any normal, since most of a quad
// lies outside it. The quad's gradients pick each view's mip for its on-screen size.
fn surface(in: Varyings, ddx_uv: vec2<f32>, ddy_uv: vec2<f32>) -> Surface {
    var ray = -in.toward_view;
    if view.clip_from_view[3].w != 1.0 {
        ray = normalize(in.rest - view.world_position);
    }
    let count = impostor.settings.x;
    // Half a texel of the coarser mip the gradients pick, across a view's tile. The bake
    // leaves at least that much empty round the tree in every view (half a texel of its
    // coarsest level) and builds every level view by view, so a ray that near the tile's edge
    // meets nothing, and a read farther in never filters in the next view.
    let size = vec2<f32>(textureDimensions(albedo_texture));
    let texels = max(length(ddx_uv * size), length(ddy_uv * size));
    let level = clamp(ceil(log2(max(texels, 1e-6))), 0.0, f32(textureNumLevels(albedo_texture)) - 1.0);
    let inset = min(0.5 * exp2(level) * count / size, vec2(0.5));
    var uvs: array<vec2<f32>, 4>;
    var frames: array<mat3x3<f32>, 4>;
    var weights: vec4<f32>;
    var colour = vec3(0.0);
    var alpha = 0.0;
    for (var i = 0u; i < 4u; i += 1u) {
        let step = vec2(f32(i & 1u), f32(i >> 1u));
        let cell = in.frame.xy + step;
        frames[i] = view_frame(cell, in.yaw);
        // Across the view's tile, which holds only its crop; a ray outside it meets nothing.
        let crop = (view_position(in, frames[i], ray) - impostor.crop.xy) / (impostor.crop.zw - impostor.crop.xy);
        let inside = all(crop >= inset) && all(crop <= vec2(1.0) - inset);
        let weight = mix(1.0 - in.frame.z, in.frame.z, step.x) * mix(1.0 - in.frame.w, in.frame.w, step.y);
        uvs[i] = (cell + clamp(crop, inset, vec2(1.0) - inset)) / count;
        let a = textureSampleGrad(albedo_texture, impostor_sampler, uvs[i], ddx_uv, ddy_uv);
        weights[i] = select(0.0, a.a * weight, inside);
        colour += a.rgb * weights[i];
        alpha += weights[i];
    }
    if alpha < 0.5 {
        discard;
    }
    var normal = vec3(0.0);
    var occlusion = 0.0;
    var depth = 0.0;
    for (var i = 0u; i < 4u; i += 1u) {
        let n = textureSampleGrad(normal_texture, impostor_sampler, uvs[i], ddx_uv, ddy_uv);
        // Each view's normal is in its own frame.
        normal += (frames[i] * normalize(decode_normal(n.xy * 2.0 - 1.0))) * weights[i];
        depth += (n.z * 2.0 - 1.0) * weights[i];
        occlusion += n.w * weights[i];
    }
    var out: Surface;
    out.colour = clamp(colour / alpha, vec3(0.0), vec3(1.0));
    // Opposed views can cancel; a zero normal must never reach lighting as NaN.
    let length_n = length(normal);
    out.normal = select(in.toward_view, normal / length_n, length_n > 1e-4);
    out.occlusion = clamp(occlusion / alpha, 0.0, 1.0);
    out.depth = depth / alpha * in.radius;
    return out;
}

@fragment
fn fragment(in: Varyings) -> ForwardOutput {
    // Derivatives before any discard, while every fragment of the quad is still running,
    // scaled by the view's mip bias as every mesh texture is (sharper under a temporal
    // upscaler, which resolves to the output's pixels).
    let tile = impostor.settings.x * (impostor.crop.zw - impostor.crop.xy);
    let mip_scale = exp2(view.mip_bias);
    let ddx_uv = dpdx(in.corner) / tile * mip_scale;
    let ddy_uv = dpdy(in.corner) / tile * mip_scale;
@if(CROSSFADE_SAMPLE_MASK) {
    let crossfade_mask = crossfade_sample_mask(in.position, in.dither);
    if crossfade_mask == 0u {
        discard;
    }
}
@else
    dither_discard(in.position, in.dither);
    let s = surface(in, ddx_uv, ddy_uv);
    var out: ForwardOutput;
    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.material.base_color = vec4(s.colour, 1.0);
    pbr_input.material.perceptual_roughness = 0.85;
    // Foliage keeps half the default reflectance, like the mesh crowns.
    pbr_input.material.reflectance *= 0.5;
    // Crown occlusion: indirect light, and the crown self-shadow past the shadow range.
    pbr_input.diffuse_occlusion = vec3(s.occlusion);
    pbr_input.specular_occlusion = s.occlusion;
    pbr_input.frag_coord = in.position;
    // Shadows, the forest shadow march and the view vector start at the crown's surface.
    let surface_position = vec4(in.world_position.xyz + in.toward_view * s.depth, 1.0);
    pbr_input.world_position = surface_position;
    pbr_input.world_normal = s.normal;
    pbr_input.N = s.normal;
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.V = calculate_view(surface_position, pbr_input.is_orthographic);
    pbr_input.flags = MESH_FLAGS_SHADOW_RECEIVER_BIT;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
@if(CROSSFADE_SAMPLE_MASK)
    out.sample_mask = crossfade_mask;
    return out;
}
