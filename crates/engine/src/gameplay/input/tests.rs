use super::*;
use crate::gameplay::camera::CAMERA_DEFAULT_DISTANCE;

#[test]
fn trackpad_orbit_is_independent_of_frame_batching() {
    let simulate = |fps: u32, pixels_per_event: f32| {
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin)
            .init_resource::<Time>()
            .init_resource::<GameInputEnabled>()
            .init_resource::<GamePointerInputBlocked>()
            .add_systems(Update, update_camera_controls);
        let camera = app
            .world_mut()
            .spawn((
                MainCamera,
                CameraRig {
                    yaw: 0.0,
                    target_yaw: 0.0,
                    distance: CAMERA_DEFAULT_DISTANCE,
                    target_distance: CAMERA_DEFAULT_DISTANCE,
                    pitch_offset: 0.0,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(1));
        for _ in 0..fps {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f64(1.0 / f64::from(fps)));
            // Identical 120 Hz native event stream, grouped into render frames.
            for _ in 0..120 / fps {
                app.world_mut().write_message(MouseWheel {
                    unit: MouseScrollUnit::Pixel,
                    phase: bevy::input::touch::TouchPhase::Moved,
                    x: pixels_per_event,
                    y: 0.0,
                    window: Entity::PLACEHOLDER,
                });
            }
            app.update();
        }
        let rig = app.world().get::<CameraRig>(camera).unwrap();
        (rig.yaw, rig.target_yaw)
    };
    for speed in [2.0, 60.0] {
        let a = simulate(60, speed);
        let b = simulate(120, speed);
        assert!(
            (a.1 - b.1).abs() < 0.0001,
            "gesture distance changed: {a:?} / {b:?}"
        );
        assert!(
            (a.0 - b.0).abs() < 0.0001,
            "smoothing changed with FPS: {a:?} / {b:?}"
        );
    }
}

#[test]
fn irregular_scroll_batches_are_consumed_once_and_account_for_clipping() {
    use bevy::input::touch::TouchPhase;
    let mut app = App::new();
    app.add_plugins(bevy::input::InputPlugin)
        .init_resource::<Time>()
        .init_resource::<GameInputEnabled>()
        .init_resource::<GamePointerInputBlocked>()
        .init_resource::<CameraInputDiagnostics>()
        .add_systems(Update, update_camera_controls);
    let camera = app
        .world_mut()
        .spawn((
            MainCamera,
            CameraRig {
                yaw: 0.0,
                target_yaw: 0.0,
                distance: CAMERA_DEFAULT_DISTANCE,
                target_distance: CAMERA_DEFAULT_DISTANCE,
                pitch_offset: 0.0,
            },
        ))
        .id();
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs(1));
    let mut expected_yaw = 0.0;
    let mut sent = 0;
    for (frame, batch_size) in [0, 1, 0, 4, 2, 0, 3, 0].into_iter().enumerate() {
        // Idle frames, uneven native batches, and an event above the current clamp.
        let mut raw_x = 0.0;
        let mut clipped = 0;
        for _ in 0..batch_size {
            let x = if sent == 5 {
                100.0_f32
            } else {
                2.0 + sent as f32
            };
            let phase = match sent % 3 {
                0 => TouchPhase::Started,
                1 => TouchPhase::Moved,
                _ => TouchPhase::Ended,
            };
            app.world_mut().write_message(MouseWheel {
                unit: MouseScrollUnit::Pixel,
                phase,
                x,
                y: 0.0,
                window: Entity::PLACEHOLDER,
            });
            raw_x += x;
            clipped += usize::from(x > 80.0);
            expected_yaw -= x.min(80.0) * CAMERA_TRACKPAD_ORBIT_SPEED;
            sent += 1;
        }
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(17));
        app.update();
        let d = app.world().resource::<CameraInputDiagnostics>();
        assert_eq!(d.sequence, frame as u64 + 1);
        assert_eq!(d.wheel_events, batch_size);
        assert_eq!(d.pixel_delta.x, raw_x);
        assert_eq!(d.clipped_events, clipped);
        assert_eq!(d.requested_orbit, d.applied_orbit);
        assert!(
            (app.world().get::<CameraRig>(camera).unwrap().target_yaw - expected_yaw).abs() < 1e-6
        );
    }
    assert_eq!(sent, 10);
}

#[test]
fn camera_gestures_work_and_are_not_replayed_after_ui_capture_or_unlock() {
    let mut app = App::new();
    app.add_plugins(bevy::input::InputPlugin)
        .init_resource::<Time>()
        .init_resource::<GameInputEnabled>()
        .init_resource::<GamePointerInputBlocked>()
        .init_resource::<CameraInputDiagnostics>()
        .add_systems(Update, update_camera_controls);
    let camera = app
        .world_mut()
        .spawn((
            MainCamera,
            CameraRig {
                yaw: 0.0,
                target_yaw: 0.0,
                distance: CAMERA_DEFAULT_DISTANCE,
                target_distance: CAMERA_DEFAULT_DISTANCE,
                pitch_offset: 0.0,
            },
        ))
        .id();
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs(1));
    let gesture = |app: &mut App| {
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Pixel,
            phase: bevy::input::touch::TouchPhase::Moved,
            x: 20.0,
            y: 10.0,
            window: Entity::PLACEHOLDER,
        });
        app.world_mut()
            .write_message(PanGesture(Vec2::new(20.0, 0.0)));
        app.world_mut().write_message(PinchGesture(0.1));
        app.update();
    };
    app.world_mut().resource_mut::<GameInputEnabled>().0 = false;
    gesture(&mut app);
    let d = app.world().resource::<CameraInputDiagnostics>();
    assert_eq!(d.wheel_events, 1);
    assert!(!d.enabled);
    assert_ne!(d.requested_orbit, 0.0);
    assert_eq!(d.applied_orbit, 0.0);
    app.world_mut().resource_mut::<GameInputEnabled>().0 = true;
    app.update();
    assert_eq!(
        app.world().get::<CameraRig>(camera).unwrap().target_yaw,
        0.0
    );
    app.world_mut().resource_mut::<GamePointerInputBlocked>().0 = true;
    gesture(&mut app);
    let d = app.world().resource::<CameraInputDiagnostics>();
    assert_eq!(d.wheel_events, 1);
    assert!(d.pointer_blocked);
    assert_eq!(d.applied_orbit, 0.0);
    app.world_mut().resource_mut::<GamePointerInputBlocked>().0 = false;
    app.update();
    let rig = app.world().get::<CameraRig>(camera).unwrap();
    assert_eq!(rig.target_yaw, 0.0);
    assert_eq!(rig.target_distance, CAMERA_DEFAULT_DISTANCE);
    gesture(&mut app);
    let rig = app.world().get::<CameraRig>(camera).unwrap();
    assert!(rig.target_yaw < 0.0, "uncaptured pan must orbit");
    assert!(
        rig.target_distance < CAMERA_DEFAULT_DISTANCE,
        "uncaptured pinch must zoom"
    );
}
