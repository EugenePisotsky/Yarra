// Procedural blade geometry. There are no vertex streams: a fixed procedural vertex budget is decoded
// from vertex_index while instance_index selects the compact data placement emitted.
import package::shaders::vegetation::types::{
    Camera, PI, SPECIES_INDEX_MASK, LOD_MORPH_MASK, LOD_MORPH_SHIFT,
    LIGHTING_MODE_VERTEX_ONLY_DIAGNOSTIC,
};
import package::shaders::vegetation::blade::{
    PreparedBlade, FAR_WIDTH_TARGET_HALF_PIXELS, FAR_WIDTH_MAXIMUM_SCALE,
    FAR_WIDTH_FADE_START_METERS, FAR_WIDTH_FADE_END_METERS,
    normalize3_or, prepare_blade, paired_base_width, paired_ribbon_linear_t,
};
import package::shaders::vegetation::draw::bindings::{
    VertexOutput, procedural_instances, species, camera, config, prepared_arena, canopy_field,
};
@if(TEMPORAL_GRASS)
import package::shaders::vegetation::draw::bindings::previous_prepared_arena;

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

fn geometry_pose(vertex_index: u32, instance_index: u32, pose_camera: Camera, previous: bool) -> VertexOutput {
    // Keep the indirect commands, instance counts, index fetches, and vertex invocations intact,
    // but avoid every procedural instance read and all deformation math. Comparing this mode with
    // the full vertex-only run separates raw topology throughput from vertex-shader cost.
    if (config.values.z == LIGHTING_MODE_VERTEX_ONLY_DIAGNOSTIC) {
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

    var instance = procedural_instances[instance_index];
    if previous {
        // Both poses use the same stable root/seed, independent of atomic append order.
        instance.root_clump.x += camera.render_origin.x - pose_camera.render_origin.x;
        instance.root_clump.z += camera.render_origin.y - pose_camera.render_origin.y;
    }
    let low_lod = (instance.geometry.y >> 31u) != 0u;
    let profile = species[instance.geometry.y & SPECIES_INDEX_MASK];
    let lod_morph = f32((instance.geometry.y >> LOD_MORPH_SHIFT) & LOD_MORPH_MASK)
        / f32(LOD_MORPH_MASK);
    let is_broad_leaf = profile.root_color.w >= 1.5;
    let blade_index = (vertex_index >> 5u) & 1u;
    var topology_row = (vertex_index >> 1u) & 15u;
    var side_sign = select(-1.0, 1.0, (vertex_index & 1u) != 0u);
    var blade: PreparedBlade;
    var prepared_index = 0u;
    if (config.workload.z != 0u && !previous) {
        prepared_index = prepared_arena.indices[instance_index];
    }
    @if(TEMPORAL_GRASS)
    if previous && config.workload.z != 0u { prepared_index = previous_prepared_arena.indices[instance_index]; }
    if (prepared_index != 0u) {
        blade = prepared_arena.blades[prepared_index - 1u + blade_index];
        @if(TEMPORAL_GRASS)
        if previous { blade = previous_prepared_arena.blades[prepared_index - 1u + blade_index]; }
    } else {
        blade = prepare_blade(instance, profile, pose_camera, config, blade_index);
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
    // that those branches immediately replace. Keep all morphing paths intact.
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
        let to_camera = normalize3_or(pose_camera.camera_position.xyz - curve_position, physical_normal);
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
        // Preserve coverage without turning the far field into world-space slabs. Low LOD widens the
        // retained population sublinearly as candidates are removed; using an exponent below 0.5 keeps
        // this visibly weaker than full area preservation. The projected estimate additionally catches
        // very distant or edge-on subpixel ribbons. Both terms are derived independently of the
        // density-transition width, so blades selected for removal still contract all the way to zero.
        var far_width_scale = 1.0;
        if (pose_camera.projection.w > 0.5 && !is_broad_leaf) {
            let low_lod_coverage_scale = blade.topology.z;
            let camera_distance = max(distance(pose_camera.camera_position.xyz, curve_position), 0.05);
            let side_view_alignment = clamp(dot(rendered_ribbon_side, to_camera), -1.0, 1.0);
            let projected_side_factor = sqrt(max(
                1.0 - side_view_alignment * side_view_alignment,
                0.0,
            ));
            let projected_authored_half_width = authored_half_width
                * pose_camera.projection.x
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
        world_position = p0 + side_sign * paired_base_width(blade, instance, pose_camera)
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
    output.clip_position = pose_camera.clip_from_world * vec4<f32>(world_position, 1.0);
    output.color = color;
    output.world_normal = physical_normal;
    output.world_position = world_position;
    output.canopy_coordinates = vec4(max(0.0, dot(world_position - p0, surface_normal)),
        distance(pose_camera.camera_position.xyz, p0), canopy_edge_depth(p0.xz),
        distance(pose_camera.camera_position.xz, p0.xz));
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
    return output;
}
