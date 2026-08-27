//! Crash-recoverable journal for dirty object, catalog, region, and dense-source working sets.

use std::{
    fs::{self, File},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    thread::{self, JoinHandle},
};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use serde::{Deserialize, Serialize};
use world::{
    AssetId, CellCoord, GroundCoverBladeRecipe, GroundCoverLayerId, GroundCoverPresetId,
    GroundCoverRegionId, GroundCoverVisualId, ObjectActivationPolicy, ObjectDefinitionId,
    StableObjectId, WorldSpaceId,
};
use world_db::{
    DenseSourceRecord, GroundCoverCatalogRecord, SourceGroundCoverCardVisualRecord,
    SourceGroundCoverCellMaskRecord, SourceGroundCoverPresetRecord, SourceGroundCoverRegionRecord,
    SourceGroundCoverVisualDefinition, SourceGroundCoverVisualRecord, SourceObjectDefinitionRecord,
    SourceObjectRecord, SourceObjectViewRecord, SourceTerrainCellWeightPageRecord,
};

use crate::{
    catalog_editing::{DirtyRegionSnapshot, GroundCoverRegionWorkingSet},
    domain_editing::{DenseDomainWorkingSets, DirtyDenseSnapshot},
    editing::{DirtyObjectSnapshot, EditorHistory, EditorObjectWorkingSet},
    ground_cover_catalog::{DirtyCatalogSnapshot, GroundCoverCatalogWorkingSet},
};

const JOURNAL_SCHEMA_VERSION: u32 = 4;
const OLDEST_SUPPORTED_JOURNAL_SCHEMA_VERSION: u32 = 1;
const JOURNAL_CHANNEL_CAPACITY: usize = 1;

pub(crate) struct EditorJournalPlugin {
    project_database: PathBuf,
}

impl EditorJournalPlugin {
    pub(crate) fn new(project_database: impl Into<PathBuf>) -> Self {
        Self {
            project_database: project_database.into(),
        }
    }
}

impl Plugin for EditorJournalPlugin {
    fn build(&self, app: &mut App) {
        let path = self.project_database.with_extension("editor-journal.ron");
        app.insert_resource(EditorJournalConfig {
            project_database: self.project_database.clone(),
            path,
        })
        .init_resource::<EditorJournalStatus>()
        .add_systems(Startup, start_journal_worker)
        .add_systems(
            Update,
            (
                receive_journal_results,
                restore_loaded_journal,
                dispatch_dirty_journal,
            )
                .chain(),
        );
    }
}

#[derive(Resource)]
struct EditorJournalConfig {
    project_database: PathBuf,
    path: PathBuf,
}

#[derive(Resource, Default)]
pub(crate) struct EditorJournalStatus {
    message: String,
    pending_restore: Option<JournalRecovery>,
    last_dispatched_revision: Option<JournalRevision>,
    last_written_revision: Option<JournalRevision>,
    recovered_entries: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JournalRevision {
    objects: u64,
    catalog: u64,
    regions: u64,
    dense: u64,
}

#[derive(Debug)]
struct JournalRecovery {
    objects: Vec<JournalEntry>,
    catalog: Vec<JournalCatalogEntry>,
    regions: Vec<JournalRegionEntry>,
    dense: Vec<JournalDenseEntry>,
}

impl EditorJournalStatus {
    pub(crate) fn message(&self) -> &str {
        if self.message.is_empty() {
            "journal starting"
        } else {
            &self.message
        }
    }

    pub(crate) const fn recovered_entries(&self) -> usize {
        self.recovered_entries
    }
}

#[derive(Resource)]
struct JournalWorker {
    requests: Sender<JournalRequest>,
    results: Receiver<JournalResult>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for JournalWorker {
    fn drop(&mut self) {
        let _ = self.requests.send(JournalRequest::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

enum JournalRequest {
    Write {
        revision: JournalRevision,
        file: JournalFile,
    },
    Clear {
        revision: JournalRevision,
    },
    Shutdown,
}

enum JournalResult {
    Loaded(Result<Option<JournalFile>, String>),
    Written {
        revision: JournalRevision,
        result: Result<(), String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalFile {
    schema_version: u32,
    project_database: PathBuf,
    entries: Vec<JournalEntry>,
    #[serde(default)]
    catalog_entries: Vec<JournalCatalogEntry>,
    #[serde(default)]
    region_entries: Vec<JournalRegionEntry>,
    #[serde(default)]
    dense_entries: Vec<JournalDenseEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalCatalogEntry {
    base: Option<JournalCatalogRecord>,
    current: Option<JournalCatalogRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum JournalCatalogRecord {
    Preset {
        id: GroundCoverPresetId,
        key: String,
        display_name: String,
        enabled: bool,
        visual: GroundCoverVisualId,
        density_per_square_meter: f32,
        seed: u32,
        source_revision: i64,
    },
    CardVisual {
        id: GroundCoverVisualId,
        key: String,
        display_name: String,
        source_revision: i64,
        built_in_atlas_version: u32,
        #[serde(default)]
        procedural_recipe: Option<GroundCoverBladeRecipe>,
        bottom_color: [f32; 3],
        top_color: [f32; 3],
        minimum_card_height: f32,
        maximum_card_height: f32,
        minimum_card_width: f32,
        maximum_card_width: f32,
        flattened_card_probability: f32,
        maximum_wind_displacement: f32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalRegionEntry {
    base: Option<JournalRegion>,
    current: Option<JournalRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalRegion {
    id: GroundCoverRegionId,
    layer: GroundCoverLayerId,
    space: WorldSpaceId,
    preset: GroundCoverPresetId,
    display_name: String,
    enabled: bool,
    density_multiplier: f32,
    source_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalEntry {
    presentation: JournalPresentation,
    base: Option<JournalObject>,
    current: Option<JournalObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalPresentation {
    object: JournalObject,
    definition_key: String,
    definition_display_name: String,
    definition_visual_asset: Option<AssetId>,
    definition_activation: ObjectActivationPolicy,
    visual_uri: Option<String>,
    visual_bounds: Option<[f32; 3]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalObject {
    id: StableObjectId,
    space: WorldSpaceId,
    owner_cell: CellCoord,
    definition: ObjectDefinitionId,
    local_translation: [f32; 3],
    yaw: f32,
    scale: f32,
    source_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalDenseEntry {
    base: Option<JournalDenseRecord>,
    current: JournalDenseRecord,
    runtime: Option<JournalDenseRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum JournalDenseRecord {
    TerrainWeights {
        space: WorldSpaceId,
        cell: CellCoord,
        page: u8,
        resolution: u16,
        rgba: Vec<u8>,
        source_revision: i64,
    },
    GroundCoverMask {
        region: GroundCoverRegionId,
        space: WorldSpaceId,
        cell: CellCoord,
        resolution: u8,
        coverage: Vec<u8>,
        source_revision: i64,
    },
}

impl From<&SourceObjectRecord> for JournalObject {
    fn from(object: &SourceObjectRecord) -> Self {
        Self {
            id: object.id,
            space: object.space,
            owner_cell: object.owner_cell,
            definition: object.definition,
            local_translation: object.local_translation,
            yaw: object.yaw,
            scale: object.scale,
            source_revision: object.source_revision,
        }
    }
}

impl From<JournalObject> for SourceObjectRecord {
    fn from(object: JournalObject) -> Self {
        Self {
            id: object.id,
            space: object.space,
            owner_cell: object.owner_cell,
            definition: object.definition,
            local_translation: object.local_translation,
            yaw: object.yaw,
            scale: object.scale,
            source_revision: object.source_revision,
        }
    }
}

impl From<DirtyObjectSnapshot> for JournalEntry {
    fn from(snapshot: DirtyObjectSnapshot) -> Self {
        let presentation = snapshot.presentation;
        Self {
            presentation: JournalPresentation {
                object: JournalObject::from(&presentation.object),
                definition_key: presentation.definition.key,
                definition_display_name: presentation.definition.display_name,
                definition_visual_asset: presentation.definition.visual_asset,
                definition_activation: presentation.definition.activation,
                visual_uri: presentation.visual_uri,
                visual_bounds: presentation.visual_bounds,
            },
            base: snapshot.base.as_ref().map(JournalObject::from),
            current: snapshot.current.as_ref().map(JournalObject::from),
        }
    }
}

impl From<JournalEntry> for DirtyObjectSnapshot {
    fn from(entry: JournalEntry) -> Self {
        let object = SourceObjectRecord::from(entry.presentation.object);
        let definition = SourceObjectDefinitionRecord {
            id: object.definition,
            key: entry.presentation.definition_key,
            display_name: entry.presentation.definition_display_name,
            visual_asset: entry.presentation.definition_visual_asset,
            activation: entry.presentation.definition_activation,
        };
        Self {
            presentation: SourceObjectViewRecord {
                object,
                definition,
                visual_uri: entry.presentation.visual_uri,
                visual_bounds: entry.presentation.visual_bounds,
            },
            base: entry.base.map(SourceObjectRecord::from),
            current: entry.current.map(SourceObjectRecord::from),
        }
    }
}

impl From<&DenseSourceRecord> for JournalDenseRecord {
    fn from(record: &DenseSourceRecord) -> Self {
        match record {
            DenseSourceRecord::TerrainWeights(record) => Self::TerrainWeights {
                space: record.space,
                cell: record.cell,
                page: record.page,
                resolution: record.resolution,
                rgba: record.rgba.clone(),
                source_revision: record.source_revision,
            },
            DenseSourceRecord::GroundCoverMask(record) => Self::GroundCoverMask {
                region: record.region,
                space: record.space,
                cell: record.cell,
                resolution: record.resolution,
                coverage: record.coverage.clone(),
                source_revision: record.source_revision,
            },
        }
    }
}

impl From<JournalDenseRecord> for DenseSourceRecord {
    fn from(record: JournalDenseRecord) -> Self {
        match record {
            JournalDenseRecord::TerrainWeights {
                space,
                cell,
                page,
                resolution,
                rgba,
                source_revision,
            } => Self::TerrainWeights(SourceTerrainCellWeightPageRecord {
                space,
                cell,
                page,
                resolution,
                rgba,
                source_revision,
            }),
            JournalDenseRecord::GroundCoverMask {
                region,
                space,
                cell,
                resolution,
                coverage,
                source_revision,
            } => Self::GroundCoverMask(SourceGroundCoverCellMaskRecord {
                region,
                space,
                cell,
                resolution,
                coverage,
                source_revision,
            }),
        }
    }
}

impl From<DirtyDenseSnapshot> for JournalDenseEntry {
    fn from(snapshot: DirtyDenseSnapshot) -> Self {
        Self {
            base: snapshot.base.as_ref().map(JournalDenseRecord::from),
            current: JournalDenseRecord::from(&snapshot.current),
            runtime: snapshot.runtime.as_ref().map(JournalDenseRecord::from),
        }
    }
}

impl From<JournalDenseEntry> for DirtyDenseSnapshot {
    fn from(entry: JournalDenseEntry) -> Self {
        Self {
            base: entry.base.map(DenseSourceRecord::from),
            current: DenseSourceRecord::from(entry.current),
            runtime: entry.runtime.map(DenseSourceRecord::from),
        }
    }
}

impl From<&SourceGroundCoverRegionRecord> for JournalRegion {
    fn from(region: &SourceGroundCoverRegionRecord) -> Self {
        Self {
            id: region.id,
            layer: region.layer,
            space: region.space,
            preset: region.preset,
            display_name: region.display_name.clone(),
            enabled: region.enabled,
            density_multiplier: region.density_multiplier,
            source_revision: region.source_revision,
        }
    }
}

impl From<JournalRegion> for SourceGroundCoverRegionRecord {
    fn from(region: JournalRegion) -> Self {
        Self {
            id: region.id,
            layer: region.layer,
            space: region.space,
            preset: region.preset,
            display_name: region.display_name,
            enabled: region.enabled,
            density_multiplier: region.density_multiplier,
            source_revision: region.source_revision,
        }
    }
}

impl From<DirtyRegionSnapshot> for JournalRegionEntry {
    fn from(snapshot: DirtyRegionSnapshot) -> Self {
        Self {
            base: snapshot.base.as_ref().map(JournalRegion::from),
            current: snapshot.current.as_ref().map(JournalRegion::from),
        }
    }
}

impl From<JournalRegionEntry> for DirtyRegionSnapshot {
    fn from(entry: JournalRegionEntry) -> Self {
        Self {
            base: entry.base.map(SourceGroundCoverRegionRecord::from),
            current: entry.current.map(SourceGroundCoverRegionRecord::from),
        }
    }
}

impl From<&GroundCoverCatalogRecord> for JournalCatalogRecord {
    fn from(record: &GroundCoverCatalogRecord) -> Self {
        match record {
            GroundCoverCatalogRecord::Preset(record) => Self::Preset {
                id: record.id,
                key: record.key.clone(),
                display_name: record.display_name.clone(),
                enabled: record.enabled,
                visual: record.visual,
                density_per_square_meter: record.density_per_square_meter,
                seed: record.seed,
                source_revision: record.source_revision,
            },
            GroundCoverCatalogRecord::Visual(record) => {
                let SourceGroundCoverVisualDefinition::CardCluster(card) = &record.definition;
                Self::CardVisual {
                    id: record.id,
                    key: record.key.clone(),
                    display_name: record.display_name.clone(),
                    source_revision: record.source_revision,
                    built_in_atlas_version: card.built_in_atlas_version,
                    procedural_recipe: card.procedural_recipe,
                    bottom_color: card.bottom_color,
                    top_color: card.top_color,
                    minimum_card_height: card.minimum_card_height,
                    maximum_card_height: card.maximum_card_height,
                    minimum_card_width: card.minimum_card_width,
                    maximum_card_width: card.maximum_card_width,
                    flattened_card_probability: card.flattened_card_probability,
                    maximum_wind_displacement: card.maximum_wind_displacement,
                }
            }
        }
    }
}

impl From<JournalCatalogRecord> for GroundCoverCatalogRecord {
    fn from(record: JournalCatalogRecord) -> Self {
        match record {
            JournalCatalogRecord::Preset {
                id,
                key,
                display_name,
                enabled,
                visual,
                density_per_square_meter,
                seed,
                source_revision,
            } => Self::Preset(SourceGroundCoverPresetRecord {
                id,
                key,
                display_name,
                enabled,
                visual,
                density_per_square_meter,
                seed,
                source_revision,
            }),
            JournalCatalogRecord::CardVisual {
                id,
                key,
                display_name,
                source_revision,
                built_in_atlas_version,
                procedural_recipe,
                bottom_color,
                top_color,
                minimum_card_height,
                maximum_card_height,
                minimum_card_width,
                maximum_card_width,
                flattened_card_probability,
                maximum_wind_displacement,
            } => Self::Visual(SourceGroundCoverVisualRecord {
                id,
                key,
                display_name,
                source_revision,
                definition: SourceGroundCoverVisualDefinition::CardCluster(
                    SourceGroundCoverCardVisualRecord {
                        built_in_atlas_version,
                        procedural_recipe,
                        bottom_color,
                        top_color,
                        minimum_card_height,
                        maximum_card_height,
                        minimum_card_width,
                        maximum_card_width,
                        flattened_card_probability,
                        maximum_wind_displacement,
                    },
                ),
            }),
        }
    }
}

impl From<DirtyCatalogSnapshot> for JournalCatalogEntry {
    fn from(snapshot: DirtyCatalogSnapshot) -> Self {
        Self {
            base: snapshot.base.as_ref().map(JournalCatalogRecord::from),
            current: snapshot.current.as_ref().map(JournalCatalogRecord::from),
        }
    }
}

impl From<JournalCatalogEntry> for DirtyCatalogSnapshot {
    fn from(entry: JournalCatalogEntry) -> Self {
        Self {
            base: entry.base.map(GroundCoverCatalogRecord::from),
            current: entry.current.map(GroundCoverCatalogRecord::from),
        }
    }
}

fn start_journal_worker(mut commands: Commands, config: Res<EditorJournalConfig>) {
    let (request_sender, request_receiver) = bounded(JOURNAL_CHANNEL_CAPACITY);
    let (result_sender, result_receiver) = bounded(JOURNAL_CHANNEL_CAPACITY + 1);
    let path = config.path.clone();
    let project_database = config.project_database.clone();
    let worker_thread = thread::Builder::new()
        .name("yarra-editor-journal".into())
        .spawn(move || journal_worker(path, project_database, request_receiver, result_sender))
        .expect("failed to spawn editor journal worker");
    commands.insert_resource(JournalWorker {
        requests: request_sender,
        results: result_receiver,
        thread: Some(worker_thread),
    });
}

fn journal_worker(
    path: PathBuf,
    project_database: PathBuf,
    requests: Receiver<JournalRequest>,
    results: Sender<JournalResult>,
) {
    let _ = results.send(JournalResult::Loaded(load_journal(
        &path,
        &project_database,
    )));
    while let Ok(request) = requests.recv() {
        let (revision, result) = match request {
            JournalRequest::Write { revision, file } => {
                (revision, write_journal_atomically(&path, &file))
            }
            JournalRequest::Clear { revision } => (revision, clear_journal(&path)),
            JournalRequest::Shutdown => return,
        };
        if results
            .send(JournalResult::Written { revision, result })
            .is_err()
        {
            return;
        }
    }
}

fn load_journal(path: &Path, project_database: &Path) -> Result<Option<JournalFile>, String> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let file: JournalFile = ron::from_str(&source)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
    if !(OLDEST_SUPPORTED_JOURNAL_SCHEMA_VERSION..=JOURNAL_SCHEMA_VERSION)
        .contains(&file.schema_version)
    {
        return Err(format!(
            "unsupported journal schema {} in {}",
            file.schema_version,
            path.display()
        ));
    }
    if file.project_database != project_database {
        return Err(format!(
            "journal {} belongs to a different project",
            path.display()
        ));
    }
    Ok(Some(file))
}

fn write_journal_atomically(path: &Path, file: &JournalFile) -> Result<(), String> {
    let source = ron::ser::to_string_pretty(file, ron::ser::PrettyConfig::default())
        .map_err(|error| format!("could not encode editor journal: {error}"))?;
    let temporary = path.with_extension("editor-journal.ron.tmp");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let mut output = File::create(&temporary)
        .map_err(|error| format!("could not create {}: {error}", temporary.display()))?;
    output
        .write_all(source.as_bytes())
        .and_then(|()| output.sync_all())
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("could not publish {}: {error}", path.display()))
}

fn clear_journal(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not remove {}: {error}", path.display())),
    }
}

fn receive_journal_results(
    worker: Option<Res<JournalWorker>>,
    mut status: ResMut<EditorJournalStatus>,
) {
    let Some(worker) = worker else {
        return;
    };
    loop {
        match worker.results.try_recv() {
            Ok(JournalResult::Loaded(Ok(Some(file)))) => {
                let object_count = file.entries.len();
                let catalog_count = file.catalog_entries.len();
                let region_count = file.region_entries.len();
                let dense_count = file.dense_entries.len();
                status.pending_restore = Some(JournalRecovery {
                    objects: file.entries,
                    catalog: file.catalog_entries,
                    regions: file.region_entries,
                    dense: file.dense_entries,
                });
                status.message = format!(
                    "recovering {object_count} object(s), {catalog_count} definition(s), {region_count} region(s), and {dense_count} dense record(s)"
                );
            }
            Ok(JournalResult::Loaded(Ok(None))) => {
                status.message = "journal clean".into();
            }
            Ok(JournalResult::Loaded(Err(error))) => {
                status.message = format!("journal recovery failed: {error}");
            }
            Ok(JournalResult::Written {
                revision,
                result: Ok(()),
            }) => {
                status.last_written_revision = Some(revision);
                status.message = "dirty journal current".into();
            }
            Ok(JournalResult::Written {
                revision,
                result: Err(error),
            }) => {
                if status.last_dispatched_revision == Some(revision) {
                    status.last_dispatched_revision = None;
                }
                status.message = format!("journal write failed: {error}");
            }
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                status.message = "journal worker stopped".into();
                return;
            }
        }
    }
}

fn restore_loaded_journal(
    mut status: ResMut<EditorJournalStatus>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut catalog: ResMut<GroundCoverCatalogWorkingSet>,
    mut regions: ResMut<GroundCoverRegionWorkingSet>,
    mut dense_domains: ResMut<DenseDomainWorkingSets>,
    mut history: ResMut<EditorHistory>,
) {
    let Some(recovery) = status.pending_restore.take() else {
        return;
    };
    let mut recovered_objects = 0;
    for entry in recovery.objects {
        recovered_objects += usize::from(objects.restore_dirty_snapshot(entry.into()));
    }
    let mut recovered_regions = 0;
    for entry in recovery.regions {
        recovered_regions += usize::from(regions.restore_dirty_snapshot(entry.into()));
    }
    let mut recovered_catalog = 0;
    for entry in recovery.catalog {
        recovered_catalog += usize::from(catalog.restore_dirty_snapshot(entry.into()));
    }
    let mut recovered_dense = 0;
    for entry in recovery.dense {
        recovered_dense += usize::from(dense_domains.restore_dirty_snapshot(entry.into()));
    }
    let recovered = recovered_objects + recovered_catalog + recovered_regions + recovered_dense;
    if recovered != 0 {
        history.clear();
    }
    status.recovered_entries = recovered;
    status.message = format!(
        "recovered {recovered_objects} object(s), {recovered_catalog} definition(s), {recovered_regions} region(s), and {recovered_dense} dense record(s)"
    );
}

fn dispatch_dirty_journal(
    worker: Option<Res<JournalWorker>>,
    config: Res<EditorJournalConfig>,
    objects: Res<EditorObjectWorkingSet>,
    catalog: Res<GroundCoverCatalogWorkingSet>,
    regions: Res<GroundCoverRegionWorkingSet>,
    dense_domains: Res<DenseDomainWorkingSets>,
    mut status: ResMut<EditorJournalStatus>,
) {
    let Some(worker) = worker else {
        return;
    };
    let revision = JournalRevision {
        objects: objects.edit_revision(),
        catalog: catalog.edit_revision(),
        regions: regions.edit_revision(),
        dense: dense_domains.edit_revision(),
    };
    if status.pending_restore.is_some() || status.last_dispatched_revision == Some(revision) {
        return;
    }
    let entries = objects
        .dirty_snapshots()
        .into_iter()
        .map(JournalEntry::from)
        .collect::<Vec<_>>();
    let dense_entries = dense_domains
        .dirty_snapshots()
        .into_iter()
        .map(JournalDenseEntry::from)
        .collect::<Vec<_>>();
    let catalog_entries = catalog
        .dirty_snapshots()
        .into_iter()
        .map(JournalCatalogEntry::from)
        .collect::<Vec<_>>();
    let region_entries = regions
        .dirty_snapshots()
        .into_iter()
        .map(JournalRegionEntry::from)
        .collect::<Vec<_>>();
    let request = if entries.is_empty()
        && catalog_entries.is_empty()
        && region_entries.is_empty()
        && dense_entries.is_empty()
    {
        JournalRequest::Clear { revision }
    } else {
        JournalRequest::Write {
            revision,
            file: JournalFile {
                schema_version: JOURNAL_SCHEMA_VERSION,
                project_database: config.project_database.clone(),
                entries,
                catalog_entries,
                region_entries,
                dense_entries,
            },
        }
    };
    match worker.requests.try_send(request) {
        Ok(()) => status.last_dispatched_revision = Some(revision),
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => {
            status.message = "journal worker stopped".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::ObjectActivationPolicy;

    fn journal_file(project: &Path) -> JournalFile {
        let object = JournalObject {
            id: StableObjectId([1; 16]),
            space: WorldSpaceId(2),
            owner_cell: CellCoord { x: -4, z: 7 },
            definition: ObjectDefinitionId([3; 16]),
            local_translation: [1.0, 2.0, 3.0],
            yaw: 0.5,
            scale: 1.25,
            source_revision: 9,
        };
        JournalFile {
            schema_version: JOURNAL_SCHEMA_VERSION,
            project_database: project.to_path_buf(),
            entries: vec![JournalEntry {
                presentation: JournalPresentation {
                    object: object.clone(),
                    definition_key: "tree".into(),
                    definition_display_name: "Tree".into(),
                    definition_visual_asset: None,
                    definition_activation: ObjectActivationPolicy::RenderOnly,
                    visual_uri: Some("tree.gltf".into()),
                    visual_bounds: Some([2.0, 8.0, 2.0]),
                },
                base: Some(object.clone()),
                current: Some(JournalObject {
                    local_translation: [9.0, 2.0, 3.0],
                    ..object
                }),
            }],
            catalog_entries: vec![JournalCatalogEntry {
                base: Some(JournalCatalogRecord::Preset {
                    id: GroundCoverPresetId([5; 16]),
                    key: "meadow".into(),
                    display_name: "Meadow".into(),
                    enabled: true,
                    visual: GroundCoverVisualId([6; 16]),
                    density_per_square_meter: 5.0,
                    seed: 17,
                    source_revision: 2,
                }),
                current: Some(JournalCatalogRecord::Preset {
                    id: GroundCoverPresetId([5; 16]),
                    key: "meadow".into(),
                    display_name: "Meadow".into(),
                    enabled: true,
                    visual: GroundCoverVisualId([6; 16]),
                    density_per_square_meter: 7.5,
                    seed: 17,
                    source_revision: 2,
                }),
            }],
            region_entries: vec![JournalRegionEntry {
                base: None,
                current: Some(JournalRegion {
                    id: GroundCoverRegionId([8; 16]),
                    layer: GroundCoverLayerId([4; 16]),
                    space: WorldSpaceId(2),
                    preset: GroundCoverPresetId([5; 16]),
                    display_name: "Recovered meadow".into(),
                    enabled: true,
                    density_multiplier: 0.75,
                    source_revision: 0,
                }),
            }],
            dense_entries: vec![JournalDenseEntry {
                base: Some(JournalDenseRecord::GroundCoverMask {
                    region: GroundCoverRegionId([8; 16]),
                    space: WorldSpaceId(2),
                    cell: CellCoord { x: -4, z: 7 },
                    resolution: 2,
                    coverage: vec![0; 4],
                    source_revision: 4,
                }),
                current: JournalDenseRecord::GroundCoverMask {
                    region: GroundCoverRegionId([8; 16]),
                    space: WorldSpaceId(2),
                    cell: CellCoord { x: -4, z: 7 },
                    resolution: 2,
                    coverage: vec![255, 0, 0, 0],
                    source_revision: 4,
                },
                runtime: Some(JournalDenseRecord::GroundCoverMask {
                    region: GroundCoverRegionId([8; 16]),
                    space: WorldSpaceId(2),
                    cell: CellCoord { x: -4, z: 7 },
                    resolution: 2,
                    coverage: vec![0; 4],
                    source_revision: 4,
                }),
            }],
        }
    }

    #[test]
    fn journal_round_trips_atomically_and_validates_project_identity() {
        let directory = std::env::temp_dir().join(format!(
            "yarra-editor-journal-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let project = directory.join("project.sqlite");
        let other_project = directory.join("other.sqlite");
        let path = directory.join("project.editor-journal.ron");
        let file = journal_file(&project);

        write_journal_atomically(&path, &file).unwrap();
        let loaded = load_journal(&path, &project).unwrap().unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.catalog_entries.len(), 1);
        assert_eq!(loaded.region_entries.len(), 1);
        assert_eq!(loaded.dense_entries.len(), 1);
        assert!(load_journal(&path, &other_project).is_err());
        clear_journal(&path).unwrap();
        assert!(load_journal(&path, &project).unwrap().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_one_object_only_journal_remains_readable() {
        #[derive(Serialize)]
        struct VersionOneJournalFile {
            schema_version: u32,
            project_database: PathBuf,
            entries: Vec<JournalEntry>,
        }

        let directory = std::env::temp_dir().join(format!(
            "yarra-editor-journal-v1-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let project = directory.join("project.sqlite");
        let path = directory.join("project.editor-journal.ron");
        let current = journal_file(&project);
        let legacy = VersionOneJournalFile {
            schema_version: 1,
            project_database: project.clone(),
            entries: current.entries,
        };
        let source =
            ron::ser::to_string_pretty(&legacy, ron::ser::PrettyConfig::default()).unwrap();
        fs::write(&path, source).unwrap();

        let loaded = load_journal(&path, &project).unwrap().unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.region_entries.is_empty());
        assert!(loaded.dense_entries.is_empty());
        assert!(loaded.catalog_entries.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_two_object_and_dense_journal_remains_readable() {
        #[derive(Serialize)]
        struct VersionTwoJournalFile {
            schema_version: u32,
            project_database: PathBuf,
            entries: Vec<JournalEntry>,
            dense_entries: Vec<JournalDenseEntry>,
        }

        let directory = std::env::temp_dir().join(format!(
            "yarra-editor-journal-v2-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let project = directory.join("project.sqlite");
        let path = directory.join("project.editor-journal.ron");
        let current = journal_file(&project);
        let legacy = VersionTwoJournalFile {
            schema_version: 2,
            project_database: project.clone(),
            entries: current.entries,
            dense_entries: current.dense_entries,
        };
        let source =
            ron::ser::to_string_pretty(&legacy, ron::ser::PrettyConfig::default()).unwrap();
        fs::write(&path, source).unwrap();

        let loaded = load_journal(&path, &project).unwrap().unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.region_entries.is_empty());
        assert_eq!(loaded.dense_entries.len(), 1);
        assert!(loaded.catalog_entries.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_three_region_journal_remains_readable() {
        #[derive(Serialize)]
        struct VersionThreeJournalFile {
            schema_version: u32,
            project_database: PathBuf,
            entries: Vec<JournalEntry>,
            region_entries: Vec<JournalRegionEntry>,
            dense_entries: Vec<JournalDenseEntry>,
        }

        let directory = std::env::temp_dir().join(format!(
            "yarra-editor-journal-v3-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let project = directory.join("project.sqlite");
        let path = directory.join("project.editor-journal.ron");
        let current = journal_file(&project);
        let legacy = VersionThreeJournalFile {
            schema_version: 3,
            project_database: project.clone(),
            entries: current.entries,
            region_entries: current.region_entries,
            dense_entries: current.dense_entries,
        };
        let source =
            ron::ser::to_string_pretty(&legacy, ron::ser::PrettyConfig::default()).unwrap();
        fs::write(&path, source).unwrap();

        let loaded = load_journal(&path, &project).unwrap().unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.region_entries.len(), 1);
        assert_eq!(loaded.dense_entries.len(), 1);
        assert!(loaded.catalog_entries.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn journal_entry_restores_local_and_base_snapshots() {
        let project = Path::new("project.sqlite");
        let entry = journal_file(project).entries.pop().unwrap();
        let snapshot = DirtyObjectSnapshot::from(entry);
        assert_eq!(snapshot.base.as_ref().unwrap().source_revision, 9);
        assert_eq!(snapshot.current.unwrap().local_translation[0], 9.0);
        assert_eq!(snapshot.presentation.definition.display_name, "Tree");
    }

    #[test]
    fn dense_journal_entry_restores_base_current_and_runtime_snapshots() {
        let project = Path::new("project.sqlite");
        let entry = journal_file(project).dense_entries.pop().unwrap();
        let snapshot = DirtyDenseSnapshot::from(entry);
        let DenseSourceRecord::GroundCoverMask(base) = snapshot.base.unwrap() else {
            panic!("expected a ground-cover base")
        };
        let DenseSourceRecord::GroundCoverMask(current) = snapshot.current else {
            panic!("expected ground-cover current intent")
        };
        let DenseSourceRecord::GroundCoverMask(runtime) = snapshot.runtime.unwrap() else {
            panic!("expected a ground-cover runtime baseline")
        };
        assert_eq!(base.coverage, vec![0; 4]);
        assert_eq!(current.coverage, vec![255, 0, 0, 0]);
        assert_eq!(runtime.source_revision, 4);
    }

    #[test]
    fn region_journal_entry_restores_an_unsaved_creation() {
        let project = Path::new("project.sqlite");
        let entry = journal_file(project).region_entries.pop().unwrap();
        let snapshot = DirtyRegionSnapshot::from(entry);
        assert!(snapshot.base.is_none());
        let current = snapshot.current.unwrap();
        assert_eq!(current.display_name, "Recovered meadow");
        assert_eq!(current.density_multiplier, 0.75);
    }

    #[test]
    fn catalog_journal_entry_restores_an_unsaved_preset_update() {
        let project = Path::new("project.sqlite");
        let entry = journal_file(project).catalog_entries.pop().unwrap();
        let snapshot = DirtyCatalogSnapshot::from(entry);
        let Some(GroundCoverCatalogRecord::Preset(current)) = snapshot.current else {
            panic!("expected a preset")
        };
        assert_eq!(current.density_per_square_meter, 7.5);
        assert_eq!(snapshot.base.unwrap().source_revision(), 2);
    }
}
