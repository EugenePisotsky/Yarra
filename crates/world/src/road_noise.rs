//! The value noise roads are drawn with, shared by the compiler, the runtime's wear
//! shading and the shaders (its hash is 32-bit so they reproduce it).

/// Domain-warped value noise, 0..1, in patches about `size` metres across, at world XZ `p`.
/// Its hash is 32-bit so shaders reproduce it.
pub fn road_patch(p: [f64; 2], size: f32, seed: u32) -> f64 {
    let p = p.map(|v| v / f64::from(size));
    let warp = [
        value(p.map(|v| v * 0.43), seed ^ 0x731) - 0.5,
        value(p.map(|v| v * 0.43), seed ^ 0x891) - 0.5,
    ];
    let p = [p[0] + warp[0] * 0.6, p[1] + warp[1] * 0.6];
    value(p, seed) * 0.75 + value(p.map(|v| v * 2.0), seed ^ 0x463) * 0.25
}

fn hash32(mut v: u32) -> u32 {
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846c_a68b);
    v ^ (v >> 16)
}

fn hash_bits(x: i64, z: i64, seed: u32) -> u32 {
    hash32(
        (x as i32 as u32).wrapping_mul(0x8da6_b343)
            ^ (z as i32 as u32).wrapping_mul(0xd816_3841)
            ^ seed,
    )
}

fn hash(x: i64, z: i64, seed: u32) -> f64 {
    f64::from(hash_bits(x, z, seed)) / 4_294_967_296.0
}

fn quintic(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

pub fn value(p: [f64; 2], seed: u32) -> f64 {
    let x = p[0].floor() as i64;
    let z = p[1].floor() as i64;
    let u = quintic(p[0] - p[0].floor());
    let v = quintic(p[1] - p[1].floor());
    let a = hash(x, z, seed) * (1.0 - u) + hash(x + 1, z, seed) * u;
    let b = hash(x, z + 1, seed) * (1.0 - u) + hash(x + 1, z + 1, seed) * u;
    a * (1.0 - v) + b * v
}

/// Four value noises at once, 0..1: each lattice corner's hash gives a byte to each.
pub fn value4(p: [f64; 2], seed: u32) -> [f64; 4] {
    let x = p[0].floor() as i64;
    let z = p[1].floor() as i64;
    let u = quintic(p[0] - p[0].floor());
    let v = quintic(p[1] - p[1].floor());
    let corner = |dx, dz| {
        let h = hash_bits(x + dx, z + dz, seed);
        [0, 8, 16, 24].map(|s| f64::from((h >> s) & 0xff) / 255.0)
    };
    let (a, b, c, d) = (corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1));
    std::array::from_fn(|i| {
        (a[i] * (1.0 - u) + b[i] * u) * (1.0 - v) + (c[i] * (1.0 - u) + d[i] * u) * v
    })
}

pub fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn falloff(distance: f64, radius: f64, softness: f64) -> f64 {
    smooth((radius + softness - distance) / (2.0 * softness))
}
