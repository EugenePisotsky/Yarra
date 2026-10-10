//! Bevy/WGPU rendering of procedural grass and ground cover.
//!
//! The renderer schedules resident population fields on the GPU, classifies each candidate once,
//! emits compact instances into bounded topology/LOD bins, finalizes indexed indirect arguments,
//! and exposes non-blocking workload diagnostics. Environment lighting and directional-shadow
//! reception use Bevy's view data. A shared analytic wind field deforms the procedural curves;
//! grass casting and additional representation families build on these contracts without depending
//! on the deleted ground-cover renderer.

use bevy::{
    pbr::MeshPipelineSystems,
    prelude::*,
    render::{
        RenderApp, RenderStartup, extract_component::ExtractComponentPlugin,
        extract_resource::ExtractResourcePlugin,
    },
    transform::TransformSystems,
};

pub mod canopy_coverage;
mod diagnostics;
mod environment;
mod renderer;
mod scene_state;
mod settings;
pub use diagnostics::{VegetationDiagnostics, VegetationDiagnosticsSnapshot};
pub(crate) use environment::VegetationSun;
pub use environment::{VegetationAmbientGain, VegetationLighting, VegetationWind};
use environment::{advance_vegetation_wind, sync_vegetation_sun};
pub use renderer::terrain_contact_radius;
pub(crate) use scene_state::VegetationDraw;
pub use scene_state::{
    VegetationLodFocus, VegetationRenderOrigin, VegetationSceneState, VegetationTerrainGate,
    VegetationView, VegetationViewDisabled,
};
use scene_state::{attach_default_views, maintain_draw_entity};
pub use settings::{
    VegetationBladePreparation, VegetationDensityMode, VegetationDiagnosticMode,
    VegetationLightingMode, VegetationPreparationCapacity, VegetationProfileMode,
    VegetationSettings,
};

pub const PROCEDURAL_DISTANCE_METERS: f32 = 96.0;

/// Installs the vegetation render path.
/// Timing instrumentation is an application choice; this plugin never installs GPU probes.
///
/// It remains dormant until a [`VegetationSceneState`] resource exists and a camera carries
/// [`VegetationView`]. Applications own world streaming and scene construction.
pub struct VegetationRenderPlugin;

impl Plugin for VegetationRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VegetationPreparationCapacity>();
        let capacity = *app.world().resource::<VegetationPreparationCapacity>();
        let diagnostics = VegetationDiagnostics::default();
        app.insert_resource(diagnostics.clone());
        app.add_plugins((
            ExtractResourcePlugin::<VegetationSceneState>::default(),
            ExtractResourcePlugin::<VegetationSettings>::default(),
            ExtractResourcePlugin::<VegetationLighting>::default(),
            ExtractResourcePlugin::<VegetationWind>::default(),
            ExtractResourcePlugin::<VegetationLodFocus>::default(),
            ExtractResourcePlugin::<VegetationRenderOrigin>::default(),
            ExtractResourcePlugin::<VegetationTerrainGate>::default(),
            ExtractResourcePlugin::<VegetationBladePreparation>::default(),
            ExtractResourcePlugin::<VegetationSun>::default(),
            ExtractResourcePlugin::<VegetationAmbientGain>::default(),
            ExtractComponentPlugin::<VegetationView>::default(),
            ExtractComponentPlugin::<VegetationDraw>::default(),
        ))
        .init_resource::<VegetationSettings>()
        .init_resource::<VegetationLighting>()
        .init_resource::<VegetationWind>()
        .init_resource::<VegetationLodFocus>()
        .init_resource::<VegetationRenderOrigin>()
        .init_resource::<VegetationTerrainGate>()
        .init_resource::<VegetationBladePreparation>()
        .init_resource::<VegetationSun>()
        .init_resource::<VegetationAmbientGain>()
        .add_systems(Update, advance_vegetation_wind)
        .add_systems(
            PostUpdate,
            sync_vegetation_sun.after(TransformSystems::Propagate),
        )
        .add_systems(PostUpdate, (attach_default_views, maintain_draw_entity));

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(diagnostics)
            .insert_resource(capacity);
        render_app.add_systems(
            RenderStartup,
            renderer::initialize.after(MeshPipelineSystems),
        );
        renderer::install(render_app);
    }
}
