//! Background threads that own a database, a file or a compiler for the editor.
//!
//! The editor only talks to them through bounded channels and never blocks a frame on them.

use std::path::PathBuf;
use std::thread::{self, JoinHandle};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use world_db::ProjectReader;

/// A named thread fed through bounded request and result channels.
///
/// Dropping it disconnects the requests: the thread answers what is already queued, its receive
/// loop ends, and the drop joins it unless the worker was [detached](Self::detached).
#[derive(Resource)]
pub(crate) struct Worker<Request: Send + 'static, Response: Send + 'static> {
    requests: Option<Sender<Request>>,
    results: Receiver<Response>,
    thread: Option<JoinHandle<()>>,
}

impl<Request: Send + 'static, Response: Send + 'static> Worker<Request, Response> {
    /// Starts `run` on its own thread with channels of the given capacities.
    pub(crate) fn spawn(
        name: &str,
        request_capacity: usize,
        result_capacity: usize,
        run: impl FnOnce(Receiver<Request>, Sender<Response>) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (requests, input) = bounded(request_capacity);
        let (output, results) = bounded(result_capacity);
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || run(input, output))?;
        Ok(Self {
            requests: Some(requests),
            results,
            thread: Some(thread),
        })
    }

    /// Dropping the worker no longer waits for its thread, which must stop on its own once the
    /// requests disconnect (a long job checks a cancellation token instead).
    pub(crate) fn detached(mut self) -> Self {
        self.thread = None;
        self
    }

    pub(crate) fn try_send(&self, request: Request) -> Result<(), TrySendError<Request>> {
        self.sender().try_send(request)
    }

    #[cfg(test)]
    pub(crate) fn send(
        &self,
        request: Request,
    ) -> Result<(), crossbeam_channel::SendError<Request>> {
        self.sender().send(request)
    }

    pub(crate) fn try_recv(&self) -> Result<Response, TryRecvError> {
        self.results.try_recv()
    }

    #[cfg(test)]
    pub(crate) fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Response, crossbeam_channel::RecvTimeoutError> {
        self.results.recv_timeout(timeout)
    }

    fn sender(&self) -> &Sender<Request> {
        self.requests
            .as_ref()
            .expect("requests stay connected until drop")
    }
}

impl<Query: Send + 'static, Answer: Send + 'static> Worker<Query, (Query, Result<Answer, String>)> {
    /// One query at a time against a read-only project database: each answer comes back with its
    /// query. A database that cannot be opened answers every query with that error.
    pub(crate) fn project_reader(
        name: &str,
        path: PathBuf,
        read: impl Fn(&ProjectReader, &Query) -> Result<Answer, String> + Send + 'static,
    ) -> Result<Self, String> {
        Self::spawn(name, 1, 1, move |queries, answers| {
            let reader = ProjectReader::open_read_only(&path).map_err(|e| e.to_string());
            while let Ok(query) = queries.recv() {
                let answer = reader
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|reader| read(reader, &query));
                if answers.send((query, answer)).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())
    }
}

impl<Request: Send + 'static, Response: Send + 'static> Drop for Worker<Request, Response> {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
