//! Unsaved gameplay areas: the project's one record and the editor's version of it.
use crate::project_store::ProjectEditorStore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use world::GameplayArea;
use world_db::{GameplayAreasRecord, GameplayAreasWriteResult};

/// Dirty state kept in the recovery journal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub revision: i64,
    pub base: Vec<GameplayArea>,
    pub current: Vec<GameplayArea>,
}

#[derive(Default)]
pub(crate) struct WorkingSet {
    /// What the project holds, once it has been read.
    base: Option<GameplayAreasRecord>,
    current: Arc<[GameplayArea]>,
    /// Changes with every edit, so the journal knows to write again.
    pub revision: u64,
    pub saving: Option<u64>,
    pub error: Option<String>,
    /// What someone else saved while this editor had unsaved changes.
    pub conflict: Option<GameplayAreasRecord>,
}

impl WorkingSet {
    /// Adopts the project's record the first time it is seen.
    pub fn pin(&mut self, source: &GameplayAreasRecord) {
        if self.base.is_none() {
            self.base = Some(source.clone());
            self.current = source.areas.clone();
        }
    }
    pub fn loaded(&self) -> bool {
        self.base.is_some()
    }
    pub fn areas(&self) -> &Arc<[GameplayArea]> {
        &self.current
    }
    pub fn dirty_count(&self) -> usize {
        usize::from(self.base.as_ref().is_some_and(|b| b.areas != self.current))
    }
    pub fn blocked(&self) -> bool {
        self.base.is_none() || self.saving.is_some() || self.conflict.is_some()
    }
    /// Replaces the whole set. A set the project would refuse is refused here.
    pub fn apply(&mut self, areas: Arc<[GameplayArea]>) -> Result<(), String> {
        if self.blocked() {
            return Err("Gameplay areas are loading, saving or in conflict".into());
        }
        world::validate_gameplay_areas(&areas)?;
        self.current = areas;
        self.revision += 1;
        self.error = None;
        Ok(())
    }
    pub fn journal(&self) -> Option<Snapshot> {
        let base = self.base.as_ref()?;
        (base.areas != self.current).then(|| Snapshot {
            revision: base.revision,
            base: base.areas.to_vec(),
            current: self.current.to_vec(),
        })
    }
    /// Brings back unsaved work after a crash, unless there is newer unsaved work already.
    pub fn restore(&mut self, snapshot: Snapshot) -> bool {
        if snapshot.revision < 1
            || world::validate_gameplay_areas(&snapshot.base).is_err()
            || world::validate_gameplay_areas(&snapshot.current).is_err()
            || self.dirty_count() > 0
        {
            return false;
        }
        self.base = Some(GameplayAreasRecord {
            revision: snapshot.revision,
            areas: snapshot.base.into(),
        });
        self.current = snapshot.current.into();
        self.revision += 1;
        true
    }
    pub fn queue_save(&mut self, store: &mut ProjectEditorStore) -> bool {
        let Some(base) = self.base.as_ref().filter(|_| !self.blocked()) else {
            return false;
        };
        self.saving = store.queue_gameplay_areas(base.revision, self.current.clone());
        self.error = None;
        self.saving.is_some()
    }
    pub fn complete(&mut self, result: Result<GameplayAreasWriteResult, String>) -> bool {
        self.saving = None;
        match result {
            Ok(GameplayAreasWriteResult::Committed(record)) => {
                self.base = Some(record);
                self.revision += 1;
                self.error = None;
                true
            }
            Ok(GameplayAreasWriteResult::Conflict(actual)) => {
                self.conflict = Some(actual);
                self.error = Some(
                    "Gameplay areas changed in the project database. Resolve the conflict in \
                     the Areas window."
                        .into(),
                );
                false
            }
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }
    /// Continues from what the project holds now, keeping or dropping the local version.
    pub fn resolve_conflict(&mut self, keep_local: bool) {
        if let Some(actual) = self.conflict.take() {
            if !keep_local {
                self.current = actual.areas.clone();
            }
            self.base = Some(actual);
            self.revision += 1;
            self.error = None;
        }
    }
}
