// Visibility, detail and population LOD of one candidate, from its projected size and spacing.
import package::shaders::vegetation::types::{GrassType, MAX_PROCEDURAL_DISTANCE};
import package::shaders::vegetation::random::random01;
import package::shaders::vegetation::lod::{WIND_REACH, detail_distance, projected_distance_pixels};
import package::shaders::vegetation::placement::bindings::camera;
import package::shaders::vegetation::placement::sampling::{Candidate, SurfaceResult};

// Blades smaller than this on screen are not drawn.
const MINIMUM_EXTENT_PIXELS: f32 = 0.75;
// Population thinning by projected root spacing: every root while they stand this many pixels
// apart, then fewer as they crowd together, to the far share.
const FULL_SPACING_PIXELS: f32 = 6.0;
const MIDDLE_SPACING_PIXELS: f32 = 2.0;
const FAR_SPACING_PIXELS: f32 = 0.75;
const MIDDLE_FRACTION: f32 = 0.55;
const FAR_FRACTION: f32 = 0.30;
// The high-detail disk's edge, where blades morph into the low-detail mesh, as shares of its
// radius. Each root takes a stable radius in the band so the edge never forms a line.
const HIGH_DETAIL_STAGGER: vec2<f32> = vec2(0.85, 1.0);
const MORPH_BAND: f32 = 0.18;

fn candidate_is_visible(candidate: Candidate, surface: SurfaceResult, grass: GrassType) -> bool {
    let maximum_height = grass.size.y;
    let maximum_reach = grass.placement.w + maximum_height * camera.wind.z * WIND_REACH;
    let root = vec3<f32>(candidate.root.x, surface.height, candidate.root.y);
    let distance = length(root - camera.camera_position.xyz);
    if (distance > MAX_PROCEDURAL_DISTANCE + maximum_reach) {
        return false;
    }
    let center = vec3<f32>(candidate.root.x, surface.height + maximum_height * 0.45, candidate.root.y);
    let clip = camera.clip_from_world * vec4<f32>(center, 1.0);
    if (clip.w <= 1e-5) {
        return false;
    }
    let ndc = clip.xy / clip.w;
    let extent = max(maximum_reach, maximum_height * 0.55);
    let margin = min(0.4, 0.035 + extent / max(distance, 1.0) * 1.8);
    return abs(ndc.x) <= 1.0 + margin && abs(ndc.y) <= 1.0 + margin;
}

fn projected_blade_extent_pixels(candidate: Candidate, surface: SurfaceResult, grass: GrassType) -> f32 {
    let maximum_height = grass.size.y;
    let maximum_reach = grass.placement.w + maximum_height * camera.wind.z * WIND_REACH;
    let root = vec3<f32>(candidate.root.x, surface.height, candidate.root.y);
    var pixels = projected_distance_pixels(root, root + surface.normal * maximum_height, camera);
    // A blade seen from overhead can project its height to nearly zero while its tilt still
    // covers part of the screen. Bound both horizontal axes.
    pixels = max(pixels, projected_distance_pixels(root, root + vec3(maximum_reach, 0.0, 0.0), camera));
    pixels = max(pixels, projected_distance_pixels(root, root + vec3(0.0, 0.0, maximum_reach), camera));
    return pixels;
}

fn projected_spacing_pixels(grass: GrassType, candidate: Candidate, surface: SurfaceResult) -> f32 {
    let root = vec3<f32>(candidate.root.x, surface.height, candidate.root.y);
    let to_camera = camera.camera_position.xyz - root;
    let distance = max(length(to_camera), 1e-3);
    let foreshortening = sqrt(clamp(abs(dot(surface.normal, to_camera / distance)), 0.0, 1.0));
    // The linearized projected size of one lattice cell. Unlike blade height it remains
    // large when the camera looks down on visibly separated roots.
    return grass.placement.y * camera.projection.x * foreshortening / distance;
}

// Preserve separately visible roots, then thin the population as their projected cells become
// difficult to resolve. Survivors widen to keep the field's coverage.
fn population_density(projected_spacing: f32) -> f32 {
    if (projected_spacing >= FULL_SPACING_PIXELS) {
        return 1.0;
    }
    if (projected_spacing >= MIDDLE_SPACING_PIXELS) {
        return mix(MIDDLE_FRACTION, 1.0,
            smoothstep(MIDDLE_SPACING_PIXELS, FULL_SPACING_PIXELS, projected_spacing));
    }
    if (projected_spacing <= FAR_SPACING_PIXELS) {
        return FAR_FRACTION;
    }
    return mix(FAR_FRACTION, MIDDLE_FRACTION,
        smoothstep(FAR_SPACING_PIXELS, MIDDLE_SPACING_PIXELS, projected_spacing));
}

// 1 well inside the high-detail disk, falling to 0 across its staggered edge. Blades with weight
// 0 take the low-detail mesh; the others take the high one, morphing toward the low by it.
fn detail_weight(candidate: Candidate) -> f32 {
    let radius = camera.projection.w
        * mix(HIGH_DETAIL_STAGGER.x, HIGH_DETAIL_STAGGER.y, random01(candidate.seed ^ 0x6a09e667u));
    let distance = detail_distance(candidate.root, camera);
    return 1.0 - smoothstep(radius * (1.0 - MORPH_BAND), radius, distance);
}
