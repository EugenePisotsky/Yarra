//! Bevy/WGPU integration for vegetation V2.
//!
//! The renderer schedules resident population fields on the GPU, classifies each candidate once,
//! emits compact instances into bounded topology/LOD bins, finalizes indexed indirect arguments,
//! and exposes non-blocking workload diagnostics. Environment lighting and directional-shadow
//! reception use Bevy's view data. A shared analytic wind field deforms the procedural curves;
//! grass casting and additional representation families build on these contracts without depending
//! on the deleted ground-cover renderer.

use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

use bevy::{
    color::LinearRgba,
    light::SunDisk,
    pbr::MeshPipelineSystems,
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
    },
    transform::TransformSystems,
};
use vegetation::{SceneValidationError, VegetationScene};

pub mod canopy_coverage;
mod renderer;
pub use renderer::terrain_contact_radius;

/// Per-source-page terrain readiness. This gate changes rendering, never authored
/// coverage or placement data. Empty means all pages are permitted.
#[derive(Resource, ExtractResource, Clone, Debug, Default, PartialEq, Eq)]
pub struct VegetationTerrainGate {
    pub block_all: bool,
    pub blocked_pages: std::collections::BTreeSet<[u32; 3]>,
}
impl VegetationTerrainGate {
    pub fn page_id(page: &vegetation::VegetationFieldPage) -> [u32; 3] {
        [
            page.origin_xz[0].to_bits(),
            page.origin_xz[1].to_bits(),
            page.size.to_bits(),
        ]
    }
}

pub const PROCEDURAL_DISTANCE_METERS: f32 = 96.0;

static NEXT_SCENE_REVISION: AtomicU64 = AtomicU64::new(1);

/// Installs the independent vegetation V2 render path.
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
            ExtractResourcePlugin::<VegetationDebugSettings>::default(),
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
        .init_resource::<VegetationDebugSettings>()
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

/// Optional render-space focus for gameplay grass detail. A free editor camera can leave
/// this unset; an orbit camera supplies its subject so zoom never moves detail behind it.
#[derive(Resource, ExtractResource, Clone, Copy, Debug, Default)]
pub struct VegetationLodFocus {
    pub position: Option<Vec3>,
}

/// Canonical XZ offset of render coordinates, for world-anchored wind and shading.
#[derive(Resource, ExtractResource, Default, Clone, Copy, Debug)]
pub struct VegetationRenderOrigin {
    pub world_xz: [f64; 2],
}

/// Startup-only capacity for the prepared-blade arena. Applications own configuration.
#[derive(Resource, Clone, Copy)]
pub struct VegetationPreparationCapacity(pub u64);
impl Default for VegetationPreparationCapacity {
    fn default() -> Self {
        Self(131_072)
    }
}

/// Prepare shared curve and wind values once per blade, with a bounded GPU cache.
#[derive(Resource, ExtractResource, Clone, Copy, Debug)]
pub struct VegetationBladePreparation {
    pub enabled: bool,
}
impl Default for VegetationBladePreparation {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Global environment response shared by every vegetation species.
///
/// Species keep their own colors, roughness, transmission, and AO. These values tune how strongly
/// the renderer applies the world's directional light and its received shadows to that authored
/// material response.
#[derive(Resource, ExtractResource, Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct VegetationLighting {
    pub diffuse_strength: f32,
    pub specular_strength: f32,
    pub transmission_strength: f32,
    pub received_shadow_strength: f32,
    #[serde(default)]
    pub canopy: vegetation::CanopyShading,
    #[serde(skip)]
    pub canopy_origin: [f32; 2],
}

impl Default for VegetationLighting {
    fn default() -> Self {
        Self {
            diffuse_strength: 0.82,
            specular_strength: 0.28,
            transmission_strength: 0.34,
            received_shadow_strength: 0.78,
            canopy: Default::default(),
            canopy_origin: [0.0; 2],
        }
    }
}

/// Shared low-frequency wind field used by procedural vegetation.
///
/// The field is intentionally analytic so CPU gameplay and GPU rendering can sample the same
/// travelling wave without a texture dependency. Ribbons use bounded rotations of their resting
/// curves; broad leaves retain longitudinal detail. This resource supplies the shared field.
#[derive(Resource, ExtractResource, Debug, Clone, Copy)]
pub struct VegetationWind {
    /// External study/replay transport owns phase and disables diagnostic keyboard shortcuts.
    /// Default false preserves the game clock and controls.
    pub externally_driven: bool,
    pub enabled: bool,
    /// Horizontal direction in world XZ coordinates.
    pub direction: Vec2,
    /// Maximum horizontal tip displacement as a fraction of blade height.
    pub strength: f32,
    /// Broad-wave spatial frequency in radians per world unit.
    pub spatial_frequency: f32,
    /// Travelling-wave angular speed in radians per second.
    pub speed: f32,
    /// Relative strength of the slower gust layer.
    pub gustiness: f32,
    /// Grass-only hashed flutter amplitude as a fraction of blade height.
    pub flutter: f32,
    /// Clock multiplier for every travelling wave and flutter. Weather changes this rather than
    /// `speed`: phase is `time * speed`, so varying `speed` would jump the whole field.
    pub rate: f32,
    phase_seconds: f32,
}

impl Default for VegetationWind {
    fn default() -> Self {
        Self {
            externally_driven: false,
            enabled: true,
            direction: Vec2::new(0.92, 0.38).normalize(),
            strength: 0.82,
            spatial_frequency: 0.12,
            speed: 2.4,
            gustiness: 0.95,
            flutter: 0.28,
            rate: 1.0,
            phase_seconds: 0.0,
        }
    }
}

impl VegetationWind {
    /// Synchronize procedural wind for a deterministic replay or shared weather clock.
    pub fn set_phase_seconds(&mut self, seconds: f32) {
        assert!(seconds.is_finite(), "wind phase must be finite");
        self.phase_seconds = seconds.rem_euclid(4096.0);
    }

    /// Samples the coherent scalar push used by non-rendering systems.
    pub fn sample_force(&self, world_xz: Vec2) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        let direction = self.direction.try_normalize().unwrap_or(Vec2::X);
        let cross_direction = Vec2::new(-direction.y, direction.x);
        let frequency = self.spatial_frequency.max(0.001);
        let speed = self.speed.max(0.0);
        let broad_phase = world_xz.dot(direction) * frequency - self.phase_seconds * speed;
        let cross_phase =
            world_xz.dot(cross_direction) * frequency * 0.71 + self.phase_seconds * speed * 0.37;
        let broad_wave = (broad_phase + cross_phase.sin() * 0.85).sin();
        let gust_wave = (broad_phase * 0.43 - cross_phase * 0.61).sin();
        let broad_amount = broad_wave * 0.5 + 0.5;
        let gust_coordinate = (gust_wave * 0.5 + 0.5).clamp(0.0, 1.0);
        let gust_rise = ((gust_coordinate - 0.28) / 0.72).clamp(0.0, 1.0);
        let gust_pulse = gust_rise * gust_rise * (3.0 - 2.0 * gust_rise);
        self.strength.max(0.0)
            * (0.25 + broad_amount * 0.18 + gust_pulse * self.gustiness.clamp(0.0, 1.0) * 0.90)
                .clamp(0.12, 1.30)
    }

    /// Current procedural wind phase, in seconds.
    pub fn phase_seconds(self) -> f32 {
        self.phase_seconds
    }
}

fn advance_vegetation_wind(time: Res<Time>, mut wind: ResMut<VegetationWind>) {
    if wind.externally_driven {
        return;
    }
    // Bound hitch recovery so a paused debugger does not produce a single violent deformation.
    // The rate bound keeps one frame's advance below the grass history-reset threshold.
    let rate = if wind.rate.is_finite() {
        wind.rate.clamp(0.0, 2.0)
    } else {
        1.0
    };
    wind.phase_seconds =
        (wind.phase_seconds + time.delta_secs().min(0.1) * rate).rem_euclid(4096.0);
}

/// Exposure adaptation relative to the authored scene. Grass bounds its ambient fill in exposed
/// units for the stylized clear-day look; weather that opens exposure under cloud raises that
/// bound by the same factor, so grass keeps pace with PBR terrain, trees and characters.
#[derive(Resource, ExtractResource, Debug, Clone, Copy, PartialEq)]
pub struct VegetationAmbientGain(pub f32);
impl Default for VegetationAmbientGain {
    fn default() -> Self {
        Self(1.0)
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
    directional_lights: Query<(
        &DirectionalLight,
        &GlobalTransform,
        Option<&SunDisk>,
        Option<&Visibility>,
    )>,
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

    let Some((light, transform, _, _)) = directional_lights
        .iter()
        .filter(|(light, transform, disk, visibility)| {
            light.illuminance.is_finite()
                && light.illuminance > 0.0
                && visibility.is_none_or(|v| *v != Visibility::Hidden)
                // Celestial lights below the ground still illuminate the sky at twilight,
                // but must not steal surface lighting from the moon above the horizon.
                && disk.is_none_or(|d| transform.back().y > -(d.angular_size * 0.5).sin())
        })
        .max_by(|(left, ..), (right, ..)| left.illuminance.total_cmp(&right.illuminance))
    else {
        vegetation_sun.active = false;
        vegetation_sun.radiance = Vec3::ZERO;
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
    pub blade_preparation_enabled: bool,
    pub blade_preparation_bytes: u64,
    pub blade_preparation_dispatches: u64,
    pub blade_preparation_reuses: u64,
    pub prepared_blades: u32,
    pub preparation_fallback_blades: u32,
    pub scene_revision: u64,
    /// Cumulative completed placement dispatches and frames that reused their results.
    pub generation_dispatches: u64,
    pub generation_reuses: u64,
    pub candidate_cache_enabled: bool,
    pub candidate_cache_bytes: u64,
    pub candidate_cache_builds: u64,
    pub candidate_cache_ready_items: u32,
    pub candidate_cache_planned_items: u32,
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
    /// Current scene-adaptive capacities for single-high/single-low/split-high/split-low.
    pub topology_instance_capacities: [u32; 4],
    pub procedural_instance_bytes: u64,
    pub gpu_samples: u64,
    /// Scene revision associated with the last completed GPU readback (not CPU preparation).
    pub gpu_scene_revision: u64,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
}

/// Selects an isolated V2 workload for target-hardware measurements.
///
/// `DrawFrozen` intentionally retains the instances and indirect arguments produced by the last
/// full/compute frame. It is meaningful only with a fixed camera and stable residency.
/// `Full` automatically reuses placement when its inputs are unchanged; `ComputeOnly` and
/// `ScheduleOnly` always execute so their isolated workload remains measurable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[repr(u32)]
pub enum VegetationProfileMode {
    #[default]
    Full = 0,
    DrawFrozen = 1,
    ComputeOnly = 2,
    ScheduleOnly = 3,
    /// Skip all vegetation GPU work, including scheduling and draws.
    Disabled = 4,
}

impl VegetationProfileMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::DrawFrozen => "draw frozen (fixed camera)",
            Self::ComputeOnly => "compute only",
            Self::ScheduleOnly => "schedule only",
            Self::Disabled => "disabled",
        }
    }
}

/// Selects the population-density policy independently from procedural geometry LOD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
}

/// Selects the foliage-lighting response without changing authored material data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[repr(u32)]
pub enum VegetationLightingMode {
    /// Exposure-aware rounded blade highlights blended into a stable clump response.
    #[default]
    RoundedGloss = 0,
    /// The previous empirical response, retained only for runtime visual comparison.
    Legacy = 1,
    /// Minimal fragment path used to separate geometry/coverage cost from foliage lighting.
    UnlitDiagnostic = 2,
    /// Keeps all vertex invocations but skips procedural instance reads and deformation.
    VertexOnlyDiagnostic = 3,
}

impl VegetationLightingMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::RoundedGloss => "rounded + clump gloss",
            Self::Legacy => "legacy",
            Self::UnlitDiagnostic => "unlit diagnostic",
            Self::VertexOnlyDiagnostic => "minimal vertex diagnostic",
        }
    }
}

/// Shape-only comparison for a bounded study. Every active mode uses high topology but retains
/// production candidate acceptance, density fade, and width compensation. It is not a cost preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[repr(u32)]
pub enum VegetationShapeInspection {
    #[default]
    Off = 0,
    Current = 1,
    Full = 2,
    Low = 3,
    Morph = 4,
    Cause = 5,
}

impl VegetationShapeInspection {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Current,
        Self::Full,
        Self::Low,
        Self::Morph,
        Self::Cause,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Production",
            Self::Current => "Current shape",
            Self::Full => "Full shape",
            Self::Low => "Low shape",
            Self::Morph => "Morph weight",
            Self::Cause => "Simplification cause",
        }
    }
}

/// Opt-in material experiment. Off specializes out all band arithmetic and varyings.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum VegetationBladeBands {
    #[default]
    Off,
    Subtle,
    Medium,
    Mask,
    /// Hold geometry at wind time zero to inspect marks moving over fixed blades.
    MotionMask,
}

impl VegetationBladeBands {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Subtle,
        Self::Medium,
        Self::Mask,
        Self::MotionMask,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Subtle => "Subtle",
            Self::Medium => "Medium",
            Self::Mask => "Band mask",
            Self::MotionMask => "Motion mask (fixed blades)",
        }
    }
}

fn default_band_density() -> f32 {
    1.0
}

/// Shared rendering controls, including production quality and explicit diagnostic overrides.
/// Applications own the UI: the game uses F1 and the editor uses workspace controls.
/// Keep this serialized type and its fields compatible with existing vegetation study files.
#[derive(Resource, ExtractResource, Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct VegetationDebugSettings {
    pub mode: VegetationDebugMode,
    pub profile_mode: VegetationProfileMode,
    pub density_mode: VegetationDensityMode,
    pub lighting_mode: VegetationLightingMode,
    pub far_width_compensation: bool,
    /// Optional GPU statistics atomics, independent of placement's required draw counters.
    pub gpu_counters_enabled: bool,
    /// Skip production-only LOD work once a candidate is known to be rejected.
    pub early_rejection: bool,
    /// Cache stable candidate acceptance across camera movement, with a bounded reference fallback.
    pub candidate_cache_enabled: bool,
    #[serde(default)]
    pub shape_inspection: VegetationShapeInspection,
    /// Isolate view opening in a shape study without editing the species catalog.
    #[serde(default)]
    pub inspection_disable_opening: bool,
    #[serde(default)]
    pub blade_bands: VegetationBladeBands,
    /// Authored source density relative to the study's 44 roots/m² baseline, never LOD retention.
    #[serde(default = "default_band_density")]
    pub blade_band_density: f32,
}

impl Default for VegetationDebugSettings {
    fn default() -> Self {
        Self {
            mode: default(),
            profile_mode: default(),
            density_mode: default(),
            lighting_mode: default(),
            far_width_compensation: true,
            gpu_counters_enabled: false,
            early_rejection: true,
            candidate_cache_enabled: true,
            shape_inspection: default(),
            inspection_disable_opening: false,
            blade_bands: default(),
            blade_band_density: default_band_density(),
        }
    }
}

/// Immutable render-facing vegetation snapshot shared by gameplay, editor previews and probes.
#[derive(Resource, ExtractResource, Clone, Debug)]
pub struct VegetationSceneState {
    // Immutable snapshots are shared with extraction and contact certification.
    // Copying every field on every render extraction scales with resident area.
    scene: Arc<VegetationScene>,
    revision: u64,
    // Page residency changes source packing, but not how stable blade seeds are animated.
    catalog_revision: u64,
}

impl VegetationSceneState {
    pub fn new(scene: VegetationScene) -> Result<Self, SceneValidationError> {
        scene.validate()?;
        let revision = NEXT_SCENE_REVISION.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            scene: Arc::new(scene),
            revision,
            catalog_revision: revision,
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
        let revision = NEXT_SCENE_REVISION.fetch_add(1, Ordering::Relaxed);
        if scene.catalog != self.scene.catalog {
            self.catalog_revision = revision;
        }
        self.scene = Arc::new(scene);
        self.revision = revision;
        Ok(())
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn catalog_revision(&self) -> u64 {
        self.catalog_revision
    }
}

/// Opts a camera into vegetation rendering. Diagnostics are configured separately.
#[derive(Component, ExtractComponent, Clone, Copy, Debug, Default)]
pub struct VegetationView;

/// Excludes non-vegetation editor cameras from automatic scene attachment.
#[derive(Component)]
pub struct VegetationViewDisabled;

#[derive(Component, ExtractComponent, Clone, Copy, Debug, Default)]
pub(crate) struct VegetationDraw;

fn attach_default_views(
    mut commands: Commands,
    scene: Option<Res<VegetationSceneState>>,
    cameras: Query<
        (
            Entity,
            &Camera,
            Has<VegetationView>,
            Has<VegetationViewDisabled>,
        ),
        With<Camera3d>,
    >,
) {
    for (entity, camera, has_view, disabled) in &cameras {
        let enabled = scene.is_some() && camera.is_active && !disabled;
        if enabled && !has_view {
            commands.entity(entity).insert(VegetationView);
        } else if !enabled && has_view {
            commands.entity(entity).remove::<VegetationView>();
        }
    }
}

fn maintain_draw_entity(
    mut commands: Commands,
    scene: Option<Res<VegetationSceneState>>,
    draw_entities: Query<Entity, With<VegetationDraw>>,
) {
    if scene.is_some() && draw_entities.is_empty() {
        commands.spawn((VegetationDraw, Name::new("Vegetation draw")));
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
    fn scene_snapshots_share_data_and_replace_atomically() {
        let mut scene = VegetationSceneState::reference();
        let snapshot = scene.clone();
        assert!(Arc::ptr_eq(&scene.scene, &snapshot.scene));
        let mut next = scene.scene().clone();
        next.pages.clear();
        scene.replace(next).unwrap();
        assert!(!snapshot.scene().pages.is_empty());
        assert!(scene.scene().pages.is_empty());
        assert_ne!(snapshot.revision(), scene.revision());
        assert_eq!(snapshot.catalog_revision(), scene.catalog_revision());
        let mut next = scene.scene().clone();
        next.catalog.species[0].material.root_color[0] *= 0.5;
        scene.replace(next).unwrap();
        assert_ne!(snapshot.catalog_revision(), scene.catalog_revision());
    }

    #[test]
    fn external_transport_keeps_exact_wind_phase() {
        let mut app = App::new();
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(0.05));
        let mut wind = VegetationWind::default();
        wind.externally_driven = true;
        wind.set_phase_seconds(7.125);
        app.insert_resource(time)
            .insert_resource(wind)
            .init_resource::<VegetationDebugSettings>()
            .add_systems(Update, advance_vegetation_wind);
        app.update();
        let wind = app.world().resource::<VegetationWind>();
        assert_eq!(wind.phase_seconds(), 7.125);
        assert!(wind.enabled);
        assert_eq!(
            app.world()
                .resource::<VegetationDebugSettings>()
                .density_mode,
            VegetationDensityMode::Balanced
        );
        app.world_mut()
            .resource_mut::<VegetationWind>()
            .externally_driven = false;
        app.update();
        assert!(app.world().resource::<VegetationWind>().phase_seconds() > 7.125);
    }

    #[test]
    fn wind_rate_scales_the_clock_and_is_bounded() {
        let advance = |rate: f32, delta: f32| {
            let mut app = App::new();
            let mut time: Time = Time::default();
            time.advance_by(std::time::Duration::from_secs_f32(delta));
            let mut wind = VegetationWind { rate, ..default() };
            wind.set_phase_seconds(10.0);
            app.insert_resource(time)
                .insert_resource(wind)
                .add_systems(Update, advance_vegetation_wind);
            app.update();
            app.world().resource::<VegetationWind>().phase_seconds() - 10.0
        };
        assert!((advance(1.0, 0.05) - 0.05).abs() < 1e-5);
        assert!((advance(1.5, 0.05) - 0.075).abs() < 1e-5);
        assert_eq!(advance(0.0, 0.05), 0.0);
        // Hitches and invalid rates stay below the 0.25 s grass history-reset threshold.
        assert!((advance(5.0, 1.0) - 0.2).abs() < 1e-5);
        assert!((advance(f32::NAN, 0.05) - 0.05).abs() < 1e-5);
    }

    #[test]
    fn inactive_and_excluded_cameras_do_not_own_vegetation_draws() {
        let mut app = App::new();
        app.insert_resource(VegetationSceneState::reference())
            .add_systems(Update, attach_default_views);
        let active = app
            .world_mut()
            .spawn((Camera3d::default(), Camera::default()))
            .id();
        let inactive = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera {
                    is_active: false,
                    ..default()
                },
                VegetationView,
            ))
            .id();
        let excluded = app
            .world_mut()
            .spawn((Camera3d::default(), VegetationViewDisabled))
            .id();
        app.update();
        assert!(app.world().get::<VegetationView>(active).is_some());
        assert!(app.world().get::<VegetationView>(inactive).is_none());
        assert!(app.world().get::<VegetationView>(excluded).is_none());
    }

    #[test]
    fn default_wind_is_strong_coherent_and_cpu_sampleable() {
        let wind = VegetationWind::default();
        assert!(wind.enabled);
        assert!(wind.strength >= 0.8);
        assert!(wind.gustiness >= 0.9);
        assert!(wind.flutter >= 0.25);
        assert!((wind.direction.length() - 1.0).abs() < 1e-5);
        assert!(wind.sample_force(Vec2::new(12.0, -8.0)).is_finite());

        let mut disabled = wind;
        disabled.enabled = false;
        assert_eq!(disabled.sample_force(Vec2::ZERO), 0.0);
    }
}

#[cfg(test)]
mod night_lighting_tests {
    use super::*;

    #[test]
    fn moon_above_horizon_wins_over_twilight_sun_and_hidden_lights_are_excluded() {
        let mut app = App::new();
        app.init_resource::<VegetationSun>()
            .add_systems(Update, sync_vegetation_sun);
        app.world_mut().spawn((
            DirectionalLight {
                illuminance: 100_000.0,
                ..default()
            },
            GlobalTransform::from(
                Transform::from_xyz(1.0, -0.5, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
            ),
            SunDisk::EARTH,
        ));
        let moon = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    illuminance: 1800.0,
                    ..default()
                },
                GlobalTransform::from(
                    Transform::from_xyz(-1.0, 0.5, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
                ),
                SunDisk::EARTH,
            ))
            .id();
        app.update();
        let light = app.world().resource::<VegetationSun>();
        assert!(light.active);
        assert!(light.direction_to_light.y > 0.0);
        assert_eq!(light.radiance, Vec3::splat(1800.0));
        app.world_mut().entity_mut(moon).insert(Visibility::Hidden);
        app.update();
        assert!(!app.world().resource::<VegetationSun>().active);
        assert_eq!(app.world().resource::<VegetationSun>().radiance, Vec3::ZERO);
    }
}
