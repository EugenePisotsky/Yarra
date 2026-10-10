//! Session-only F1 experiments; weather remains the normal source of shared wind.
use bevy::prelude::*;
use vegetation_render::VegetationWind;

#[derive(Resource, Clone, Debug)]
pub struct TreeWindTuning {
    pub manual: bool,
    pub strength: f32,
    pub gustiness: f32,
    pub heading: f32,
    pub sway: f32,
    pub branches: f32,
    pub flutter: f32,
    pub rhythm: f32,
}
impl Default for TreeWindTuning {
    fn default() -> Self {
        Self {
            manual: false,
            strength: 0.82,
            gustiness: 0.95,
            heading: 22.4,
            sway: 1.,
            branches: 1.,
            flutter: 1.,
            rhythm: 1.,
        }
    }
}
#[derive(Resource, Default)]
pub(super) struct WindTuningState {
    saved: Option<(f32, f32, Vec2)>,
    manual_pose: Option<(f32, f32, Vec2)>,
}
impl WindTuningState {
    // Restore the automatic input before weather updates it. Keep the smoothed
    // manual output separately so changing weather cannot poison its baseline.
    fn restore(&mut self, wind: &mut VegetationWind) {
        if let Some((strength, gustiness, direction)) = self.saved.take()
            && !wind.externally_driven
        {
            wind.strength = strength;
            wind.gustiness = gustiness;
            wind.direction = direction;
        }
    }
    pub fn apply(&mut self, wind: &mut VegetationWind, tuning: &TreeWindTuning, dt: f32) {
        if wind.externally_driven {
            self.manual_pose = None;
            return;
        }
        if tuning.manual {
            self.saved = Some((wind.strength, wind.gustiness, wind.direction));
            let (strength, gustiness, direction) =
                *self.manual_pose.get_or_insert(self.saved.unwrap());
            let blend = 1. - (-dt.clamp(0., 0.1) / 0.65).exp();
            wind.strength = strength + (tuning.strength.clamp(0., 2.) - strength) * blend;
            wind.gustiness = gustiness + (tuning.gustiness.clamp(0., 1.) - gustiness) * blend;
            let angle = tuning.heading.to_radians();
            let from = direction.y.atan2(direction.x);
            let delta = (angle - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            let angle = from + delta * blend;
            wind.direction = Vec2::new(angle.cos(), angle.sin());
            self.manual_pose = Some((wind.strength, wind.gustiness, wind.direction));
        } else {
            self.manual_pose = None;
        }
    }
}
pub(super) fn restore_automatic(
    wind: Option<ResMut<VegetationWind>>,
    mut state: ResMut<WindTuningState>,
) {
    if let Some(mut wind) = wind {
        state.restore(&mut wind);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_override_settles_and_restores_without_resetting_transport() {
        let mut wind = VegetationWind::default();
        wind.set_phase_seconds(42.);
        let original = wind;
        let mut state = WindTuningState::default();
        let mut tuning = TreeWindTuning {
            manual: true,
            strength: 0.,
            ..default()
        };
        for _ in 0..240 {
            state.restore(&mut wind);
            state.apply(&mut wind, &tuning, 1. / 60.);
        }
        assert!(wind.strength < 0.003);
        assert_eq!(wind.phase_seconds(), 42.);
        tuning.manual = false;
        state.restore(&mut wind);
        state.apply(&mut wind, &tuning, 1. / 60.);
        assert_eq!(wind.strength, original.strength);
        assert_eq!(wind.direction, original.direction);
        wind.externally_driven = true;
        tuning.manual = true;
        state.apply(&mut wind, &tuning, 1.);
        assert_eq!(wind.strength, original.strength);
    }
    #[test]
    fn weather_changes_under_manual_wind_and_opposite_headings_work() {
        let mut wind = VegetationWind::default();
        wind.direction = Vec2::X;
        let mut state = WindTuningState::default();
        let mut tuning = TreeWindTuning {
            manual: true,
            heading: 180.,
            ..default()
        };
        for _ in 0..300 {
            state.restore(&mut wind);
            wind.strength = 1.4; // Automatic producer keeps changing below the override.
            state.apply(&mut wind, &tuning, 1. / 60.);
        }
        assert!(wind.direction.x < -0.99);
        tuning.manual = false;
        state.restore(&mut wind);
        state.apply(&mut wind, &tuning, 1. / 60.);
        assert_eq!(wind.strength, 1.4);
    }
}
