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
const SPECIES_INDEX_MASK: u32 = 0x0000ffffu;
const LOD_MORPH_MASK: u32 = 0x00007fffu;
const LOD_MORPH_SHIFT: u32 = 16u;
const BALANCED_DENSITY_FADE_BAND: f32 = 0.10;
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

struct PreparedBlade {
    p0_width: vec4<f32>,
    p1_authored_width: vec4<f32>,
    p2_phase: vec4<f32>,
    p3_amplitude: vec4<f32>,
    side: vec4<f32>,
    wind_forward: vec4<f32>,
    surface_clump: vec4<f32>,
    // x: section count, y: low section count, z: low LOD coverage scale
    topology: vec4<f32>,
}

struct PreparedArena {
    // Zero means fallback to the original calculation; otherwise first blade index + 1.
    indices: array<u32, 344064>,
    blades: array<PreparedBlade>,
}

fn prepare_blade(
    instance: ProceduralInstance, profile: Species, camera: Camera,
    debug_config: DebugConfig, blade_index: u32,
) -> PreparedBlade {
    let low_lod = (instance.geometry.y >> 31u) != 0u;
    let lod_morph = f32((instance.geometry.y >> LOD_MORPH_SHIFT) & LOD_MORPH_MASK)
        / f32(LOD_MORPH_MASK);
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
    let height = mix(profile.bounds.x, profile.bounds.y, height_coordinate);
    let is_broad_leaf = profile.root_color.w >= 1.5;
    var blade_count = clamp(u32(profile.topology.z + 0.5), 1u, 2u);
    if (!is_broad_leaf && profile.height_packing.y > 0.0) {
        blade_count = select(1u, 2u, height <= profile.height_packing.y);
    }
    // Generation already classified this blade using its complete projected envelope. Reuse that
    // per-instance result rather than repeating five clip-space projections for every vertex.

    var forward = vec3<f32>(rest_direction.x, 0.0, rest_direction.y);
    forward -= surface_normal * dot(forward, surface_normal);
    forward = normalize3_or(forward, vec3<f32>(1.0, 0.0, 0.0));
    let base_side = normalize3_or(cross(surface_normal, forward), vec3<f32>(0.0, 0.0, 1.0));

    // Static u16 indices encode side in bit 0, row in bits 1..4, and blade in bit 5. A paired
    // render unit divides the single-blade budget: 4+3 high sections or two low triangles.
    let authored_section_count = select(profile.topology.x, profile.topology.y, low_lod);
    var maximum_sections = select(MAX_SECTIONS, MAX_LOW_SECTIONS, low_lod);
    if (blade_count > 1u) {
        maximum_sections = select(select(4u, 3u, blade_index != 0u), 1u, low_lod);
    }
    let section_count = clamp(u32(authored_section_count + 0.5), 1u, maximum_sections);

    let blade_seed = hash32(seed ^ blade_index * 0x9e3779b9u);
    let paired_side = select(-1.0, 1.0, blade_index != 0u);
    let facing_jitter = (random01(blade_seed ^ 0x68e31da4u) - 0.5)
        * select(0.7, 0.34, is_broad_leaf);
    let spread_angle = paired_side * profile.shape_secondary.y * 0.5 + facing_jitter;
    let blade_forward = normalize3_or(
        forward * cos(spread_angle) + base_side * sin(spread_angle),
        forward,
    );
    let blade_side = normalize3_or(cross(surface_normal, blade_forward), base_side);

    // Density LOD is a coverage transition, not a growth animation. Keeping the complete
    // centreline prevents a rejected subset from visibly rising out of the ground. High-only
    // candidates contract into the low geometry boundary; balanced mode additionally keeps a narrow
    // stable-rank band of low blades and contracts their width before compute stops emitting them.
    let high_transition_width = select(
        lod_morph,
        1.0,
        lod_rank < population_density || low_lod,
    );
    var balanced_low_width = 1.0;
    if (
        debug_config.values.y == DENSITY_MODE_BALANCED
        && population_density < 0.999
    ) {
        let half_band = BALANCED_DENSITY_FADE_BAND * 0.5;
        balanced_low_width = 1.0 - smoothstep(
            max(population_density - half_band, 0.0),
            min(population_density + half_band, 1.0),
            lod_rank,
        );
    }
    let density_width = select(high_transition_width, balanced_low_width, low_lod);
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

    let low_section_count = select(
        clamp(u32(profile.topology.y + 0.5), 1u, MAX_LOW_SECTIONS),
        1u,
        blade_count > 1u,
    );
    var coverage_scale = 1.0;
    if (camera.projection.w > 0.5 && !is_broad_leaf && low_lod) {
        coverage_scale = clamp(pow(1.0 / max(population_density, 0.125),
            LOW_LOD_COVERAGE_WIDTH_EXPONENT), 1.0, FAR_WIDTH_MAXIMUM_SCALE);
    }
    return PreparedBlade(
        vec4(p0, half_width), vec4(p1, authored_half_width),
        vec4(p2, blade_wind_phase), vec4(p3, blade_wind_amplitude),
        vec4(blade_side, 0.0), vec4(animated_wind_forward, 0.0),
        vec4(surface_normal, clump_variant),
        vec4(f32(section_count), f32(low_section_count), coverage_scale, 0.0),
    );
}
