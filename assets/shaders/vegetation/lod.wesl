// View-dependent detail shared by scheduling, placement and blade preparation: the gameplay
// detail focus, projected sizes and the quarter candidate lattice.
import package::shaders::vegetation::types::{Camera, WorkItem};
import package::shaders::vegetation::random::hash_cell;

// Bounds wind displacement per metre of blade height at unit wind strength, so visibility and
// projected-size bounds hold for every pose.
const WIND_REACH: f32 = 1.65;

// Equal-area ellipse: spend the same high-topology budget along the view direction.
// The gameplay centre follows the subject; clients without a focus retain the camera disk.
fn detail_distance(root: vec2<f32>, camera: Camera) -> f32 {
    let delta = root - camera.lod_focus.xy;
    let forward = camera.lod_focus.zw;
    if (dot(forward, forward) < 0.5) { return length(root - camera.camera_position.xz); }
    let along = dot(delta, forward) / 1.5;
    let across = dot(delta, vec2(-forward.y, forward.x)) * 1.5;
    return length(vec2(along, across));
}

fn near_field_coverage(root: vec2<f32>, camera: Camera) -> f32 {
    if (dot(camera.lod_focus.zw, camera.lod_focus.zw) < 0.5) { return 0.0; }
    return 1.0 - smoothstep(12.0, 26.0, detail_distance(root, camera));
}

fn projected_distance_pixels(origin: vec3<f32>, endpoint: vec3<f32>, camera: Camera) -> f32 {
    let origin_clip = camera.clip_from_world * vec4<f32>(origin, 1.0);
    let endpoint_clip = camera.clip_from_world * vec4<f32>(endpoint, 1.0);
    if (origin_clip.w <= 1e-5 || endpoint_clip.w <= 1e-5) {
        // Near-plane intersections are not safe to reduce or cull from projected size.
        return 1e30;
    }
    return length(
        endpoint_clip.xy / endpoint_clip.w - origin_clip.xy / origin_clip.w,
    ) * camera.camera_position.w * 0.5;
}

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

fn remap_quarter_candidate(item: WorkItem, reduced_index: u32) -> u32 {
    if (item.peers.z != 0u) {
        let groups_per_cell = item.candidate_layout.x / 4u;
        let cell_index = reduced_index / groups_per_cell;
        let group = reduced_index % groups_per_cell;
        let cell = vec2<i32>(
            item.domain.x + i32(cell_index % u32(item.domain.z)),
            item.domain.y + i32(cell_index / u32(item.domain.z)),
        );
        let rotation = hash_cell(
            item.population.z,
            cell,
            0xa24baed5u ^ (group * 0x9e3779b9u),
        ) & 3u;
        let selected_child = group * 4u + ((4u - rotation) & 3u);
        return cell_index * item.candidate_layout.x + selected_child;
    }

    let minimum = item.domain.xy;
    let maximum = minimum + item.domain.zw;
    let block_minimum = vec2<i32>(floor_div_two(minimum.x), floor_div_two(minimum.y));
    let block_maximum = vec2<i32>(
        floor_div_two(maximum.x - 1) + 1,
        floor_div_two(maximum.y - 1) + 1,
    );
    let block_count_x = u32(block_maximum.x - block_minimum.x);
    let block = block_minimum + vec2<i32>(
        i32(reduced_index % block_count_x),
        i32(reduced_index / block_count_x),
    );
    let rotation = hash_cell(item.population.z, block, 0xa24baed5u) & 3u;
    let selected_quadrant = (4u - rotation) & 3u;
    let cell = block * 2 + vec2<i32>(
        i32(selected_quadrant & 1u),
        i32(selected_quadrant >> 1u),
    );
    if (any(cell < minimum) || any(cell >= maximum)) {
        return 0xffffffffu;
    }
    let local = cell - minimum;
    return (u32(local.y) * u32(item.domain.z) + u32(local.x)) * item.candidate_layout.x;
}
