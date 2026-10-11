// View-dependent detail shared by scheduling, placement and drawing: the gameplay detail focus
// and projected sizes.
import package::shaders::vegetation::types::Camera;

// Bounds wind displacement per metre of blade height at unit wind strength, so visibility and
// projected-size bounds hold for every pose.
const WIND_REACH: f32 = 1.65;

// Equal-area ellipse: spend the same high-detail budget along the view direction.
// The gameplay centre follows the subject; clients without a focus retain the camera disk.
fn detail_distance(root: vec2<f32>, camera: Camera) -> f32 {
    let delta = root - camera.lod_focus.xy;
    let forward = camera.lod_focus.zw;
    if (dot(forward, forward) < 0.5) { return length(root - camera.camera_position.xz); }
    let along = dot(delta, forward) / 1.5;
    let across = dot(delta, vec2(-forward.y, forward.x)) * 1.5;
    return length(vec2(along, across));
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
