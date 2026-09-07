//! Timed comparisons of the audit composite, UI drawing, and basic ground draw.
use super::{AuditRenderPath, AuditSettings, Scene};
use bevy::prelude::*;
use terrain_render::TerrainShadingMode;

const PHASE_SECONDS: f64 = 20.0;

struct Phase {
    name: &'static str,
    path: AuditRenderPath,
    ui: bool,
    scene: Scene,
}

const PHASES: [Phase; 8] = [
    Phase {
        name: "warmup",
        path: AuditRenderPath::Composite,
        ui: true,
        scene: Scene::Clear,
    },
    Phase {
        name: "clear_composite_ui",
        path: AuditRenderPath::Composite,
        ui: true,
        scene: Scene::Clear,
    },
    Phase {
        name: "clear_direct_ui",
        path: AuditRenderPath::Direct,
        ui: true,
        scene: Scene::Clear,
    },
    Phase {
        name: "clear_direct_no_ui",
        path: AuditRenderPath::Direct,
        ui: false,
        scene: Scene::Clear,
    },
    Phase {
        name: "flat_direct_no_ui",
        path: AuditRenderPath::Direct,
        ui: false,
        scene: Scene::Ground,
    },
    Phase {
        name: "clear_direct_no_ui_return",
        path: AuditRenderPath::Direct,
        ui: false,
        scene: Scene::Clear,
    },
    Phase {
        name: "clear_direct_ui_return",
        path: AuditRenderPath::Direct,
        ui: true,
        scene: Scene::Clear,
    },
    Phase {
        name: "clear_composite_ui_return",
        path: AuditRenderPath::Composite,
        ui: true,
        scene: Scene::Clear,
    },
];

#[derive(Resource, Default)]
pub(super) struct BaselineRun {
    pub(super) toggle_requested: bool,
    saved: Option<AuditSettings>,
    phase: usize,
    phase_started_s: f64,
}

pub(super) fn advance(
    mut run: ResMut<BaselineRun>,
    mut settings: ResMut<AuditSettings>,
    real_time: Res<Time<Real>>,
    game_time: Res<Time>,
) {
    let now = real_time.elapsed_secs_f64();
    if std::mem::take(&mut run.toggle_requested) {
        if let Some(saved) = run.saved.take() {
            *settings = saved;
            settings.changed_at = game_time.elapsed_secs_f64();
            warn!("RENDER_BASELINE event=cancel elapsed_s={now:.3}");
            return;
        }
        run.saved = Some(settings.clone());
        run.phase = 0;
        run.phase_started_s = now;
        apply_phase(&mut settings, &PHASES[0], game_time.elapsed_secs_f64());
        warn!("RENDER_BASELINE event=start elapsed_s={now:.3} duration_s=160");
        return;
    }
    if run.saved.is_none() || now - run.phase_started_s < PHASE_SECONDS {
        return;
    }
    run.phase += 1;
    // Do not skip measurements if a frame stalls or the app is backgrounded.
    run.phase_started_s = now;
    if let Some(phase) = PHASES.get(run.phase) {
        apply_phase(&mut settings, phase, game_time.elapsed_secs_f64());
    } else {
        *settings = run.saved.take().unwrap();
        settings.changed_at = game_time.elapsed_secs_f64();
        warn!("RENDER_BASELINE event=complete elapsed_s={now:.3} phases=8");
    }
}

fn apply_phase(settings: &mut AuditSettings, phase: &Phase, game_time: f64) {
    settings.scene = phase.scene;
    settings.ground_shading = TerrainShadingMode::Flat;
    settings.shadows = 2;
    settings.prepass = false;
    settings.msaa = Msaa::Off;
    settings.scale_index = 0;
    settings.render_path = phase.path;
    settings.show_ui = phase.ui;
    settings.controls_locked = true;
    settings.baseline_phase = Some(phase.name);
    settings.changed_at = game_time;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn baseline_advances_on_real_time_and_restores_settings_after_hidden_ui() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Time<Real>>()
            .init_resource::<BaselineRun>()
            .insert_resource(AuditSettings {
                scene: Scene::Ground,
                ground_shading: TerrainShadingMode::SingleTexture,
                prepass: true,
                msaa: Msaa::Sample4,
                scale_index: 2,
                render_path: AuditRenderPath::Composite,
                ..default()
            })
            .add_systems(Update, advance);
        app.world_mut()
            .resource_mut::<BaselineRun>()
            .toggle_requested = true;
        for phase in &PHASES {
            app.update();
            let s = app.world().resource::<AuditSettings>();
            assert_eq!(s.baseline_phase, Some(phase.name));
            assert_eq!(s.scene, phase.scene);
            assert_eq!(s.show_ui, phase.ui);
            assert!(s.controls_locked);
            assert!(!s.prepass);
            assert_eq!(s.msaa, Msaa::Off);
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .advance_by(Duration::from_secs(20));
        }
        app.update();
        let restored = app.world().resource::<AuditSettings>();
        assert_eq!(restored.baseline_phase, None);
        assert!(restored.show_ui);
        assert!(!restored.controls_locked);
        assert_eq!(restored.scene, Scene::Ground);
        assert_eq!(restored.ground_shading, TerrainShadingMode::SingleTexture);
        assert_eq!(restored.scale_index, 2);
        assert!(restored.prepass);
        assert_eq!(restored.msaa, Msaa::Sample4);
        assert!(app.world().resource::<BaselineRun>().saved.is_none());
    }

    #[test]
    fn cancelling_restores_the_saved_settings() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Time<Real>>()
            .init_resource::<BaselineRun>()
            .init_resource::<AuditSettings>()
            .add_systems(Update, advance);
        app.world_mut()
            .resource_mut::<BaselineRun>()
            .toggle_requested = true;
        app.update();
        app.world_mut()
            .resource_mut::<BaselineRun>()
            .toggle_requested = true;
        app.update();
        let s = app.world().resource::<AuditSettings>();
        assert_eq!(s.baseline_phase, None);
        assert_eq!(s.scene, Scene::Current);
        assert!(s.show_ui);
        assert!(!s.controls_locked);
    }
}
