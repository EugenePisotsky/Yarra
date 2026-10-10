// Visibility, topology and population LOD of one candidate, from its projected size and spacing.
import package::shaders::vegetation::types::{
    WorkItem, DENSITY_MODE_BALANCED, MAX_PROCEDURAL_DISTANCE, BALANCED_DENSITY_FADE_BAND,
};
import package::shaders::vegetation::random::{hash32, random01};
import package::shaders::vegetation::lod::{WIND_REACH, detail_distance, projected_distance_pixels};
import package::shaders::vegetation::placement::bindings::{SpeciesChoice, camera, config};
import package::shaders::vegetation::placement::sampling::{Candidate, SurfaceResult};

// Geometry quality control, separate from the near-field density footprint.
// 1.0 restores the previous range; 0.65 switches to the low mesh 35% sooner.
const HIGH_TOPOLOGY_DISTANCE_SCALE: f32 = 0.65;
const BALANCED_DENSITY_FULL_SPACING_PIXELS: f32 = 6.0;
const BALANCED_DENSITY_MIDDLE_SPACING_PIXELS: f32 = 2.0;
const BALANCED_DENSITY_FAR_SPACING_PIXELS: f32 = 0.75;
const BALANCED_DENSITY_MIDDLE_FRACTION: f32 = 0.55;
const BALANCED_DENSITY_FAR_FRACTION: f32 = 0.30;

fn candidate_is_visible(
    candidate: Candidate,
    surface: SurfaceResult,
    choice: SpeciesChoice,
) -> bool {
    let maximum_reach = bitcast<f32>(choice.metadata.z)
        + bitcast<f32>(choice.metadata.w) * camera.wind.z * WIND_REACH;
    let maximum_height = bitcast<f32>(choice.metadata.w);
    let root = vec3<f32>(candidate.root.x, surface.height, candidate.root.y);
    let distance = length(root - camera.camera_position.xyz);
    if (distance > MAX_PROCEDURAL_DISTANCE + maximum_reach) {
        return false;
    }

    let center = vec3<f32>(
        candidate.root.x,
        surface.height + maximum_height * 0.45,
        candidate.root.y,
    );
    let clip = camera.clip_from_world * vec4<f32>(center, 1.0);
    if (clip.w <= 1e-5) {
        return false;
    }
    let ndc = clip.xy / clip.w;
    let extent = max(maximum_reach, maximum_height * 0.55);
    let margin = min(0.4, 0.035 + extent / max(distance, 1.0) * 1.8);
    return abs(ndc.x) <= 1.0 + margin && abs(ndc.y) <= 1.0 + margin;
}

fn projected_blade_extent_pixels(
    candidate: Candidate,
    surface: SurfaceResult,
    choice: SpeciesChoice,
) -> f32 {
    let maximum_height = bitcast<f32>(choice.metadata.w);
    let maximum_reach = bitcast<f32>(choice.metadata.z)
        + maximum_height * camera.wind.z * WIND_REACH;
    let root = vec3<f32>(candidate.root.x, surface.height, candidate.root.y);
    let tip = root + surface.normal * maximum_height;
    var maximum_pixels = projected_distance_pixels(root, tip, camera);
    // A blade seen from overhead can project its surface-normal height to nearly zero while its
    // authored bend/tilt still covers a large part of the screen. Bound both horizontal axes so
    // classification is conservative regardless of the candidate's random facing.
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root + vec3<f32>(maximum_reach, 0.0, 0.0), camera),
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root - vec3<f32>(maximum_reach, 0.0, 0.0), camera),
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root + vec3<f32>(0.0, 0.0, maximum_reach), camera),
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root - vec3<f32>(0.0, 0.0, maximum_reach), camera),
    );
    return maximum_pixels;
}

fn blade_extent_limits_pixels(
    candidate: Candidate,
    surface: SurfaceResult,
    choice: SpeciesChoice,
    high_radius: f32,
) -> vec2<f32> {
    let projected_extent = projected_blade_extent_pixels(candidate, surface, choice);
    let high_threshold = choice.threshold.y;
    let bounded_high_radius = max(high_radius * HIGH_TOPOLOGY_DISTANCE_SCALE, 1e-3);
    // Stable per-root staggering softens the topology transition. The focus ellipse
    // preserves the disk area used by the CPU budget (0.5 * 1.3² = 84.5% maximum).
    let staggered_high_radius = bounded_high_radius * mix(
        1.10, 1.30, random01(candidate.seed ^ 0x6a09e667u),
    );
    let distance = detail_distance(candidate.root, camera);
    let high_weight = 1.0 - smoothstep(
        staggered_high_radius * 0.90,
        staggered_high_radius,
        distance,
    );
    // High topology is admitted through a stable world-space footprint sized from the authored root
    // density and the device-profile bin capacity. The annulus uses the existing high-to-low
    // geometry morph. It never relies on atomic append order to decide which roots survive.
    let budget_extent = high_threshold * mix(0.999, 1.45, high_weight);
    return vec2(projected_extent, budget_extent);
}

fn generated_candidate_height(choice: SpeciesChoice, candidate: Candidate) -> f32 {
    let group_key = u32(round(candidate.clump_variant * 65535.0));
    // Procedural instances retain the low 24 seed bits; classify from that exact persisted value so
    // the draw shader reconstructs the same height and therefore the same topology class.
    let unit_seed = hash32(candidate.seed & 0x00ffffffu);
    let source_coordinate = mix(
        random01(unit_seed ^ 0xa511e9b3u),
        random01(group_key ^ 0x52dce729u),
        choice.height.w,
    );
    let height_exponent = exp2(-2.0 * choice.height.z);
    let height_coordinate = pow(clamp(source_coordinate, 0.0, 1.0), height_exponent);
    return mix(choice.height.x, choice.height.y, height_coordinate);
}

fn candidate_topology_class(
    choice: SpeciesChoice,
    generated_height: f32,
) -> u32 {
    if (choice.packing.x > 0.0) {
        return select(0u, 1u, generated_height <= choice.packing.x);
    }
    return min(choice.metadata.y, 1u);
}

fn projected_population_spacing_pixels(
    item: WorkItem,
    candidate: Candidate,
    surface: SurfaceResult,
) -> f32 {
    let spacing = inverseSqrt(max(item.flow_density.z, 1e-6));
    let root = vec3<f32>(candidate.root.x, surface.height, candidate.root.y);
    let to_camera = camera.camera_position.xyz - root;
    let distance = max(length(to_camera), 1e-3);
    let foreshortening = sqrt(clamp(abs(dot(surface.normal, to_camera / distance)), 0.0, 1.0));
    // This is the linearized projected area of one density cell. Unlike blade height it remains
    // large when the camera looks down on visibly separated roots.
    return spacing * camera.projection.x * foreshortening / distance;
}

fn population_lod_density(choice: SpeciesChoice, projected_spacing: f32) -> f32 {
    // Geometry complexity follows the blade envelope, while population follows projected root-cell
    // spacing. Stable rank turns this continuous target into a nested spatial fade.
    let low_boundary = choice.threshold.z;
    let far_boundary = choice.threshold.w;
    let low_density = choice.density.y;
    let far_density = choice.density.z;
    let full_boundary = max(low_boundary * 2.0, low_boundary + 1e-4);
    if (projected_spacing >= full_boundary) {
        return 1.0;
    }
    if (projected_spacing >= low_boundary) {
        return mix(
            low_density,
            1.0,
            smoothstep(low_boundary, full_boundary, projected_spacing),
        );
    }
    if (projected_spacing <= far_boundary) {
        return far_density;
    }
    return mix(
        far_density,
        low_density,
        smoothstep(far_boundary, low_boundary, projected_spacing),
    );
}

fn balanced_population_lod_density(projected_spacing: f32) -> f32 {
    // Preserve separately visible roots, then spend density only as their projected cells become
    // difficult to resolve. The final 30% plateau is an interim ribbon-only horizon policy; a
    // future coverage representation can eventually replace it below this range.
    if (projected_spacing >= BALANCED_DENSITY_FULL_SPACING_PIXELS) {
        return 1.0;
    }
    if (projected_spacing >= BALANCED_DENSITY_MIDDLE_SPACING_PIXELS) {
        return mix(
            BALANCED_DENSITY_MIDDLE_FRACTION,
            1.0,
            smoothstep(
                BALANCED_DENSITY_MIDDLE_SPACING_PIXELS,
                BALANCED_DENSITY_FULL_SPACING_PIXELS,
                projected_spacing,
            ),
        );
    }
    if (projected_spacing <= BALANCED_DENSITY_FAR_SPACING_PIXELS) {
        return BALANCED_DENSITY_FAR_FRACTION;
    }
    return mix(
        BALANCED_DENSITY_FAR_FRACTION,
        BALANCED_DENSITY_MIDDLE_FRACTION,
        smoothstep(
            BALANCED_DENSITY_FAR_SPACING_PIXELS,
            BALANCED_DENSITY_MIDDLE_SPACING_PIXELS,
            projected_spacing,
        ),
    );
}

fn population_lod_retention_limit(population_density: f32, topology_class: u32) -> f32 {
    // Balanced mode keeps a narrow, stable rank band alive so the draw shader can contract retiring
    // blades laterally. Centering that band on the target approximately preserves integrated width.
    if (
        config.values.y == DENSITY_MODE_BALANCED
        && population_density < 0.999
    ) {
        return min(population_density + BALANCED_DENSITY_FADE_BAND * 0.5, 1.0);
    }
    return population_density;
}
