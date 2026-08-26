//! Query-backed, cursor-paginated project navigation for outliner and asset browser UI.

use std::{
    path::PathBuf,
    thread::{self, JoinHandle},
};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use engine::WorldViewpoint;
use world::WorldSpaceId;
use world_db::{
    ProjectReader, SourceObjectOutlinerCursor, SourceObjectPaletteCursor,
    SourceObjectPaletteRecord, SourceObjectViewRecord,
};

const NAVIGATION_PAGE_SIZE: usize = 64;
const NAVIGATION_REQUEST_CAPACITY: usize = 2;
const MAX_CURSOR_HISTORY: usize = 32;

pub(crate) struct ProjectNavigationPlugin {
    database_path: PathBuf,
}

impl ProjectNavigationPlugin {
    pub(crate) fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
        }
    }
}

impl Plugin for ProjectNavigationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ProjectNavigationPath(self.database_path.clone()))
            .init_resource::<ProjectNavigationStore>()
            .add_systems(Startup, start_navigation_worker)
            .add_systems(
                Update,
                (
                    receive_navigation_results,
                    follow_outliner_world_space,
                    dispatch_navigation_request,
                )
                    .chain(),
            );
    }
}

#[derive(Resource)]
struct ProjectNavigationPath(PathBuf);

#[derive(Debug, Clone)]
enum NavigationRequestKind {
    Palette {
        search: String,
        cursor: Option<SourceObjectPaletteCursor>,
    },
    Outliner {
        space: WorldSpaceId,
        search: String,
        cursor: Option<SourceObjectOutlinerCursor>,
    },
}

#[derive(Debug, Clone)]
struct NavigationRequest {
    id: u64,
    kind: NavigationRequestKind,
}

enum NavigationResult {
    Opened(Result<(), String>),
    Palette {
        id: u64,
        cursor: Option<SourceObjectPaletteCursor>,
        result: Result<world_db::SourceObjectPalettePage, String>,
    },
    Outliner {
        id: u64,
        cursor: Option<SourceObjectOutlinerCursor>,
        result: Result<world_db::SourceObjectOutlinerPage, String>,
    },
}

#[derive(Resource)]
struct NavigationWorker {
    requests: Sender<NavigationRequest>,
    results: Receiver<NavigationResult>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for NavigationWorker {
    fn drop(&mut self) {
        drop(self.requests.clone());
        if let Some(thread) = self.thread.take() {
            // The application owns the final sender. Replacing it disconnects the worker without
            // an unbounded or blocking shutdown request.
            let (replacement, _) = bounded(0);
            let original = std::mem::replace(&mut self.requests, replacement);
            drop(original);
            let _ = thread.join();
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct ProjectNavigationStore {
    status: String,
    next_request_id: u64,
    latest_palette_id: u64,
    latest_outliner_id: u64,
    pending_palette: Option<NavigationRequest>,
    pending_outliner: Option<NavigationRequest>,
    in_flight: Option<u64>,
    palette_search: String,
    palette_records: Vec<SourceObjectPaletteRecord>,
    palette_cursor: Option<SourceObjectPaletteCursor>,
    palette_next: Option<SourceObjectPaletteCursor>,
    palette_history: Vec<Option<SourceObjectPaletteCursor>>,
    outliner_space: Option<WorldSpaceId>,
    outliner_search: String,
    outliner_records: Vec<SourceObjectViewRecord>,
    outliner_cursor: Option<SourceObjectOutlinerCursor>,
    outliner_next: Option<SourceObjectOutlinerCursor>,
    outliner_history: Vec<Option<SourceObjectOutlinerCursor>>,
    stale_results: u64,
}

impl ProjectNavigationStore {
    pub(crate) fn status(&self) -> &str {
        if self.status.is_empty() {
            "opening navigation queries"
        } else {
            &self.status
        }
    }

    pub(crate) fn palette_search(&self) -> &str {
        &self.palette_search
    }

    pub(crate) fn palette_records(&self) -> &[SourceObjectPaletteRecord] {
        &self.palette_records
    }

    pub(crate) fn outliner_search(&self) -> &str {
        &self.outliner_search
    }

    pub(crate) fn outliner_records(&self) -> &[SourceObjectViewRecord] {
        &self.outliner_records
    }

    pub(crate) fn search_palette(&mut self, search: String) {
        let search = search.trim().to_owned();
        if self.palette_search == search {
            return;
        }
        self.palette_search = search;
        self.palette_history.clear();
        self.queue_palette(None);
    }

    pub(crate) fn search_outliner(&mut self, search: String) {
        let search = search.trim().to_owned();
        if self.outliner_search == search {
            return;
        }
        self.outliner_search = search;
        self.outliner_history.clear();
        self.queue_outliner(None);
    }

    pub(crate) fn palette_next(&mut self) {
        let Some(next) = self.palette_next.clone() else {
            return;
        };
        push_bounded(&mut self.palette_history, self.palette_cursor.clone());
        self.queue_palette(Some(next));
    }

    pub(crate) fn palette_previous(&mut self) {
        let Some(previous) = self.palette_history.pop() else {
            return;
        };
        self.queue_palette(previous);
    }

    pub(crate) fn outliner_next(&mut self) {
        let Some(next) = self.outliner_next.clone() else {
            return;
        };
        push_bounded(&mut self.outliner_history, self.outliner_cursor.clone());
        self.queue_outliner(Some(next));
    }

    pub(crate) fn outliner_previous(&mut self) {
        let Some(previous) = self.outliner_history.pop() else {
            return;
        };
        self.queue_outliner(previous);
    }

    pub(crate) fn palette_has_previous(&self) -> bool {
        !self.palette_history.is_empty()
    }

    pub(crate) fn palette_has_next(&self) -> bool {
        self.palette_next.is_some()
    }

    pub(crate) fn outliner_has_previous(&self) -> bool {
        !self.outliner_history.is_empty()
    }

    pub(crate) fn outliner_has_next(&self) -> bool {
        self.outliner_next.is_some()
    }

    pub(crate) fn stale_results(&self) -> u64 {
        self.stale_results
    }

    fn queue_palette(&mut self, cursor: Option<SourceObjectPaletteCursor>) {
        let request = NavigationRequestKind::Palette {
            search: self.palette_search.clone(),
            cursor,
        };
        self.queue(request);
    }

    fn queue_outliner(&mut self, cursor: Option<SourceObjectOutlinerCursor>) {
        let Some(space) = self.outliner_space else {
            return;
        };
        let request = NavigationRequestKind::Outliner {
            space,
            search: self.outliner_search.clone(),
            cursor,
        };
        self.queue(request);
    }

    fn queue(&mut self, kind: NavigationRequestKind) {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let request = NavigationRequest {
            id: self.next_request_id,
            kind,
        };
        match request.kind {
            NavigationRequestKind::Palette { .. } => {
                self.latest_palette_id = request.id;
                self.pending_palette = Some(request);
            }
            NavigationRequestKind::Outliner { .. } => {
                self.latest_outliner_id = request.id;
                self.pending_outliner = Some(request);
            }
        }
    }
}

fn push_bounded<T>(history: &mut Vec<T>, value: T) {
    if history.len() == MAX_CURSOR_HISTORY {
        history.remove(0);
    }
    history.push(value);
}

fn start_navigation_worker(mut commands: Commands, path: Res<ProjectNavigationPath>) {
    let (request_sender, request_receiver) = bounded(NAVIGATION_REQUEST_CAPACITY);
    let (result_sender, result_receiver) = bounded(NAVIGATION_REQUEST_CAPACITY + 1);
    let database_path = path.0.clone();
    let worker_thread = thread::Builder::new()
        .name("yarra-project-navigation".into())
        .spawn(move || navigation_worker(database_path, request_receiver, result_sender))
        .expect("failed to spawn project navigation worker");
    commands.insert_resource(NavigationWorker {
        requests: request_sender,
        results: result_receiver,
        thread: Some(worker_thread),
    });
}

fn navigation_worker(
    database_path: PathBuf,
    requests: Receiver<NavigationRequest>,
    results: Sender<NavigationResult>,
) {
    let reader = match ProjectReader::open_read_only(&database_path) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = results.send(NavigationResult::Opened(Err(error.to_string())));
            return;
        }
    };
    if results.try_send(NavigationResult::Opened(Ok(()))).is_err() {
        return;
    }
    while let Ok(request) = requests.recv() {
        let result = match request.kind {
            NavigationRequestKind::Palette { search, cursor } => NavigationResult::Palette {
                id: request.id,
                cursor: cursor.clone(),
                result: reader
                    .read_object_palette_page(&search, cursor.as_ref(), NAVIGATION_PAGE_SIZE)
                    .map_err(|error| error.to_string()),
            },
            NavigationRequestKind::Outliner {
                space,
                search,
                cursor,
            } => NavigationResult::Outliner {
                id: request.id,
                cursor: cursor.clone(),
                result: reader
                    .read_object_outliner_page(
                        space,
                        &search,
                        cursor.as_ref(),
                        NAVIGATION_PAGE_SIZE,
                    )
                    .map_err(|error| error.to_string()),
            },
        };
        if matches!(results.try_send(result), Err(TrySendError::Disconnected(_))) {
            return;
        }
    }
}

fn receive_navigation_results(
    worker: Option<Res<NavigationWorker>>,
    mut store: ResMut<ProjectNavigationStore>,
) {
    let Some(worker) = worker else {
        return;
    };
    loop {
        match worker.results.try_recv() {
            Ok(NavigationResult::Opened(Ok(()))) => {
                store.status = "navigation ready".into();
                store.queue_palette(None);
            }
            Ok(NavigationResult::Opened(Err(error))) => {
                store.status = format!("navigation failed: {error}");
            }
            Ok(NavigationResult::Palette { id, cursor, result }) => {
                if store.in_flight != Some(id) || id != store.latest_palette_id {
                    store.stale_results = store.stale_results.saturating_add(1);
                    if store.in_flight == Some(id) {
                        store.in_flight = None;
                    }
                    continue;
                }
                store.in_flight = None;
                match result {
                    Ok(page) => {
                        store.palette_cursor = cursor;
                        store.palette_records = page.records;
                        store.palette_next = page.next_cursor;
                    }
                    Err(error) => store.status = format!("asset query failed: {error}"),
                }
            }
            Ok(NavigationResult::Outliner { id, cursor, result }) => {
                if store.in_flight != Some(id) || id != store.latest_outliner_id {
                    store.stale_results = store.stale_results.saturating_add(1);
                    if store.in_flight == Some(id) {
                        store.in_flight = None;
                    }
                    continue;
                }
                store.in_flight = None;
                match result {
                    Ok(page) => {
                        store.outliner_cursor = cursor;
                        store.outliner_records = page.records;
                        store.outliner_next = page.next_cursor;
                    }
                    Err(error) => store.status = format!("outliner query failed: {error}"),
                }
            }
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                store.status = "navigation worker stopped".into();
                return;
            }
        }
    }
}

fn follow_outliner_world_space(
    viewpoint: Res<WorldViewpoint>,
    mut store: ResMut<ProjectNavigationStore>,
) {
    let space = viewpoint.position().map(|position| position.space);
    if store.outliner_space == space {
        return;
    }
    store.outliner_space = space;
    store.outliner_history.clear();
    store.outliner_records.clear();
    store.outliner_next = None;
    store.queue_outliner(None);
}

fn dispatch_navigation_request(
    worker: Option<Res<NavigationWorker>>,
    mut store: ResMut<ProjectNavigationStore>,
) {
    if store.in_flight.is_some() {
        return;
    }
    let Some(worker) = worker else {
        return;
    };
    let Some(request) = store
        .pending_palette
        .take()
        .or_else(|| store.pending_outliner.take())
    else {
        return;
    };
    match worker.requests.try_send(request.clone()) {
        Ok(()) => store.in_flight = Some(request.id),
        Err(TrySendError::Full(request)) => match request.kind {
            NavigationRequestKind::Palette { .. } => store.pending_palette = Some(request),
            NavigationRequestKind::Outliner { .. } => store.pending_outliner = Some(request),
        },
        Err(TrySendError::Disconnected(_)) => {
            store.status = "navigation worker stopped".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_history_is_bounded() {
        let mut history = Vec::new();
        for value in 0..(MAX_CURSOR_HISTORY + 5) {
            push_bounded(&mut history, value);
        }
        assert_eq!(history.len(), MAX_CURSOR_HISTORY);
        assert_eq!(history[0], 5);
    }

    #[test]
    fn newer_search_replaces_obsolete_pending_query() {
        let mut store = ProjectNavigationStore::default();
        store.search_palette("tree".into());
        let first = store.pending_palette.as_ref().unwrap().id;
        store.search_palette("rock".into());
        assert!(store.pending_palette.as_ref().unwrap().id > first);
        let NavigationRequestKind::Palette { search, .. } =
            &store.pending_palette.as_ref().unwrap().kind
        else {
            panic!("palette search should queue a palette query");
        };
        assert_eq!(search, "rock");
    }
}
