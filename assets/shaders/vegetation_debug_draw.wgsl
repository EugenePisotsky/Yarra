#import "shaders/grass_canopy.wgsl"::{canopy_visibility_at}

#import "shaders/vegetation_blade.wgsl"::{
    ProceduralInstance, DebugInstance, Species, Camera, DebugConfig, PreparedBlade, PreparedArena,
    PI, SPECIES_INDEX_MASK, LOD_MORPH_MASK, LOD_MORPH_SHIFT, LIGHTING_MODE_LEGACY,
    LIGHTING_MODE_UNLIT_DIAGNOSTIC, LIGHTING_MODE_VERTEX_ONLY_DIAGNOSTIC,
    FAR_WIDTH_TARGET_HALF_PIXELS, FAR_WIDTH_MAXIMUM_SCALE,
    FAR_WIDTH_FADE_START_METERS, FAR_WIDTH_FADE_END_METERS,
    hash32, random01, normalize3_or, prepare_blade, paired_base_width, paired_ribbon_linear_t, shape_inspection_mode, inspected_shape_morph, inspected_opening,
}

#import bevy_pbr::{
    lighting as pbr_lighting,
    mesh_view_bindings as view_bindings,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    shadows,
}

// Vegetation V2 procedural topology and placement diagnostics.
//
// The geometry path has no vertex streams. A fixed procedural vertex budget is decoded from
// vertex_index while instance_index selects compact data emitted by the placement compute pass.

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) blade_t: f32,
    // x: roughness, y: transmission, z: AO, w: geometry/diagnostic flag
    @location(4) material: vec4<f32>,
    @location(5) blade_u: f32,
    // xyz: terrain surface normal, w: stable clump variant
    @location(6) surface_normal_clump: vec4<f32>,
    // xyz: physical width axis before view opening, w: authored normal-rounding strength
    @location(7) ribbon_side_rounding: vec4<f32>,
    // Height above root plane, camera distance, grass-boundary depth, horizontal camera distance.
    @location(10) canopy_coordinates: vec4<f32>,
#ifdef BLADE_BAND_STUDY
    // Physical curve t, relative height, authored width / length, bounded wind drift.
    @location(8) band_coordinates: vec4<f32>,
    @location(9) @interpolate(flat) band_seed: u32,
#endif
}

// Group zero is Bevy's mesh-view bind group, including directional shadow cascades.
@group(1) @binding(0) var<storage, read> procedural_instances: array<ProceduralInstance>;
@group(1) @binding(1) var<storage, read> diagnostic_instances: array<DebugInstance>;
@group(1) @binding(2) var<storage, read> species: array<Species>;
@group(1) @binding(3) var<uniform> camera: Camera;
@group(1) @binding(4) var<uniform> debug_config: DebugConfig;
@group(1) @binding(5) var<storage, read> prepared_arena: PreparedArena;
// Independently uploaded, asynchronous canopy boundary; never part of placement coverage.
@group(1) @binding(6) var<storage, read> canopy_field: array<f32>;
fn canopy_edge_depth(root: vec2<f32>) -> f32 {
    if camera.canopy_appearance.x <= 0.0 { return 4.0; }
    if arrayLength(&canopy_field) < 9u { return 0.0; }
    let size = vec2<u32>(u32(canopy_field[3]), u32(canopy_field[4]));
    if size.x == 0u || size.y == 0u { return 0.0; }
    let grid = clamp((root - vec2(canopy_field[0], canopy_field[1])) / canopy_field[2] - vec2(0.5),
        vec2(0.0), vec2<f32>(size - vec2(1u)));
    let lo = vec2<u32>(floor(grid));
    let hi = min(lo + vec2(1u), size - vec2(1u));
    let t = fract(grid);
    return mix(mix(canopy_field[8u + lo.y * size.x + lo.x], canopy_field[8u + lo.y * size.x + hi.x], t.x),
        mix(canopy_field[8u + hi.y * size.x + lo.x], canopy_field[8u + hi.y * size.x + hi.x], t.x), t.y);
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

fn geometry_vertex(vertex_index: u32, instance_index: u32) -> VertexOutput {
    // Keep the indirect commands, instance counts, index fetches, and vertex invocations intact,
    // but avoid every procedural instance read and all deformation math. Comparing this mode with
    // the full vertex-only run separates raw topology throughput from vertex-shader cost.
    if (debug_config.values.z == LIGHTING_MODE_VERTEX_ONLY_DIAGNOSTIC) {
        var output: VertexOutput;
        output.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        output.color = vec3<f32>(0.0);
        output.world_normal = vec3<f32>(0.0, 1.0, 0.0);
        output.world_position = vec3<f32>(0.0);
        output.blade_t = 0.0;
        output.material = vec4<f32>(0.0);
        output.blade_u = 0.0;
        output.surface_normal_clump = vec4<f32>(0.0);
        output.ribbon_side_rounding = vec4<f32>(0.0);
        return output;
    }

    let instance = procedural_instances[instance_index];
    let inspection = shape_inspection_mode(debug_config);
    let low_lod = (instance.geometry.y >> 31u) != 0u && inspection == 0u;
    let profile = species[instance.geometry.y & SPECIES_INDEX_MASK];
    let production_morph = f32((instance.geometry.y >> LOD_MORPH_SHIFT) & LOD_MORPH_MASK)
        / f32(LOD_MORPH_MASK);
    let lod_morph = inspected_shape_morph(production_morph, debug_config);
    let is_broad_leaf = profile.root_color.w >= 1.5;
    let blade_index = (vertex_index >> 5u) & 1u;
    var topology_row = (vertex_index >> 1u) & 15u;
    var side_sign = select(-1.0, 1.0, (vertex_index & 1u) != 0u);
    var blade: PreparedBlade;
    var prepared_index = 0u;
    if (debug_config.workload.z != 0u) {
        prepared_index = prepared_arena.indices[instance_index];
    }
    if (prepared_index != 0u) {
        blade = prepared_arena.blades[prepared_index - 1u + blade_index];
    } else {
        blade = prepare_blade(instance, profile, camera, debug_config, blade_index);
    }
    let p0 = blade.p0_width.xyz;
    let p1 = blade.p1_authored_width.xyz;
    let p2 = blade.p2_phase.xyz;
    let p3 = blade.p3_amplitude.xyz;
    let half_width = blade.p0_width.w;
    let authored_half_width = blade.p1_authored_width.w;
    let blade_wind_phase = blade.p2_phase.w;
    let blade_wind_amplitude = max(blade.p3_amplitude.w, 0.0);
    let blade_side = blade.side.xyz;
    let animated_wind_forward = blade.wind_forward.xyz;
    let surface_normal = blade.surface_clump.xyz;
    let clump_variant = blade.surface_clump.w;
    let section_count = u32(blade.topology.x);
    let paired_ribbon = blade.topology.y < 0.0;
    let paired_main = paired_ribbon && blade_index == 0u;
    let paired_companion = paired_ribbon && blade_index != 0u;
    let low_section_count = u32(abs(blade.topology.y));

    // Sections above the species budget collapse at the tip. This lets all species in a topology
    // bin share one indirect command while retaining artist-controlled longitudinal distribution.
    var authored_linear_t = f32(min(topology_row, section_count)) / f32(section_count);
    if (paired_ribbon) {
        authored_linear_t = paired_ribbon_linear_t(topology_row, section_count, paired_main);
    }
    let low_linear_t = round(authored_linear_t * f32(low_section_count))
        / f32(low_section_count);
    let linear_t = select(
        mix(low_linear_t, authored_linear_t, lod_morph),
        authored_linear_t,
        low_lod || paired_ribbon,
    );
    let t = pow(linear_t, max(profile.topology.w, 0.2));
    // Production low paired blades have morph zero: the low-mesh branches below supply
    // their complete position and physical frame. Avoid building a cubic/view-opened ribbon
    // that those branches immediately replace. Keep inspection and all morphing paths intact.
    let fully_low_paired = low_lod && paired_ribbon && lod_morph == 0.0;
    var world_position = p0;
    var physical_normal = surface_normal;
    var local_ribbon_side = blade_side;
    if (!fully_low_paired) {
        var curve_position = cubic_bezier(p0, p1, p2, p3, t);
        var curve_derivative = cubic_bezier_derivative(p0, p1, p2, p3, t);
        if (blade_wind_amplitude > 1e-5) {
            // Ghost's grass bob offsets phase by both blade identity and position along the blade. The
            // analytic derivative keeps transported ribbon frames and rounded lighting attached to the
            // animated silhouette instead of shading the rest curve.
            let envelope = t * t;
            let envelope_derivative = 2.0 * t;
            let forward_phase = blade_wind_phase + t * 3.20;
            let side_phase = blade_wind_phase * 1.37 + 1.20 - t * 2.35;
            let detail_direction = animated_wind_forward * sin(forward_phase) * 0.72
                + blade_side * sin(side_phase) * 0.76
                + surface_normal * sin(forward_phase + 1.57) * 0.18;
            let detail_derivative = animated_wind_forward * cos(forward_phase) * 3.20 * 0.72
                - blade_side * cos(side_phase) * 2.35 * 0.76
                + surface_normal * cos(forward_phase + 1.57) * 3.20 * 0.18;
            curve_position += blade_wind_amplitude * envelope * detail_direction;
            curve_derivative += blade_wind_amplitude
                * (envelope_derivative * detail_direction + envelope * detail_derivative);
        }
        let curve_tangent = normalize3_or(curve_derivative, surface_normal);

        // Width is present at the root and tapers toward the tip, as in the folded-strip model.
        let ribbon_taper = max(1.0 - t * t, 0.0);
        let broad_taper = pow(max(sin(PI * t), 0.0), 0.58);
        let taper = select(ribbon_taper, broad_taper, is_broad_leaf);
        // Transport the authored root-side axis onto the plane perpendicular to the local Bezier
        // tangent. A constant root frame makes strongly curved ribbons kink and exposes their edge at
        // the wrong angle; the transported frame follows the curve without adding vertices.
        local_ribbon_side = normalize3_or(
            blade_side - curve_tangent * dot(blade_side, curve_tangent),
            blade_side,
        );
        physical_normal = normalize3_or(
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
        if (!is_broad_leaf && inspected_opening(profile, debug_config) > 0.0) {
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
            let opening_tangent = min(inspected_opening(profile, debug_config), requested_tangent);
            let opening_direction = opening_remainder / max(remainder_length, 1e-4);
            rendered_ribbon_side = normalize3_or(
                local_ribbon_side + opening_direction * opening_tangent,
                local_ribbon_side,
            );
        }
        // Preserve coverage without turning the far field into world-space slabs. Low LOD widens the
        // retained population sublinearly as candidates are removed; using an exponent below 0.5 keeps
        // this visibly weaker than full area preservation. The projected estimate additionally catches
        // very distant or edge-on subpixel ribbons. Both terms are derived independently of the
        // density-transition width, so blades selected for removal still contract all the way to zero.
        var far_width_scale = 1.0;
        if (camera.projection.w > 0.5 && !is_broad_leaf) {
            let low_lod_coverage_scale = blade.topology.z;
            let camera_distance = max(distance(camera.camera_position.xyz, curve_position), 0.05);
            let side_view_alignment = clamp(dot(rendered_ribbon_side, to_camera), -1.0, 1.0);
            let projected_side_factor = sqrt(max(
                1.0 - side_view_alignment * side_view_alignment,
                0.0,
            ));
            let projected_authored_half_width = authored_half_width
                * camera.projection.x
                * projected_side_factor
                / camera_distance;
            let required_scale = clamp(
                FAR_WIDTH_TARGET_HALF_PIXELS / max(projected_authored_half_width, 1e-4),
                1.0,
                FAR_WIDTH_MAXIMUM_SCALE,
            );
            let distance_weight = smoothstep(
                FAR_WIDTH_FADE_START_METERS,
                FAR_WIDTH_FADE_END_METERS,
                camera_distance,
            );
            let subpixel_width_scale = mix(1.0, required_scale, distance_weight);
            far_width_scale = max(low_lod_coverage_scale, subpixel_width_scale);
        }
        world_position = curve_position
            + rendered_ribbon_side * side_sign * half_width * far_width_scale * taper;
    }
    var shading_side = side_sign;
    var shading_t = t;
    if (paired_ribbon && !low_lod && topology_row == 0u) {
        world_position = p0 + side_sign * paired_base_width(blade, instance, camera)
            * (half_width / max(authored_half_width, 1e-6));
    }
    if (paired_main && lod_morph < 1.0) {
        // Morph positions onto the actual low mesh, rather than moving samples along the cubic.
        // The latter bunches rows and makes the high blade visibly kink before its bin changes.
        let shoulder = vec3(blade.side.w, blade.wind_forward.w, blade.topology.w);
        let shoulder_t = pow(0.5, max(profile.topology.w, 0.2));
        let upper_segment = t > shoulder_t;
        let segment_weight = select(t / shoulder_t, (t - shoulder_t) / (1.0 - shoulder_t), upper_segment);
        let low_center = mix(select(p0, shoulder, upper_segment), select(shoulder, p3, upper_segment), segment_weight);
        let shoulder_weight = select(segment_weight, 1.0 - segment_weight, upper_segment);
        shading_side *= mix(shoulder_weight, 1.0, lod_morph);
        let low_position = low_center + side_sign * (half_width / max(authored_half_width, 1e-6))
            * shoulder_weight * blade.wind_forward.xyz;
        world_position = mix(low_position, world_position, lod_morph);

        let shoulder_derivative = cubic_bezier_derivative(p0, p1, p2, p3, shoulder_t);
        let derivative_a = select(p1 - p0, shoulder_derivative, upper_segment);
        let derivative_b = select(shoulder_derivative, p3 - p2, upper_segment);
        let normal_a = normalize3_or(cross(blade_side, derivative_a), surface_normal);
        let normal_b = normalize3_or(cross(blade_side, derivative_b), surface_normal);
        let low_normal = mix(normal_a, normal_b, segment_weight);
        physical_normal = mix(low_normal, physical_normal, lod_morph);
        let tangent_a = normalize3_or(derivative_a, surface_normal);
        let tangent_b = normalize3_or(derivative_b, surface_normal);
        let side_a = normalize3_or(blade_side - tangent_a * dot(blade_side, tangent_a), blade_side);
        let side_b = normalize3_or(blade_side - tangent_b * dot(blade_side, tangent_b), blade_side);
        local_ribbon_side = mix(mix(side_a, side_b, segment_weight), local_ribbon_side, lod_morph);
    }

    if (paired_companion && lod_morph < 1.0) {
        // The first companion width row becomes the low triangle's base. The shared
        // root edge collapses with the main root, making the initial quad degenerate.
        var low_t = t;
        if (!low_lod) {
            low_t = clamp((t - blade_wind_phase) / max(1.0 - blade_wind_phase, 1e-5), 0.0, 1.0);
        }
        let root_collapse = select(1.0, 0.0, !low_lod && topology_row == 0u);
        let low_position = mix(p0, p3, low_t) + side_sign * (half_width / max(authored_half_width, 1e-6))
            * (1.0 - low_t) * root_collapse * blade.wind_forward.xyz;
        world_position = mix(low_position, world_position, lod_morph);
        let tangent_a = normalize3_or(p1 - p0, surface_normal);
        let tangent_b = normalize3_or(p3 - p2, surface_normal);
        let normal_a = normalize3_or(cross(blade_side, tangent_a), surface_normal);
        let normal_b = normalize3_or(cross(blade_side, tangent_b), surface_normal);
        physical_normal = mix(mix(normal_a, normal_b, low_t), physical_normal, lod_morph);
        let side_a = normalize3_or(blade_side - tangent_a * dot(blade_side, tangent_a), blade_side);
        let side_b = normalize3_or(blade_side - tangent_b * dot(blade_side, tangent_b), blade_side);
        local_ribbon_side = mix(mix(side_a, side_b, low_t), local_ribbon_side, lod_morph);
        shading_side *= mix(1.0 - low_t, 1.0, lod_morph);
        shading_t = mix(low_t, t, lod_morph);
    }

    // View opening is a silhouette correction, not a material deformation. Preserve the physical
    // blade frame so changing the opening does not rotate every blade's lighting away from the sun
    // and darken the field. The fragment shader reconstructs the rounded cross-section per pixel.
    let variation = (clump_variant * 2.0 - 1.0) * profile.material.x;
    let color = mix(profile.root_color.xyz, profile.tip_color_height.xyz, shading_t) * (1.0 + variation);

    var output: VertexOutput;
    output.clip_position = camera.clip_from_world * vec4<f32>(world_position, 1.0);
    output.color = color;
    output.world_normal = physical_normal;
    output.world_position = world_position;
    output.canopy_coordinates = vec4(max(0.0, dot(world_position - p0, surface_normal)),
        distance(camera.camera_position.xyz, p0), canopy_edge_depth(p0.xz),
        distance(camera.camera_position.xz, p0.xz));
    output.blade_t = shading_t;
    output.material = vec4<f32>(
        profile.material.y,
        profile.material.z,
        mix(profile.shading.x, profile.shading.y, shading_t),
        1.0,
    );
    output.blade_u = shading_side * 0.5 + 0.5;
    output.surface_normal_clump = vec4<f32>(surface_normal, clump_variant);
    output.ribbon_side_rounding = vec4<f32>(local_ribbon_side, profile.material.w);
#ifdef BLADE_BAND_STUDY
    // Identity excludes the high byte (LOD density) and compacted instance_index.
    let band_seed = hash32(instance.geometry.w & 0x00ffffffu);
    let h = vec3<f32>(dot(p1 - p0, surface_normal), dot(p2 - p0, surface_normal), dot(p3 - p0, surface_normal));
    // Cheap estimate of this blade's crown, not a measurement of neighboring occluders.
    let crown = max(0.05, max(h.z, max(dot(h, vec3(0.375, 0.375, 0.125)), dot(h, vec3(0.140625, 0.421875, 0.421875)))));
    let relative_height = dot(world_position - p0, surface_normal) / crown;
    let band_length = select(max(length(p3 - p0), 0.01), -blade.p3_amplitude.w, paired_ribbon);
    let normalized_width = clamp(2.0 * authored_half_width / band_length, 0.008, 0.085);
    let phase = camera.wind.w * camera.wind_shape.y
        + dot(instance.root_clump.xz + camera.render_origin.xy, camera.wind.xy) * camera.wind_shape.x
        + f32(band_seed & 255u) * (2.0 * PI / 255.0);
    let drift = normalized_width * min(camera.wind.z, 1.0)
        * (0.75 * sin(phase) + 0.25 * sin(phase * 1.37 + 1.2));
    output.band_coordinates = vec4<f32>(select(shading_t, -shading_t, blade_index != 0u), relative_height, normalized_width, drift);
    output.band_seed = band_seed;
#endif
    if (inspection == 4u || inspection == 5u) {
        // Green: full shape. Red: low endpoint. Orange: budget, blue: projected extent.
        output.color = mix(vec3(0.95, 0.04, 0.08), vec3(0.05, 0.90, 0.15), production_morph);
        if (inspection == 5u) {
            let decision = diagnostic_instances[instance_index].diagnostics;
            let limiting_color = select(vec3(0.05, 0.35, 1.0), vec3(1.0, 0.35, 0.03), decision.y < decision.x);
            output.color = select(limiting_color, vec3(0.05, 0.90, 0.15), production_morph >= 0.999);
        }
        output.material.w = 0.0;
    }

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
    output.blade_u = strip.y * 0.5 + 0.5;
    output.surface_normal_clump = vec4<f32>(surface_normal, instance.direction_species.y);
    output.ribbon_side_rounding = vec4<f32>(side, 0.0);
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

fn stable_clump_normal(surface_normal: vec3<f32>, clump_variant: f32) -> vec3<f32> {
    let tangent = normalize3_or(
        cross(vec3<f32>(0.0, 1.0, 0.0), surface_normal),
        vec3<f32>(1.0, 0.0, 0.0),
    );
    let bitangent = normalize3_or(
        cross(surface_normal, tangent),
        vec3<f32>(0.0, 0.0, 1.0),
    );
    let angle = clump_variant * 2.0 * PI;
    let tilt_direction = tangent * cos(angle) + bitangent * sin(angle);
    // A shallow, group-stable tilt makes neighboring blades enter and leave the broad highlight
    // together. The surface normal keeps the response attached to terrain relief.
    return normalize3_or(surface_normal + tilt_direction * 0.11, surface_normal);
}

fn analytic_rounded_normal(
    flat_normal: vec3<f32>,
    ribbon_side: vec3<f32>,
    blade_u: f32,
    authored_rounding: f32,
) -> vec3<f32> {
    let orthogonal_side = normalize3_or(
        ribbon_side - flat_normal * dot(ribbon_side, flat_normal),
        vec3<f32>(1.0, 0.0, 0.0),
    );
    // A projected cylinder has x = sin(theta). Reconstructing cos(theta) here gives every covered
    // pixel a genuine curved cross-section instead of merely interpolating two slightly tilted edge
    // normals. Most authored values predate this shader and occupy the lower 0..0.4 range, so that
    // range deliberately spans flat to nearly cylindrical while retaining zero as exactly flat.
    let signed_width = clamp(blade_u * 2.0 - 1.0, -0.995, 0.995);
    let cylinder_height = sqrt(max(1.0 - signed_width * signed_width, 0.0));
    let cylinder_normal = normalize3_or(
        flat_normal * cylinder_height + orthogonal_side * signed_width,
        flat_normal,
    );
    let rounding = smoothstep(0.0, 0.42, authored_rounding);
    return normalize3_or(mix(flat_normal, cylinder_normal, rounding), flat_normal);
}

fn ggx_foliage_specular(
    normal: vec3<f32>,
    view_direction: vec3<f32>,
    light_direction: vec3<f32>,
    half_direction: vec3<f32>,
    alpha_roughness: f32,
) -> f32 {
    let n_dot_l = max(dot(normal, light_direction), 0.0);
    let n_dot_v = max(dot(normal, view_direction), 0.04);
    let n_dot_h = max(dot(normal, half_direction), 0.0);
    let v_dot_h = clamp(dot(view_direction, half_direction), 0.0, 1.0);
    let one_minus_v_dot_h = 1.0 - v_dot_h;
    let one_minus_v_dot_h_2 = one_minus_v_dot_h * one_minus_v_dot_h;
    let fresnel = 0.04
        + 0.96 * one_minus_v_dot_h_2 * one_minus_v_dot_h_2 * one_minus_v_dot_h;
    let distribution = pbr_lighting::D_GGX(alpha_roughness, n_dot_h);
    let visibility = pbr_lighting::V_SmithGGXCorrelated(
        alpha_roughness,
        n_dot_v,
        max(n_dot_l, 0.04),
    );
    return min(fresnel * distribution * visibility * n_dot_l, 1.5);
}

#ifdef BLADE_BAND_STUDY
// Advect the sampling coordinate, not a center trapped inside a stationary slot.
// Neighboring cells allow independently tilted marks to cross cell boundaries.
fn study_band_mask(input: VertexOutput) -> f32 {
    let mark_seed = hash32(input.band_seed ^ select(0u, 0x9e3779b9u, input.band_coordinates.x < 0.0));
    let blade_random = vec3<f32>(f32(mark_seed & 255u),
        f32((mark_seed >> 8u) & 255u), f32((mark_seed >> 16u) & 255u)) / 255.0;
    let count = mix(5.0, 10.0, blade_random.x);
    let warp = mix(0.20, 0.70, blade_random.y);
    let t = abs(input.band_coordinates.x);
    let width = input.band_coordinates.z;
    let height = input.band_coordinates.y;
    // At full wind a mark travels up to 1.25 authored widths from its rest position.
    let sample_t = t - 1.25 * input.band_coordinates.w;
    let slope = count * (1.0 + warp - 2.0 * warp * sample_t);
    let axis = count * sample_t * (1.0 + warp - warp * sample_t) + blade_random.z;
    let across = (input.blade_u - 0.5) * min(width * slope, 0.70);
    // Derivatives of continuous coordinates only; hashing/floor must not enter AA.
    let dx = vec2<f32>(dpdx(axis), dpdx(across));
    let dy = vec2<f32>(dpdy(axis), dpdy(across));
    let cell = i32(floor(axis));
    let density = f32((debug_config.workload.w >> 16u) & 255u) / 255.0;
    // Even dense grass has missing marks. Varying centers and spacing avoids a comb.
    let probability = density * mix(0.82, 0.62, smoothstep(0.2, 0.85, height));
    var mask = 0.0;
    for (var offset = -1; offset <= 1; offset += 1) {
        let candidate = cell + offset;
        let seed = hash32(mark_seed ^ (bitcast<u32>(candidate) * 0x85ebca6bu));
        let r = vec4<f32>(f32(seed & 255u), f32((seed >> 8u) & 255u),
            f32((seed >> 16u) & 255u), f32(seed >> 24u)) / 255.0;
        let center = 0.15 + 0.70 * r.x;
        let tilt = (2.0 * r.y - 1.0) * 1.80;
        let local = axis - f32(candidate) + across * tilt;
        let pixel_span = abs(dot(dx, vec2(1.0, tilt))) + abs(dot(dy, vec2(1.0, tilt)));
        let band_width = clamp(width * slope * (0.55 + r.z * 0.85), 0.025, 0.52);
        let feather = min(0.11, max(band_width * 0.20, pixel_span * 0.60));
        let shape = 1.0 - smoothstep(band_width * 0.5 - feather, band_width * 0.5 + feather, abs(local - center));
        let resolved = smoothstep(0.7, 2.0, band_width / max(pixel_span, 0.00001));
        let presence = 1.0 - smoothstep(probability - 0.08, probability + 0.08, r.w);
        mask = max(mask, shape * resolved * presence * mix(0.75, 1.0, r.z));
    }
    // Max reach: 0.35*1.8 tilt + 0.52/2 width + 0.11 feather = 1 cell.
    // Centers stay in [0.15,0.85], so the three candidates include every contributor.
    let crown_clearance = 1.0 - smoothstep(0.70, 1.05, height);
    let height_strength = mix(1.0, 0.55, smoothstep(0.20, 0.85, height));
    let root_clearance = smoothstep(0.005, 0.025, t);
    return mask * crown_clearance * height_strength
        * root_clearance * smoothstep(0.0, 0.15, density);
}
#endif

@fragment
fn fragment(
    input: VertexOutput,
) -> @location(0) vec4<f32> {
    if (input.material.w < 0.5) {
        return vec4<f32>(input.color, 1.0);
    }
#ifdef BLADE_BAND_STUDY
    let band_mask = study_band_mask(input);
#ifdef BLADE_BAND_MASK
    return vec4<f32>(vec3<f32>(1.0 - band_mask), 1.0);
#endif
    let band_visibility = 1.0 - band_mask * (f32(#{BLADE_BAND_STRENGTH}) / 100.0);
#else
    let band_visibility = 1.0;
#endif
    if (debug_config.values.z == LIGHTING_MODE_UNLIT_DIAGNOSTIC) {
        return vec4<f32>(input.color, 1.0);
    }

    let view_direction = normalize3_or(
        camera.camera_position.xyz - input.world_position,
        vec3<f32>(0.0, 1.0, 0.0),
    );
    // Vegetation ribbons are two-sided. Face the physical blade plane toward the viewer instead of
    // deriving its sign from triangle winding: adjacent transported width axes can twist a highly
    // curved strip without meaning that its lighting side should invert.
    let face_sign = select(-1.0, 1.0, dot(input.world_normal, view_direction) >= 0.0);
    let flat_blade_normal = normalize3_or(
        input.world_normal * face_sign,
        vec3<f32>(0.0, 1.0, 0.0),
    );
    let blade_normal = analytic_rounded_normal(
        flat_blade_normal,
        input.ribbon_side_rounding.xyz,
        input.blade_u,
        input.ribbon_side_rounding.w,
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
    let ambient_occlusion = clamp(input.material.z, 0.0, 1.0);
    let canopy_visibility = canopy_visibility_at(input.world_position.xz,
        input.canopy_coordinates.x, input.canopy_coordinates.y,
        camera.canopy_appearance, camera.canopy_shape, camera.canopy_distance,
        camera.canopy_origin, camera.canopy_appearance.z, input.canopy_coordinates.z, input.canopy_coordinates.w);
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
    let sun_active = camera.sun_direction.w;

    if (debug_config.values.z == LIGHTING_MODE_LEGACY) {
        let ambient_tint = radiance_tint(camera.ambient_radiance.xyz, vec3<f32>(1.0));
        let ambient = input.color
            * ambient_tint
            * mix(0.22, 0.42, ambient_occlusion)
            * received_shadow
            * mix(1.0, band_visibility, 0.65);
        let wrapped_diffuse = clamp((dot(normal, light_direction) + 0.48) / 1.48, 0.0, 1.0);
        let back_light = pow(max(dot(-normal, light_direction), 0.0), 1.5)
            * input.material.y;
        let roughness = clamp(input.material.x, 0.04, 1.0);
        let specular_power = mix(96.0, 4.0, roughness);
        let specular = pow(max(dot(normal, half_direction), 0.0), specular_power)
            * mix(0.24, 0.035, roughness)
            * (1.0 - distance_stability);
        let diffuse = input.color
            * sun_tint
            * wrapped_diffuse
            * camera.lighting.x
            * shadow_visibility * band_visibility
            * sun_active;
        let transmission = input.color
            * sun_tint
            * back_light
            * camera.lighting.z
            * shadow_visibility * band_visibility
            * sun_active;
        let highlight = sun_tint
            * specular
            * camera.lighting.y
            * shadow_visibility * band_visibility
            * sun_active;
        return vec4<f32>((ambient + diffuse + transmission + highlight) * canopy_visibility, 1.0);
    }

    // Direct-light energy must use the same camera exposure as Bevy's PBR path. Normalizing the
    // directional radiance to a tint made a 100,000-lux sun indistinguishable from a dim light.
    let exposed_sun = camera.sun_radiance.xyz * view_bindings::view.exposure;
    let exposed_sun_peak = max(max(exposed_sun.r, exposed_sun.g), exposed_sun.b);
    let exposed_sun_tint = radiance_tint(exposed_sun, sun_tint);
    let sun_elevation = clamp(light_direction.y, 0.0, 1.0);
    let low_sun = 1.0 - smoothstep(0.28, 0.72, sun_elevation);

    // Rounded local normals move the highlight from one side of a nearby ribbon to the other.
    // Once that variation becomes unresolved, converge the broad specular response on a stable
    // clump normal. Diffuse lighting below retains each blade's direction: sharing this normal
    // with diffuse made the field uniformly lit when viewed away from the sun.
    let surface_normal = normalize3_or(
        input.surface_normal_clump.xyz,
        vec3<f32>(0.0, 1.0, 0.0),
    );
    let clump_normal = stable_clump_normal(surface_normal, input.surface_normal_clump.w);
    let shading_normal = normalize3_or(
        mix(blade_normal, clump_normal, distance_stability),
        clump_normal,
    );
    let perceptual_roughness = clamp(input.material.x, 0.08, 1.0);
    let base_alpha_roughness = perceptual_roughness * perceptual_roughness;
    let normal_width = fwidth(blade_normal);
    let normal_variance = clamp(dot(normal_width, normal_width), 0.0, 0.5);
    // Treat screen-space normal variance as unresolved microgeometry. Distance roughening remains
    // necessary for narrow far ribbons whose derivatives can be deceptively uniform per triangle.
    let filtered_alpha_roughness = clamp(
        base_alpha_roughness
            + normal_variance * 0.72
            + distance_stability * 0.34,
        0.035,
        0.95,
    );
    let filtered_broad_specular = ggx_foliage_specular(
        shading_normal,
        view_direction,
        light_direction,
        half_direction,
        filtered_alpha_roughness,
    );
    // Leaves have a narrow waxy sheen sitting over the broad material response. Keeping this lobe
    // distinct is what makes the half-vector select one side of the analytic cylinder; a single
    // high-roughness lobe only reads as a field-wide brightness gradient.
    let local_sheen_weight = 1.0 - smoothstep(22.0, 48.0, camera_distance);
    var local_sheen_specular = 0.0;
    // Once the narrow lobe has faded out, avoid evaluating a second GGX response.
    // Normal derivatives remain above this branch so filtering stays quad-consistent.
    if local_sheen_weight > 0.0 {
        let sheen_alpha_roughness = clamp(
            mix(0.10, 0.22, perceptual_roughness) + normal_variance * 0.24,
            0.08,
            0.48,
        );
        local_sheen_specular = ggx_foliage_specular(
            blade_normal,
            view_direction,
            light_direction,
            half_direction,
            sheen_alpha_roughness,
        );
    }
    // The narrow waxy lobe is useful while a blade has a resolvable width, but becomes glitter once
    // it is a subpixel line. A broad clump lobe survives so sunrise still sweeps coherently across
    // the field instead of flattening to diffuse-only shading.
    let broad_specular_weight = mix(0.16, 0.26, distance_stability);
    let specular_lobe = filtered_broad_specular * broad_specular_weight
        + local_sheen_specular * local_sheen_weight;

    let upper_ribbon = smoothstep(0.14, 0.78, input.blade_t);
    let far_highlight_weight = mix(
        1.0,
        0.08,
        smoothstep(38.0, 84.0, camera_distance),
    );
    let specular_elevation = mix(0.22, 1.0, low_sun);
    let specular_energy = min(exposed_sun_peak, 8.0);
    let highlight = exposed_sun_tint
        * specular_lobe
        * upper_ribbon
        * specular_elevation
        * far_highlight_weight
        * specular_energy
        * camera.lighting.y
        * shadow_visibility * band_visibility
        * sun_active;

    // A leaf's body is a thin sheet. The strongly rounded normal above shapes its waxy gloss;
    // using that same cylinder for diffuse rolls exposed margins out of the sun and paints dark
    // longitudinal stripes onto a lit face. Retain only a shallow fold for body illumination.
    let diffuse_normal = normalize3_or(
        mix(flat_blade_normal, blade_normal, 0.18 * (1.0 - distance_stability)),
        flat_blade_normal,
    );
    let leaf_n_dot_l = dot(diffuse_normal, light_direction);
    let directional_diffuse = clamp((leaf_n_dot_l + 0.18) / 1.18, 0.0, 1.0);
    let canopy_diffuse = clamp((dot(clump_normal, light_direction) + 0.18) / 1.18, 0.0, 1.0);
    // Average part of the unresolved illumination, retaining 35% of the blade response.
    // Leaving its full contrast on subpixel ribbons produces a stippled horizon; replacing it
    // completely with a common normal removes the directional variation we need to preserve.
    let diffuse_filter = distance_stability * 0.65;
    let wrapped_diffuse = mix(directional_diffuse, canopy_diffuse, diffuse_filter);
    let diffuse_energy = min(exposed_sun_peak * camera.lighting.x, 1.20);
    let diffuse = input.color
        * exposed_sun_tint
        * wrapped_diffuse
        * diffuse_energy
        * shadow_visibility * band_visibility
        * sun_active;

    let backscatter_alignment = clamp(dot(-view_direction, light_direction), 0.0, 1.0);
    let backscatter = smoothstep(0.14, 0.86, backscatter_alignment);
    let transmission_energy = min(exposed_sun_peak, 3.0);
    let transmission = mix(
        input.color * exposed_sun_tint,
        exposed_sun_tint,
        0.22,
    ) * backscatter
        * low_sun
        * upper_ribbon
        * input.material.y
        * far_highlight_weight
        * transmission_energy
        * camera.lighting.z
        * shadow_visibility * band_visibility
        * sun_active;

    // A broad upper-hemisphere fill keeps sky-facing surfaces readable between sun highlights.
    // Use scene intensity and exposure rather than a normalized tint plus fixed body brightness.
    // This is directional ambient illumination, not visibility of neighboring blades.
    let exposed_ambient = camera.ambient_radiance.xyz * view_bindings::view.exposure;
    let ambient_peak = max(max(exposed_ambient.r, exposed_ambient.g), exposed_ambient.b);
    let bounded_ambient = exposed_ambient / max(1.0, ambient_peak / 0.60);
    // Both sides of a thin leaf receive sky fill; viewer-facing normal flips must not blacken
    // the underside of an otherwise exposed leaf. Directional contrast comes from the sun term.
    let sky_facing = mix(abs(flat_blade_normal.y), clump_normal.y, diffuse_filter);
    let sky_fill = mix(0.75, 1.0, clamp(sky_facing, 0.0, 1.0));
    let foliage_ambient = input.color
        * bounded_ambient
        * mix(0.40, 0.85, ambient_occlusion)
        * sky_fill
        * received_shadow
        * mix(1.0, band_visibility, 0.65);

    return vec4<f32>((foliage_ambient + diffuse + transmission + highlight) * canopy_visibility, 1.0);
}
