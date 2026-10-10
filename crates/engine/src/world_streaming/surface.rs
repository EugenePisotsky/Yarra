//! CPU terrain relief and vegetation fields of resident cells, for gameplay and vegetation.
use super::*;

/// CPU-readable relief carried by a resident streamed terrain entity.
///
/// This is the bridge for vegetation, character grounding, interactions, and later shadow proxies:
/// every consumer samples the canonical cooked surface. With the terrain hierarchy
/// this entity owns only CPU data; it does not also create a ground mesh.
#[derive(Component, Debug, Clone)]
pub struct StreamedTerrainSurface {
    pub key: PageKey,
    pub cell_size: f32,
    pub heightfield: TerrainHeightfield,
}

/// Terrain-independent vegetation fields attached for one resident streamed cell.
///
/// Consumers join this component with the matching [`StreamedTerrainSurface`] by page key. No
/// height or normal samples are duplicated in the vegetation payload.
#[derive(Component, Debug, Clone)]
pub struct StreamedVegetationFieldPage {
    pub key: PageKey,
    pub cell_size: f32,
    pub data: VegetationFieldPageData,
}

impl StreamedTerrainSurface {
    pub fn sample_world(&self, world_xz: [f32; 2]) -> world::TerrainSurfaceSample {
        let origin = self.key.cell.origin(self.cell_size);
        self.heightfield.sample(
            [
                world_xz[0] - origin[0] as f32,
                world_xz[1] - origin[1] as f32,
            ],
            self.cell_size,
        )
    }

    pub fn contains_world(&self, world_xz: [f32; 2]) -> bool {
        let origin = self.key.cell.origin(self.cell_size);
        world_xz[0] >= origin[0] as f32
            && world_xz[1] >= origin[1] as f32
            && world_xz[0] <= origin[0] as f32 + self.cell_size
            && world_xz[1] <= origin[1] as f32 + self.cell_size
    }
}

/// Samples resident terrain from render-space X/Z coordinates.
///
/// The conversion through [`WorldOrigin`] keeps gameplay consumers correct after a floating-origin
/// rebase. At a shared page edge the lowest stable page key wins; cooked edge samples are exact, so
/// either page produces the same height and normal.
pub fn sample_resident_terrain_surface<'a>(
    origin: &WorldOrigin,
    surfaces: impl IntoIterator<Item = &'a StreamedTerrainSurface>,
    render_xz: [f32; 2],
) -> Option<world::TerrainSurfaceSample> {
    let active_space = origin.space()?;
    let mut selected: Option<(&StreamedTerrainSurface, [f32; 2])> = None;
    for surface in surfaces {
        if surface.key.space != active_space {
            continue;
        }
        let render_origin = origin.cell.origin(surface.cell_size);
        let world_xz = [
            render_xz[0] + render_origin[0] as f32,
            render_xz[1] + render_origin[1] as f32,
        ];
        if !surface.contains_world(world_xz) {
            continue;
        }
        if selected
            .as_ref()
            .is_none_or(|(current, _)| surface.key < current.key)
        {
            selected = Some((surface, world_xz));
        }
    }
    selected.map(|(surface, world_xz)| surface.sample_world(world_xz))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_terrain_sampling_converts_from_rebased_render_space() {
        let space = WorldSpaceId(7);
        let cell = CellCoord { x: 10, z: -4 };
        let origin = WorldOrigin {
            space: Some(space),
            cell,
        };
        let heightfield =
            TerrainHeightfield::from_heights(2, &[2.5, 2.5, 2.5, 2.5], 2.5, 2.5, 32.0).unwrap();
        let surface = StreamedTerrainSurface {
            key: PageKey {
                space,
                cell,
                domain: PageDomain::Terrain,
                lod: 0,
            },
            cell_size: 32.0,
            heightfield,
        };

        let sample = sample_resident_terrain_surface(&origin, [&surface], [16.0, 16.0]).unwrap();
        assert_eq!(sample.height, 2.5);
        assert_eq!(sample.normal, [0.0, 1.0, 0.0]);
    }
}
