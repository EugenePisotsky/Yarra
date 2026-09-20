//! Renderer-independent settings for the first, ground-view cloud layer.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudSettings {
    pub enabled: bool,
    pub coverage: f32,
    pub density: f32,
    pub base_metres: f32,
    pub thickness_metres: f32,
    pub size_metres: f32,
    pub erosion: f32,
    pub wind_degrees: f32,
    pub wind_metres_per_second: f32,
    pub seed: u32,
}
impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            coverage: 0.48,
            density: 0.8,
            base_metres: 1200.,
            thickness_metres: 650.,
            size_metres: 1800.,
            erosion: 0.3,
            wind_degrees: 35.,
            wind_metres_per_second: 12.,
            seed: 7,
        }
    }
}
impl CloudSettings {
    pub fn scattered() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }
    pub fn overcast() -> Self {
        Self {
            enabled: true,
            coverage: 0.92,
            density: 1.2,
            thickness_metres: 450.,
            erosion: 0.15,
            ..Self::default()
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        let valid = [
            (self.coverage, 0., 1.),
            (self.density, 0., 3.),
            (self.base_metres, 300., 6000.),
            (self.thickness_metres, 100., 3000.),
            (self.size_metres, 300., 6000.),
            (self.erosion, 0., 1.),
            (self.wind_degrees, -180., 180.),
            (self.wind_metres_per_second, 0., 100.),
        ]
        .iter()
        .all(|&(v, lo, hi)| v.is_finite() && v >= lo && v <= hi);
        if valid {
            Ok(())
        } else {
            Err("Cloud settings contain an invalid physical range")
        }
    }
    pub fn period_metres(&self) -> f64 {
        f64::from(self.size_metres) * 16.
    }
    /// Wrapping in f64 preserves detail through origin rebases and long sessions.
    pub fn wrapped_origin(&self, origin: [f64; 2]) -> [f32; 2] {
        origin.map(|v| v.rem_euclid(self.period_metres()) as f32)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_and_rebasing() {
        let mut p = CloudSettings::scattered();
        assert!(p.validate().is_ok());
        let origin = [1_000_000_032., -1_000_000_032.];
        let a = p.wrapped_origin(origin);
        let b = p.wrapped_origin([origin[0] + 32., origin[1] - 32.]);
        let period = p.period_metres() as f32;
        assert!(((a[0] + 32.).rem_euclid(period) - b[0]).abs() < 0.001);
        assert!(((a[1] - 32.).rem_euclid(period) - b[1]).abs() < 0.001);
        p.coverage = f32::NAN;
        assert!(p.validate().is_err());
    }
}
