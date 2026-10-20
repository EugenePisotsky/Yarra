//! Worlds imported from a heightfield export: their catalog, cells, ground masks and start
//! views.
use super::*;
use environment::CoverageTile;
use world::WorldViewBookmark;
use world_db::ImportedTerrainCell;

/// 1 m geometry and paint; ground weights are still compiled at 0.5 m.
pub(crate) const HEIGHT_SIDE: usize = 33;

pub(crate) const MASK_SIDE: usize = 33;
const VIEW_VISIBILITY: f32 = 20_000.;

/// Terrain height at a world XZ position.
pub(crate) type HeightFn<'a> = dyn Fn(Vec2) -> f32 + Sync + 'a;

/// A terrain tool's mask at a world XZ position, from 0 to 1.
pub(crate) type MaskFn<'a> = dyn Fn(Vec2) -> f32 + Sync + 'a;

/// The same ordered mask contract that Houdini reads for its material preview.
#[derive(serde::Deserialize)]
struct IslandGround {
    name: String,
    mask: String,
    surfaces: Vec<(String, f32)>,
}

#[derive(serde::Deserialize)]
struct GroundLayers {
    layers: Vec<IslandGround>,
}

/// The masks an island world's layers read, in stack order.
#[cfg(test)]
pub(crate) fn ground_mask_names() -> Vec<String> {
    ground_layers().into_iter().map(|g| g.mask).collect()
}

fn ground_layers() -> Vec<IslandGround> {
    toml::from_str::<GroundLayers>(include_str!(
        "../../../assets/packs/terrain/reteya-ground-layers.toml"
    ))
    .expect("the tracked ground bindings parse")
    .layers
}

/// Adds the island ground layers above the meadows: a ground preset for each and a layer
/// reading its mask.
fn island_ground(project: &mut ProjectDocument) {
    use environment::{
        GroundTreatment, Layer, LayerId, OutputId, Preset, PresetId, PresetKind, SurfaceWeight,
    };
    let pack = TerrainPack::baltic();
    let definition = &mut project.environments[0];
    let first = definition.layers.len() as i32;

    for (index, ground) in ground_layers().into_iter().skip(2).enumerate() {
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
                    .map(|(key, weight)| SurfaceWeight {
                        surface: pack.surface(key),
                        weight: *weight,
                    })
                    .collect(),
            }),
        );
        definition.layers.push(Layer {
            id: LayerId(id("layer")),
            revision: 1,
            name: ground.name,
            preset: soil.id,
            overrides: vec![],
            order: first + index as i32,
            seed: 42,
            enabled: true,
            opacity: 1.0,
            imported_mask: Some(ground.mask),
        });
        project.presets.presets.push(soil);
    }
}

/// The layers of `definition` that read a mask, each with the imported mask of that name.
/// Painted layers are left out; imports never touch them. A layer whose mask the import lacks
/// is an error.
pub(crate) fn imported_layers<'a>(
    definition: &environment::EnvironmentDefinition,
    imported: &[(&str, &'a MaskFn<'a>)],
) -> Result<Vec<(environment::LayerId, &'a MaskFn<'a>)>> {
    definition
        .layers
        .iter()
        .filter_map(|layer| Some((layer, layer.imported_mask.as_deref()?)))
        .map(|(layer, name)| {
            let (_, sample) = imported.iter().find(|(n, _)| *n == name).with_context(|| {
                let available: Vec<_> = imported.iter().map(|(n, _)| *n).collect();
                format!(
                    "layer {:?} reads mask {name:?}, which the import does not provide; \
                         available: {}",
                    layer.name,
                    available.join(", ")
                )
            })?;
            Ok((layer.id, *sample))
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
    project.cells.clear();
    project.terrain_cell_heightfields.clear();
    project.environment_cells.clear();
    project.objects.clear();
    project.roads = Default::default();
    project
}

/// Heights and the coverage of imported `layers` in one cell, sampled every metre on the
/// world-wide grid, so neighbours share their edges. Heights sit on the cooked 1/1024 m grid,
/// so an exactly flat cell (open sea) is detected and stores no heightfield. Coverage fades
/// out over the last 48 m towards the world's XZ `bounds`: beyond them nothing is built, and
/// the cook requires neighbouring cells to agree along their shared edges.
pub(crate) fn terrain_cell(
    cell: CellCoord,
    height: &HeightFn,
    layers: &[(environment::LayerId, &MaskFn)],
    bounds: [Vec2; 2],
) -> ImportedTerrainCell {
    let point = |i: usize, j: usize, side: usize| {
        let step = DEFAULT_CELL_SIZE / (side - 1) as f32;
        Vec2::new(
            cell.x as f32 * DEFAULT_CELL_SIZE + i as f32 * step,
            cell.z as f32 * DEFAULT_CELL_SIZE + j as f32 * step,
        )
    };
    let heights: Vec<f32> = (0..HEIGHT_SIDE)
        .flat_map(|j| (0..HEIGHT_SIDE).map(move |i| (i, j)))
        .map(|(i, j)| world::quantize_terrain_height(height(point(i, j, HEIGHT_SIDE))))
        .collect();
    let mut tiles: Vec<_> = layers
        .iter()
        .map(|&(layer, _)| CoverageTile {
            layer,
            samples: Vec::with_capacity(MASK_SIDE * MASK_SIDE),
        })
        .collect();

    for j in 0..MASK_SIDE {
        for i in 0..MASK_SIDE {
            let p = point(i, j, MASK_SIDE);
            let fade = smoothstep(0., 48., (p - bounds[0]).min(bounds[1] - p).min_element());

            for (tile, (_, sample)) in tiles.iter_mut().zip(layers) {
                let weight = sample(p).clamp(0., 1.);
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

#[cfg(test)]
mod ground_tests {
    use super::*;

    #[test]
    fn tracked_bindings_match_the_houdini_stack_and_every_mask_is_required() {
        let project = base_document("Ground", [-100., 1000.], 0.);
        let definition = &project.environments[0];
        let expected: Vec<_> = ground_layers().into_iter().map(|g| g.mask).collect();
        let mut layers: Vec<_> = definition.layers.iter().collect();
        layers.sort_by_key(|l| l.order);
        let actual: Vec<_> = layers
            .iter()
            .map(|l| l.imported_mask.as_deref().unwrap())
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(actual.len(), 16);
        definition.validate(&project.presets).unwrap();
        let Err(error) = imported_layers(definition, &[]) else {
            panic!("layers read masks the import lacks")
        };
        assert!(format!("{error:#}").contains("\"land\""), "{error:#}");
    }
}
