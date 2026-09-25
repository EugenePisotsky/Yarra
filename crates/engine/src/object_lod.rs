//! Visual-object LOD selection shared by streaming and editor previews.
use crate::WorldViewCamera;
use bevy::{gltf::GltfAssetLabel, prelude::*, transform::TransformSystems};
use world::{CellCoord, StableObjectId, WorldSpaceId};

const MAX_LOD_SWITCHES_PER_FRAME: usize = 32;
const LOD_HYSTERESIS_FRACTION: f32 = 0.12;

/// Installs object LOD updates after transform propagation. Requires a WorldViewCamera.
/// WorldStreamingPlugin includes this plugin for both game and editor applications.
/// Applications without streaming can install it for collection previews.
pub struct ObjectLodPlugin;
impl Plugin for ObjectLodPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VisualLodScale>().add_systems(
            PostUpdate,
            update_screen_space_lods.after(TransformSystems::Propagate),
        );
    }
}

#[derive(Component)]
pub(crate) struct ScreenSpaceLod {
    variants: Vec<ScreenSpaceLodVariant>,
    current: usize,
    bounds_height: f32,
    projected_height: f32,
}

pub(crate) struct ScreenSpaceLodVariant {
    pub(crate) lod: u8,
    pub(crate) scene: Handle<WorldAsset>,
    pub(crate) minimum_screen_height: f32,
}

impl ScreenSpaceLod {
    /// Callers validate nonempty variants and descending thresholds before attachment.
    pub(crate) fn new(variants: Vec<ScreenSpaceLodVariant>, bounds_height: f32) -> Self {
        assert!(
            !variants.is_empty(),
            "object LOD requires at least one validated variant"
        );
        Self {
            current: variants.len() - 1,
            variants,
            bounds_height,
            projected_height: 0.0,
        }
    }

    pub(crate) fn scene_root(&self) -> WorldAssetRoot {
        WorldAssetRoot(self.variants[self.current].scene.clone())
    }

    pub(crate) fn current_lod(&self) -> u8 {
        self.variants[self.current].lod
    }
    pub(crate) fn projected_height(&self) -> f32 {
        self.projected_height
    }
    pub(crate) fn variants(&self) -> &[ScreenSpaceLodVariant] {
        &self.variants
    }
}

/// Multiplies projected size for visual LOD selection; collision is unchanged.
#[derive(Resource, Clone, Copy, Debug)]
pub struct VisualLodScale(pub f32);
impl Default for VisualLodScale {
    fn default() -> Self {
        Self(1.0)
    }
}

fn update_screen_space_lods(
    lod_scale: Res<VisualLodScale>,
    camera: Single<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    mut objects: Query<(&GlobalTransform, &mut WorldAssetRoot, &mut ScreenSpaceLod)>,
) {
    let (camera, camera_transform) = *camera;
    let mut switches = 0;
    for (transform, mut scene_root, mut screen_lod) in &mut objects {
        let (scale, _, translation) = transform.to_scale_rotation_translation();
        let bottom = translation;
        let top = translation + Vec3::Y * screen_lod.bounds_height * scale.y.abs();
        let (Ok(bottom), Ok(top)) = (
            camera.world_to_viewport(camera_transform, bottom),
            camera.world_to_viewport(camera_transform, top),
        ) else {
            continue;
        };
        let projected_height = bottom.distance(top);
        if !projected_height.is_finite() {
            continue;
        }
        screen_lod.projected_height = projected_height;

        let current = screen_lod.current;
        let target = select_lod_index(
            screen_lod.variants.len(),
            current,
            projected_height * lod_scale.0.clamp(0.25, 4.0),
            |index| screen_lod.variants[index].minimum_screen_height,
        );
        if target == current {
            continue;
        }
        if switches >= MAX_LOD_SWITCHES_PER_FRAME {
            continue;
        }

        scene_root.0 = screen_lod.variants[target].scene.clone();
        screen_lod.current = target;
        switches += 1;
    }
}

fn select_lod_index(
    variant_count: usize,
    current: usize,
    projected_height: f32,
    minimum_screen_height: impl Fn(usize) -> f32,
) -> usize {
    debug_assert!(variant_count > 0 && current < variant_count);
    let raw_target = (0..variant_count)
        .find(|index| projected_height >= minimum_screen_height(*index))
        .unwrap_or(variant_count - 1);
    if raw_target > current {
        let downgrade_below = minimum_screen_height(current) * (1.0 - LOD_HYSTERESIS_FRACTION);
        if projected_height >= downgrade_below {
            current
        } else {
            raw_target
        }
    } else if raw_target < current {
        let upgrade_above = minimum_screen_height(raw_target) * (1.0 + LOD_HYSTERESIS_FRACTION);
        if projected_height <= upgrade_above {
            current
        } else {
            raw_target
        }
    } else {
        current
    }
}

/// Marks cooked generated objects so authoring can replace only the derived cell output.
#[derive(Component)]
pub struct GeneratedEnvironmentObject {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
}

/// Editor world previews share the runtime object's screen-space LOD selection.
pub fn spawn_collection_visual(
    commands: &mut Commands,
    server: &AssetServer,
    transform: Transform,
    asset: &world_db::CollectionAssetView,
) -> Entity {
    let variants: Vec<_> = asset
        .variants
        .iter()
        .map(|v| ScreenSpaceLodVariant {
            lod: v.lod,
            scene: server.load(GltfAssetLabel::Scene(0).from_asset(v.uri.clone())),
            minimum_screen_height: v.minimum_screen_height,
        })
        .collect();
    let lod = ScreenSpaceLod::new(
        variants,
        asset
            .variants
            .iter()
            .map(|v| v.bounds[1])
            .fold(0.0_f32, f32::max),
    );
    commands
        .spawn((
            lod.scene_root(),
            transform,
            lod,
            Name::new(format!("Generated {}", asset.name)),
        ))
        .id()
}

#[derive(Component, Debug, Clone, Copy)]
pub struct StreamedVisualObject {
    pub id: StableObjectId,
}

/// Unscaled extent of a placed object's largest LOD: half widths in X/Z, and height above its
/// root. The root transform's uniform scale applies. Used for rain shelter.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct ObjectFootprint {
    pub half_extent: Vec2,
    pub height: f32,
}

#[cfg(test)]
mod tests;
