//! Small deterministic, periodic 3D asset. Generated once, never in the frame loop.
fn hash(x: i32, y: i32, z: i32, period: i32) -> f32 {
    let mut v = (x.rem_euclid(period) as u32).wrapping_mul(0x8da6b343)
        ^ (y.rem_euclid(period) as u32).wrapping_mul(0xd8163841)
        ^ (z.rem_euclid(period) as u32).wrapping_mul(0xcb1ab31f);
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb352d);
    v ^= v >> 15;
    (v & 0x00ffffff) as f32 / 16777215.
}
fn value(p: [f32; 3], period: i32) -> f32 {
    let i = p.map(|x| x.floor() as i32);
    let f = p.map(|x| {
        let t = x - x.floor();
        t * t * (3. - 2. * t)
    });
    let mut v = 0.;
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let w = [x, y, z]
                    .into_iter()
                    .enumerate()
                    .map(|(j, k)| if k == 0 { 1. - f[j] } else { f[j] })
                    .product::<f32>();
                v += w * hash(i[0] + x, i[1] + y, i[2] + z, period);
            }
        }
    }
    v
}
fn worley(p: [f32; 3], period: i32) -> f32 {
    let base = p.map(|v| v.floor() as i32);
    let mut best = 3.0f32;
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                let c = [base[0] + x, base[1] + y, base[2] + z];
                let jitter = [
                    hash(c[0], c[1], c[2], period),
                    hash(c[0] + 37, c[1] + 19, c[2] + 13, period),
                    hash(c[0] + 11, c[1] + 41, c[2] + 29, period),
                ];
                let d = (0..3)
                    .map(|j| (c[j] as f32 + jitter[j] - p[j]).powi(2))
                    .sum::<f32>();
                best = best.min(d);
            }
        }
    }
    (1. - best.sqrt()).clamp(0., 1.)
}
pub fn generate(size: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((size * size * size * 4) as usize);
    for z in 0..size {
        for y in 0..size {
            for x in 0..size {
                let uv = [x, y, z].map(|v| v as f32 / size as f32);
                let v = value(uv.map(|v| v * 4.), 4) * 0.625
                    + value(uv.map(|v| v * 8.), 8) * 0.25
                    + value(uv.map(|v| v * 16.), 16) * 0.125;
                let w = worley(uv.map(|v| v * 4.), 4);
                let detail =
                    worley(uv.map(|v| v * 12.), 12) * 0.7 + worley(uv.map(|v| v * 24.), 24) * 0.3;
                for c in [
                    v * 0.65 + w * 0.35,
                    detail,
                    value(uv.map(|v| v * 2.), 2),
                    value([uv[0] * 4. + 13.7, uv[1] * 4. + 7.1, uv[2] * 4. + 3.3], 4),
                ] {
                    data.push((c * 255.).round() as u8);
                }
            }
        }
    }
    data
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn periodic_and_repeatable() {
        let p = [0.37, 1.2, -0.2];
        assert!((value(p, 4) - value([p[0] + 4., p[1], p[2]], 4)).abs() < 0.00001);
        assert!((worley(p, 4) - worley([p[0] + 4., p[1], p[2]], 4)).abs() < 0.00001);
        assert_eq!(generate(4), generate(4));
    }
}
