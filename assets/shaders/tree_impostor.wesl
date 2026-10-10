// Impostor LOD (crates/engine/src/tree_impostor.rs): each instance is four vertices at its
// root, spread here into a quad facing the camera. The baked views lie on a hemi-octahedral
// grid (YarraVegetation scripts/bake_impostor.py), each stored as the tile of its crop: the
// four views nearest the camera's direction are blended, then lit with the trees' crown
// shading where the baked depth puts the crown's surface, not on the quad. Near the camera
// the quad bends in the wind as its mesh LODs' trunk does.
import bevy_pbr::{render::mesh_functions, render::mesh_view_bindings::view, render::view_transformations::position_world_to_clip};
// Depth-only passes (the shadow cascades) have no outputs, only discards.
@if(PREPASS_PIPELINE && PREPASS_FRAGMENT)
import bevy_pbr::prepass::io::FragmentOutput;
@if(PREPASS_PIPELINE && MOTION_VECTOR_PREPASS)
import bevy_pbr::prepass::bindings::previous_view_uniforms;
@if(!PREPASS_PIPELINE)
import bevy_pbr::{render::pbr_types, render::pbr_functions::{calculate_view, main_pass_post_lighting_processing}, render::mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT};
@if(!PREPASS_PIPELINE)
import package::shaders::clouds::pbr_lighting::apply_pbr_lighting;
@if(!PREPASS_PIPELINE && CROSSFADE_SAMPLE_MASK)
import package::shaders::crossfade::crossfade_sample_mask;

// The forward pass's output (Bevy's forward_io::FragmentOutput), with the crossfade's covered
// samples where the view crossfades by sample mask.
@if(!PREPASS_PIPELINE)
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
    // Views per side, pixels per metre of the LOD projection, orthographic flag, and the
    // farthest hand-off from the mesh LODs in metres.
    settings: vec4<f32>,
    // The far mesh LOD's wind profile (tree_wind.wesl tree_profile) and stem height in
    // object metres (x).
    wind_profile: vec4<f32>,
    wind_height: vec4<f32>,
}
// Quads smaller than this many pixels across are not drawn.
const MINIMUM_PIXELS: f32 = 1.0;
@group(constants::MATERIAL_BIND_GROUP) @binding(130) var<uniform> impostor: ImpostorParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(131) var albedo_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(132) var impostor_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(133) var normal_texture: texture_2d<f32>;
// Every instance's fade (tree_impostor.rs ImpostorFades), from its batch's first slot (the
// mesh tag) on: 0 while no resident object controls it, otherwise 64 + its dither level.
@group(constants::MATERIAL_BIND_GROUP) @binding(134) var<storage, read> fades: array<u32>;
// The mesh tag of a batch without fade slots.
const NO_SLOTS: u32 = 0xffffffffu;

// tree_wind.wesl's wind, shared with every tree (tree_wind.rs WindBuffer).
struct WindPose {
    field: vec4<f32>,
    phases: vec4<f32>,
    response: vec4<f32>,
    camera: vec4<f32>,
    hierarchy: vec4<f32>,
    sway_phases: vec4<f32>,
}
struct WindFrames {
    current: WindPose,
    previous: WindPose,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(135) var<storage, read> wind: WindFrames;
// Impostors sway as fully as their mesh LODs where they take over from them, so the
// dissolve matches, then less with distance: not at all by WIND_SPAN times that distance,
// nor past WIND_END metres. A whole crown swinging as one reads as a card far away.
const WIND_SPAN: f32 = 2.5;
const WIND_END: f32 = 450.0;

// Impostor wind: begin (tree_wind/gpu_tests.rs checks it against the mesh LODs' wind)
fn sway_turn(v: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    return v * c + cross(axis, v) * sin(angle) + axis * dot(axis, v) * (1.0 - c);
}

// tree_wind.wesl sway_signal, for a tree rooted at `root`.
fn sway_wave(root: vec3<f32>, pose: WindPose, period: f32) -> f32 {
    let frequencies = vec4(0.55, 1.3, 2.7, 4.3);
    let spatial = vec4(0.035, 0.07, 0.14, 0.28);
    let phase = pose.sway_phases + dot(root.xz, pose.field.xy) * spatial;
    let ratio = frequencies * max(0.5, period * pose.hierarchy.w) / 6.2831853;
    let spring = vec4(1.0) - ratio * ratio;
    let damping = 1.5 * ratio;
    let gain = inverseSqrt(spring * spring + damping * damping);
    let lag = atan2(damping, spring);
    return dot(sin(phase + lag) * gain, vec4(0.65, 0.38, 0.19, 0.09));
}

// Where the wind moves `point` of an upright tree rooted at `root`, with a stem `height`
// metres tall: as tree_wind.wesl's structural_frame and sway_point move a point of its
// trunk. The stem bends into an arc and points beside it turn with it; `amount` scales the
// lean.
fn sway_point_of_trunk(point: vec3<f32>, root: vec3<f32>, height: f32, profile: vec4<f32>,
                       pose: WindPose, amount: f32) -> vec3<f32> {
    let up = vec3(0.0, 1.0, 0.0);
    let direction = vec3(pose.field.x, 0.0, pose.field.y);
    var bend_axis = cross(up, direction);
    if dot(bend_axis, bend_axis) < 1e-8 { bend_axis = vec3(0.0, 0.0, 1.0); }
    bend_axis = normalize(bend_axis);
    let pressure = pose.field.w * pose.field.w;
    let wave = sway_wave(root, pose, profile.y);
    let lean = amount * clamp(0.11 * pressure * profile.x * profile.w * pose.hierarchy.x
                              * (1.0 + pose.response.x * wave), -0.15, 0.8);
    let tree_height = max(0.1, height);
    let y = max(0.0, point.y - root.y);
    let angle = lean * clamp(y / tree_height, 0.0, 1.0);
    let curvature = lean / tree_height;
    var center = up * y;
    if abs(curvature) > 1e-6 {
        center = up * (sin(angle) / curvature)
                 + cross(bend_axis, up) * ((1.0 - cos(angle)) / curvature);
        center += sway_turn(up, bend_axis, angle) * max(0.0, y - tree_height);
    }
    return root + center + sway_turn(point - root - up * y, bend_axis, angle);
}
// Impostor wind: end

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    // Yaw, uniform scale, switch (height over threshold), and the quad corner (0-5: bottom,
    // middle and top rows, left then right) plus eight times the instance's index within
    // the batch.
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
    // The views' half size in metres, which the baked depth is a share of.
    @location(7) @interpolate(flat) radius: f32,
    // Shadow casters in front of a cascade's near plane clamp to it rather than vanish.
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    @location(8) unclipped_depth: f32,
    // In the previous frame's wind.
    @if(PREPASS_PIPELINE && MOTION_VECTOR_PREPASS)
    @location(9) previous_world_position: vec4<f32>,
}

// Bevy's Quat::from_rotation_y.
fn rotate_y(v: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
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

    // The object's mesh LODs fade it in and out over time while its cell is resident
    // (object_lod.rs); otherwise it is drawn, so a tree is never missing while its cell
    // loads.
    let packed = u32(v.instance.w);
    let base = mesh_functions::get_tag(v.instance_index);
    var fade = 0u;
    if base != NO_SLOTS {
        fade = fades[base + (packed >> 3u)];
    }
    out.dither = select(i32(fade) - 64, 0, fade == 0u);
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

    // Shadow cascades are orthographic views from the sun: there the quad faces the sun and
    // shows the view baked from its direction, so impostors cast their crowns' shadows.
    // Which instances draw still follows the main camera's LOD distances (above).
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
    let index = packed & 7u;
    let quad = vec2(f32(index & 1u), 1.0 - 0.5 * f32(index >> 1u));
    let corner = mix(impostor.crop.xy, impostor.crop.zw, quad);
    let offset = right * ((corner.x * 2.0 - 1.0) * radius) + up * ((1.0 - corner.y * 2.0) * radius);
    let rest = centre + offset;
    out.world_position = vec4(rest, 1.0);
    // Leaning as the mesh LODs' trunk does where it takes over from them (the LOD
    // projection's switch distance, within the hand-off), less beyond.
    let handoff = min(v.instance.z * impostor.settings.y, impostor.settings.w);
    let calm = max(min(handoff * WIND_SPAN, WIND_END), handoff + 1.0);
    let sway = select(1.0 - smoothstep(handoff, calm, distance), 1.0, orthographic);
    let height = impostor.wind_height.x * scale;
    if sway > 0.0 {
        out.world_position = vec4(sway_point_of_trunk(rest, root, height, impostor.wind_profile, wind.current, sway), 1.0);
    }
@if(PREPASS_PIPELINE && MOTION_VECTOR_PREPASS) {
    out.previous_world_position = out.world_position;
    if sway > 0.0 {
        out.previous_world_position = vec4(sway_point_of_trunk(rest, root, height, impostor.wind_profile, wind.previous, sway), 1.0);
    }
}
    out.position = position_world_to_clip(out.world_position.xyz);
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
}

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

// Coverage-weighted blend of the four nearest views; discards outside the crown before
// reading any normal, since most of a quad lies outside it. The quad's gradients pick each
// view's mip for its on-screen size.
fn surface(in: Varyings, ddx_uv: vec2<f32>, ddy_uv: vec2<f32>) -> Surface {
    // Across the view's tile, which holds only its crop.
    let corner = clamp(
        (in.corner - impostor.crop.xy) / (impostor.crop.zw - impostor.crop.xy),
        vec2(0.0),
        vec2(1.0),
    );
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
    var depth = 0.0;
    for (var i = 0u; i < 4u; i += 1u) {
        let n = textureSampleGrad(normal_texture, impostor_sampler, uvs[i], ddx_uv, ddy_uv);
        normal += normalize(decode_normal(n.xy * 2.0 - 1.0)) * weights[i];
        depth += (n.z * 2.0 - 1.0) * weights[i];
        occlusion += n.w * weights[i];
    }
    var out: Surface;
    out.colour = clamp(colour / alpha, vec3(0.0), vec3(1.0));
    // Opposed views can cancel; a zero normal must never reach lighting as NaN.
    let length_n = length(normal);
    let n = select(vec3(0.0, 0.0, 1.0), normal / length_n, length_n > 1e-4);
    out.normal = normalize(in.right * n.x + in.up * n.y + in.toward_view * n.z);
    out.occlusion = clamp(occlusion / alpha, 0.0, 1.0);
    out.depth = depth / alpha * in.radius;
    return out;
}

// Depth-only passes (the shadow cascades): the fade's and the crown's discards only.
@if(PREPASS_PIPELINE && !PREPASS_FRAGMENT)
@fragment
fn fragment(in: Varyings) {
    // Derivatives before any discard, while every fragment of the quad is still running.
    let tile = impostor.settings.x * (impostor.crop.zw - impostor.crop.xy);
    let ddx_uv = dpdx(in.corner) / tile;
    let ddy_uv = dpdy(in.corner) / tile;
    dither_discard(in.position, in.dither);
    let s = surface(in, ddx_uv, ddy_uv);
}

// The prepass's outputs (Bevy's prepass_io::FragmentOutput), or the forward pass's.
@if(PREPASS_PIPELINE && PREPASS_FRAGMENT)
alias ImpostorOutput = FragmentOutput;
@elif(!PREPASS_PIPELINE)
alias ImpostorOutput = ForwardOutput;

@if(!PREPASS_PIPELINE || PREPASS_FRAGMENT)
@fragment
fn fragment(in: Varyings) -> ImpostorOutput {
    // Derivatives before any discard, while every fragment of the quad is still running.
    let tile = impostor.settings.x * (impostor.crop.zw - impostor.crop.xy);
    let ddx_uv = dpdx(in.corner) / tile;
    let ddy_uv = dpdy(in.corner) / tile;
@if(!PREPASS_PIPELINE && CROSSFADE_SAMPLE_MASK) {
    let crossfade_mask = crossfade_sample_mask(in.position, in.dither);
    if crossfade_mask == 0u {
        discard;
    }
}
@else
    dither_discard(in.position, in.dither);
    let s = surface(in, ddx_uv, ddy_uv);
    var out: ImpostorOutput;
@if(PREPASS_PIPELINE) {
@if(NORMAL_PREPASS)
    out.normal = vec4(s.normal * 0.5 + 0.5, 1.0);
@if(MOTION_VECTOR_PREPASS) {
    // The quad turns with the camera, which reprojection ignores; the wind it follows.
    // Both clip positions unjittered, as Bevy's calculate_motion_vector takes them (since 0.20
    // the previous view's clip_from_world carries that frame's jitter).
    let clip = view.unjittered_clip_from_world * in.world_position;
    let previous = previous_view_uniforms.unjittered_clip_from_world * in.previous_world_position;
    out.motion_vector = (clip.xy / clip.w - previous.xy / previous.w) * vec2(0.5, -0.5);
}
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    out.frag_depth = in.unclipped_depth;
}
@else {
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
}
    return out;
}
