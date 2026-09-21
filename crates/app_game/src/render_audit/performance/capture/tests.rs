use super::super::report;
use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn recording_app() -> App {
    let mut app = App::new();
    app.init_resource::<Time<Real>>()
        .init_resource::<FramePacing>()
        .init_resource::<Telemetry>()
        .init_resource::<RuntimeSettings>()
        .init_resource::<PendingPipelines>()
        .insert_resource(timing::History {
            enabled: true,
            gpu_disabled: false,
            ..default()
        })
        .init_resource::<timing::Stamp>()
        .init_resource::<engine::StreamingStats>()
        .init_resource::<engine::TerrainLodStats>()
        .init_resource::<engine::AtmosphereState>()
        .init_resource::<CaptureSession>()
        .add_systems(Update, sample.map(drop));
    app.world_mut().spawn((
        Window {
            focused: true,
            ..default()
        },
        PrimaryWindow,
    ));
    app.world_mut().spawn((
        Camera::default(),
        GlobalTransform::default(),
        WorldViewCamera,
    ));
    app.world_mut().resource_mut::<FramePacing>().rate = FrameRate::new(60);
    app.world_mut()
        .resource_mut::<RuntimeSettings>()
        .gpu_pass_timings = true;
    app.world_mut()
        .resource_scope(|world, mut session: Mut<CaptureSession>| {
            assert!(session.start(
                0,
                &mut world.resource_mut::<RuntimeSettings>(),
                FrameRate::new(60),
                0.0
            ));
        });
    app
}
fn advance(app: &mut App, seconds: f64) {
    app.world_mut()
        .resource_mut::<Time<Real>>()
        .advance_by(std::time::Duration::from_secs_f64(seconds));
    app.update();
}
#[test]
fn capture_without_probes_keeps_frame_data_and_explains_missing_timings() {
    let mut app = recording_app();
    *app.world_mut().resource_mut::<timing::History>() = default();
    advance(&mut app, 2.1);
    advance(&mut app, SAMPLE_SECONDS + 0.1);
    let panel = app.world().resource::<CaptureSession>();
    let capture = panel.slots[0].as_ref().unwrap();
    assert!(!capture.frames.is_empty());
    assert!(capture.gpu.is_empty() && capture.cpu.is_empty());
    assert!(
        capture
            .warnings
            .iter()
            .any(|w| w.contains("disabled at launch"))
    );
    assert!(capture.context.contains("Timing instrumentation:"));
    assert!(panel.message.contains("Frame/app timings only"));
    let history = app.world().resource::<timing::History>();
    assert!(report::timing_details(history, default()).contains("disabled"));
}

#[test]
fn capture_rejects_an_external_fps_change() {
    let mut app = recording_app();
    advance(&mut app, 2.1);
    app.world_mut().resource_mut::<FramePacing>().rate = FrameRate::new(30);
    advance(&mut app, 0.1);
    let panel = app.world().resource::<CaptureSession>();
    assert!(!panel.recording());
    assert!(panel.slots[0].is_none());
    assert!(panel.message.contains("FPS limit changed"));
    assert!(!app.world().resource::<RuntimeSettings>().controls_locked);
}

#[test]
fn capture_waits_for_upscaling_and_flags_a_backend_change() {
    let mut app = recording_app();
    let camera = app
        .world_mut()
        .query_filtered::<Entity, With<WorldViewCamera>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .entity_mut(camera)
        .insert(upscaling::UpscaleStatus::default());
    advance(&mut app, 2.1);
    assert!(
        app.world()
            .resource::<CaptureSession>()
            .active
            .as_ref()
            .unwrap()
            .sampling_since
            .is_none()
    );
    app.world_mut()
        .get_mut::<upscaling::UpscaleStatus>(camera)
        .unwrap()
        .active = Some(upscaling::UpscaleMethod::Linear);
    advance(&mut app, 0.1);
    assert!(
        app.world()
            .resource::<CaptureSession>()
            .active
            .as_ref()
            .unwrap()
            .sampling_since
            .is_some()
    );
    app.world_mut()
        .get_mut::<upscaling::UpscaleStatus>(camera)
        .unwrap()
        .active = Some(upscaling::UpscaleMethod::MetalFxSpatial);
    advance(&mut app, 0.1);
    let recording = app
        .world()
        .resource::<CaptureSession>()
        .active
        .as_ref()
        .unwrap();
    assert!(
        recording
            .capture
            .warnings
            .iter()
            .any(|w| w.contains("upscaler"))
    );
    assert_eq!(
        recording.capture.upscaler.as_ref().unwrap().active,
        Some(upscaling::UpscaleMethod::Linear)
    );
}

#[test]
fn capture_is_bounded_and_restores_input_lock() {
    let mut app = recording_app();
    advance(&mut app, 2.1);
    assert!(
        app.world()
            .resource::<CaptureSession>()
            .active
            .as_ref()
            .unwrap()
            .sampling_since
            .is_some()
    );
    advance(&mut app, 5.0);
    assert!(app.world().resource::<CaptureSession>().recording());
    advance(&mut app, 5.1);
    let panel = app.world().resource::<CaptureSession>();
    assert!(!panel.recording());
    assert_eq!(panel.slots[0].as_ref().unwrap().frames.len(), 2);
    assert!(!app.world().resource::<RuntimeSettings>().controls_locked);
    assert!(app.world().resource::<RuntimeSettings>().gpu_pass_timings);
}
#[test]
fn capture_aborts_if_loading_never_settles_or_focus_is_lost() {
    let mut app = recording_app();
    app.world_mut()
        .resource_mut::<engine::StreamingStats>()
        .loading = 1;
    advance(&mut app, MAX_SETTLE_SECONDS + 0.1);
    assert!(!app.world().resource::<CaptureSession>().recording());
    assert!(app.world().resource::<CaptureSession>().slots[0].is_none());
    assert!(!app.world().resource::<RuntimeSettings>().controls_locked);
    assert!(app.world().resource::<RuntimeSettings>().gpu_pass_timings);
    let mut app = recording_app();
    advance(&mut app, 2.1);
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .unwrap();
    app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
    advance(&mut app, 0.1);
    assert!(!app.world().resource::<CaptureSession>().recording());
    assert!(
        app.world()
            .resource::<CaptureSession>()
            .message
            .contains("lost focus")
    );
    assert!(!app.world().resource::<RuntimeSettings>().controls_locked);
}

#[test]
fn restore_and_export_use_the_captured_configuration() {
    let mut app = recording_app();
    *app.world_mut().resource_mut::<timing::Stamp>() = timing::Stamp {
        frame: 100,
        epoch: 7,
        ..default()
    };
    advance(&mut app, 2.1);
    *app.world_mut().resource_mut::<timing::Stamp>() = timing::Stamp {
        frame: 200,
        epoch: 7,
        ..default()
    };
    for (frame, epoch, ms, detailed) in [
        (90, 7, 9.0, false),
        (102, 6, 99.0, false),
        (106, 7, 2.5, false),
        (120, 7, 200.0, true),
        (201, 7, 30.0, false),
    ] {
        app.world_mut()
            .resource_mut::<timing::History>()
            .gpu
            .push_back(timing::GpuSample {
                stamp: timing::Stamp {
                    frame,
                    epoch,
                    detailed,
                },
                elapsed_ms: ms,
                invalid_scopes: 0,
                scopes: vec![("cloud, \"test\"".into(), 0.5)],
            });
    }
    advance(&mut app, 10.1);
    {
        let mut settings = app.world_mut().resource_mut::<RuntimeSettings>();
        settings.hide_objects = true;
        settings.clouds = engine::CloudQuality::Off;
        settings.canopy.strength = 0.123;
        settings.terrain_macro = settings.terrain_macro.toggled();
        settings.page_gizmos = true;
    }
    app.world_mut().resource_mut::<FramePacing>().rate = FrameRate::new(120);
    app.world_mut()
        .resource_scope(|world, mut session: Mut<CaptureSession>| {
            world.resource_scope(|world, mut pacing: Mut<FramePacing>| {
                session.restore(
                    0,
                    &mut world.resource_mut::<RuntimeSettings>(),
                    &mut pacing,
                    12.5,
                );
            });
        });
    assert_eq!(app.world().resource::<FramePacing>().rate.fps(), 60);
    let settings = app.world().resource::<RuntimeSettings>();
    assert!(!settings.hide_objects);
    let captured = &app.world().resource::<CaptureSession>().slots[0]
        .as_ref()
        .unwrap()
        .settings;
    assert_eq!(settings.canopy, captured.canopy);
    assert_eq!(settings.terrain_macro, captured.terrain_macro);
    assert_eq!(settings.page_gizmos, captured.page_gizmos);
    assert_eq!(
        settings.clouds,
        app.world().resource::<CaptureSession>().slots[0]
            .as_ref()
            .unwrap()
            .settings
            .clouds
    );
    let root = std::env::temp_dir().join(format!(
        "yarra-performance-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let dir = report::export_to(app.world().resource::<CaptureSession>(), &root).unwrap();
    let report = std::fs::read_to_string(dir.join("report.txt")).unwrap();
    assert!(!report.contains("Settings changed:"));
    assert!(report.contains("3D viewport:"));
    assert!(report.contains("hide_objects: false"));
    let frames = std::fs::read_to_string(dir.join("A.csv")).unwrap();
    assert!(frames.starts_with("frame,frame_interval_ms,main_app_elapsed_ms"));
    assert_eq!(frames.lines().count(), 2);
    let gpu = std::fs::read_to_string(dir.join("A-gpu.csv")).unwrap();
    assert!(gpu.contains("106,7,2.500000,render_total"));
    assert!(gpu.contains("\"cloud, \"\"test\"\"\""));
    assert_eq!(
        gpu.lines().count(),
        3,
        "only normal, in-range samples from this settings epoch are captured"
    );
    assert!(dir.join("A-cpu.csv").exists());
    assert!(!dir.join("B.csv").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn overlapping_recordings_cannot_replace_the_snapshot_or_reset_its_controls() {
    let mut session = CaptureSession::default();
    let mut settings = RuntimeSettings {
        controls_locked: false,
        gpu_pass_timings: true,
        ..default()
    };
    let mut pacing = FramePacing::default();
    assert!(session.start(0, &mut settings, pacing.rate, 0.0));
    assert!(!session.start(1, &mut settings, pacing.rate, 1.0));
    session.reset(&mut settings, &mut pacing);
    session.restore(0, &mut settings, &mut pacing, 1.0);
    assert!(settings.controls_locked && !settings.gpu_pass_timings);
    assert_eq!(session.progress(1.0).unwrap().slot, 0);
    assert!(session.cancel(&mut settings, "Cancelled"));
    assert!(!settings.controls_locked && settings.gpu_pass_timings);
    assert!(!session.cancel(&mut settings, "Must not replace result"));
    assert_eq!(session.message, "Cancelled");
}

#[test]
fn capture_excludes_cpu_work_from_other_frames_and_changed_settings() {
    let mut app = recording_app();
    *app.world_mut().resource_mut::<timing::Stamp>() = timing::Stamp {
        frame: 100,
        epoch: 7,
        ..default()
    };
    advance(&mut app, 2.1);
    for (frame, epoch) in [(100, 7), (101, 7), (150, 8), (201, 7)] {
        app.world_mut()
            .resource_mut::<timing::History>()
            .cpu
            .push_back(timing::CpuSample {
                stamp: timing::Stamp {
                    frame,
                    epoch,
                    ..default()
                },
                render: false,
                work: [1.0; timing::KINDS],
                systems: vec![],
            });
    }
    *app.world_mut().resource_mut::<timing::Stamp>() = timing::Stamp {
        frame: 200,
        epoch: 8,
        ..default()
    };
    advance(&mut app, 10.1);
    let capture = app.world().resource::<CaptureSession>().slots[0]
        .as_ref()
        .unwrap();
    assert_eq!(capture.cpu.len(), 1);
    assert_eq!(capture.cpu[0].stamp.frame, 101);
    assert!(
        capture
            .warnings
            .iter()
            .any(|w| w.contains("Settings changed"))
    );
}
