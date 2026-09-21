use super::*;

#[test]
fn live_rate_policy_keeps_clock_and_presentation_together() {
    let original = WinitSettings::game();
    for fps in [0, 60, 30, 120, 0, 60, 45, 0] {
        let request = FramePacing {
            rate: FrameRate::new(fps),
            ..default()
        };
        let policy = request.resolve(true);
        let events = policy.event_loop(&original);
        if fps == 0 {
            assert_eq!(policy.source, ClockSource::FollowDisplay);
            assert_eq!(policy.presentation_interval, Duration::ZERO);
            assert_eq!(events.focused_mode, original.focused_mode);
            assert_eq!(events.unfocused_mode, original.unfocused_mode);
        } else {
            assert_eq!(policy.source, ClockSource::Display);
            assert_eq!(
                policy.presentation_interval,
                Duration::from_secs_f64(1.0 / f64::from(fps))
            );
            assert_eq!(
                events.focused_mode,
                UpdateMode::Reactive {
                    wait: Duration::MAX,
                    react_to_device_events: false,
                    react_to_user_events: true,
                    react_to_window_events: false,
                }
            );
            assert_eq!(events.focused_mode, events.unfocused_mode);
        }
    }
}

#[test]
fn fallback_and_diagnostic_modes_do_not_add_a_second_limiter() {
    let original = WinitSettings::game();
    let request = FramePacing {
        rate: FrameRate::new(60),
        ..default()
    };
    for policy in [
        request.resolve(false),
        FramePacing {
            timer: true,
            ..request
        }
        .resolve(true),
    ] {
        assert_eq!(policy.source, ClockSource::Timer);
        assert_eq!(policy.presentation_interval, Duration::ZERO);
        assert_eq!(
            policy.event_loop(&original).focused_mode,
            UpdateMode::Reactive {
                wait: Duration::from_secs_f64(1.0 / 60.0),
                react_to_device_events: false,
                react_to_user_events: false,
                react_to_window_events: false,
            }
        );
    }
}

#[test]
fn runtime_changes_apply_once_and_restore_original_event_loop() {
    let mut app = App::new();
    let original = WinitSettings::game();
    app.insert_resource(FramePacing {
        timer: true,
        ..default()
    })
    .insert_resource(Controller {
        last: None,
        original_event_loop: original.clone(),
    })
    .add_systems(PostUpdate, apply_pending);
    for fps in [0, 60, 30, 120, 0, 60, 0] {
        set_fps(&mut app, fps);
        app.update();
        let expected = app.world().resource::<FramePacing>().resolve(false);
        assert_eq!(*app.world().resource::<AppliedPacing>(), expected);
        let expected_events = expected.event_loop(&original);
        let events = app.world().resource::<WinitSettings>();
        assert_eq!(events.focused_mode, expected_events.focused_mode);
        assert_eq!(events.unfocused_mode, expected_events.unfocused_mode);
        // Neither resources nor a native clock are recreated on unchanged frames.
        let tick = app
            .world()
            .get_resource_ref::<AppliedPacing>()
            .unwrap()
            .last_changed();
        app.update();
        assert_eq!(
            app.world()
                .get_resource_ref::<AppliedPacing>()
                .unwrap()
                .last_changed(),
            tick
        );
    }
}

#[test]
fn selector_cycles_presets_and_handles_custom_launch_rates() {
    let mut rate = FrameRate::default();
    for fps in [30, 60, 120, 0, 30] {
        rate = rate.next();
        assert_eq!(rate.fps(), fps);
    }
    assert_eq!(FrameRate::new(45).label(), "FPS limit: 45");
    assert_eq!(FrameRate::new(45).next(), FrameRate::default());
    for invalid in [1, 14, 241] {
        assert!(std::panic::catch_unwind(|| FrameRate::new(invalid)).is_err());
    }
}
