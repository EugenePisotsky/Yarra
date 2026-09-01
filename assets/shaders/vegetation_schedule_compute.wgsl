// Vegetation V2 view scheduler.
//
// Resident page fields remain persistent source data. This pass compacts only the fields that can
// affect the current view and writes the indirect candidate-dispatch dimensions. Placement work
// therefore scales with visible streamed coverage rather than every resident field.

struct WorkItem {
    page: vec4<f32>,
    domain: vec4<i32>,
    candidate_layout: vec4<u32>,
    population: vec4<u32>,
    growth: vec4<f32>,
    direction_weights: vec4<f32>,
    flow_density: vec4<f32>,
    grouping: vec4<f32>,
    group_density: vec4<f32>,
    orientation: vec4<f32>,
    peers: vec4<u32>,
    // xy: surface offset/resolution, zw: minimum/maximum page height as f32 bits
    surface: vec4<u32>,
    // x: maximum height, y: maximum horizontal reach, z: minimum high-LOD threshold,
    // w: maximum low-LOD density
    bounds: vec4<f32>,
}

struct Camera {
    clip_from_world: mat4x4<f32>,
    // xyz: world position, w: viewport height
    camera_position: vec4<f32>,
    // x: vertical focal length in pixels, y: viewport width, z: viewport height
    projection: vec4<f32>,
    // Draw-only environment values keep one camera layout across all vegetation passes.
    sun_direction: vec4<f32>,
    sun_radiance: vec4<f32>,
    ambient_radiance: vec4<f32>,
    lighting: vec4<f32>,
}

struct DispatchIndirectArgs {
    workgroup_count_x: atomic<u32>,
    workgroup_count_y: atomic<u32>,
    workgroup_count_z: atomic<u32>,
}

struct Telemetry {
    values: array<atomic<u32>, 16>,
}

struct DebugConfig {
    // x: diagnostic mode, y: density mode, z: lighting mode, w: reserved
    values: vec4<u32>,
}

@group(0) @binding(0) var<storage, read> work_items: array<WorkItem>;
@group(0) @binding(1) var<storage, read_write> visible_work_items: array<u32>;
@group(0) @binding(2) var<storage, read_write> candidate_dispatch: DispatchIndirectArgs;
@group(0) @binding(3) var<uniform> camera: Camera;
@group(0) @binding(4) var<storage, read_write> telemetry: Telemetry;
@group(0) @binding(5) var<uniform> debug_config: DebugConfig;

const WORKGROUP_SIZE: u32 = 64u;
const MAX_PROCEDURAL_DISTANCE: f32 = 96.0;
const QUARTER_LOD_FLAG: u32 = 0x80000000u;

fn floor_div_two(value: i32) -> i32 {
    if (value < 0 && (value & 1) != 0) {
        return (value - 1) / 2;
    }
    return value / 2;
}

fn quarter_candidate_count(item: WorkItem) -> u32 {
    if (item.peers.z != 0u) {
        return item.candidate_layout.y / 4u;
    }
    let minimum = item.domain.xy;
    let maximum = minimum + item.domain.zw;
    let block_minimum = vec2<i32>(floor_div_two(minimum.x), floor_div_two(minimum.y));
    let block_maximum = vec2<i32>(
        floor_div_two(maximum.x - 1) + 1,
        floor_div_two(maximum.y - 1) + 1,
    );
    let block_count = vec2<u32>(block_maximum - block_minimum);
    return block_count.x * block_count.y;
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

fn maximum_projected_extent(item: WorkItem) -> f32 {
    let minimum_height = bitcast<f32>(item.surface.z);
    let maximum_height = bitcast<f32>(item.surface.w);
    let blade_height = item.bounds.x;
    let blade_reach = item.bounds.y;
    var maximum_pixels = 0.0;
    for (var elevation = 0u; elevation < 2u; elevation += 1u) {
        let root_height = select(minimum_height, maximum_height, elevation != 0u);
        for (var corner = 0u; corner < 4u; corner += 1u) {
            let x = select(item.page.x, item.page.x + item.page.z, (corner & 1u) != 0u);
            let z = select(item.page.y, item.page.y + item.page.z, (corner & 2u) != 0u);
            let root = vec3<f32>(x, root_height, z);
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root + vec3<f32>(0.0, blade_height, 0.0)),
            );
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root + vec3<f32>(blade_reach, 0.0, 0.0)),
            );
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root - vec3<f32>(blade_reach, 0.0, 0.0)),
            );
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root + vec3<f32>(0.0, 0.0, blade_reach)),
            );
            maximum_pixels = max(
                maximum_pixels,
                projected_distance_pixels(root, root - vec3<f32>(0.0, 0.0, blade_reach)),
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
        debug_config.values.x != 0u
        || debug_config.values.y != 0u
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
    let half_size = item.page.z * 0.5;
    let center_xz = item.page.xy + vec2<f32>(half_size);
    let minimum_height = bitcast<f32>(item.surface.z);
    let maximum_height = bitcast<f32>(item.surface.w);
    let horizontal_radius = half_size * 1.41421356 + item.bounds.y;

    let camera_delta = center_xz - camera.camera_position.xz;
    let horizontal_distance = length(camera_delta);
    if (horizontal_distance > MAX_PROCEDURAL_DISTANCE + horizontal_radius) {
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
        item.page.x - item.bounds.y,
        minimum_height - item.bounds.y - 0.25,
        item.page.y - item.bounds.y,
    );
    let maximum = vec3<f32>(
        item.page.x + item.page.z + item.bounds.y,
        maximum_height + item.bounds.x + item.bounds.y,
        item.page.y + item.page.z + item.bounds.y,
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

@compute @workgroup_size(64, 1, 1)
fn schedule(@builtin(global_invocation_id) invocation: vec3<u32>) {
    if (invocation.x == 0u) {
        atomicStore(&candidate_dispatch.workgroup_count_z, 1u);
    }
    if (invocation.x >= arrayLength(&work_items)) {
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
    atomicAdd(&telemetry.values[0], 1u);
    let workgroup_count = (candidate_count + WORKGROUP_SIZE - 1u) / WORKGROUP_SIZE;
    atomicMax(
        &candidate_dispatch.workgroup_count_x,
        workgroup_count,
    );
    atomicMax(&telemetry.values[10], workgroup_count);
}
