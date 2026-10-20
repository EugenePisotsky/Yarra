//! Bevy/WGPU rendering of procedural grass, after Ghost of Tsushima's.
//!
//! The renderer schedules resident grass fields on the GPU, places blades on jittered lattices
//! gathered into Voronoi clumps, classifies each candidate once, emits compact instances into
//! high- and low-detail bins, finalizes indexed indirect arguments, and exposes non-blocking
//! workload diagnostics. The vertex shader builds each blade as a cubic Bézier bent by the shared
//! wind field; lighting uses rounded normals, clump-normal gloss and translucency, Bevy's view
//! data and directional shadows.

use bevy::{
    pbr::MeshPipelineSystems,
    prelude::*,
    render::{
        RenderApp, RenderStartup, extract_component::ExtractComponentPlugin,
        extract_resource::ExtractResourcePlugin,
    },
    transform::TransformSystems,
};

mod diagnostics;
mod environment;
mod renderer;
mod scene_state;
mod settings;
mod trample;

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
pub use settings::{VegetationProfileMode, VegetationSettings};
pub use trample::{GrassBender, GrassBenders, MAX_GRASS_BENDERS};

pub const PROCEDURAL_DISTANCE_METERS: f32 = 96.0;

/// Installs the vegetation render path.
/// Timing instrumentation is an application choice; this plugin never installs GPU probes.
///
/// It remains dormant until a [`VegetationSceneState`] resource exists and a camera carries
/// [`VegetationView`]. Applications own world streaming and scene construction.
pub struct VegetationRenderPlugin;

impl Plugin for VegetationRenderPlugin {
    fn build(&self, app: &mut App) {
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
            ExtractResourcePlugin::<VegetationSun>::default(),
            ExtractResourcePlugin::<VegetationAmbientGain>::default(),
            ExtractResourcePlugin::<GrassBenders>::default(),
            ExtractComponentPlugin::<VegetationView>::default(),
            ExtractComponentPlugin::<VegetationDraw>::default(),
        ))
        .init_resource::<VegetationSettings>()
        .init_resource::<VegetationLighting>()
        .init_resource::<VegetationWind>()
        .init_resource::<VegetationLodFocus>()
        .init_resource::<VegetationRenderOrigin>()
        .init_resource::<VegetationTerrainGate>()
        .init_resource::<VegetationSun>()
        .init_resource::<VegetationAmbientGain>()
        .init_resource::<GrassBenders>()
        .add_systems(Update, advance_vegetation_wind)
        .add_systems(
            PostUpdate,
            sync_vegetation_sun.after(TransformSystems::Propagate),
        )
        .add_systems(PostUpdate, (attach_default_views, maintain_draw_entity));

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app.insert_resource(diagnostics);
        render_app.add_systems(
            RenderStartup,
            renderer::initialize.after(MeshPipelineSystems),
        );
        renderer::install(render_app);
    }
}
