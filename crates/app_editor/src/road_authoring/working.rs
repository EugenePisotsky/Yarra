use environment::roads::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use world_db::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RoadChange {
    pub key: RoadRecordKey,
    pub record: Option<RoadSourceRecord>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RoadEntry {
    pub base: RoadRecordState,
    pub current: Option<RoadSourceRecord>,
}
impl RoadEntry {
    fn dirty(&self) -> bool {
        !same(self.base.record.as_ref(), self.current.as_ref())
    }
}
pub(crate) fn same(a: Option<&RoadSourceRecord>, b: Option<&RoadSourceRecord>) -> bool {
    let normalized = |r: Option<&RoadSourceRecord>| {
        r.map(|r| {
            let mut r = r.clone();
            r.set_revision(1);
            r
        })
    };
    normalized(a) == normalized(b)
}
#[derive(Default)]
pub(crate) struct RoadWorkingSet {
    pub entries: BTreeMap<RoadRecordKey, RoadEntry>,
    pub complete_knots: BTreeSet<RoadKnotId>,
    pub style_usage: BTreeMap<RoadProfileId, Vec<RoadStyleUsage>>,
    pub route_minimum_widths: BTreeMap<RoadId, Option<f32>>,
    pub revision: u64,
    pub saving: Option<u64>,
    pub conflict: bool,
    pub error: Option<String>,
}
impl RoadWorkingSet {
    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }
    pub fn dirty_count(&self) -> usize {
        self.entries.values().filter(|e| e.dirty()).count()
    }
    pub fn get(&self, key: RoadRecordKey) -> Option<&RoadSourceRecord> {
        self.entries.get(&key)?.current.as_ref()
    }

    pub fn changes(&self, keys: impl IntoIterator<Item = RoadRecordKey>) -> Vec<RoadChange> {
        keys.into_iter()
            .map(|key| RoadChange {
                key,
                record: self.get(key).cloned(),
            })
            .collect()
    }
    pub fn replay(&mut self, from: &[RoadChange], to: &[RoadChange]) -> Result<(), String> {
        if from
            .iter()
            .any(|c| !same(self.get(c.key), c.record.as_ref()))
        {
            return Err("A road changed outside this command; reload it before undoing".into());
        }
        self.apply(to)
    }
    pub fn apply(&mut self, changes: &[RoadChange]) -> Result<(), String> {
        if self.saving.is_some() || self.conflict {
            return Err("Resolve the road save first".into());
        }
        let mut unique = BTreeSet::new();
        for c in changes {
            if !unique.insert(c.key) || c.record.as_ref().is_some_and(|r| r.key() != c.key) {
                return Err("Invalid road command identity".into());
            }
        }
        let mut next = self.entries.clone();
        for c in changes {
            let e = next.entry(c.key).or_insert_with(|| RoadEntry {
                base: RoadRecordState {
                    key: c.key,
                    revision: None,
                    record: None,
                },
                current: None,
            });
            e.current = c.record.clone();
            if let Some(r) = &mut e.current {
                r.set_revision(e.base.revision.unwrap_or(1));
            }
        }
        if next.len() > MAX_ROAD_DEPENDENCIES
            || next.values().filter(|e| e.dirty()).count() > MAX_ROAD_WRITES
        {
            return Err(
                "Save roads and clear older undo history before editing more controls".into(),
            );
        }
        // Reference closure must survive every undo/redo/delete, not just a later database save.
        for e in next.values() {
            if let Some(r) = &e.current {
                for key in r.references() {
                    if next.get(&key).and_then(|e| e.current.as_ref()).is_none() {
                        return Err("Road command is missing a referenced record".into());
                    }
                }
            }
        }
        if changes
            .iter()
            .any(|c| !same(self.get(c.key), c.record.as_ref()))
        {
            self.entries = next;
            self.error = None;
            self.bump();
        }
        Ok(())
    }
    /// Keep command records and their references; protect incident spans of locally moved knots.
    pub fn pins(&self, history: &BTreeSet<RoadRecordKey>) -> BTreeSet<RoadRecordKey> {
        let mut pins = history.clone();
        pins.extend(
            self.entries
                .iter()
                .filter(|(_, e)| e.dirty())
                .map(|(k, _)| *k),
        );
        self.closure(pins)
    }
    fn closure(&self, mut pins: BTreeSet<RoadRecordKey>) -> BTreeSet<RoadRecordKey> {
        // Junction members are one bounded connection, not an expansion of a whole route.
        for (key, e) in &self.entries {
            for r in [e.current.as_ref(), e.base.record.as_ref()]
                .into_iter()
                .flatten()
            {
                if let RoadSourceRecord::Junction(j) = r
                    && (pins.contains(key)
                        || j.junction
                            .knots
                            .iter()
                            .any(|k| pins.contains(&RoadRecordKey::Knot(*k))))
                {
                    pins.insert(*key);
                    pins.extend(j.junction.knots.iter().copied().map(RoadRecordKey::Knot));
                }
            }
        }
        let knots = pins
            .iter()
            .filter_map(|k| {
                if let RoadRecordKey::Knot(id) = k {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();
        for (key, e) in &self.entries {
            for r in [e.current.as_ref(), e.base.record.as_ref()]
                .into_iter()
                .flatten()
            {
                if let RoadSourceRecord::Span(s) = r
                    && (knots.contains(&s.start) || knots.contains(&s.end))
                {
                    pins.insert(*key);
                }
            }
        }
        let mut queue = pins.iter().copied().collect::<Vec<_>>();
        while let Some(key) = queue.pop() {
            if let Some(e) = self.entries.get(&key) {
                for r in [e.current.as_ref(), e.base.record.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    for reference in r.references() {
                        if pins.insert(reference) {
                            queue.push(reference);
                        }
                    }
                }
            }
        }
        pins
    }
    pub fn reconcile(&mut self, snapshot: RoadAuthoringSnapshot, pins: &BTreeSet<RoadRecordKey>) {
        let fresh = snapshot
            .records
            .iter()
            .map(|s| s.key)
            .collect::<BTreeSet<_>>();
        self.entries
            .retain(|k, e| e.dirty() || pins.contains(k) || fresh.contains(k));
        for state in snapshot.records {
            match self.entries.get_mut(&state.key) {
                Some(e) if e.dirty() => {
                    if e.base.revision != state.revision {
                        self.conflict = true;
                        self.error=Some("Road source changed on disk. Local edits are retained; discard them to reload.".into());
                    }
                }
                _ => {
                    self.entries.insert(
                        state.key,
                        RoadEntry {
                            current: state.record.clone(),
                            base: state,
                        },
                    );
                }
            }
        }
        self.complete_knots = snapshot.complete_knots;
        self.style_usage = snapshot.style_usage;
        self.route_minimum_widths = snapshot.route_minimum_widths;
        self.bump();
    }
    pub fn writes(&self) -> Vec<RoadSourceWrite> {
        self.entries
            .iter()
            .filter(|(_, e)| e.dirty())
            .map(|(key, e)| RoadSourceWrite {
                key: *key,
                expected_revision: e.base.revision,
                record: e.current.clone(),
            })
            .collect()
    }
    pub fn dependencies(&self) -> Vec<RoadDependency> {
        self.closure(
            self.entries
                .iter()
                .filter(|(_, e)| e.dirty())
                .map(|(k, _)| *k)
                .collect(),
        )
        .into_iter()
        .filter_map(|key| {
            let e = self.entries.get(&key)?;
            (!e.dirty())
                .then_some(e.base.revision)
                .flatten()
                .map(|revision| RoadDependency { key, revision })
        })
        .collect()
    }
    pub fn overrides(&self) -> Vec<RoadChange> {
        let pins = self.closure(
            self.entries
                .iter()
                .filter(|(_, e)| e.dirty())
                .map(|(k, _)| *k)
                .collect(),
        );
        self.changes(pins)
    }
    pub fn preview_regions(
        &self,
        space: world::WorldSpaceId,
        size: f32,
    ) -> Option<Vec<RoadCellBounds>> {
        let overrides = self.overrides();
        if overrides.is_empty() {
            return Some(vec![]);
        }
        if self.entries.iter().any(|(k, e)| {
            e.dirty()
                && e.base.record.is_some()
                && matches!(k, RoadRecordKey::Road(_) | RoadRecordKey::Profile(_))
        }) {
            return None;
        }
        let mut result = vec![];
        for base in [true, false] {
            let records = self
                .entries
                .iter()
                .filter_map(|(k, e)| {
                    if base {
                        e.base.record.clone()
                    } else {
                        e.current.clone()
                    }
                    .map(|r| (*k, r))
                })
                .collect::<BTreeMap<_, _>>();
            for c in &overrides {
                if let Some(RoadSourceRecord::Junction(j)) = records.get(&c.key)
                    && j.space == space
                {
                    result.push(j.junction.bounds(size).ok()?);
                }
                let Some(RoadSourceRecord::Span(s)) = records.get(&c.key) else {
                    continue;
                };
                let Some(RoadSourceRecord::Road(r)) = records.get(&RoadRecordKey::Road(s.road))
                else {
                    return None;
                };
                if r.space != space {
                    continue;
                }
                let Some(RoadSourceRecord::Profile(p)) =
                    records.get(&RoadRecordKey::Profile(r.road.profile))
                else {
                    return None;
                };
                let Some(RoadSourceRecord::Knot(a)) = records.get(&RoadRecordKey::Knot(s.start))
                else {
                    return None;
                };
                let Some(RoadSourceRecord::Knot(b)) = records.get(&RoadRecordKey::Knot(s.end))
                else {
                    return None;
                };
                result.push(
                    influence_bounds(
                        &RoadSpan {
                            id: s.id,
                            revision: s.revision,
                            road: s.road,
                            start: a.knot.clone(),
                            end: b.knot.clone(),
                        },
                        p,
                        size,
                    )
                    .ok()?,
                );
            }
        }
        for bounds in &mut result {
            bounds.minimum.x = bounds.minimum.x.saturating_sub(1);
            bounds.minimum.z = bounds.minimum.z.saturating_sub(1);
            bounds.maximum.x = bounds.maximum.x.saturating_add(1);
            bounds.maximum.z = bounds.maximum.z.saturating_add(1);
        }
        Some(result)
    }
    pub fn journal(&self) -> Vec<RoadEntry> {
        if self.dirty_count() == 0 {
            return vec![];
        }
        self.pins(&BTreeSet::new())
            .into_iter()
            .filter_map(|k| self.entries.get(&k).cloned())
            .collect()
    }
    pub fn restore(&mut self, entries: Vec<RoadEntry>) -> usize {
        if entries.len() > MAX_ROAD_DEPENDENCIES {
            return 0;
        }
        let count = entries.len();
        let next = entries
            .into_iter()
            .map(|e| (e.base.key, e))
            .collect::<BTreeMap<_, _>>();
        if next.len() != count
            || next.values().any(|e| {
                e.base.revision == Some(0)
                    || e.base
                        .record
                        .as_ref()
                        .is_some_and(|r| Some(r.revision()) != e.base.revision)
                    || e.current.as_ref().is_some_and(|r| {
                        r.revision() == 0
                            || r.references()
                                .iter()
                                .any(|k| next.get(k).and_then(|e| e.current.as_ref()).is_none())
                    })
            })
            || next.values().filter(|e| e.dirty()).count() > MAX_ROAD_WRITES
            || next.iter().any(|(k, e)| {
                e.base
                    .record
                    .iter()
                    .chain(e.current.iter())
                    .any(|r| r.key() != *k)
            })
        {
            return 0;
        }
        let count = next.values().filter(|e| e.dirty()).count();
        self.entries = next;
        self.bump();
        count
    }
    pub fn finish_save(&mut self, id: u64, outcome: &crate::project_store::DenseSaveOutcome) {
        if self.saving != Some(id) {
            return;
        }
        self.saving = None;
        use crate::project_store::DenseSaveOutcome::*;
        match outcome {
            Committed(commit) => {
                if let Some(roads) = &commit.roads {
                    for s in &roads.records {
                        self.entries.insert(
                            s.key,
                            RoadEntry {
                                base: s.clone(),
                                current: s.record.clone(),
                            },
                        );
                    }
                }
                self.error = None;
            }
            RoadConflict { actual } => {
                self.conflict = true;
                self.error = Some(format!(
                    "Road {:?} changed on disk. Local edits are retained.",
                    actual.key
                ));
            }
            Failed(e) => self.error = Some(e.clone()),
            _ => {}
        }
        self.bump();
    }
    pub fn discard(&mut self) {
        self.entries.clear();
        self.complete_knots.clear();
        self.conflict = false;
        self.error = None;
        self.bump();
    }
}
