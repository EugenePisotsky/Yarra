use super::*;

/// A certified, bounded source-height halo. Absent cells in `loaded_cells` are world edges.
/// Heights must be original authoring data, never previously graded/cooked pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainSource {
    pub space: WorldSpaceId,
    pub cell_size: f32,
    pub minimum_height: f32,
    pub maximum_height: f32,
    pub loaded_cells: Vec<CellCoord>,
    pub cells: Vec<TerrainSourceCell>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainSourceCell {
    /// Original constant-height cells can retain a four-vertex mesh when relief is absent.
    pub flat: bool,
    pub cell: CellCoord,
    pub revision: u64,
    pub resolution: u16,
    pub heights: Vec<f32>,
}
impl TerrainSource {
    fn validate(&self, targets: &[CellCoord]) -> Result<(), CompileError> {
        if self.loaded_cells.len() > 9
            || self.cells.len() > 9
            || !self.cell_size.is_finite()
            || self.cell_size <= 0.0
            || !self.minimum_height.is_finite()
            || !self.maximum_height.is_finite()
            || self.minimum_height > self.maximum_height
        {
            return Err(road_error("invalid source-height halo"));
        }
        let loaded = self
            .loaded_cells
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        if loaded.len() != self.loaded_cells.len() {
            return Err(road_error("duplicate height halo cell"));
        }
        let mut pages = BTreeMap::new();
        for page in &self.cells {
            if !loaded.contains(&page.cell)
                || pages.insert(page.cell, page).is_some()
                || !(2..=world::MAX_TERRAIN_HEIGHTFIELD_RESOLUTION).contains(&page.resolution)
                || page.heights.len() != usize::from(page.resolution).pow(2)
                || (page.flat && page.heights.windows(2).any(|v| v[0] != v[1]))
                || page
                    .heights
                    .iter()
                    .any(|h| !h.is_finite() || *h < self.minimum_height || *h > self.maximum_height)
            {
                return Err(road_error("invalid original terrain height samples"));
            }
        }
        for &cell in targets {
            if !pages.contains_key(&cell) {
                return Err(road_error("missing original terrain cell"));
            }
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let neighbour = offset(cell, dx, dz)?;
                    if !loaded.contains(&neighbour) {
                        return Err(road_error("incomplete original terrain halo"));
                    }
                }
            }
        }
        for page in &self.cells {
            let n = usize::from(page.resolution);
            for (dx, dz) in [(1, 0), (0, 1)] {
                let Some(other) = pages.get(&offset(page.cell, dx, dz)?) else {
                    continue;
                };
                if other.resolution != page.resolution {
                    return Err(road_error("adjacent heightfields need the same resolution"));
                }
                for i in 0..n {
                    let (a, b) = if dx == 1 {
                        (i * n + n - 1, i * n)
                    } else {
                        ((n - 1) * n + i, i)
                    };
                    if page.heights[a] != other.heights[b] {
                        return Err(road_error("original heightfield borders disagree"));
                    }
                }
            }
        }
        Ok(())
    }
}
fn offset(cell: CellCoord, dx: i32, dz: i32) -> Result<CellCoord, CompileError> {
    Ok(CellCoord {
        x: cell
            .x
            .checked_add(dx)
            .ok_or(road_error("height halo coordinate overflow"))?,
        z: cell
            .z
            .checked_add(dz)
            .ok_or(road_error("height halo coordinate overflow"))?,
    })
}

impl RoadPlan {
    fn depression(&self, cell: CellCoord, uv: [f64; 2]) -> f64 {
        let world = [
            (f64::from(cell.x) + uv[0]) * f64::from(self.size),
            (f64::from(cell.z) + uv[1]) * f64::from(self.size),
        ];
        let roads = self.cells[&cell]
            .iter()
            .map(|(index, curves)| {
                let road = &self.roads[*index];
                let p = &road.profile;
                let relief = &p.relief;
                if !relief.enabled() {
                    return 0.0;
                }
                let closest = curves
                    .iter()
                    .map(|i| road.curves[*i].closest(cell, uv, f64::from(self.size)))
                    .min_by(|a, b| a.distance_squared.total_cmp(&b.distance_squared))
                    .unwrap();
                // Entirely inside the corridor: independent of cosmetic edge noise and wear.
                let bed = smooth(
                    (closest.width * 0.5 - closest.distance_squared.sqrt())
                        / f64::from(relief.shoulder_falloff).min(closest.width * 0.5),
                );
                let signed_noise = |length, seed| noise::patch(world, length, seed) * 2.0 - 1.0;
                let broad = signed_noise(relief.road_variation_length, road.source.seed ^ 0x906a);
                let depth = f64::from(relief.road_depth)
                    * (1.0 + f64::from(relief.road_variation) * broad)
                    * bed;
                let shared = signed_noise(relief.track_variation_length, road.source.seed ^ 0x418c);
                let mut ruts: f64 = 0.0;
                for (side, salt) in [(-1.0, 0x41ab), (1.0, 0x873d)] {
                    let distance = (closest.lateral - side * f64::from(p.track_spacing) * 0.5)
                        .hypot(closest.longitudinal);
                    let t = (distance / (f64::from(p.track_width) * 0.5)).min(1.0);
                    let shape =
                        smooth(1.0 - t.powf(2.0 + 6.0 * (1.0 - f64::from(relief.rut_roundness))));
                    let uneven = shared * 0.65
                        + signed_noise(relief.track_variation_length, road.source.seed ^ salt)
                            * 0.35;
                    ruts = ruts.max(
                        f64::from(relief.track_depth)
                            * (1.0 + f64::from(relief.track_variation) * uneven)
                            * shape,
                    );
                }
                self.joined_depth(road.source.id, cell, uv, depth + ruts)
            })
            .fold(0.0, f64::max);
        self.junctions
            .iter()
            .map(|j| j.depth(cell, uv, self.size) * j.blend(cell, uv, self.size))
            .fold(roads, f64::max)
    }

    fn terrain(
        &self,
        cell: CellCoord,
        source: &TerrainSource,
    ) -> Result<world::TerrainHeightfield, CompileError> {
        let pages = source
            .cells
            .iter()
            .map(|p| (p.cell, p))
            .collect::<BTreeMap<_, _>>();
        let page = pages[&cell];
        let n = usize::from(page.resolution);
        let step = source.cell_size / (n - 1) as f32;
        for (index, curves) in &self.cells[&cell] {
            let p = &self.roads[*index].profile;
            if !curves.is_empty() {
                p.validate_relief_detail(step).map_err(road_error)?;
            }
        }
        for j in &self.junctions {
            if j.bounds.contains(cell) {
                j.profile.validate_relief_detail(step).map_err(road_error)?;
            }
        }
        let sample = |page: &TerrainSourceCell, x: usize, z: usize| {
            let count = usize::from(page.resolution);
            page.heights[z * count + x]
                - self.depression(
                    page.cell,
                    [x as f64 / (count - 1) as f64, z as f64 / (count - 1) as f64],
                ) as f32
        };
        let mut heights = Vec::with_capacity(n * n);
        let mut normals = Vec::with_capacity(n * n);
        for z in 0..n {
            for x in 0..n {
                let center = sample(page, x, z);
                heights.push(center);
                let neighbour = |dx: i32, dz: i32| -> Result<(f32, f32), CompileError> {
                    let nx = x as i32 + dx;
                    let nz = z as i32 + dz;
                    if (0..n as i32).contains(&nx) && (0..n as i32).contains(&nz) {
                        return Ok((sample(page, nx as usize, nz as usize), 1.0));
                    }
                    let other = offset(
                        cell,
                        if nx < 0 {
                            -1
                        } else if nx >= n as i32 {
                            1
                        } else {
                            0
                        },
                        if nz < 0 {
                            -1
                        } else if nz >= n as i32 {
                            1
                        } else {
                            0
                        },
                    )?;
                    if let Some(page) = pages.get(&other) {
                        let ox = if nx < 0 {
                            n - 2
                        } else if nx >= n as i32 {
                            1
                        } else {
                            nx as usize
                        };
                        let oz = if nz < 0 {
                            n - 2
                        } else if nz >= n as i32 {
                            1
                        } else {
                            nz as usize
                        };
                        Ok((sample(page, ox, oz), 1.0))
                    } else {
                        Ok((center, 0.0))
                    }
                };
                let (l, ls) = neighbour(-1, 0)?;
                let (r, rs) = neighbour(1, 0)?;
                let (d, ds) = neighbour(0, -1)?;
                let (u, us) = neighbour(0, 1)?;
                let dx = (r - l) / ((ls + rs) * step);
                let dz = (u - d) / ((us + ds) * step);
                let length = (dx * dx + 1.0 + dz * dz).sqrt();
                normals.push([-dx / length, 1.0 / length, -dz / length]);
            }
        }
        if page.flat
            && heights.windows(2).all(|v| v[0] == v[1])
            && normals.iter().all(|n| *n == [0.0, 1.0, 0.0])
        {
            return world::TerrainHeightfield::from_heights_and_normals(
                2,
                &[heights[0]; 4],
                &[[0.0, 1.0, 0.0]; 4],
                source.minimum_height,
                source.maximum_height,
            )
            .map_err(|_| road_error("flat terrain exceeds world height bounds"));
        }
        world::TerrainHeightfield::from_heights_and_normals(
            page.resolution,
            &heights,
            &normals,
            source.minimum_height,
            source.maximum_height,
        )
        .map_err(|_| {
            road_error(
                "road relief exceeds world height bounds or creates invalid terrain; reduce depth",
            )
        })
    }
}

impl CompilePlan {
    /// One output and its certified source halo, used identically by live preview and cooking.
    pub fn compile_cell_with_terrain(
        &self,
        cell: CellCoord,
        coverage: &environment::CoverageSnapshot,
        roads: &RoadSnapshot,
        terrain: &TerrainSource,
        profile: RoadCompileProfile,
    ) -> Result<CompiledCell, CompileError> {
        terrain.validate(&[cell])?;
        if terrain.space != self.definition.space || terrain.cell_size != self.definition.cell_size
        {
            return Err(road_error("source terrain belongs to another world/grid"));
        }
        let mut halo = terrain.loaded_cells.clone();
        halo.sort();
        let road_plan = RoadPlan::new(self, &halo, roads, profile)?;
        let segments = road_plan
            .roads
            .iter()
            .flat_map(|r| &r.curves)
            .map(|c| c.vertices.len() - 1)
            .sum::<usize>();
        let n = usize::from(
            terrain
                .cells
                .iter()
                .find(|p| p.cell == cell)
                .unwrap()
                .resolution,
        );
        if (n * n * 5).saturating_mul(
            segments
                .saturating_add(road_plan.junctions.len() * 16)
                .saturating_add(32),
        ) > road_plan.profile.max_sample_work
        {
            return Err(CompileError::Budget("road relief sample work"));
        }
        let mut compiled = self
            .compile_with_influences(&[cell], coverage, Some(&road_plan))?
            .remove(0);
        compiled.terrain = Some(road_plan.terrain(cell, terrain)?);
        compiled.objects = self.scatter(
            cell,
            &crate::coverage::Coverage::new(self, coverage)?,
            Some(&road_plan),
            compiled.terrain.as_ref(),
        )?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"yarra.road-relief.v1");
        hash.update(&compiled.input_fingerprint);
        let mut canonical = terrain.clone();
        canonical.loaded_cells.sort();
        canonical.cells.sort_by_key(|c| c.cell);
        hash.update(&bincode::serde::encode_to_vec(
            &canonical,
            bincode::config::standard(),
        )?);
        for c in halo {
            hash.update(&road_plan.fingerprint(c, [0; 32])?);
        }
        compiled.input_fingerprint = *hash.finalize().as_bytes();
        Ok(compiled)
    }
}
