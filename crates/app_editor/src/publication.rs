//! Explicit background publication of mutable project source into one immutable runtime snapshot.

use std::{
    path::PathBuf,
    thread::{self, JoinHandle},
};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use engine::WorldGenerationReload;

use crate::{
    derived_jobs::DerivedArtifactStore, domain_editing::DenseDomainWorkingSets,
    editing::EditorObjectWorkingSet, project_store::ProjectEditorStore,
};

const PUBLICATION_CHANNEL_CAPACITY: usize = 1;

pub(crate) struct RuntimePublicationPlugin {
    project_database: PathBuf,
    runtime_database: PathBuf,
    asset_root: PathBuf,
}

impl RuntimePublicationPlugin {
    pub(crate) fn new(
        project_database: impl Into<PathBuf>,
        runtime_database: impl Into<PathBuf>,
        asset_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            project_database: project_database.into(),
            runtime_database: runtime_database.into(),
            asset_root: asset_root.into(),
        }
    }
}

impl Plugin for RuntimePublicationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(RuntimePublicationPaths {
            project_database: self.project_database.clone(),
            runtime_database: self.runtime_database.clone(),
            asset_root: self.asset_root.clone(),
        })
        .init_resource::<RuntimePublicationState>()
        .add_systems(Startup, start_publication_worker)
        .add_systems(
            Update,
            (dispatch_publication, receive_publication_result).chain(),
        );
    }
}

#[derive(Resource)]
pub(crate) struct RuntimePublicationPaths {
    pub(crate) project_database: PathBuf,
    pub(crate) runtime_database: PathBuf,
    pub(crate) asset_root: PathBuf,
}

#[derive(Debug, Clone)]
enum PublicationPhase {
    Idle,
    Queued {
        request_id: u64,
        source_epoch: u64,
    },
    Cooking {
        request_id: u64,
        source_epoch: u64,
    },
    Adopting {
        request_id: u64,
        source_epoch: u64,
        generation: String,
    },
    Published {
        generation: String,
    },
    Failed {
        error: String,
    },
}

#[derive(Resource, Debug)]
pub(crate) struct RuntimePublicationState {
    phase: PublicationPhase,
    next_request_id: u64,
}

impl Default for RuntimePublicationState {
    fn default() -> Self {
        Self {
            phase: PublicationPhase::Idle,
            next_request_id: 0,
        }
    }
}

impl RuntimePublicationState {
    pub(crate) fn request(&mut self, source_epoch: u64) -> bool {
        if source_epoch == 0 || self.active() {
            return false;
        }
        let request_id = self.next_request_id.wrapping_add(1).max(1);
        self.next_request_id = request_id;
        self.phase = PublicationPhase::Queued {
            request_id,
            source_epoch,
        };
        true
    }

    pub(crate) fn active(&self) -> bool {
        matches!(
            self.phase,
            PublicationPhase::Queued { .. }
                | PublicationPhase::Cooking { .. }
                | PublicationPhase::Adopting { .. }
        )
    }

    pub(crate) fn status(&self) -> String {
        match &self.phase {
            PublicationPhase::Idle => "Runtime publication idle".into(),
            PublicationPhase::Queued { .. } => "Runtime publication queued".into(),
            PublicationPhase::Cooking { .. } => {
                "Cooking and validating a complete staging generation…".into()
            }
            PublicationPhase::Adopting { generation, .. } => {
                format!("Adopting runtime generation {generation}…")
            }
            PublicationPhase::Published { generation } => {
                format!("Published and adopted runtime generation {generation}")
            }
            PublicationPhase::Failed { error } => format!("Runtime publication failed: {error}"),
        }
    }

    pub(crate) fn published_generation(&self) -> Option<&str> {
        match &self.phase {
            PublicationPhase::Published { generation } => Some(generation),
            _ => None,
        }
    }

    pub(crate) fn failure(&self) -> Option<&str> {
        match &self.phase {
            PublicationPhase::Failed { error } => Some(error),
            _ => None,
        }
    }
}

#[derive(Resource)]
struct RuntimePublicationWorker {
    requests: Sender<PublicationRequest>,
    results: Receiver<PublicationResult>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for RuntimePublicationWorker {
    fn drop(&mut self) {
        let _ = self.requests.send(PublicationRequest::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug)]
enum PublicationRequest {
    Publish { request_id: u64, source_epoch: u64 },
    Shutdown,
}

#[derive(Debug)]
struct PublicationResult {
    request_id: u64,
    source_epoch: u64,
    result: Result<String, String>,
}

fn start_publication_worker(
    mut commands: Commands,
    paths: Res<RuntimePublicationPaths>,
    terrain: Res<engine::TerrainLodPreview>,
) {
    let (request_sender, request_receiver) = bounded(PUBLICATION_CHANNEL_CAPACITY);
    let (result_sender, result_receiver) = bounded(PUBLICATION_CHANNEL_CAPACITY);
    let project_database = paths.project_database.clone();
    let runtime_database = paths.runtime_database.clone();
    let bake_root = terrain.enabled.then(|| paths.asset_root.clone());
    let worker_thread = thread::Builder::new()
        .name("yarra-runtime-publisher".into())
        .spawn(move || {
            publication_worker(
                project_database,
                runtime_database,
                bake_root,
                request_receiver,
                result_sender,
            )
        })
        .expect("failed to spawn runtime publication worker");
    commands.insert_resource(RuntimePublicationWorker {
        requests: request_sender,
        results: result_receiver,
        thread: Some(worker_thread),
    });
}

fn publication_worker(
    project_database: PathBuf,
    runtime_database: PathBuf,
    bake_root: Option<PathBuf>,
    requests: Receiver<PublicationRequest>,
    results: Sender<PublicationResult>,
) {
    while let Ok(request) = requests.recv() {
        let PublicationRequest::Publish {
            request_id,
            source_epoch,
        } = request
        else {
            return;
        };
        let result = match &bake_root {
            Some(root) => world_cook::TerrainBakeLibrary::load(root).and_then(|library| {
                world_cook::cook_project_with_materials(
                    &project_database,
                    &runtime_database,
                    &library,
                )
                .map(|report| report.manifest)
            }),
            None => world_cook::cook_project(&project_database, &runtime_database),
        }
        .map(|manifest| manifest.generation_id)
        .map_err(|error| error.to_string());
        if results
            .send(PublicationResult {
                request_id,
                source_epoch,
                result,
            })
            .is_err()
        {
            return;
        }
    }
}

fn dispatch_publication(
    worker: Option<Res<RuntimePublicationWorker>>,
    mut state: ResMut<RuntimePublicationState>,
) {
    let PublicationPhase::Queued {
        request_id,
        source_epoch,
    } = state.phase
    else {
        return;
    };
    let Some(worker) = worker else {
        return;
    };
    match worker.requests.try_send(PublicationRequest::Publish {
        request_id,
        source_epoch,
    }) {
        Ok(()) => {
            state.phase = PublicationPhase::Cooking {
                request_id,
                source_epoch,
            };
        }
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => {
            state.phase = PublicationPhase::Failed {
                error: "publication worker stopped".into(),
            };
        }
    }
}

fn receive_publication_result(
    worker: Option<Res<RuntimePublicationWorker>>,
    project: Res<ProjectEditorStore>,
    mut reload: ResMut<WorldGenerationReload>,
    mut state: ResMut<RuntimePublicationState>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut dense: ResMut<DenseDomainWorkingSets>,
    mut artifacts: ResMut<DerivedArtifactStore>,
) {
    if let Some(worker) = worker {
        loop {
            match worker.results.try_recv() {
                Ok(completed) => {
                    let PublicationPhase::Cooking {
                        request_id,
                        source_epoch,
                    } = state.phase
                    else {
                        continue;
                    };
                    if request_id != completed.request_id || source_epoch != completed.source_epoch
                    {
                        continue;
                    }
                    match completed.result {
                        Ok(_) if project.source_epoch() != source_epoch => {
                            state.phase = PublicationPhase::Failed {
                                error: format!(
                                    "source changed from epoch {source_epoch} to {} while cooking; publish again",
                                    project.source_epoch()
                                ),
                            };
                        }
                        Ok(generation) => {
                            if reload.request(generation.clone()) {
                                state.phase = PublicationPhase::Adopting {
                                    request_id,
                                    source_epoch,
                                    generation,
                                };
                            } else {
                                state.phase = PublicationPhase::Failed {
                                    error: "runtime streamer could not queue generation adoption"
                                        .into(),
                                };
                            }
                        }
                        Err(error) => {
                            state.phase = PublicationPhase::Failed { error };
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if state.active() {
                        state.phase = PublicationPhase::Failed {
                            error: "publication worker stopped".into(),
                        };
                    }
                    break;
                }
            }
        }
    }

    let PublicationPhase::Adopting {
        request_id,
        source_epoch,
        generation,
    } = &state.phase
    else {
        return;
    };
    let request_id = *request_id;
    let source_epoch = *source_epoch;
    let generation = generation.clone();
    let Some(result) = reload.take_completion() else {
        return;
    };
    match result {
        Ok(adopted) if adopted == generation => {
            objects.adopt_runtime_generation();
            dense.adopt_runtime_generation();
            artifacts.clear_failures_for_generation_adoption();
            state.phase = PublicationPhase::Published {
                generation: adopted,
            };
        }
        Ok(adopted) => {
            state.phase = PublicationPhase::Failed {
                error: format!(
                    "publication request {request_id} for source epoch {source_epoch} adopted unexpected generation {adopted}"
                ),
            };
        }
        Err(error) => {
            state.phase = PublicationPhase::Failed { error };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn publication_requests_are_single_flight_and_retryable_after_failure() {
        let mut state = RuntimePublicationState::default();
        assert!(!state.request(0));
        assert!(state.request(3));
        assert!(state.active());
        assert!(!state.request(4));
        state.phase = PublicationPhase::Failed {
            error: "validation failed".into(),
        };
        assert!(state.request(4));
    }

    #[test]
    fn failed_cook_does_not_replace_the_existing_runtime_file() {
        let directory = std::env::temp_dir().join(format!(
            "yarra-publication-failure-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let project = directory.join("missing.project.sqlite");
        let runtime = directory.join("world.runtime.sqlite");
        fs::write(&runtime, b"existing immutable generation").unwrap();
        let (requests, request_receiver) = crossbeam_channel::unbounded();
        let (results, result_receiver) = crossbeam_channel::unbounded();
        requests
            .send(PublicationRequest::Publish {
                request_id: 1,
                source_epoch: 7,
            })
            .unwrap();
        requests.send(PublicationRequest::Shutdown).unwrap();

        publication_worker(project, runtime.clone(), None, request_receiver, results);
        assert!(result_receiver.recv().unwrap().result.is_err());
        assert_eq!(
            fs::read(&runtime).unwrap(),
            b"existing immutable generation"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_requested_material_inputs_fail_publication_without_plain_fallback() {
        let directory = std::env::temp_dir().join(format!(
            "yarra-material-publication-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let runtime = directory.join("world.runtime.sqlite");
        fs::write(&runtime, b"existing immutable generation").unwrap();
        let (requests, receiver) = crossbeam_channel::unbounded();
        let (results, result_receiver) = crossbeam_channel::unbounded();
        requests
            .send(PublicationRequest::Publish {
                request_id: 1,
                source_epoch: 1,
            })
            .unwrap();
        requests.send(PublicationRequest::Shutdown).unwrap();
        publication_worker(
            directory.join("source.sqlite"),
            runtime.clone(),
            Some(directory.join("missing-assets")),
            receiver,
            results,
        );
        assert!(result_receiver.recv().unwrap().result.is_err());
        assert_eq!(
            fs::read(&runtime).unwrap(),
            b"existing immutable generation"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn publication_worker_emits_the_validated_generation_it_published() {
        let directory = std::env::temp_dir().join(format!(
            "yarra-publication-success-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let project = directory.join("project.sqlite");
        world_cook::create_demo_project(&project).unwrap();
        let runtime = directory.join("world.runtime.sqlite");
        let (requests, request_receiver) = crossbeam_channel::unbounded();
        let (results, result_receiver) = crossbeam_channel::unbounded();
        requests
            .send(PublicationRequest::Publish {
                request_id: 2,
                source_epoch: 9,
            })
            .unwrap();
        requests.send(PublicationRequest::Shutdown).unwrap();

        publication_worker(project, runtime.clone(), None, request_receiver, results);
        let published = result_receiver.recv().unwrap().result.unwrap();
        let reader = world_db::RuntimeReader::open_immutable(&runtime).unwrap();
        assert_eq!(reader.manifest().generation_id, published);
        fs::remove_dir_all(directory).unwrap();
    }
}
