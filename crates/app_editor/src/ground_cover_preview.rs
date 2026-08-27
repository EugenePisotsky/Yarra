//! Disposable editor presentation for accepted, source-derived ground-cover pages.

use std::collections::{BTreeMap, HashMap, HashSet};

use bevy::prelude::*;
use engine::{WorldCatalog, WorldOrigin, WorldViewpoint};
use ground_cover::{GroundCoverPage3d, GroundCoverPageAsset};
use world::{CellCoord, GroundCoverPage, GroundCoverSpecies, PageDomain, PageKey};

use crate::{
    catalog_editing::GroundCoverRegionWorkingSet,
    derived_jobs::{DerivedArtifact, DerivedArtifactStore, DerivedJobScope},
    domain_editing::DenseDomainWorkingSets,
    ground_cover_catalog::GroundCoverCatalogWorkingSet,
    tools::DerivedProduct,
};

// The runtime index is currently a 7-by-7 cell window. Matching it keeps editor override GPU
// residency bounded even when saved source changes remain pinned across a large project.
const OVERRIDE_PRESENTATION_RADIUS_CELLS: u32 = 3;
const MAX_PRESENTED_GROUND_COVER_OVERRIDES: usize = 49;

pub(crate) struct GroundCoverPreviewPlugin;

impl Plugin for GroundCoverPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, sync_derived_ground_cover_pages);
    }
}

#[derive(Component, Debug, Clone)]
struct EditorDerivedGroundCoverPage {
    key: PageKey,
    input_revision: u64,
    fingerprint: u64,
    origin_cell: CellCoord,
    asset: Option<Handle<GroundCoverPageAsset>>,
}

#[derive(Component, Debug, Clone)]
struct EditorHiddenCookedGroundCoverPage {
    key: PageKey,
    page: GroundCoverPage3d,
}

struct DesiredGroundCoverPage<'a> {
    input_revision: u64,
    fingerprint: u64,
    page: &'a GroundCoverPage,
    species: &'a [GroundCoverSpecies],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverrideUpdate {
    Keep,
    Rebase,
    Replace,
}

#[allow(clippy::too_many_arguments)]
fn sync_derived_ground_cover_pages(
    mut commands: Commands,
    artifacts: Res<DerivedArtifactStore>,
    dense: Res<DenseDomainWorkingSets>,
    regions: Res<GroundCoverRegionWorkingSet>,
    ground_cover_catalog: Res<GroundCoverCatalogWorkingSet>,
    viewpoint: Res<WorldViewpoint>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    mut assets: ResMut<Assets<GroundCoverPageAsset>>,
    derived_pages: Query<(Entity, &EditorDerivedGroundCoverPage)>,
    cooked_pages: Query<(Entity, &GroundCoverPage3d), Without<EditorDerivedGroundCoverPage>>,
    hidden_cooked_pages: Query<(Entity, &EditorHiddenCookedGroundCoverPage)>,
) {
    let desired = desired_ground_cover_pages(
        &artifacts,
        &dense,
        &regions,
        &ground_cover_catalog,
        &viewpoint,
        &origin,
    );
    let desired_keys = desired.keys().copied().collect::<HashSet<_>>();
    let mut existing = derived_pages
        .iter()
        .map(|(entity, page)| (page.key, (entity, page.clone())))
        .collect::<HashMap<_, _>>();

    for (&key, desired_page) in &desired {
        let Some(space) = catalog.world_space(key.space) else {
            continue;
        };
        if let Some((entity, current)) = existing.remove(&key) {
            match override_update(
                &current,
                desired_page.input_revision,
                desired_page.fingerprint,
                origin.cell(),
            ) {
                OverrideUpdate::Keep => {}
                OverrideUpdate::Rebase => {
                    if let Some(handle) = current.asset.as_ref()
                        && let Some(mut asset) = assets.get_mut(handle.id())
                    {
                        asset.origin_cell = origin.cell();
                        asset.cell_size = space.cell_size;
                    }
                    commands
                        .entity(entity)
                        .insert(EditorDerivedGroundCoverPage {
                            origin_cell: origin.cell(),
                            ..current
                        });
                }
                OverrideUpdate::Replace => {
                    remove_owned_asset(&mut assets, current.asset.as_ref());
                    let asset = create_override_asset(
                        &mut assets,
                        key,
                        origin.cell(),
                        space.cell_size,
                        desired_page,
                    );
                    let mut entity_commands = commands.entity(entity);
                    entity_commands.insert(EditorDerivedGroundCoverPage {
                        key,
                        input_revision: desired_page.input_revision,
                        fingerprint: desired_page.fingerprint,
                        origin_cell: origin.cell(),
                        asset: asset.clone(),
                    });
                    if let Some(asset) = asset {
                        entity_commands.insert(GroundCoverPage3d(asset));
                    } else {
                        entity_commands.remove::<GroundCoverPage3d>();
                    }
                }
            }
        } else {
            let asset = create_override_asset(
                &mut assets,
                key,
                origin.cell(),
                space.cell_size,
                desired_page,
            );
            let mut entity = commands.spawn((
                EditorDerivedGroundCoverPage {
                    key,
                    input_revision: desired_page.input_revision,
                    fingerprint: desired_page.fingerprint,
                    origin_cell: origin.cell(),
                    asset: asset.clone(),
                },
                Name::new(format!(
                    "Editor ground-cover override {}, {}",
                    key.cell.x, key.cell.z
                )),
            ));
            if let Some(asset) = asset {
                entity.insert(GroundCoverPage3d(asset));
            }
        }
    }

    // Entries outside the bounded presentation window remain as CPU artifacts and pinned source
    // state. Their GPU assets are recreated only when the camera returns.
    for (_, (entity, page)) in existing {
        remove_owned_asset(&mut assets, page.asset.as_ref());
        commands.entity(entity).despawn();
    }

    // Cooked assets remain owned by the immutable runtime streamer. Removing only their active
    // presentation component keeps them recoverable without mutating or duplicating runtime data.
    for (entity, page) in &cooked_pages {
        let Some(asset) = assets.get(page.id()) else {
            continue;
        };
        if desired_keys.contains(&asset.key) {
            commands
                .entity(entity)
                .remove::<GroundCoverPage3d>()
                .insert(EditorHiddenCookedGroundCoverPage {
                    key: asset.key,
                    page: page.clone(),
                });
        }
    }
    for (entity, hidden) in &hidden_cooked_pages {
        if !desired_keys.contains(&hidden.key) {
            commands
                .entity(entity)
                .insert(hidden.page.clone())
                .remove::<EditorHiddenCookedGroundCoverPage>();
        }
    }
}

fn desired_ground_cover_pages<'a>(
    artifacts: &'a DerivedArtifactStore,
    dense: &DenseDomainWorkingSets,
    regions: &GroundCoverRegionWorkingSet,
    catalog: &GroundCoverCatalogWorkingSet,
    viewpoint: &WorldViewpoint,
    origin: &WorldOrigin,
) -> BTreeMap<PageKey, DesiredGroundCoverPage<'a>> {
    let Some(position) = viewpoint.position() else {
        return BTreeMap::new();
    };
    if origin.space() != Some(position.space) {
        return BTreeMap::new();
    }
    artifacts
        .iter()
        .filter_map(|(job, input_revision, artifact)| {
            let DerivedJobScope::Cell { space, cell } = job.scope else {
                return None;
            };
            if job.product != DerivedProduct::GroundCoverPage
                || space != position.space
                || cell.chebyshev_distance(position.cell) > OVERRIDE_PRESENTATION_RADIUS_CELLS
                || !ground_cover_override_required(
                    dense.ground_cover_cell_diverges(space, cell),
                    regions.runtime_diverged(),
                    catalog.runtime_diverged(),
                )
            {
                return None;
            }
            let DerivedArtifact::GroundCoverPage {
                page,
                species,
                fingerprint,
                ..
            } = artifact
            else {
                return None;
            };
            Some((
                PageKey {
                    space,
                    cell,
                    domain: PageDomain::GroundCover,
                    lod: 0,
                },
                DesiredGroundCoverPage {
                    input_revision,
                    fingerprint: *fingerprint,
                    page,
                    species,
                },
            ))
        })
        .take(MAX_PRESENTED_GROUND_COVER_OVERRIDES)
        .collect()
}

fn ground_cover_override_required(
    mask_diverged: bool,
    regions_diverged: bool,
    catalog_diverged: bool,
) -> bool {
    mask_diverged || regions_diverged || catalog_diverged
}

fn override_update(
    current: &EditorDerivedGroundCoverPage,
    input_revision: u64,
    fingerprint: u64,
    origin_cell: CellCoord,
) -> OverrideUpdate {
    if current.input_revision != input_revision || current.fingerprint != fingerprint {
        OverrideUpdate::Replace
    } else if current.origin_cell != origin_cell {
        OverrideUpdate::Rebase
    } else {
        OverrideUpdate::Keep
    }
}

fn create_override_asset(
    assets: &mut Assets<GroundCoverPageAsset>,
    key: PageKey,
    origin_cell: CellCoord,
    cell_size: f32,
    desired: &DesiredGroundCoverPage<'_>,
) -> Option<Handle<GroundCoverPageAsset>> {
    (!desired.page.clusters.is_empty()).then(|| {
        assets.add(GroundCoverPageAsset {
            key,
            origin_cell,
            cell_size,
            page: desired.page.clone(),
            species: desired.species.to_vec(),
        })
    })
}

fn remove_owned_asset(
    assets: &mut Assets<GroundCoverPageAsset>,
    asset: Option<&Handle<GroundCoverPageAsset>>,
) {
    if let Some(asset) = asset {
        assets.remove(asset.id());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::WorldSpaceId;

    fn marker(
        revision: u64,
        fingerprint: u64,
        origin_cell: CellCoord,
    ) -> EditorDerivedGroundCoverPage {
        EditorDerivedGroundCoverPage {
            key: PageKey {
                space: WorldSpaceId(1),
                cell: CellCoord::ZERO,
                domain: PageDomain::GroundCover,
                lod: 0,
            },
            input_revision: revision,
            fingerprint,
            origin_cell,
            asset: None,
        }
    }

    #[test]
    fn accepted_page_replacement_and_origin_rebase_are_distinct_updates() {
        let current = marker(4, 91, CellCoord::ZERO);
        assert_eq!(
            override_update(&current, 4, 91, CellCoord::ZERO),
            OverrideUpdate::Keep
        );
        assert_eq!(
            override_update(&current, 4, 91, CellCoord { x: 8, z: -2 }),
            OverrideUpdate::Rebase
        );
        assert_eq!(
            override_update(&current, 5, 92, CellCoord::ZERO),
            OverrideUpdate::Replace
        );
    }

    #[test]
    fn visual_region_and_mask_edits_each_enable_source_preview() {
        assert!(ground_cover_override_required(true, false, false));
        assert!(ground_cover_override_required(false, true, false));
        assert!(ground_cover_override_required(false, false, true));
        assert!(!ground_cover_override_required(false, false, false));
    }

    #[test]
    fn empty_compiled_pages_need_no_gpu_asset_but_still_have_override_identity() {
        let mut assets = Assets::<GroundCoverPageAsset>::default();
        let page = GroundCoverPage {
            clusters: Vec::new(),
        };
        let desired = DesiredGroundCoverPage {
            input_revision: 1,
            fingerprint: 7,
            page: &page,
            species: &[],
        };
        assert!(
            create_override_asset(
                &mut assets,
                marker(1, 7, CellCoord::ZERO).key,
                CellCoord::ZERO,
                32.0,
                &desired,
            )
            .is_none()
        );
        assert!(assets.is_empty());
    }
}
