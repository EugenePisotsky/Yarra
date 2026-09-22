//! Study session, reference selection and saved world state.
use crate::workspaces::vegetation::{
    comparison::ComparisonView,
    references::References,
    stage::{CharacterPlacement, GrassEdge, GroundMode, ReferenceSetup},
    study::{HEIGHT, StudyCamera, StudyLaunch, WIDTH, WindStudy},
};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use bevy_egui::egui;
use std::{path::PathBuf, time::Instant};
use vegetation_render::{
    VegetationDebugSettings, VegetationLighting, VegetationSceneState, VegetationWind,
};

pub(super) struct RestoredWorld {
    pub(super) scene: VegetationSceneState,
    pub(super) settings: VegetationDebugSettings,
    pub(super) lighting: VegetationLighting,
    pub(super) wind: VegetationWind,
    pub(super) sun: (Entity, Transform, DirectionalLight, Option<RenderLayers>),
    pub(super) ambient: GlobalAmbientLight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SceneSignature {
    revision: u64,
    pub(super) population: usize,
    seed: u32,
    field_size: u32,
    edge: Option<[u32; 3]>,
}

#[derive(Resource)]
pub(super) struct StudyState {
    pub(super) launch: StudyLaunch,
    pub(super) refs: References,
    pub(super) camera: StudyCamera,
    pub(super) camera_label: String,
    pub(super) comparison: ComparisonView,
    pub(super) render_size: [u32; 2],
    pub(super) msaa_samples: u32,
    pub(super) seed: u32,
    pub(super) wind: WindStudy,
    pub(super) playing: bool,
    pub(super) playback_speed: f32,
    pub(super) restore: Option<RestoredWorld>,
    pub(super) own_settings: Option<(VegetationDebugSettings, VegetationLighting)>,
    pub(super) own_environment: Option<(Transform, DirectionalLight, GlobalAmbientLight)>,
    pub(super) image: Handle<Image>,
    pub(super) texture: egui::TextureId,
    pub(super) signature: Option<SceneSignature>,
    pub(super) candidates: u32,
    pub(super) ready_frames: u32,
    pub(super) error: Option<String>,
    pub(super) message: String,
    pub(super) capture_dir: Option<PathBuf>,
    pub(super) pending: u32,
    pub(super) capture_finished: bool,
    pub(super) capture_failed: bool,
    pub(super) started: Instant,
    pub(super) show_reference: bool,
    pub(super) show_inspector: bool,
    pub(super) show_colors: bool,
    pub(super) show_canopy: bool,
    pub(super) canopy_message: Option<String>,
    pub(super) show_picker: bool,
    pub(super) save_study: bool,
    pub(super) show_character: bool,
    pub(super) show_ruler: bool,
    pub(super) character: CharacterPlacement,
    pub(super) field_size: f32,
    pub(super) ground: GroundMode,
    pub(super) edge: Option<GrassEdge>,
    pub(super) character_ready: bool,
}

impl StudyState {
    pub(super) fn new(launch: StudyLaunch) -> Self {
        let refs = References::default();
        let camera = launch
            .camera
            .clone()
            .or_else(|| launch.load.as_ref().map(|d| d.camera.clone()))
            .unwrap_or_default();
        let mut wind = launch
            .load
            .as_ref()
            .map(|d| d.wind.clone())
            .unwrap_or_else(|| VegetationWind::default().into());
        if let Some(time) = launch.time {
            wind.time = time;
        }
        let seed = launch.load.as_ref().map_or(0x5a37_c19d, |d| d.seed);
        let capture_dir = launch.capture.clone();
        let show_character =
            launch.character || launch.load.as_ref().is_none_or(|d| d.show_character);
        let show_ruler = launch.ruler
            || launch.load.as_ref().is_some_and(|d| {
                if d.version == 1 {
                    d.show_character
                } else {
                    d.show_ruler
                }
            });
        let character = launch
            .load
            .as_ref()
            .map(|d| d.character.clone())
            .unwrap_or_default();
        let field_size = launch.load.as_ref().map_or(16.0, |d| d.patch_size);
        let ground = launch
            .load
            .as_ref()
            .map_or(GroundMode::Meadow, |d| d.ground);
        let edge = launch.load.as_ref().and_then(|d| d.edge);
        let comparison = launch
            .load
            .as_ref()
            .map(|d| d.comparison.clone())
            .unwrap_or_default();
        let render_size = launch
            .load
            .as_ref()
            .map_or([WIDTH, HEIGHT], |d| d.render_size);
        let msaa_samples = launch.load.as_ref().map_or(1, |d| d.msaa_samples);
        let show_inspector = launch.show_inspector;
        let show_colors = launch.show_colors;
        let show_canopy = launch.show_canopy;
        let show_picker = launch.show_picker;
        let playing = launch.play;
        let camera_label = if launch.load.is_some() {
            "Study replay"
        } else {
            "Approximate match"
        }
        .into();
        Self {
            launch,
            refs,
            camera,
            camera_label,
            comparison,
            render_size,
            msaa_samples,
            seed,
            wind,
            playing,
            playback_speed: 1.0,
            restore: None,
            own_settings: None,
            own_environment: None,
            image: default(),
            texture: egui::TextureId::default(),
            signature: None,
            candidates: 0,
            ready_frames: 0,
            error: None,
            message: String::new(),
            capture_dir,
            pending: 0,
            capture_finished: false,
            capture_failed: false,
            started: Instant::now(),
            show_reference: !show_canopy,
            show_inspector,
            show_colors,
            show_canopy,
            canopy_message: None,
            show_picker,
            save_study: false,
            show_character,
            show_ruler,
            character,
            field_size,
            ground,
            edge,
            character_ready: false,
        }
    }

    pub(super) fn match_reference(&mut self, index: usize) {
        if index >= self.refs.items.len() {
            return;
        }
        self.refs.selected = index;
        let setup = self.refs.matching_setup();
        self.camera = setup.camera;
        self.character = setup.character;
        self.field_size = setup.field_size;
        self.ground = setup.ground;
        self.edge = setup.edge;
        self.show_character = true;
        self.camera_label = format!(
            "{} · {}",
            self.refs.title(),
            if self.refs.has_saved_camera() {
                "saved setup"
            } else {
                "approximate scale match"
            }
        );
        self.comparison = ComparisonView::default();
        self.ready_frames = 0;
    }

    pub(super) fn reference_setup(&self) -> ReferenceSetup {
        ReferenceSetup {
            camera: self.camera.clone(),
            character: self.character.clone(),
            field_size: self.field_size,
            ground: self.ground,
            edge: self.edge,
        }
    }

    pub(super) fn scene_signature(&self, revision: u64, population: usize) -> SceneSignature {
        SceneSignature {
            revision,
            population,
            seed: self.seed,
            field_size: self.field_size.to_bits(),
            edge: self.edge.map(GrassEdge::signature),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::vegetation::{comparison::ComparisonView, study::StudyLaunch};

    use std::path::PathBuf;

    #[test]
    fn selecting_reference_restores_camera_and_framing_without_changing_wind() {
        let mut state = StudyState::new(StudyLaunch::default());
        state.refs.items = vec![(PathBuf::from("edge_2-0123456789abcdef.png"), None)];
        state.comparison = ComparisonView {
            zoom: 4.0,
            center: [0.3, 0.7],
        };
        state.wind.time = 13.25;
        state.match_reference(0);
        assert_eq!(state.camera, state.refs.matching_camera());
        assert_eq!(state.character.xz, [-0.55, 0.15]);
        assert!(state.show_character);
        assert_eq!(state.field_size, 16.0);
        assert!(state.edge.is_some());
        let signature = state.scene_signature(4, 3);
        state.edge.as_mut().unwrap().offset += 0.1;
        assert_ne!(signature, state.scene_signature(4, 3));
        assert_eq!(state.comparison, ComparisonView::default());
        assert_eq!(state.wind.time, 13.25);
    }
}
