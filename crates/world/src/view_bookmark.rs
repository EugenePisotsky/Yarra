//! Optional launch/test viewpoints, independent of the authored world database.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldViewBookmark {
    /// World-space feet/focus position in the database's default world space.
    pub position: [f32; 3],
    pub yaw_degrees: f32,
    pub pitch_degrees: f32,
    pub distance: f32,
    pub fog_visibility: f32,
    /// Optional world-space route, sampled by distance rather than point count.
    #[serde(default)]
    pub route: Vec<[f32; 3]>,
}

impl WorldViewBookmark {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self
            .position
            .iter()
            .chain(self.route.iter().flatten())
            .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
            || !self.yaw_degrees.is_finite()
            || !(5.0..=80.0).contains(&self.pitch_degrees)
            || !(4.0..=24.0).contains(&self.distance)
            || !(10.0..=100_000.0).contains(&self.fog_visibility)
            || self.route.len() > 4096
        {
            return Err("invalid viewpoint coordinates, camera, fog or route");
        }
        Ok(())
    }

    pub fn route_position(&self, distance: f32) -> [f32; 3] {
        let mut previous = self.position;
        let mut remaining = distance.max(0.);
        for point in &self.route {
            let delta = std::array::from_fn::<_, 3, _>(|i| point[i] - previous[i]);
            let length = delta.iter().map(|v| v * v).sum::<f32>().sqrt();
            if length > 0. && remaining < length {
                return std::array::from_fn(|i| previous[i] + delta[i] * remaining / length);
            }
            remaining -= length;
            previous = *point;
        }
        previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_use_distance_and_stop_at_the_destination() {
        let mut view = WorldViewBookmark {
            position: [0.; 3],
            yaw_degrees: 45.,
            pitch_degrees: 20.,
            distance: 10.,
            fog_visibility: 2500.,
            route: vec![[0.; 3], [3., 4., 0.], [3., 4., 10.]],
        };
        assert_eq!(view.validate(), Ok(()));
        assert_eq!(view.route_position(2.5), [1.5, 2., 0.]);
        assert_eq!(view.route_position(10.), [3., 4., 5.]);
        assert_eq!(view.route_position(100.), [3., 4., 10.]);
        view.position[0] = f32::NAN;
        assert!(view.validate().is_err());
    }
}
