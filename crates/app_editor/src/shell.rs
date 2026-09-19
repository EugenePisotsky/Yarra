//! Editor application composition and shared workspace shell.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use bevy::{
    asset::AssetPlugin,
    camera::CameraOutputMode,
    camera::visibility::RenderLayers,
    diagnostic::FrameTimeDiagnosticsPlugin,
    prelude::*,
    render::{render_resource::BlendState, view::Msaa},
    window::{PresentMode, WindowResolution},
    winit::{UpdateMode, WinitSettings},
};
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext, egui,
};
use engine::{WorldEnvironmentPlugin, WorldStreamingPlugin, WorldViewCamera};
use terrain_render::TerrainRenderPlugin;

use crate::{
    derived_jobs::DerivedJobsPlugin,
    journal::EditorJournalPlugin,
    navigation::ProjectNavigationPlugin,
    project_store::ProjectEditorStorePlugin,
    publication::RuntimePublicationPlugin,
    tools::EditorToolsPlugin,
    workspaces::{
        AnimationWorkspaceCamera, AnimationWorkspacePlugin, EditorFramePacing, EditorWorkspace,
        EditorWorkspacesPlugin, PresetWorkspaceCamera, PresetWorkspacePlugin,
        VegetationWorkspaceCamera, VegetationWorkspacePlugin, WorldWorkspacePlugin,
    },
};

pub(crate) const AUTHORING_FRAME_RATE: f64 = 30.0;
const INTERACTIVE_PREVIEW_FRAME_RATE: f64 = 60.0;
const UNFOCUSED_FRAME_RATE: f64 = 5.0;

pub(crate) fn run() -> std::result::Result<(), String> {
    let asset_root = resolve_asset_root();
    let runtime_database = runtime_database_path(&asset_root);
    let project_database = project_database_path();
    let start_view = engine::WorldStartView::from_args()?;
    crate::startup::validate_databases(&project_database, &runtime_database)?;
    App::new()
        .insert_resource(start_view)
        .insert_resource(ClearColor(Color::srgb(0.055, 0.065, 0.075)))
        .insert_resource(editor_winit_settings())
        .insert_resource(EguiGlobalSettings {
            auto_create_primary_context: false,
            ..default()
        })
        .init_resource::<EditorInputCapture>()
        .init_resource::<EditorUiFrame>()
        .init_resource::<EditorWindowRegistry>()
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: asset_root.to_string_lossy().into_owned(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Yarra Editor".into(),
                        resolution: WindowResolution::new(1440, 900),
                        present_mode: PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins((
            FrameTimeDiagnosticsPlugin::default(),
            WorldEnvironmentPlugin::editor(),
            TerrainRenderPlugin,
            EguiPlugin::default(),
            DerivedJobsPlugin::new(project_database.clone()),
            EditorToolsPlugin,
            EditorWorkspacesPlugin,
            WorldWorkspacePlugin,
            AnimationWorkspacePlugin,
            VegetationWorkspacePlugin {
                runtime_database: runtime_database.clone(),
            },
            WorldStreamingPlugin::editor(runtime_database.clone()),
            ProjectEditorStorePlugin::new(project_database.clone()),
            ProjectNavigationPlugin::new(project_database.clone()),
            EditorJournalPlugin::new(project_database.clone()),
            RuntimePublicationPlugin::new(project_database, runtime_database, asset_root),
        ))
        .add_plugins(PresetWorkspacePlugin)
        .configure_sets(
            EguiPrimaryContextPass,
            (
                EditorUiSet::Shell,
                EditorUiSet::Workspace,
                EditorUiSet::Capture,
            )
                .chain(),
        )
        .add_systems(Startup, setup_editor_shell)
        .add_systems(
            Update,
            (sync_workspace_cameras, update_workspace_frame_pacing).chain(),
        )
        .add_systems(
            EguiPrimaryContextPass,
            editor_shell.in_set(EditorUiSet::Shell),
        )
        .add_systems(
            EguiPrimaryContextPass,
            capture_editor_input.in_set(EditorUiSet::Capture),
        )
        .run();
    Ok(())
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EditorUiSet {
    Shell,
    Workspace,
    Capture,
}

#[derive(Resource, Default)]
pub(crate) struct EditorInputCapture {
    pub(crate) wants_pointer: bool,
    pub(crate) wants_keyboard: bool,
}

#[derive(Resource, Default)]
pub(crate) struct EditorUiFrame(pub(crate) Option<egui::Ui>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct EditorWindowId(pub(crate) &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EditorWindowDescriptor {
    pub(crate) id: EditorWindowId,
    pub(crate) workspace: EditorWorkspace,
    pub(crate) label: &'static str,
    pub(crate) default_open: bool,
}

/// Visibility state for workspace-owned floating windows.
///
/// This registry deliberately knows nothing about authoring tools or their source domains. Opening
/// a utility window is a presentation choice and must never expand the world's data demand by
/// itself. Workspace plugins register descriptors; the shell only renders their common menu.
#[derive(Resource, Default)]
pub(crate) struct EditorWindowRegistry {
    descriptors: Vec<EditorWindowDescriptor>,
    open: HashMap<EditorWindowId, bool>,
}

impl EditorWindowRegistry {
    pub(crate) fn register(&mut self, descriptor: EditorWindowDescriptor) {
        assert!(
            self.descriptors
                .iter()
                .all(|registered| registered.id != descriptor.id),
            "duplicate editor window {}",
            descriptor.id.0
        );
        self.open.insert(descriptor.id, descriptor.default_open);
        self.descriptors.push(descriptor);
    }

    pub(crate) fn windows_for(
        &self,
        workspace: EditorWorkspace,
    ) -> impl Iterator<Item = EditorWindowDescriptor> + '_ {
        self.descriptors
            .iter()
            .filter(move |descriptor| descriptor.workspace == workspace)
            .copied()
    }

    pub(crate) fn is_open(&self, id: EditorWindowId) -> bool {
        self.open.get(&id).copied().unwrap_or(false)
    }

    pub(crate) fn set_open(&mut self, id: EditorWindowId, open: bool) {
        if let Some(value) = self.open.get_mut(&id) {
            *value = open;
        }
    }
}

#[derive(Component)]
pub(crate) struct EditorUiCamera;

pub(crate) fn setup_editor_shell(mut commands: Commands) {
    commands.spawn((
        PrimaryEguiContext,
        Camera2d,
        Msaa::Off,
        RenderLayers::none(),
        editor_ui_camera(),
        EditorUiCamera,
        Name::new("Editor shell UI camera"),
    ));
}

pub(crate) fn editor_ui_camera() -> Camera {
    Camera {
        order: 100,
        output_mode: CameraOutputMode::Write {
            blend_state: Some(BlendState::ALPHA_BLENDING),
            clear_color: ClearColorConfig::None,
        },
        clear_color: ClearColorConfig::Custom(Color::NONE),
        ..default()
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn sync_workspace_cameras(
    workspace: Res<State<EditorWorkspace>>,
    mut world_cameras: Query<
        &mut Camera,
        (
            Without<PresetWorkspaceCamera>,
            With<WorldViewCamera>,
            Without<AnimationWorkspaceCamera>,
            Without<VegetationWorkspaceCamera>,
        ),
    >,
    mut animation_cameras: Query<
        &mut Camera,
        (
            Without<PresetWorkspaceCamera>,
            With<AnimationWorkspaceCamera>,
            Without<WorldViewCamera>,
            Without<VegetationWorkspaceCamera>,
        ),
    >,
    mut vegetation_cameras: Query<
        &mut Camera,
        (
            Without<PresetWorkspaceCamera>,
            With<VegetationWorkspaceCamera>,
            Without<WorldViewCamera>,
            Without<AnimationWorkspaceCamera>,
        ),
    >,
    mut preset_cameras: Query<
        &mut Camera,
        (
            With<PresetWorkspaceCamera>,
            Without<WorldViewCamera>,
            Without<AnimationWorkspaceCamera>,
            Without<VegetationWorkspaceCamera>,
        ),
    >,
) {
    for mut camera in &mut preset_cameras {
        camera.is_active = *workspace.get() == EditorWorkspace::Presets;
    }
    let world_active = *workspace.get() == EditorWorkspace::World;
    for mut camera in &mut world_cameras {
        camera.is_active = world_active;
    }
    for mut camera in &mut animation_cameras {
        camera.is_active = *workspace.get() == EditorWorkspace::Animation;
    }
    for mut camera in &mut vegetation_cameras {
        camera.is_active = *workspace.get() == EditorWorkspace::Vegetation;
    }
}

fn update_workspace_frame_pacing(
    pacing: Res<EditorFramePacing>,
    mut settings: ResMut<WinitSettings>,
) {
    let frame_rate = if pacing.full_rate_preview() {
        INTERACTIVE_PREVIEW_FRAME_RATE
    } else {
        AUTHORING_FRAME_RATE
    };
    let focused_mode = reactive_update_mode(frame_rate, true);
    if settings.focused_mode != focused_mode {
        settings.focused_mode = focused_mode;
    }
}

fn editor_shell(
    mut contexts: EguiContexts,
    workspace: Res<State<EditorWorkspace>>,
    mut next_workspace: ResMut<NextState<EditorWorkspace>>,
    mut windows: ResMut<EditorWindowRegistry>,
    mut frame: ResMut<EditorUiFrame>,
) -> Result {
    let active_workspace = *workspace.get();
    let context = contexts.ctx_mut()?;
    frame.0 = Some(egui::Ui::new(
        context.clone(),
        "editor_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(context.viewport_rect()),
    ));
    let viewport_ui = frame
        .0
        .as_mut()
        .expect("editor shell just initialized the UI frame");
    egui::Panel::top("editor_top_bar").show(viewport_ui, |ui| {
        ui.horizontal(|ui| {
            ui.strong("Yarra Editor");
            ui.separator();
            ui.menu_button(active_workspace.label(), |ui| {
                for workspace in EditorWorkspace::ALL {
                    if ui
                        .selectable_label(workspace == active_workspace, workspace.label())
                        .clicked()
                    {
                        next_workspace.set(workspace);
                        ui.close();
                    }
                }
            });
            let workspace_windows = windows.windows_for(active_workspace).collect::<Vec<_>>();
            if !workspace_windows.is_empty() {
                ui.menu_button("Tools", |ui| {
                    for descriptor in workspace_windows {
                        let mut open = windows.is_open(descriptor.id);
                        if ui.checkbox(&mut open, descriptor.label).changed() {
                            windows.set_open(descriptor.id, open);
                        }
                    }
                });
            }
        });
    });
    Ok(())
}

fn capture_editor_input(
    mut contexts: EguiContexts,
    mut capture: ResMut<EditorInputCapture>,
    mut frame: ResMut<EditorUiFrame>,
) -> Result {
    let context = contexts.ctx_mut()?;
    capture.wants_pointer = context.egui_wants_pointer_input();
    capture.wants_keyboard = context.egui_wants_keyboard_input();
    frame.0 = None;
    Ok(())
}

pub(crate) fn editor_winit_settings() -> WinitSettings {
    WinitSettings {
        focused_mode: reactive_update_mode(AUTHORING_FRAME_RATE, true),
        unfocused_mode: reactive_update_mode(UNFOCUSED_FRAME_RATE, true),
    }
}

fn reactive_update_mode(frame_rate: f64, wake_on_input: bool) -> UpdateMode {
    UpdateMode::Reactive {
        wait: Duration::from_secs_f64(1.0 / frame_rate),
        react_to_device_events: wake_on_input,
        react_to_user_events: wake_on_input,
        react_to_window_events: wake_on_input,
    }
}

fn runtime_database_path(asset_root: &std::path::Path) -> PathBuf {
    let mut arguments = std::env::args_os();
    while let Some(argument) = arguments.next() {
        if argument == "--world-db" {
            return arguments
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| panic!("--world-db requires a database path"));
        }
    }
    asset_root.join(world::DEFAULT_RUNTIME_DATABASE)
}

fn project_database_path() -> PathBuf {
    let mut arguments = std::env::args_os();
    while let Some(argument) = arguments.next() {
        if argument == "--project-db" {
            return arguments
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| panic!("--project-db requires a database path"));
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(world::DEFAULT_PROJECT_DATABASE)
}

fn resolve_asset_root() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(current_directory) = std::env::current_dir() {
        candidates.push(current_directory.join("assets"));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(executable_directory) = executable.parent()
    {
        candidates.push(executable_directory.join("assets"));
        candidates.push(executable_directory.join("../Resources/assets"));
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets"));

    candidates
        .into_iter()
        .find(|candidate| candidate.is_dir())
        .unwrap_or_else(|| PathBuf::from("assets"))
}

#[cfg(test)]
mod window_registry_tests {
    use super::*;

    const WORLD_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
        id: EditorWindowId("test.world"),
        workspace: EditorWorkspace::World,
        label: "World",
        default_open: true,
    };
    const ANIMATION_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
        id: EditorWindowId("test.animation"),
        workspace: EditorWorkspace::Animation,
        label: "Animation browser",
        default_open: false,
    };

    #[test]
    fn floating_window_visibility_is_typed_and_workspace_scoped() {
        let mut registry = EditorWindowRegistry::default();
        registry.register(WORLD_WINDOW);
        registry.register(ANIMATION_WINDOW);

        assert_eq!(
            registry
                .windows_for(EditorWorkspace::World)
                .collect::<Vec<_>>(),
            vec![WORLD_WINDOW]
        );
        assert!(registry.is_open(WORLD_WINDOW.id));
        assert!(!registry.is_open(ANIMATION_WINDOW.id));

        registry.set_open(WORLD_WINDOW.id, false);
        registry.set_open(ANIMATION_WINDOW.id, true);
        assert!(!registry.is_open(WORLD_WINDOW.id));
        assert!(registry.is_open(ANIMATION_WINDOW.id));
    }
}
