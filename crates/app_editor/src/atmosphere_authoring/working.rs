use crate::project_store::ProjectEditorStore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use world::{WorldSpaceId, atmosphere::AtmosphereProfile};
use world_db::{AtmosphereWrite, AtmosphereWriteResult, WorldSpaceRecord};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub space: WorldSpaceId,
    pub revision: i64,
    pub base: AtmosphereProfile,
    pub current: AtmosphereProfile,
}
#[derive(Default)]
pub(crate) struct WorkingSet {
    pub entries: BTreeMap<WorldSpaceId, Snapshot>,
    pub revision: u64,
    pub saving: Option<u64>,
    pub error: Option<String>,
    pub conflict: Option<(WorldSpaceId, i64, AtmosphereProfile)>,
    pub gesture: Option<(WorldSpaceId, AtmosphereProfile)>,
}
impl WorkingSet {
    pub fn pin(&mut self, source: &WorldSpaceRecord) {
        self.entries.entry(source.id).or_insert_with(|| Snapshot {
            space: source.id,
            revision: source.atmosphere_revision,
            base: source.atmosphere.clone(),
            current: source.atmosphere.clone(),
        });
    }
    pub fn dirty_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.base != e.current)
            .count()
    }
    pub fn blocked(&self) -> bool {
        self.saving.is_some() || self.gesture.is_some() || self.conflict.is_some()
    }
    pub fn apply(&mut self, space: WorldSpaceId, profile: &AtmosphereProfile) -> bool {
        if self.blocked() || profile.validate().is_err() {
            return false;
        }
        let Some(entry) = self.entries.get_mut(&space) else {
            return false;
        };
        entry.current = profile.clone();
        self.revision += 1;
        self.error = None;
        true
    }
    pub fn finish_gesture(
        &mut self,
    ) -> Option<(WorldSpaceId, AtmosphereProfile, AtmosphereProfile)> {
        let (space, before) = self.gesture.take()?;
        let after = self.entries.get(&space)?.current.clone();
        if before == after {
            return None;
        }
        self.revision += 1;
        Some((space, before, after))
    }
    pub fn cancel_gesture(&mut self) {
        if let Some((space, before)) = self.gesture.take() {
            self.entries.get_mut(&space).unwrap().current = before;
        }
    }
    pub fn journal(&self) -> Vec<Snapshot> {
        self.entries
            .values()
            .filter_map(|entry| {
                let mut entry = entry.clone();
                if let Some((space, before)) = &self.gesture {
                    if *space == entry.space {
                        entry.current = before.clone();
                    }
                }
                (entry.base != entry.current).then_some(entry)
            })
            .collect()
    }
    pub fn restore(&mut self, snapshots: Vec<Snapshot>) -> usize {
        let mut count = 0;
        for snapshot in snapshots.into_iter().take(32) {
            if snapshot.base.validate().is_err()
                || snapshot.current.validate().is_err()
                || snapshot.revision < 1
            {
                continue;
            }
            if self
                .entries
                .get(&snapshot.space)
                .is_none_or(|e| e.base == e.current)
            {
                self.entries.insert(snapshot.space, snapshot);
                count += 1;
            }
        }
        self.revision += 1;
        count
    }
    pub fn queue_save(&mut self, store: &mut ProjectEditorStore) -> bool {
        if self.blocked() {
            return false;
        }
        let writes = self
            .entries
            .values()
            .filter(|e| e.base != e.current)
            .map(|e| AtmosphereWrite {
                space: e.space,
                expected_revision: e.revision,
                profile: e.current.clone(),
            })
            .collect();
        self.saving = store.queue_atmospheres(writes);
        self.error = None;
        self.saving.is_some()
    }
    pub fn complete(&mut self, result: Result<AtmosphereWriteResult, String>) -> bool {
        self.saving = None;
        match result {
            Ok(AtmosphereWriteResult::Committed(records)) => {
                for (id, revision, profile) in records {
                    if let Some(entry) = self.entries.get_mut(&id) {
                        entry.revision = revision;
                        entry.base = profile;
                    }
                }
                self.revision += 1;
                self.error = None;
                true
            }
            Ok(AtmosphereWriteResult::Conflict {
                space,
                revision,
                actual,
            }) => {
                self.conflict = Some((space, revision, actual));
                self.error = Some(
                    "Atmosphere changed in the project database. Resolve the conflict below."
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
    pub fn resolve_conflict(&mut self, keep_local: bool) {
        if let Some((id, revision, actual)) = self.conflict.take() {
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.revision = revision;
                entry.base = actual.clone();
                if !keep_local {
                    entry.current = actual;
                }
            }
            self.revision += 1;
            self.error = None;
        }
    }
}
