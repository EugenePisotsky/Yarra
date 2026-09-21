use super::*;
use crate::actor::TerrainGrounded;
use lod::contact::{ContactCertificate, ContactPriority, ContactRegion, MAX_CONTACT_REGIONS};
use vegetation_render::{VegetationDebugScene, VegetationTerrainGate, VegetationWind};

const GUARD_METERS: f64 = 16.;
const ACTOR_RADIUS: f64 = 0.35;
const MAX_CACHED_PAGES: usize = 4096;
// At most one largest supported grid, or a batch of smaller grids, per frame.
const SOURCE_SAMPLES_PER_FRAME: usize = 257 * 257;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ContactSystems {
    Collect,
    Publish,
}

#[derive(Resource, Default)]
pub(super) struct ContactInputs {
    pub planning: Vec<ContactRegion>,
    pub required: Vec<ContactRegion>,
    pub error: Option<String>,
    camera: Option<DVec3>,
    view: Option<LodView>,
    radius: f64,
    cache_identity: Option<(u64, CellCoord, String, WorldSpaceId)>,
    pages: Vec<ContactPage>,
    scene: Option<VegetationDebugScene>,
}
struct ContactPage {
    id: [u32; 3],
    key: Option<TerrainNodeKey>,
    bounds: [DVec3; 2],
    source_index: usize,
    source_error: Option<f32>,
}

/// Accuracy of the currently drawn terrain, not a planned replacement. It is
/// refreshed before extraction/visibility and also gates prospective actor steps.
#[derive(Resource, Default)]
pub struct TerrainContactReadiness {
    enabled: bool,
    space: Option<WorldSpaceId>,
    identity: Option<(String, WorldSpaceId)>,
    cell_size: f64,
    domain: Vec<[DVec3; 2]>,
    certificates: Vec<ContactCertificate>,
}
impl TerrainContactReadiness {
    pub fn permits(&self, region: &ContactRegion) -> bool {
        !self.enabled
            || (region.validate()
                && lod::contact::covered(region, self.domain.iter().copied())
                && self.certificates.iter().all(|c| region.accepts(c)))
    }
    pub(crate) fn actor_ready(&self, position: Vec3, origin: &WorldOrigin) -> bool {
        if !self.enabled {
            return true;
        }
        if origin.space() != self.space {
            return false;
        }
        let shift = origin_shift(origin.cell(), self.cell_size);
        self.permits(&actor_region(position.as_dvec3() + shift, ACTOR_RADIUS))
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<ContactInputs>()
        .init_resource::<TerrainContactReadiness>()
        .add_systems(PostUpdate, collect.in_set(ContactSystems::Collect))
        .add_systems(PostUpdate, publish.in_set(ContactSystems::Publish));
}
fn origin_shift(origin: CellCoord, size: f64) -> DVec3 {
    DVec3::new(origin.x as f64 * size, 0., origin.z as f64 * size)
}
fn actor_region(point: DVec3, radius: f64) -> ContactRegion {
    // Grounded actors require the surface beneath them even before its height is
    // loaded (e.g. teleport to a mountain). Flying actors do not carry this role.
    ContactRegion {
        bounds: [
            DVec3::new(point.x - radius, -f32::MAX as f64, point.z - radius),
            DVec3::new(point.x + radius, f32::MAX as f64, point.z + radius),
        ],
        exact: true,
        tolerance: 0.,
        priority: ContactPriority::Actor,
    }
}
fn distance(point: DVec3, bounds: [DVec3; 2]) -> f64 {
    point.distance(point.clamp(bounds[0], bounds[1]))
}
fn collect(
    config: Res<TerrainLodPreview>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    active_space: Res<ActiveWorldSpace>,
    scene: Option<Res<VegetationDebugScene>>,
    wind: Option<Res<VegetationWind>>,
    camera: Query<
        (
            &Camera,
            &GlobalTransform,
            Option<&bevy::camera::MainPassResolutionOverride>,
        ),
        With<WorldViewCamera>,
    >,
    actors: Query<&GlobalTransform, With<TerrainGrounded>>,
    mut inputs: ResMut<ContactInputs>,
) {
    inputs.planning.clear();
    inputs.required.clear();
    inputs.error = None;
    inputs.camera = None;
    inputs.view = None;
    let Some((projection, camera, resolution)) = camera
        .iter()
        .find(|(c, _, _)| c.is_active)
        .filter(|_| config.enabled)
    else {
        return;
    };
    let Some(space) = active_space
        .current()
        .and_then(|id| catalog.world_space(id))
    else {
        return;
    };
    let shift = origin_shift(origin.cell(), space.cell_size as f64);
    let eye = camera.translation().as_dvec3() + shift;
    inputs.camera = Some(eye);
    if let Some(size) = resolution
        .map(|r| r.0)
        .or_else(|| projection.physical_viewport_size())
    {
        inputs.view = Some(LodView {
            clip_from_world: projection.clip_from_view().as_dmat4()
                * camera.to_matrix().as_dmat4().inverse()
                * DMat4::from_translation(-shift),
            contact_position: eye,
            viewport: [size.x, size.y],
        });
    }
    for actor in &actors {
        let point = actor.translation().as_dvec3() + shift;
        inputs.required.push(actor_region(point, ACTOR_RADIUS));
        inputs.planning.push(actor_region(
            point,
            config.settings.exact_radius.max(ACTOR_RADIUS) + GUARD_METERS,
        ));
        if inputs.planning.len() > MAX_CONTACT_REGIONS {
            inputs.error = Some("too many terrain contact regions".into());
            return;
        }
    }
    let Some(scene) = scene else {
        inputs.pages.clear();
        inputs.scene = None;
        inputs.cache_identity = None;
        return;
    };
    let identity = (
        scene.revision(),
        origin.cell(),
        catalog.generation_id().to_owned(),
        space.id,
    );
    if inputs.cache_identity.as_ref() != Some(&identity) {
        // A streamed page or coverage edit changes the scene revision, but does
        // not invalidate the height certificate of every other resident page.
        // Canonical keys survive rebasing; the generation/space boundary does not.
        let reuse = inputs
            .cache_identity
            .as_ref()
            .is_some_and(|old| old.2 == identity.2 && old.3 == identity.3);
        let previous: BTreeMap<_, _> = std::mem::take(&mut inputs.pages)
            .into_iter()
            .filter_map(|p| p.key.map(|k| (k, p)))
            .collect();
        inputs.cache_identity = None;
        if scene.scene().pages.len() > MAX_CACHED_PAGES {
            inputs.scene = None;
            inputs.error = Some("vegetation contact page budget exceeded".into());
            return;
        }
        for (source_index, page) in scene.scene().pages.iter().enumerate() {
            if page
                .fields
                .iter()
                .all(|f| f.coverage.iter().all(|&v| v == 0))
            {
                continue;
            }
            let x = f64::from(page.origin_xz[0]) + shift.x;
            let z = f64::from(page.origin_xz[1]) + shift.z;
            let cell = CellCoord {
                x: (x / space.cell_size as f64).round() as i32,
                z: (z / space.cell_size as f64).round() as i32,
            };
            let leaf = TerrainNodeKey::leaf(space.id, cell);
            let rendered_min = patch_transform(leaf, origin.cell(), space.cell_size).translation;
            // Compare actual render coordinates; near alignment is not sufficient
            // to certify contact with a cliff or a narrow road rut.
            let key = (page.size == space.cell_size
                && page.origin_xz == [rendered_min.x, rendered_min.z])
            .then_some(leaf);
            let source_error = key.and_then(|k| previous.get(&k)).and_then(|old| {
                let prior = inputs.scene.as_ref()?.scene().pages.get(old.source_index)?;
                reuse
                    .then(|| reusable_source_error(old.source_error, prior, page))
                    .flatten()
            });
            let [x, z] = if key.is_some() {
                cell.origin(space.cell_size)
            } else {
                [x, z]
            };
            let (low, high) = page
                .surface
                .heights
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), &h| {
                    (low.min(h), high.max(h))
                });
            inputs.pages.push(ContactPage {
                id: VegetationTerrainGate::page_id(page),
                key,
                source_index,
                source_error,
                bounds: [
                    DVec3::new(x, low as f64, z),
                    DVec3::new(x + page.size as f64, high as f64, z + page.size as f64),
                ],
            });
        }
        inputs.scene = Some(scene.clone());
        inputs.cache_identity = Some(identity);
    }
    inputs.radius = f64::from(vegetation_render::terrain_contact_radius(
        &scene.scene().catalog,
        wind.as_deref().unwrap_or(&VegetationWind::default()),
    ));
    let mut requests = Vec::new();
    let mut required = Vec::new();
    for page in &inputs.pages {
        // Only nearby roots need detailed terrain. A valley below an elevated view
        // is not nearby merely because it shares the camera's horizontal cell.
        let d = distance(eye, page.bounds);
        if d <= inputs.radius + GUARD_METERS {
            let mut bounds = page.bounds;
            bounds[0] -= DVec3::new(GUARD_METERS, 0., GUARD_METERS);
            bounds[1] += DVec3::new(GUARD_METERS, 0., GUARD_METERS);
            requests.push(ContactRegion {
                bounds,
                exact: true,
                tolerance: 0.,
                priority: ContactPriority::Vegetation,
            });
        }
        if d <= inputs.radius {
            required.push(ContactRegion {
                bounds: page.bounds,
                exact: true,
                tolerance: 0.,
                priority: ContactPriority::Vegetation,
            });
        }
    }
    // Small source cells can require hundreds of grass pages. Combine planning
    // footprints into 32 m bins; readiness still checks each actual page separately.
    inputs.planning.extend(coalesce_grass_requests(requests));
    inputs.required.extend(required);
    if inputs.planning.len() > MAX_CONTACT_REGIONS {
        inputs.error = Some("too many terrain contact regions".into());
    }
}

fn coalesce_grass_requests(requests: Vec<ContactRegion>) -> Vec<ContactRegion> {
    let mut bins: BTreeMap<(i64, i64), ContactRegion> = BTreeMap::new();
    for r in requests {
        let center = (r.bounds[0] + r.bounds[1]) * 0.5;
        let key = (
            (center.x / 32.).floor() as i64,
            (center.z / 32.).floor() as i64,
        );
        bins.entry(key)
            .and_modify(|b| {
                b.bounds[0] = b.bounds[0].min(r.bounds[0]);
                b.bounds[1] = b.bounds[1].max(r.bounds[1]);
            })
            .or_insert(r);
    }
    bins.into_values().collect()
}

impl TerrainLodStream {
    pub(super) fn certificates(&self, size: f64) -> Vec<ContactCertificate> {
        let mut result = Vec::new();
        for &(key, edges) in self.active.keys() {
            if self
                .transition
                .as_ref()
                .is_none_or(|t| !t.running() || t.keeps(&(key, edges)))
            {
                result.push(lod::contact::static_certificate(
                    key,
                    edges,
                    &self.metadata,
                    size,
                ));
            }
        }
        if let Some(t) = self.transition.as_ref().filter(|t| t.running()) {
            result.extend(t.certificates().cloned());
        }
        result
    }
    pub(crate) fn sample_contact_height(
        &self,
        position: Vec3,
        origin: &WorldOrigin,
        readiness: &TerrainContactReadiness,
    ) -> Option<f32> {
        if !readiness.enabled || !readiness.actor_ready(position, origin) {
            return None;
        }
        let space = readiness.space?;
        if self.identity != readiness.identity {
            return None;
        }
        let point = position.as_dvec3() + origin_shift(origin.cell(), readiness.cell_size);
        let cell = CellCoord {
            x: (point.x / readiness.cell_size).floor() as i32,
            z: (point.z / readiness.cell_size).floor() as i32,
        };
        let field = self
            .nodes
            .get(&TerrainNodeKey::leaf(space, cell))?
            .heightfield
            .as_ref()?;
        Some(
            field
                .sample(
                    [
                        (point.x - cell.x as f64 * readiness.cell_size) as f32,
                        (point.z - cell.z as f64 * readiness.cell_size) as f32,
                    ],
                    readiness.cell_size as f32,
                )
                .height,
        )
    }
}

#[derive(Component)]
struct ContactHidden(Visibility);

fn publish(
    mut commands: Commands,
    config: Res<TerrainLodPreview>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    stream: Res<TerrainLodStream>,
    mut readiness: ResMut<TerrainContactReadiness>,
    mut inputs: ResMut<ContactInputs>,
    scene: Option<Res<VegetationDebugScene>>,
    gate: Option<ResMut<VegetationTerrainGate>>,
    mut actors: Query<
        (
            Entity,
            &GlobalTransform,
            &mut Visibility,
            Option<&ContactHidden>,
        ),
        With<TerrainGrounded>,
    >,
    mut stats: ResMut<TerrainLodStats>,
) {
    readiness.enabled = config.enabled;
    readiness.space = stream.identity.as_ref().map(|i| i.1);
    readiness.identity = stream.identity.clone();
    readiness.cell_size = readiness
        .space
        .and_then(|id| catalog.world_space(id))
        .map_or(1., |s| s.cell_size as f64);
    readiness.domain = stream
        .active
        .keys()
        .map(|(k, _)| stream.metadata[k].bounds(readiness.cell_size))
        .collect();
    readiness.certificates = stream.certificates(readiness.cell_size);
    if inputs.error.is_some() {
        readiness.domain.clear();
    }
    stats.drawn_contact_limited =
        inputs.error.is_some() || inputs.required.iter().any(|r| !readiness.permits(r));
    stats.drawn_maximum_visible_error = inputs.view.as_ref().map_or(0., |view| {
        readiness
            .certificates
            .iter()
            .filter(|c| view.visible(c.bounds))
            .map(|c| view.projected_error(c.bounds, c.error))
            .fold(0., f64::max)
    });
    stats.blocked_actors = 0;
    for (entity, transform, mut visibility, hidden) in &mut actors {
        let ready = !readiness.enabled
            || stream
                .sample_contact_height(transform.translation(), &origin, &readiness)
                .is_some_and(|h| (h - transform.translation().y).abs() <= 0.001);
        if ready {
            if let Some(previous) = hidden {
                *visibility = previous.0;
                commands.entity(entity).remove::<ContactHidden>();
            }
        } else {
            stats.blocked_actors += 1;
            if hidden.is_none() {
                commands.entity(entity).insert(ContactHidden(*visibility));
            }
            *visibility = Visibility::Hidden;
        }
    }
    stats.blocked_grass_pages = 0;
    stats.mismatched_grass_pages = 0;
    if let Some(mut gate) = gate {
        let mut next = VegetationTerrainGate::default();
        if readiness.enabled && inputs.camera.is_some() {
            next.block_all = inputs.error.is_some() || stream.active.is_empty();
            if let Some(scene) = scene {
                let mut samples = 0;
                for page in &mut inputs.pages {
                    if page.source_error.is_none()
                        && samples < SOURCE_SAMPLES_PER_FRAME
                        && let Some(field) = page
                            .key
                            .and_then(|k| stream.nodes.get(&k))
                            .and_then(|n| n.heightfield.as_ref())
                    {
                        let source = &scene.scene().pages[page.source_index];
                        let count =
                            usize::from(source.surface.resolution.max(field.resolution)).pow(2);
                        if samples + count <= SOURCE_SAMPLES_PER_FRAME {
                            page.source_error = Some(source_error(source, field));
                            samples += count;
                            stats.contact_source_checks += 1;
                            stats.contact_source_samples += count as u64;
                        }
                    }
                    let tolerance = config.settings.contact_tolerance
                        - page.source_error.unwrap_or(f32::INFINITY);
                    let region = ContactRegion {
                        bounds: page.bounds,
                        exact: true,
                        tolerance,
                        priority: ContactPriority::Vegetation,
                    };
                    if !readiness.permits(&region) {
                        next.blocked_pages.insert(page.id);
                        stats.blocked_grass_pages += 1;
                    }
                    if page
                        .source_error
                        .is_some_and(|e| e > config.settings.contact_tolerance)
                        || page.key.is_none()
                    {
                        stats.mismatched_grass_pages += 1;
                    }
                }
            }
        }
        if *gate != next {
            *gate = next;
        }
    }
}

// Geometry-only identity: painting coverage and editing species/lighting do not
// change the placement surface. Callers separately require the same canonical key,
// world space and publication generation before considering reuse.
fn reusable_source_error(
    error: Option<f32>,
    before: &vegetation::VegetationFieldPage,
    after: &vegetation::VegetationFieldPage,
) -> Option<f32> {
    error.filter(|_| {
        before.size == after.size
            && before.surface.resolution == after.surface.resolution
            && before.surface.heights == after.surface.heights
    })
}

// Both grids use nested power-of-two cells and the same diagonal. The difference
// is linear on the finer triangles, so its maximum occurs at a finer-grid vertex.
fn source_error(page: &vegetation::VegetationFieldPage, field: &TerrainHeightfield) -> f32 {
    let a = page.surface.resolution - 1;
    let b = field.resolution - 1;
    if !a.is_power_of_two() || !b.is_power_of_two() {
        return f32::INFINITY;
    }
    // These fields have already passed scene/node admission validation. Equal
    // grids need only a linear height comparison; normals and validity are irrelevant.
    if a == b {
        return page
            .surface
            .heights
            .iter()
            .zip(&field.heights)
            .map(|(a, b)| (a - b).abs())
            .fold(0., f32::max);
    }
    let n = a.max(b);
    let mut error = 0_f32;
    for z in 0..=n {
        for x in 0..=n {
            error = error.max(
                (grid_height(&page.surface.heights, a, n, x, z)
                    - grid_height(&field.heights, b, n, x, z))
                .abs(),
            );
        }
    }
    error
}

// Evaluate only height on the nested, shared-diagonal grids. Integer addressing
// avoids normal decoding and repeated whole-field validation in each sample.
fn grid_height(heights: &[f32], intervals: u16, fine: u16, x: u16, z: u16) -> f32 {
    let ratio = fine / intervals;
    let (gx, gz) = (usize::from(x / ratio), usize::from(z / ratio));
    let side = usize::from(intervals) + 1;
    if x.is_multiple_of(ratio) && z.is_multiple_of(ratio) {
        return heights[gz * side + gx];
    }
    let (x1, z1) = ((gx + 1).min(side - 1), (gz + 1).min(side - 1));
    let weights = vegetation::surface_triangle_weights(
        f32::from(x % ratio) / f32::from(ratio),
        f32::from(z % ratio) / f32::from(ratio),
    );
    [
        gz * side + gx,
        gz * side + x1,
        z1 * side + gx,
        z1 * side + x1,
    ]
    .into_iter()
    .zip(weights)
    .map(|(i, w)| heights[i] * w)
    .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "short CPU benchmark of streaming surface certification"]
    fn profile_surface_certification() {
        for (source_resolution, terrain_resolution) in [(33, 33), (65, 33), (257, 33)] {
            let page = vegetation::VegetationFieldPage {
                origin_xz: [0.; 2],
                size: 8.,
                fields: vec![],
                surface: vegetation::VegetationSurfaceField::flat(
                    source_resolution,
                    0.,
                    [0., 1., 0.],
                ),
            };
            let field = TerrainHeightfield::from_heights(
                terrain_resolution,
                &vec![0.; usize::from(terrain_resolution).pow(2)],
                0.,
                0.,
                8.,
            )
            .unwrap();
            let start = std::time::Instant::now();
            assert_eq!(
                source_error(std::hint::black_box(&page), std::hint::black_box(&field)),
                0.
            );
            eprintln!(
                "CONTACT_CHECK source={source_resolution} terrain={terrain_resolution} ms={:.3}",
                start.elapsed().as_secs_f64() * 1000.
            );
        }
    }

    #[test]
    fn source_certificates_survive_streaming_coverage_and_rebase_but_not_height_or_generation() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let space = WorldSpaceId(1);
        let mut scene = vegetation::fixtures::reference_scene();
        scene.pages.truncate(1);
        scene.pages[0].origin_xz = [0.; 2];
        let size = scene.pages[0].size;
        world.insert_resource(TerrainLodPreview {
            enabled: true,
            ..default()
        });
        world.insert_resource(WorldCatalog {
            generation_id: "one".into(),
            world_spaces: vec![WorldSpaceInfo {
                atmosphere: Default::default(),
                id: space,
                name: "test".into(),
                cell_size: size,
                minimum_y: -10.,
                maximum_y: 10.,
            }],
            ..default()
        });
        world.insert_resource(WorldOrigin {
            space: Some(space),
            cell: CellCoord::ZERO,
        });
        world.insert_resource(ActiveWorldSpace {
            current: Some(space),
            ..default()
        });
        world.insert_resource(ContactInputs::default());
        world.insert_resource(VegetationDebugScene::new(scene.clone()).unwrap());
        world.spawn((
            Camera::default(),
            GlobalTransform::default(),
            WorldViewCamera,
        ));
        world.run_system_once(collect).unwrap();
        world.resource_mut::<ContactInputs>().pages[0].source_error = Some(0.003);
        // Page arrival reorders source indices. Coverage changes are not geometry changes.
        let mut other = scene.pages[0].clone();
        other.origin_xz[0] = size;
        scene.pages[0].fields[0].coverage.fill(127);
        scene.pages.insert(0, other);
        world
            .resource_mut::<VegetationDebugScene>()
            .replace(scene.clone())
            .unwrap();
        world.run_system_once(collect).unwrap();
        assert_eq!(
            world.resource::<ContactInputs>().pages[1].source_error,
            Some(0.003)
        );
        assert_eq!(
            world.resource::<ContactInputs>().pages[0].source_error,
            None
        );
        world.resource_mut::<WorldOrigin>().cell.x = 1;
        for page in &mut scene.pages {
            page.origin_xz[0] -= size;
        }
        world
            .resource_mut::<VegetationDebugScene>()
            .replace(scene.clone())
            .unwrap();
        world.run_system_once(collect).unwrap();
        assert_eq!(
            world.resource::<ContactInputs>().pages[1].source_error,
            Some(0.003)
        );
        scene.pages[1].surface.heights[0] += 0.05;
        world
            .resource_mut::<VegetationDebugScene>()
            .replace(scene)
            .unwrap();
        world.run_system_once(collect).unwrap();
        assert_eq!(
            world.resource::<ContactInputs>().pages[1].source_error,
            None
        );
        world.resource_mut::<ContactInputs>().pages[1].source_error = Some(0.05);
        world.resource_mut::<WorldCatalog>().generation_id = "two".into();
        world.run_system_once(collect).unwrap();
        assert!(
            world
                .resource::<ContactInputs>()
                .pages
                .iter()
                .all(|p| p.source_error.is_none())
        );
    }

    #[test]
    fn height_only_grid_evaluation_matches_triangle_sampler() {
        for intervals in [2u16, 4, 8, 32] {
            let resolution = intervals + 1;
            let heights: Vec<_> = (0..usize::from(resolution).pow(2))
                .map(|i| ((i * 17 % 41) as f32 - 20.) * 0.3)
                .collect();
            let field =
                TerrainHeightfield::from_heights(resolution, &heights, -10., 10., 8.).unwrap();
            let fine = intervals * 2;
            for z in 0..=fine {
                for x in 0..=fine {
                    let reference = field
                        .sample(
                            [x as f32 / fine as f32 * 8., z as f32 / fine as f32 * 8.],
                            8.,
                        )
                        .height;
                    assert!(
                        (grid_height(&heights, intervals, fine, x, z) - reference).abs() < 0.00001
                    );
                }
            }
        }
    }

    #[test]
    fn small_cell_grass_requests_merge_without_losing_any_protected_extent() {
        let requests: Vec<_> = (-14..=14)
            .flat_map(|x| {
                (-14..=14).map(move |z| ContactRegion {
                    bounds: [
                        DVec3::new(x as f64 * 8. - 16., -1., z as f64 * 8. - 16.),
                        DVec3::new((x + 1) as f64 * 8. + 16., 2., (z + 1) as f64 * 8. + 16.),
                    ],
                    exact: true,
                    tolerance: 0.,
                    priority: ContactPriority::Vegetation,
                })
            })
            .collect();
        let merged = coalesce_grass_requests(requests.clone());
        assert!(merged.len() < MAX_CONTACT_REGIONS);
        for r in requests {
            assert!(
                merged.iter().any(|m| m.bounds[0].cmple(r.bounds[0]).all()
                    && m.bounds[1].cmpge(r.bounds[1]).all())
            );
        }
    }

    #[test]
    fn grass_contact_compares_the_whole_surface_not_only_coarse_vertices() {
        let mut page = vegetation::VegetationFieldPage {
            origin_xz: [320., -320.],
            size: 32.,
            fields: vec![],
            surface: vegetation::VegetationSurfaceField::flat(5, 0., [0., 1., 0.]),
        };
        let coarse = TerrainHeightfield::from_heights(3, &[0.; 9], -1., 1., 32.).unwrap();
        assert_eq!(source_error(&page, &coarse), 0.);
        page.surface.heights[6] = 0.05; // A fine vertex absent from the coarse grid.
        assert!((source_error(&page, &coarse) - 0.05).abs() < 1e-6);
        page.surface = vegetation::VegetationSurfaceField::flat(3, 0., [0., 1., 0.]);
        let mut heights = [0.; 25];
        heights[6] = -0.07;
        let fine = TerrainHeightfield::from_heights(5, &heights, -1., 1., 32.).unwrap();
        assert!((source_error(&page, &fine) - 0.07).abs() < 1e-6);
    }

    #[test]
    fn contact_readiness_uses_canonical_space_and_rejects_morphing_ground() {
        let bounds = [DVec3::new(320., 0., -320.), DVec3::new(352., 0., -288.)];
        let mut readiness = TerrainContactReadiness {
            enabled: true,
            space: Some(WorldSpaceId(1)),
            cell_size: 32.,
            domain: vec![bounds],
            certificates: vec![ContactCertificate {
                bounds,
                error: 0.,
                exact: true,
            }],
            ..default()
        };
        let mut origin = WorldOrigin {
            space: Some(WorldSpaceId(1)),
            cell: CellCoord { x: 10, z: -10 },
        };
        assert!(readiness.actor_ready(Vec3::new(16., 100., 16.), &origin));
        assert!(!readiness.actor_ready(Vec3::new(48., 0., 16.), &origin));
        origin.cell = CellCoord { x: 0, z: 0 };
        assert!(readiness.actor_ready(Vec3::new(336., 100., -304.), &origin));
        origin.space = Some(WorldSpaceId(2));
        assert!(!readiness.actor_ready(Vec3::new(336., 100., -304.), &origin));
        origin.space = Some(WorldSpaceId(1));
        readiness.certificates[0].error = 0.02;
        readiness.certificates[0].exact = false;
        assert!(!readiness.actor_ready(Vec3::new(336., 100., -304.), &origin));
    }
}
