//! The wind near the ground: its speed, and the gusts that cross the land with it. One frozen
//! pattern of gusts is carried downwind with the air, so a gust that lifts snow off the ground
//! upwind reaches the camera seconds later and hurries the falling snow there.
//! `shaders/weather/gust.wesl` evaluates the same pattern on the GPU.
use crate::{AtmosphereState, WorldEnvironmentView, environment::EnvironmentOrigin};

use bevy::prelude::*;

/// Air speed near the ground at a weather wind strength of 1, metres a second.
pub const AIR_SPEED: f32 = 2.5;

/// The gusts' pattern repeats after this many metres along and across the wind: a whole number
/// of its octaves' cells each way.
pub const GUST_PERIOD: f64 = 960.0;

/// The pattern's two octaves: cells in metres along and across the wind (gusts reach further
/// downwind than across it), and their share.
const GUST_OCTAVES: [([f32; 2], f32); 2] = [([60.0, 40.0], 0.65), ([30.0, 20.0], 0.35)];

/// The second octave's cells are offset from the first's, by this share of a cell.
const GUST_SHIFT: [f32; 2] = [0.37, 0.61];

/// How far gusts swell the wind above and below its mean at a weather gustiness of 1, and at
/// most: a lull never stills the air.
const GUST_SWELL: f32 = 0.35;

const MAX_SWELL: f32 = 0.6;

#[derive(Resource, Clone, Copy, Debug)]
pub struct Gusts {
    /// Unit direction the wind blows towards, in XZ.
    pub direction: Vec2,
    /// Mean air speed, metres a second.
    pub speed: f32,
    /// How far gusts swell the speed above and below its mean, as a share of it.
    pub swell: f32,
    /// Metres the air has travelled downwind, wrapped to the pattern's period.
    travel: f64,
    /// The gust where the camera is, as a share of the mean speed.
    pub here: f32,
}

impl Default for Gusts {
    fn default() -> Self {
        Self {
            direction: Vec2::X,
            speed: AIR_SPEED,
            swell: GUST_SWELL,
            travel: 0.0,
            here: 1.0,
        }
    }
}

impl Gusts {
    /// The share of a period the air has travelled downwind, 0..1.
    pub fn travelled(&self) -> f32 {
        (self.travel / GUST_PERIOD) as f32
    }

    /// Where the pattern lies along and across the wind at render XZ zero, for a world whose
    /// render origin is `origin`: world-anchored and wrapped to the period, so it never jumps
    /// when the origin moves.
    pub fn offsets(&self, origin: [f64; 2]) -> [f32; 2] {
        let [x, z] = [f64::from(self.direction.x), f64::from(self.direction.y)];
        let along = origin[0] * x + origin[1] * z - self.travel;
        let across = origin[0] * z - origin[1] * x;
        [along, across].map(|v| v.rem_euclid(GUST_PERIOD) as f32)
    }

    /// The air's speed at render XZ `p` as a share of the mean, with the pattern at `offsets`.
    pub fn at(&self, p: Vec2, offsets: [f32; 2]) -> f32 {
        let along = p.dot(self.direction) + offsets[0];
        let across = p.perp_dot(self.direction) + offsets[1];
        let t = smoothstep(0.25, 0.75, pattern(along, across));
        1.0 + self.swell * (2.0 * t - 1.0)
    }
}

/// The gusts' pattern `along` and `across` metres in the wind's frame, 0..1.
fn pattern(along: f32, across: f32) -> f32 {
    GUST_OCTAVES
        .iter()
        .zip([[0.0; 2], GUST_SHIFT])
        .map(|(&(cells, share), shift)| {
            let period = cells.map(|c| (GUST_PERIOD as f32 / c).round() as i32);
            let p = Vec2::new(along / cells[0] + shift[0], across / cells[1] + shift[1]);
            value(p, period) * share
        })
        .sum()
}

/// Smooth value noise over cells repeating after `period` of them each way.
fn value(p: Vec2, period: [i32; 2]) -> f32 {
    let cell = p.floor();
    let f = p - cell;
    let f = f * f * (3.0 - 2.0 * f);
    let (x, y) = (cell.x as i32, cell.y as i32);
    let corner = |dx: i32, dy: i32| {
        hash(
            (x + dx).rem_euclid(period[0]),
            (y + dy).rem_euclid(period[1]),
        )
    };
    let low = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * f.x;
    let high = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * f.x;
    low + (high - low) * f.y
}

fn hash(x: i32, y: i32) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841);
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    (v & 0x00ff_ffff) as f32 / 16_777_215.0
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Carries the gusts downwind with the weather's wind and finds the one at the camera.
pub(crate) fn advance(
    state: Res<AtmosphereState>,
    time: Res<Time>,
    origin: Res<EnvironmentOrigin>,
    views: Query<&GlobalTransform, With<WorldEnvironmentView>>,
    mut gusts: ResMut<Gusts>,
) {
    let heading = state.profile.clouds.wind_degrees.to_radians();
    let weather = state.weather;
    gusts.direction = Vec2::new(heading.cos(), heading.sin());
    gusts.speed = AIR_SPEED * weather.map_or(1.0, |w| w.wind_strength);
    gusts.swell = (GUST_SWELL * weather.map_or(1.0, |w| w.wind_gustiness)).clamp(0.0, MAX_SWELL);
    let step = f64::from(time.delta_secs().min(0.1));
    gusts.travel = (gusts.travel + f64::from(gusts.speed) * step).rem_euclid(GUST_PERIOD);
    let offsets = gusts.offsets(origin.0);
    gusts.here = views
        .iter()
        .next()
        .map_or(1.0, |view| gusts.at(view.translation().xz(), offsets));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gusts() -> Gusts {
        Gusts {
            direction: Vec2::new(0.6, 0.8),
            speed: 5.0,
            swell: 0.4,
            travel: 123.0,
            here: 1.0,
        }
    }

    #[test]
    fn the_pattern_repeats_after_its_period_each_way() {
        let period = GUST_PERIOD as f32;

        for (along, across) in [(3.7, 11.2), (-250.0, 480.5), (901.3, -77.0)] {
            let here = pattern(along, across);
            assert!((pattern(along + period, across) - here).abs() < 1e-4);
            assert!((pattern(along, across - period) - here).abs() < 1e-4);
        }
    }

    #[test]
    fn gusts_swell_the_wind_both_ways_around_its_mean() {
        let g = gusts();
        let offsets = g.offsets([0.0, 0.0]);
        let samples: Vec<f32> = (0..400)
            .map(|i| {
                g.at(
                    Vec2::new((i % 20) as f32 * 37.0, (i / 20) as f32 * 41.0),
                    offsets,
                )
            })
            .collect();
        let (low, high) = samples
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), &s| (l.min(s), h.max(s)));
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        assert!(low >= 1.0 - g.swell - 1e-6 && high <= 1.0 + g.swell + 1e-6);
        assert!(low < 0.8 && high > 1.2, "{low}..{high}");
        assert!((mean - 1.0).abs() < 0.1, "{mean}");
    }

    #[test]
    fn gusts_travel_downwind_with_the_air_and_stay_put_as_the_origin_moves() {
        let mut g = gusts();
        let p = Vec2::new(40.0, -12.0);
        let before = g.at(p, g.offsets([0.0, 0.0]));
        g.travel += 30.0;
        let downwind = p + g.direction * 30.0;
        assert!((g.at(downwind, g.offsets([0.0, 0.0])) - before).abs() < 1e-4);

        let origin = [5120.0, -2048.0];
        let shifted = downwind - Vec2::new(origin[0] as f32, origin[1] as f32);
        assert!((g.at(shifted, g.offsets(origin)) - before).abs() < 1e-3);
    }
}
