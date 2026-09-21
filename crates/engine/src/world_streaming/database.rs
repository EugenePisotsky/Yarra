//! Bevy startup and bounded transport for immutable runtime database IO.
mod protocol;
mod reader;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
pub(super) use protocol::{
    DatabaseRequest, DatabaseResult, FetchedPage, TerrainMaterialQuery, TerrainMaterialReply,
    TerrainQuery, TerrainReply,
};
use std::{
    path::PathBuf,
    thread::{self, JoinHandle},
};

const REQUEST_CAPACITY: usize = 16;
const RESULT_CAPACITY: usize = 32;

pub(super) struct WorldDatabasePlugin(pub PathBuf);
impl Plugin for WorldDatabasePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(WorldDatabasePath(self.0.clone()))
            .add_systems(Startup, start_database_worker);
    }
}
#[derive(Resource)]
struct WorldDatabasePath(PathBuf);

/// Owns the worker lifetime and channels. Streaming routes replies to the owning consumer.
#[derive(Resource)]
pub(super) struct WorldDatabaseWorker {
    requests: Sender<DatabaseRequest>,
    results: Receiver<DatabaseResult>,
    cancel: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}
impl WorldDatabaseWorker {
    fn spawn(path: PathBuf) -> Self {
        Self::spawn_with_capacity(path, REQUEST_CAPACITY, RESULT_CAPACITY)
    }
    fn spawn_with_capacity(path: PathBuf, requests: usize, results: usize) -> Self {
        let (requests, input) = bounded(requests);
        let (output, results) = bounded(results);
        let (cancel, cancelled) = bounded(0);
        let thread = thread::Builder::new()
            .name("yarra-world-db".into())
            .spawn(move || reader::run(path, input, output, cancelled))
            .expect("failed to spawn the world database worker");
        Self {
            requests,
            results,
            cancel: Some(cancel),
            thread: Some(thread),
        }
    }
    pub(super) fn try_send(
        &self,
        request: DatabaseRequest,
    ) -> Result<(), TrySendError<DatabaseRequest>> {
        self.requests.try_send(request)
    }
    pub(super) fn try_recv(&self) -> Result<DatabaseResult, TryRecvError> {
        self.results.try_recv()
    }
    #[cfg(test)]
    pub(super) fn test_channel_pair(
        request_capacity: usize,
        result_capacity: usize,
    ) -> (Self, Receiver<DatabaseRequest>, Sender<DatabaseResult>) {
        let (requests, input) = bounded(request_capacity);
        let (output, results) = bounded(result_capacity);
        (
            Self {
                requests,
                results,
                cancel: None,
                thread: None,
            },
            input,
            output,
        )
    }
}
impl Drop for WorldDatabaseWorker {
    fn drop(&mut self) {
        // Disconnect first: wake either channel wait without enqueuing a shutdown command.
        // A synchronous SQLite read must finish, but queued work/replies cannot prevent exit.
        drop(self.cancel.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn start_database_worker(mut commands: Commands, path: Res<WorldDatabasePath>) {
    commands.insert_resource(WorldDatabaseWorker::spawn(path.0.clone()));
}

#[cfg(test)]
mod tests;
