// Shared artistic canopy envelope. Ground and lower blades sample the same world-space field.
// Distance develops stationary pockets; it never scrolls the pattern or exceeds the shade ceiling.
fn canopy_hash(p: vec2<i32>) -> f32 {
    var h = bitcast<u32>(p.x) * 1597334677u ^ bitcast<u32>(p.y) * 3812015801u;
    h = (h ^ (h >> 16u)) * 2246822519u;
    return f32(h ^ (h >> 13u)) / 4294967295.0;
}
fn canopy_noise(p: vec2<f32>) -> f32 {
    let c = vec2<i32>(floor(p));
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    return mix(mix(canopy_hash(c), canopy_hash(c + vec2(1, 0)), w.x),
        mix(canopy_hash(c + vec2(0, 1)), canopy_hash(c + vec2(1, 1)), w.x), w.y);
}
fn canopy_visibility_at(world_xz: vec2<f32>, height: f32, camera_distance: f32,
    appearance: vec4<f32>, shape: vec4<f32>, distance_settings: vec4<f32>,
    origin: vec4<f32>, surface_amount: f32, edge_depth: f32, horizontal_distance: f32) -> f32 {
    if appearance.x <= 0.0 || surface_amount <= 0.0 { return 1.0; }
    // Every pocket stays below this maximum height. Reject unshaded fragments before noise;
    // the packed patchiness/softness controls keep the envelope inside these bounds.
    if height >= max(0.02, shape.x) || edge_depth <= 0.0 || horizontal_distance >= 96.0 {
        return 1.0;
    }
    let far = smoothstep(distance_settings.x, distance_settings.y, camera_distance);
    let distance_fade = mix(appearance.w, 1.0, far);
    if distance_fade <= 0.0 { return 1.0; }
    let p = (world_xz + origin.xy) / max(shape.z, 0.2);
    let n = canopy_noise(p) * 0.75 + canopy_noise(p * 2.13 + vec2(7.3, -4.7)) * 0.25;
    // Start with pocket cores, then gradually reveal their surroundings. Far openings survive.
    let threshold = distance_settings.w + (1.0 - far) * distance_settings.z * 0.4;
    let patches = smoothstep(threshold, threshold + 0.3, n);
    let shelter = mix(1.0, patches, shape.w);
    // Height above the local ground plane, not blade length: a curled lower tip belongs
    // to the underlayer, while a neighbouring upright blade emerges from the same pocket.
    let top = max(0.02, shape.x) * mix(0.65, 1.0, shelter);
    let below = 1.0 - smoothstep(top * (1.0 - shape.y), top, max(height, 0.0));
    let edge = smoothstep(0.0, max(origin.z, 0.05), max(edge_depth, 0.0));
    // Match the 96 m procedural-grass domain; do not leave shaded bare terrain beyond it.
    let range_fade = 1.0 - smoothstep(80.0, 96.0, horizontal_distance);
    return 1.0 - appearance.x * clamp(surface_amount, 0.0, 1.0)
        * distance_fade * shelter * below * edge * range_fade;
}
