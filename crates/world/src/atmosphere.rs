//! Small, renderer-independent atmosphere definitions. Colors are explicitly sRGB;
//! evaluation interpolates them in linear light. Time is a normalized day phase.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightingPhase {
    pub sun_srgb: [f32; 3],
    pub sun_lux: f32,
    pub ambient_srgb: [f32; 3],
    pub ambient_lux: f32,
}

/// Art-directed full-moon lighting. Position is independent of the sun path;
/// orbital motion and lunar phases can be added without changing the day palette.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NightLighting {
    pub enabled: bool,
    pub azimuth_degrees: f32,
    pub elevation_degrees: f32,
    pub diameter_degrees: f32,
    pub light_srgb: [f32; 3],
    pub illuminance_lux: f32,
    pub exposure_ev100: f32,
}

impl Default for NightLighting {
    fn default() -> Self {
        Self {
            enabled: true,
            azimuth_degrees: -125.0,
            elevation_degrees: 12.0,
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
    pub exposure_ev100: f32,
    pub bloom_intensity: f32,
    /// Additional low-altitude aerosol visibility, independent of molecular scattering.
    pub visibility_metres: f32,
    pub haze_srgb: [f32; 3],
    pub molecular_density: f32,
    /// Night, Sunrise, Day, Sunset. Sunrise and sunset share an elevation, not a palette.
    pub phases: [LightingPhase; 4],
    #[serde(default)]
    pub night: NightLighting,
}

pub const PHASE_NAMES: [&str; 4] = ["Night", "Sunrise", "Day", "Sunset"];
// Low visible sun for inspection, just above the geometric horizon crossing.
pub const PHASE_TIMES: [f32; 4] = [0.0, 0.265, 0.5, 0.735];

impl Default for AtmosphereProfile {
    fn default() -> Self {
        Self {
            outdoor: true,
            initial_phase: 0.34,
            day_seconds: 1200.0,
            azimuth_degrees: 147.0,
            maximum_elevation_degrees: 60.0,
            sun_diameter_degrees: 0.75,
            exposure_ev100: 13.0,
            bloom_intensity: 0.15,
            visibility_metres: 20_000.0,
            haze_srgb: [0.93, 0.96, 1.0],
            molecular_density: 1.0,
            phases: [
                LightingPhase {
                    sun_srgb: [1.0; 3],
                    sun_lux: 0.0,
                    ambient_srgb: [0.25, 0.34, 0.55],
                    ambient_lux: 700.0,
                },
                LightingPhase {
                    sun_srgb: [1.0, 0.90, 0.72],
                    sun_lux: 100_000.0,
                    ambient_srgb: [0.54, 0.62, 0.82],
                    ambient_lux: 3000.0,
                },
                LightingPhase {
                    sun_srgb: [1.0, 0.98, 0.95],
                    sun_lux: 128_000.0,
                    ambient_srgb: [0.60, 0.72, 0.92],
                    ambient_lux: 6000.0,
                },
                LightingPhase {
                    sun_srgb: [1.0, 0.80, 0.60],
                    sun_lux: 100_000.0,
                    ambient_srgb: [0.64, 0.55, 0.78],
                    ambient_lux: 2500.0,
                },
            ],
            night: NightLighting::default(),
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
            || !range(self.night.azimuth_degrees, -180.0, 180.0)
            || !range(self.night.elevation_degrees, 5.0, 85.0)
            || !range(self.night.diameter_degrees, 0.1, 8.0)
            || !range(self.night.illuminance_lux, 0.0, 20_000.0)
            || !range(self.night.exposure_ev100, 0.0, 20.0)
            || !color(self.night.light_srgb)
            || self.phases.iter().any(|p| {
                !color(p.sun_srgb)
                    || !color(p.ambient_srgb)
                    || !range(p.sun_lux, 0.0, 200_000.0)
                    || !range(p.ambient_lux, 0.0, 20_000.0)
            })
        {
            return Err("Atmosphere settings contain an invalid color, time or physical range");
        }
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
    pub moon_linear: [f32; 3],
    pub moon_lux: f32,
    pub exposure_ev100: f32,
    pub night_weight: f32,
}

pub fn linear_rgb(srgb: [f32; 3]) -> [f32; 3] {
    srgb.map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    })
}

/// Caller supplies a validated profile and finite phase. No ECS, database or wall clock.
pub fn evaluate(profile: &AtmosphereProfile, phase: f32) -> EvaluatedAtmosphere {
    let phase = phase.rem_euclid(1.0);
    let orbit = (phase - 0.25) * std::f32::consts::TAU;
    let elevation = (orbit.sin() * profile.maximum_elevation_degrees.to_radians().sin()).asin();
    let azimuth = profile.azimuth_degrees.to_radians();
    // Tilted great-circle path: at noon the configured maximum elevation is reached.
    let x = orbit.cos();
    let y = orbit.sin() * profile.maximum_elevation_degrees.to_radians().sin();
    let z = orbit.sin() * profile.maximum_elevation_degrees.to_radians().cos();
    let direction_to_sun = [
        x * azimuth.cos() + z * azimuth.sin(),
        y,
        -x * azimuth.sin() + z * azimuth.cos(),
    ];
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
    let t = t * t * (3.0 - 2.0 * t);
    // Adapt only after sunset, reaching the authored night appearance by -12°.
    // Shallow sun paths still reach night at midnight and remain continuous.
    let night =
        (-elevation.to_degrees() / profile.maximum_elevation_degrees.min(12.0)).clamp(0.0, 1.0);
    let night = night * night * (3.0 - 2.0 * night);
    let moon_azimuth = profile.night.azimuth_degrees.to_radians();
    let moon_elevation = profile.night.elevation_degrees.to_radians();
    let a = &profile.phases[a];
    let b = &profile.phases[b];
    let mix = |a: f32, b: f32| a + (b - a) * t;
    let mix_color = |a: [f32; 3], b: [f32; 3]| {
        let a = linear_rgb(a);
        let b = linear_rgb(b);
        std::array::from_fn(|i| mix(a[i], b[i]))
    };
    EvaluatedAtmosphere {
        direction_to_sun,
        sun_linear: mix_color(a.sun_srgb, b.sun_srgb),
        sun_lux: mix(a.sun_lux, b.sun_lux),
        ambient_linear: mix_color(a.ambient_srgb, b.ambient_srgb),
        ambient_lux: mix(a.ambient_lux, b.ambient_lux),
        direction_to_moon: [
            moon_azimuth.sin() * moon_elevation.cos(),
            moon_elevation.sin(),
            moon_azimuth.cos() * moon_elevation.cos(),
        ],
        moon_linear: linear_rgb(profile.night.light_srgb),
        moon_lux: if profile.night.enabled {
            profile.night.illuminance_lux * night
        } else {
            0.0
        },
        exposure_ev100: profile.exposure_ev100
            + (profile.night.exposure_ev100 - profile.exposure_ev100) * night,
        night_weight: night,
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
        assert_eq!(night.moon_lux, p.night.illuminance_lux);
        assert_eq!(night.exposure_ev100, p.night.exposure_ev100);
        assert!(night.direction_to_moon[1] > 0.0);
        assert!((night.direction_to_moon.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5);
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
    fn cycle_wraps_and_sunrise_and_sunset_have_distinct_palettes() {
        let p = AtmosphereProfile::default();
        assert_eq!(p.validate(), Ok(()));
        let dawn = evaluate(&p, 0.25);
        let dusk = evaluate(&p, 0.75);
        assert!(dawn.direction_to_sun[1].abs() < 1e-6);
        assert!(dusk.direction_to_sun[1].abs() < 1e-6);
        assert_ne!(dawn.sun_linear, dusk.sun_linear);
        let before = evaluate(&p, 1.0 - 1e-6);
        let after = evaluate(&p, 1e-6);
        assert!((before.ambient_lux - after.ambient_lux).abs() < 0.01);
        assert_eq!(evaluate(&p, 0.34).sun_linear, evaluate(&p, 1.34).sun_linear);
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
                for (a, b) in a.sun_linear.into_iter().zip(b.sun_linear) {
                    assert!((a - b).abs() < 1e-4);
                }
                assert!((a.sun_lux - b.sun_lux).abs() < 0.1);
            }
        }
    }

    #[test]
    fn rejects_nonfinite_and_out_of_range_source() {
        let mut p = AtmosphereProfile::default();
        p.visibility_metres = f32::NAN;
        assert!(p.validate().is_err());
        p = Default::default();
        p.phases[1].sun_srgb[0] = 2.0;
        assert!(p.validate().is_err());
    }
}
