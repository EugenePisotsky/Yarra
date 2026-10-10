//! The render-facing vegetation scene, terrain readiness, LOD focus and the cameras and draw
//! entity that opt into rendering.
use bevy::{
    prelude::*,
    render::{extract_component::ExtractComponent, extract_resource::ExtractResource},
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use vegetation::{SceneValidationError, VegetationScene};

static NEXT_SCENE_REVISION: AtomicU64 = AtomicU64::new(1);
static NEXT_PAGE_KEY: AtomicU64 = AtomicU64::new(1);

/// Per-source-page terrain readiness. This gate changes rendering, never authored
/// coverage or placement data. Empty means all pages are permitted.
#[derive(Resource, ExtractResource, Clone, Debug, Default, PartialEq, Eq)]
#[extract_app(bevy::render::RenderApp)]
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

/// Optional render-space focus for gameplay grass detail. A free editor camera can leave
/// this unset; an orbit camera supplies its subject so zoom never moves detail behind it.
#[derive(Resource, ExtractResource, Clone, Copy, Debug, Default)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationLodFocus {
    pub position: Option<Vec3>,
}

/// Canonical XZ offset of render coordinates, for world-anchored wind and shading.
#[derive(Resource, ExtractResource, Default, Clone, Copy, Debug)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationRenderOrigin {
    pub world_xz: [f64; 2],
}

/// Immutable render-facing vegetation snapshot shared by gameplay, editor previews and probes.
#[derive(Resource, ExtractResource, Clone, Debug)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationSceneState {
    // Immutable snapshots are shared with extraction and contact certification.
    // Copying every field on every render extraction scales with resident area.
    scene: Arc<VegetationScene>,
    revision: u64,
    // Page residency changes source packing, but not how stable blade seeds are animated.
    catalog_revision: u64,
    /// One content key per page, or empty when the owner does not track page content.
    page_keys: Arc<[u64]>,
}

impl VegetationSceneState {
    pub fn new(scene: VegetationScene) -> Result<Self, SceneValidationError> {
        scene.validate()?;
        let revision = NEXT_SCENE_REVISION.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            scene: Arc::new(scene),
            revision,
            catalog_revision: revision,
            page_keys: Arc::new([]),
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
        self.replace_with_page_keys(scene, Vec::new())
    }

    /// Replaces the scene with one content key per page (from [`Self::new_page_key`]). An
    /// unchanged key promises unchanged surface and coverage samples, though the page may
    /// have moved with the render origin, so the renderer keeps those samples in place
    /// instead of uploading them again. Without keys every page is uploaded.
    pub fn replace_with_page_keys(
        &mut self,
        scene: VegetationScene,
        page_keys: Vec<u64>,
    ) -> Result<(), SceneValidationError> {
        scene.validate()?;
        debug_assert!(page_keys.is_empty() || page_keys.len() == scene.pages.len());
        let revision = NEXT_SCENE_REVISION.fetch_add(1, Ordering::Relaxed);
        if scene.catalog != self.scene.catalog {
            self.catalog_revision = revision;
        }
        self.scene = Arc::new(scene);
        self.revision = revision;
        self.page_keys = page_keys.into();
        Ok(())
    }

    /// A process-unique page content key for [`Self::replace_with_page_keys`].
    pub fn new_page_key() -> u64 {
        NEXT_PAGE_KEY.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn page_keys(&self) -> &[u64] {
        &self.page_keys
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
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationView;

/// Excludes non-vegetation editor cameras from automatic scene attachment.
#[derive(Component)]
pub struct VegetationViewDisabled;

#[derive(Component, ExtractComponent, Clone, Copy, Debug, Default)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct VegetationDraw;

#[allow(clippy::type_complexity)] // One camera query.
pub(crate) fn attach_default_views(
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

pub(crate) fn maintain_draw_entity(
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
}
