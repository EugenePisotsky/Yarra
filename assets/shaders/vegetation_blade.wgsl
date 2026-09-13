struct ProceduralInstance {
    // xyz: root, w: packed clump variant and nested LOD rank
    root_clump: vec4<f32>,
    // x: packed rest direction, y: species index + LOD morph + low flag,
    // z: packed surface normal xz,
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
    // x: root AO, y: tip AO, z: high-LOD threshold, w: reserved
    shading: vec4<f32>,
    // xyz: group coherence for height, complete silhouette, and lateral curve
    group_response: vec4<f32>,
    // x: short/tall height bias, y: pair-below height, zw: single/split high-topology radii
    height_packing: vec4<f32>,
}

struct Camera {
    clip_from_world: mat4x4<f32>,
    camera_position: vec4<f32>,
    // x: vertical focal length in pixels, y: viewport width, z: viewport height,
    // w: far-ribbon screen-space width compensation enabled
    projection: vec4<f32>,
    // xyz: direction from the surface toward the strongest directional light, w: active
    sun_direction: vec4<f32>,
    // xyz: strongest directional-light color and global ambient-light color
    sun_radiance: vec4<f32>,
    ambient_radiance: vec4<f32>,
    // x: diffuse, y: specular, z: transmission, w: received-shadow strength
    lighting: vec4<f32>,
    // xy: world-XZ direction, z: maximum tip displacement / height, w: phase seconds
    wind: vec4<f32>,
    // x: spatial frequency, y: speed, z: gustiness, w: hashed blade flutter
    wind_shape: vec4<f32>,
}

struct DebugConfig {
    // x: 0 geometry, 1 accepted species, 2 parent links, 3 outcomes, 4 group structure
    // y: 0 authored density, 1 balanced production density, 2 full-density reference
    // z: 0 rounded/clump gloss, 1 legacy empirical lighting
    // w: scene-adaptive single-low arena capacity (draw uses it only through indirect offsets)
    values: vec4<u32>,
    // x: live work items, y: diagnostic counters enabled, z: prepared blade data available
    workload: vec4<u32>,
}

const PI: f32 = 3.141592653589793;
const MAX_SECTIONS: u32 = 8u;
const MAX_LOW_SECTIONS: u32 = 3u;
const DENSITY_MODE_BALANCED: u32 = 1u;
const DENSITY_MODE_FULL_REFERENCE: u32 = 2u;
const SPECIES_INDEX_MASK: u32 = 0x0000ffffu;
const LOD_MORPH_MASK: u32 = 0x00007fffu;
const LOD_MORPH_SHIFT: u32 = 16u;
const BALANCED_DENSITY_FADE_BAND: f32 = 0.10;
const SPLIT_LOW_DENSITY_BUDGET_SCALE: f32 = 0.65;
const LIGHTING_MODE_LEGACY: u32 = 1u;
const LIGHTING_MODE_UNLIT_DIAGNOSTIC: u32 = 2u;
const LIGHTING_MODE_VERTEX_ONLY_DIAGNOSTIC: u32 = 3u;
const FAR_WIDTH_TARGET_HALF_PIXELS: f32 = 0.60;
const FAR_WIDTH_MAXIMUM_SCALE: f32 = 1.75;
const FAR_WIDTH_FADE_START_METERS: f32 = 14.0;
const FAR_WIDTH_FADE_END_METERS: f32 = 30.0;
const LOW_LOD_COVERAGE_WIDTH_EXPONENT: f32 = 0.35;

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

fn rotate_by_quaternion(value: vec3<f32>, rotation: vec4<f32>) -> vec3<f32> {
    return value + 2.0 * cross(rotation.xyz, cross(rotation.xyz, value) + rotation.w * value);
}

struct PreparedBlade {
    p0_width: vec4<f32>,
    p1_authored_width: vec4<f32>,
    // xyz: p2, w: broad-leaf flutter phase / paired ribbon first high-row parameter.
    p2_phase: vec4<f32>,
    p3_amplitude: vec4<f32>,
    side: vec4<f32>,
    wind_forward: vec4<f32>,
    surface_clump: vec4<f32>,
    // x: section count, y: low section count (negative for paired ribbons), z: coverage scale.
    // Paired main blades store low shoulder xyz in side.w / wind_forward.w / topology.w;
    // wind_forward.xyz stores the opened low width vector (shoulder for main, root for companion).
    // Arena size stays 128 bytes. These values are needed only while morphing or at low LOD.
    topology: vec4<f32>,
}

struct PreparedArena {
    // Zero means fallback to the original calculation; otherwise first blade index + 1.
    indices: array<u32, 344064>,
    blades: array<PreparedBlade>,
}

// workload.w bits 4..7 are a bounded shape study; bit 8 isolates ribbon view opening.
// Keep production density decisions in the packed instance, independent of the displayed shape.
fn shape_inspection_mode(config: DebugConfig) -> u32 {
    return select(0u, (config.workload.w >> 4u) & 15u, config.values.x == 0u);
}

fn inspected_shape_morph(original: f32, config: DebugConfig) -> f32 {
    let mode = shape_inspection_mode(config);
    if (mode == 2u) { return 1.0; }
    if (mode == 3u) { return 0.0; }
    return original;
}

fn inspected_opening(profile: Species, config: DebugConfig) -> f32 {
    return select(profile.shape_secondary.z, 0.0,
        shape_inspection_mode(config) != 0u && (config.workload.w & 256u) != 0u);
}

// Spend the fixed rows where the authored arches change direction. The main shoulder remains
// exactly at 0.5 before the authored power, preserving the existing low-kite anchor.
fn paired_ribbon_linear_t(row: u32, sections: u32, main: bool) -> f32 {
    if (main && sections == 5u) {
        return array<f32, 6>(0.0, 0.128, 0.292, 0.5, 0.768, 1.0)[min(row, 5u)];
    }
    if (!main && sections == 4u) {
        return array<f32, 5>(0.0, 0.183, 0.423, 0.723, 1.0)[min(row, 4u)];
    }
    if (main) {
        let middle = max((sections + 1u) / 2u, 1u);
        let r = min(row, sections);
        if (r <= middle) { return 0.5 * f32(r) / f32(middle); }
        return 0.5 + 0.5 * f32(r - middle) / f32(sections - middle);
    }
    return f32(min(row, sections)) / f32(sections);
}

fn prepare_blade(
    instance: ProceduralInstance, profile: Species, camera: Camera,
    debug_config: DebugConfig, blade_index: u32,
) -> PreparedBlade {
    let production_low_lod = (instance.geometry.y >> 31u) != 0u;
    let low_lod = production_low_lod && shape_inspection_mode(debug_config) == 0u;
    let lod_morph = f32((instance.geometry.y >> LOD_MORPH_SHIFT) & LOD_MORPH_MASK)
        / f32(LOD_MORPH_MASK);
    let geometry_morph = inspected_shape_morph(lod_morph, debug_config);
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
    let seed = instance.geometry.w & 0x00ffffffu;
    let population_density = f32(instance.geometry.w >> 24u) / 255.0;
    let unit_seed = hash32(seed);
    let source_height_coordinate = mix(
        random01(unit_seed ^ 0xa511e9b3u),
        random01(group_key ^ 0x52dce729u),
        profile.group_response.x,
    );
    let height_exponent = exp2(-2.0 * profile.height_packing.x);
    let height_coordinate = pow(
        clamp(source_height_coordinate, 0.0, 1.0),
        height_exponent,
    );
    var height = mix(profile.bounds.x, profile.bounds.y, height_coordinate);
    let is_broad_leaf = profile.root_color.w >= 1.5;
    var blade_count = clamp(u32(profile.topology.z + 0.5), 1u, 2u);
    if (!is_broad_leaf && profile.height_packing.y > 0.0) {
        blade_count = select(1u, 2u, height <= profile.height_packing.y);
    }
    let paired_main = !is_broad_leaf && blade_count > 1u && blade_index == 0u;
    let paired_companion = !is_broad_leaf && blade_count > 1u && blade_index != 0u;
    if (paired_companion) {
        height *= 0.80;
    }
    // Generation already classified this blade using its complete projected envelope. Reuse that
    // per-instance result rather than repeating five clip-space projections for every vertex.

    var forward = vec3<f32>(rest_direction.x, 0.0, rest_direction.y);
    forward -= surface_normal * dot(forward, surface_normal);
    forward = normalize3_or(forward, vec3<f32>(1.0, 0.0, 0.0));
    let base_side = normalize3_or(cross(surface_normal, forward), vec3<f32>(0.0, 0.0, 1.0));

    // Static u16 indices encode side in bit 0, row in bits 1..4, and blade in bit 5. A paired
    // render unit gives five sections to the long blade and four to its shorter companion.
    // Shared pointed roots and tips keep the original 18-input / 14-triangle high budget.
    let authored_section_count = select(profile.topology.x, profile.topology.y, low_lod);
    var maximum_sections = select(MAX_SECTIONS, MAX_LOW_SECTIONS, low_lod);
    if (blade_count > 1u) {
        maximum_sections = select(select(5u, 4u, blade_index != 0u), select(2u, 1u, blade_index != 0u), low_lod);
    }
    if (is_broad_leaf) {
        maximum_sections = select(select(4u, 3u, blade_index != 0u), 1u, low_lod);
    }
    var section_count = clamp(u32(authored_section_count + 0.5), 1u, maximum_sections);
    if (paired_main) {
        section_count = max(section_count, 2u);
    }

    let blade_seed = hash32(seed ^ blade_index * 0x9e3779b9u);
    let paired_side = select(-1.0, 1.0, blade_index != 0u);
    let facing_jitter = (random01(blade_seed ^ 0x68e31da4u) - 0.5)
        * select(0.7, 0.34, is_broad_leaf);
    let spread_angle = paired_side * profile.shape_secondary.y * 0.5 + facing_jitter;
    let blade_forward = normalize3_or(
        forward * cos(spread_angle) + base_side * sin(spread_angle),
        forward,
    );
    var blade_side = normalize3_or(cross(surface_normal, blade_forward), base_side);

    // Density LOD is a coverage transition, not a growth animation. Keeping the complete
    // centreline prevents a rejected subset from visibly rising out of the ground. Both topology
    // bins must meet at the same width, including the partially retained stable-rank fade band.
    // Otherwise survivors snap narrower and retiring blades reappear at the bin boundary.
    var balanced_low_width = select(0.0, 1.0, lod_rank < population_density);
    if (
        debug_config.values.y == DENSITY_MODE_BALANCED
        && population_density < 0.999
    ) {
        let half_band = BALANCED_DENSITY_FADE_BAND * 0.5;
        let budget_band = half_band * select(1.0, SPLIT_LOW_DENSITY_BUDGET_SCALE, blade_count > 1u);
        balanced_low_width = 1.0 - smoothstep(
            max(population_density - budget_band, 0.0),
            min(population_density + budget_band, 1.0),
            lod_rank,
        );
    }
    let high_transition_width = mix(balanced_low_width, 1.0, lod_morph);
    let density_width = select(high_transition_width, balanced_low_width, production_low_lod);
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
    let authored_half_width = mix(
        profile.bounds.z,
        profile.bounds.w,
        random01(blade_seed ^ 0x63d83595u),
    );
    let half_width = density_width * authored_half_width;
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
    var p3 = curve_root + surface_normal * height * tilt_cosine + blade_forward * tip_forward;
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
        if (paired_companion) {
            // The shorter companion retains its authored gentler curve.
            p1 = mix(mix(p0, p3, 1.0 / 3.0), p1, 0.75);
            p2 = mix(mix(p0, p3, 2.0 / 3.0), p2, 0.75);
        }
    }

    // The shared field supplies the dominant push, while a hashed phase keeps blades inside one
    // clump from moving in lockstep. Both topology LODs still sample the same deformed cubic.
    var animated_wind_forward = blade_forward;
    var blade_wind_phase = 0.0;
    var blade_wind_amplitude = 0.0;
    if (camera.wind.z > 1e-5) {
        let horizontal_wind = normalize3_or(
            vec3<f32>(camera.wind.x, 0.0, camera.wind.y),
            blade_forward,
        );
        let wind_forward = normalize3_or(
            horizontal_wind - surface_normal * dot(horizontal_wind, surface_normal),
            blade_forward,
        );
        let wind_direction = normalize(camera.wind.xy);
        let cross_direction = vec2<f32>(-wind_direction.y, wind_direction.x);
        let frequency = camera.wind_shape.x;
        let wind_speed = camera.wind_shape.y;
        let broad_phase = dot(root.xz, wind_direction) * frequency
            - camera.wind.w * wind_speed;
        let cross_phase = dot(root.xz, cross_direction) * frequency * 0.71
            + camera.wind.w * wind_speed * 0.37;
        let broad_wave = sin(broad_phase + sin(cross_phase) * 0.85);
        let gust_wave = sin(broad_phase * 0.43 - cross_phase * 0.61);
        let broad_amount = broad_wave * 0.5 + 0.5;
        let gust_coordinate = clamp(gust_wave * 0.5 + 0.5, 0.0, 1.0);
        let gust_rise = clamp((gust_coordinate - 0.28) / 0.72, 0.0, 1.0);
        let gust_pulse = gust_rise * gust_rise * (3.0 - 2.0 * gust_rise);
        let wind_force = camera.wind.z * clamp(
            0.25 + broad_amount * 0.18 + gust_pulse * camera.wind_shape.z * 0.90,
            0.12,
            1.30,
        );

        let camera_distance = distance(camera.camera_position.xyz, root);
        let detail_weight = mix(
            0.28,
            1.0,
            1.0 - smoothstep(24.0, 72.0, camera_distance),
        );
        let clump_phase = clump_variant * 2.0 * PI;
        let blade_phase = random01(blade_seed ^ 0x3c6ef372u) * 2.0 * PI;
        let mixed_phase = mix(clump_phase, blade_phase, 0.72);
        let bob = sin(
            camera.wind.w * wind_speed * 2.15
                + cross_phase * 1.31
                + mixed_phase,
        );
        let flutter = sin(
            camera.wind.w * wind_speed * 4.10
                + broad_phase * 2.13
                + blade_phase,
        );
        let species_response = select(1.0, 0.72, is_broad_leaf);
        if (!is_broad_leaf) {
            // Move the complete resting cubic as one bounded shape. This preserves its curvature
            // and length instead of pulling handles apart and adding waves at every vertex.
            // Limit the downward pitch further when the resting tip is near the ground.
            let tip_clearance = max(dot(p3 - p0, surface_normal) / max(height, 1e-4) - 0.035, 0.0);
            let pitch = min(clamp(wind_force * 0.20 + bob * camera.wind.z * 0.04, -0.10, 0.18), tip_clearance);
            let yaw = clamp(flutter * camera.wind.z * camera.wind_shape.w * detail_weight * 0.25, -0.07, 0.07);
            let pitch_axis = normalize3_or(cross(surface_normal, wind_forward), blade_side);
            let pitch_rotation = vec4<f32>(pitch_axis * sin(pitch * 0.5), cos(pitch * 0.5));
            let yaw_rotation = vec4<f32>(surface_normal * sin(yaw * 0.5), cos(yaw * 0.5));
            let rotation = vec4<f32>(
                yaw_rotation.w * pitch_rotation.xyz + pitch_rotation.w * yaw_rotation.xyz
                    + cross(yaw_rotation.xyz, pitch_rotation.xyz),
                yaw_rotation.w * pitch_rotation.w - dot(yaw_rotation.xyz, pitch_rotation.xyz),
            );
            p1 = p0 + rotate_by_quaternion(p1 - p0, rotation);
            p2 = p0 + rotate_by_quaternion(p2 - p0, rotation);
            p3 = p0 + rotate_by_quaternion(p3 - p0, rotation);
            blade_side = rotate_by_quaternion(blade_side, rotation);
            // Zero amplitude skips the old per-vertex curl, bob and flutter path for ribbons.
        } else {
            let coherent_push = wind_forward * height * wind_force * species_response;
            // Large vertical bob and cross-wind flutter make the gust readable from a static camera.
            // Their tip-heavy application below keeps the lower blade anchored in the terrain.
            let bob_offset = surface_normal * height * camera.wind.z * bob * 0.072;
            let flutter_offset = blade_side
                * height
                * camera.wind.z
                * camera.wind_shape.w
                * detail_weight
                * flutter;
            p1 += coherent_push * 0.06;
            p2 += coherent_push * 0.58 + bob_offset * 0.52 + flutter_offset * 0.48;
            p3 += coherent_push + bob_offset + flutter_offset * 1.08;

            animated_wind_forward = wind_forward;
            blade_wind_phase = camera.wind.w * wind_speed * 3.25
                + broad_phase * 1.71
                + blade_phase;
            blade_wind_amplitude = height
                * camera.wind.z
                * camera.wind_shape.w
                * detail_weight
                * species_response;
        }
    }

    // A drooping blade's endpoint can be almost on the soil. Using only that endpoint
    // for its low triangle erases the canopy, even though the full arch stays above it.
    // Keep a representative canopy height in the same triangle and approach it through
    // the existing morph; no additional vertices, roots, or high-detail radius.
    if (!is_broad_leaf && !paired_main) {
        let canopy_midpoint = (p0 + 3.0 * p1 + 3.0 * p2 + p3) * 0.125;
        let canopy_lift = max(dot(canopy_midpoint - p3, surface_normal), 0.0);
        p3 += surface_normal * canopy_lift * (1.0 - geometry_morph);
    }

    let low_section_count = select(
        clamp(u32(profile.topology.y + 0.5), 1u, MAX_LOW_SECTIONS),
        select(2u, 1u, blade_index != 0u || is_broad_leaf),
        blade_count > 1u,
    );
    let budget_coverage = select(1.0, 1.0 / SPLIT_LOW_DENSITY_BUDGET_SCALE,
        blade_count > 1u && debug_config.values.y != DENSITY_MODE_FULL_REFERENCE);
    let coverage_density = min(population_density * budget_coverage, 1.0);
    var coverage_scale = 1.0;
    if (camera.projection.w > 0.5 && !is_broad_leaf) {
        let low_coverage_scale = clamp(pow(1.0 / max(coverage_density, 0.125),
            LOW_LOD_COVERAGE_WIDTH_EXPONENT), 1.0, FAR_WIDTH_MAXIMUM_SCALE);
        // Fade toward the exact low-bin coverage before switching topology. Applying this only
        // after the switch made a visible width step around the high-detail disk.
        coverage_scale = mix(low_coverage_scale * budget_coverage, 1.0, lod_morph);
    }
    var low_shoulder = vec3<f32>(0.0);
    if ((paired_main || paired_companion) && geometry_morph < 1.0) {
        let t = select(0.0, pow(0.5, max(profile.topology.w, 0.2)), paired_main);
        let u = 1.0 - t;
        low_shoulder = p0 * u * u * u + p1 * 3.0 * u * u * t
            + p2 * 3.0 * u * t * t + p3 * t * t * t;
        let tangent = normalize3_or((p1 - p0) * u * u + (p2 - p1) * 2.0 * u * t
            + (p3 - p2) * t * t, surface_normal);
        var side = normalize3_or(blade_side - tangent * dot(blade_side, tangent), blade_side);
        let normal = normalize3_or(cross(side, tangent), surface_normal);
        let to_camera = normalize3_or(camera.camera_position.xyz - low_shoulder, normal);
        let camera_side = normalize3_or(cross(tangent, to_camera), side);
        let alignment = dot(camera_side, side);
        let aligned = select(-camera_side, camera_side, alignment >= 0.0);
        let remainder = aligned - side * abs(alignment);
        let remainder_length = length(remainder);
        let opening = min(inspected_opening(profile, debug_config), remainder_length / max(abs(alignment), 1e-4));
        side = normalize3_or(side + remainder / max(remainder_length, 1e-4) * opening, side);
        var width_scale = 1.0;
        if (camera.projection.w > 0.5) {
            let camera_distance = max(distance(camera.camera_position.xyz, low_shoulder), 0.05);
            let alignment_to_view = clamp(dot(side, to_camera), -1.0, 1.0);
            let projected_width = authored_half_width * camera.projection.x
                * sqrt(max(1.0 - alignment_to_view * alignment_to_view, 0.0)) / camera_distance;
            let required = clamp(FAR_WIDTH_TARGET_HALF_PIXELS / max(projected_width, 1e-4), 1.0, FAR_WIDTH_MAXIMUM_SCALE);
            let subpixel = mix(1.0, required, smoothstep(FAR_WIDTH_FADE_START_METERS, FAR_WIDTH_FADE_END_METERS, camera_distance));
            let density_scale = clamp(pow(1.0 / max(coverage_density, 0.125), LOW_LOD_COVERAGE_WIDTH_EXPONENT), 1.0, FAR_WIDTH_MAXIMUM_SCALE);
            width_scale = max(subpixel, density_scale * budget_coverage);
        }
        // Fixed low shoulder, independent of high-LOD row or morph position.
        // Preserve the existing low silhouette: its 4/3 shoulder factor was derived from
        // the former quadratic width integral (2/3). The pointed high mesh now uses a
        // cubic width profile to recover the area removed at its root; do not inflate LOD.
        animated_wind_forward = side * authored_half_width * select(1.0, 4.0 / 3.0, paired_main) * width_scale;
    }
    if (paired_companion && !low_lod && geometry_morph < 1.0) {
        // Ribbons have no per-vertex flutter; reuse that phase word for the first high row.
        // The low triangle expands from that row while the pointed root becomes degenerate.
        let high_sections = clamp(u32(profile.topology.x + 0.5), 2u, 4u);
        blade_wind_phase = pow(paired_ribbon_linear_t(1u, high_sections, false), max(profile.topology.w, 0.2));
    }
    return PreparedBlade(
        vec4(p0, half_width), vec4(p1, authored_half_width),
        vec4(p2, blade_wind_phase), vec4(p3, blade_wind_amplitude),
        vec4(blade_side, low_shoulder.x), vec4(animated_wind_forward, low_shoulder.y),
        vec4(surface_normal, clump_variant),
        vec4(f32(section_count), select(f32(low_section_count), -f32(low_section_count), paired_main || paired_companion), coverage_scale, low_shoulder.z),
    );
}
