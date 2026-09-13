// Perspective-correct screen traversal, following the DDA approach described by
// McGuire & Mara, JCGT 3(4), 2014: https://jcgt.org/published/0003/04/04/
// This implementation uses reciprocal clip W and ray distance/W. It is independent
// of receiver IDs and the triangle reference. One texture read per visited pixel.
struct ContactTrace {
    visibility: f32,
    reach: f32,
    reads: u32,
}

fn ray_parameter(s: f32, inverse_w: vec2<f32>, t_over_w: vec2<f32>) -> f32 {
    return mix(t_over_w.x, t_over_w.y, s) / mix(inverse_w.x, inverse_w.y, s);
}

fn pixel_contact(position: vec3<f32>, receiver_normal: vec3<f32>) -> ContactTrace {
    let direction = normalize(camera.sun_direction.xyz);
    let start_t = fixture.trace.y;
    var end_t = fixture.forward_length.w;
    let start_clip = camera.clip_from_world * vec4(position + direction * start_t, 1.0);
    var end_clip = camera.clip_from_world * vec4(position + direction * end_t, 1.0);
    if (start_clip.w <= 0.05) { return ContactTrace(1.0, 0.0, 1u); }
    if (end_clip.w < 0.05) {
        end_t = mix(start_t, end_t, (start_clip.w - 0.05) / (start_clip.w - end_clip.w));
        end_clip = camera.clip_from_world * vec4(position + direction * end_t, 1.0);
    }
    let dimensions = vec2<f32>(textureDimensions(captured_position));
    let project_scale = dimensions * vec2(0.5, -0.5);
    let p0 = start_clip.xy / start_clip.w * project_scale + dimensions * 0.5;
    let p1 = end_clip.xy / end_clip.w * project_scale + dimensions * 0.5;
    let delta = p1 - p0;
    let major = max(abs(delta.x), abs(delta.y));
    // A ray contained in one pixel has no reliable separate blocker in this depth layer.
    if (major < 0.5 || any(p0 < vec2(0.5)) || any(p0 > dimensions - vec2(0.5))) {
        return ContactTrace(1.0, 0.0, 1u);
    }
    let y_major = abs(delta.y) > abs(delta.x);
    let p_major = select(p0.x, p0.y, y_major);
    let sign_major = sign(select(delta.x, delta.y, y_major));
    // Start at the next pixel centre; never read the originating receiver pixel.
    let first = ((floor(p_major) + 0.5 + sign_major) - p_major) / sign_major;
    let inverse_w = vec2(1.0 / start_clip.w, 1.0 / end_clip.w);
    let t_over_w = vec2(start_t, end_t) * inverse_w;
    var screen_end = 1.0;
    for (var axis = 0u; axis < 2u; axis++) {
        if (abs(delta[axis]) > 1e-5) {
            let bound = select(0.5, dimensions[axis] - 0.5, delta[axis] > 0.0);
            screen_end = min(screen_end, max(0.0, (bound - p0[axis]) / delta[axis]));
        }
    }
    // One read has already reconstructed the receiver and its local depth plane.
    let steps = min(u32(fixture.counts.z), 16u) - 1u;
    let end_s = min(screen_end, min(1.0, (first + f32(steps) - 0.5) / major));
    let reach = ray_parameter(end_s, inverse_w, t_over_w);
    var reads = 1u;
    var occlusion = 0.0;
    for (var step = 0u; step < steps; step++) {
        let centre = first + f32(step);
        let lo_s = max(0.0, (centre - 0.5) / major);
        let hi_s = min(end_s, (centre + 0.5) / major);
        if (lo_s >= hi_s) { break; }
        let sample_s = min(centre / major, end_s);
        let pixel = vec2<i32>(floor(p0 + delta * sample_s));
        let sample = textureLoad(captured_position, pixel, 0);
        reads++;
        if (sample.w == 0.0) { continue; }
        // Reject the local depth-derived plane, including the ground. The estimate
        // deliberately has the same depth-discontinuity limitations as a screen pass.
        let plane_distance = abs(dot(sample.xyz - position, receiver_normal));
        if (dot(receiver_normal, receiver_normal) > 0.5 && plane_distance <= fixture.contact.z) { continue; }
        let za = 1.0 / mix(inverse_w.x, inverse_w.y, lo_s);
        let zb = 1.0 / mix(inverse_w.x, inverse_w.y, hi_s);
        let z_min = min(za, zb);
        let z_max = max(za, zb);
        let scene_z = dot(sample.xyz - camera.camera_position.xyz, fixture.forward_length.xyz);
        // Intersect the complete depth interval, not one point on the ray. Only a
        // small finite thickness behind the recorded surface is permitted.
        if (z_max <= scene_z || z_min >= scene_z + fixture.trace.x) { continue; }
        let penetration = max(z_min - scene_z, 0.0);
        let confidence = 1.0 - smoothstep(0.0, fixture.trace.x, penetration);
        let hit_t = ray_parameter(sample_s, inverse_w, t_over_w);
        let distance_fade = 1.0 - smoothstep(fixture.contact.w, 1.0, hit_t / max(reach, start_t));
        let edge_distance = min(min(f32(pixel.x), f32(pixel.y)),
            min(dimensions.x - 1.0 - f32(pixel.x), dimensions.y - 1.0 - f32(pixel.y)));
        let edge_fade = smoothstep(0.0, 3.0, edge_distance);
        occlusion = max(occlusion, confidence * distance_fade * edge_fade);
        if (occlusion >= 0.999) { break; }
    }
    return ContactTrace(1.0 - occlusion, reach, reads);
}
