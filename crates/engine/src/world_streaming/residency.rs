//! Source-page lifetime: bounded fetch/decode, admission, cooling and ownership cleanup.
//! The world coordinator supplies demand and invalidates pages on world/generation changes.
//! All systems remain in the coordinator's ordered WorldStreamingSystems chain.
pub(super) mod attachment;
#[cfg(test)]
pub(super) mod tests;

use super::database::{DatabaseRequest, FetchedPage, WorldDatabaseWorker};
use super::{
    ActiveWorldSpace, StreamPhase, WorldGenerationReload, WorldOrigin, WorldStream, source_demand,
};
use crate::object_lod::ScreenSpaceLod;
use attachment::{PageAttachment, WorldRenderAssets, attach_page, despawn_attachment};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use crossbeam_channel::TrySendError;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    time::Duration,
};
use terrain_render::{TerrainMacroVariation, TerrainMaterial};
use world::{ObjectDefinitionId, PageDomain, PageKey, TerrainTextureSetId};
use world_db::{DecodedPage, PageDependency, RuntimeObjectDefinition, TerrainRenderResources};

const COOLING_SECONDS: f32 = 2.0;
pub(super) const MAX_PENDING_SOURCE_PAGES: usize = 16;
const MAX_ATTACHMENTS_PER_FRAME: usize = 2;
const MAX_RESIDENT_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESIDENT_GPU_BYTES_ESTIMATE: u64 = 256 * 1024 * 1024;

#[derive(Resource, Default)]
pub(super) struct SourceResidency {
    pub(super) admission_blocked: usize,
    pub(super) desired: BTreeSet<PageKey>,
    pub(super) priorities: BTreeMap<PageKey, source_demand::Priority>,
    pub(super) pages: HashMap<PageKey, PageState>,
    pub(super) definition_cache: HashMap<ObjectDefinitionId, RuntimeObjectDefinition>,
    decode_tasks: Vec<DecodeTask>,
    next_request_id: u64,
}

pub(super) enum PageState {
    Loading {
        request_id: u64,
    },
    Decoding {
        request_id: u64,
    },
    Prepared(PreparedPage),
    Resident(PageAttachment),
    Cooling {
        attachment: PageAttachment,
        remove_at: Duration,
    },
    Failed(String),
}

pub(super) struct PreparedPage {
    pub(super) decoded: DecodedPage,
    pub(super) dependencies: Vec<PageDependency>,
    pub(super) definitions: Vec<RuntimeObjectDefinition>,
    pub(super) terrain: Option<TerrainRenderResources>,
    pub(super) height_only: bool,
}

struct DecodeTask {
    request_id: u64,
    key: PageKey,
    task: Task<Result<PreparedPage, String>>,
}

impl SourceResidency {
    pub(super) fn set_demand(&mut self, priorities: BTreeMap<PageKey, source_demand::Priority>) {
        self.desired = priorities.keys().copied().collect();
        self.priorities = priorities;
    }

    pub(super) fn request_missing(
        &mut self,
        worker: &WorldDatabaseWorker,
        generation: &str,
        height_only: bool,
    ) -> Result<(), String> {
        // Bound all fetched, decoding and waiting-to-attach work, not just the worker's
        // channel. Otherwise a full resident budget accumulates an unbounded backlog.
        let in_flight = self
            .pages
            .values()
            .filter(|p| {
                matches!(
                    p,
                    PageState::Loading { .. } | PageState::Decoding { .. } | PageState::Prepared(_)
                )
            })
            .count();
        let mut missing: Vec<_> = self
            .priorities
            .iter()
            .filter(|(key, _)| !self.pages.contains_key(key))
            .map(|(&k, &p)| (k, p))
            .collect();
        missing.sort_by(source_demand::compare);
        for (key, _) in missing
            .into_iter()
            .take(MAX_PENDING_SOURCE_PAGES.saturating_sub(in_flight))
        {
            let request_id = self.next_request_id.wrapping_add(1).max(1);
            match worker.try_send(DatabaseRequest::ReadPage {
                generation: generation.to_owned(),
                request_id,
                key,
                height_only: height_only && key.domain == PageDomain::TerrainRender,
            }) {
                Ok(()) => {
                    self.next_request_id = request_id;
                    self.pages.insert(key, PageState::Loading { request_id });
                }
                Err(TrySendError::Full(_)) => break,
                Err(TrySendError::Disconnected(_)) => {
                    return Err("database request channel closed".into());
                }
            }
        }
        Ok(())
    }

    pub(super) fn receive_page(
        &mut self,
        request_id: u64,
        key: PageKey,
        result: Result<Option<FetchedPage>, String>,
    ) {
        let request_is_current = matches!(
            self.pages.get(&key),
            Some(PageState::Loading { request_id: current }) if *current == request_id
        );
        if !request_is_current {
            return;
        }
        if !self.desired.contains(&key) {
            self.pages.remove(&key);
            return;
        }
        match result {
            Ok(Some(fetched)) => {
                let task = AsyncComputeTaskPool::get().spawn(async move {
                    fetched
                        .encoded
                        .decode()
                        .map(|mut decoded| {
                            if fetched.height_only {
                                decoded.gpu_bytes_estimate = 0;
                            }
                            PreparedPage {
                                decoded,
                                dependencies: fetched.dependencies,
                                definitions: fetched.definitions,
                                terrain: fetched.terrain,
                                height_only: fetched.height_only,
                            }
                        })
                        .map_err(|error| error.to_string())
                });
                self.pages.insert(key, PageState::Decoding { request_id });
                self.decode_tasks.push(DecodeTask {
                    request_id,
                    key,
                    task,
                });
            }
            Ok(None) => {
                self.pages
                    .insert(key, PageState::Failed("page is missing".into()));
            }
            Err(error) => {
                self.pages.insert(key, PageState::Failed(error));
            }
        }
    }

    pub(super) fn clear(
        &mut self,
        commands: &mut Commands,
        terrain_meshes: &mut Assets<Mesh>,
        terrain_materials: &mut Assets<TerrainMaterial>,
        terrain_images: &mut Assets<Image>,
    ) {
        for (_, state) in self.pages.drain() {
            match state {
                PageState::Resident(attachment) | PageState::Cooling { attachment, .. } => {
                    despawn_attachment(
                        commands,
                        terrain_meshes,
                        terrain_materials,
                        terrain_images,
                        attachment,
                    );
                }
                _ => {}
            }
        }
        self.decode_tasks.clear();
        self.desired.clear();
        self.priorities.clear();
    }
}

pub(super) fn receive_decode_results(mut stream: ResMut<SourceResidency>) {
    let mut completed = Vec::new();
    for (index, decode) in stream.decode_tasks.iter_mut().enumerate() {
        if let Some(result) = check_ready(&mut decode.task) {
            completed.push((index, decode.request_id, decode.key, result));
        }
    }
    for (index, request_id, key, result) in completed.into_iter().rev() {
        stream.decode_tasks.swap_remove(index);
        let request_is_current = matches!(
            stream.pages.get(&key),
            Some(PageState::Decoding { request_id: current }) if *current == request_id
        );
        if !request_is_current {
            continue;
        }
        if !stream.desired.contains(&key) {
            stream.pages.remove(&key);
            continue;
        }
        match result {
            Ok(prepared) => {
                for definition in &prepared.definitions {
                    stream
                        .definition_cache
                        .insert(definition.id, definition.clone());
                }
                stream.pages.insert(key, PageState::Prepared(prepared));
            }
            Err(error) => {
                stream.pages.insert(key, PageState::Failed(error));
            }
        }
    }
}

pub(super) fn attach_prepared_pages(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    render_assets: Option<Res<WorldRenderAssets>>,
    stats: Res<StreamingStats>,
    mut terrain_meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    macro_variation: Res<TerrainMacroVariation>,
    origin: Res<WorldOrigin>,
    world: Res<WorldStream>,
    mut stream: ResMut<SourceResidency>,
) {
    stream.admission_blocked = 0;
    let Some(render_assets) = render_assets else {
        return;
    };
    let vegetation_catalog = world
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.vegetation_catalog.clone());
    let mut keys: Vec<_> = stream
        .pages
        .iter()
        .filter_map(|(key, state)| matches!(state, PageState::Prepared(_)).then_some(*key))
        .collect();
    keys.sort_by(|a, b| {
        source_demand::compare(
            &(*a, stream.priorities.get(a).copied().unwrap_or((3, 0.))),
            &(*b, stream.priorities.get(b).copied().unwrap_or((3, 0.))),
        )
    });
    // Admission checks are cheap and the prepared queue is bounded. A page that
    // cannot fit must not consume an attachment slot and starve smaller pages.
    let mut attachment_attempts = 0;
    let mut admitted_decoded_bytes = stats.decoded_bytes;
    let mut admitted_gpu_bytes = stats.gpu_bytes_estimate;
    let mut admitted_terrain_texture_sets = stats.terrain_texture_sets.clone();

    for key in keys {
        if attachment_attempts == MAX_ATTACHMENTS_PER_FRAME {
            break;
        }
        let Some(cell_size) = world
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.world_space(key.space))
            .map(|space| space.cell_size)
        else {
            stream.pages.insert(
                key,
                PageState::Failed("page references an unknown world space".into()),
            );
            continue;
        };
        let Some(PageState::Prepared(prepared)) = stream.pages.remove(&key) else {
            continue;
        };
        if !stream.desired.contains(&key) {
            continue;
        }
        let page_decoded_bytes = prepared.decoded.decoded_bytes;
        let page_gpu_bytes = prepared.decoded.gpu_bytes_estimate
            + prepared
                .dependencies
                .iter()
                .map(|dependency| dependency.gpu_bytes_estimate)
                .sum::<u64>()
            + prepared
                .terrain
                .as_ref()
                .filter(|_| !prepared.height_only)
                .map_or(0, |terrain| {
                    if admitted_terrain_texture_sets.contains(&terrain.texture_set.id) {
                        0
                    } else {
                        terrain.texture_set.runtime_gpu_bytes()
                    }
                });
        if page_decoded_bytes > MAX_RESIDENT_DECODED_BYTES
            || page_gpu_bytes > MAX_RESIDENT_GPU_BYTES_ESTIMATE
        {
            stream.pages.insert(
                key,
                PageState::Failed(format!(
                    "page exceeds the development residency profile: {} decoded bytes, {} estimated GPU bytes",
                    page_decoded_bytes, page_gpu_bytes
                )),
            );
            continue;
        }
        if admitted_decoded_bytes.saturating_add(page_decoded_bytes) > MAX_RESIDENT_DECODED_BYTES
            || admitted_gpu_bytes.saturating_add(page_gpu_bytes) > MAX_RESIDENT_GPU_BYTES_ESTIMATE
        {
            stream.admission_blocked += 1;
            stream.pages.insert(key, PageState::Prepared(prepared));
            continue;
        }
        attachment_attempts += 1;
        match attach_page(
            &mut commands,
            &asset_server,
            &render_assets,
            vegetation_catalog.as_ref(),
            &mut terrain_meshes,
            &mut terrain_materials,
            &mut terrain_images,
            *macro_variation,
            origin.cell,
            cell_size,
            prepared,
        ) {
            Ok(attachment) => {
                admitted_decoded_bytes =
                    admitted_decoded_bytes.saturating_add(attachment.decoded_bytes);
                admitted_gpu_bytes =
                    admitted_gpu_bytes.saturating_add(attachment.gpu_bytes_estimate);
                if let Some((texture_set, gpu_bytes)) = attachment.terrain_texture_set
                    && admitted_terrain_texture_sets.insert(texture_set)
                {
                    admitted_gpu_bytes = admitted_gpu_bytes.saturating_add(gpu_bytes);
                }
                stream.pages.insert(key, PageState::Resident(attachment));
            }
            Err(error) => {
                stream.pages.insert(key, PageState::Failed(error));
            }
        }
    }
}

pub(super) fn cool_and_remove_pages(
    mut commands: Commands,
    time: Res<Time>,
    mut terrain_meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    mut stream: ResMut<SourceResidency>,
) {
    let now = time.elapsed();
    let keys: Vec<_> = stream.pages.keys().copied().collect();
    for key in keys {
        let demanded = stream.desired.contains(&key);
        let Some(state) = stream.pages.remove(&key) else {
            continue;
        };
        match state {
            PageState::Resident(attachment) if !demanded => {
                stream.pages.insert(
                    key,
                    PageState::Cooling {
                        attachment,
                        remove_at: now + Duration::from_secs_f32(COOLING_SECONDS),
                    },
                );
            }
            PageState::Cooling { attachment, .. } if demanded => {
                stream.pages.insert(key, PageState::Resident(attachment));
            }
            PageState::Cooling {
                attachment,
                remove_at,
            } if now >= remove_at => {
                despawn_attachment(
                    &mut commands,
                    &mut terrain_meshes,
                    &mut terrain_materials,
                    &mut terrain_images,
                    attachment,
                );
            }
            PageState::Prepared(_) if !demanded => {}
            other => {
                stream.pages.insert(key, other);
            }
        }
    }
}

#[derive(Resource, Default)]
pub struct StreamingStats {
    pub status: String,
    pub demanded: usize,
    pub loading: usize,
    pub prepared: usize,
    pub resident: usize,
    pub cooling: usize,
    pub failed: usize,
    pub owned_entities: usize,
    pub decoded_bytes: u64,
    pub gpu_bytes_estimate: u64,
    pub cached_definitions: usize,
    pub gameplay_objects: usize,
    pub vegetation_pages: usize,
    pub height_only_pages: usize,
    pub indexed_cells: usize,
    pub source_demand_error: Option<String>,
    pub pending_decoded_bytes: u64,
    pub budget_waiting: usize,
    pub lod_counts: BTreeMap<u8, usize>,
    pub minimum_projected_height: f32,
    pub maximum_projected_height: f32,
    terrain_texture_sets: BTreeSet<TerrainTextureSetId>,
}

pub(super) fn update_streaming_stats(
    world: Res<WorldStream>,
    stream: Res<SourceResidency>,
    active_space: Res<ActiveWorldSpace>,
    lod_objects: Query<&ScreenSpaceLod>,
    mut stats: ResMut<StreamingStats>,
    reload: Res<WorldGenerationReload>,
) {
    stats.status = match &world.phase {
        StreamPhase::Opening => "opening SQLite".into(),
        StreamPhase::Ready => world.manifest.as_ref().map_or_else(
            || "ready".into(),
            |manifest| {
                let space = active_space
                    .current
                    .and_then(|id| manifest.world_space(id))
                    .map(|space| format!("{} ({})", space.name, space.id.0))
                    .unwrap_or_else(|| "no active space".into());
                format!("{} | generation {}", space, manifest.generation_id)
            },
        ),
        StreamPhase::Failed(error) => format!("failed: {error}"),
    };
    stats.indexed_cells = world.descriptors.len();
    if reload.active() {
        stats.status.push_str(if reload.commit_requested {
            " | committing published generation"
        } else {
            " | preparing published generation"
        });
    }
    if let Some(error) = &reload.last_error {
        stats
            .status
            .push_str(&format!(" | publication adoption failed: {error}"));
    }
    if active_space.requested.is_some() {
        stats.status.push_str(" | preparing world entry");
    }
    if let Some(error) = &active_space.transition_error {
        stats
            .status
            .push_str(&format!(" | entry rejected: {error}"));
    }
    stats.source_demand_error = world.demand_error.clone();
    stats.budget_waiting = stream.admission_blocked;
    if stream.admission_blocked > 0 {
        stats.status.push_str(" | source residency budget full");
    }
    if let Some(error) = &world.demand_error {
        stats.status = format!("source demand limited: {error}");
    }
    stats.pending_decoded_bytes = 0;
    stats.height_only_pages = 0;
    stats.demanded = stream.desired.len();
    stats.loading = 0;
    stats.prepared = 0;
    stats.resident = 0;
    stats.cooling = 0;
    stats.failed = 0;
    stats.owned_entities = 0;
    stats.decoded_bytes = 0;
    stats.gpu_bytes_estimate = 0;
    stats.cached_definitions = stream.definition_cache.len();
    stats.gameplay_objects = 0;
    stats.vegetation_pages = 0;
    stats.terrain_texture_sets.clear();
    stats.lod_counts.clear();
    stats.minimum_projected_height = f32::INFINITY;
    stats.maximum_projected_height = 0.0;
    for lod in &lod_objects {
        *stats.lod_counts.entry(lod.current_lod()).or_default() += 1;
        stats.minimum_projected_height = stats.minimum_projected_height.min(lod.projected_height());
        stats.maximum_projected_height = stats.maximum_projected_height.max(lod.projected_height());
    }
    if stats.lod_counts.is_empty() {
        stats.minimum_projected_height = 0.0;
    }
    for state in stream.pages.values() {
        match state {
            PageState::Loading { .. } | PageState::Decoding { .. } => stats.loading += 1,
            PageState::Prepared(p) => {
                stats.prepared += 1;
                stats.pending_decoded_bytes += p.decoded.decoded_bytes;
            }
            PageState::Resident(attachment) => {
                stats.resident += 1;
                stats.owned_entities += attachment.entities.len();
                stats.decoded_bytes += attachment.decoded_bytes;
                stats.gpu_bytes_estimate += attachment.gpu_bytes_estimate;
                stats.gameplay_objects += attachment.gameplay_objects;
                stats.vegetation_pages += attachment.vegetation_pages;
                stats.height_only_pages += attachment.height_only_pages;
                account_terrain_texture_set(&mut stats, attachment);
            }
            PageState::Cooling { attachment, .. } => {
                stats.cooling += 1;
                stats.owned_entities += attachment.entities.len();
                stats.decoded_bytes += attachment.decoded_bytes;
                stats.gpu_bytes_estimate += attachment.gpu_bytes_estimate;
                stats.gameplay_objects += attachment.gameplay_objects;
                stats.vegetation_pages += attachment.vegetation_pages;
                stats.height_only_pages += attachment.height_only_pages;
                account_terrain_texture_set(&mut stats, attachment);
            }
            PageState::Failed(error) => {
                let _ = error;
                stats.failed += 1;
            }
        }
    }
}

fn account_terrain_texture_set(stats: &mut StreamingStats, attachment: &PageAttachment) {
    let Some((texture_set, gpu_bytes)) = attachment.terrain_texture_set else {
        return;
    };
    if stats.terrain_texture_sets.insert(texture_set) {
        stats.gpu_bytes_estimate = stats.gpu_bytes_estimate.saturating_add(gpu_bytes);
    }
}
