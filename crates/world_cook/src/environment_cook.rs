use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use environment::*;
use environment_compile::{CompilePlan, CompileProfile, merge_runtime_catalogs};
use vegetation::{VegetationCatalog, VegetationFieldPageData};
use world::{CellCoord, TerrainSurfaceId, WorldSpaceId};
use world_db::{ProjectDocument, SourceEnvironmentCellRecord, environment_dependency_cells};

pub(super) struct TerrainSlot {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub slot: u8,
    pub surface: TerrainSurfaceId,
}
pub(super) struct TerrainWeights {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub page: u8,
    pub resolution: u16,
    pub rgba: Vec<u8>,
    pub source_revision: i64,
}
pub(super) struct VegetationPage {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub data: VegetationFieldPageData,
    pub source_revision: i64,
}
pub(super) struct CookedEnvironment {
    pub terrain: BTreeMap<(WorldSpaceId, CellCoord), world::TerrainHeightfield>,
    pub catalog: Option<VegetationCatalog>,
    pub slots: Vec<TerrainSlot>,
    pub weights: Vec<TerrainWeights>,
    pub vegetation: Vec<VegetationPage>,
    pub fingerprint: [u8; 32],
}

pub(super) fn compile_environment(project: &ProjectDocument) -> Result<CookedEnvironment> {
    let roads = world_db::RoadDocumentIndex::new(&project.roads, &project.environments)?;
    let empty = VegetationCatalog {
        species: Vec::new(),
        populations: Vec::new(),
        assemblages: Vec::new(),
    };
    let plants = project.vegetation_catalog.as_ref().unwrap_or(&empty);
    let mut definitions = BTreeMap::new();
    let mut plans = BTreeMap::new();
    for definition in &project.environments {
        if definitions.insert(definition.space, definition).is_some() {
            bail!("duplicate environment world");
        }
        let world = project
            .world_spaces
            .iter()
            .find(|s| s.id == definition.space)
            .context("environment world does not exist")?;
        let terrain = project
            .terrain_profiles
            .iter()
            .find(|p| p.space == definition.space)
            .context("environment world has no terrain profile")?;
        if definition.cell_size != world.cell_size {
            bail!("environment and terrain cell sizes differ");
        }
        for surface in &definition.surfaces {
            if !project
                .terrain_texture_layers
                .iter()
                .any(|l| l.texture_set == terrain.texture_set && l.surface == *surface)
            {
                bail!("environment surface is not in the world's terrain texture set");
            }
        }
        plans.insert(
            definition.space,
            CompilePlan::new(
                definition,
                plants,
                &project.presets,
                CompileProfile {
                    terrain_resolution: terrain.weight_resolution,
                    ..Default::default()
                },
            )?,
        );
    }
    let mut source = BTreeMap::new();
    for record in &project.environment_cells {
        let definition = definitions
            .get(&record.space)
            .context("coverage world has no environment definition")?;
        if record.source_revision < 0
            || record.definition_revision != definition.revision
            || source.insert((record.space, record.cell), record).is_some()
        {
            bail!("invalid environment coverage header");
        }
    }
    let heights = project
        .terrain_cell_heightfields
        .iter()
        .map(|h| ((h.space, h.cell), h))
        .collect::<BTreeMap<_, _>>();
    let base_cells = project
        .cells
        .iter()
        .map(|c| ((c.space, c.cell), c))
        .collect::<BTreeMap<_, _>>();
    let mut result = CookedEnvironment {
        terrain: BTreeMap::new(),
        catalog: Some(merge_runtime_catalogs(&plans.values().collect::<Vec<_>>())?),
        slots: Vec::new(),
        weights: Vec::new(),
        vegetation: Vec::new(),
        fingerprint: [0; 32],
    };
    let mut hash = blake3::Hasher::new();
    for (space, plan) in &plans {
        hash.update(&plan.fingerprint());
        // Validate authored masks outside terrain too. They can own boundary endpoints, but
        // cannot hide invalid layer references or seams simply because terrain is not built there.
        let requested: BTreeSet<_> = source
            .keys()
            .filter(|(s, _)| s == space)
            .map(|(_, c)| *c)
            .chain(
                project
                    .cells
                    .iter()
                    .filter(|c| c.space == *space)
                    .map(|c| c.cell),
            )
            .collect();
        let terrain_cells: BTreeSet<_> = project
            .cells
            .iter()
            .filter(|c| c.space == *space)
            .map(|c| c.cell)
            .collect();
        // One output at a time: a 3x3 halo is bounded even for large source grids and many layers.
        for cell in requested {
            let coverage = CoverageSnapshot {
                space: *space,
                cells: environment_dependency_cells(&[cell])?
                    .into_iter()
                    .map(|cell| {
                        source.get(&(*space, cell)).map_or(
                            CoverageCell {
                                cell,
                                revision: 0,
                                tiles: Vec::new(),
                            },
                            |r| r.coverage(),
                        )
                    })
                    .collect(),
            };
            if !terrain_cells.contains(&cell) {
                plan.validate_coverage(&[cell], &coverage)?;
                continue;
            }
            let halo = environment_dependency_cells(&[cell])?;
            let bounds = environment::roads::RoadCellBounds {
                minimum: CellCoord {
                    x: cell.x - 1,
                    z: cell.z - 1,
                },
                maximum: CellCoord {
                    x: cell.x + 1,
                    z: cell.z + 1,
                },
            };
            let road_snapshot = roads.snapshot(*space, bounds, definitions[space].cell_size)?;
            let world = project
                .world_spaces
                .iter()
                .find(|w| w.id == *space)
                .unwrap();
            let terrain = environment_compile::TerrainSource {
                space: *space,
                cell_size: world.cell_size,
                minimum_height: world.minimum_y,
                maximum_height: world.maximum_y,
                cells: halo
                    .iter()
                    .filter_map(|&cell| {
                        base_cells.get(&(*space, cell)).map(|base| {
                            if let Some(h) = heights.get(&(*space, cell)) {
                                environment_compile::TerrainSourceCell {
                                    flat: false,
                                    cell,
                                    resolution: h.resolution,
                                    heights: h.heights.clone(),
                                    revision: h.source_revision as u64,
                                }
                            } else {
                                environment_compile::TerrainSourceCell {
                                    flat: true,
                                    cell,
                                    resolution: 33,
                                    heights: vec![base.height; 33 * 33],
                                    revision: base.source_revision as u64,
                                }
                            }
                        })
                    })
                    .collect(),
                loaded_cells: halo,
            };
            let compiled = plan.compile_cell_with_terrain(
                cell,
                &coverage,
                &road_snapshot,
                &terrain,
                Default::default(),
            )?;
            result
                .terrain
                .insert((*space, cell), compiled.terrain.clone().unwrap());
            hash.update(&compiled.input_fingerprint);
            let source_revision = source.get(&(*space, cell)).map_or(0, |r| r.source_revision);
            result
                .slots
                .extend(
                    compiled
                        .ground
                        .surfaces
                        .iter()
                        .enumerate()
                        .map(|(slot, &surface)| TerrainSlot {
                            space: *space,
                            cell,
                            slot: slot as u8,
                            surface,
                        }),
                );
            result
                .weights
                .extend(compiled.ground.weight_pages.into_iter().enumerate().map(
                    |(page, weights)| TerrainWeights {
                        space: *space,
                        cell,
                        page: page as u8,
                        resolution: weights.resolution,
                        rgba: weights.rgba,
                        source_revision,
                    },
                ));
            if !compiled.vegetation.fields.is_empty() {
                result.vegetation.push(VegetationPage {
                    space: *space,
                    cell,
                    data: compiled.vegetation,
                    source_revision,
                });
            }
        }
    }
    for cell in &project.cells {
        if !plans.contains_key(&cell.space) {
            bail!(
                "world {:?} requires an explicit environment definition and base surface",
                cell.space
            );
        }
    }
    result.fingerprint = *hash.finalize().as_bytes();
    Ok(result)
}

pub(super) fn demo_environment(
    overworld: WorldSpaceId,
    interior: WorldSpaceId,
    green: TerrainSurfaceId,
    dry: TerrainSurfaceId,
) -> (
    PresetLibrary,
    Vec<EnvironmentDefinition>,
    Vec<SourceEnvironmentCellRecord>,
) {
    let grass = ChannelId(super::stable_id("ground-grass-channel"));
    let presets = environment::fixtures::meadow_library(dry, green, grass);
    let roots = [
        environment::fixtures::DRY_MEADOW,
        environment::fixtures::GREEN_MEADOW,
        environment::fixtures::CLEARING,
    ];
    let definition = EnvironmentDefinition {
        space: overworld,
        revision: 1,
        cell_size: super::DEFAULT_CELL_SIZE,
        mask_resolution: super::DEMO_TERRAIN_WEIGHT_RESOLUTION,
        surfaces: vec![green, dry],
        base_surface: dry,
        layers: roots
            .iter()
            .map(|id| presets.get(*id).unwrap())
            .enumerate()
            .map(|(index, c)| Layer {
                id: LayerId(super::stable_id(&format!("demo-layer-{}", c.name))),
                revision: 1,
                name: c.name.clone(),
                preset: c.id,
                overrides: vec![],
                order: index as i32,
                seed: 42,
                enabled: true,
                opacity: 1.0,
            })
            .collect(),
    };
    let mut records = Vec::new();
    let n = usize::from(definition.mask_resolution);
    for x in super::DEMO_WORLD_CELL_RANGE {
        for z in super::DEMO_WORLD_CELL_RANGE {
            let mut tiles: Vec<_> = definition
                .layers
                .iter()
                .map(|l| CoverageTile {
                    layer: l.id,
                    samples: Vec::with_capacity(n * n),
                })
                .collect();
            for sample_z in 0..n {
                for sample_x in 0..n {
                    let point = glam::Vec2::new(
                        (x as f32 + sample_x as f32 / (n - 1) as f32) * definition.cell_size,
                        (z as f32 + sample_z as f32 / (n - 1) as f32) * definition.cell_size,
                    );
                    let extent = super::DEMO_WORLD_CELL_RANGE.end as f32 * definition.cell_size;
                    let meadow = super::smoothstep(0.0, 12.0, extent - point.abs().max_element());
                    let green_amount = (1.0
                        - (super::demo_terrain_mix_offset()
                            + super::demo_terrain_mix_signal(point)
                                * super::demo_terrain_mix_amplitude()))
                    .clamp(0.0, 1.0);
                    // A sampled clearing fixture; the editable curved-road source comes in a later slice.
                    let clearing =
                        (1.0 - super::smoothstep(
                            1.8,
                            3.8,
                            (point.y - (10.0 + (point.x * 0.025).sin() * 8.0)).abs(),
                        )) * meadow;
                    for (tile, weight) in
                        tiles
                            .iter_mut()
                            .zip([meadow, green_amount * meadow, clearing])
                    {
                        tile.samples.push((weight * 255.0).round() as u8);
                    }
                }
            }
            tiles.retain(|t| t.samples.iter().any(|&v| v != 0));
            records.push(SourceEnvironmentCellRecord {
                space: overworld,
                cell: CellCoord { x, z },
                source_revision: 1,
                definition_revision: 1,
                tiles,
            });
        }
    }
    let interior = EnvironmentDefinition {
        space: interior,
        revision: 1,
        cell_size: definition.cell_size,
        mask_resolution: definition.mask_resolution,
        surfaces: vec![dry],
        base_surface: dry,
        layers: Vec::new(),
    };
    (presets, vec![definition, interior], records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::{PageDomain, PagePayload};

    #[test]
    fn cooked_pages_match_the_bounded_source_compiler_and_preserve_manual_objects() {
        let project = super::super::demo_project_document();
        let definition = &project.environments[0];
        let cell = CellCoord::ZERO;
        let halo = environment_dependency_cells(&[cell]).unwrap();
        let snapshot = CoverageSnapshot {
            space: definition.space,
            cells: halo
                .iter()
                .map(|cell| {
                    project
                        .environment_cells
                        .iter()
                        .find(|r| r.space == definition.space && r.cell == *cell)
                        .unwrap()
                        .coverage()
                })
                .collect(),
        };
        let plan = CompilePlan::new(
            definition,
            project.vegetation_catalog.as_ref().unwrap(),
            &project.presets,
            Default::default(),
        )
        .unwrap();
        let compiled = plan.compile_cells(&[cell], &snapshot).unwrap().remove(0);
        let manual_ids: BTreeSet<_> = project.objects.iter().map(|o| o.id.0).collect();
        let build = super::super::build_runtime(project.clone()).unwrap();
        let terrain = build
            .pages
            .iter()
            .find(|p| {
                p.key.space == definition.space
                    && p.key.cell == cell
                    && p.key.domain == PageDomain::TerrainRender
            })
            .unwrap()
            .clone()
            .decode()
            .unwrap();
        let PagePayload::TerrainHeightfield(terrain) = terrain.payload else {
            panic!()
        };
        assert_eq!(terrain.surfaces, compiled.ground.surfaces);
        assert_eq!(terrain.weight_pages, compiled.ground.weight_pages);
        let plants = build
            .pages
            .iter()
            .find(|p| {
                p.key.space == definition.space
                    && p.key.cell == cell
                    && p.key.domain == PageDomain::Vegetation
            })
            .unwrap()
            .clone()
            .decode()
            .unwrap();
        let PagePayload::Vegetation(plants) = plants.payload else {
            panic!()
        };
        assert_eq!(plants, compiled.vegetation);
        let mut reordered = project.clone();
        reordered.presets.presets.reverse();
        reordered.environments.reverse();
        reordered.environment_cells.reverse();
        for definition in &mut reordered.environments {
            definition.layers.reverse();
        }
        for cell in &mut reordered.environment_cells {
            cell.tiles.reverse();
        }
        let repeat = super::super::build_runtime(reordered).unwrap();
        assert_eq!(build.manifest.content_hash, repeat.manifest.content_hash);
        let mut cleared = project;
        cleared.environment_cells.clear();
        assert_eq!(
            cleared
                .objects
                .iter()
                .map(|o| o.id.0)
                .collect::<BTreeSet<_>>(),
            manual_ids
        );
        let empty = super::super::build_runtime(cleared).unwrap();
        assert_ne!(build.manifest.content_hash, empty.manifest.content_hash);
        assert!(
            empty
                .pages
                .iter()
                .all(|p| p.key.domain != PageDomain::Vegetation)
        );
        for page in &build.pages {
            if matches!(
                page.key.domain,
                PageDomain::StaticObjects | PageDomain::GameplayObjects
            ) {
                assert_eq!(
                    page.checksum,
                    empty
                        .pages
                        .iter()
                        .find(|p| p.key == page.key)
                        .unwrap()
                        .checksum
                );
            }
        }
    }

    #[test]
    fn unpainted_world_has_only_its_explicit_base_and_missing_definitions_fail() {
        let mut project = super::super::demo_project_document();
        project.environment_cells.clear();
        let compiled = compile_environment(&project).unwrap();
        assert!(compiled.vegetation.is_empty());
        assert!(compiled.weights.is_empty());
        for slot in &compiled.slots {
            assert_eq!(
                slot.surface,
                project
                    .environments
                    .iter()
                    .find(|d| d.space == slot.space)
                    .unwrap()
                    .base_surface
            );
        }
        project.environments.clear();
        assert!(compile_environment(&project).is_err());
    }
}

#[cfg(test)]
mod road_tests {
    use super::*;
    use environment::roads::*;
    use world_db::{RoadDocument, RoadSourceRecord, SourceRoad, SourceRoadKnot, SourceRoadSpan};
    #[test]
    fn persisted_roads_cook_the_same_fields_as_a_bounded_database_snapshot() {
        let mut project = super::super::demo_project_document();
        let definition = project.environments[0].clone();
        let space = definition.space;
        let p = CartTrackProfile {
            id: RoadProfileId([21; 16]),
            revision: 1,
            name: "Coarse-grid road fixture".into(),
            ground: environment::fixtures::DRY_GROUND,
            vegetation_channel: ChannelId(super::super::stable_id("ground-grass-channel")),
            track_spacing: 3.8,
            track_width: 2.4,
            edge_softness: 0.55,
            center_ground: 0.0,
            center_retention: 1.0,
            shoulder_ground: 0.15,
            shoulder_retention: 0.55,
            track_retention: 0.0,
            edge_variation: 0.08,
            edge_patch_size: 4.0,
            breakup: 0.4,
            breakup_patch_size: 4.0,
            relief: Default::default(),
        };
        let road = Road {
            id: RoadId([21; 16]),
            revision: 1,
            name: "Cooked route".into(),
            profile: p.id,
            seed: 48,
            enabled: true,
            order: 0,
            direction: TravelDirection::Bidirectional,
            travel_modes: vec![RoadTravelMode::Foot, RoadTravelMode::Cart],
        };
        let knot = |id, x| RoadKnot {
            id: RoadKnotId([id; 16]),
            revision: 1,
            position: RoadPoint::from_relative(
                CellCoord::ZERO,
                [x, 16.0],
                f64::from(definition.cell_size),
            )
            .unwrap(),
            incoming: [-32.0, 0.0],
            outgoing: [32.0, 0.0],
            width: 9.0,
        };
        let a = knot(21, -16.0);
        let b = knot(22, 80.0);
        let baseline = compile_environment(&project).unwrap();
        project.roads = RoadDocument {
            records: vec![
                RoadSourceRecord::Profile(p),
                RoadSourceRecord::Road(SourceRoad {
                    space,
                    road: road.clone(),
                }),
                RoadSourceRecord::Knot(SourceRoadKnot {
                    road: road.id,
                    knot: a.clone(),
                }),
                RoadSourceRecord::Knot(SourceRoadKnot {
                    road: road.id,
                    knot: b.clone(),
                }),
                RoadSourceRecord::Span(SourceRoadSpan {
                    id: RoadSpanId([21; 16]),
                    revision: 1,
                    road: road.id,
                    start: a.id,
                    end: b.id,
                }),
            ],
        };
        let directory =
            std::env::temp_dir().join(format!("yarra-road-cook-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("project.sqlite");
        world_db::write_project_database(&path, &project).unwrap();
        let reader = world_db::ProjectReader::open_read_only(&path).unwrap();
        let loaded = world_db::read_project_database(&path).unwrap();
        let cooked = compile_environment(&loaded).unwrap();
        assert_ne!(baseline.fingerprint, cooked.fingerprint);
        let snapshot = reader
            .read_environment_with_roads(
                space,
                &environment_dependency_cells(&[CellCoord::ZERO]).unwrap(),
                RoadCellBounds {
                    minimum: CellCoord::ZERO,
                    maximum: CellCoord::ZERO,
                },
            )
            .unwrap();
        let e = &snapshot.environment;
        let terrain = project
            .terrain_profiles
            .iter()
            .find(|p| p.space == space)
            .unwrap();
        let plan = CompilePlan::new(
            &e.definition,
            &e.vegetation_catalog,
            &e.presets,
            CompileProfile {
                terrain_resolution: terrain.weight_resolution,
                ..Default::default()
            },
        )
        .unwrap();
        let compiled = plan
            .compile_cells_with_roads(
                &[CellCoord::ZERO],
                &e.coverage,
                &snapshot.roads.roads,
                Default::default(),
            )
            .unwrap()
            .remove(0);
        let vegetation = cooked
            .vegetation
            .iter()
            .find(|v| v.space == space && v.cell == CellCoord::ZERO)
            .unwrap();
        assert_eq!(vegetation.data, compiled.vegetation);
        assert_ne!(
            vegetation.data,
            baseline
                .vegetation
                .iter()
                .find(|v| v.space == space && v.cell == CellCoord::ZERO)
                .unwrap()
                .data
        );
        let weights = cooked
            .weights
            .iter()
            .filter(|v| v.space == space && v.cell == CellCoord::ZERO)
            .collect::<Vec<_>>();
        assert_eq!(weights.len(), compiled.ground.weight_pages.len());
        for (a, b) in weights.iter().zip(&compiled.ground.weight_pages) {
            assert_eq!(a.rgba, b.rgba);
            assert_eq!(a.resolution, b.resolution);
        }
        let mut reordered = loaded;
        reordered.roads.records.reverse();
        assert_eq!(
            compile_environment(&reordered).unwrap().fingerprint,
            cooked.fingerprint
        );
        drop(reader);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
