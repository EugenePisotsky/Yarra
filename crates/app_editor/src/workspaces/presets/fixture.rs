//! A controlled, disposable patch evaluated by the same compiler as the world.
use environment::roads::*;
use environment::*;
use environment_compile::{CompilePlan, CompileProfile, CompiledGround};
use vegetation::{VegetationCatalog, VegetationFieldPage, VegetationScene, VegetationSurfaceField};
use world::{CellCoord, TerrainSurfaceId, WorldSpaceId};

pub(super) const SEED: u32 = 0x50a7_2026;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Footprint {
    Full,
    #[default]
    Patch,
    Hole,
}
impl Footprint {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Full => "Full coverage",
            Self::Patch => "Soft patch",
            Self::Hole => "Patch with hole",
        }
    }
    fn sample(self, x: f64, z: f64, size: f64) -> u8 {
        let radius = ((x - size / 2.0).powi(2) + (z - size / 2.0).powi(2)).sqrt() / size;
        let outer = ((0.44 - radius) / 0.09).clamp(0.0, 1.0);
        let value = match self {
            Self::Full => 1.0,
            Self::Patch => outer,
            Self::Hole => outer * ((radius - 0.10) / 0.07).clamp(0.0, 1.0),
        };
        (value * 255.0).round() as u8
    }
}
#[derive(Clone)]
pub(super) struct RoadFixture {
    pub profile: CartTrackProfile,
    pub curved: bool,
    pub width: f32,
}
impl RoadFixture {
    fn snapshot(&self, size: f32) -> RoadSnapshot {
        let size64 = f64::from(size);
        let offset = if self.curved { size64 * 0.15 } else { 0.0 };
        let knot = |id, x, z, incoming, outgoing| RoadKnot {
            id: RoadKnotId([id; 16]),
            revision: 1,
            position: RoadPoint::from_relative(CellCoord::ZERO, [x, z], size64).unwrap(),
            incoming,
            outgoing,
            width: self.width,
        };
        RoadSnapshot {
            space: WorldSpaceId(0),
            cell_size: size,
            loaded_bounds: RoadCellBounds {
                minimum: CellCoord::ZERO,
                maximum: CellCoord::ZERO,
            },
            truncated: false,
            junctions: vec![],
            profiles: vec![self.profile.clone()],
            roads: vec![Road {
                id: RoadId([90; 16]),
                revision: 1,
                name: "Preview road".into(),
                profile: self.profile.id,
                seed: SEED,
                enabled: true,
                order: 0,
                direction: TravelDirection::Bidirectional,
                travel_modes: vec![RoadTravelMode::Foot, RoadTravelMode::Cart],
            }],
            spans: vec![RoadSpan {
                id: RoadSpanId([91; 16]),
                revision: 1,
                road: RoadId([90; 16]),
                start: knot(92, -size64, size64 / 2.0 - offset, [0.0; 2], [size64, 0.0]),
                end: knot(
                    93,
                    size64 * 2.0,
                    size64 / 2.0 + offset,
                    [-size64, 0.0],
                    [0.0; 2],
                ),
            }],
        }
    }
}
#[derive(Clone)]
pub(super) struct Request {
    pub road: Option<RoadFixture>,
    pub library: PresetLibrary,
    pub plants: VegetationCatalog,
    pub preset: PresetId,
    pub base: TerrainSurfaceId,
    pub surfaces: Vec<TerrainSurfaceId>,
    pub size: u16,
    pub footprint: Footprint,
    pub underlay: Option<PresetId>,
}
pub(super) struct Product {
    pub scene: VegetationScene,
    pub ground: CompiledGround,
    pub terrain: Option<world::TerrainHeightfield>,
    pub size: f32,
    pub candidates: u32,
}
impl Request {
    pub(super) fn compile(self) -> Result<Product, String> {
        if ![8, 16].contains(&self.size) {
            return Err("Preview patch must be 8 or 16 metres".into());
        }
        let layer = |id, order, preset| Layer {
            id: LayerId([id; 16]),
            revision: 1,
            name: "Preview".into(),
            preset,
            overrides: vec![],
            order,
            seed: SEED,
            enabled: true,
            opacity: 1.0,
        };
        let mut layers = vec![];
        if let Some(p) = self.underlay {
            if !matches!(
                self.library.get(p).map(|p| &p.kind),
                Some(PresetKind::Foliage(_))
            ) {
                return Err("Reference underlay must be a foliage preset".into());
            }
            layers.push(layer(80, 0, p));
        }
        if self.road.is_none() {
            layers.push(layer(81, 1, self.preset));
        }
        let definition = EnvironmentDefinition {
            space: WorldSpaceId(0),
            revision: 1,
            cell_size: self.size as f32,
            mask_resolution: 33,
            surfaces: self.surfaces,
            base_surface: self.base,
            layers,
        };
        let plan = CompilePlan::new(
            &definition,
            &self.plants,
            &self.library,
            CompileProfile {
                terrain_resolution: if self.road.is_some() {
                    self.size * 8 + 1
                } else {
                    65
                },
                vegetation_resolution: if self.road.is_some() {
                    self.size * 8
                } else {
                    64
                },
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let size = f64::from(self.size);
        let mut cells = vec![];
        for x in -1..=1 {
            for z in -1..=1 {
                cells.push(CoverageCell {
                    cell: CellCoord { x, z },
                    revision: 1,
                    tiles: definition
                        .layers
                        .iter()
                        .map(|l| CoverageTile {
                            layer: l.id,
                            samples: (0..33 * 33)
                                .map(|i| {
                                    if l.order == 0 && self.road.is_none() {
                                        255
                                    } else {
                                        self.footprint.sample(
                                            f64::from(x) * size + (i % 33) as f64 * size / 32.0,
                                            f64::from(z) * size + (i / 33) as f64 * size / 32.0,
                                            size,
                                        )
                                    }
                                })
                                .collect(),
                        })
                        .collect(),
                });
            }
        }
        let coverage = CoverageSnapshot {
            space: definition.space,
            cells,
        };
        let cell = if let Some(road) = &self.road {
            let loaded_cells = coverage.cells.iter().map(|c| c.cell).collect::<Vec<_>>();
            let resolution = self.size * 4 + 1; // Match the current world's 25 cm height grid.
            let terrain = environment_compile::TerrainSource {
                space: definition.space,
                cell_size: self.size as f32,
                minimum_height: -8.0,
                maximum_height: 8.0,
                cells: loaded_cells
                    .iter()
                    .map(|&cell| environment_compile::TerrainSourceCell {
                        flat: false,
                        cell,
                        revision: 1,
                        resolution,
                        heights: vec![0.0; usize::from(resolution).pow(2)],
                    })
                    .collect(),
                loaded_cells,
            };
            let mut snapshot = road.snapshot(self.size as f32);
            snapshot.loaded_bounds = RoadCellBounds {
                minimum: CellCoord { x: -1, z: -1 },
                maximum: CellCoord { x: 1, z: 1 },
            };
            plan.compile_cell_with_terrain(
                CellCoord::ZERO,
                &coverage,
                &snapshot,
                &terrain,
                Default::default(),
            )
        } else {
            plan.compile_cells(&[CellCoord::ZERO], &coverage)
                .map(|mut cells| cells.remove(0))
        }
        .map_err(|e| e.to_string())?;
        let surface = cell.terrain.as_ref().map_or_else(
            || VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
            |h| {
                let n = usize::from(h.resolution);
                VegetationSurfaceField {
                    resolution: h.resolution,
                    heights: (0..n * n).map(|i| h.height_at(i % n, i / n)).collect(),
                    normals_oct: h.normals_oct.clone(),
                    validity: vec![255; n * n],
                }
            },
        );
        let scene = VegetationScene {
            catalog: plan.catalog().clone(),
            pages: vec![VegetationFieldPage {
                origin_xz: [0.0; 2],
                size: self.size as f32,
                surface,
                fields: cell.vegetation.fields,
            }],
        };
        scene.validate().map_err(|e| e.to_string())?;
        let candidates = scene
            .pages
            .iter()
            .flat_map(|page| {
                page.fields.iter().map(|field| {
                    vegetation::candidate_domain(
                        page,
                        scene.catalog.population(field.population).unwrap(),
                    )
                    .candidate_count()
                })
            })
            .fold(0u32, u32::saturating_add);
        if candidates > 250_000 {
            return Err(
                "Preview exceeds 250,000 candidates; reduce the patch size or preset density"
                    .into(),
            );
        }
        Ok(Product {
            scene,
            ground: cell.ground,
            terrain: cell.terrain,
            size: self.size as f32,
            candidates,
        })
    }
}
