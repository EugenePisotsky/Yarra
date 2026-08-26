//! Crash-recoverable, bounded dirty-object journal.

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
use world_db::{SourceObjectDefinitionRecord, SourceObjectRecord, SourceObjectViewRecord};

use crate::editing::{DirtyObjectSnapshot, EditorHistory, EditorObjectWorkingSet};

const JOURNAL_SCHEMA_VERSION: u32 = 1;
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
    pending_restore: Option<Vec<JournalEntry>>,
    last_dispatched_revision: u64,
    last_written_revision: u64,
    recovered_entries: usize,
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
    Write { revision: u64, file: JournalFile },
    Clear { revision: u64 },
    Shutdown,
}

enum JournalResult {
    Loaded(Result<Option<JournalFile>, String>),
    Written {
        revision: u64,
        result: Result<(), String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalFile {
    schema_version: u32,
    project_database: PathBuf,
    entries: Vec<JournalEntry>,
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
    if file.schema_version != JOURNAL_SCHEMA_VERSION {
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
                let count = file.entries.len();
                status.pending_restore = Some(file.entries);
                status.message = format!("recovering {count} dirty object(s)");
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
                status.last_written_revision = status.last_written_revision.max(revision);
                status.message = "dirty journal current".into();
            }
            Ok(JournalResult::Written {
                revision,
                result: Err(error),
            }) => {
                if status.last_dispatched_revision == revision {
                    status.last_dispatched_revision = revision.saturating_sub(1);
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
    mut history: ResMut<EditorHistory>,
) {
    let Some(entries) = status.pending_restore.take() else {
        return;
    };
    let mut recovered = 0;
    for entry in entries {
        recovered += usize::from(objects.restore_dirty_snapshot(entry.into()));
    }
    if recovered > 0 {
        history.clear();
    }
    status.recovered_entries = recovered;
    status.message = format!("recovered {recovered} dirty object(s)");
}

fn dispatch_dirty_journal(
    worker: Option<Res<JournalWorker>>,
    config: Res<EditorJournalConfig>,
    objects: Res<EditorObjectWorkingSet>,
    mut status: ResMut<EditorJournalStatus>,
) {
    let Some(worker) = worker else {
        return;
    };
    let revision = objects.edit_revision();
    if status.pending_restore.is_some() || status.last_dispatched_revision == revision {
        return;
    }
    let entries = objects
        .dirty_snapshots()
        .into_iter()
        .map(JournalEntry::from)
        .collect::<Vec<_>>();
    let request = if entries.is_empty() {
        JournalRequest::Clear { revision }
    } else {
        JournalRequest::Write {
            revision,
            file: JournalFile {
                schema_version: JOURNAL_SCHEMA_VERSION,
                project_database: config.project_database.clone(),
                entries,
            },
        }
    };
    match worker.requests.try_send(request) {
        Ok(()) => status.last_dispatched_revision = revision,
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
        assert!(load_journal(&path, &other_project).is_err());
        clear_journal(&path).unwrap();
        assert!(load_journal(&path, &project).unwrap().is_none());
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
}
