//! Bevy/WGPU integration for vegetation V2.
//!
//! The renderer schedules resident population fields on the GPU, classifies each candidate once,
//! emits compact instances into bounded topology/LOD bins, finalizes indexed indirect arguments,
//! and exposes non-blocking workload diagnostics. Environment lighting and directional-shadow
//! reception use Bevy's view data; wind, grass casting, and additional representation families
//! build on these contracts without depending on the deleted ground-cover renderer.

use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

use bevy::{
    color::LinearRgba,
    pbr::MeshPipelineSystems,
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        diagnostic::RenderDiagnosticsPlugin,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
    },
    transform::TransformSystems,
};
use vegetation::{SceneValidationError, VegetationScene};

mod renderer;

static NEXT_SCENE_REVISION: AtomicU64 = AtomicU64::new(1);

/// Installs the independent vegetation V2 render path.
///
/// It remains dormant until a [`VegetationDebugScene`] resource exists and a camera carries
/// [`VegetationDebugView`]. The current game integration enables this path explicitly while V2 is
/// measured; the renderer contracts themselves are the production foundation.
pub struct VegetationRenderPlugin;

impl Plugin for VegetationRenderPlugin {
    fn build(&self, app: &mut App) {
        let diagnostics = VegetationDiagnostics::default();
        app.insert_resource(diagnostics.clone());
        app.add_plugins((
            RenderDiagnosticsPlugin,
            ExtractResourcePlugin::<VegetationDebugScene>::default(),
            ExtractResourcePlugin::<VegetationDebugSettings>::default(),
            ExtractResourcePlugin::<VegetationLighting>::default(),
            ExtractResourcePlugin::<VegetationSun>::default(),
            ExtractComponentPlugin::<VegetationDebugView>::default(),
            ExtractComponentPlugin::<VegetationDebugDraw>::default(),
        ))
        .init_resource::<VegetationDebugSettings>()
        .init_resource::<VegetationLighting>()
        .init_resource::<VegetationSun>()
        .add_systems(Update, cycle_debug_mode)
        .add_systems(
            PostUpdate,
            sync_vegetation_sun.after(TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            (attach_default_debug_views, maintain_debug_draw_entity),
        );

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

/// Global environment response shared by every vegetation species.
///
/// Species keep their own colors, roughness, transmission, and AO. These values tune how strongly
/// the renderer applies the world's directional light and its received shadows to that authored
/// material response.
#[derive(Resource, ExtractResource, Debug, Clone, Copy)]
pub struct VegetationLighting {
    pub diffuse_strength: f32,
    pub specular_strength: f32,
    pub transmission_strength: f32,
    pub received_shadow_strength: f32,
}

impl Default for VegetationLighting {
    fn default() -> Self {
        Self {
            diffuse_strength: 0.82,
            specular_strength: 0.28,
            transmission_strength: 0.34,
            received_shadow_strength: 0.78,
        }
    }
}

/// Render-facing snapshot of the strongest directional light and the global ambient fill.
#[derive(Resource, ExtractResource, Debug, Clone, Copy)]
pub(crate) struct VegetationSun {
    direction_to_light: Vec3,
    radiance: Vec3,
    ambient_radiance: Vec3,
    active: bool,
}

impl Default for VegetationSun {
    fn default() -> Self {
        Self {
            direction_to_light: Vec3::Y,
            radiance: Vec3::ONE,
            ambient_radiance: Vec3::ONE,
            active: false,
        }
    }
}

fn sync_vegetation_sun(
    directional_lights: Query<(&DirectionalLight, &GlobalTransform)>,
    ambient: Option<Res<GlobalAmbientLight>>,
    mut vegetation_sun: ResMut<VegetationSun>,
) {
    if let Some(ambient) = ambient {
        let color = LinearRgba::from(ambient.color);
        vegetation_sun.ambient_radiance =
            Vec3::new(color.red, color.green, color.blue) * ambient.brightness.max(0.0);
    } else {
        vegetation_sun.ambient_radiance = Vec3::ONE;
    }

    let Some((light, transform)) = directional_lights
        .iter()
        .filter(|(light, _)| light.illuminance.is_finite())
        .max_by(|(left, _), (right, _)| left.illuminance.total_cmp(&right.illuminance))
    else {
        vegetation_sun.active = false;
        return;
    };
    let color = LinearRgba::from(light.color);
    vegetation_sun.direction_to_light = transform.back().into();
    vegetation_sun.radiance =
        Vec3::new(color.red, color.green, color.blue) * light.illuminance.max(0.0);
    vegetation_sun.active = light.illuminance > 0.0;
}

/// A low-frequency snapshot of V2 source lifetime and GPU placement work.
///
/// GPU values are copied into a tiny staging buffer without waiting for the device, so they are
/// normally one or more frames behind the scene counters. This is intentional: diagnostics must
/// not perturb the timings they are meant to explain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VegetationDiagnosticsSnapshot {
    pub scene_revision: u64,
    pub source_repacks: u64,
    /// Cumulative backing-buffer growth events. This should stop after warm-up traversal.
    pub source_buffer_reallocations: u64,
    /// Bytes uploaded for the most recent source revision.
    pub source_uploaded_bytes: u64,
    /// Grow-only capacity retained by resident source and scheduling buffers.
    pub source_buffer_capacity_bytes: u64,
    pub source_pages: u32,
    pub source_work_items: u32,
    pub maximum_candidates_per_item: u32,
    pub scheduled_work_items: u32,
    /// Padded compute lanes in the sampled indirect dispatch.
    pub dispatched_candidate_lanes: u32,
    pub candidate_evaluations: u32,
    pub eligible_instances: [u32; 4],
    pub emitted_instances: [u32; 4],
    pub capacity_dropped_instances: [u32; 4],
    /// Indices submitted by the four procedural draws for the sampled frame.
    pub submitted_indices: u64,
    /// Maximum distinct topology inputs per instance before the GPU's post-transform cache.
    pub topology_vertex_inputs: u64,
    /// Fixed number of compact procedural instance records in the active device profile.
    pub procedural_instance_capacity: u32,
    pub procedural_instance_bytes: u64,
    pub gpu_samples: u64,
}

/// Thread-safe bridge used by the main and render worlds for vegetation diagnostics.
#[derive(Resource, Clone, Debug, Default)]
pub struct VegetationDiagnostics {
    snapshot: Arc<RwLock<VegetationDiagnosticsSnapshot>>,
}

impl VegetationDiagnostics {
    pub fn snapshot(&self) -> VegetationDiagnosticsSnapshot {
        *self
            .snapshot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn update(&self, update: impl FnOnce(&mut VegetationDiagnosticsSnapshot)) {
        let mut snapshot = self
            .snapshot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut snapshot);
    }
}

/// What the V2 GPU placement diagnostic visualizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum VegetationDebugMode {
    /// Procedural curved blades emitted into single- and split-blade indirect bins.
    #[default]
    ProceduralGeometry = 0,
    /// Accepted roots, colored by their selected species.
    AcceptedSpecies = 1,
    /// Accepted parent/child relationships, drawn from the shared parent to each child root.
    ParentLinks = 2,
    /// All page-owned candidates, colored by their acceptance or rejection reason.
    CandidateOutcomes = 3,
    /// Accepted roots linked to their common parent or analytic Voronoi centre.
    GroupStructure = 4,
}

impl VegetationDebugMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ProceduralGeometry => "procedural geometry",
            Self::AcceptedSpecies => "accepted species",
            Self::ParentLinks => "parent links",
            Self::CandidateOutcomes => "candidate outcomes",
            Self::GroupStructure => "group structure",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::ProceduralGeometry => Self::AcceptedSpecies,
            Self::AcceptedSpecies => Self::ParentLinks,
            Self::ParentLinks => Self::CandidateOutcomes,
            Self::CandidateOutcomes => Self::GroupStructure,
            Self::GroupStructure => Self::ProceduralGeometry,
        }
    }
}

/// Selects an isolated V2 workload for target-hardware measurements.
///
/// `DrawFrozen` intentionally retains the instances and indirect arguments produced by the last
/// full/compute frame. It is meaningful only with a fixed camera and stable residency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum VegetationProfileMode {
    #[default]
    Full = 0,
    DrawFrozen = 1,
    ComputeOnly = 2,
    ScheduleOnly = 3,
}

impl VegetationProfileMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::DrawFrozen => "draw frozen (fixed camera)",
            Self::ComputeOnly => "compute only",
            Self::ScheduleOnly => "schedule only",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Full => Self::DrawFrozen,
            Self::DrawFrozen => Self::ComputeOnly,
            Self::ComputeOnly => Self::ScheduleOnly,
            Self::ScheduleOnly => Self::Full,
        }
    }
}

/// Selects the population-density policy independently from procedural geometry LOD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum VegetationDensityMode {
    /// Use the density fractions and projected thresholds authored on each representation.
    Authored = 0,
    /// Production policy: preserve density while roots are clearly visible, then taper near subpixel spacing.
    #[default]
    Balanced = 1,
    /// Keep every otherwise eligible candidate as a diagnostic visual and cost ceiling.
    FullReference = 2,
}

impl VegetationDensityMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Authored => "authored thinning",
            Self::Balanced => "balanced production",
            Self::FullReference => "100% reference",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Authored => Self::Balanced,
            Self::Balanced => Self::FullReference,
            Self::FullReference => Self::Authored,
        }
    }
}

/// Runtime controls for the V2 placement and profiling diagnostics.
///
/// Press `X` to cycle the visual explanation, `P` to isolate render workloads, and `O` to cycle
/// balanced production, full-reference, and authored population density.
#[derive(Resource, ExtractResource, Debug, Clone, Copy, Default)]
pub struct VegetationDebugSettings {
    pub mode: VegetationDebugMode,
    pub profile_mode: VegetationProfileMode,
    pub density_mode: VegetationDensityMode,
}

fn cycle_debug_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mut settings: ResMut<VegetationDebugSettings>,
) {
    if keys.just_pressed(KeyCode::KeyX) {
        settings.mode = settings.mode.next();
        warn!("vegetation-v2 diagnostic: {}", settings.mode.label());
    }
    if keys.just_pressed(KeyCode::KeyP) {
        settings.profile_mode = settings.profile_mode.next();
        warn!(
            "vegetation-v2 profile workload: {}",
            settings.profile_mode.label()
        );
    }
    if keys.just_pressed(KeyCode::KeyO) {
        settings.density_mode = settings.density_mode.next();
        warn!(
            "vegetation-v2 LOD density: {}",
            settings.density_mode.label()
        );
    }
}

/// An explicitly enabled, in-memory V2 scene used to validate GPU placement before persistence.
#[derive(Resource, ExtractResource, Clone, Debug)]
pub struct VegetationDebugScene {
    scene: VegetationScene,
    revision: u64,
}

impl VegetationDebugScene {
    pub fn new(scene: VegetationScene) -> Result<Self, SceneValidationError> {
        scene.validate()?;
        Ok(Self {
            scene,
            revision: NEXT_SCENE_REVISION.fetch_add(1, Ordering::Relaxed),
        })
    }

    /// The two-page dry-tuft and mixed-green fixture used by the first vertical slice.
    pub fn reference() -> Self {
        Self::new(vegetation::fixtures::reference_scene()).expect("reference fixture is valid")
    }

    pub fn scene(&self) -> &VegetationScene {
        &self.scene
    }

    pub fn replace(&mut self, scene: VegetationScene) -> Result<(), SceneValidationError> {
        scene.validate()?;
        self.scene = scene;
        self.revision = NEXT_SCENE_REVISION.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}

/// Opts a camera into V2 rendering and its placement diagnostics.
#[derive(Component, ExtractComponent, Clone, Copy, Debug, Default)]
pub struct VegetationDebugView;

#[derive(Component, ExtractComponent, Clone, Copy, Debug, Default)]
pub(crate) struct VegetationDebugDraw;

fn attach_default_debug_views(
    mut commands: Commands,
    scene: Option<Res<VegetationDebugScene>>,
    cameras: Query<(Entity, Has<VegetationDebugView>), With<Camera3d>>,
) {
    for (entity, has_debug_view) in &cameras {
        if scene.is_some() && !has_debug_view {
            commands.entity(entity).insert(VegetationDebugView);
        } else if scene.is_none() && has_debug_view {
            commands.entity(entity).remove::<VegetationDebugView>();
        }
    }
}

fn maintain_debug_draw_entity(
    mut commands: Commands,
    scene: Option<Res<VegetationDebugScene>>,
    draw_entities: Query<Entity, With<VegetationDebugDraw>>,
) {
    if scene.is_some() && draw_entities.is_empty() {
        commands.spawn((VegetationDebugDraw, Name::new("Vegetation V2 debug draw")));
    } else if scene.is_none() {
        for entity in &draw_entities {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_cycle_enters_frozen_draw_directly_after_full() {
        let full = VegetationProfileMode::Full;
        let draw = full.next();
        let compute = draw.next();
        let schedule = compute.next();
        assert_eq!(draw, VegetationProfileMode::DrawFrozen);
        assert_eq!(compute, VegetationProfileMode::ComputeOnly);
        assert_eq!(schedule, VegetationProfileMode::ScheduleOnly);
        assert_eq!(schedule.next(), full);
    }

    #[test]
    fn density_cycle_keeps_balanced_between_authored_and_full_reference() {
        assert_eq!(
            VegetationDensityMode::default(),
            VegetationDensityMode::Balanced
        );
        let authored = VegetationDensityMode::Authored;
        let balanced = authored.next();
        let full = balanced.next();
        assert_eq!(balanced, VegetationDensityMode::Balanced);
        assert_eq!(full, VegetationDensityMode::FullReference);
        assert_eq!(full.next(), authored);
    }
}
