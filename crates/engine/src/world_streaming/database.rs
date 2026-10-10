//! Bevy startup and bounded transport for immutable runtime database IO.
mod protocol;
mod reader;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
pub(super) use protocol::{
    DatabaseRequest, DatabaseResult, FarObjectPayloads, FetchedPage, RequestId,
    TerrainMaterialQuery, TerrainMaterialReply, TerrainQuery, TerrainReply,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
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

/// Owns the worker lifetime and channels. Streaming routes replies to the owning consumer,
/// which recognises its own by the id [`WorldDatabaseWorker::send`] returned.
#[derive(Resource)]
pub(super) struct WorldDatabaseWorker {
    requests: Sender<(RequestId, DatabaseRequest)>,
    results: Receiver<(RequestId, DatabaseResult)>,
    last_id: AtomicU64,
    cancel: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

/// Why a request was not queued.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum NotSent {
    /// The bounded queue is full; ask again on a later frame.
    Full,
    Stopped,
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
            last_id: AtomicU64::new(0),
            cancel: Some(cancel),
            thread: Some(thread),
        }
    }
    /// Queues a request without blocking and returns the id its reply will carry.
    pub(super) fn send(&self, request: DatabaseRequest) -> Result<RequestId, NotSent> {
        let id = self.last_id.fetch_add(1, Ordering::Relaxed) + 1;
        match self.requests.try_send((id, request)) {
            Ok(()) => Ok(id),
            Err(TrySendError::Full(_)) => Err(NotSent::Full),
            Err(TrySendError::Disconnected(_)) => Err(NotSent::Stopped),
        }
    }
    pub(super) fn try_recv(&self) -> Result<(RequestId, DatabaseResult), TryRecvError> {
        self.results.try_recv()
    }
    #[cfg(test)]
    #[allow(clippy::type_complexity)] // The worker's two channel ends, as the thread sees them.
    pub(super) fn test_channel_pair(
        request_capacity: usize,
        result_capacity: usize,
    ) -> (
        Self,
        Receiver<(RequestId, DatabaseRequest)>,
        Sender<(RequestId, DatabaseResult)>,
    ) {
        let (requests, input) = bounded(request_capacity);
        let (output, results) = bounded(result_capacity);
        (
            Self {
                requests,
                results,
                last_id: AtomicU64::new(0),
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
