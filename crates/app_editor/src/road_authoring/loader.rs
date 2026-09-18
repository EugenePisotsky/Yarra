use super::*;
use crate::project_store::ProjectDatabasePath;
use crossbeam_channel::{Receiver, Sender, bounded};
use std::thread;
#[derive(Clone, PartialEq)]
struct Key {
    space: WorldSpaceId,
    bounds: RoadCellBounds,
    epoch: u64,
    pins: Vec<RoadRecordKey>,
    refresh: u64,
}

impl Key {
    /// A shifted discovery window does not invalidate already loaded controls.
    /// Editing still needs the current source epoch, pinned dependencies and focus cell.
    fn covers(&self, desired: &Self, focus: CellCoord) -> bool {
        self.space == desired.space
            && self.epoch == desired.epoch
            && self.refresh == desired.refresh
            && self.bounds.contains(focus)
            && desired
                .pins
                .iter()
                .all(|pin| self.pins.binary_search(pin).is_ok())
    }
}
struct Worker {
    send: Option<Sender<Key>>,
    receive: Receiver<(Key, Result<world_db::RoadAuthoringSnapshot, String>)>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.send.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
impl Worker {
    fn new(path: std::path::PathBuf) -> Result<Self, String> {
        let (send, requests) = bounded::<Key>(1);
        let (results, receive) = bounded(1);
        let thread = thread::Builder::new()
            .name("road-authoring".into())
            .spawn(move || {
                let reader =
                    world_db::ProjectReader::open_read_only(&path).map_err(|e| e.to_string());
                while let Ok(key) = requests.recv() {
                    let result = reader.as_ref().map_err(Clone::clone).and_then(|r| {
                        r.read_road_authoring_snapshot(key.space, key.bounds, &key.pins)
                            .map_err(|e| e.to_string())
                    });
                    if results.send((key, result)).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            send: Some(send),
            receive,
            thread: Some(thread),
        })
    }
}
#[derive(Resource, Default)]
pub(super) struct RoadLoader {
    worker: Option<Worker>,
    in_flight: bool,
    accepted: Option<Key>,
    submitted: Option<Key>,
    desired: Option<Key>,
}
#[allow(clippy::too_many_arguments)] // Bevy system parameters.
pub(super) fn update(
    path: Res<ProjectDatabasePath>,
    project: Res<ProjectEditorStore>,
    view: Res<engine::WorldViewpoint>,
    history: Res<EditorHistory>,
    mut dense: ResMut<DenseDomainWorkingSets>,
    mut state: ResMut<RoadToolState>,
    tools: Res<EditorToolRegistry>,
    workspace: Res<State<EditorWorkspace>>,
    mut loader: ResMut<RoadLoader>,
) {
    let needed = *workspace.get() == EditorWorkspace::Presets
        || (*workspace.get() == EditorWorkspace::World
            && tools
                .active(EditorWorkspace::World)
                .is_some_and(|t| t.id == ROAD_TOOL.id));
    if !needed || dense.gesture_active || dense.saving() {
        return;
    }
    let Some(position) = view.position() else {
        return;
    };
    if dense.definition(position.space).is_none() {
        return;
    }
    let center = state
        .hover
        .filter(|_| state.space == Some(position.space))
        .map_or(position.cell, |p| p.cell);
    let bounds = RoadCellBounds {
        minimum: CellCoord {
            x: center.x.saturating_sub(2),
            z: center.z.saturating_sub(2),
        },
        maximum: CellCoord {
            x: center.x.saturating_add(2),
            z: center.z.saturating_add(2),
        },
    };
    let mut pins = history.road_keys();
    if let Some(id) = state.road {
        pins.insert(RoadRecordKey::Road(id));
    }
    if let Some(id) = state.knot {
        pins.insert(RoadRecordKey::Knot(id));
    }
    if let Some(id) = state.span {
        pins.insert(RoadRecordKey::Span(id));
    }
    let pins = dense.roads.pins(&pins);
    let key = Key {
        space: position.space,
        bounds,
        epoch: project.source_epoch(),
        pins: pins.iter().copied().collect(),
        refresh: state.refresh,
    };
    if loader.desired.as_ref() != Some(&key) {
        loader.desired = Some(key.clone());
        loader.submitted = None;
        state.load_error = None;
    }
    if let Some((finished, result)) = loader
        .worker
        .as_ref()
        .and_then(|w| w.receive.try_recv().ok())
    {
        loader.in_flight = false;
        if finished == key {
            match result {
                Ok(snapshot) => {
                    dense.roads.reconcile(snapshot, &pins);
                    state.load_error = None;
                    state.loaded_space = Some(finished.space);
                    loader.accepted = Some(finished);
                }
                Err(e) => state.load_error = Some(e),
            }
        }
    }
    state.ready = loader
        .accepted
        .as_ref()
        .is_some_and(|loaded| loaded.covers(&key, center))
        && state.load_error.is_none();
    state.ready_window = loader
        .accepted
        .as_ref()
        .filter(|_| state.ready)
        .map(|loaded| (loaded.space, loaded.bounds));
    if loader.in_flight
        || loader.accepted.as_ref() == Some(&key)
        || loader.submitted.as_ref() == Some(&key)
    {
        return;
    }
    if loader.worker.is_none() {
        match Worker::new(path.0.clone()) {
            Ok(w) => loader.worker = Some(w),
            Err(e) => {
                state.load_error = Some(e);
                return;
            }
        }
    }
    if loader
        .worker
        .as_ref()
        .unwrap()
        .send
        .as_ref()
        .unwrap()
        .try_send(key.clone())
        .is_ok()
    {
        loader.in_flight = true;
        loader.submitted = Some(key);
        state.load_error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Key {
        Key {
            space: WorldSpaceId(1),
            bounds: RoadCellBounds {
                minimum: CellCoord { x: -2, z: -2 },
                maximum: CellCoord { x: 2, z: 2 },
            },
            epoch: 1,
            pins: vec![RoadRecordKey::Road(RoadId([1; 16]))],
            refresh: 0,
        }
    }

    #[test]
    fn moving_inside_loaded_area_keeps_controls_ready_without_claiming_unloaded_dependencies() {
        let loaded = key();
        let mut next = loaded.clone();
        next.bounds.minimum.x += 1;
        next.bounds.maximum.x += 1;
        assert!(loaded.covers(&next, CellCoord { x: 1, z: 0 }));
        assert!(!loaded.covers(&next, CellCoord { x: 3, z: 0 }));
        next.pins.push(RoadRecordKey::Knot(RoadKnotId([2; 16])));
        assert!(!loaded.covers(&next, CellCoord::ZERO));
        next = loaded.clone();
        next.epoch += 1;
        assert!(!loaded.covers(&next, CellCoord::ZERO));
        next = loaded.clone();
        next.refresh += 1;
        assert!(!loaded.covers(&next, CellCoord::ZERO));
        next = loaded.clone();
        next.space = WorldSpaceId(2);
        assert!(!loaded.covers(&next, CellCoord::ZERO));
    }

    #[test]
    fn road_status_is_stable_and_click_checks_actual_cell_even_before_next_loader_frame() {
        let loaded = key();
        let mut state = RoadToolState {
            ready: true,
            loaded_space: Some(loaded.space),
            ready_window: Some((loaded.space, loaded.bounds)),
            ..Default::default()
        };
        let label = state.discovery_status(loaded.space);
        assert!(state.can_create_at(loaded.space, CellCoord::ZERO));
        assert!(!state.can_create_at(loaded.space, CellCoord { x: 3, z: 0 }));
        assert!(!state.can_create_at(WorldSpaceId(2), CellCoord::ZERO));
        state.ready = false;
        assert_eq!(state.discovery_status(loaded.space), label);
        assert!(!state.can_create_at(loaded.space, CellCoord::ZERO));
        assert!(
            state
                .discovery_status(WorldSpaceId(2))
                .starts_with("Loading")
        );
    }
}
