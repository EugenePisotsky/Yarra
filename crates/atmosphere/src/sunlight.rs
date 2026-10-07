//! Sunlight left after the atmosphere, for the effects that light air and clouds themselves:
//! haze, mist, light shafts, particles and the cloud layer. Bevy dims and reddens the light
//! reaching surfaces with its transmittance table; this integrates the same medium
//! (`ScatteringMedium::earth` with the near-ground aerosol layer set in `crate::apply`) along the
//! sun's direction, so a low sun lights the air as it lights the ground.
use bevy::math::Vec3;

const INNER_RADIUS: f64 = 6_360_000.0;
const OUTER_RADIUS: f64 = 6_460_000.0;
const RAYLEIGH: [f64; 3] = [5.802e-6, 13.558e-6, 33.100e-6];
const OZONE: [f64; 3] = [0.650e-6, 1.881e-6, 0.085e-6];
/// Falloff scales as shares of the atmosphere's height, as in the medium.
const RAYLEIGH_SCALE: f64 = 8.0 / 60.0;
const AEROSOL_SCALE: f64 = 1200.0 / 100_000.0;
const STEPS: usize = 64;

/// Bevy's exponential falloff at `altitude` metres (`Falloff::Exponential`).
fn exponential(altitude: f64, scale: f64) -> f64 {
    let p = 1.0 - altitude / (OUTER_RADIUS - INNER_RADIUS);
    let s = -1.0 / scale;
    (((1.0 - p) * s).exp() - s.exp()) / (1.0 - s.exp())
}

fn ozone(altitude: f64) -> f64 {
    let p = 1.0 - altitude / (OUTER_RADIUS - INNER_RADIUS);
    (1.0 - (p - 0.75).abs() / 0.15).max(0.0)
}

/// Share of the sun's light reaching `altitude` metres above the ground from a sun at
/// `elevation_sine`, per channel. The part of the disc (angular radius `disc_radius`) below the
/// horizon is lost; `visibility_metres` sets the aerosol near the ground.
pub fn sun_transmittance(
    molecular_density: f32,
    visibility_metres: f32,
    altitude: f32,
    elevation_sine: f32,
    disc_radius: f32,
) -> Vec3 {
    let r = INNER_RADIUS + f64::from(altitude.max(0.0));
    // Elevation of the horizon seen from this height, below zero.
    let horizon = -(1.0 - (INNER_RADIUS / r).powi(2)).max(0.0).sqrt();
    let mu = f64::from(elevation_sine.clamp(-1.0, 1.0));
    let radius = f64::from(disc_radius.max(1e-4));
    let edge = ((mu - horizon) / radius * 0.5 + 0.5).clamp(0.0, 1.0);
    let visible = edge * edge * (3.0 - 2.0 * edge);
    if visible <= 0.0 {
        return Vec3::ZERO;
    }
    // The upper half of a disc just at the horizon still passes over the ground.
    let mu = mu.max(horizon + 1e-6);
    let length = -r * mu + (r * r * (mu * mu - 1.0) + OUTER_RADIUS * OUTER_RADIUS).sqrt();
    let aerosol = 3.912 / f64::from(visibility_metres.clamp(10.0, 100_000.0));
    let mut depth = [0.0_f64; 3];
    // Steps crowd towards the start, where the air is densest.
    for i in 0..STEPS {
        let (a, b) = (i as f64 / STEPS as f64, (i + 1) as f64 / STEPS as f64);
        let t = length * (0.5 * (a + b)).powi(2);
        let dt = length * (b * b - a * a);
        let h = ((t * t + 2.0 * r * mu * t + r * r).sqrt() - INNER_RADIUS).max(0.0);
        let rayleigh = exponential(h, RAYLEIGH_SCALE) * f64::from(molecular_density);
        let haze = exponential(h, AEROSOL_SCALE) * aerosol;
        let o = ozone(h);
        for c in 0..3 {
            depth[c] += (RAYLEIGH[c] * rayleigh + haze + OZONE[c] * o) * dt;
        }
    }
    Vec3::new(
        (-depth[0]).exp() as f32,
        (-depth[1]).exp() as f32,
        (-depth[2]).exp() as f32,
    ) * visible as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISC: f32 = 0.0065;
    fn at(degrees: f32, altitude: f32) -> Vec3 {
        sun_transmittance(1.0, 20_000.0, altitude, degrees.to_radians().sin(), DISC)
    }

    #[test]
    fn a_low_sun_is_dimmer_and_redder_and_a_set_sun_gives_no_light() {
        let noon = at(60.0, 30.0);
        let morning = at(27.6, 30.0);
        let sunset = at(4.7, 30.0);
        // Clear vertical air: Rayleigh, a hazy summer aerosol layer and ozone.
        assert!(
            noon.x > 0.6 && noon.x < 0.8 && noon.z > 0.35 && noon.z < 0.55,
            "{noon}"
        );
        assert!(morning.y < noon.y && sunset.y < morning.y);
        let redness = |t: Vec3| t.x / t.z;
        assert!(redness(sunset) > 5.0 * redness(noon), "{sunset}");
        assert_eq!(at(-2.0, 30.0), Vec3::ZERO);
        // Half the disc at the horizon.
        let horizon = sun_transmittance(1.0, 20_000.0, 0.0, 0.0, DISC);
        assert!(horizon.x > 0.0 && horizon.x < at(0.5, 0.0).x);
    }

    #[test]
    fn clouds_above_the_haze_keep_their_light_after_the_ground_has_lost_it() {
        assert!(at(4.7, 2000.0).y > 5.0 * at(4.7, 30.0).y);
        // From 2 km the horizon lies 1.4 degrees down: clouds still see a sun that has set.
        assert_eq!(at(-1.0, 30.0), Vec3::ZERO);
        assert!(at(-1.0, 2000.0).x > 0.0);
        // Thinner haze lets more through.
        let clear = sun_transmittance(1.0, 80_000.0, 30.0, 0.08, DISC);
        assert!(clear.y > at(4.6, 30.0).y);
    }
}
