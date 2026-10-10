//! Where render space is: the logical viewpoint streaming follows, and the world cell at the
//! render origin, which moves with the viewpoint in the editor (floating origin).
use super::*;

/// Logical position around which the world index and preload set are requested.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct WorldViewpoint {
    pub(super) position: Option<WorldPosition>,
}

impl WorldViewpoint {
    pub fn position(&self) -> Option<WorldPosition> {
        self.position
    }

    pub fn set(&mut self, position: WorldPosition) {
        self.position = Some(position);
    }
}

/// The logical cell represented by render-space origin.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct WorldOrigin {
    pub(super) space: Option<WorldSpaceId>,
    pub(super) cell: CellCoord,
}

impl WorldOrigin {
    pub fn space(&self) -> Option<WorldSpaceId> {
        self.space
    }

    pub fn cell(&self) -> CellCoord {
        self.cell
    }

    fn offset(&self, catalog: &WorldCatalog) -> Option<(WorldSpaceId, [f64; 2])> {
        let space = self.space?;
        let size = catalog.world_space(space)?.cell_size;
        Some((space, self.cell.origin(size)))
    }

    /// The world position of a render-space one, once a world is open.
    pub fn to_world(
        &self,
        catalog: &WorldCatalog,
        render: Vec3,
    ) -> Option<(WorldSpaceId, [f64; 3])> {
        let (space, [x, z]) = self.offset(catalog)?;
        Some((
            space,
            [
                f64::from(render.x) + x,
                f64::from(render.y),
                f64::from(render.z) + z,
            ],
        ))
    }

    /// Where a world position is in render space. Only positions in the open space have one.
    pub fn to_render(
        &self,
        catalog: &WorldCatalog,
        space: WorldSpaceId,
        world: [f64; 3],
    ) -> Option<Vec3> {
        let (open, [x, z]) = self.offset(catalog)?;
        if open != space {
            return None;
        }
        Some(Vec3::new(
            (world[0] - x) as f32,
            world[1] as f32,
            (world[2] - z) as f32,
        ))
    }
}

pub(super) fn sync_stream_focus_to_viewpoint(
    active_space: Res<ActiveWorldSpace>,
    origin: Res<WorldOrigin>,
    stream: Res<WorldStream>,
    focuses: Query<&Transform, With<WorldStreamFocus>>,
    mut viewpoint: ResMut<WorldViewpoint>,
) {
    let Some(space_id) = active_space.current else {
        return;
    };
    let Some(space) = stream
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.world_space(space_id))
    else {
        return;
    };
    let Some(transform) = focuses.iter().next() else {
        return;
    };
    let origin_world = origin.cell.origin(space.cell_size);
    viewpoint.set(WorldPosition::from_world(
        space_id,
        [
            origin_world[0] + f64::from(transform.translation.x),
            f64::from(transform.translation.y),
            origin_world[1] + f64::from(transform.translation.z),
        ],
        space.cell_size,
    ));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_world_origin(
    mut commands: Commands,
    config: Res<WorldStreamingConfig>,
    terrain_lod: Res<TerrainHierarchy>,
    viewpoint: Res<WorldViewpoint>,
    mut origin: ResMut<WorldOrigin>,
    mut terrain_meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    mut stream: ResMut<WorldStream>,
    mut residency: ResMut<SourceResidency>,
    mut roots: rebase::Roots,
) {
    let Some(position) = viewpoint.position else {
        return;
    };
    let desired_cell = desired_origin_cell(
        rebase::effective_config(*config, terrain_lod.enabled),
        *origin,
        position,
    );
    if origin.space == Some(position.space)
        && origin.cell == desired_cell
        && stream.height_only == terrain_lod.enabled
    {
        return;
    }

    let previous = *origin;
    origin.space = Some(position.space);
    origin.cell = desired_cell;
    // CPU source pages use canonical keys. Keep them and their pending work on a
    // same-space rebase; only render roots need translating. The editable renderer
    // still rebuilds its materials using the editor's existing origin contract.
    if previous.space == origin.space
        && let Some(size) = stream
            .manifest
            .as_ref()
            .and_then(|m| m.world_space(position.space))
            .map(|s| s.cell_size)
    {
        rebase::shift_roots(
            &mut roots,
            previous.cell,
            origin.cell,
            size,
            terrain_lod.enabled && stream.height_only,
        );
    }
    if previous.space != origin.space || !terrain_lod.enabled || !stream.height_only {
        clear_streamed_pages(
            &mut commands,
            &mut terrain_meshes,
            &mut terrain_materials,
            &mut terrain_images,
            &mut stream,
            &mut residency,
        );
    }
    stream.height_only = terrain_lod.enabled;
    info!(
        "rebased render origin from {:?}:{:?} to {:?}:{:?}",
        previous.space, previous.cell, origin.space, origin.cell
    );
}

pub(super) fn desired_origin_cell(
    config: WorldStreamingConfig,
    origin: WorldOrigin,
    position: WorldPosition,
) -> CellCoord {
    match config.floating_origin_threshold_cells {
        Some(threshold)
            if origin.space != Some(position.space)
                || origin.cell.chebyshev_distance(position.cell) > threshold =>
        {
            position.cell
        }
        Some(_) => origin.cell,
        None => CellCoord::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_and_render_positions_convert_through_the_origin_cell() {
        // 16 m cells, so the render origin sits at world (32, -16).
        let (catalog, origin) =
            test_world_resources(WorldSpaceId(7), CellCoord { x: 2, z: -1 }, None);
        let render = Vec3::new(1.0, 5.0, 2.0);
        let (space, world) = origin.to_world(&catalog, render).unwrap();
        assert_eq!((space, world), (WorldSpaceId(7), [33.0, 5.0, -14.0]));
        assert_eq!(origin.to_render(&catalog, space, world), Some(render));
        // A position in another space is not in this one's render space.
        assert!(origin.to_render(&catalog, WorldSpaceId(8), world).is_none());
        // Before a world is open there is nothing to convert against.
        let unopened = WorldOrigin::default();
        assert!(unopened.to_world(&catalog, render).is_none());
        let none = WorldCatalog::default();
        assert!(origin.to_render(&none, space, world).is_none());
    }

    #[test]
    fn editor_origin_rebases_only_after_its_threshold() {
        let origin = WorldOrigin {
            space: Some(WorldSpaceId(1)),
            cell: CellCoord { x: 10, z: 20 },
        };
        let nearby = WorldPosition {
            space: WorldSpaceId(1),
            cell: CellCoord { x: 18, z: 12 },
            local: [0.0; 3],
        };
        let remote = WorldPosition {
            cell: CellCoord { x: 19, z: 12 },
            ..nearby
        };

        assert_eq!(
            desired_origin_cell(WorldStreamingConfig::editor(), origin, nearby),
            origin.cell
        );
        assert_eq!(
            desired_origin_cell(WorldStreamingConfig::editor(), origin, remote),
            remote.cell
        );
        assert_eq!(
            desired_origin_cell(WorldStreamingConfig::game(), origin, remote),
            CellCoord::ZERO
        );
    }
}
