use super::*;

// Evaluate the shader's phase terms to test the CPU/GPU coordinate contract.
fn phases_at(pose: WindPose, local: [f32; 2]) -> [f32; 4] {
    let [x, z, frequency, _] = pose.field;
    let broad = (local[0] * x + local[1] * z) * frequency;
    let cross = (-local[0] * z + local[1] * x) * frequency * 0.71;
    [
        broad + pose.phases[0],
        cross + pose.phases[1],
        broad * 0.43 - cross * 0.61 + pose.phases[2],
        local[0] * 1.91 - local[1] * 1.37 + pose.phases[3],
    ]
}
fn force(pose: WindPose, local: [f32; 2]) -> f32 {
    let [broad, cross, gust, _] = phases_at(pose, local);
    let amount = (broad + cross.sin() * 0.85).sin() * 0.5 + 0.5;
    let rise = (((gust.sin() * 0.5 + 0.5) - 0.28) / 0.72).clamp(0., 1.);
    pose.field[3]
        * (0.25 + amount * 0.18 + rise * rise * (3. - 2. * rise) * pose.response[0] * 0.90)
            .clamp(0.12, 1.30)
}
#[test]
fn tree_and_grass_share_direction_force_and_disabled_state() {
    let mut wind = VegetationWind::default();
    wind.set_phase_seconds(17.4);
    for point in [[0., 0.], [-43., 61.], [160., -96.]] {
        let pose = WindPose::sample(&wind, TreeWindResponse::default(), [0.; 2]);
        assert!((force(pose, point) - wind.sample_force(Vec2::from_array(point))).abs() < 0.00002);
    }
    wind.enabled = false;
    assert_eq!(
        WindPose::sample(&wind, TreeWindResponse::default(), [0.; 2]).field[3],
        0.
    );
}
#[test]
fn rebase_preserves_every_wave_including_fractional_gust_at_large_coordinates() {
    let mut wind = VegetationWind::default();
    wind.set_phase_seconds(123.5);
    for origin in [[0., 0.], [8_000_000., -3_000_000.]] {
        let shifted = [origin[0] + 256., origin[1] - 256.];
        let before = WindPose::sample(&wind, TreeWindResponse::default(), origin);
        let after = WindPose::sample(&wind, TreeWindResponse::default(), shifted);
        let a = phases_at(before, [270., -240.]);
        let b = phases_at(after, [14., 16.]);
        for (a, b) in a.into_iter().zip(b) {
            assert!((a.sin() - b.sin()).abs() < 0.0002, "{a} / {b}");
        }
    }
}
#[test]
fn externally_driven_pause_is_exact_and_bad_settings_stay_bounded() {
    let mut wind = VegetationWind::default();
    wind.externally_driven = true;
    wind.set_phase_seconds(70.);
    let a = WindPose::sample(&wind, TreeWindResponse::default(), [0.; 2]);
    let b = WindPose::sample(&wind, TreeWindResponse::default(), [0.; 2]);
    assert_eq!(a, b);
    wind.direction = Vec2::splat(f32::NAN);
    wind.strength = f32::INFINITY;
    let pose = WindPose::sample(
        &wind,
        TreeWindResponse {
            branch_amplitude: 99.,
            flutter_amplitude: -1.,
        },
        [0.; 2],
    );
    assert!(
        pose.field
            .iter()
            .chain(pose.phases.iter())
            .chain(pose.response.iter())
            .all(|v| v.is_finite())
    );
    assert_eq!(pose.field[3], 0.);
    assert_eq!(pose.response[1], 0.5);
    assert_eq!(pose.response[2], 0.);
    assert_eq!(std::mem::size_of::<WindFrames>(), 96);
}
