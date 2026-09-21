use super::super::capture::Capture;
use super::*;
use crate::{frame_pacing::FrameRate, runtime_settings::RuntimeSettings};
use bevy::prelude::*;

#[test]
fn percentiles_use_frame_intervals_and_ignore_invalid_samples() {
    let s = stats([8.0, 8.0, 10.0, 30.0, f64::NAN, 0.0]);
    assert_eq!(s.median, 8.0);
    assert_eq!(s.p95, 30.0);
    assert_eq!(s.mean, 14.0);
    assert_eq!(duration_stats([0.0, 0.0, 2.0, f64::NAN]).median, 0.0);
}
#[test]
fn comparison_surfaces_multiple_changes() {
    let mut capture = Capture {
        settings: RuntimeSettings::default(),
        frame_rate: default(),
        frames: vec![8.0; 10],
        app_times: vec![2.0; 10],
        thermal_start: "nominal".into(),
        thermal_end: "nominal".into(),
        context: String::new(),
        upscaler: None,
        camera: Mat4::IDENTITY,
        phase: 0.5,
        viewport: UVec2::ZERO,
        warnings: vec![],
        gpu: vec![],
        cpu: vec![],
    };
    capture.gpu = (1..=5)
        .map(|frame| timing::GpuSample {
            stamp: timing::Stamp { frame, ..default() },
            elapsed_ms: 1.5,
            scopes: vec![],
            invalid_scopes: 0,
        })
        .collect();
    let mut b = capture.clone();
    for sample in &mut b.gpu {
        sample.elapsed_ms = 2.2;
    }
    b.settings.clouds = engine::CloudQuality::Off;
    b.frame_rate = FrameRate::new(60);
    b.settings.hide_objects = true;
    b.thermal_start = "fair".into();
    let mut state = CaptureSession::default();
    state.slots = [Some(capture), Some(b)];
    let report = comparison(&state);
    assert!(report.contains("Clouds:"));
    assert!(report.contains("FPS limit: Follow display -> FPS limit: 60"));
    assert!(report.contains("Object draws:"));
    assert!(report.contains("Conditions differ"));
    assert!(report.contains("B - A GPU median: +0.70 ms"));
    assert!(report.contains("B - A median: +0.00 ms"));
}

#[test]
fn export_requires_a_completed_capture() {
    let error = export(&CaptureSession::default()).unwrap_err();
    assert_eq!(error.to_string(), "Capture A or B first.");
}
