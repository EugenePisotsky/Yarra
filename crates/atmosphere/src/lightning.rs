//! Lightning in storms. A strike is a seed, the point on the ground it hits and its start time;
//! from those, pure functions give the branching channel down from the cloud base and the light
//! over the strike's second: a few return strokes, each a bright pulse of the channel, and the
//! clouds' afterglow. The engine schedules strikes from the weather; the sky composite draws the
//! channel and the flash on the clouds and haze, and `crate::apply` adds the flash to the
//! ambient light.
use bevy::math::Vec3;

/// Segments of one channel: the main channel and its branches.
pub const SEGMENTS: usize = 16;
const MAIN: usize = 10;
/// Seconds a strike lasts, afterglow included.
pub const DURATION: f32 = 1.2;
/// Linear colour of the flash, and the ambient light (Bevy's brightness) a close strike adds at
/// its peak. The cloud deck spreads it: a flash lights the ground from everywhere.
pub const FLASH_COLOR: [f32; 3] = [0.85, 0.9, 1.0];
pub const FLASH_AMBIENT: f32 = 2000.0;
/// Light a close strike's flash scatters from the cloud deck and the rain haze, unexposed.
pub const FLASH_SKY: f32 = 9000.0;

/// One strike: what it hits (render space) and when it began.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strike {
    pub seed: u32,
    pub ground: Vec3,
    /// Height of the cloud base it falls from.
    pub base: f32,
    pub started: f64,
}

/// A strike as drawn this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightningFlash {
    /// Where the channel leaves the cloud base, render space.
    pub top: Vec3,
    /// Light of the flash relative to a close strike's peak (0..~1.5).
    pub flash: f32,
    /// Brightness of the channel itself, 0..1.
    pub channel: f32,
    /// Segment ends in pairs (xyz, width), render space.
    pub segments: [[f32; 4]; 2 * SEGMENTS],
}

fn hash(seed: u32, i: u32) -> f32 {
    let mut x = seed ^ i.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x as f32 / u32::MAX as f32
}

/// Return strokes: two to four, tens of milliseconds apart.
fn strokes(seed: u32) -> impl Iterator<Item = f32> {
    let count = 2 + (hash(seed, 101) * 2.99) as u32;
    let mut at = 0.0;
    (0..count).map(move |k| {
        if k > 0 {
            at += 0.04 + 0.1 * hash(seed, 102 + k);
        }
        at
    })
}

/// Flash and channel brightness `age` seconds into a strike.
pub fn envelope(seed: u32, age: f32) -> (f32, f32) {
    if !(0.0..DURATION).contains(&age) {
        return (0.0, 0.0);
    }
    let mut flash = 0.0;
    let mut channel = 0.0_f32;
    for (k, at) in strokes(seed).enumerate() {
        let since = age - at;
        if since < 0.0 {
            continue;
        }
        // The first stroke is the brightest.
        let strength = if k == 0 {
            1.0
        } else {
            0.5 + 0.4 * hash(seed, 110 + k as u32)
        };
        channel = channel.max(strength * (-since / 0.035).exp());
        flash += strength * (-since / 0.06).exp();
    }
    // The clouds' afterglow fades over the rest of the second.
    flash += 0.12 * (1.0 - age / DURATION);
    (flash.min(1.5), channel.min(1.0))
}

/// The channel of `strike`: a jagged main channel from the cloud base to the ground, with
/// branches forking down and out from it.
pub fn channel(strike: &Strike) -> [[f32; 4]; 2 * SEGMENTS] {
    let seed = strike.seed;
    let height = (strike.base - strike.ground.y).max(50.0);
    let step = height / MAIN as f32;
    let mut segments = [[0.0; 4]; 2 * SEGMENTS];
    let mut point = strike.ground + Vec3::Y * height;
    let mut points = [point; MAIN + 1];
    for i in 0..MAIN {
        // Each step wanders sideways, and the channel ends on the strike point.
        let left = (MAIN - i) as f32;
        let towards = (strike.ground + Vec3::Y * (height - step * (i + 1) as f32) - point) / left;
        let wander = Vec3::new(
            hash(seed, i as u32) - 0.5,
            0.0,
            hash(seed, 50 + i as u32) - 0.5,
        ) * step
            * 0.9;
        let next = if i + 1 == MAIN {
            strike.ground
        } else {
            point + towards + wander + Vec3::Y * (-step - towards.y)
        };
        segments[2 * i] = [point.x, point.y, point.z, 1.0];
        segments[2 * i + 1] = [next.x, next.y, next.z, 1.0];
        point = next;
        points[i + 1] = next;
    }
    // Branches fork from the upper channel, two segments each, thinner and dimmer.
    for b in 0..(SEGMENTS - MAIN) / 2 {
        let from = 1 + (hash(seed, 200 + b as u32) * (MAIN as f32 * 0.6)) as usize;
        let mut point = points[from];
        let angle = hash(seed, 210 + b as u32) * std::f32::consts::TAU;
        let out = Vec3::new(angle.cos(), 0.0, angle.sin());
        for s in 0..2 {
            let next = point + out * step * (0.5 + 0.4 * hash(seed, 220 + b as u32 * 2 + s))
                - Vec3::Y * step * (0.5 + 0.5 * hash(seed, 230 + b as u32 * 2 + s));
            let i = MAIN + b * 2 + s as usize;
            segments[2 * i] = [point.x, point.y, point.z, 0.45];
            segments[2 * i + 1] = [next.x, next.y, next.z, 0.45];
            point = next;
        }
    }
    segments
}

/// The strike as drawn `now`, or `None` once it is over.
pub fn flash(strike: &Strike, now: f64) -> Option<LightningFlash> {
    let age = (now - strike.started) as f32;
    let (flash, channel) = envelope(strike.seed, age);
    (flash > 0.0).then(|| LightningFlash {
        top: Vec3::new(strike.ground.x, strike.base, strike.ground.z),
        flash,
        channel,
        segments: self::channel(strike),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strike(seed: u32) -> Strike {
        Strike {
            seed,
            ground: Vec3::new(3000.0, 10.0, -2000.0),
            base: 1200.0,
            started: 100.0,
        }
    }

    #[test]
    fn strokes_pulse_and_fade_within_the_strike() {
        for seed in 0..50 {
            let (peak, channel) = envelope(seed, 0.0);
            assert!(peak > 1.0 && channel == 1.0, "{seed}");
            // Between strokes the channel dims; afterwards only the afterglow is left.
            let (late, dark) = envelope(seed, 0.9);
            assert!(late < 0.1 && dark < 0.01, "{seed} {late} {dark}");
            assert_eq!(envelope(seed, DURATION), (0.0, 0.0));
            assert_eq!(envelope(seed, -0.01), (0.0, 0.0));
            assert!(flash(&strike(seed), 101.3).is_none());
        }
    }

    #[test]
    fn the_channel_runs_from_the_cloud_base_to_the_strike_point() {
        for seed in 0..50 {
            let s = strike(seed);
            let segments = channel(&s);
            let top = Vec3::from_slice(&segments[0][..3]);
            let bottom = Vec3::from_slice(&segments[2 * MAIN - 1][..3]);
            assert!((top.y - s.base).abs() < 1e-3);
            assert!(bottom.distance(s.ground) < 1e-3);
            // Connected, falling all the way, and never far from the strike's column.
            for i in 0..MAIN {
                let a = Vec3::from_slice(&segments[2 * i][..3]);
                let b = Vec3::from_slice(&segments[2 * i + 1][..3]);
                assert!(b.y < a.y, "{seed}");
                if i > 0 {
                    assert_eq!(segments[2 * i - 1], segments[2 * i]);
                }
                let off = (a - s.ground).with_y(0.0).length();
                assert!(off < s.base - s.ground.y, "{seed}");
            }
            for branch in &segments[2 * MAIN..] {
                assert!(branch[3] < 1.0 && branch[1] > s.ground.y - 200.0);
            }
        }
    }
}
