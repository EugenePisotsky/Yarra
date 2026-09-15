//! Shared canopy controls and world-terrain integration.
mod world;

use crate::{
    shell::{
        EditorUiFrame, EditorUiSet, EditorWindowDescriptor, EditorWindowId, EditorWindowRegistry,
    },
    workspaces::{EditorWorkspace, world_workspace_active},
};
use bevy::prelude::*;
use bevy_egui::{EguiPrimaryContextPass, egui};
use std::path::PathBuf;
use vegetation::CanopyShading;
use vegetation_render::{VegetationDebugScene, VegetationDebugSettings, VegetationDensityMode, VegetationLighting};

pub(crate) const CANOPY_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.canopy"),
    workspace: EditorWorkspace::World,
    label: "Canopy",
    default_open: false,
};

pub(crate) struct EditorCanopyPlugin;
impl Plugin for EditorCanopyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<world::GroundTiles>()
            .add_systems(Startup, load_saved_look)
            .add_systems(
                Update,
                world::sync
                    .after(crate::vegetation_authoring::VegetationPreviewSync)
                    .before(terrain_render::TerrainMaterialPreparation)
                    .run_if(world_workspace_active),
            )
            .add_systems(
                EguiPrimaryContextPass,
                world_ui
                    .run_if(world_workspace_active)
                    .in_set(EditorUiSet::Workspace),
            );
    }
}

fn look_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/vegetation/canopy-look.ron")
}

fn load_saved_look(mut lighting: ResMut<VegetationLighting>) {
    let path = look_path();
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| ron::from_str::<CanopyShading>(&s).map_err(|e| e.to_string()))
    {
        Ok(look) => lighting.canopy = look,
        Err(error) => warn!("Canopy look {}: {error}", path.display()),
    }
}

fn world_ui(
    mut frame: ResMut<EditorUiFrame>,
    mut windows: ResMut<EditorWindowRegistry>,
    mut lighting: ResMut<VegetationLighting>,
    mut settings: ResMut<VegetationDebugSettings>,
    scene: Res<VegetationDebugScene>,
    mut message: Local<Option<String>>,
) {
    let mut open = windows.is_open(CANOPY_WINDOW.id);
    if !open {
        return;
    }
    let Some(root) = frame.0.as_mut() else {
        return;
    };
    let rect = root.available_rect_before_wrap();
    egui::Window::new("Canopy · ground and grass")
        .id(egui::Id::new(CANOPY_WINDOW.id.0))
        .open(&mut open).default_width(350.0)
        .default_pos(rect.left_top() + egui::vec2(30.0, 90.0))
        .constrain_to(rect).vscroll(true).show(root.ctx(), |ui| {
            ui.label("Shared with the grass study. Changes preview on world terrain and grass.");
            if scene.scene().pages.is_empty() {
                ui.label("Activate the Vegetation tool with Live preview to show grass and its ground shading.");
            }
            draw_controls(ui, &mut lighting.canopy, &mut message);
            density_controls(ui, &mut settings);
            if let Some(message) = &*message { ui.label(message); }
        });
    windows.set_open(CANOPY_WINDOW.id, open);
}

pub(crate) fn density_controls(ui: &mut egui::Ui, settings: &mut VegetationDebugSettings) {
    ui.separator();
    ui.label("Grass density · geometry LOD stays enabled");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut settings.density_mode, VegetationDensityMode::FullReference, "Keep all roots");
        ui.selectable_value(&mut settings.density_mode, VegetationDensityMode::Balanced, "Balanced");
    });
    ui.small("Keep all roots isolates canopy from density thinning. This choice is saved with the study, separately from the canopy look.");
}

pub(crate) fn draw_controls(
    ui: &mut egui::Ui,
    look: &mut CanopyShading,
    message: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut look.enabled, "Enabled");
        if ui.button("Reset experiment").clicked() {
            *look = vegetation::CanopyShading::experiment();
        }
    });
    ui.add(egui::Slider::new(&mut look.strength, 0.0..=0.98).text("Maximum darkness"));
    ui.horizontal(|ui| {
        if ui.button("Combined").clicked() {
            look.enabled = true;
            look.ground_amount = 1.0;
            look.blade_amount = 1.0;
        }
        if ui.button("Ground only").clicked() {
            look.enabled = true;
            look.ground_amount = 1.0;
            look.blade_amount = 0.0;
        }
        if ui.button("Blades only").clicked() {
            look.enabled = true;
            look.ground_amount = 0.0;
            look.blade_amount = 1.0;
        }
    });
    ui.add(egui::Slider::new(&mut look.ground_amount, 0.0..=1.0).text("Ground amount"));
    ui.add(egui::Slider::new(&mut look.blade_amount, 0.0..=1.0).text("Blade amount"));
    ui.separator();
    ui.add(egui::Slider::new(&mut look.height_metres, 0.02..=1.0).text("Shade height · m"));
    ui.add(egui::Slider::new(&mut look.softness, 0.1..=1.0).text("Height softness"));
    ui.add(egui::Slider::new(&mut look.patch_metres, 0.2..=4.0).text("Patch size · m"));
    ui.add(egui::Slider::new(&mut look.patchiness, 0.0..=1.0).text("Patch variation"));
    ui.add(egui::Slider::new(&mut look.openings, 0.0..=0.8).text("Open / lighter pockets"));
    ui.separator();
    ui.label("Distance gradient · actual camera distance");
    ui.add(egui::Slider::new(&mut look.near_strength, 0.0..=1.0).text("Nearby shade fraction"));
    ui.add(egui::Slider::new(&mut look.distance_start, 0.0..=30.0).text("Fade starts · m"));
    ui.add(egui::Slider::new(&mut look.distance_end, 1.0..=80.0).text("Maximum at · m"));
    look.distance_end = look.distance_end.max(look.distance_start + 0.1);
    ui.add(egui::Slider::new(&mut look.patch_growth, 0.0..=1.0).text("Pocket expansion"));
    ui.add(egui::Slider::new(&mut look.edge_width, 0.05..=4.0).text("Grass edge fade · m"));
    ui.small("Nearby shade 0 leaves close ground normal. Linked zoom in Study only magnifies the image.");
    ui.separator();
    let path = look_path();
    ui.horizontal(|ui| {
        if ui.button("Save canopy look").clicked() {
            *message = Some(
                match ron::ser::to_string_pretty(look, ron::ser::PrettyConfig::default())
                    .map_err(|e| e.to_string())
                    .and_then(|s| std::fs::write(&path, s).map_err(|e| e.to_string()))
                {
                    Ok(()) => {
                        "Saved for editor startup and game. Press H in the game to reload canopy."
                            .into()
                    }
                    Err(e) => e,
                },
            );
        }
        if ui.button("Load saved look").clicked() {
            *message = Some(
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|s| ron::from_str(&s).map_err(|e| e.to_string()))
                {
                    Ok(saved) => {
                        *look = saved;
                        "Loaded saved look.".into()
                    }
                    Err(e) => e,
                },
            );
        }
    });
}
