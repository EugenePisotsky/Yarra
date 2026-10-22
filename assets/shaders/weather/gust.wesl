// The gusts that cross the land with the wind (crates/atmosphere/src/wind.rs, whose `Gusts::at`
// this mirrors): a frozen pattern carried downwind with the air, reaching further along the wind
// than across it. `gusts` is the environment's: the wind's unit direction in XZ, and where the
// pattern lies along and across it at render XZ zero, world-anchored and wrapped to its period.

// Render XZ `p` in the wind's frame: metres along and across the wind, in the pattern's place.
fn wind_frame(p: vec2<f32>, gusts: vec4<f32>) -> vec2<f32> {
    return vec2(dot(p, gusts.xy), p.x * gusts.y - p.y * gusts.x) + gusts.zw;
}

// The air's speed at `frame` (wind_frame) as a share of its mean, swelling by `swell` each way:
// two octaves of cells 60 by 40 and 30 by 20 metres, repeating after 960 metres
// (`GUST_OCTAVES`, `GUST_SHIFT`, `GUST_PERIOD`).
fn gust(frame: vec2<f32>, swell: f32) -> f32 {
    let n = gust_value(frame / vec2(60.0, 40.0), vec2(16, 24)) * 0.65
        + gust_value(frame / vec2(30.0, 20.0) + vec2(0.37, 0.61), vec2(32, 48)) * 0.35;
    return 1.0 + swell * (2.0 * smoothstep(0.25, 0.75, n) - 1.0);
}

// Smooth value noise over cells repeating after `period` of them each way.
fn gust_value(p: vec2<f32>, period: vec2<i32>) -> f32 {
    let cell = floor(p);
    let f = p - cell;
    let s = f * f * (3.0 - 2.0 * f);
    let c = vec2<i32>(cell);
    let low = mix(gust_hash(c, period), gust_hash(c + vec2(1, 0), period), s.x);
    let high = mix(gust_hash(c + vec2(0, 1), period), gust_hash(c + vec2(1, 1), period), s.x);
    return mix(low, high, s.y);
}

fn gust_hash(cell: vec2<i32>, period: vec2<i32>) -> f32 {
    let w = bitcast<vec2<u32>>(((cell % period) + period) % period);
    var v = (w.x * 0x8da6b343u) ^ (w.y * 0xd8163841u);
    v = v ^ (v >> 16u);
    v = v * 0x7feb352du;
    v = v ^ (v >> 15u);
    return f32(v & 0x00ffffffu) / 16777215.0;
}
