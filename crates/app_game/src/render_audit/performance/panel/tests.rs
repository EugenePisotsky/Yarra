use super::*;
use crate::{launch::LaunchOptions, render_audit::PerformancePanelPlugin};
use engine::{GameInputEnabled, GamePointerInputBlocked};

fn panel_app() -> App {
    let mut app = App::new();
    app.init_resource::<LaunchOptions>()
        .init_resource::<Time>()
        .init_resource::<Time<Real>>()
        .init_resource::<bevy::diagnostic::FrameCount>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<Touches>()
        .init_resource::<bevy::input::mouse::AccumulatedMouseScroll>()
        .init_resource::<FramePacing>()
        .init_resource::<RuntimeSettings>()
        .init_resource::<GameInputEnabled>()
        .init_resource::<GamePointerInputBlocked>()
        .init_resource::<engine::StreamingStats>()
        .init_resource::<engine::TerrainLodStats>()
        .init_resource::<engine::AtmosphereState>()
        .init_resource::<vegetation_render::VegetationDiagnostics>()
        .add_plugins(PerformancePanelPlugin)
        .add_systems(
            Update,
            crate::runtime_settings::apply_input_lock
                .in_set(crate::runtime_settings::RuntimeSettingsApply),
        );
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
    app.update();
    app
}

fn tick(app: &mut App, seconds: f64) {
    app.world_mut()
        .resource_mut::<Time<Real>>()
        .advance_by(std::time::Duration::from_secs_f64(seconds));
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
}

fn click(app: &mut App, action: Action) {
    let button = app.world_mut().spawn((Interaction::Pressed, action)).id();
    tick(app, 0.01);
    app.world_mut().despawn(button);
}

#[test]
fn f1_and_escape_cancel_without_replacing_results_or_losing_prior_controls() {
    for key in [KeyCode::F1, KeyCode::Escape] {
        for locked in [false, true] {
            let mut app = panel_app();
            click(&mut app, Action::Capture(0));
            tick(&mut app, 2.1);
            tick(&mut app, 10.1);
            let previous =
                super::super::report::comparison(app.world().resource::<CaptureSession>());
            {
                let mut settings = app.world_mut().resource_mut::<RuntimeSettings>();
                settings.controls_locked = locked;
                settings.gpu_pass_timings = true;
            }
            click(&mut app, Action::Capture(0));
            assert!(!app.world().resource::<PanelState>().open);
            assert!(!app.world().resource::<GameInputEnabled>().0);
            assert!(!app.world().resource::<RuntimeSettings>().gpu_pass_timings);
            // Other controls and restore must not change settings mid-capture.
            let rate = app.world().resource::<FramePacing>().rate;
            let reset = app
                .world_mut()
                .spawn((Interaction::Pressed, Control::Reset))
                .id();
            let fps = app
                .world_mut()
                .spawn((Interaction::Pressed, Control::FrameRate))
                .id();
            click(&mut app, Action::Restore(0));
            app.world_mut().despawn(reset);
            app.world_mut().despawn(fps);
            assert!(app.world().resource::<CaptureSession>().recording());
            assert_eq!(app.world().resource::<FramePacing>().rate, rate);
            assert!(!app.world().resource::<RuntimeSettings>().gpu_pass_timings);
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
            tick(&mut app, 0.01);
            let session = app.world().resource::<CaptureSession>();
            assert!(!session.recording());
            assert!(session.message.contains("previous results kept"));
            let after = super::super::report::comparison(session);
            // The message changes; the completed result must be identical.
            assert_eq!(
                previous.split_once('\n').unwrap().1,
                after.split_once('\n').unwrap().1
            );
            assert!(app.world().resource::<PanelState>().open);
            assert_eq!(app.world().resource::<GameInputEnabled>().0, !locked);
            assert!(app.world().resource::<RuntimeSettings>().gpu_pass_timings);
        }
    }
}

#[test]
fn completion_and_automatic_abort_reopen_panel_and_restore_controls_in_the_same_frame() {
    for cancel in [false, true] {
        let mut app = panel_app();
        click(&mut app, Action::Capture(1));
        assert!(!app.world().resource::<PanelState>().open);
        assert!(!app.world().resource::<GameInputEnabled>().0);
        if cancel {
            app.world_mut()
                .resource_mut::<engine::StreamingStats>()
                .loading = 1;
            tick(&mut app, 8.1);
        } else {
            tick(&mut app, 2.1);
            tick(&mut app, 10.1);
        }
        let session = app.world().resource::<CaptureSession>();
        assert!(!session.recording());
        assert_eq!(session.slots[1].is_none(), cancel);
        assert!(app.world().resource::<PanelState>().open);
        assert!(app.world().resource::<GameInputEnabled>().0);
        // Closing the panel later must not replay an old completion notification.
        click(&mut app, Action::Toggle);
        tick(&mut app, 0.1);
        assert!(!app.world().resource::<PanelState>().open);
    }
}
