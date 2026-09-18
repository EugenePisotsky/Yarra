use super::*;
use crate::project_store::{ProjectDatabasePath, ProjectQueryWindow};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{collections::BTreeSet, thread};
use world_db::{EnvironmentLayerPresence, ProjectReader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Query {
    window: ProjectQueryWindow,
    epoch: u64,
    definition_revision: u64,
}
struct Worker {
    requests: Option<Sender<Query>>,
    results: Receiver<(Query, Result<EnvironmentLayerPresence, String>)>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Worker {
    fn start(path: std::path::PathBuf) -> Result<Self, String> {
        let (requests, input) = bounded::<Query>(1);
        let (output, results) = bounded(1);
        let thread = thread::Builder::new()
            .name("layer-browser".into())
            .spawn(move || {
                let reader = ProjectReader::open_read_only(&path).map_err(|e| e.to_string());
                while let Ok(q) = input.recv() {
                    let result = reader.as_ref().map_err(Clone::clone).and_then(|reader| {
                        reader
                            .read_environment_layer_presence(
                                q.window.space,
                                q.window.minimum,
                                q.window.maximum,
                                world_db::MAX_ENVIRONMENT_PRESENCE_ROWS,
                            )
                            .map_err(|e| e.to_string())
                    });
                    if output.send((q, result)).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            requests: Some(requests),
            results,
            thread: Some(thread),
        })
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
type MembershipCache = (Query, u64, u64, BTreeSet<LayerId>);
#[derive(Resource, Default)]
pub(crate) struct EnvironmentLayerBrowser {
    cached: Option<MembershipCache>,
    worker: Option<Worker>,
    desired: Option<Query>,
    submitted: Option<Query>,
    accepted: Option<(Query, EnvironmentLayerPresence)>,
    in_flight: bool,
    pub(crate) all: bool,
    pub(crate) search: String,
    error: Option<String>,
}
fn inside(cell: CellCoord, window: ProjectQueryWindow) -> bool {
    cell.x >= window.minimum.x
        && cell.x <= window.maximum.x
        && cell.z >= window.minimum.z
        && cell.z <= window.maximum.z
}
/// Replace complete touched-cell membership, including erasures, before collecting layer IDs.
fn membership<R: std::borrow::Borrow<SourceEnvironmentCellRecord>>(
    window: ProjectQueryWindow,
    saved: impl IntoIterator<Item = (CellCoord, LayerId)>,
    local: impl IntoIterator<Item = R>,
) -> BTreeSet<LayerId> {
    let mut entries = saved.into_iter().collect::<BTreeSet<_>>();
    for record in local {
        let record = record.borrow();
        if record.space != window.space || !inside(record.cell, window) {
            continue;
        }
        entries.retain(|(cell, _)| *cell != record.cell);
        entries.extend(
            record
                .tiles
                .iter()
                .filter(|t| t.samples.iter().any(|v| *v != 0))
                .map(|t| (record.cell, t.layer)),
        );
    }
    entries.into_iter().map(|(_, layer)| layer).collect()
}
impl EnvironmentLayerBrowser {
    pub(crate) fn refresh(&mut self) {
        self.submitted = None;
        self.error = None;
    }
    pub(crate) fn includes(
        &self,
        layer: &environment::Layer,
        selected: Option<LayerId>,
        nearby: &BTreeSet<LayerId>,
    ) -> bool {
        selected == Some(layer.id)
            || ((self.all || nearby.contains(&layer.id))
                && layer
                    .name
                    .to_lowercase()
                    .contains(&self.search.to_lowercase()))
    }

    pub(crate) fn nearby(
        &mut self,
        dense: &DenseDomainWorkingSets,
        space: WorldSpaceId,
    ) -> BTreeSet<LayerId> {
        let Some(q) = self.desired.filter(|q| q.window.space == space) else {
            return BTreeSet::new();
        };
        if let Some((key, edits, definitions, layers)) = &self.cached
            && *key == q
            && *edits == dense.edit_revision()
            && *definitions == dense.definition_edit_revision()
        {
            return layers.clone();
        }
        let saved = self
            .accepted
            .as_ref()
            .filter(|(key, _)| *key == q)
            .into_iter()
            .flat_map(|(_, p)| p.entries.iter().copied());
        let local = dense.dirty_environment_cells();
        let mut result = membership(q.window, saved, local);
        for d in dense
            .dirty_definition_snapshots()
            .iter()
            .filter(|d| d.current.space == space)
        {
            result.extend(
                d.current
                    .layers
                    .iter()
                    .filter(|l| !d.base.layers.iter().any(|old| old.id == l.id))
                    .map(|l| l.id),
            );
        }
        self.cached = Some((
            q,
            dense.edit_revision(),
            dense.definition_edit_revision(),
            result.clone(),
        ));
        result
    }
    pub(crate) fn status(&self) -> String {
        if let Some(e) = &self.error {
            return format!("Nearby layers unavailable: {e}");
        }
        if let Some((_, p)) = &self.accepted {
            if p.truncated {
                return "Partial nearby results · use All to find other layers".into();
            }
            return "Nearby · 5 × 5 cells around the editing focus".into();
        }
        "Loading nearby layers… Selected and new layers stay visible.".into()
    }
}
pub(super) fn update(
    mut browser: ResMut<EnvironmentLayerBrowser>,
    project: Res<ProjectEditorStore>,
    path: Res<ProjectDatabasePath>,
    workspace: Res<State<EditorWorkspace>>,
    tools: Res<EditorToolRegistry>,
) {
    let desired = if *workspace.get() == EditorWorkspace::World
        && tools
            .active(EditorWorkspace::World)
            .is_some_and(|t| t.id == ENVIRONMENT_TOOL.id)
    {
        project.desired_window().and_then(|window| {
            project
                .environments()
                .iter()
                .find(|d| d.space == window.space)
                .map(|d| Query {
                    window,
                    epoch: project.source_epoch(),
                    definition_revision: d.revision,
                })
        })
    } else {
        None
    };
    if browser.desired != desired {
        browser.desired = desired;
        browser.accepted = None;
        browser.cached = None;
        browser.error = None;
        browser.submitted = None;
    }
    let completion = browser
        .worker
        .as_ref()
        .and_then(|w| w.results.try_recv().ok());
    if let Some((q, result)) = completion {
        browser.in_flight = false;
        if Some(q) == browser.desired {
            browser.cached = None;
            match result {
                Ok(p) if p.definition_revision == q.definition_revision => {
                    browser.accepted = Some((q, p))
                }
                Ok(_) => {
                    browser.error = Some(
                        "Source changed during discovery; move the editing focus to refresh".into(),
                    )
                }
                Err(e) => browser.error = Some(e),
            }
        }
    }
    let Some(q) = browser.desired else {
        return;
    };
    if browser.in_flight || browser.submitted == Some(q) {
        return;
    }
    if browser.worker.is_none() {
        match Worker::start(path.0.clone()) {
            Ok(w) => browser.worker = Some(w),
            Err(e) => {
                browser.error = Some(e);
                browser.submitted = Some(q);
                return;
            }
        }
    }
    if browser
        .worker
        .as_ref()
        .unwrap()
        .requests
        .as_ref()
        .unwrap()
        .try_send(q)
        .is_ok()
    {
        browser.submitted = Some(q);
        browser.in_flight = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn window() -> ProjectQueryWindow {
        ProjectQueryWindow {
            space: WorldSpaceId(1),
            minimum: CellCoord { x: -2, z: -2 },
            maximum: CellCoord { x: 2, z: 2 },
        }
    }
    fn record(cell: CellCoord, layer: LayerId, value: u8) -> SourceEnvironmentCellRecord {
        SourceEnvironmentCellRecord {
            space: WorldSpaceId(1),
            cell,
            source_revision: 1,
            definition_revision: 1,
            tiles: vec![environment::CoverageTile {
                layer,
                samples: vec![value; 9],
            }],
        }
    }
    #[test]
    fn pending_paint_replaces_saved_membership_including_erasure_and_ignores_other_windows() {
        let a = LayerId([1; 16]);
        let b = LayerId([2; 16]);
        let other = LayerId([3; 16]);
        let result = membership(
            window(),
            [(CellCoord::ZERO, a)],
            [
                record(CellCoord::ZERO, a, 0),
                record(CellCoord { x: 1, z: 0 }, b, 255),
                record(CellCoord { x: 20, z: 0 }, other, 255),
            ],
        );
        assert_eq!(result, BTreeSet::from([b]));
        let result = membership(
            window(),
            [(CellCoord::ZERO, a), (CellCoord { x: 1, z: 0 }, a)],
            [record(CellCoord::ZERO, a, 0)],
        );
        assert_eq!(result, BTreeSet::from([a]));
    }
    #[test]
    fn selection_stays_visible_outside_the_window_and_search_includes_disabled_layers() {
        let layer = environment::Layer {
            id: LayerId([1; 16]),
            revision: 1,
            name: "Forest".into(),
            preset: environment::PresetId([1; 16]),
            overrides: vec![],
            order: 0,
            seed: 1,
            enabled: false,
            opacity: 1.0,
        };
        let mut browser = EnvironmentLayerBrowser {
            search: "not a match".into(),
            ..Default::default()
        };
        assert!(browser.includes(&layer, Some(layer.id), &BTreeSet::new()));
        assert!(!browser.includes(&layer, None, &BTreeSet::from([layer.id])));
        browser.search = "FORE".into();
        assert!(browser.includes(&layer, None, &BTreeSet::from([layer.id])));
        assert!(!browser.includes(&layer, None, &BTreeSet::new()));
        browser.all = true;
        assert!(browser.includes(&layer, None, &BTreeSet::new()));
    }
}
