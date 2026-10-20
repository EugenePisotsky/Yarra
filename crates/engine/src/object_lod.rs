//! Visual-object LOD shared by streaming and editor previews.
//!
//! Every LOD of an object is its own child scene. Bevy's visibility ranges show the LOD
//! whose distance band contains the camera and dither neighbouring LODs into each other
//! across a short band around every switch, so LOD changes dissolve instead of popping.
//! Shadow cascades resolve the same ranges from the main camera, but Bevy's shadow pass
//! does not dither: inside a band both LODs cast whole shadows, and the shadow changes
//! shape at the band's ends. Objects whose last LOD is an impostor (trees and shrubs) are
//! not drawn here: the GPU trees draw every representation of them (`crate::trees`).
use crate::{ActiveWorldView, WorldViewCamera};

use bevy::{
    camera::{
        ShadowLodOrigin,
        visibility::{VisibilityRange, VisibilitySystems},
    },
    gltf::GltfAssetLabel,
    prelude::*,
    transform::TransformSystems,
    window::PrimaryWindow,
    world_serialization::WorldInstanceReady,
};

use world::{CellCoord, StableObjectId, WorldSpaceId};

/// Each switch crossfades between 90% and 110% of its switch distance.
pub(crate) const CROSSFADE_FRACTION: f32 = 0.1;

/// Streamed cells keep their objects resident within this distance of the camera
/// (`world_streaming::source_demand`).
pub(crate) const OBJECT_RESIDENCY_METRES: f32 = 192.0;

/// Installs object LOD ranges after transform propagation. Requires a WorldViewCamera.
/// WorldStreamingPlugin includes this plugin for both game and editor applications.
/// Applications without streaming can install it for collection previews.
pub struct ObjectLodPlugin;

impl Plugin for ObjectLodPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::trees::impostor::TreeImpostorPlugin)
            .init_resource::<VisualLodScale>()
            .init_resource::<LodProjection>()
            .add_observer(range_new_lod_scene)
            .add_observer(shadow_lods_follow_world_view)
            .add_systems(
                PostUpdate,
                update_object_lods
                    .after(TransformSystems::Propagate)
                    .before(VisibilitySystems::CheckVisibility),
            );
    }
}

/// An object whose LODs are child scenes; see [`ScreenSpaceLod::spawn_scenes`].
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

impl ScreenSpaceLodVariant {
    /// A cataloged mesh LOD's scene.
    pub(crate) fn load(
        server: &AssetServer,
        lod: u8,
        uri: &str,
        minimum_screen_height: f32,
    ) -> Self {
        debug_assert!(!world::is_impostor_uri(uri), "impostors draw on the GPU");
        Self {
            lod,
            scene: server.load(GltfAssetLabel::Scene(0).from_asset(uri.to_owned())),
            minimum_screen_height,
        }
    }
}

/// A child scene holding one LOD (by index) of its parent's [`ScreenSpaceLod`].
#[derive(Component)]
pub(crate) struct LodScene(pub(crate) usize);

/// Overrides which LOD scene of a [`ScreenSpaceLod`] draws, for comparisons
/// (the LOD lab, `crate::trees::lab`).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForcedLod {
    /// The distance picks the LOD, as for every other object.
    Auto,
    /// Only this variant (by index) draws, at any distance.
    Only(usize),
    /// No mesh LOD draws.
    Nothing,
}

const NEVER: VisibilityRange = VisibilityRange {
    start_margin: 0.0..0.0,
    end_margin: 0.0..0.0,
    use_aabb: false,
};

const ALWAYS: VisibilityRange = VisibilityRange {
    start_margin: 0.0..0.0,
    end_margin: f32::MAX..f32::MAX,
    use_aabb: false,
};

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

    /// Spawns one child scene per LOD under `entity`. ObjectLodPlugin gives their meshes
    /// visibility ranges; without it every LOD would draw.
    pub(crate) fn spawn_scenes(&self, entity: &mut EntityCommands) {
        entity.with_children(|children| {
            for (index, variant) in self.variants.iter().enumerate() {
                children.spawn((
                    WorldAssetRoot(variant.scene.clone()),
                    LodScene(index),
                    Name::new(format!("LOD{}", variant.lod)),
                ));
            }
        });
    }

    /// The LOD whose band contains the camera (ignoring crossfades), for statistics.
    pub(crate) fn current_lod(&self) -> u8 {
        self.variants[self.current].lod
    }

    #[cfg(test)]
    pub(crate) fn projected_height(&self) -> f32 {
        self.projected_height
    }

    pub(crate) fn variants(&self) -> &[ScreenSpaceLodVariant] {
        &self.variants
    }

    pub(crate) fn thresholds(&self) -> impl Iterator<Item = f32> + '_ {
        self.variants.iter().map(|v| v.minimum_screen_height)
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

/// Logical pixels per metre of object height at one metre from the camera (anywhere, for
/// an orthographic camera), including [`VisualLodScale`]. Zero until the camera is known.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub(crate) struct LodProjection {
    pixels_per_metre: f32,
    orthographic: bool,
    /// Output pixels per logical pixel: the window's scale factor, whatever the render scale
    /// (an upscaler resolves to the output).
    output_scale: f32,
}

impl Default for LodProjection {
    fn default() -> Self {
        Self {
            pixels_per_metre: 0.0,
            orthographic: false,
            output_scale: 1.0,
        }
    }
}

impl LodProjection {
    /// A perspective projection with `pixels_per_metre`, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn perspective(pixels_per_metre: f32, output_scale: f32) -> Self {
        Self {
            pixels_per_metre,
            orthographic: false,
            output_scale,
        }
    }

    pub(crate) fn pixels_per_metre(&self) -> f32 {
        self.pixels_per_metre
    }

    pub(crate) fn orthographic(&self) -> bool {
        self.orthographic
    }

    pub(crate) fn output_scale(&self) -> f32 {
        self.output_scale
    }

    /// The projected height below which an impostor whose views hold `texels_per_metre`
    /// texels per metre has one texel or more per output pixel, for an object `height`
    /// metres tall. Independent of the object's scale and of distance; Object detail moves
    /// it out with every other switch.
    pub(crate) fn impostor_threshold(&self, height: f32, texels_per_metre: f32) -> f32 {
        height * texels_per_metre / self.output_scale
    }

    /// Distance at which an object `height` metres tall projects to `threshold` pixels; a
    /// threshold of 0 (the last LOD) never switches.
    pub(crate) fn switch_distance(&self, height: f32, threshold: f32) -> f32 {
        if threshold > 0.0 {
            height * self.pixels_per_metre / threshold
        } else {
            f32::INFINITY
        }
    }

    /// Projected height of an object `height` metres tall, `distance` metres away. Seen
    /// side-on at its distance, so pitching the camera never changes it (projecting the
    /// root-to-top segment shrank when the camera looked down).
    fn projected(&self, height: f32, distance: f32) -> f32 {
        if self.orthographic {
            height * self.pixels_per_metre
        } else {
            height * self.pixels_per_metre / distance.max(0.01)
        }
    }

    /// Visibility range of LOD `index`: shown between the distances where the object's
    /// projected height crosses the previous LOD's threshold and its own, crossfading over
    /// [`CROSSFADE_FRACTION`] of each switch distance.
    ///
    /// A LOD whose switches lie closer together than the crossfade needs is skipped, and
    /// the LOD before it keeps drawing until the skipped LOD's own switch: the next LOD
    /// (or the impostor, which fades in there) must meet a LOD fading out, never a gap.
    pub(crate) fn range(&self, thresholds: &[f32], height: f32, index: usize) -> VisibilityRange {
        if self.orthographic {
            // Size does not change with distance: one LOD is always the right one.
            return if select(thresholds, self.projected(height, 1.0)) == index {
                ALWAYS
            } else {
                NEVER
            };
        }

        let switch = |i: usize| self.switch_distance(height, thresholds[i]);
        let fits = |start: f32, end: f32| crossfade(start).end <= crossfade(end).start;

        // A LOD starts at the previous switch, whether the LOD before drew up to it or
        // was skipped and extended its own predecessor there.
        let start = if index == 0 { 0.0 } else { switch(index - 1) };

        if index > 0 && !fits(start, switch(index)) {
            return NEVER;
        }

        // Later LODs too close to fit a band of their own extend this one.
        let mut end = switch(index);

        for i in index + 1..thresholds.len() {
            if fits(end, switch(i)) {
                break;
            }
            end = switch(i);
        }

        VisibilityRange {
            start_margin: if index == 0 {
                0.0..0.0
            } else {
                crossfade(start)
            },
            end_margin: if end.is_finite() {
                crossfade(end)
            } else {
                f32::MAX..f32::MAX
            },
            use_aabb: false,
        }
    }
}

/// The distances over which a LOD switching at `distance` crossfades.
pub(crate) fn crossfade(distance: f32) -> std::ops::Range<f32> {
    distance * (1.0 - CROSSFADE_FRACTION)..distance * (1.0 + CROSSFADE_FRACTION)
}

fn select(thresholds: &[f32], projected_height: f32) -> usize {
    thresholds
        .iter()
        .position(|&minimum| projected_height >= minimum)
        .unwrap_or(thresholds.len() - 1)
}

fn object_height(lod: &ScreenSpaceLod, scale: Vec3) -> f32 {
    lod.bounds_height * scale.y.abs()
}

/// The visibility range of LOD `index`, unless a [`ForcedLod`] overrides it.
fn object_range(
    forced: Option<ForcedLod>,
    projection: &LodProjection,
    thresholds: &[f32],
    height: f32,
    index: usize,
) -> VisibilityRange {
    match forced.unwrap_or(ForcedLod::Auto) {
        ForcedLod::Auto => projection.range(thresholds, height, index),
        ForcedLod::Only(only) if only == index => ALWAYS,
        ForcedLod::Only(_) | ForcedLod::Nothing => NEVER,
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_object_lods(
    mut commands: Commands,
    lod_scale: Res<VisualLodScale>,
    mut projection: ResMut<LodProjection>,
    view: ActiveWorldView,
    mut objects: Query<(
        &GlobalTransform,
        &mut ScreenSpaceLod,
        Option<&Children>,
        Option<Ref<ForcedLod>>,
    )>,
    scenes: Query<(&LodScene, Option<&Children>)>,
    descendants: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(view) = view.current() else {
        return;
    };
    let (camera, camera_transform) = (view.camera, view.transform);
    let Some(viewport) = camera.logical_viewport_size() else {
        return;
    };
    let clip = camera.clip_from_view();
    let next = LodProjection {
        pixels_per_metre: 0.5 * viewport.y * clip.y_axis.y * lod_scale.0.clamp(0.25, 4.0),
        orthographic: clip.w_axis.w == 1.0,
        // The game renders into an image at its render scale; the window is the output.
        output_scale: window
            .single()
            .map(Window::scale_factor)
            .ok()
            .or_else(|| camera.target_scaling_factor())
            .unwrap_or(1.0),
    };

    // Window size, display, field of view and Object detail change the switch distances;
    // they change rarely, so only then are ranges rewritten.
    let changed = next.orthographic != projection.orthographic
        || next.output_scale != projection.output_scale
        || (next.pixels_per_metre - projection.pixels_per_metre).abs()
            > 0.005 * projection.pixels_per_metre.max(f32::EPSILON);

    if changed {
        *projection = next;
    }

    let eye = camera_transform.translation();

    for (transform, mut lod, children, forced) in &mut objects {
        let (scale, _, translation) = transform.to_scale_rotation_translation();
        let height = object_height(&lod, scale);
        let centre = translation + Vec3::Y * height * 0.5;
        lod.projected_height = projection.projected(height, eye.distance(centre));
        let thresholds: Vec<f32> = lod.thresholds().collect();
        lod.current = select(&thresholds, lod.projected_height);

        if !changed && !forced.as_ref().is_some_and(Ref::is_changed) {
            continue;
        }

        let forced = forced.map(|f| *f);

        for &child in children.into_iter().flatten() {
            if let Ok((level, scene_children)) = scenes.get(child) {
                let range = object_range(forced, &projection, &thresholds, height, level.0);
                for &scene_child in scene_children.into_iter().flatten() {
                    apply_range(&mut commands, scene_child, &range, &descendants, &meshes);
                }
            }
        }
    }
}

/// Shadow passes dither LODs by distance from Bevy's shadow LOD origin, while CPU culling
/// picks shadow casters by distance from the world camera. Without an explicit origin
/// Bevy uses the camera that renders to the window: the UI camera at the world origin,
/// since world views render to an image. Casters near the world camera were then dithered
/// away, and shadows vanished as the camera moved.
fn shadow_lods_follow_world_view(added: On<Add<WorldViewCamera>>, mut commands: Commands) {
    commands.entity(added.entity).insert(ShadowLodOrigin);
}

/// A LOD scene spawns its meshes asynchronously; range them as soon as they exist.
#[allow(clippy::too_many_arguments)]
fn range_new_lod_scene(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    projection: Res<LodProjection>,
    scenes: Query<(&LodScene, &ChildOf, Option<&Children>)>,
    objects: Query<(&ScreenSpaceLod, &Transform, Option<&ForcedLod>)>,
    descendants: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
) {
    let Ok((level, parent, children)) = scenes.get(ready.entity) else {
        return;
    };
    let Ok((lod, transform, forced)) = objects.get(parent.parent()) else {
        return;
    };

    if projection.pixels_per_metre <= 0.0 {
        return; // update_object_lods ranges every scene once the camera is known
    }

    let thresholds: Vec<f32> = lod.thresholds().collect();
    let range = object_range(
        forced.copied(),
        &projection,
        &thresholds,
        object_height(lod, transform.scale),
        level.0,
    );

    for &child in children.into_iter().flatten() {
        apply_range(&mut commands, child, &range, &descendants, &meshes);
    }
}

fn apply_range(
    commands: &mut Commands,
    entity: Entity,
    range: &VisibilityRange,
    descendants: &Query<&Children>,
    meshes: &Query<(), With<Mesh3d>>,
) {
    for entity in std::iter::once(entity).chain(descendants.iter_descendants(entity)) {
        if meshes.contains(entity) {
            commands.entity(entity).try_insert(range.clone());
        }
    }
}

/// Marks cooked generated objects so authoring can replace only the derived cell output.
#[derive(Component)]
pub struct GeneratedEnvironmentObject {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
}

/// Editor world previews share the runtime object's mesh LODs. Previews have no impostor
/// batches, so their last mesh LOD stays visible at any distance.
pub fn spawn_collection_visual(
    commands: &mut Commands,
    server: &AssetServer,
    transform: Transform,
    asset: &world_db::CollectionAssetView,
) -> Entity {
    let mut variants: Vec<_> = asset
        .variants
        .iter()
        .filter(|v| !world::is_impostor_uri(&v.uri))
        .map(|v| ScreenSpaceLodVariant::load(server, v.lod, &v.uri, v.minimum_screen_height))
        .collect();

    if let Some(last) = variants.last_mut() {
        last.minimum_screen_height = 0.0;
    }

    let lod = ScreenSpaceLod::new(
        variants,
        asset
            .variants
            .iter()
            .map(|v| v.bounds[1])
            .fold(0.0_f32, f32::max),
    );
    let mut entity = commands.spawn((
        transform,
        Visibility::default(),
        Name::new(format!("Generated {}", asset.name)),
    ));
    lod.spawn_scenes(&mut entity);
    entity.insert(lod).id()
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
