// Bevy's 4x4 dither pattern for visibility-range crossfades.
const DITHER_THRESHOLD_MAP: vec4<u32> = vec4(0x0a020800, 0x060e040c, 0x09010b03, 0x050d070f);

// Bevy's visibility_range_dither (whole pixels), for pipelines Bevy gives no
// VISIBILITY_RANGE_DITHER: tree impostors, so impostor and mesh crossfades share one pattern.
fn dither_discard(frag_coord: vec4<f32>, dither: i32) {
    if dither == 0 {
        return;
    }
    if dither <= -16 || dither >= 16 {
        discard;
    }
    let coords = vec2<u32>(floor(frag_coord.xy)) % 4u;
    let threshold = i32((DITHER_THRESHOLD_MAP[coords.y] >> (coords.x * 8u)) & 0xffu);
    if (dither >= 0 && dither + threshold >= 16) || (dither < 0 && 1 + dither + threshold <= 0) {
        discard;
    }
}
