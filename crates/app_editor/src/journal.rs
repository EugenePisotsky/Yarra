//! Crash-recoverable journal for dirty objects, environment definitions and coverage.

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
    AssetId, CellCoord, ObjectActivationPolicy, ObjectDefinitionId, StableObjectId, WorldSpaceId,
};
use world_db::{
    DenseSourceRecord, SourceEnvironmentCellRecord, SourceObjectDefinitionRecord,
    SourceObjectRecord, SourceObjectViewRecord,
};

use crate::{
    domain_editing::{
        DenseDomainWorkingSets, DirtyDefinitionSnapshot, DirtyDenseSnapshot, DirtyPresetSnapshot,
    },
    editing::{DirtyObjectSnapshot, EditorHistory, EditorObjectWorkingSet},
};

const JOURNAL_SCHEMA_VERSION: u32 = 13;
const OLDEST_SUPPORTED_JOURNAL_SCHEMA_VERSION: u32 = 12;
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
    atmosphere: u64,
    roads: u64,
    objects: u64,
    dense: u64,
}

#[derive(Debug)]
struct JournalRecovery {
    atmospheres: Vec<crate::atmosphere_authoring::working::Snapshot>,
    roads: Vec<crate::road_authoring::working::RoadEntry>,
    presets: Option<DirtyPresetSnapshot>,
    definitions: Vec<DirtyDefinitionSnapshot>,
    objects: Vec<JournalEntry>,
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
    atmospheres: Vec<crate::atmosphere_authoring::working::Snapshot>,
    roads: Vec<crate::road_authoring::working::RoadEntry>,
    presets: Option<DirtyPresetSnapshot>,
    definition_entries: Vec<DirtyDefinitionSnapshot>,
    schema_version: u32,
    project_database: PathBuf,
    entries: Vec<JournalEntry>,
    #[serde(default)]
    dense_entries: Vec<JournalDenseEntry>,
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
    EnvironmentCoverage {
        space: WorldSpaceId,
        cell: CellCoord,
        definition_revision: u64,
        tiles: Vec<environment::CoverageTile>,
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

impl JournalDenseRecord {
    fn from_source(record: &DenseSourceRecord) -> Option<Self> {
        match record {
            DenseSourceRecord::EnvironmentCoverage(record) => Some(Self::EnvironmentCoverage {
                space: record.space,
                cell: record.cell,
                definition_revision: record.definition_revision,
                tiles: record.tiles.clone(),
                source_revision: record.source_revision,
            }),
        }
    }
}

impl From<JournalDenseRecord> for DenseSourceRecord {
    fn from(record: JournalDenseRecord) -> Self {
        match record {
            JournalDenseRecord::EnvironmentCoverage {
                space,
                cell,
                definition_revision,
                tiles,
                source_revision,
            } => Self::EnvironmentCoverage(SourceEnvironmentCellRecord {
                space,
                cell,
                definition_revision,
                tiles,
                source_revision,
            }),
        }
    }
}

impl JournalDenseEntry {
    fn from_snapshot(snapshot: DirtyDenseSnapshot) -> Option<Self> {
        Some(Self {
            base: snapshot
                .base
                .as_ref()
                .and_then(JournalDenseRecord::from_source),
            current: JournalDenseRecord::from_source(&snapshot.current)?,
            runtime: snapshot
                .runtime
                .as_ref()
                .and_then(JournalDenseRecord::from_source),
        })
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
                let dense_count = file.dense_entries.len();
                status.pending_restore = Some(JournalRecovery {
                    atmospheres: file.atmospheres,
                    roads: file.roads,
                    presets: file.presets,
                    definitions: file.definition_entries,
                    objects: file.entries,
                    dense: file.dense_entries,
                });
                status.message = format!(
                    "recovering {object_count} object(s) and {dense_count} environment coverage record(s)"
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

pub(crate) fn restore_loaded_journal(
    mut status: ResMut<EditorJournalStatus>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut dense_domains: ResMut<DenseDomainWorkingSets>,
    mut history: ResMut<EditorHistory>,
    project: Res<crate::project_store::ProjectEditorStore>,
) {
    let Some(plants) = project.vegetation_catalog() else {
        return;
    };
    let Some(library) = project.presets() else {
        return;
    };
    let Some(recovery) = status.pending_restore.take() else {
        return;
    };
    dense_domains.initialize_presets(library);
    let recovered_presets = recovery.presets.map_or(0, |p| {
        usize::from(dense_domains.restore_preset_snapshot(p, plants))
    });
    let mut recovered_definitions = 0;
    for entry in recovery.definitions {
        recovered_definitions +=
            usize::from(dense_domains.restore_definition_snapshot(entry, plants));
    }
    let mut recovered_objects = 0;
    for entry in recovery.objects {
        recovered_objects += usize::from(objects.restore_dirty_snapshot(entry.into()));
    }
    let mut recovered_dense = 0;
    for entry in recovery.dense {
        recovered_dense += usize::from(dense_domains.restore_dirty_snapshot(entry.into()));
    }
    let recovered_atmospheres = dense_domains.atmospheres.restore(recovery.atmospheres);
    let recovered_roads = dense_domains.roads.restore(recovery.roads);
    let recovered = recovered_atmospheres
        + recovered_roads
        + recovered_objects
        + recovered_dense
        + recovered_definitions
        + recovered_presets;
    if recovered != 0 {
        history.clear();
    }
    status.recovered_entries = recovered;
    status.message = format!(
        "recovered {recovered_roads} road records, {recovered_objects} object(s), {recovered_dense} coverage cells and {recovered_definitions} layer definitions and {recovered_presets} preset library"
    );
}

fn dispatch_dirty_journal(
    worker: Option<Res<JournalWorker>>,
    config: Res<EditorJournalConfig>,
    objects: Res<EditorObjectWorkingSet>,
    dense_domains: Res<DenseDomainWorkingSets>,
    mut status: ResMut<EditorJournalStatus>,
) {
    let Some(worker) = worker else {
        return;
    };
    let revision = JournalRevision {
        atmosphere: dense_domains.atmospheres.revision,
        roads: dense_domains.roads.revision,
        objects: objects.edit_revision(),
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
        .filter_map(JournalDenseEntry::from_snapshot)
        .collect::<Vec<_>>();
    let definition_entries = dense_domains.dirty_definition_snapshots();
    let presets = dense_domains.dirty_preset_snapshot();
    let roads = dense_domains.roads.journal();
    let atmospheres = dense_domains.atmospheres.journal();
    let request = if atmospheres.is_empty()
        && roads.is_empty()
        && entries.is_empty()
        && dense_entries.is_empty()
        && definition_entries.is_empty()
        && presets.is_none()
    {
        JournalRequest::Clear { revision }
    } else {
        JournalRequest::Write {
            revision,
            file: JournalFile {
                atmospheres,
                roads,
                presets,
                schema_version: JOURNAL_SCHEMA_VERSION,
                project_database: config.project_database.clone(),
                entries,
                dense_entries,
                definition_entries,
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
    #[test]
    fn journal_recovers_a_new_layer_and_its_unsaved_coverage_together() {
        let dir =
            std::env::temp_dir().join(format!("yarra-layer-journal-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let project = dir.join("project.sqlite");
        let journal = dir.join("journal.ron");
        world_cook::create_demo_project(&project).unwrap();
        let reader = world_db::ProjectReader::open_read_only(&project).unwrap();
        let base = reader
            .read_environment_definitions()
            .unwrap()
            .into_iter()
            .find(|d| !d.layers.is_empty())
            .unwrap();
        let plants = reader
            .read_environment_snapshot(base.space, &[CellCoord::ZERO])
            .unwrap()
            .vegetation_catalog;
        let mut current = base.clone();
        let mut layer = base.layers[0].clone();
        layer.id = environment::LayerId([92; 16]);
        layer.order = 50;
        let base_presets = reader.read_environment_presets().unwrap();
        let mut presets = base_presets.clone();
        let mut preset = presets.get(layer.preset).unwrap().clone();
        preset.id = environment::PresetId([92; 16]);
        preset.name = "Recovered preset".into();
        layer.preset = preset.id;
        presets.presets.push(preset);
        current.layers.push(layer.clone());
        let paint = SourceEnvironmentCellRecord {
            space: base.space,
            cell: CellCoord::ZERO,
            source_revision: 0,
            definition_revision: base.revision,
            tiles: vec![environment::CoverageTile {
                layer: layer.id,
                samples: vec![128; usize::from(base.mask_resolution).pow(2)],
            }],
        };
        let file = JournalFile {
            atmospheres: vec![],
            roads: vec![],
            presets: Some(DirtyPresetSnapshot {
                base: base_presets,
                current: presets.clone(),
            }),
            definition_entries: vec![DirtyDefinitionSnapshot {
                base,
                current: current.clone(),
            }],
            schema_version: JOURNAL_SCHEMA_VERSION,
            project_database: project.clone(),
            entries: vec![],
            dense_entries: vec![
                JournalDenseEntry::from_snapshot(DirtyDenseSnapshot {
                    base: None,
                    current: DenseSourceRecord::EnvironmentCoverage(paint.clone()),
                    runtime: None,
                })
                .unwrap(),
            ],
        };
        write_journal_atomically(&journal, &file).unwrap();
        let recovered = load_journal(&journal, &project).unwrap().unwrap();
        let mut dense = DenseDomainWorkingSets::default();
        dense.initialize_presets(&reader.read_environment_presets().unwrap());
        assert!(dense.restore_preset_snapshot(recovered.presets.unwrap(), &plants));
        assert_eq!(dense.presets(), Some(&presets));
        for entry in recovered.definition_entries {
            assert!(dense.restore_definition_snapshot(entry, &plants));
        }
        for entry in recovered.dense_entries {
            assert!(dense.restore_dirty_snapshot(entry.into()));
        }
        assert_eq!(dense.definition(current.space), Some(&current));
        assert_eq!(
            dense.environment_record(current.space, CellCoord::ZERO),
            Some(&paint)
        );
        assert_eq!(dense.dirty_count(), 3);
        assert!(load_journal(&journal, &dir.join("different.sqlite")).is_err());
        drop(reader);
        fs::remove_dir_all(dir).unwrap();
    }
}
