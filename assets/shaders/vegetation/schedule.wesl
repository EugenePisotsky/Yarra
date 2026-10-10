// Visible work scheduling.
//
// Resident page fields remain persistent source data. This pass compacts only the fields that can
// affect the current view and writes the indirect candidate-dispatch dimensions. Placement work
// therefore scales with visible streamed coverage rather than every resident field.
import package::shaders::vegetation::types::{
    WorkItem, Camera, Config, Telemetry, QUARTER_LOD_FLAG, MAX_PROCEDURAL_DISTANCE,
};
import package::shaders::vegetation::lod::{
    WIND_REACH, projected_distance_pixels, quarter_candidate_count,
};

struct DispatchIndirectArgs {
    workgroup_count_x: atomic<u32>,
    workgroup_count_y: atomic<u32>,
    workgroup_count_z: atomic<u32>,
}

@group(0) @binding(0) var<storage, read> work_items: array<WorkItem>;
@group(0) @binding(1) var<storage, read_write> visible_work_items: array<u32>;
@group(0) @binding(2) var<storage, read_write> candidate_dispatch: DispatchIndirectArgs;
@group(0) @binding(3) var<uniform> camera: Camera;
@group(0) @binding(4) var<storage, read_write> telemetry: Telemetry;
@group(0) @binding(5) var<uniform> config: Config;

const WORKGROUP_SIZE: u32 = constants::WORKGROUP_SIZE;

fn maximum_projected_extent(item: WorkItem) -> f32 {
    let minimum_height = bitcast<f32>(item.surface.z);
    let maximum_height = bitcast<f32>(item.surface.w);
    let blade_height = item.bounds.x;
    let blade_reach = item.bounds.y + blade_height * camera.wind.z * WIND_REACH;
    var maximum_pixels = 0.0;
    for (var elevation = 0u; elevation < 2u; elevation += 1u) {
        let root_height = select(minimum_height, maximum_height, elevation != 0u);
        for (var corner = 0u; corner < 4u; corner += 1u) {
            let x = select(item.page.x, item.page.x + item.page.z, (corner & 1u) != 0u);
            let z = select(item.page.y, item.page.y + item.page.z, (corner & 2u) != 0u);
            let root = vec3<f32>(x, root_height, z);
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root + vec3<f32>(0.0, blade_height, 0.0), camera),
            );
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root + vec3<f32>(blade_reach, 0.0, 0.0), camera),
            );
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root - vec3<f32>(blade_reach, 0.0, 0.0), camera),
            );
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root + vec3<f32>(0.0, 0.0, blade_reach), camera),
            );
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root - vec3<f32>(0.0, 0.0, blade_reach), camera),
            );
        }
    }
    return maximum_pixels;
}

fn projected_population_spacing_at(root: vec3<f32>, spacing: f32) -> f32 {
    let root_clip = camera.clip_from_world * vec4<f32>(root, 1.0);
    let x_clip = camera.clip_from_world
        * vec4<f32>(root + vec3<f32>(spacing, 0.0, 0.0), 1.0);
    let z_clip = camera.clip_from_world
        * vec4<f32>(root + vec3<f32>(0.0, 0.0, spacing), 1.0);
    if (root_clip.w <= 1e-5 || x_clip.w <= 1e-5 || z_clip.w <= 1e-5) {
        return 1e30;
    }
    let pixel_scale = camera.camera_position.w * 0.5;
    let delta_x = (x_clip.xy / x_clip.w - root_clip.xy / root_clip.w) * pixel_scale;
    let delta_z = (z_clip.xy / z_clip.w - root_clip.xy / root_clip.w) * pixel_scale;
    // Square root of projected cell area gives a linear screen-space root-spacing metric. It
    // remains large in overhead views and naturally shrinks under distance and foreshortening.
    return sqrt(abs(delta_x.x * delta_z.y - delta_x.y * delta_z.x));
}

fn maximum_projected_population_spacing(item: WorkItem) -> f32 {
    let minimum_height = bitcast<f32>(item.surface.z);
    let maximum_height = bitcast<f32>(item.surface.w);
    let spacing = inverseSqrt(max(item.flow_density.z, 1e-6));
    var maximum_pixels = 0.0;
    for (var elevation = 0u; elevation < 2u; elevation += 1u) {
        let root_height = select(minimum_height, maximum_height, elevation != 0u);
        for (var corner = 0u; corner < 4u; corner += 1u) {
            let x = select(item.page.x, item.page.x + item.page.z, (corner & 1u) != 0u);
            let z = select(item.page.y, item.page.y + item.page.z, (corner & 2u) != 0u);
            maximum_pixels = max(
                maximum_pixels,
                projected_population_spacing_at(vec3<f32>(x, root_height, z), spacing),
            );
        }
    }
    return maximum_pixels;
}

fn can_schedule_quarter_lod(item: WorkItem) -> bool {
    // The balanced and full-reference modes must schedule the complete candidate lattice; a quarter
    // dispatch cannot recover roots omitted before generation.
    if (
        config.values.x != 0u
        || config.values.y != 0u
        || item.bounds.w > 0.2501
    ) {
        return false;
    }
    if (item.peers.z != 0u && item.candidate_layout.x % 4u != 0u) {
        return false;
    }
    // Direct quarter dispatch cannot recover omitted roots. Stay comfortably inside the authored
    // low-density plateau; the full lattice handles the screen-space transition around it.
    if (maximum_projected_population_spacing(item) > item.page.w * 0.7) {
        return false;
    }
    // The margin keeps work on the full lattice throughout the topology/density transition.
    return maximum_projected_extent(item) <= item.bounds.z * 0.82;
}

fn item_is_visible(item: WorkItem) -> bool {
    // Contact readiness can change without invalidating source buffers or caches.
    if (item.flow_density.w > 0.5) { return false; }
    let half_size = item.page.z * 0.5;
    let center_xz = item.page.xy + vec2<f32>(half_size);
    let minimum_height = bitcast<f32>(item.surface.z);
    let maximum_height = bitcast<f32>(item.surface.w);
    let wind_reach = item.bounds.x * camera.wind.z * WIND_REACH;
    let blade_reach = item.bounds.y + wind_reach;
    let horizontal_radius = half_size * 1.41421356 + blade_reach;

    let camera_delta = center_xz - camera.camera_position.xz;
    let horizontal_distance = length(camera_delta);
    if (horizontal_distance > MAX_PROCEDURAL_DISTANCE + horizontal_radius) {
        return false;
    }
    // Root distance is three-dimensional, including cameras high above valleys.
    // Use the full root-height interval, not the page centre, for conservative scheduling.
    let nearest_root = clamp(camera.camera_position.xyz,
        vec3<f32>(item.page.x, minimum_height, item.page.y),
        vec3<f32>(item.page.x + item.page.z, maximum_height, item.page.y + item.page.z));
    if (distance(nearest_root, camera.camera_position.xyz) > MAX_PROCEDURAL_DISTANCE + blade_reach) {
        return false;
    }
    // A camera inside/very near the page necessarily sees some of it.
    if (horizontal_distance <= horizontal_radius + 1.0) {
        return true;
    }

    // Reject only if every corner lies outside the same homogeneous side plane. Projecting the
    // page centre and estimating an angular radius is not conservative for a large page close to
    // the camera: its centre can leave the view while a corner still occupies much of the screen.
    // The upper bound includes procedural vegetation height/bend beyond terrain relief.
    let minimum = vec3<f32>(
        item.page.x - blade_reach,
        minimum_height - blade_reach - 0.25,
        item.page.y - blade_reach,
    );
    let maximum = vec3<f32>(
        item.page.x + item.page.z + blade_reach,
        maximum_height + item.bounds.x + blade_reach,
        item.page.y + item.page.z + blade_reach,
    );
    var outside_left = true;
    var outside_right = true;
    var outside_bottom = true;
    var outside_top = true;
    for (var corner_index = 0u; corner_index < 8u; corner_index += 1u) {
        let corner = vec3<f32>(
            select(minimum.x, maximum.x, (corner_index & 1u) != 0u),
            select(minimum.y, maximum.y, (corner_index & 2u) != 0u),
            select(minimum.z, maximum.z, (corner_index & 4u) != 0u),
        );
        let clip = camera.clip_from_world * vec4<f32>(corner, 1.0);
        outside_left = outside_left && clip.x < -clip.w;
        outside_right = outside_right && clip.x > clip.w;
        outside_bottom = outside_bottom && clip.y < -clip.w;
        outside_top = outside_top && clip.y > clip.w;
    }
    return !(outside_left || outside_right || outside_bottom || outside_top);
}

@compute @workgroup_size(WORKGROUP_SIZE, 1, 1)
fn schedule(@builtin(global_invocation_id) invocation: vec3<u32>) {
    if (invocation.x == 0u) {
        atomicStore(&candidate_dispatch.workgroup_count_z, 1u);
    }
    // The grow-only buffer may contain retired page records after a scene shrinks.
    if (invocation.x >= config.workload.x) {
        return;
    }

    let item = work_items[invocation.x];
    if (!item_is_visible(item)) {
        return;
    }
    let quarter_lod = can_schedule_quarter_lod(item);
    let candidate_count = select(
        item.candidate_layout.y,
        quarter_candidate_count(item),
        quarter_lod,
    );
    let visible_slot = atomicAdd(&candidate_dispatch.workgroup_count_y, 1u);
    visible_work_items[visible_slot] = invocation.x | select(0u, QUARTER_LOD_FLAG, quarter_lod);
    if (config.workload.y != 0u) {
        atomicAdd(&telemetry.values[0], 1u);
    }
    let workgroup_count = (candidate_count + WORKGROUP_SIZE - 1u) / WORKGROUP_SIZE;
    atomicMax(
        &candidate_dispatch.workgroup_count_x,
        workgroup_count,
    );
    if (config.workload.y != 0u) {
        atomicMax(&telemetry.values[10], workgroup_count);
    }
}
