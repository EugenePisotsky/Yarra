//! Each world's compile plan, and the demo world's environment.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use environment::*;
use environment_compile::{CompilePlan, CompileProfile};
use vegetation::VegetationCatalog;
use world::{CellCoord, TerrainSurfaceId, WorldSpaceId};
use world_db::{ProjectDocument, SourceEnvironmentCellRecord};

pub(super) fn prepare_plans(
    project: &ProjectDocument,
) -> Result<BTreeMap<WorldSpaceId, CompilePlan>> {
    let empty = VegetationCatalog::default();
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

    Ok(plans)
}

pub(super) fn demo_environment(
    overworld: WorldSpaceId,
    interior: WorldSpaceId,
    pack: &super::TerrainPack,
    green: TerrainSurfaceId,
    dry: TerrainSurfaceId,
) -> (
    PresetLibrary,
    Vec<EnvironmentDefinition>,
    Vec<SourceEnvironmentCellRecord>,
) {
    let presets = environment::fixtures::meadow_library(dry, green);
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
        surfaces: pack.surface_ids(),
        base_surface: dry,
        worn: pack.worn.clone(),
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
                imported_mask: None,
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
        worn: vec![],
        layers: Vec::new(),
    };
    (presets, vec![definition, interior], records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TestDirectory, cook_document, decoded_pages};
    use world::{PageDomain, PageKey, PagePayload, TerrainNodeKey};
    use world_db::{RuntimeReader, environment_dependency_cells};

    fn payload(reader: &RuntimeReader, space: WorldSpaceId, domain: PageDomain) -> PagePayload {
        let key = PageKey {
            space,
            cell: CellCoord::ZERO,
            domain,
        };
        reader
            .read_page(key)
            .unwrap()
            .unwrap()
            .decode()
            .unwrap()
            .payload
    }

    #[test]
    fn cooked_pages_match_the_bounded_source_compiler_and_preserve_manual_objects() {
        let project = super::super::demo_project_document();
        let definition = &project.environments[0];
        let space = definition.space;
        let cell = CellCoord::ZERO;
        let halo = environment_dependency_cells(&[cell]).unwrap();
        let snapshot = CoverageSnapshot {
            space,
            cells: halo
                .iter()
                .map(|cell| {
                    project
                        .environment_cells
                        .iter()
                        .find(|r| r.space == space && r.cell == *cell)
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
        let directory = TestDirectory::new("environment-cook");
        let reader = directory.cook(&project);
        let PagePayload::TerrainHeightfield(terrain) = payload(&reader, space, PageDomain::Terrain)
        else {
            panic!()
        };
        assert_eq!(terrain.surfaces, compiled.ground.surfaces);

        // The control tile lays the compiled ground's weights out by texture layer.
        let leaf = crate::GroundLayers::read(&reader, space)
            .unwrap()
            .leaf(
                cell,
                terrain.heightfield.clone(),
                &compiled.ground,
                compiled.wear.clone(),
            )
            .unwrap();
        assert_eq!(
            crate::published_terrain_control(&reader, TerrainNodeKey::leaf(space, cell)).unwrap(),
            Some(leaf.control)
        );
        assert_eq!(
            payload(&reader, space, PageDomain::Vegetation),
            PagePayload::Vegetation(compiled.vegetation)
        );

        // The order sources are listed in changes nothing.
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

        let repeat = directory.cook(&reordered);
        assert_eq!(
            reader.manifest().content_hash,
            repeat.manifest().content_hash
        );

        // Without paint the ground changes; placed objects do not.
        let mut cleared = project.clone();
        cleared.environment_cells.clear();
        let empty = directory.cook(&cleared);
        assert_ne!(
            reader.manifest().content_hash,
            empty.manifest().content_hash
        );

        for cell in &project.cells {
            for domain in [PageDomain::StaticObjects, PageDomain::GameplayObjects] {
                let key = PageKey {
                    space: cell.space,
                    cell: cell.cell,
                    domain,
                };
                assert_eq!(
                    reader.read_page(key).unwrap().map(|p| p.checksum),
                    empty.read_page(key).unwrap().map(|p| p.checksum)
                );
            }
        }
    }

    #[test]
    fn unpainted_world_has_only_its_explicit_base_and_missing_definitions_fail() {
        let mut project = super::super::demo_project_document();
        project.environment_cells.clear();
        let directory = TestDirectory::new("unpainted");
        let reader = directory.cook(&project);
        let mut vegetation = 0;

        // Unpainted cells show their base ground and grow what grows on it.
        for definition in &project.environments {
            for page in decoded_pages(&reader, definition.space) {
                match page.payload {
                    PagePayload::TerrainHeightfield(ground) => {
                        assert_eq!(ground.surfaces, [definition.base_surface]);
                    }
                    PagePayload::Vegetation(_) => vegetation += 1,
                    _ => {}
                }
            }
        }

        assert!(vegetation > 0);
        project.environments.clear();
        assert!(cook_document(&project, &directory.0.join("undefined")).is_err());
    }
}

#[cfg(test)]
mod road_tests {
    use super::*;
    use crate::{TestDirectory, cook_document};
    use environment::roads::*;
    use world::{PageDomain, PageKey, PagePayload};

    use world_db::{
        RoadDocument, RoadSourceRecord, RuntimeReader, SourceRoad, SourceRoadKnot, SourceRoadSpan,
        environment_dependency_cells,
    };

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
            vegetation_channel: GRASS_CHANNEL,
            track_spacing: 3.8,
            track_width: 2.4,
            edge_softness: 0.55,
            edge_variation: 0.08,
            edge_patch_size: 4.0,
            track_wander: 0.0,
            track_wander_length: 8.0,
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
            traffic: environment::roads::DEFAULT_TRAFFIC,
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
        let directory = TestDirectory::new("road-cook");
        let baseline = directory.cook(&project);
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
        let roads = directory.0.join("roads");
        let cooked =
            RuntimeReader::open_immutable(&cook_document(&project, &roads).unwrap()).unwrap();
        assert_ne!(
            baseline.manifest().content_hash,
            cooked.manifest().content_hash
        );
        let source =
            world_db::ProjectReader::open_read_only(&roads.join("project.sqlite")).unwrap();
        let snapshot = source
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
        let page = |reader: &RuntimeReader, domain| {
            let key = PageKey {
                space,
                cell: CellCoord::ZERO,
                domain,
            };
            reader
                .read_page(key)
                .unwrap()
                .unwrap()
                .decode()
                .unwrap()
                .payload
        };
        let vegetation = page(&cooked, PageDomain::Vegetation);
        assert_eq!(vegetation, PagePayload::Vegetation(compiled.vegetation));
        assert_ne!(vegetation, page(&baseline, PageDomain::Vegetation));
        let PagePayload::TerrainHeightfield(ground) = page(&cooked, PageDomain::Terrain) else {
            panic!()
        };
        assert_eq!(ground.surfaces, compiled.ground.surfaces);

        // The order roads are listed in changes nothing.
        let mut reordered = world_db::read_project_database(&roads.join("project.sqlite")).unwrap();
        reordered.roads.records.reverse();
        drop(source);
        assert_eq!(
            directory.cook(&reordered).manifest().content_hash,
            cooked.manifest().content_hash
        );
    }
}
