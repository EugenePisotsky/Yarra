//! Detailed source demand stays independent of the distant terrain cover.
use super::*;
use bevy::math::DVec3;

pub(super) const PRELOAD_METERS: f64 = 16.;
pub(super) type Window = [CellCoord; 2];
pub(super) type Priority = (u8, f64);

#[derive(Resource, Default)]
pub(super) struct SourceView {
    space: Option<WorldSpaceId>,
    eye: DVec3,
    radius: f64,
}

pub(super) fn collect_view(
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    detail: Res<WorldDetailDemand>,
    cameras: Query<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    scene: Option<Res<vegetation_render::VegetationDebugScene>>,
    wind: Option<Res<vegetation_render::VegetationWind>>,
    mut view: ResMut<SourceView>,
) {
    view.space = None;
    if !detail.enabled() {
        return;
    }
    let Some(space) = origin.space().and_then(|id| catalog.world_space(id)) else {
        return;
    };
    let Some((_, transform)) = cameras.iter().find(|(c, _)| c.is_active) else {
        return;
    };
    let base = origin.cell().origin(space.cell_size);
    view.eye = transform.translation().as_dvec3() + DVec3::new(base[0], 0., base[1]);
    let wind = wind.as_deref().copied().unwrap_or_default();
    let vegetation = scene
        .as_ref()
        .map(|s| &s.scene().catalog)
        .or_else(|| catalog.vegetation());
    view.radius = vegetation.map_or(
        f64::from(vegetation_render::PROCEDURAL_DISTANCE_METERS),
        |c| f64::from(vegetation_render::terrain_contact_radius(c, &wind)),
    ) + PRELOAD_METERS;
    if view.eye.is_finite() && view.radius.is_finite() {
        view.space = Some(space.id);
    }
}

/// At most two bounded windows; never scan the rectangle between a detached camera
/// and a gameplay/editing focus. Identity depends on covered cells, not view rotation.
pub(super) fn windows(
    position: WorldPosition,
    space: &world_db::WorldSpaceRecord,
    view: &SourceView,
    hierarchy: bool,
) -> Result<Vec<Window>, String> {
    let c = position.cell;
    let mut windows = vec![[
        CellCoord {
            x: c.x.saturating_sub(INDEX_RADIUS_CELLS),
            z: c.z.saturating_sub(INDEX_RADIUS_CELLS),
        },
        CellCoord {
            x: c.x.saturating_add(INDEX_RADIUS_CELLS),
            z: c.z.saturating_add(INDEX_RADIUS_CELLS),
        },
    ]];
    // Query only cheap descriptors in XZ; use each compiled cell's actual height
    // bounds below. Authored world height ranges are not a culling certificate.
    if hierarchy && view.space == Some(position.space) {
        let size = f64::from(space.cell_size);
        let coordinate = |v: f64| -> Result<i32, String> {
            let v = (v / size).floor();
            if v < i32::MIN as f64 || v > i32::MAX as f64 {
                Err("source range exceeds cell coordinates".into())
            } else {
                Ok(v as i32)
            }
        };
        let camera = [
            CellCoord {
                x: coordinate(view.eye.x - view.radius)?,
                z: coordinate(view.eye.z - view.radius)?,
            },
            CellCoord {
                x: coordinate(view.eye.x + view.radius)?,
                z: coordinate(view.eye.z + view.radius)?,
            },
        ];
        if contains(camera, windows[0]) {
            windows[0] = camera;
        } else if !contains(windows[0], camera) {
            windows.push(camera);
        }
    }
    let cells: u64 = windows
        .iter()
        .map(|w| {
            ((i64::from(w[1].x) - i64::from(w[0].x) + 1) as u64)
                .saturating_mul((i64::from(w[1].z) - i64::from(w[0].z) + 1) as u64)
        })
        .fold(0, u64::saturating_add);
    if cells > world_db::MAX_CELL_DESCRIPTOR_QUERY as u64 {
        return Err(format!(
            "source index needs {cells} cells; limit is {}",
            world_db::MAX_CELL_DESCRIPTOR_QUERY
        ));
    }
    Ok(windows)
}
fn contains(a: Window, b: Window) -> bool {
    a[0].x <= b[0].x && a[0].z <= b[0].z && a[1].x >= b[1].x && a[1].z >= b[1].z
}

fn distance_squared(eye: DVec3, descriptor: &CellDescriptor, size: f32) -> f64 {
    let min = descriptor.cell.origin(size);
    let low = DVec3::new(min[0], f64::from(descriptor.minimum_y), min[1]);
    let high = DVec3::new(
        min[0] + f64::from(size),
        f64::from(descriptor.maximum_y),
        min[1] + f64::from(size),
    );
    eye.distance_squared(eye.clamp(low, high))
}

pub(super) fn demand(
    descriptors: &[CellDescriptor],
    position: WorldPosition,
    cell_size: f32,
    origin: CellCoord,
    camera: Option<&Frustum>,
    detail: bool,
    gameplay: bool,
    hierarchy: bool,
    view: &SourceView,
) -> BTreeMap<PageKey, Priority> {
    let mut result = BTreeMap::new();
    for d in descriptors {
        let cell_distance = d.cell.chebyshev_distance(position.cell);
        let local = cell_distance <= VISUAL_SOURCE_RESIDENCY_RADIUS_CELLS;
        let active = cell_distance <= GAMEPLAY_PRELOAD_RADIUS_CELLS;
        // Expanding source range must not implicitly expand detailed objects.
        let visible = local
            && detail
            && camera.is_some_and(|f| cell_intersects_frustum(f, d, origin, cell_size));
        let source_distance = distance_squared(view.eye, d, cell_size);
        let near = if hierarchy {
            view.space == Some(position.space) && source_distance <= view.radius.powi(2)
        } else {
            local
        };
        let terrain = if hierarchy {
            near || if gameplay { active } else { local }
        } else {
            visible || local
        };
        let priority = if active || (!gameplay && local) {
            (0, f64::from(cell_distance))
        } else {
            (1, source_distance)
        };
        for (domain, needed) in [
            (PageDomain::TerrainRender, terrain),
            (PageDomain::Vegetation, near),
            (PageDomain::StaticObjects, visible),
            (PageDomain::GameplayObjects, gameplay && active),
        ] {
            if needed && d.has_domain(domain) {
                let priority = if domain == PageDomain::StaticObjects {
                    (2, f64::from(cell_distance))
                } else {
                    priority
                };
                result.insert(
                    PageKey {
                        space: position.space,
                        cell: d.cell,
                        domain,
                        lod: 0,
                    },
                    priority,
                );
            }
        }
    }
    result
}

pub(super) fn compare(a: &(PageKey, Priority), b: &(PageKey, Priority)) -> std::cmp::Ordering {
    a.1.0
        .cmp(&b.1.0)
        .then_with(|| a.1.1.total_cmp(&b.1.1))
        .then_with(|| a.0.cmp(&b.0))
}

/// Keep CPU relief and surface inputs; GPU near shading admits its own bounded subset.
pub(super) fn attach_height_source(
    commands: &mut Commands,
    page: PreparedPage,
    cell_size: f32,
) -> Result<PageAttachment, String> {
    let key = page.decoded.key;
    let (heightfield, surfaces, weights) = match page.decoded.payload {
        PagePayload::TerrainHeightfield(t) => (t.heightfield, t.surfaces, t.weight_pages),
        PagePayload::TerrainRender(t) => (
            TerrainHeightfield::from_heights(2, &[t.height; 4], t.height, t.height, cell_size)
                .map_err(|e| e.to_string())?,
            t.surfaces,
            t.weight_pages,
        ),
        _ => return Err("height-only request returned a non-terrain page".into()),
    };
    heightfield.validate().map_err(|e| e.to_string())?;
    let bounds = [
        heightfield
            .heights
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min),
        heightfield
            .heights
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max),
    ];
    let near = page
        .terrain
        .map(|resources| {
            let layers = surfaces
                .iter()
                .map(|id| {
                    let r = resources
                        .surfaces
                        .iter()
                        .find(|s| s.surface.id == *id)
                        .ok_or("unresolved near terrain surface")?;
                    Ok(TerrainSurfaceLayer {
                        surface: r.surface.clone(),
                        layer: r.layer,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok::<_, String>(terrain_render::near::NearSource {
                key,
                cell_size,
                height_bounds: bounds,
                surfaces,
                weights,
                profile: resources.profile,
                texture_set: resources.texture_set,
                layers,
            })
        })
        .transpose()?;
    let entity = commands
        .spawn((
            StreamedTerrainSurface {
                key,
                cell_size,
                heightfield,
            },
            StreamedPageEntity(key),
            Name::new(format!("Terrain source {}, {}", key.cell.x, key.cell.z)),
        ))
        .id();
    if let Some(near) = near {
        commands.entity(entity).insert(near);
    }
    Ok(PageAttachment {
        entities: vec![entity],
        decoded_bytes: page.decoded.decoded_bytes,
        height_only_pages: 1,
        ..default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn position() -> WorldPosition {
        WorldPosition {
            space: WorldSpaceId(1),
            cell: CellCoord::ZERO,
            local: [0.; 3],
        }
    }
    fn view(eye: DVec3) -> SourceView {
        SourceView {
            space: Some(WorldSpaceId(1)),
            eye,
            radius: 112.,
        }
    }
    fn space(size: f32) -> world_db::WorldSpaceRecord {
        world_db::WorldSpaceRecord {
            id: WorldSpaceId(1),
            name: "test".into(),
            cell_size: size,
            minimum_y: -100.,
            maximum_y: 100.,
        }
    }
    fn descriptor(cell: CellCoord) -> CellDescriptor {
        CellDescriptor {
            cell,
            minimum_y: 0.,
            maximum_y: 1.,
            domain_mask: [
                PageDomain::TerrainRender,
                PageDomain::Vegetation,
                PageDomain::StaticObjects,
                PageDomain::GameplayObjects,
            ]
            .into_iter()
            .map(world_db::domain_bit)
            .sum(),
        }
    }
    #[test]
    fn small_cells_cover_blade_range_without_loading_distant_objects_or_gameplay() {
        let view = view(DVec3::new(1., 2., 1.));
        let windows = windows(position(), &space(8.), &view, true).unwrap();
        assert_eq!(windows.len(), 1);
        assert!(windows[0][0].x <= -14 && windows[0][1].x >= 14);
        let distant = descriptor(CellCoord { x: 10, z: 0 });
        let local = descriptor(CellCoord::ZERO);
        let frustum = Frustum::default();
        let selected = demand(
            &[local, distant],
            position(),
            8.,
            CellCoord::ZERO,
            Some(&frustum),
            true,
            true,
            true,
            &view,
        );
        let has = |cell, domain| {
            selected.contains_key(&PageKey {
                space: WorldSpaceId(1),
                cell,
                domain,
                lod: 0,
            })
        };
        assert!(has(CellCoord { x: 10, z: 0 }, PageDomain::Vegetation));
        assert!(has(CellCoord { x: 10, z: 0 }, PageDomain::TerrainRender));
        assert!(!has(CellCoord { x: 10, z: 0 }, PageDomain::StaticObjects));
        assert!(!has(CellCoord { x: 10, z: 0 }, PageDomain::GameplayObjects));
        assert!(has(CellCoord::ZERO, PageDomain::GameplayObjects));
        let legacy = demand(
            &[descriptor(CellCoord { x: 10, z: 0 })],
            position(),
            8.,
            CellCoord::ZERO,
            Some(&frustum),
            true,
            true,
            false,
            &view,
        );
        assert!(legacy.is_empty());
    }
    #[test]
    fn elevated_views_keep_local_consumers_but_do_not_load_valley_grass() {
        let view = view(DVec3::new(1., 1000., 1.));
        assert_eq!(
            windows(position(), &space(8.), &view, true).unwrap().len(),
            1
        );
        let selected = demand(
            &[
                descriptor(CellCoord::ZERO),
                descriptor(CellCoord { x: 10, z: 0 }),
            ],
            position(),
            8.,
            CellCoord::ZERO,
            None,
            true,
            true,
            true,
            &view,
        );
        assert_eq!(selected.len(), 2); // Actor terrain plus gameplay; no grass or far sources.
        assert!(
            selected
                .keys()
                .all(|k| k.cell == CellCoord::ZERO && k.domain != PageDomain::Vegetation)
        );
    }
    #[test]
    fn detached_camera_queries_separate_windows_and_rejects_oversized_ranges() {
        let view = view(DVec3::new(-100_000., 2., 100_000.));
        let windows = windows(position(), &space(8.), &view, true).unwrap();
        assert_eq!(windows.len(), 2);
        assert!(
            !windows
                .iter()
                .any(|w| contains(*w, [CellCoord { x: -5000, z: 5000 }; 2]))
        );
        assert!(super::windows(position(), &space(0.01), &view, true).is_err());
        let mut huge = view;
        huge.radius = 1e20;
        assert!(super::windows(position(), &space(8.), &huge, true).is_err());
    }
    #[test]
    fn pending_work_is_prioritized_by_consumer_and_distance() {
        let view = view(DVec3::ZERO);
        let d = demand(
            &[
                descriptor(CellCoord { x: -12, z: 0 }),
                descriptor(CellCoord { x: 1, z: 0 }),
                descriptor(CellCoord { x: 5, z: 0 }),
            ],
            position(),
            8.,
            CellCoord::ZERO,
            None,
            true,
            true,
            true,
            &view,
        );
        let mut order: Vec<_> = d.into_iter().collect();
        order.sort_by(compare);
        assert_eq!(order[0].0.cell, CellCoord { x: 1, z: 0 });
        assert_eq!(order.last().unwrap().0.cell, CellCoord { x: -12, z: 0 });
    }

    fn height_page(key: PageKey) -> PreparedPage {
        PreparedPage {
            height_only: true,
            terrain: None,
            definitions: vec![],
            dependencies: vec![],
            decoded: DecodedPage {
                key,
                decoded_bytes: 64,
                gpu_bytes_estimate: 0,
                payload: PagePayload::TerrainHeightfield(world::TerrainHeightfieldPage {
                    heightfield: TerrainHeightfield::from_heights(2, &[1., 2., 4., 8.], 1., 8., 8.)
                        .unwrap(),
                    surfaces: vec![],
                    weight_pages: vec![],
                }),
            },
        }
    }
    #[test]
    fn height_sources_preserve_relief_without_allocating_render_assets() {
        let mut world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        let key = PageKey {
            space: WorldSpaceId(1),
            cell: CellCoord { x: -3, z: 2 },
            domain: PageDomain::TerrainRender,
            lod: 0,
        };
        let attachment = attach_height_source(&mut commands, height_page(key), 8.).unwrap();
        queue.apply(&mut world);
        assert_eq!(attachment.gpu_bytes_estimate, 0);
        assert_eq!(attachment.height_only_pages, 1);
        assert!(
            attachment.owned_terrain_meshes.is_empty()
                && attachment.owned_terrain_materials.is_empty()
                && attachment.owned_terrain_images.is_empty()
                && attachment.terrain_texture_set.is_none()
        );
        let entity = attachment.entities[0];
        assert!(world.get::<Mesh3d>(entity).is_none());
        let surface = world.get::<StreamedTerrainSurface>(entity).unwrap();
        assert_eq!(surface.key, key);
        assert_eq!(surface.heightfield.heights, vec![1., 2., 4., 8.]);
        assert!((surface.sample_world([-22., 22.]).height - 4.25).abs() < 1e-5);
    }
    #[test]
    fn draining_the_worker_queue_does_not_bypass_the_pending_page_cap() {
        let (requests, rx) = bounded(MAX_DATABASE_REQUESTS_IN_FLIGHT);
        let (_tx, results) = bounded(1);
        let mut app = App::new();
        app.insert_resource(WorldDatabaseWorker {
            requests,
            results,
            thread: None,
        })
        .insert_resource(WorldStreamingConfig::game())
        .insert_resource(WorldDetailDemand::default())
        .insert_resource(ActiveWorldSpace {
            current: Some(WorldSpaceId(1)),
            ..default()
        })
        .insert_resource(WorldOrigin {
            space: Some(WorldSpaceId(1)),
            cell: CellCoord::ZERO,
        })
        .insert_resource(WorldViewpoint {
            position: Some(position()),
        })
        .insert_resource(view(DVec3::ZERO))
        .insert_resource(WorldStream {
            phase: StreamPhase::Ready,
            height_only: true,
            manifest: Some(RuntimeManifest {
                schema_version: world::RUNTIME_SCHEMA_VERSION,
                generation_id: "test".into(),
                content_hash: [0; 32],
                default_world_space: WorldSpaceId(1),
                world_spaces: vec![space(8.)],
                vegetation_catalog: None,
            }),
            descriptors: (-3..=3)
                .flat_map(|x| (-3..=3).map(move |z| descriptor(CellCoord { x, z })))
                .collect(),
            ..default()
        })
        .add_systems(Update, calculate_page_demand);
        app.update();
        assert_eq!(rx.try_iter().count(), MAX_DATABASE_REQUESTS_IN_FLIGHT);
        let key = *app
            .world()
            .resource::<WorldStream>()
            .pages
            .keys()
            .next()
            .unwrap();
        app.world_mut()
            .resource_mut::<WorldStream>()
            .pages
            .insert(key, PageState::Prepared(height_page(key)));
        app.update();
        assert_eq!(
            rx.try_iter().count(),
            0,
            "queued, decoding and prepared work share the cap"
        );
        app.world_mut()
            .resource_mut::<WorldStream>()
            .pages
            .insert(key, PageState::Resident(default()));
        app.update();
        assert_eq!(rx.try_iter().count(), 1);
    }
}
