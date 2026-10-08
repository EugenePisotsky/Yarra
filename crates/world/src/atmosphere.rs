//! Small, renderer-independent atmosphere definitions. Colors are explicitly sRGB;
//! evaluation interpolates them in linear light. Time is a day number and a normalized day
//! phase.
use serde::{Deserialize, Serialize};

/// Sky light of one part of the day: the ambient light surfaces receive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightingPhase {
    pub ambient_srgb: [f32; 3],
    pub ambient_lux: f32,
}

/// The moon. It follows the sun's path across the sky and lags behind the sun by its age: new
/// beside the sun, full opposite it, rising about 50 minutes later each day. Its light is
/// art-directed moonlight: a full moon's illuminance, less as less of the moon is lit, and only
/// at night.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NightLighting {
    pub enabled: bool,
    /// Days since new moon at the first day's midnight: 7.4 first quarter, 14.8 full, 22.1
    /// last quarter.
    pub age_days: f32,
    pub diameter_degrees: f32,
    pub light_srgb: [f32; 3],
    /// Moonlight of a full moon, before the atmosphere.
    pub illuminance_lux: f32,
    pub exposure_ev100: f32,
}

impl Default for NightLighting {
    fn default() -> Self {
        Self {
            enabled: true,
            // Waxing gibbous: the first evening's moon is high in the east at sunset and sets
            // after midnight.
            age_days: 10.0,
            diameter_degrees: 2.0,
            light_srgb: [140.0 / 255.0, 191.0 / 255.0, 1.0],
            illuminance_lux: 1800.0,
            exposure_ev100: 8.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AtmosphereProfile {
    pub outdoor: bool,
    pub initial_phase: f32,
    pub day_seconds: f32,
    pub azimuth_degrees: f32,
    pub maximum_elevation_degrees: f32,
    pub sun_diameter_degrees: f32,
    /// Sunlight above the atmosphere, which reddens and dims it towards the horizon.
    pub sun_srgb: [f32; 3],
    pub sun_lux: f32,
    pub exposure_ev100: f32,
    pub bloom_intensity: f32,
    /// Additional low-altitude aerosol visibility, independent of molecular scattering.
    pub visibility_metres: f32,
    pub haze_srgb: [f32; 3],
    pub molecular_density: f32,
    /// Sky light at Night, Sunrise, Day, Sunset. Sunrise and sunset share an elevation, not a
    /// palette.
    pub phases: [LightingPhase; 4],
    #[serde(default)]
    pub night: NightLighting,
    #[serde(default)]
    pub clouds: crate::clouds::CloudSettings,
    /// Ground haze and valley mist.
    #[serde(default)]
    pub fog: crate::fog::FogSettings,
    /// Weather presets and random sequence the game plays over this profile.
    #[serde(default)]
    pub weather: crate::weather::WeatherSettings,
}

pub const PHASE_NAMES: [&str; 4] = ["Night", "Sunrise", "Day", "Sunset"];
// Low visible sun for inspection, just above the geometric horizon crossing.
pub const PHASE_TIMES: [f32; 4] = [0.0, 0.265, 0.5, 0.735];
/// Days from one new moon to the next.
pub const SYNODIC_MONTH_DAYS: f32 = 29.530_59;
/// The moon's phases by eighths of a month, from new.
pub const MOON_PHASE_NAMES: [&str; 8] = [
    "new moon",
    "waxing crescent",
    "first quarter",
    "waxing gibbous",
    "full moon",
    "waning gibbous",
    "last quarter",
    "waning crescent",
];
/// Index into [`MOON_PHASE_NAMES`] of the phase nearest a moon `age` days old.
pub fn moon_phase(age: f32) -> usize {
    (age.rem_euclid(SYNODIC_MONTH_DAYS) / SYNODIC_MONTH_DAYS * 8.0).round() as usize % 8
}
/// The sun stops lighting even the high sky by the end of astronomical twilight; it fades out
/// over the last degrees before.
const SUN_FADING_DEGREES: f32 = -10.0;
const SUN_GONE_DEGREES: f32 = -18.0;
/// Sun depression by which the sky light has the night's hue: twilight turns blue well before
/// it is dark.
const TWILIGHT_BLUE_DEGREES: f32 = 4.0;
/// Share of the night sky light's saturation taken away at the start of night. The night's
/// blue is authored for the colourless look presentation gives full night; earlier, while
/// colours still show, it read electric.
const TWILIGHT_PALE: f32 = 0.5;
/// Depth below the horizon over which moonlight fades out after the moon's disc has set.
const MOON_SETTING_DEGREES: f32 = 3.0;
/// Moonlight is authored for a moon this high. A higher moon lights open ground more steeply,
/// which is partly balanced so the night keeps its tone as the moon climbs: under a moon at
/// 50° open ground gets about 1.4 times the light, where it would get 3.7.
const MOON_TONE_ELEVATION_DEGREES: f32 = 12.0;
const MOON_HEIGHT_BALANCE: f32 = 0.75;

impl Default for AtmosphereProfile {
    fn default() -> Self {
        Self {
            outdoor: true,
            initial_phase: 0.34,
            day_seconds: 1200.0,
            azimuth_degrees: 147.0,
            maximum_elevation_degrees: 60.0,
            sun_diameter_degrees: 0.75,
            sun_srgb: [1.0, 0.98, 0.95],
            sun_lux: 128_000.0,
            exposure_ev100: 13.0,
            bloom_intensity: 0.15,
            visibility_metres: 60_000.0,
            haze_srgb: [0.93, 0.96, 1.0],
            molecular_density: 1.0,
            phases: [
                LightingPhase {
                    ambient_srgb: [0.25, 0.34, 0.55],
                    ambient_lux: 700.0,
                },
                LightingPhase {
                    ambient_srgb: [0.54, 0.62, 0.82],
                    ambient_lux: 4500.0,
                },
                LightingPhase {
                    ambient_srgb: [0.60, 0.72, 0.92],
                    ambient_lux: 9000.0,
                },
                LightingPhase {
                    ambient_srgb: [0.58, 0.60, 0.80],
                    ambient_lux: 3750.0,
                },
            ],
            night: NightLighting::default(),
            clouds: Default::default(),
            fog: Default::default(),
            weather: Default::default(),
        }
    }
}

fn range(value: f32, minimum: f32, maximum: f32) -> bool {
    value.is_finite() && (minimum..=maximum).contains(&value)
}
fn color(value: [f32; 3]) -> bool {
    value.into_iter().all(|v| range(v, 0.0, 1.0))
}

impl AtmosphereProfile {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !range(self.initial_phase, 0.0, 1.0)
            || self.initial_phase == 1.0
            || !range(self.day_seconds, 10.0, 604_800.0)
            || !range(self.azimuth_degrees, -180.0, 180.0)
            || !range(self.maximum_elevation_degrees, 5.0, 85.0)
            || !range(self.sun_diameter_degrees, 0.1, 5.0)
            || !range(self.exposure_ev100, 0.0, 20.0)
            || !range(self.bloom_intensity, 0.0, 1.0)
            || !range(self.visibility_metres, 50.0, 100_000.0)
            || !range(self.molecular_density, 0.1, 4.0)
            || !color(self.haze_srgb)
            || !color(self.sun_srgb)
            || !range(self.sun_lux, 0.0, 200_000.0)
            || !range(self.night.age_days, 0.0, SYNODIC_MONTH_DAYS)
            || !range(self.night.diameter_degrees, 0.1, 8.0)
            || !range(self.night.illuminance_lux, 0.0, 20_000.0)
            || !range(self.night.exposure_ev100, 0.0, 20.0)
            || !color(self.night.light_srgb)
            || self
                .phases
                .iter()
                .any(|p| !color(p.ambient_srgb) || !range(p.ambient_lux, 0.0, 20_000.0))
        {
            return Err("Atmosphere settings contain an invalid color, time or physical range");
        }
        self.clouds.validate()?;
        self.fog.validate()?;
        self.weather.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EvaluatedAtmosphere {
    pub direction_to_sun: [f32; 3],
    pub sun_linear: [f32; 3],
    pub sun_lux: f32,
    pub ambient_linear: [f32; 3],
    pub ambient_lux: f32,
    pub direction_to_moon: [f32; 3],
    /// The moon's north, square to the direction to it: the pole of its path across the sky.
    pub moon_north: [f32; 3],
    pub moon_linear: [f32; 3],
    pub moon_lux: f32,
    /// Days since new moon and the share of the moon's face that is lit, 0..1.
    pub moon_age_days: f32,
    pub moon_lit: f32,
    pub exposure_ev100: f32,
    pub night_weight: f32,
    /// How far the eye has adapted to the night, 0..1: exposure and the loss of colour follow
    /// it. It runs ahead of the night's light, so twilight never reads darker than the night.
    pub adaptation: f32,
    /// Valley mist amount, 0..1 (`FogSettings::mist_amount`).
    pub mist: f32,
}

/// Sun elevation by which morning and evening mist has thinned to its high-sun amount.
pub const MIST_BURN_OFF_DEGREES: f32 = 40.0;

pub fn linear_rgb(srgb: [f32; 3]) -> [f32; 3] {
    srgb.map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    })
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The atmosphere on the first day (day 0) at `phase`.
pub fn evaluate(profile: &AtmosphereProfile, phase: f32) -> EvaluatedAtmosphere {
    evaluate_at(profile, 0, phase)
}

/// The atmosphere `phase` into day `day`; days only move the moon. Caller supplies a validated
/// profile and finite phase. No ECS, database or wall clock.
pub fn evaluate_at(profile: &AtmosphereProfile, day: i32, phase: f32) -> EvaluatedAtmosphere {
    let time = f64::from(day) + f64::from(phase);
    let phase = phase.rem_euclid(1.0);
    let maximum = profile.maximum_elevation_degrees.to_radians();
    let azimuth = profile.azimuth_degrees.to_radians();
    // Tilted great-circle path: rising at orbit 0, at the configured maximum elevation at
    // orbit 90°. The sun is at orbit 0 at 6:00.
    let across = [azimuth.cos(), 0.0, -azimuth.sin()];
    let up = [
        maximum.cos() * azimuth.sin(),
        maximum.sin(),
        maximum.cos() * azimuth.cos(),
    ];
    let path = |orbit: f32| -> [f32; 3] {
        std::array::from_fn(|i| across[i] * orbit.cos() + up[i] * orbit.sin())
    };
    let orbit = (phase - 0.25) * std::f32::consts::TAU;
    let direction_to_sun = path(orbit);
    let elevation = direction_to_sun[1].clamp(-1.0, 1.0).asin();
    let twilight = if phase < 0.5 { 1 } else { 3 };
    let (a, b, t) = if elevation >= 0.0 {
        (
            twilight,
            2,
            (elevation.to_degrees() / profile.maximum_elevation_degrees.min(25.0)).clamp(0.0, 1.0),
        )
    } else {
        (
            twilight,
            0,
            (-elevation.to_degrees() / profile.maximum_elevation_degrees.min(12.0)).clamp(0.0, 1.0),
        )
    };
    let t = smoothstep(t);
    // Mist burns off more slowly than the light changes: it lasts well into the morning.
    let mist = {
        let amount = &profile.fog.mist_amount;
        let t = if elevation >= 0.0 {
            smoothstep(
                elevation.to_degrees()
                    / profile.maximum_elevation_degrees.min(MIST_BURN_OFF_DEGREES),
            )
        } else {
            t
        };
        amount[a] + (amount[b] - amount[a]) * t
    };
    // Adapt only after sunset, reaching the authored night appearance by -12°.
    // Shallow sun paths still reach night at midnight and remain continuous.
    let night = smoothstep(-elevation.to_degrees() / profile.maximum_elevation_degrees.min(12.0));
    let adaptation = night.sqrt();
    // The moon lags the sun along its path by its elongation, which grows by a turn a month.
    let age =
        (f64::from(profile.night.age_days) + time).rem_euclid(f64::from(SYNODIC_MONTH_DAYS)) as f32;
    let elongation = age / SYNODIC_MONTH_DAYS * std::f32::consts::TAU;
    let direction_to_moon = path(orbit - elongation);
    let moon_lit = 0.5 * (1.0 - elongation.cos());
    let reference = MOON_TONE_ELEVATION_DEGREES.to_radians().sin();
    let moon_height = (reference / direction_to_moon[1].max(reference)).powf(MOON_HEIGHT_BALANCE);
    // After its disc has set the moon stops lighting the sky over a few degrees.
    let moon_radius = (profile.night.diameter_degrees * 0.5).to_radians();
    let moon_up = smoothstep(
        (direction_to_moon[1].clamp(-1.0, 1.0).asin() + moon_radius)
            / MOON_SETTING_DEGREES.to_radians()
            + 1.0,
    );
    let a = &profile.phases[a];
    let b = &profile.phases[b];
    let mix = |a: f32, b: f32| a + (b - a) * t;
    let mix_color = |a: [f32; 3], b: [f32; 3]| {
        let a = linear_rgb(a);
        let b = linear_rgb(b);
        std::array::from_fn(|i| mix(a[i], b[i]))
    };
    let overcast = if profile.clouds.enabled {
        ((profile.clouds.coverage - 0.6) / 0.35).clamp(0., 1.) * profile.clouds.density.min(1.)
    } else {
        0.
    };
    let mut ambient_linear = mix_color(a.ambient_srgb, b.ambient_srgb);
    if elevation < 0.0 {
        // Twilight sky light takes the night's colour while it dims: blue, not the sunset's,
        // and paler until night.
        let blue = smoothstep(-elevation.to_degrees() / TWILIGHT_BLUE_DEGREES);
        let from = linear_rgb(a.ambient_srgb);
        let to = linear_rgb(b.ambient_srgb);
        let (lf, lt) = (luminance(from).max(1e-6), luminance(to).max(1e-6));
        let pale = TWILIGHT_PALE * (1.0 - night);
        let to = to.map(|c| c + (lt - c) * pale);
        let brightness = luminance(ambient_linear);
        ambient_linear = std::array::from_fn(|i| {
            (from[i] / lf + (to[i] / lt - from[i] / lf) * blue) * brightness
        });
    }
    let mean = ambient_linear.iter().sum::<f32>() / 3.;
    for c in &mut ambient_linear {
        *c += (mean - *c) * overcast * 0.35;
    }
    // One sun above the atmosphere: the atmosphere alone reddens it, so the sky stays blue
    // around a low sun and turns deep blue after it has set.
    let sun_up = smoothstep(
        (elevation.to_degrees() - SUN_GONE_DEGREES) / (SUN_FADING_DEGREES - SUN_GONE_DEGREES),
    );
    // The orbit's pole, on the side above the horizon.
    let pole = [
        -up[1] * across[2],
        across[2] * up[0] - across[0] * up[2],
        across[0] * up[1],
    ];
    let pole = if pole[1] < 0.0 {
        pole.map(|v| -v)
    } else {
        pole
    };
    EvaluatedAtmosphere {
        direction_to_sun,
        sun_linear: linear_rgb(profile.sun_srgb),
        sun_lux: profile.sun_lux * sun_up,
        ambient_linear,
        ambient_lux: mix(a.ambient_lux, b.ambient_lux) * (1. + overcast * 0.4),
        direction_to_moon,
        moon_north: pole,
        moon_linear: linear_rgb(profile.night.light_srgb),
        moon_lux: if profile.night.enabled {
            // Art-directed: a half moon gives about a third of a full moon's light, where the
            // real moon gives a tenth.
            profile.night.illuminance_lux * moon_lit.powf(1.5) * moon_height * night * moon_up
        } else {
            0.0
        },
        moon_age_days: age,
        moon_lit,
        exposure_ev100: profile.exposure_ev100
            + (profile.night.exposure_ev100 - profile.exposure_ev100) * adaptation,
        night_weight: night,
        adaptation,
        mist,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moon_and_exposure_blend_continuously_and_leave_daylight_unchanged() {
        let p = AtmosphereProfile::default();
        let day = evaluate(&p, 0.5);
        assert_eq!(day.moon_lux, 0.0);
        assert_eq!(day.exposure_ev100, p.exposure_ev100);
        let night = evaluate(&p, 0.0);
        assert_eq!(night.sun_lux, 0.0);
        assert!(night.moon_lux > 0.0 && night.moon_lux < p.night.illuminance_lux);
        assert_eq!(night.exposure_ev100, p.night.exposure_ev100);
        assert!(night.direction_to_moon[1] > 0.0);
        assert!((night.direction_to_moon.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5);
        let full = AtmosphereProfile {
            night: NightLighting {
                age_days: SYNODIC_MONTH_DAYS / 2.0,
                ..p.night.clone()
            },
            ..p.clone()
        };
        // Up to the authored height a full moon gives the authored light; higher, less.
        let balance = |e: &EvaluatedAtmosphere| {
            let reference = MOON_TONE_ELEVATION_DEGREES.to_radians().sin();
            (reference / e.direction_to_moon[1].max(reference)).powf(MOON_HEIGHT_BALANCE)
        };
        let midnight = evaluate(&full, 0.0);
        assert!(balance(&midnight) < 0.5);
        assert!((midnight.moon_lux - p.night.illuminance_lux * balance(&midnight)).abs() < 1.0);
        let high = midnight.moon_lux * midnight.direction_to_moon[1];
        let low = p.night.illuminance_lux * MOON_TONE_ELEVATION_DEGREES.to_radians().sin();
        assert!(high > low && high < 1.6 * low, "{high} {low}");
        for maximum in [5.0, 60.0, 85.0] {
            let p = AtmosphereProfile {
                maximum_elevation_degrees: maximum,
                ..p.clone()
            };
            for phase in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let a = evaluate(&p, phase - 1e-6);
                let b = evaluate(&p, phase + 1e-6);
                assert!((a.moon_lux - b.moon_lux).abs() < 0.01);
                assert!((a.exposure_ev100 - b.exposure_ev100).abs() < 0.001);
            }
        }
        let mut disabled = p;
        disabled.night.enabled = false;
        assert_eq!(evaluate(&disabled, 0.0).moon_lux, 0.0);
        disabled.night.exposure_ev100 = f32::NAN;
        assert!(disabled.validate().is_err());
    }
    #[test]
    fn the_moon_lags_the_sun_by_its_age_and_lights_by_its_phase() {
        let p = AtmosphereProfile::default();
        let at_age = |age: f32, phase: f32| {
            let profile = AtmosphereProfile {
                night: NightLighting {
                    age_days: age,
                    ..p.night.clone()
                },
                ..p.clone()
            };
            evaluate(&profile, phase)
        };
        let dot = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
        // New: beside the sun, unlit and dark. Full: opposite, all lit, highest at midnight.
        // Ages are counted from midnight.
        let new = at_age(SYNODIC_MONTH_DAYS - 0.5, 0.5);
        assert!(dot(new.direction_to_moon, new.direction_to_sun) > 0.999);
        assert!(new.moon_lit < 1e-4 && new.moon_lux == 0.0);
        let full = at_age(SYNODIC_MONTH_DAYS / 2.0, 0.0);
        assert!(dot(full.direction_to_moon, full.direction_to_sun) < -0.999);
        assert!(full.moon_lit > 0.9999);
        assert!(
            (full.direction_to_moon[1].asin().to_degrees() - p.maximum_elevation_degrees).abs()
                < 0.01
        );
        // A half moon gives a third of a full moon's light, a thin crescent next to none.
        let quarter = at_age(SYNODIC_MONTH_DAYS / 4.0 - 0.875, 0.875);
        assert!((quarter.moon_lit - 0.5).abs() < 1e-3);
        let reference = MOON_TONE_ELEVATION_DEGREES.to_radians().sin();
        let height = (reference / quarter.direction_to_moon[1]).powf(MOON_HEIGHT_BALANCE);
        let ratio = quarter.moon_lux / p.night.illuminance_lux / height;
        assert!((0.3..0.4).contains(&ratio), "{ratio}");
        for (i, v) in quarter.moon_north.into_iter().enumerate() {
            assert!((v - full.moon_north[i]).abs() < 1e-6);
        }
        assert!(dot(quarter.moon_north, quarter.direction_to_moon).abs() < 1e-5);
        assert!(dot(quarter.moon_north, quarter.direction_to_sun).abs() < 1e-5);
        // No moonlight by day, even with the moon up.
        let day = at_age(SYNODIC_MONTH_DAYS / 4.0 - 0.6, 0.6);
        assert!(day.direction_to_moon[1] > 0.0 && day.moon_lux == 0.0);
        // Days move the moon: it rises later each day, about 50 minutes.
        let rise = |day: i32| {
            (0..2880)
                .map(|m| m as f32 / 2880.0)
                .find(|&phase| {
                    let a = evaluate_at(&p, day, phase).direction_to_moon[1];
                    let b = evaluate_at(&p, day, phase + 1.0 / 2880.0).direction_to_moon[1];
                    a < 0.0 && b >= 0.0
                })
                .unwrap()
        };
        let later = (rise(1) - rise(0)) * 24.0 * 60.0;
        assert!((45.0..55.0).contains(&later), "{later}");
        assert_eq!(
            evaluate_at(&p, 3, 0.2).moon_age_days,
            evaluate_at(&p, 2, 1.2).moon_age_days
        );
        // Moonlight changes gradually through the night, rising, setting and dawn included: no
        // more than a fifth of a full moon's light a minute.
        for day in 0..30 {
            for m in 0..1440 {
                let a = evaluate_at(&p, day, m as f32 / 1440.0).moon_lux;
                let b = evaluate_at(&p, day, (m + 1) as f32 / 1440.0).moon_lux;
                assert!(
                    (a - b).abs() < 0.2 * p.night.illuminance_lux,
                    "day {day} minute {m}: {a} {b}"
                );
            }
        }
    }
    #[test]
    fn twilight_turns_blue_and_the_sun_leaves_the_sky_by_its_end() {
        let p = AtmosphereProfile::default();
        let depression = |degrees: f32| {
            // Evening phase with the sun `degrees` below the horizon.
            let sine = -degrees.to_radians().sin() / p.maximum_elevation_degrees.to_radians().sin();
            evaluate(&p, 0.75 + sine.asin().abs() / std::f32::consts::TAU)
        };
        let night = linear_rgb(p.phases[0].ambient_srgb);
        let hue = |c: [f32; 3]| c.map(|v| v / luminance(c));
        // Blue while still light, paler than the night's blue; the night's own by its start.
        let sunset = hue(linear_rgb(p.phases[3].ambient_srgb));
        let twilight = depression(TWILIGHT_BLUE_DEGREES);
        let blue = hue(twilight.ambient_linear);
        assert!(twilight.night_weight < 0.5);
        assert!(blue[2] / blue[0] > sunset[2] / sunset[0]);
        assert!(blue[2] / blue[0] < hue(night)[2] / hue(night)[0]);
        for (a, b) in hue(depression(12.0).ambient_linear)
            .into_iter()
            .zip(hue(night))
        {
            assert!((a - b).abs() < 1e-3);
        }
        assert_eq!(depression(10.0).sun_lux, p.sun_lux);
        assert_eq!(depression(18.0).sun_lux, 0.0);
        // The sun's colour above the atmosphere never changes.
        assert_eq!(depression(-30.0).sun_linear, depression(-1.0).sun_linear);
    }
    #[test]
    fn cycle_wraps_and_sunrise_and_sunset_have_distinct_palettes() {
        let p = AtmosphereProfile::default();
        assert_eq!(p.validate(), Ok(()));
        let dawn = evaluate(&p, 0.25);
        let dusk = evaluate(&p, 0.75);
        assert!(dawn.direction_to_sun[1].abs() < 1e-6);
        assert!(dusk.direction_to_sun[1].abs() < 1e-6);
        assert_ne!(dawn.ambient_linear, dusk.ambient_linear);
        let before = evaluate(&p, 1.0 - 1e-6);
        let after = evaluate(&p, 1e-6);
        assert!((before.ambient_lux - after.ambient_lux).abs() < 0.01);
        assert_eq!(
            evaluate(&p, 0.34).ambient_linear,
            evaluate(&p, 1.34).ambient_linear
        );
        for i in 0..1000 {
            let e = evaluate(&p, i as f32 / 1000.0);
            let length: f32 = e.direction_to_sun.iter().map(|v| v * v).sum();
            assert!((length - 1.0).abs() < 1e-5);
        }
    }
    #[test]
    fn shallow_sun_paths_remain_continuous_at_noon_and_midnight() {
        for maximum in [5.0, 15.0, 60.0, 85.0] {
            let p = AtmosphereProfile {
                maximum_elevation_degrees: maximum,
                ..Default::default()
            };
            for phase in [0.0, 0.5] {
                let a = evaluate(&p, phase - 1e-5);
                let b = evaluate(&p, phase + 1e-5);
                for (a, b) in a.ambient_linear.into_iter().zip(b.ambient_linear) {
                    assert!((a - b).abs() < 1e-4);
                }
                assert!((a.sun_lux - b.sun_lux).abs() < 0.1);
            }
        }
    }

    #[test]
    fn morning_mist_burns_off_as_the_sun_climbs() {
        let p = AtmosphereProfile::default();
        let amount = p.fog.mist_amount;
        assert!((evaluate(&p, 0.25).mist - amount[1]).abs() < 1e-4);
        assert!((evaluate(&p, 0.5).mist - amount[2]).abs() < 1e-4);
        assert!((evaluate(&p, 0.75).mist - amount[3]).abs() < 1e-4);
        assert!((evaluate(&p, 0.0).mist - amount[0]).abs() < 1e-4);
        // The game's morning still holds part of the dawn mist.
        let morning = evaluate(&p, p.initial_phase).mist;
        assert!(
            morning > amount[2] + 0.2 && morning < amount[1],
            "{morning}"
        );
        for i in 0..1000 {
            let (a, b) = (i as f32 / 1000.0, (i as f32 + 1.0) / 1000.0);
            assert!((evaluate(&p, a).mist - evaluate(&p, b).mist).abs() < 0.02);
        }
    }

    #[test]
    fn rejects_nonfinite_and_out_of_range_source() {
        let mut p = AtmosphereProfile::default();
        p.visibility_metres = f32::NAN;
        assert!(p.validate().is_err());
        p = Default::default();
        p.sun_srgb[0] = 2.0;
        assert!(p.validate().is_err());
        p = Default::default();
        p.night.age_days = 31.0;
        assert!(p.validate().is_err());
    }
}
