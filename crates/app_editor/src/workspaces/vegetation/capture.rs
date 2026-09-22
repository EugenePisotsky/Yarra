//! Versioned study export and bounded viewport/editor capture.
use crate::{
    vegetation_authoring::VegetationAuthoringState,
    workspaces::vegetation::{
        ground, ground_treatment,
        stage::GroundMode,
        state::StudyState,
        study::{MAX_CANDIDATES, MAX_FIELD_CANDIDATES, PATCH_SIZE, StudyDocument, directory},
        viewport::GROUND_COLOR,
    },
};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use engine::{DEFAULT_CHARACTER_PRESENTATION_ID, WorldSun};
use vegetation_render::{
    VegetationDebugSettings, VegetationDiagnostics, VegetationLighting, VegetationSceneState,
};

fn document(
    state: &StudyState,
    catalog: &vegetation::VegetationCatalog,
    settings: VegetationDebugSettings,
    lighting: VegetationLighting,
    sun: (&Transform, &DirectionalLight),
    ambient: &GlobalAmbientLight,
) -> Option<StudyDocument> {
    let selected = state.signature.as_ref()?.population;
    let catalog = catalog.clone();
    let population = catalog.populations.get(selected)?.key.clone();
    Some(StudyDocument {
        version: 2,
        camera: state.camera.clone(),
        seed: state.seed,
        population,
        wind: state.wind.clone(),
        settings,
        lighting,
        catalog,
        reference: state.refs.name(),
        comparison: state.comparison.clone(),
        render_size: state.render_size,
        patch_size: state.field_size,
        msaa_samples: state.msaa_samples,
        exposure_ev100: 13.0,
        sun_rotation: sun.0.rotation.to_array(),
        sun_color: sun.1.color.to_srgba().to_f32_array(),
        sun_illuminance: sun.1.illuminance,
        ambient_color: ambient.color.to_srgba().to_f32_array(),
        ambient_brightness: ambient.brightness,
        ground_color: GROUND_COLOR,
        show_character: state.show_character,
        show_ruler: state.show_ruler,
        character: state.character.clone(),
        ground: state.ground,
        edge: state.edge,
        character_profile: DEFAULT_CHARACTER_PRESENTATION_ID.into(),
        build: format!(
            "yarra-app-editor {} · {}",
            env!("CARGO_PKG_VERSION"),
            std::env::var("YARRA_STUDY_BUILD")
                .unwrap_or_else(|_| "unidentified local build".into())
        ),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn capture(
    mut commands: Commands,
    mut state: ResMut<StudyState>,
    scene: Res<VegetationSceneState>,
    mut settings: ResMut<VegetationDebugSettings>,
    lighting: Res<VegetationLighting>,
    diagnostics: Res<VegetationDiagnostics>,
    render_diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    render_device: Option<Res<bevy::render::renderer::RenderDevice>>,
    authoring: Res<VegetationAuthoringState>,
    ground: Res<ground::StudyGroundAssets>,
    treatment: Res<ground_treatment::TreatmentAssets>,
    sun: Single<(&Transform, &DirectionalLight), With<WorldSun>>,
    ambient: Res<GlobalAmbientLight>,
    mut exit: MessageWriter<AppExit>,
) {
    if state.launch.exit && state.capture_finished {
        exit.write(if state.capture_failed {
            AppExit::error()
        } else {
            AppExit::Success
        });
        return;
    }
    if state.launch.exit && state.started.elapsed().as_secs() > 60 {
        error!(
            "Vegetation capture timed out: {}",
            state
                .error
                .as_deref()
                .unwrap_or("waiting for renderer/screenshot")
        );
        exit.write(AppExit::error());
        return;
    }
    if state.pending > 0
        || state.error.is_some()
        || (state.show_character && !state.character_ready)
        || (state.ground != GroundMode::Neutral && !ground.ready)
    {
        return;
    }
    if state.capture_dir.is_some() && !settings.gpu_counters_enabled {
        settings.gpu_counters_enabled = true;
        state.ready_frames = 0;
        return;
    }
    let Some((catalog, selected, revision)) = authoring.study_source() else {
        return;
    };
    if state.signature != Some(state.scene_signature(revision, selected)) {
        return;
    }
    let Some(doc) = document(&state, catalog, *settings, *lighting, *sun, &ambient) else {
        return;
    };
    if state.save_study {
        let path = directory().join("study.ron");
        state.message = match doc.write(&path) {
            Ok(()) => format!("Study saved: {}", path.display()),
            Err(e) => e,
        };
        state.save_study = false;
    }
    let stats = diagnostics.snapshot();
    if state.capture_dir.is_none()
        || state.ready_frames < if state.launch.profile { 360 } else { 65 }
        || stats.gpu_scene_revision != scene.revision()
        || stats.gpu_samples < 2
    {
        return;
    }
    let folder = state.capture_dir.take().unwrap();
    if state.launch.profile {
        let mut lines = vec![format!("Device features: {:?}", render_device.as_ref().map(|d| d.features())),
            "Recent render samples after 360 ready frames. elapsed_gpu is GPU ms; elapsed_cpu is CPU submission time. Missing GPU entries mean unsupported, not zero cost.".into()];
        for diagnostic in render_diagnostics.iter() {
            if diagnostic.path().as_str().starts_with("render/") {
                lines.push(format!(
                    "{}\t{}\t{:?}",
                    diagnostic.path(),
                    diagnostic.suffix,
                    diagnostic.values().copied().collect::<Vec<_>>()
                ));
            }
        }
        if let Err(error) = std::fs::create_dir_all(&folder)
            .and_then(|()| std::fs::write(folder.join("render-timings.txt"), lines.join("\n")))
        {
            state.message = error.to_string();
            state.capture_finished = true;
            state.capture_failed = true;
            return;
        }
    }
    if state.ground.treatment().is_some()
        && let Err(error) = treatment.write_diagnostics(&folder)
    {
        state.message = error;
        state.capture_finished = true;
        state.capture_failed = true;
        return;
    }
    if let Err(error) = doc.write(&folder.join("study.ron")) {
        state.message = error;
        state.capture_finished = true;
        state.capture_failed = true;
        return;
    }
    if let Err(error) = std::fs::write(
        folder.join("diagnostics.txt"),
        format!(
            "{stats:#?}\nShape inspection: {:?} (active comparisons use high topology; not a performance measurement)\nView opening disabled: {}\nField: {} x {} m\nGround: {}\nGrass edge: {:?}\nCandidate budget: {} / {}\nBlade bands: {:?}; source density factor: {:.3}\nGPU timings: see render-timings.txt when --profile is active; editor FPS is not a vegetation timing.\n",
            settings.shape_inspection,
            settings.inspection_disable_opening,
            state.field_size,
            state.field_size,
            state.ground.label(),
            state.edge,
            state.candidates,
            if state.field_size == PATCH_SIZE {
                MAX_CANDIDATES
            } else {
                MAX_FIELD_CANDIDATES
            },
            settings.blade_bands,
            settings.blade_band_density
        ),
    ) {
        state.message = error.to_string();
        state.capture_finished = true;
        state.capture_failed = true;
        return;
    }
    state.playing = false;
    state.pending = 2;
    state.capture_finished = false;
    state.capture_failed = false;
    state.message = format!("Capturing {}", folder.display());
    for (request, file) in [
        (Screenshot::image(state.image.clone()), "viewport.png"),
        (Screenshot::primary_window(), "editor.png"),
    ] {
        let path = folder.join(file);
        commands.spawn(request).observe(
            move |event: On<ScreenshotCaptured>, mut state: ResMut<StudyState>| {
                let saved = event
                    .image
                    .clone()
                    .try_into_dynamic()
                    .map_err(|e| e.to_string())
                    .and_then(|image| image.to_rgb8().save(&path).map_err(|e| e.to_string()));
                if let Err(e) = saved {
                    state.capture_failed = true;
                    state.message = format!("Capture failed: {e}");
                }
                state.pending = state.pending.saturating_sub(1);
                if state.pending == 0 {
                    state.capture_finished = true;
                    if !state.capture_failed {
                        state.message = format!("Captured {}", path.parent().unwrap().display());
                        info!("{}", state.message);
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::vegetation::{
        comparison::ComparisonView,
        stage::{CharacterPlacement, GrassEdge, GroundMode},
        state::StudyState,
        study::{StudyDocument, StudyLaunch, bounded_scene},
    };

    use vegetation_render::VegetationSceneState;

    #[test]
    fn reproduction_round_trip_validates_and_retains_exact_phase_and_catalog() {
        let mut state = StudyState::new(StudyLaunch::default());
        state.signature = Some(state.scene_signature(4, 3));
        state.wind.time = 17.625;
        state.edge = Some(GrassEdge::default());
        state.comparison = ComparisonView {
            zoom: 2.0,
            center: [0.6, 0.4],
        };
        let scene = VegetationSceneState::new(
            bounded_scene(&vegetation::fixtures::reference_catalog(), 3, state.seed)
                .unwrap()
                .0,
        )
        .unwrap();
        let doc = document(
            &state,
            &scene.scene().catalog,
            default(),
            default(),
            (&Transform::default(), &DirectionalLight::default()),
            &GlobalAmbientLight::default(),
        )
        .unwrap();
        let encoded = ron::ser::to_string(&doc).unwrap();
        let decoded: StudyDocument = ron::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded.wind.time, 17.625);
        assert_eq!(decoded.catalog, doc.catalog);
        assert_eq!(decoded.camera, doc.camera);
        assert_eq!(decoded.comparison, state.comparison);
        assert_eq!(decoded.character, state.character);
        assert_eq!(decoded.patch_size, state.field_size);
        assert_eq!(decoded.ground, GroundMode::Meadow);
        assert_eq!(decoded.edge, state.edge);
        let mut legacy = doc.clone();
        legacy.version = 1;
        legacy.render_size = [1280, 960];
        legacy.patch_size = 4.0;
        let field = format!(
            "comparison:{},",
            ron::ser::to_string(&legacy.comparison).unwrap()
        );
        let encoded = ron::ser::to_string(&legacy).unwrap();
        assert!(encoded.contains(&field));
        let mut encoded = encoded.replacen(&field, "", 1);
        for field in [
            format!(
                "character:{},",
                ron::ser::to_string(&legacy.character).unwrap()
            ),
            format!("ground:{},", ron::ser::to_string(&legacy.ground).unwrap()),
            format!("show_ruler:{},", legacy.show_ruler),
            format!("edge:{},", ron::ser::to_string(&legacy.edge).unwrap()),
        ] {
            assert!(encoded.contains(&field));
            encoded = encoded.replacen(&field, "", 1);
        }
        let old: StudyDocument = ron::from_str(&encoded).unwrap();
        old.validate().unwrap();
        assert_eq!(old.comparison, ComparisonView::default());
        assert_eq!(old.ground, GroundMode::Neutral);
        assert_eq!(old.character, CharacterPlacement::default());
        assert_eq!(old.edge, None);
        assert!(old.show_ruler);
        assert_eq!(
            StudyState::new(StudyLaunch {
                load: Some(old),
                ..default()
            })
            .render_size,
            [1280, 960]
        );
        let mut invalid = decoded;
        invalid.camera.pitch = f32::NAN;
        assert!(invalid.validate().is_err());
    }
}
