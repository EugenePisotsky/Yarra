// LOD crossfades as 4x MSAA sample masks (CROSSFADE_SAMPLE_MASK, set by the cloud material
// for 4-sample views). Bevy's visibility-range dither keeps or discards whole pixels in a
// 4x4 pattern, which reads as a stipple without temporal AA. Here an object fading out
// covers a pixel's low samples and the one fading in the remaining high ones, by the same
// pattern, so the resolve blends them: 64 steps instead of 16 and no visible pattern.
const DITHER_THRESHOLD_MAP: vec4<u32> = vec4(0x0a020800, 0x060e040c, 0x09010b03, 0x050d070f);

// Bevy's dither level (0 visible, 1..15 fading out, -15..-1 fading in, else hidden) as
// the covered samples of a 4-sample pixel; 0 when nothing is covered.
fn crossfade_sample_mask(frag_coord: vec4<f32>, dither: i32) -> u32 {
    if dither == 0 {
        return 0xfu;
    }
    if dither <= -16 || dither >= 16 {
        return 0u;
    }
    let coords = vec2<u32>(floor(frag_coord.xy)) % 4u;
    let threshold = i32((DITHER_THRESHOLD_MAP[coords.y] >> (coords.x * 8u)) & 0xffu);
    if dither > 0 {
        // Visible sixteenths 16 - dither, spread over the pixel's samples from the lowest.
        let samples = u32((4 * (16 - dither) + threshold) >> 4u);
        return (1u << samples) - 1u;
    }
    // The complement of an object fading out at level dither + 16, from the highest.
    let samples = u32((4 * (16 + dither) + 15 - threshold) >> 4u);
    return ((1u << samples) - 1u) << (4u - samples);
}

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
