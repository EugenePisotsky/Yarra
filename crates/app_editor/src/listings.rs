//! Query-backed, cursor-paginated listings of placed objects (the Navigator's outliner) and
//! placeable assets (the asset palette).

use std::path::PathBuf;

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use engine::WorldViewpoint;
use world::WorldSpaceId;
use world_db::{
    ProjectReader, SourceObjectOutlinerCursor, SourceObjectPaletteCursor,
    SourceObjectPaletteRecord, SourceObjectViewRecord,
};

const LISTING_PAGE_SIZE: usize = 64;
const LISTING_REQUEST_CAPACITY: usize = 2;
const MAX_CURSOR_HISTORY: usize = 32;

pub(crate) struct ProjectListingsPlugin {
    database_path: PathBuf,
}

impl ProjectListingsPlugin {
    pub(crate) fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
        }
    }
}

impl Plugin for ProjectListingsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ProjectListingsPath(self.database_path.clone()))
            .init_resource::<ProjectListings>()
            .add_systems(Startup, start_listing_worker)
            .add_systems(
                Update,
                (
                    receive_listing_results,
                    follow_outliner_world_space,
                    dispatch_listing_request,
                )
                    .chain(),
            );
    }
}

#[derive(Resource)]
struct ProjectListingsPath(PathBuf);

#[derive(Debug, Clone)]
enum ListingRequestKind {
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
struct ListingRequest {
    id: u64,
    kind: ListingRequestKind,
}

enum ListingResult {
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

type ListingWorker = crate::worker::Worker<ListingRequest, ListingResult>;

#[derive(Resource, Default)]
pub(crate) struct ProjectListings {
    status: String,
    next_request_id: u64,
    latest_palette_id: u64,
    latest_outliner_id: u64,
    pending_palette: Option<ListingRequest>,
    pending_outliner: Option<ListingRequest>,
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

impl ProjectListings {
    pub(crate) fn status(&self) -> &str {
        if self.status.is_empty() {
            "opening listings queries"
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
        let request = ListingRequestKind::Palette {
            search: self.palette_search.clone(),
            cursor,
        };
        self.queue(request);
    }

    fn queue_outliner(&mut self, cursor: Option<SourceObjectOutlinerCursor>) {
        let Some(space) = self.outliner_space else {
            return;
        };
        let request = ListingRequestKind::Outliner {
            space,
            search: self.outliner_search.clone(),
            cursor,
        };
        self.queue(request);
    }

    fn queue(&mut self, kind: ListingRequestKind) {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let request = ListingRequest {
            id: self.next_request_id,
            kind,
        };
        match request.kind {
            ListingRequestKind::Palette { .. } => {
                self.latest_palette_id = request.id;
                self.pending_palette = Some(request);
            }
            ListingRequestKind::Outliner { .. } => {
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

fn start_listing_worker(mut commands: Commands, path: Res<ProjectListingsPath>) {
    let database_path = path.0.clone();
    let worker = ListingWorker::spawn(
        "yarra-project-listings",
        LISTING_REQUEST_CAPACITY,
        LISTING_REQUEST_CAPACITY + 1,
        move |requests, results| listing_worker(database_path, requests, results),
    )
    .expect("failed to spawn project listings worker");
    commands.insert_resource(worker);
}

fn listing_worker(
    database_path: PathBuf,
    requests: Receiver<ListingRequest>,
    results: Sender<ListingResult>,
) {
    let reader = match ProjectReader::open_read_only(&database_path) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = results.send(ListingResult::Opened(Err(error.to_string())));
            return;
        }
    };
    if results.try_send(ListingResult::Opened(Ok(()))).is_err() {
        return;
    }
    while let Ok(request) = requests.recv() {
        let result = match request.kind {
            ListingRequestKind::Palette { search, cursor } => ListingResult::Palette {
                id: request.id,
                cursor: cursor.clone(),
                result: reader
                    .read_object_palette_page(&search, cursor.as_ref(), LISTING_PAGE_SIZE)
                    .map_err(|error| error.to_string()),
            },
            ListingRequestKind::Outliner {
                space,
                search,
                cursor,
            } => ListingResult::Outliner {
                id: request.id,
                cursor: cursor.clone(),
                result: reader
                    .read_object_outliner_page(space, &search, cursor.as_ref(), LISTING_PAGE_SIZE)
                    .map_err(|error| error.to_string()),
            },
        };
        if matches!(results.try_send(result), Err(TrySendError::Disconnected(_))) {
            return;
        }
    }
}

fn receive_listing_results(worker: Option<Res<ListingWorker>>, mut store: ResMut<ProjectListings>) {
    let Some(worker) = worker else {
        return;
    };
    loop {
        match worker.try_recv() {
            Ok(ListingResult::Opened(Ok(()))) => {
                store.status = "listings ready".into();
                store.queue_palette(None);
            }
            Ok(ListingResult::Opened(Err(error))) => {
                store.status = format!("listings failed: {error}");
            }
            Ok(ListingResult::Palette { id, cursor, result }) => {
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
            Ok(ListingResult::Outliner { id, cursor, result }) => {
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
                store.status = "listings worker stopped".into();
                return;
            }
        }
    }
}

fn follow_outliner_world_space(viewpoint: Res<WorldViewpoint>, mut store: ResMut<ProjectListings>) {
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

fn dispatch_listing_request(
    worker: Option<Res<ListingWorker>>,
    mut store: ResMut<ProjectListings>,
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
    match worker.try_send(request.clone()) {
        Ok(()) => store.in_flight = Some(request.id),
        Err(TrySendError::Full(request)) => match request.kind {
            ListingRequestKind::Palette { .. } => store.pending_palette = Some(request),
            ListingRequestKind::Outliner { .. } => store.pending_outliner = Some(request),
        },
        Err(TrySendError::Disconnected(_)) => {
            store.status = "listings worker stopped".into();
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
        let mut store = ProjectListings::default();
        store.search_palette("tree".into());
        let first = store.pending_palette.as_ref().unwrap().id;
        store.search_palette("rock".into());
        assert!(store.pending_palette.as_ref().unwrap().id > first);
        let ListingRequestKind::Palette { search, .. } =
            &store.pending_palette.as_ref().unwrap().kind
        else {
            panic!("palette search should queue a palette query");
        };
        assert_eq!(search, "rock");
    }
}
