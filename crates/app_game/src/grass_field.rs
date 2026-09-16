//! Playable field trial. The published catalog is the new setup; G compares the
//! previous 44-root catalog in the same camera/light, with normal geometry LOD.
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use engine::{StreamedTerrainSurface, StreamedVegetationFieldPage, WorldCatalog, WorldOrigin};
use std::collections::HashMap;
use terrain_render::{TerrainMaterial, TerrainMaterialPreparation};
use vegetation::VegetationCatalog;
use vegetation_render::canopy_coverage as canopy;
use vegetation_render::{VegetationDebugScene, VegetationDebugSettings};

#[derive(Resource)]
pub(super) struct GrassFieldTrial {
    pub enabled: bool,
    pub baseline: VegetationCatalog,
    canopy_look: vegetation::CanopyShading,
}

pub(super) fn install(app: &mut App) {
    let baseline = ron::from_str(include_str!(
        "../../../content/vegetation/field-baseline.ron"
    ))
    .expect("valid previous grass catalog");
    app.insert_resource(GrassFieldTrial {
        enabled: !std::env::args_os().any(|a| a == "--grass-field-baseline"),
        baseline,
        canopy_look: Default::default(),
    })
    .init_resource::<GroundTiles>()
    .add_systems(Startup, (setup, load_canopy_look))
    .add_systems(Update, reload_canopy_look)
    .add_systems(
        Update,
        toggle.before(super::conform_vegetation_debug_to_streamed_terrain),
    )
    .add_systems(
        Update,
        sync_ground
            .after(super::conform_vegetation_debug_to_streamed_terrain)
            .before(TerrainMaterialPreparation),
    )
    .add_systems(Update, status.after(sync_ground))
    .add_systems(
        PostUpdate,
        update_lod_focus.after(TransformSystems::Propagate),
    );
    let mut args = std::env::args();
    if args.any(|a| a == "--grass-density") {
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .density_mode = match args.next().as_deref() {
            Some("balanced") => vegetation_render::VegetationDensityMode::Balanced,
            Some("full") => vegetation_render::VegetationDensityMode::FullReference,
            Some("authored") => vegetation_render::VegetationDensityMode::Authored,
            _ => panic!("--grass-density requires balanced, full or authored"),
        };
    }
}

fn update_lod_focus(
    viewpoint: Res<engine::WorldViewpoint>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    mut focus: ResMut<vegetation_render::VegetationLodFocus>,
    mut lighting: ResMut<vegetation_render::VegetationLighting>,
) {
    if let Some(space) = origin.space().and_then(|id| catalog.world_space(id)) {
        let offset = origin.cell().origin(space.cell_size).map(|v| v as f32);
        if lighting.canopy_origin != offset {
            lighting.canopy_origin = offset;
        }
    }
    focus.position = viewpoint.position().and_then(|position| {
        if Some(position.space) != origin.space() {
            return None;
        }
        let space = catalog.world_space(position.space)?;
        Some(Vec3::from_array(
            position.relative_to(origin.cell(), space.cell_size),
        ))
    });
}

#[derive(Component)]
struct FieldStatus;
fn setup(mut commands: Commands) {
    commands.spawn((
        Text::new("Grass field loading"),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0.025, 0.03, 0.04, 0.75)),
        Node {
            position_type: PositionType::Absolute,
            left: px(14),
            bottom: px(14),
            padding: UiRect::all(px(6)),
            ..default()
        },
        FieldStatus,
    ));
}
fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mut trial: ResMut<GrassFieldTrial>,
    mut lighting: ResMut<vegetation_render::VegetationLighting>,
) {
    if keys.just_pressed(KeyCode::KeyG) {
        trial.enabled = !trial.enabled;
        lighting.canopy = trial.canopy_look;
        lighting.canopy.enabled &= trial.enabled;
        warn!(
            "Grass field: {}",
            if trial.enabled {
                "new published setup"
            } else {
                "previous 44-root setup"
            }
        );
    }
}
fn status(
    trial: Res<GrassFieldTrial>,
    scene: Res<VegetationDebugScene>,
    tiles: Res<GroundTiles>,
    settings: Res<VegetationDebugSettings>,
    mut text: Single<&mut Text, With<FieldStatus>>,
) {
    let ready = tiles
        .entries
        .values()
        .filter(|e| e.image.is_some() && !e.needs_bake)
        .count();
    let density = scene
        .scene()
        .catalog
        .populations
        .iter()
        .find(|population| population.key == "short_split_fill")
        .map(|population| population.density_per_square_meter);
    let field = if trial.enabled {
        "published"
    } else {
        "previous"
    };
    let field = density.map_or_else(
        || format!("{field} field"),
        |density| format!("{field} {density}-root field"),
    );
    let label = format!(
        "Grass: {} | G: compare\nDensity: {} | O: cycle | Detail follows character\nGround coverage: {ready}/{} tiles ready",
        field,
        settings.density_mode.label(),
        tiles.entries.len()
    );
    if text.0 != label {
        **text = Text::new(label);
    }
}

#[derive(Default, Resource)]
struct GroundTiles {
    entries: HashMap<Entity, Tile>,
    catalog: Option<VegetationCatalog>,
    revision: Option<u64>,
    settle_until: f64,
    applied_enabled: Option<bool>,
}
struct Tile {
    material: Handle<TerrainMaterial>,
    dependencies: Vec<Entity>,
    source_indices: Vec<usize>,
    origin: [f32; 2],
    extent: f32,
    image: Option<Handle<Image>>,
    bounds: Vec4,
    task: Option<Task<canopy::Bake>>,
    needs_bake: bool,
}

fn sync_ground(
    lighting: Res<vegetation_render::VegetationLighting>,
    trial: Res<GrassFieldTrial>,
    scene: Res<VegetationDebugScene>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    time: Res<Time>,
    terrain: Query<(
        Entity,
        &StreamedTerrainSurface,
        &MeshMaterial3d<TerrainMaterial>,
    )>,
    fields: Query<(Entity, &StreamedVegetationFieldPage)>,
    mut tiles: ResMut<GroundTiles>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    // Disable means no canopy source traversal, new bakes or texture uploads.
    if !trial.enabled
        || !lighting.canopy.enabled
        || lighting.canopy.strength <= 0.0
        || lighting.canopy.ground_amount <= 0.0
    {
        if tiles.applied_enabled != Some(false) {
            for tile in tiles.entries.values_mut() {
                if tile.task.take().is_some() {
                    tile.needs_bake = true;
                }
                if let Some(mut material) = materials.get_mut(&tile.material) {
                    material.set_canopy_coverage(None, Vec4::ZERO);
                }
            }
        }
        tiles.applied_enabled = Some(false);
        tiles.entries.retain(|entity, tile| {
            if terrain.get(*entity).is_ok() {
                return true;
            }
            if let Some(image) = &tile.image {
                images.remove(image.id());
            }
            false
        });
        return;
    }
    let Some(source_catalog) = catalog.vegetation() else {
        return;
    };
    if tiles.catalog.as_ref() != Some(source_catalog) {
        for tile in tiles.entries.values() {
            if let Some(image) = &tile.image {
                images.remove(image.id());
            }
            if let Some(mut material) = materials.get_mut(&tile.material) {
                material.set_canopy_coverage(None, Vec4::ZERO);
            }
        }
        tiles.entries.clear();
        tiles.catalog = Some(source_catalog.clone());
        tiles.revision = None;
    }
    // Expired pages release their masks/tasks as part of normal streaming.
    tiles.entries.retain(|entity, tile| {
        if terrain.get(*entity).is_ok() {
            return true;
        }
        if let Some(image) = &tile.image {
            images.remove(image.id());
        }
        false
    });
    if tiles.revision != Some(scene.revision()) {
        tiles.revision = Some(scene.revision());
        tiles.settle_until = time.elapsed_secs_f64() + 0.35;
        let pages = &scene.scene().pages;
        let field_entities: HashMap<_, _> = fields
            .iter()
            .filter(|(_, f)| Some(f.key.space) == origin.space())
            .map(|(e, f)| {
                let min = f.key.cell.origin(f.cell_size);
                let base = origin.cell().origin(f.cell_size);
                (
                    [
                        ((min[0] - base[0]) as f32).to_bits(),
                        ((min[1] - base[1]) as f32).to_bits(),
                    ],
                    e,
                )
            })
            .collect();
        for (entity, surface, handle) in &terrain {
            if Some(surface.key.space) != origin.space() {
                continue;
            }
            let min = surface.key.cell.origin(surface.cell_size);
            let base = origin.cell().origin(surface.cell_size);
            let min = [(min[0] - base[0]) as f32, (min[1] - base[1]) as f32];
            let near: Vec<_> = pages
                .iter()
                .enumerate()
                .filter(|(_, page)| {
                    (0..2).all(|i| {
                        page.origin_xz[i]
                            < min[i]
                                + surface.cell_size
                                + vegetation_render::canopy_coverage::MARGIN
                            && page.origin_xz[i] + page.size
                                > min[i] - vegetation_render::canopy_coverage::MARGIN
                    })
                })
                .collect();
            let mut dependencies: Vec<_> = near
                .iter()
                .filter_map(|(_, p)| field_entities.get(&p.origin_xz.map(f32::to_bits)).copied())
                .collect();
            dependencies.sort();
            let indices = near.iter().map(|(index, _)| *index).collect();
            let tile = tiles.entries.entry(entity).or_insert_with(|| Tile {
                material: handle.0.clone(),
                dependencies: Vec::new(),
                source_indices: Vec::new(),
                origin: min,
                extent: surface.cell_size,
                image: None,
                bounds: Vec4::ZERO,
                task: None,
                needs_bake: true,
            });
            if tile.dependencies != dependencies {
                tile.dependencies = dependencies;
                tile.task = None;
                tile.needs_bake = true;
            }
            tile.source_indices = indices;
        }
    }
    let toggled = tiles.applied_enabled != Some(trial.enabled);
    tiles.applied_enabled = Some(trial.enabled);
    for tile in tiles.entries.values_mut() {
        let complete = tile.task.as_mut().and_then(check_ready);
        let changed = complete.is_some();
        if let Some(bake) = complete {
            tile.task = None;
            tile.needs_bake = false;
            tile.bounds = bake.bounds;
            if let Some(image) = &tile.image {
                images
                    .insert(image.id(), bake.image)
                    .expect("resident canopy handle");
            } else {
                tile.image = Some(images.add(bake.image));
            }
        }
        if (toggled || changed || lighting.is_changed())
            && let Some(mut material) = materials.get_mut(&tile.material)
        {
            material.set_canopy_shading(lighting.canopy.packed(lighting.canopy_origin));
            material.set_canopy_coverage(
                if trial.enabled {
                    tile.image.clone()
                } else {
                    None
                },
                tile.bounds,
            );
        }
    }
    if !trial.enabled || time.elapsed_secs_f64() < tiles.settle_until {
        return;
    }
    // A bounded queue keeps source baking off the render/update thread. Cached
    // results survive G comparisons and are rebuilt only for changed source pages.
    let active = tiles.entries.values().filter(|e| e.task.is_some()).count();
    let mut pending: Vec<_> = tiles
        .entries
        .iter()
        .filter(|(_, e)| e.needs_bake && e.task.is_none())
        .map(|(key, e)| {
            (
                *key,
                (e.origin[0] + e.extent * 0.5).powi(2) + (e.origin[1] + e.extent * 0.5).powi(2),
            )
        })
        .collect();
    pending.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (entity, _) in pending.into_iter().take(2usize.saturating_sub(active)) {
        let tile = tiles.entries.get_mut(&entity).unwrap();
        let neighbors: Vec<_> = tile
            .source_indices
            .iter()
            .map(|&i| scene.scene().pages[i].clone())
            .collect();
        let catalog = source_catalog.clone();
        let (min, size) = (tile.origin, tile.extent);
        tile.task = Some(
            AsyncComputeTaskPool::get()
                .spawn(async move { canopy::bake(&catalog, &neighbors, min, size) }),
        );
    }
}

fn load_canopy_look(
    mut trial: ResMut<GrassFieldTrial>,
    mut lighting: ResMut<vegetation_render::VegetationLighting>,
) {
    let mut args = std::env::args();
    let explicit = args.any(|a| a == "--canopy-look");
    let path = if explicit {
        std::path::PathBuf::from(args.next().expect("--canopy-look requires a path"))
    } else {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/vegetation/canopy-look.ron")
    };
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| ron::from_str::<vegetation::CanopyShading>(&s).map_err(|e| e.to_string()))
    {
        Ok(look) => {
            trial.canopy_look = look;
            lighting.canopy = look;
            lighting.canopy.enabled &= trial.enabled;
            info!("Loaded canopy look; H reloads editor changes");
        }
        Err(error) if explicit => panic!("Requested canopy look {}: {error}", path.display()),
        Err(error) => warn!("Canopy look {}: {error}", path.display()),
    }
}
fn reload_canopy_look(
    keys: Res<ButtonInput<KeyCode>>,
    trial: ResMut<GrassFieldTrial>,
    lighting: ResMut<vegetation_render::VegetationLighting>,
) {
    if keys.just_pressed(KeyCode::KeyH) {
        load_canopy_look(trial, lighting);
    }
}
