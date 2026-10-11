// Visible work scheduling.
//
// Resident page fields remain persistent source data. This pass compacts only the fields that can
// affect the current view and writes the indirect candidate-dispatch dimensions. Placement work
// therefore scales with visible streamed coverage rather than every resident field.
import package::shaders::vegetation::types::{
    WorkItem, GrassType, Camera, Config, Telemetry, MAX_PROCEDURAL_DISTANCE,
};
import package::shaders::vegetation::lod::WIND_REACH;

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
@group(0) @binding(6) var<storage, read> types: array<GrassType>;

const WORKGROUP_SIZE: u32 = constants::WORKGROUP_SIZE;

fn item_is_visible(item: WorkItem) -> bool {
    // Contact readiness can change without invalidating source buffers or caches.
    if (item.page.w > 0.5) { return false; }
    let grass = types[item.field.w];
    let half_size = item.page.z * 0.5;
    let center_xz = item.page.xy + vec2<f32>(half_size);
    let minimum_height = bitcast<f32>(item.surface.z);
    let maximum_height = bitcast<f32>(item.surface.w);
    let blade_height = grass.size.y;
    let blade_reach = grass.placement.w + blade_height * camera.wind.z * WIND_REACH;
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
    let minimum = vec3<f32>(
        item.page.x - blade_reach,
        minimum_height - blade_reach - 0.25,
        item.page.y - blade_reach,
    );
    let maximum = vec3<f32>(
        item.page.x + item.page.z + blade_reach,
        maximum_height + blade_height + blade_reach,
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
    let visible_slot = atomicAdd(&candidate_dispatch.workgroup_count_y, 1u);
    visible_work_items[visible_slot] = invocation.x;
    if (config.workload.y != 0u) {
        atomicAdd(&telemetry.values[0], 1u);
    }
    let workgroup_count = (item.field.x + WORKGROUP_SIZE - 1u) / WORKGROUP_SIZE;
    atomicMax(&candidate_dispatch.workgroup_count_x, workgroup_count);
    if (config.workload.y != 0u) {
        atomicMax(&telemetry.values[10], workgroup_count);
    }
}
