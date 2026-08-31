//! Bevy/WGPU integration for vegetation V2.
//!
//! The renderer schedules resident population fields on the GPU, classifies each candidate once,
//! emits compact instances into bounded topology/LOD bins, finalizes indexed indirect arguments,
//! and exposes non-blocking workload diagnostics. Wind, shadows, and additional representation
//! families build on these contracts without depending on the deleted ground-cover renderer.

use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

use bevy::{
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        diagnostic::RenderDiagnosticsPlugin,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
    },
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
            ExtractComponentPlugin::<VegetationDebugView>::default(),
            ExtractComponentPlugin::<VegetationDebugDraw>::default(),
        ))
        .init_resource::<VegetationDebugSettings>()
        .add_systems(Update, cycle_debug_mode)
        .add_systems(
            PostUpdate,
            (attach_default_debug_views, maintain_debug_draw_entity),
        );

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app.insert_resource(diagnostics);
        render_app.add_systems(RenderStartup, renderer::initialize);
        renderer::install(render_app);
    }
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
}

impl VegetationDebugMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ProceduralGeometry => "procedural geometry",
            Self::AcceptedSpecies => "accepted species",
            Self::ParentLinks => "parent links",
            Self::CandidateOutcomes => "candidate outcomes",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::ProceduralGeometry => Self::AcceptedSpecies,
            Self::AcceptedSpecies => Self::ParentLinks,
            Self::ParentLinks => Self::CandidateOutcomes,
            Self::CandidateOutcomes => Self::ProceduralGeometry,
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

/// Runtime controls for the V2 placement and profiling diagnostics.
///
/// Press `X` to cycle the visual explanation and `P` to isolate render workloads.
#[derive(Resource, ExtractResource, Debug, Clone, Copy, Default)]
pub struct VegetationDebugSettings {
    pub mode: VegetationDebugMode,
    pub profile_mode: VegetationProfileMode,
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
}
