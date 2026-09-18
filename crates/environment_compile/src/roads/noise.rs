//! Small deterministic value noise evaluated at compile time, in logical world metres.
fn hash(x: i64, z: i64, seed: u32) -> f64 {
    let mut v = (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (z as u64).wrapping_mul(0xd1b54a32d192ed03)
        ^ u64::from(seed);
    v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
    v ^= v >> 31;
    (v >> 11) as f64 / (1u64 << 53) as f64
}
fn smooth(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
fn value(p: [f64; 2], seed: u32) -> f64 {
    let x = p[0].floor() as i64;
    let z = p[1].floor() as i64;
    let u = smooth(p[0] - p[0].floor());
    let v = smooth(p[1] - p[1].floor());
    let a = hash(x, z, seed) * (1.0 - u) + hash(x + 1, z, seed) * u;
    let b = hash(x, z + 1, seed) * (1.0 - u) + hash(x + 1, z + 1, seed) * u;
    a * (1.0 - v) + b * v
}
pub(super) fn patch(p: [f64; 2], size: f32, seed: u32) -> f64 {
    let p = p.map(|v| v / f64::from(size));
    let warp = [
        value(p.map(|v| v * 0.43), seed ^ 0x731) - 0.5,
        value(p.map(|v| v * 0.43), seed ^ 0x891) - 0.5,
    ];
    let p = [p[0] + warp[0] * 0.6, p[1] + warp[1] * 0.6];
    value(p, seed) * 0.75 + value(p.map(|v| v * 2.0), seed ^ 0x463) * 0.25
}
