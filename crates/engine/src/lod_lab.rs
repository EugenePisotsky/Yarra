//! LOD lab trees: spawned outside streaming with a cataloged asset's runtime LOD chain (its
//! mesh scenes, and its impostor as a batch of one, as far-object blocks draw it), so each
//! representation can be compared at any distance (`yarra-app-game --lod-lab`).
//!
//! A tree draws as the game would ([`LabRepresentation::Auto`], switching and dissolving
//! over time as every tree with an impostor does) or with one representation forced at
//! every distance. Its [`LabTree`] reports the distance band of each
//! representation under the current LOD projection.
use crate::WorldRenderRoot;
use crate::forest_shadow::NoForestShadow;
use crate::object_lod::{
    CROSSFADE_FRACTION, ForcedLod, IMPOSTOR_HANDOFF_METRES, LodProjection, ScreenSpaceLod,
    ScreenSpaceLodVariant,
};
use crate::tree_impostor::{
    ImpostorBatch, ImpostorBatchDone, ImpostorDescriptor, ImpostorInstance,
};
use bevy::{gltf::GltfAssetLabel, prelude::*};
use std::{ops::Range, path::Path, sync::Arc};

/// Updates lab trees after the user's changes and before object LODs are ranged.
pub struct LodLabPlugin;
impl Plugin for LodLabPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            (apply_representation, report)
                .chain()
                .after(bevy::transform::TransformSystems::Propagate),
        );
    }
}

/// An asset's LOD chain as cataloged: mesh scenes, then optionally an impostor.
#[derive(Clone, Debug)]
pub struct LabAsset {
    pub key: String,
    pub variants: Vec<LabVariant>,
    /// Extents of the largest LOD at unit scale: width (X), height above the root, depth (Z).
    pub bounds: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct LabVariant {
    pub uri: String,
    pub minimum_screen_height: f32,
}

impl LabAsset {
    /// Finds `key` in the catalogs `packs/*/*.catalog.ron`.
    pub fn find(packs: &Path, key: &str) -> Result<Self, String> {
        let pack = key
            .split('/')
            .next()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| format!("asset key {key:?} has no pack"))?;
        let folder = packs.join(pack);
        let entries = std::fs::read_dir(&folder)
            .map_err(|e| format!("cannot read {}: {e}", folder.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.to_string_lossy().ends_with(".catalog.ron") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let catalog: world_db::AssetImportCatalog = ron::from_str(&text)
                .map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
            if let Some(asset) = catalog.assets.into_iter().find(|a| a.key == key) {
                return Self::from_import(asset);
            }
        }
        Err(format!(
            "no catalog in {} has asset {key:?}",
            folder.display()
        ))
    }

    fn from_import(asset: world_db::AssetImport) -> Result<Self, String> {
        let variants: Vec<_> = asset
            .variants
            .iter()
            .map(|v| LabVariant {
                uri: v.uri.clone(),
                minimum_screen_height: v.minimum_screen_height,
            })
            .collect();
        let meshes = variants
            .iter()
            .filter(|v| !world::is_impostor_uri(&v.uri))
            .count();
        if meshes == 0
            || variants[..meshes]
                .iter()
                .any(|v| world::is_impostor_uri(&v.uri))
            || variants.len() > meshes + 1
            || variants
                .windows(2)
                .any(|w| w[1].minimum_screen_height > w[0].minimum_screen_height)
        {
            return Err(format!(
                "asset {:?} needs mesh LODs with descending thresholds, then at most an impostor",
                asset.key
            ));
        }
        let bounds = asset.variants.iter().fold([0.0_f32; 3], |b, v| {
            std::array::from_fn(|i| b[i].max(v.bounds[i]))
        });
        Ok(Self {
            key: asset.key,
            variants,
            bounds,
        })
    }

    pub fn meshes(&self) -> usize {
        self.variants
            .iter()
            .filter(|v| !world::is_impostor_uri(&v.uri))
            .count()
    }

    pub fn impostor(&self) -> Option<&LabVariant> {
        self.variants
            .last()
            .filter(|v| world::is_impostor_uri(&v.uri))
    }

    /// Object height over the last mesh LOD's threshold at unit scale, as far-object pages
    /// record it: times scale and pixels per metre, the impostor's hand-off distance.
    fn impostor_switch(&self) -> Option<f32> {
        self.impostor()?;
        let threshold = self.variants[self.meshes() - 1].minimum_screen_height;
        (threshold > 0.0).then(|| self.bounds[1] / threshold)
    }
}

/// What a lab tree draws.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabRepresentation {
    /// As in the game: the distance picks it.
    Auto,
    /// Mesh LOD `n` (the catalog's variant index) at every distance.
    Mesh(usize),
    /// The impostor at every distance.
    Impostor,
    /// Nothing, e.g. for a background frame.
    Hidden,
}

/// One representation's distances to the root under the current projection: it fades in
/// over `fade_in` (empty for the first) and out over `fade_out` (at `f32::MAX` for the last).
#[derive(Clone, Debug, PartialEq)]
pub struct LabBand {
    pub representation: LabRepresentation,
    pub fade_in: Range<f32>,
    pub fade_out: Range<f32>,
}

/// A lab tree's state, refreshed every frame.
#[derive(Component, Debug)]
pub struct LabTree {
    pub asset: Arc<LabAsset>,
    /// The representations the distance picks between, nearest first. Mesh LODs too close
    /// together for a band of their own are left out, as the game skips them.
    pub bands: Vec<LabBand>,
    /// Every mesh scene has spawned and the impostor (if any) is drawn.
    pub ready: bool,
    /// Logical pixels per metre of object height at one metre.
    pub pixels_per_metre: f32,
    /// The impostor's views (pixels square) and its framing radius (object space).
    pub impostor_view: Option<(u32, f32)>,
    /// Dissolving from one representation to another ([`crate::object_lod`]).
    pub fading: bool,
    batch: Option<Entity>,
}

/// Spawns `asset` at `transform` (render space) drawing as the game would.
pub fn spawn_lab_tree(
    commands: &mut Commands,
    server: &AssetServer,
    asset: Arc<LabAsset>,
    transform: Transform,
) -> Entity {
    let variants = asset
        .variants
        .iter()
        .enumerate()
        .map(|(i, v)| ScreenSpaceLodVariant {
            lod: i as u8,
            scene: (!world::is_impostor_uri(&v.uri))
                .then(|| server.load(GltfAssetLabel::Scene(0).from_asset(v.uri.clone()))),
            minimum_screen_height: v.minimum_screen_height,
        })
        .collect();
    let lod = ScreenSpaceLod::new(variants, asset.bounds[1]);
    // The impostor batch sits at the root without its rotation or scale, which its instance
    // applies, as in far-object blocks. The object's LODs fade it through its slot.
    let batch = asset
        .impostor()
        .zip(asset.impostor_switch())
        .map(|(impostor, switch)| {
            let (scale, translation) = (transform.scale, transform.translation);
            let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
            commands
                .spawn((
                    Transform::from_translation(translation),
                    Visibility::Inherited,
                    WorldRenderRoot,
                    ImpostorBatch {
                        descriptor: server.load(impostor.uri.clone()),
                        instances: vec![ImpostorInstance {
                            translation: Vec3::ZERO,
                            yaw,
                            scale: scale.y,
                            switch: switch * scale.y,
                        }],
                    },
                    Name::new(format!("Lab impostor {}", asset.key)),
                ))
                .id()
        });
    let mut entity = commands.spawn((
        transform,
        Visibility::Inherited,
        WorldRenderRoot,
        ForcedLod::Auto,
        LabRepresentation::Auto,
        Name::new(format!("Lab tree {}", asset.key)),
    ));
    lod.spawn_scenes(&mut entity);
    entity
        .insert((
            lod,
            LabTree {
                asset,
                bands: Vec::new(),
                ready: false,
                pixels_per_metre: 0.0,
                impostor_view: None,
                fading: false,
                batch,
            },
        ))
        .id()
}

fn apply_representation(
    mut commands: Commands,
    mut trees: Query<(&LabRepresentation, &LabTree, &mut ForcedLod), Changed<LabRepresentation>>,
) {
    for (representation, tree, mut forced) in &mut trees {
        // The impostor is the last variant; its object's LODs show it through its slot.
        let impostor = tree.asset.impostor().map(|_| tree.asset.variants.len() - 1);
        *forced = match *representation {
            LabRepresentation::Auto => ForcedLod::Auto,
            LabRepresentation::Mesh(index) if index < tree.asset.meshes() => ForcedLod::Only(index),
            LabRepresentation::Impostor if impostor.is_some() => ForcedLod::Only(impostor.unwrap()),
            LabRepresentation::Mesh(_)
            | LabRepresentation::Impostor
            | LabRepresentation::Hidden => ForcedLod::Nothing,
        };
        // A hidden tree leaves the forest shadows too, so a frame without it is the scene
        // without it.
        if let Some(batch) = tree.batch {
            if *representation == LabRepresentation::Hidden {
                commands.entity(batch).insert(NoForestShadow);
            } else {
                commands.entity(batch).remove::<NoForestShadow>();
            }
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn report(
    projection: Res<LodProjection>,
    descriptors: Res<Assets<ImpostorDescriptor>>,
    images: Res<Assets<Image>>,
    mut trees: Query<(
        &mut LabTree,
        &ScreenSpaceLod,
        &GlobalTransform,
        Option<&Children>,
    )>,
    scene_children: Query<(), With<Children>>,
    done: Query<(), With<ImpostorBatchDone>>,
    batches: Query<&ImpostorBatch>,
) {
    for (mut tree, lod, transform, children) in &mut trees {
        let scale = transform.to_scale_rotation_translation().0.y.abs();
        let height = tree.asset.bounds[1] * scale;
        let thresholds: Vec<f32> = lod.thresholds().collect();
        let farthest = lod.farthest_switch();
        let mut bands = Vec::new();
        if projection.pixels_per_metre() > 0.0 && !projection.orthographic() {
            for index in 0..tree.asset.meshes() {
                let range = projection.range(&thresholds, height, index, farthest);
                if range.end_margin.end <= 0.0 {
                    continue; // skipped: the LOD before it extends to its switch
                }
                bands.push(LabBand {
                    representation: LabRepresentation::Mesh(index),
                    fade_in: range.start_margin,
                    fade_out: range.end_margin,
                });
            }
            if let Some(switch) = tree.asset.impostor_switch() {
                let start =
                    (switch * scale * projection.pixels_per_metre()).min(IMPOSTOR_HANDOFF_METRES);
                bands.push(LabBand {
                    representation: LabRepresentation::Impostor,
                    fade_in: start * (1.0 - CROSSFADE_FRACTION)..start * (1.0 + CROSSFADE_FRACTION),
                    fade_out: f32::MAX..f32::MAX,
                });
            }
        }
        let scenes = children.map_or(0, |c| c.len());
        let impostor_ready = tree.batch.is_none_or(|batch| done.contains(batch));
        let ready = scenes == tree.asset.meshes()
            && children
                .into_iter()
                .flatten()
                .all(|&scene| scene_children.contains(scene))
            && impostor_ready;
        // The atlas holds each view's crop: its tile width over the crop's share of the view.
        let impostor_view = tree.batch.and_then(|batch| {
            let descriptor = descriptors.get(&batches.get(batch).ok()?.descriptor)?;
            let width = images.get(&descriptor.albedo)?.width() as f32;
            let share = (descriptor.crop.z - descriptor.crop.x).max(1e-3);
            Some((
                (width / descriptor.views.max(1) as f32 / share).round() as u32,
                descriptor.radius,
            ))
        });
        let fading = lod.fading();
        if tree.bands != bands
            || tree.ready != ready
            || tree.pixels_per_metre != projection.pixels_per_metre()
            || tree.impostor_view != impostor_view
            || tree.fading != fading
        {
            tree.bands = bands;
            tree.ready = ready;
            tree.pixels_per_metre = projection.pixels_per_metre();
            tree.impostor_view = impostor_view;
            tree.fading = fading;
        }
    }
}
