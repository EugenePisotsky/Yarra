//! Island worlds built from a height function: the procedural Phase 0 island and imported
//! heightfields share their catalog, cells, paint and start views.
use super::*;
use environment::CoverageTile;
use world::WorldViewBookmark;
use world_db::{ImportedTerrainCell, SourceEnvironmentCellRecord};

/// 1 m geometry and paint; ground weights are still compiled at 0.5 m.
pub(crate) const HEIGHT_SIDE: usize = 33;
pub(crate) const MASK_SIDE: usize = 33;
/// Composites stop at 128 m nodes (2 m texels); closer ground uses runtime near detail.
const COMPOSITE_MINIMUM_LEVEL: u8 = 2;
const VIEW_VISIBILITY: f32 = 20_000.;

/// Terrain height at a world XZ position.
pub(crate) type HeightFn<'a> = dyn Fn(Vec2) -> f32 + Sync + 'a;
/// A terrain tool's mask at a world XZ position, from 0 to 1.
pub(crate) type MaskFn<'a> = dyn Fn(Vec2) -> f32 + Sync + 'a;

/// Masks every island world has, generated from height and slope by `paint`. An imported
/// mask of the same name replaces one.
const GENERATED_MASKS: [&str; 11] = [
    "land",
    "green",
    "bare",
    "coast",
    "pine_forest",
    "spruce_forest",
    "broadleaf_forest",
    "dune",
    "beach",
    "scree",
    "rock",
];

/// An island ground layer above the meadows: its name, the mask it reads, its surfaces and
/// the share of meadow grass it keeps.
struct IslandGround {
    name: &'static str,
    mask: &'static str,
    surfaces: &'static [(&'static str, f32)],
    grass: f32,
}

/// Island ground layers, lowest first. Houdini supplies sand, rock and scree;
/// `tools/island_masks.py` derives the coast, dunes, beach and forest floors from them and
/// from `tools/forest_plan.py`'s stands.
const ISLAND_GROUND: [IslandGround; 8] = [
    IslandGround {
        name: "Coastal grass",
        mask: "coast",
        surfaces: &[("coastal-grass", 1.0)],
        grass: 0.6,
    },
    IslandGround {
        name: "Pine forest floor",
        mask: "pine_forest",
        surfaces: &[("forest-moss", 0.55), ("pine-needles", 0.45)],
        grass: 0.35,
    },
    IslandGround {
        name: "Spruce forest floor",
        mask: "spruce_forest",
        surfaces: &[("forest-floor", 0.6), ("pine-needles", 0.4)],
        grass: 0.15,
    },
    IslandGround {
        name: "Broadleaf forest floor",
        mask: "broadleaf_forest",
        surfaces: &[("leaf-litter", 0.6), ("forest-floor", 0.4)],
        grass: 0.4,
    },
    IslandGround {
        name: "Dunes",
        mask: "dune",
        surfaces: &[("dune-sand", 1.0)],
        grass: 0.1,
    },
    IslandGround {
        name: "Beach",
        mask: "beach",
        surfaces: &[("beach-sand", 1.0)],
        grass: 0.0,
    },
    IslandGround {
        name: "Scree",
        mask: "scree",
        surfaces: &[("stony-moss", 0.5), ("rocky-ground", 0.5)],
        grass: 0.2,
    },
    IslandGround {
        name: "Rock",
        mask: "rock",
        surfaces: &[("rock", 0.75), ("lichen-rock", 0.25)],
        grass: 0.0,
    },
];

/// Adds the island ground layers above the meadows: a ground preset, a grass exclusion and
/// their composition for each, and a layer reading its mask.
fn island_ground(project: &mut ProjectDocument) {
    use environment::{
        ChannelId, Exclusion, GroundTreatment, Layer, LayerId, OutputId, Preset, PresetId,
        PresetKind, PresetUse, PresetUseId, SurfaceWeight,
    };
    let pack = TerrainPack::baltic();
    let channel = ChannelId(stable_id("ground-grass-channel"));
    let definition = &mut project.environments[0];
    let first = definition.layers.len() as i32;
    for (index, ground) in ISLAND_GROUND.iter().enumerate() {
        let id = |part: &str| stable_id(&format!("island-ground/{}/{part}", ground.mask));
        let preset = |part: &str, name: String, kind| Preset {
            id: PresetId(id(part)),
            revision: 1,
            name,
            kind,
        };
        let soil = preset(
            "ground",
            format!("{} ground", ground.name),
            PresetKind::Ground(GroundTreatment {
                id: OutputId(id("ground-output")),
                strength: 1.0,
                surfaces: ground
                    .surfaces
                    .iter()
                    .map(|&(key, weight)| SurfaceWeight {
                        surface: pack.surface(key),
                        weight,
                    })
                    .collect(),
            }),
        );
        let thinning = preset(
            "grass",
            format!("{} grass", ground.name),
            PresetKind::Exclusion(Exclusion {
                id: OutputId(id("grass-output")),
                channel,
                strength: 1.0 - ground.grass,
            }),
        );
        let child = |part: &str, name: &str, preset: &Preset| PresetUse {
            id: PresetUseId(id(part)),
            name: name.into(),
            preset: preset.id,
            overrides: vec![],
        };
        let composition = preset(
            "composition",
            ground.name.into(),
            PresetKind::Composition(vec![
                child("ground-use", "Ground", &soil),
                child("grass-use", "Grass", &thinning),
            ]),
        );
        definition.layers.push(Layer {
            id: LayerId(id("layer")),
            revision: 1,
            name: ground.name.into(),
            preset: composition.id,
            overrides: vec![],
            order: first + index as i32,
            seed: 42,
            enabled: true,
            opacity: 1.0,
            imported_mask: Some(ground.mask.into()),
        });
        project
            .presets
            .presets
            .extend([soil, thinning, composition]);
    }
}

/// Where an imported layer's coverage comes from.
#[derive(Clone, Copy)]
pub(crate) enum Mask<'a> {
    /// `paint`'s output at this index of `GENERATED_MASKS`.
    Generated(usize),
    Imported(&'a MaskFn<'a>),
}

/// The layers of `definition` that read a mask, each with its source: an imported mask of that
/// name, otherwise a generated one. Painted layers are left out; imports never touch them.
pub(crate) fn imported_layers<'a>(
    definition: &environment::EnvironmentDefinition,
    imported: &[(&str, &'a MaskFn<'a>)],
) -> Result<Vec<(environment::LayerId, Mask<'a>)>> {
    definition
        .layers
        .iter()
        .filter_map(|layer| Some((layer, layer.imported_mask.as_deref()?)))
        .map(|(layer, name)| {
            let mask = match imported.iter().find(|(n, _)| *n == name) {
                Some((_, sample)) => Mask::Imported(*sample),
                None => Mask::Generated(
                    GENERATED_MASKS
                        .iter()
                        .position(|n| *n == name)
                        .with_context(|| {
                            let available: Vec<_> = imported
                                .iter()
                                .map(|(n, _)| *n)
                                .chain(GENERATED_MASKS)
                                .collect();
                            format!(
                                "layer {:?} reads mask {name:?}, which the import does not \
                                 provide; available: {}",
                                layer.name,
                                available.join(", ")
                            )
                        })?,
                ),
            };
            Ok((layer.id, mask))
        })
        .collect()
}

/// Imported masks no layer reads.
pub(crate) fn unused_masks<'a>(
    definition: &environment::EnvironmentDefinition,
    imported: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    imported
        .into_iter()
        .filter(|name| {
            !definition
                .layers
                .iter()
                .any(|l| l.imported_mask.as_deref() == Some(*name))
        })
        .map(String::from)
        .collect()
}

/// The catalog and settings of an island world, without cells.
pub(crate) fn base_document(name: &str, bounds: [f32; 2], sea_level: f32) -> ProjectDocument {
    let mut project = road_demo::document();
    let space = project.default_world_space;
    project.world_spaces.retain(|s| s.id == space);
    let world = &mut project.world_spaces[0];
    world.name = name.into();
    world.cell_size = DEFAULT_CELL_SIZE;
    [world.minimum_y, world.maximum_y] = bounds;
    world.sea_level = Some(sea_level);
    project.environments.retain(|d| d.space == space);
    let definition = &mut project.environments[0];
    definition.cell_size = DEFAULT_CELL_SIZE;
    definition.mask_resolution = MASK_SIDE as u16;
    // The meadow layers follow masks, so imports keep them up to date. Island ground layers
    // replace the demo's clearing.
    definition
        .layers
        .retain(|l| l.preset != environment::fixtures::CLEARING);
    for layer in &mut definition.layers {
        let mask = match layer.preset {
            environment::fixtures::DRY_MEADOW => "land",
            environment::fixtures::GREEN_MEADOW => "green",
            _ => continue,
        };
        layer.imported_mask = Some(mask.into());
    }
    island_ground(&mut project);
    project.terrain_profiles.retain(|p| p.space == space);
    project.terrain_profiles[0].composite_minimum_level = COMPOSITE_MINIMUM_LEVEL;
    project.cells.clear();
    project.terrain_cell_heightfields.clear();
    project.environment_cells.clear();
    project.objects.clear();
    project.roads = Default::default();
    project
}

/// Adds imported cells to a whole document, as the import writer stores them.
pub(crate) fn push_cell(project: &mut ProjectDocument, imported: ImportedTerrainCell) {
    let space = project.default_world_space;
    let definition = &project.environments[0];
    project.cells.push(SourceCellRecord {
        space,
        cell: imported.cell,
        height: imported.height,
        source_revision: 1,
    });
    if let Some(heights) = imported.heights {
        project
            .terrain_cell_heightfields
            .push(SourceTerrainCellHeightfieldRecord {
                space,
                cell: imported.cell,
                resolution: imported.resolution,
                heights,
                source_revision: 1,
            });
    }
    if !imported.tiles.is_empty() {
        project.environment_cells.push(SourceEnvironmentCellRecord {
            space,
            cell: imported.cell,
            source_revision: 1,
            definition_revision: definition.revision,
            tiles: imported.tiles,
        });
    }
}

/// The generated masks in `GENERATED_MASKS` order: dry meadow on land, green meadow in the
/// lowlands, bare ground on the shore, steep slopes and summits, sand along the shore (also
/// under water), scree and rock on steep slopes. Coast, dune and forest floors come only from
/// imported masks.
fn paint(p: Vec2, height: f32, slope: f32) -> [f32; 11] {
    let land = smoothstep(0.6, 2.0, height);
    let moist = (0.5 + 0.9 * terrain_fbm(p / 450., 0x6b8e_2d17)).clamp(0., 1.);
    let green = land * (1. - smoothstep(110., 320., height)) * moist;
    let bare = land
        * (1. - smoothstep(1.8, 4.5, height))
            .max(smoothstep(0.62, 0.95, slope))
            .max(smoothstep(520., 640., height));
    let rock = smoothstep(0.75, 0.95, slope);
    let scree = smoothstep(0.55, 0.75, slope) * (1. - rock);
    let beach = (1. - smoothstep(1.8, 3.5, height)) * (1. - smoothstep(0.45, 0.7, slope));
    [land, green, bare, 0., 0., 0., 0., 0., beach, scree, rock]
}

/// Heights and the coverage of imported `layers` in one cell, sampled every metre on the
/// world-wide grid, so neighbours share their edges. Heights sit on the cooked 1/1024 m grid,
/// so an exactly flat cell (open sea) is detected and stores no heightfield. Coverage fades
/// out over the last 48 m towards the world's XZ `bounds`: beyond them nothing is built, and
/// the cook requires neighbouring cells to agree along their shared edges.
pub(crate) fn terrain_cell(
    cell: CellCoord,
    height: &HeightFn,
    layers: &[(environment::LayerId, Mask)],
    bounds: [Vec2; 2],
) -> ImportedTerrainCell {
    let step = DEFAULT_CELL_SIZE / (HEIGHT_SIDE - 1) as f32;
    // One sample of halo on each side gives central-difference slopes that agree across
    // cell borders.
    let side = HEIGHT_SIDE + 2;
    let point = |i: usize, j: usize| {
        Vec2::new(
            cell.x as f32 * DEFAULT_CELL_SIZE + (i as f32 - 1.) * step,
            cell.z as f32 * DEFAULT_CELL_SIZE + (j as f32 - 1.) * step,
        )
    };
    let halo: Vec<f32> = (0..side)
        .flat_map(|j| (0..side).map(move |i| (i, j)))
        .map(|(i, j)| world::quantize_terrain_height(height(point(i, j))))
        .collect();
    let heights: Vec<f32> = (1..=HEIGHT_SIDE)
        .flat_map(|j| (1..=HEIGHT_SIDE).map(move |i| (i, j)))
        .map(|(i, j)| halo[j * side + i])
        .collect();
    let mut tiles: Vec<_> = layers
        .iter()
        .map(|&(layer, _)| CoverageTile {
            layer,
            samples: Vec::with_capacity(MASK_SIDE * MASK_SIDE),
        })
        .collect();
    for j in 1..=MASK_SIDE {
        for i in 1..=MASK_SIDE {
            let dx = (halo[j * side + i + 1] - halo[j * side + i - 1]) / (2. * step);
            let dz = (halo[(j + 1) * side + i] - halo[(j - 1) * side + i]) / (2. * step);
            let p = point(i, j);
            let fade = smoothstep(0., 48., (p - bounds[0]).min(bounds[1] - p).min_element());
            let generated = paint(p, halo[j * side + i], dx.hypot(dz));
            for (tile, (_, mask)) in tiles.iter_mut().zip(layers) {
                let weight = match *mask {
                    Mask::Generated(index) => generated[index],
                    Mask::Imported(sample) => sample(p).clamp(0., 1.),
                };
                tile.samples.push((weight * fade * 255.).round() as u8);
            }
        }
    }
    tiles.retain(|t| t.samples.iter().any(|&v| v != 0));
    let flat = heights.iter().all(|h| h.to_bits() == heights[0].to_bits());
    ImportedTerrainCell {
        cell,
        height: heights[0],
        heights: (!flat).then_some(heights),
        resolution: HEIGHT_SIDE as u16,
        tiles,
    }
}

/// The shore point nearest `from`, along 72 headings in 4 m steps up to 8 km.
pub(crate) fn nearest_shore(height: &HeightFn, from: Vec2) -> Option<Vec2> {
    let above = height(from) >= 1.5;
    (0..72)
        .filter_map(|i| {
            let direction = Vec2::from_angle((i as f32 * 5.).to_radians());
            (1..2000)
                .map(|step| from + direction * step as f32 * 4.)
                .find(|&p| (height(p) < 1.5) == above)
        })
        .min_by(|a, b| a.distance(from).total_cmp(&b.distance(from)))
}

/// Highest point within `radius` of `centre`: 41×41 searches, each 20 times finer, down to 2 m.
pub(crate) fn highest(height: &HeightFn, centre: Vec2, radius: f32) -> Vec2 {
    let search = |centre: Vec2, spacing: f32| {
        (0..=40)
            .flat_map(|j| {
                (0..=40).map(move |i| centre + Vec2::new(i as f32 - 20., j as f32 - 20.) * spacing)
            })
            .max_by(|a, b| height(*a).total_cmp(&height(*b)))
            .unwrap()
    };
    let (mut p, mut spacing) = (centre, radius / 20.);
    loop {
        p = search(p, spacing);
        if spacing <= 2. {
            return p;
        }
        spacing = (spacing / 20.).max(2.);
    }
}

/// The heading from `p` whose terrain rises least above eye level within 1.5 km: the most
/// open view.
pub(crate) fn open_heading(height: &HeightFn, p: Vec2) -> Vec2 {
    let eye = height(p) + 2.;
    (0..72)
        .map(|i| Vec2::from_angle((i as f32 * 5.).to_radians()))
        .min_by(|a, b| {
            let rise = |direction: Vec2| {
                (1..=75)
                    .map(|step| step as f32 * 20.)
                    .map(|d| (height(p + direction * d) - eye) / d)
                    .fold(f32::NEG_INFINITY, f32::max)
            };
            rise(*a).total_cmp(&rise(*b))
        })
        .unwrap()
}

/// A start view standing at `p` and looking along `look`. Low pitches keep the horizon in
/// frame: these views are about distance.
pub(crate) fn view(
    height: &HeightFn,
    p: Vec2,
    look: Vec2,
    pitch_degrees: f32,
    distance: f32,
) -> WorldViewBookmark {
    WorldViewBookmark {
        position: [p.x, height(p), p.y],
        // Camera yaw that looks along `look` from behind the player.
        yaw_degrees: (-look.x).atan2(-look.y).to_degrees(),
        pitch_degrees,
        distance,
        fog_visibility: VIEW_VISIBILITY,
        route: Vec::new(),
    }
}

/// Writes start views beside a project, replacing views of the same name.
pub(crate) fn write_views(project: &Path, views: &[(&str, WorldViewBookmark)]) -> Result<()> {
    let directory = project.with_extension("views");
    fs::create_dir_all(&directory)?;
    for (name, view) in views {
        view.validate().map_err(anyhow::Error::msg)?;
        fs::write(
            directory.join(format!("{name}.ron")),
            ron::ser::to_string_pretty(view, Default::default())?,
        )?;
    }
    Ok(())
}
