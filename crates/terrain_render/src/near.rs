//! Bounded close-up inputs, addressed by canonical cell independently of mesh LOD.
use crate::*;
use bevy::math::DVec3;
use std::collections::{BTreeMap, BTreeSet};
use world::{PageKey, WorldSpaceId};
pub(crate) mod gpu;
use gpu::{NearAtlas, NearEntry, NearUploadHub, Pack};

pub const NEAR_SLOTS: usize = 64;
pub const NEAR_TABLE: usize = 256;
pub const WEIGHT_SIDE: u32 = 257;
pub const CANOPY_SIDE: u32 = 130;
pub const NEAR_END: f32 = 40.;
pub const NEAR_START: f32 = 24.;

/// Published source data only. No mesh or render assets until admitted by the near cache.
#[derive(Component, Clone, Debug)]
pub struct NearSource {
    pub key: PageKey,
    pub cell_size: f32,
    pub height_bounds: [f32; 2],
    pub surfaces: Vec<TerrainSurfaceId>,
    pub weights: Vec<TerrainWeightPage>,
    pub profile: TerrainProfile,
    pub texture_set: TerrainTextureSet,
    pub layers: Vec<TerrainSurfaceLayer>,
}
#[derive(Resource, Default)]
pub struct NearView {
    pub identity: Option<(String, WorldSpaceId)>,
    pub origin: CellCoord,
    pub eye: DVec3,
}
#[derive(Resource, Default, Debug)]
pub struct NearStats {
    pub pages: usize,
    pub capacity_limited: bool,
    pub control_bytes: u64,
    pub tile_uploads: u64,
    pub ready_pages: usize,
    pub error: Option<String>,
}
struct Resident {
    cell: CellCoord,
    slot: u32,
    material: Handle<TerrainMaterial>,
    fade: f32,
    uploaded: bool,
    canopy: Option<AssetId<Image>>,
}
#[derive(Resource, Default)]
struct Cache {
    identity: Option<(String, WorldSpaceId)>,
    pages: BTreeMap<Entity, Resident>,
    atlas: Option<NearAtlas>,
    table: Vec<NearEntry>,
    pack: Option<Pack>,
    failed: BTreeSet<Entity>,
    source_weight: Option<Handle<Image>>,
}
#[derive(SystemSet, Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct NearPrepare;
#[derive(SystemSet, Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct NearUpdate;

pub(crate) fn install(app: &mut App) {
    gpu::install(app);
    app.init_resource::<NearView>()
        .init_resource::<NearStats>()
        .init_resource::<Cache>()
        .add_systems(
            PostUpdate,
            select_sources
                .in_set(NearPrepare)
                .before(TerrainMaterialPreparation),
        )
        .add_systems(
            PostUpdate,
            update
                .in_set(NearUpdate)
                .after(TerrainMaterialPreparation)
                .after(NearPrepare),
        );
}
fn distance_squared(source: &NearSource, eye: DVec3) -> f64 {
    let p = source.key.cell.origin(source.cell_size);
    let lo = DVec3::new(p[0], source.height_bounds[0] as f64, p[1]);
    let hi = DVec3::new(
        p[0] + source.cell_size as f64,
        source.height_bounds[1] as f64,
        p[1] + source.cell_size as f64,
    );
    eye.distance_squared(eye.clamp(lo, hi))
}
fn release(
    commands: &mut Commands,
    entity: Entity,
    p: Resident,
    materials: &mut Assets<TerrainMaterial>,
) {
    if let Ok(mut e) = commands.get_entity(entity) {
        e.remove::<MeshMaterial3d<TerrainMaterial>>();
    }
    // Weight handles may share the one-texel carrier; asset lifetimes release it.
    materials.remove(p.material.id());
}
#[allow(clippy::too_many_arguments)]
fn select_sources(
    mut commands: Commands,
    view: Res<NearView>,
    time: Res<Time>,
    sources: Query<(Entity, &NearSource)>,
    server: Res<AssetServer>,
    variation: Res<TerrainMacroVariation>,
    mut cache: ResMut<Cache>,
    mut stats: ResMut<NearStats>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    if cache.identity != view.identity {
        for (entity, p) in std::mem::take(&mut cache.pages) {
            release(&mut commands, entity, p, &mut materials);
        }
        *cache = Cache {
            identity: view.identity.clone(),
            ..default()
        };
        *stats = default();
    }
    let Some((_, space)) = &view.identity else {
        return;
    };
    if cache.atlas.as_ref().is_some_and(|a| a.ready() && !a.idle()) {
        return;
    }
    let mut wanted: Vec<_> = sources
        .iter()
        .filter(|(e, s)| s.key.space == *space && !cache.failed.contains(e))
        .map(|(e, s)| (e, s.key.cell, distance_squared(s, view.eye)))
        .filter(|(_, _, d)| *d < f64::from(NEAR_END + 8.).powi(2))
        .collect();
    // Stable ordering and a small residency preference prevent equal-distance churn.
    wanted.sort_by(|a, b| {
        let score = |v: &(Entity, CellCoord, f64)| {
            v.2 - if cache.pages.contains_key(&v.0) {
                16.
            } else {
                0.
            }
        };
        score(a).total_cmp(&score(b)).then(a.1.cmp(&b.1))
    });
    stats.capacity_limited = wanted.len() > NEAR_SLOTS;
    wanted.truncate(NEAR_SLOTS);
    let order: Vec<_> = wanted.into_iter().map(|v| v.0).collect();
    let wanted: BTreeSet<_> = order.iter().copied().collect();
    let step = time.delta_secs().clamp(0., 1. / 30.) / 0.3;
    let mut removed = vec![];
    for (&e, p) in &mut cache.pages {
        if sources.get(e).is_err() {
            removed.push(e);
            continue;
        }
        p.fade = if wanted.contains(&e) && p.uploaded {
            (p.fade + step).min(1.)
        } else {
            (p.fade - step).max(0.)
        };
        if p.fade == 0. && !wanted.contains(&e) {
            removed.push(e);
        }
    }
    for e in removed {
        let p = cache.pages.remove(&e).unwrap();
        release(&mut commands, e, p, &mut materials);
    }
    let mut added = 0;
    for e in order {
        if cache.pages.contains_key(&e) || added >= 2 {
            continue;
        }
        let Some(slot) =
            (0..NEAR_SLOTS as u32).find(|i| cache.pages.values().all(|p| p.slot != *i))
        else {
            break;
        };
        let (_, s) = sources.get(e).unwrap();
        if !(1..=2).contains(&s.surfaces.len())
            || (s.surfaces.len() == 2
                && !s.weights.first().is_some_and(|w| {
                    (2..=WEIGHT_SIDE as u16).contains(&w.resolution)
                        && w.rgba.len() == usize::from(w.resolution).pow(2) * 4
                }))
        {
            cache.failed.insert(e);
            stats.error = Some(
                "near terrain requires one or two surfaces and weights up to 257 samples".into(),
            );
            continue;
        }
        // Existing source stream owns decoded-byte admission. Shared texture packs
        // still have their own explicit development budget.
        if s.texture_set.runtime_gpu_bytes() > 256 * 1024 * 1024 {
            cache.failed.insert(e);
            stats.error = Some("near terrain texture set exceeds 256 MiB".into());
            continue;
        }
        match prepare_terrain_material(PrepareTerrainMaterialContext {
            asset_server: &server,
            images: &mut images,
            materials: &mut materials,
            cell: s.key.cell,
            origin_cell: view.origin,
            cell_size: s.cell_size,
            page_surfaces: &s.surfaces,
            weight_pages: &s.weights,
            profile: &s.profile,
            texture_set: &s.texture_set,
            surfaces: &s.layers,
            macro_variation: *variation,
        }) {
            Ok(mut prepared) => {
                // Carriers feed the shared albedo/canopy integrations but are never
                // drawn. Preserve authored weights in NearSource only; uploading a
                // second full per-page weight image would duplicate the array cache.
                images.remove(prepared.weight_image.id());
                let weight = cache
                    .source_weight
                    .get_or_insert_with(|| {
                        images.add(make_weight_image(&s.surfaces[..1], &[]).unwrap())
                    })
                    .clone();
                prepared.weight_image = weight.clone();
                let mut m = materials.get_mut(&prepared.material).unwrap();
                m.source_only = true;
                m.source_weights = weight.clone();
                m.weights = weight;
                commands
                    .entity(e)
                    .insert(MeshMaterial3d(prepared.material.clone()));
                cache.pages.insert(
                    e,
                    Resident {
                        cell: s.key.cell,
                        slot,
                        material: prepared.material,
                        fade: 0.,
                        uploaded: false,
                        canopy: None,
                    },
                );
                added += 1;
            }
            Err(e2) => {
                cache.failed.insert(e);
                stats.error = Some(e2);
            }
        }
    }
    cache.failed.retain(|e| sources.contains(*e));
    stats.pages = cache.pages.len();
}

#[allow(clippy::too_many_arguments)]
fn update(
    view: Res<NearView>,
    sources: Query<&NearSource>,
    source_materials: Res<Assets<TerrainMaterial>>,
    mut materials: ResMut<Assets<TerrainCompositeMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    hub: Res<NearUploadHub>,
    mut cache: ResMut<Cache>,
    mut stats: ResMut<NearStats>,
    mut events: MessageReader<AssetEvent<Image>>,
    server: Res<AssetServer>,
) {
    let changed: BTreeSet<_> = events
        .read()
        .filter_map(|e| {
            if let AssetEvent::Modified { id } = e {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    for p in cache.pages.values_mut() {
        if p.canopy.is_some_and(|id| changed.contains(&id)) {
            p.canopy = None;
        }
    }
    let Some((_, space)) = &view.identity else {
        return;
    };
    if cache.atlas.as_ref().is_some_and(|a| a.ready() && !a.idle()) {
        return;
    }
    if cache.pages.is_empty() {
        if let Some(atlas) = cache.atlas.take() {
            let ids: Vec<_> = materials
                .iter()
                .filter(|(_, m)| m.has_near(&atlas))
                .map(|(id, _)| id)
                .collect();
            for id in ids {
                materials.get_mut(id).unwrap().clear_near();
            }
        }
        cache.pack = None;
        cache.source_weight = None;
        cache.table.clear();
        stats.control_bytes = 0;
        stats.ready_pages = 0;
        return;
    }
    let pack = cache
        .pages
        .values()
        .filter_map(|p| source_materials.get(&p.material))
        .max_by_key(|m| m.prepared_albedo)
        .map(Pack::from_material);
    let Some(pack) = pack.or_else(|| cache.pack.clone()) else {
        return;
    };
    if cache.pack.as_ref() != Some(&pack) {
        cache.atlas = Some(NearAtlas::new(
            &mut images,
            &mut buffers,
            &hub,
            pack.clone(),
        ));
        cache.pack = Some(pack.clone());
        cache.table.clear();
        stats.ready_pages = 0;
        for p in cache.pages.values_mut() {
            p.uploaded = false;
            p.fade = 0.;
            p.canopy = None;
        }
    }
    let atlas = cache.atlas.as_ref().unwrap();
    stats.control_bytes = gpu::bytes();
    stats.tile_uploads = atlas.uploads();
    if !atlas.ready() {
        if let Some(error) = [&pack.base, &pack.normal, &pack.macro_image]
            .into_iter()
            .chain(pack.prepared.as_ref())
            .find_map(|h| match server.load_state(h.id()) {
                bevy::asset::LoadState::Failed(e) => Some(e.to_string()),
                _ => None,
            })
        {
            stats.error = Some(error);
        }
        return;
    }
    if cache.failed.is_empty() {
        stats.error = None;
    }
    let mut table = vec![NearEntry::default(); NEAR_TABLE];
    let mut uploads = vec![];
    for (&entity, p) in &mut cache.pages {
        let (Ok(source), Some(m)) = (sources.get(entity), source_materials.get(&p.material)) else {
            continue;
        };
        if !pack.matches(m) {
            continue;
        }
        let canopy = m
            .canopy_coverage
            .as_ref()
            .and_then(|h| images.get(h).map(|i| (h.id(), i)));
        let canopy_id = canopy.map(|v| v.0);
        if (!p.uploaded || p.canopy != canopy_id) && uploads.len() < 2 {
            uploads.push(gpu::tile(
                p.slot,
                source,
                m,
                canopy.map(|v| v.1),
                view.origin,
            ));
            p.uploaded = true;
            p.canopy = canopy_id;
        }
        if !p.uploaded {
            continue;
        }
        let mut entry = gpu::entry(source, m, &pack, p.slot, p.fade);
        // Canopy is disabled until its corresponding pixels join this transaction.
        if p.canopy.is_none() {
            entry.canopy.appearance.x = 0.;
        }
        let mut i = hash(p.cell);
        while table[i].key.w != 0 {
            i = (i + 1) & (NEAR_TABLE - 1);
        }
        table[i] = entry;
    }
    if table != cache.table || !uploads.is_empty() {
        cache.atlas.as_ref().unwrap().submit(table.clone(), uploads);
        cache.table = table;
    }
    stats.ready_pages = cache
        .pages
        .values()
        .filter(|p| p.uploaded && p.fade > 0.)
        .count();
    let atlas = cache.atlas.as_ref().unwrap();
    let ids: Vec<_> = materials
        .iter()
        .filter(|(_, m)| m.key.is_some_and(|k| k.0.space == *space) && !m.has_near(atlas))
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        materials.get_mut(id).unwrap().set_near(atlas);
    }
}
fn hash(cell: CellCoord) -> usize {
    crate::composite::atlas::hash(world::TerrainNodeKey::leaf(WorldSpaceId(0), cell))
        & (NEAR_TABLE - 1)
}

#[cfg(test)]
mod tests;
