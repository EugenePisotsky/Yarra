#import bevy_pbr::{
    mesh_view_bindings as view_bindings,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    shadows,
}

// Vegetation V2 procedural topology and placement diagnostics.
//
// The geometry path has no vertex streams. A fixed procedural vertex budget is decoded from
// vertex_index while instance_index selects compact data emitted by the placement compute pass.

struct ProceduralInstance {
    // xyz: root, w: packed clump variant and nested LOD rank
    root_clump: vec4<f32>,
    // x: packed rest direction, y: species index, z: packed surface normal xz,
    // w: low 24 bits seed + high 8 bits population-density target
    geometry: vec4<u32>,
}

struct DebugInstance {
    root_direction: vec4<f32>,
    direction_species: vec4<f32>,
    parent_status: vec4<f32>,
    diagnostics: vec4<f32>,
}

struct Species {
    // xyz: root color, w: topology family code
    root_color: vec4<f32>,
    // xyz: tip color, w: maximum height
    tip_color_height: vec4<f32>,
    // xy: height range, zw: half-width range
    bounds: vec4<f32>,
    // x: high sections, y: low sections, z: blades/render unit, w: longitudinal power
    topology: vec4<f32>,
    // xy: tilt range; zw: broad-leaf droop range
    shape: vec4<f32>,
    // x: lateral curve/camber, y: pair spread,
    // z: tangent of maximum ribbon view-opening angle / broad crown radius,
    // w: maximum horizontal reach
    shape_secondary: vec4<f32>,
    // xy: normalized-height root-handle forward/normal vector,
    // zw: normalized-height tip-handle forward/normal vector
    curve_variant_a: vec4<f32>,
    curve_variant_b: vec4<f32>,
    // x: clump color variation, y: roughness, z: transmission, w: normal rounding
    material: vec4<f32>,
    // x: root AO, y: tip AO, z: high-LOD threshold,
    // w: density-budgeted high-topology radius
    shading: vec4<f32>,
    // xyz: group coherence for height, complete silhouette, and lateral curve
    group_response: vec4<f32>,
}

struct Camera {
    clip_from_world: mat4x4<f32>,
    camera_position: vec4<f32>,
    // x: vertical focal length in pixels, y: viewport width, z: viewport height
    projection: vec4<f32>,
    // xyz: direction from the surface toward the strongest directional light, w: active
    sun_direction: vec4<f32>,
    // xyz: strongest directional-light color and global ambient-light color
    sun_radiance: vec4<f32>,
    ambient_radiance: vec4<f32>,
    // x: diffuse, y: specular, z: transmission, w: received-shadow strength
    lighting: vec4<f32>,
}

struct DebugConfig {
    // 0: geometry, 1: accepted species, 2: parent links, 3: outcomes, 4: group structure
    values: vec4<u32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) blade_t: f32,
    // x: roughness, y: transmission, z: AO, w: geometry/diagnostic flag
    @location(4) material: vec4<f32>,
}

// Group zero is Bevy's mesh-view bind group, including directional shadow cascades.
@group(1) @binding(0) var<storage, read> procedural_instances: array<ProceduralInstance>;
@group(1) @binding(1) var<storage, read> diagnostic_instances: array<DebugInstance>;
@group(1) @binding(2) var<storage, read> species: array<Species>;
@group(1) @binding(3) var<uniform> camera: Camera;
@group(1) @binding(4) var<uniform> debug_config: DebugConfig;

const PI: f32 = 3.141592653589793;
const MAX_SECTIONS: u32 = 8u;
const MAX_LOW_SECTIONS: u32 = 3u;

fn hash32(value: u32) -> u32 {
    var x = value;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    return x ^ (x >> 16u);
}

fn random01(value: u32) -> f32 {
    return f32(hash32(value)) * (1.0 / 4294967295.0);
}

fn normalize3_or(value: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(value, value);
    if (length_squared <= 1e-10) {
        return fallback;
    }
    return value * inverseSqrt(length_squared);
}

fn surface_normal_from_debug_instance(instance: DebugInstance) -> vec3<f32> {
    let normal_xz = unpack2x16snorm(bitcast<u32>(instance.direction_species.w));
    return normalize3_or(
        vec3<f32>(
            normal_xz.x,
            sqrt(max(0.0, 1.0 - dot(normal_xz, normal_xz))),
            normal_xz.y,
        ),
        vec3<f32>(0.0, 1.0, 0.0),
    );
}

fn diagnostic_quad_vertex(vertex_index: u32) -> vec2<f32> {
    switch vertex_index {
        case 0u: { return vec2<f32>(0.0, -1.0); }
        case 1u: { return vec2<f32>(0.0, 1.0); }
        case 2u: { return vec2<f32>(1.0, 1.0); }
        default: { return vec2<f32>(1.0, -1.0); }
    }
}

fn cubic_bezier(
    p0: vec3<f32>,
    p1: vec3<f32>,
    p2: vec3<f32>,
    p3: vec3<f32>,
    t: f32,
) -> vec3<f32> {
    let u = 1.0 - t;
    return u * u * u * p0
        + 3.0 * u * u * t * p1
        + 3.0 * u * t * t * p2
        + t * t * t * p3;
}

fn cubic_bezier_derivative(
    p0: vec3<f32>,
    p1: vec3<f32>,
    p2: vec3<f32>,
    p3: vec3<f32>,
    t: f32,
) -> vec3<f32> {
    let u = 1.0 - t;
    return 3.0 * u * u * (p1 - p0)
        + 6.0 * u * t * (p2 - p1)
        + 3.0 * t * t * (p3 - p2);
}

fn projected_distance_pixels(origin: vec3<f32>, endpoint: vec3<f32>) -> f32 {
    let origin_clip = camera.clip_from_world * vec4<f32>(origin, 1.0);
    let endpoint_clip = camera.clip_from_world * vec4<f32>(endpoint, 1.0);
    if (origin_clip.w <= 1e-5 || endpoint_clip.w <= 1e-5) {
        return 1e30;
    }
    return length(
        endpoint_clip.xy / endpoint_clip.w - origin_clip.xy / origin_clip.w,
    ) * camera.camera_position.w * 0.5;
}

fn projected_blade_extent_pixels(
    root: vec3<f32>,
    surface_normal: vec3<f32>,
    profile: Species,
) -> f32 {
    let reach = profile.shape_secondary.w;
    var maximum_pixels = projected_distance_pixels(
        root,
        root + surface_normal * profile.tip_color_height.w,
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root + vec3<f32>(reach, 0.0, 0.0)),
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root - vec3<f32>(reach, 0.0, 0.0)),
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root + vec3<f32>(0.0, 0.0, reach)),
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root - vec3<f32>(0.0, 0.0, reach)),
    );
    return maximum_pixels;
}

fn budgeted_projected_blade_extent_pixels(
    root: vec3<f32>,
    surface_normal: vec3<f32>,
    profile: Species,
    lod_rank: f32,
) -> f32 {
    let projected_extent = projected_blade_extent_pixels(root, surface_normal, profile);
    let high_radius = max(profile.shading.w, 1e-3);
    let staggered_high_radius = high_radius * mix(0.84, 1.12, lod_rank);
    let distance = length(root.xz - camera.camera_position.xz);
    let high_weight = 1.0 - smoothstep(
        staggered_high_radius * 0.68,
        staggered_high_radius,
        distance,
    );
    let budget_extent = profile.shading.z * mix(0.999, 1.45, high_weight);
    return min(projected_extent, budget_extent);
}

fn geometry_vertex(vertex_index: u32, instance_index: u32) -> VertexOutput {
    let instance = procedural_instances[instance_index];
    let low_lod = (instance.geometry.y >> 31u) != 0u;
    let species_index = instance.geometry.y & 0x7fffffffu;
    let profile = species[species_index];
    let root = instance.root_clump.xyz;
    let variants = unpack2x16unorm(bitcast<u32>(instance.root_clump.w));
    let clump_variant = variants.x;
    let group_key = u32(round(clump_variant * 65535.0));
    let lod_rank = variants.y;
    let normal_xz = unpack2x16snorm(instance.geometry.z);
    let surface_normal = normalize3_or(
        vec3<f32>(
            normal_xz.x,
            sqrt(max(0.0, 1.0 - dot(normal_xz, normal_xz))),
            normal_xz.y,
        ),
        vec3<f32>(0.0, 1.0, 0.0),
    );
    let rest_direction = unpack2x16snorm(instance.geometry.x);

    let projected_extent = budgeted_projected_blade_extent_pixels(
        root,
        surface_normal,
        profile,
        lod_rank,
    );
    // At the high/low boundary, high sections converge on the exact low-section samples. Stable
    // candidates outside the nested low-density subset contract laterally before leaving.
    let lod_morph = select(
        smoothstep(profile.shading.z, profile.shading.z * 1.45, projected_extent),
        0.0,
        low_lod,
    );

    var forward = vec3<f32>(rest_direction.x, 0.0, rest_direction.y);
    forward -= surface_normal * dot(forward, surface_normal);
    forward = normalize3_or(forward, vec3<f32>(1.0, 0.0, 0.0));
    let base_side = normalize3_or(cross(surface_normal, forward), vec3<f32>(0.0, 0.0, 1.0));

    // Static u16 indices encode side in bit 0, row in bits 1..4, and blade in bit 5. A paired
    // render unit divides the single-blade budget: 4+3 high sections or two low triangles.
    let blade_index = (vertex_index >> 5u) & 1u;
    let topology_row = (vertex_index >> 1u) & 15u;
    let side_sign = select(-1.0, 1.0, (vertex_index & 1u) != 0u);
    let authored_section_count = select(profile.topology.x, profile.topology.y, low_lod);
    let blade_count = clamp(u32(profile.topology.z + 0.5), 1u, 2u);
    var maximum_sections = select(MAX_SECTIONS, MAX_LOW_SECTIONS, low_lod);
    if (blade_count > 1u) {
        maximum_sections = select(select(4u, 3u, blade_index != 0u), 1u, low_lod);
    }
    let section_count = clamp(u32(authored_section_count + 0.5), 1u, maximum_sections);

    let seed = instance.geometry.w & 0x00ffffffu;
    let population_density = f32(instance.geometry.w >> 24u) / 255.0;
    let blade_seed = hash32(seed ^ blade_index * 0x9e3779b9u);
    let paired_side = select(-1.0, 1.0, blade_index != 0u);
    let is_broad_leaf = profile.root_color.w >= 1.5;
    let facing_jitter = (random01(blade_seed ^ 0x68e31da4u) - 0.5)
        * select(0.7, 0.34, is_broad_leaf);
    let spread_angle = paired_side * profile.shape_secondary.y * 0.5 + facing_jitter;
    let blade_forward = normalize3_or(
        forward * cos(spread_angle) + base_side * sin(spread_angle),
        forward,
    );
    let blade_side = normalize3_or(cross(surface_normal, blade_forward), base_side);

    // Density LOD is a coverage transition, not a growth animation. Keeping the complete
    // centreline prevents the rejected three-of-four subset from visibly rising out of the ground;
    // only ribbon width contracts as those stable candidates leave the high-detail population.
    let density_width = select(lod_morph, 1.0, lod_rank < population_density || low_lod);
    let height_coordinate = mix(
        random01(blade_seed ^ 0xa511e9b3u),
        random01(group_key ^ 0x52dce729u),
        profile.group_response.x,
    );
    let silhouette_coordinate = mix(
        random01(blade_seed ^ 0x27d4eb2fu),
        random01(group_key ^ 0x7b7d159cu),
        profile.group_response.y,
    );
    let lateral_coordinate = mix(
        random01(blade_seed ^ 0x165667b1u),
        random01(group_key ^ 0x94d049bbu),
        profile.group_response.z,
    );
    let height = mix(
        profile.bounds.x,
        profile.bounds.y,
        height_coordinate,
    );
    let half_width = density_width * mix(
        profile.bounds.z,
        profile.bounds.w,
        random01(blade_seed ^ 0x63d83595u),
    );
    let tilt = mix(profile.shape.x, profile.shape.y, silhouette_coordinate);
    let broad_leaf_bend = mix(profile.shape.z, profile.shape.w, silhouette_coordinate);
    let lateral = (lateral_coordinate * 2.0 - 1.0)
        * profile.shape_secondary.x;

    var curve_root = root;
    if (is_broad_leaf) {
        curve_root += blade_forward * profile.shape_secondary.z * 0.35;
    } else if (blade_count > 1u) {
        curve_root += base_side * paired_side * half_width * 0.75;
    }

    let tilt_sine = sin(tilt);
    let tilt_cosine = cos(tilt);
    let tip_forward = height * tilt_sine;
    let p0 = curve_root;
    let p3 = curve_root + surface_normal * height * tilt_cosine + blade_forward * tip_forward;
    var p1: vec3<f32>;
    var p2: vec3<f32>;
    if (is_broad_leaf) {
        p1 = curve_root
            + surface_normal * height * 0.34
            + blade_forward * height * broad_leaf_bend * 0.05;
        p2 = mix(curve_root, p3, 0.68)
            + surface_normal * height * broad_leaf_bend * 0.16
            + blade_forward * height * broad_leaf_bend * 0.22
            + blade_side * height * lateral * 0.22;
    } else {
        // One stable coordinate interpolates complete curve variants, keeping the two handles
        // correlated. Their independent directions and lengths provide root stiffness, broad
        // arches, late curvature, flat tips, and downward follow-through with no extra vertices.
        let curve = mix(profile.curve_variant_a, profile.curve_variant_b, silhouette_coordinate);
        p1 = curve_root + (blade_forward * curve.x + surface_normal * curve.y) * height;
        p2 = p3
            - (blade_forward * curve.z + surface_normal * curve.w) * height
            + blade_side * height * lateral * 0.22;
    }

    // Sections above the species budget collapse at the tip. This lets all species in a topology
    // bin share one indirect command while retaining artist-controlled longitudinal distribution.
    let authored_linear_t = f32(min(topology_row, section_count)) / f32(section_count);
    let low_section_count = select(
        clamp(u32(profile.topology.y + 0.5), 1u, MAX_LOW_SECTIONS),
        1u,
        blade_count > 1u,
    );
    let low_linear_t = round(authored_linear_t * f32(low_section_count))
        / f32(low_section_count);
    let linear_t = select(
        mix(low_linear_t, authored_linear_t, lod_morph),
        authored_linear_t,
        low_lod,
    );
    let t = pow(linear_t, max(profile.topology.w, 0.2));
    let curve_position = cubic_bezier(p0, p1, p2, p3, t);
    let curve_tangent = normalize3_or(
        cubic_bezier_derivative(p0, p1, p2, p3, t),
        surface_normal,
    );

    let ribbon_taper = pow(max(1.0 - t, 0.0), 0.72);
    let broad_taper = pow(max(sin(PI * t), 0.0), 0.58);
    let taper = select(ribbon_taper, broad_taper, is_broad_leaf);
    // Transport the authored root-side axis onto the plane perpendicular to the local Bezier
    // tangent. A constant root frame makes strongly curved ribbons kink and exposes their edge at
    // the wrong angle; the transported frame follows the curve without adding vertices.
    let local_ribbon_side = normalize3_or(
        blade_side - curve_tangent * dot(blade_side, curve_tangent),
        blade_side,
    );
    let physical_normal = normalize3_or(
        cross(local_ribbon_side, curve_tangent),
        surface_normal,
    );
    let to_camera = normalize3_or(camera.camera_position.xyz - curve_position, physical_normal);
    // Rotate the ribbon's *width line* toward the camera-facing width line by no more than the
    // authored angle. A width line is unoriented (S and -S describe the same two edge positions),
    // so align the camera line to the nearest hemisphere before finding the angular remainder.
    // The previous grazing-only response did almost nothing until the blade was within a few
    // degrees of perfectly edge-on, and its unnormalised remainder made the slider response hard
    // to observe. shape_secondary.z stores tan(maximum angle), allowing an exact bounded rotation
    // without trigonometry in the vertex shader.
    var rendered_ribbon_side = local_ribbon_side;
    if (!is_broad_leaf && profile.shape_secondary.z > 0.0) {
        let unaligned_camera_side = normalize3_or(
            cross(curve_tangent, to_camera),
            local_ribbon_side,
        );
        let signed_alignment = dot(unaligned_camera_side, local_ribbon_side);
        let camera_ribbon_side = select(
            -unaligned_camera_side,
            unaligned_camera_side,
            signed_alignment >= 0.0,
        );
        let alignment = abs(signed_alignment);
        let opening_remainder = camera_ribbon_side - local_ribbon_side * alignment;
        let remainder_length = length(opening_remainder);
        let requested_tangent = remainder_length / max(alignment, 1e-4);
        let opening_tangent = min(profile.shape_secondary.z, requested_tangent);
        let opening_direction = opening_remainder / max(remainder_length, 1e-4);
        rendered_ribbon_side = normalize3_or(
            local_ribbon_side + opening_direction * opening_tangent,
            local_ribbon_side,
        );
    }
    let world_position = curve_position
        + rendered_ribbon_side * side_sign * half_width * taper;

    // View opening is a silhouette correction, not a material deformation. Preserve the physical
    // blade normal so changing the opening does not rotate every blade's lighting away from the
    // sun and darken the field. The fragment shader treats this physical frame as two-sided.
    let rounded_normal = normalize3_or(
        physical_normal + local_ribbon_side * side_sign * profile.material.w,
        physical_normal,
    );
    let variation = (clump_variant * 2.0 - 1.0) * profile.material.x;
    let color = mix(profile.root_color.xyz, profile.tip_color_height.xyz, t) * (1.0 + variation);

    var output: VertexOutput;
    output.clip_position = camera.clip_from_world * vec4<f32>(world_position, 1.0);
    output.color = color;
    output.world_normal = rounded_normal;
    output.world_position = world_position;
    output.blade_t = t;
    output.material = vec4<f32>(
        profile.material.y,
        profile.material.z,
        mix(profile.shading.x, profile.shading.y, t),
        1.0,
    );
    return output;
}

fn diagnostic_vertex(vertex_index: u32, instance_index: u32) -> VertexOutput {
    let instance = diagnostic_instances[instance_index];
    let profile = species[u32(instance.direction_species.z)];
    let root = instance.root_direction.xyz;
    let surface_normal = surface_normal_from_debug_instance(instance);
    let rest_direction = normalize(
        vec2<f32>(instance.root_direction.w, instance.direction_species.x),
    );

    var surface_direction = vec3<f32>(rest_direction.x, 0.0, rest_direction.y);
    surface_direction -= surface_normal * dot(surface_direction, surface_normal);
    surface_direction = normalize3_or(surface_direction, vec3<f32>(1.0, 0.0, 0.0));

    var start = root;
    var end = root + surface_direction * profile.tip_color_height.w * 0.46
        + surface_normal * profile.tip_color_height.w * 0.52;
    var start_color = profile.root_color.xyz;
    var end_color = profile.tip_color_height.xyz;
    var width_scale = select(1.0, 2.4, profile.root_color.w >= 1.5);

    if (debug_config.values.x == 2u) {
        start = instance.parent_status.xyz + vec3<f32>(0.0, 0.035, 0.0);
        end = root + surface_normal * 0.04;
        start_color = vec3<f32>(1.0, 0.92, 0.15);
        end_color = vec3<f32>(0.15, 0.85, 1.0);
        width_scale = 0.7;
    } else if (debug_config.values.x == 4u) {
        start = instance.parent_status.xyz + vec3<f32>(0.0, 0.035, 0.0);
        end = root + surface_normal * 0.04;
        let group_key = u32(round(instance.direction_species.y * 65535.0));
        let group_color = vec3<f32>(
            mix(0.18, 1.0, random01(group_key ^ 0xa511e9b3u)),
            mix(0.18, 1.0, random01(group_key ^ 0x63d83595u)),
            mix(0.18, 1.0, random01(group_key ^ 0xc2b2ae35u)),
        );
        start_color = min(group_color * 1.28, vec3<f32>(1.0));
        end_color = group_color;
        width_scale = mix(0.45, 0.9, instance.diagnostics.w);
    } else if (debug_config.values.x == 3u) {
        start = root + surface_normal * 0.025;
        end = start + surface_normal * 0.18;
        let outcome = u32(instance.parent_status.w);
        if (outcome == 0u) {
            start_color = vec3<f32>(0.15, 1.0, 0.25);
        } else if (outcome == 1u) {
            start_color = vec3<f32>(0.15, 0.5, 1.0);
        } else if (outcome == 2u) {
            start_color = vec3<f32>(1.0, 0.1, 1.0);
        } else {
            start_color = vec3<f32>(1.0, 0.22, 0.05);
        }
        end_color = start_color;
        width_scale = select(0.65, 1.25, outcome == 0u);
    }

    var axis_delta = end - start;
    if (dot(axis_delta, axis_delta) < 1e-8) {
        axis_delta = surface_normal * 0.05;
        end = start + axis_delta;
    }
    let axis = normalize(axis_delta);
    let to_camera = normalize3_or(camera.camera_position.xyz - mix(start, end, 0.5), surface_normal);
    var side = cross(axis, to_camera);
    if (dot(side, side) < 1e-6) {
        side = vec3<f32>(-rest_direction.y, 0.0, rest_direction.x);
    } else {
        side = normalize(side);
    }
    let strip = diagnostic_quad_vertex(vertex_index);
    var width = 0.014 * width_scale;
    if (debug_config.values.x == 1u) {
        width = mix(0.018, 0.006, strip.x) * width_scale;
    }
    let world_position = mix(start, end, strip.x) + side * strip.y * width;
    let clump_tint = mix(0.82, 1.14, instance.direction_species.y);

    var output: VertexOutput;
    output.clip_position = camera.clip_from_world * vec4<f32>(world_position, 1.0);
    output.color = mix(start_color, end_color, strip.x);
    if (debug_config.values.x == 1u) {
        output.color *= clump_tint;
    }
    output.world_normal = surface_normal;
    output.world_position = world_position;
    output.blade_t = strip.x;
    output.material = vec4<f32>(1.0, 0.0, 1.0, 0.0);
    return output;
}

@vertex
fn vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    if (debug_config.values.x == 0u) {
        return geometry_vertex(vertex_index, instance_index);
    }
    return diagnostic_vertex(vertex_index, instance_index);
}

fn directional_shadow_visibility(input: VertexOutput) -> f32 {
    if (camera.sun_direction.w <= 0.0) {
        return 1.0;
    }
    let light = &view_bindings::lights.directional_lights[0u];
    if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u) {
        return 1.0;
    }

    let world_position = vec4<f32>(input.world_position, 1.0);
    let view_z = dot(vec4<f32>(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z,
    ), world_position);
    return shadows::fetch_directional_shadow(
        0u,
        world_position,
        // Terrain-facing receiver bias is stable for thin two-sided ribbons. Using the rounded
        // blade normal here can offset samples below terrain and make shadows blink by facing.
        vec3<f32>(0.0, 1.0, 0.0),
        view_z,
        input.clip_position.xy,
    );
}

fn radiance_tint(radiance: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let peak = max(max(radiance.r, radiance.g), radiance.b);
    if (peak <= 1e-6) {
        return fallback;
    }
    return radiance / peak;
}

@fragment
fn fragment(
    input: VertexOutput,
) -> @location(0) vec4<f32> {
    if (input.material.w < 0.5) {
        return vec4<f32>(input.color, 1.0);
    }

    let view_direction = normalize3_or(
        camera.camera_position.xyz - input.world_position,
        vec3<f32>(0.0, 1.0, 0.0),
    );
    // Vegetation ribbons are two-sided. Face the interpolated authored/opened normal toward the
    // viewer instead of deriving its sign from triangle winding: adjacent transported width axes
    // can twist a highly curved strip without meaning that its lighting side should invert.
    let face_sign = select(-1.0, 1.0, dot(input.world_normal, view_direction) >= 0.0);
    let blade_normal = normalize3_or(
        input.world_normal * face_sign,
        vec3<f32>(0.0, 1.0, 0.0),
    );
    let light_direction = normalize3_or(camera.sun_direction.xyz, vec3<f32>(0.0, 1.0, 0.0));
    let camera_distance = distance(camera.camera_position.xyz, input.world_position);
    // Blend unresolved middle/far blades toward a stable up-dominated field normal. This preserves
    // broad lighting direction while preventing animated or densely alternating ribbon normals from
    // turning into specular glitter.
    let field_normal = normalize3_or(
        vec3<f32>(blade_normal.x * 0.16, 1.0, blade_normal.z * 0.16),
        vec3<f32>(0.0, 1.0, 0.0),
    );
    let distance_stability = smoothstep(20.0, 72.0, camera_distance);
    let normal = normalize3_or(
        mix(blade_normal, field_normal, distance_stability),
        field_normal,
    );
    let half_direction = normalize3_or(light_direction + view_direction, normal);
    let wrapped_diffuse = clamp((dot(normal, light_direction) + 0.48) / 1.48, 0.0, 1.0);
    let back_light = pow(max(dot(-normal, light_direction), 0.0), 1.5)
        * input.material.y;
    let roughness = clamp(input.material.x, 0.04, 1.0);
    let specular_power = mix(96.0, 4.0, roughness);
    let specular = pow(max(dot(normal, half_direction), 0.0), specular_power)
        * mix(0.24, 0.035, roughness)
        * (1.0 - distance_stability);
    let ambient_occlusion = clamp(input.material.z, 0.0, 1.0);
    let shadow_visibility = directional_shadow_visibility(input);
    // The old receiver cache could only darken direct light and therefore became almost invisible
    // under the stable authored body color. Let dense/AO-heavy blade regions lose part of that body
    // as well, while retaining enough ambient fill to avoid black cutout silhouettes.
    let shadow_floor = mix(0.16, 0.42, ambient_occlusion);
    let received_shadow = mix(
        1.0,
        mix(shadow_floor, 1.0, shadow_visibility),
        camera.lighting.w,
    );
    let sun_tint = radiance_tint(camera.sun_radiance.xyz, vec3<f32>(1.0));
    let ambient_tint = radiance_tint(camera.ambient_radiance.xyz, vec3<f32>(1.0));
    let sun_active = camera.sun_direction.w;
    let ambient = input.color
        * ambient_tint
        * mix(0.22, 0.42, ambient_occlusion)
        * received_shadow;
    let diffuse = input.color
        * sun_tint
        * wrapped_diffuse
        * camera.lighting.x
        * shadow_visibility
        * sun_active;
    let transmission = input.color
        * sun_tint
        * back_light
        * camera.lighting.z
        * shadow_visibility
        * sun_active;
    let highlight = sun_tint
        * specular
        * camera.lighting.y
        * shadow_visibility
        * sun_active;
    return vec4<f32>(ambient + diffuse + transmission + highlight, 1.0);
}
