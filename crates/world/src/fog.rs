//! Renderer-independent settings for the low air of every weather: ground haze that thickens
//! towards the sea, and mist that pools in valleys. Weather adds its own reduced-visibility fog
//! in front (`weather::WeatherParams::fog`).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FogSettings {
    pub enabled: bool,
    /// Visibility in the ground haze at sea level, where 2% contrast remains.
    pub haze_visibility_metres: f32,
    /// Height over which the haze thins by a factor of e.
    pub haze_height_metres: f32,
    /// Visibility inside full valley mist.
    pub mist_visibility_metres: f32,
    /// How high mist reaches above a valley floor; open ground holds a third of that, and a
    /// fifth of the density.
    pub mist_depth_metres: f32,
    /// Mist amount, 0..1, at night, sunrise, high sun and sunset (`atmosphere::PHASE_NAMES`).
    /// Morning mist burns off as the sun climbs (see `atmosphere::evaluate`).
    pub mist_amount: [f32; 4],
    /// Amount added on fully wet ground after rain.
    pub mist_after_rain: f32,
    /// Visibility in the humid air under forest crowns, which sunbeams light up. Only the light
    /// shafts near the camera draw it; it thickens with the morning mist.
    pub canopy_air_visibility_metres: f32,
}

impl Default for FogSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            haze_visibility_metres: 30_000.,
            haze_height_metres: 250.,
            mist_visibility_metres: 150.,
            mist_depth_metres: 40.,
            mist_amount: [0.7, 1.0, 0.1, 0.3],
            mist_after_rain: 0.4,
            canopy_air_visibility_metres: 500.,
        }
    }
}

impl FogSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        let valid = [
            (self.haze_visibility_metres, 200., 200_000.),
            (self.haze_height_metres, 10., 2000.),
            (self.mist_visibility_metres, 20., 5000.),
            (self.mist_depth_metres, 0., 300.),
            (self.mist_after_rain, 0., 1.),
            (self.canopy_air_visibility_metres, 100., 100_000.),
        ]
        .into_iter()
        .chain(self.mist_amount.map(|a| (a, 0., 1.)))
        .all(|(v, lo, hi)| v.is_finite() && (lo..=hi).contains(&v));
        if valid {
            Ok(())
        } else {
            Err("Fog settings contain an invalid visibility, height or amount")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_ranges_are_checked() {
        assert_eq!(FogSettings::default().validate(), Ok(()));
        let mut fog = FogSettings::default();
        fog.mist_amount[1] = 1.5;
        assert!(fog.validate().is_err());
        fog = FogSettings {
            haze_height_metres: f32::NAN,
            ..Default::default()
        };
        assert!(fog.validate().is_err());
    }
}
