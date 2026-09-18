//! Normalized, revisioned road source. Interactive work is bounded by records/cells, not road length.
mod authoring;
mod codec;
pub use authoring::*;
mod document;
mod junctions;
mod query;
mod terrain;
pub(crate) use query::read_snapshot as read_cook_roads;
pub use terrain::RoadTerrainSnapshot;
pub(crate) use terrain::read_terrain_source;
mod transaction;
pub use document::RoadDocumentIndex;
pub(crate) use document::{read_document, validate_cook_source, write_document};
pub(crate) use transaction::{apply_transaction, validate_shared};
#[cfg(test)]
mod tests;
use crate::environment_store::{read_definition, read_library};
use crate::{ProjectReader, ProjectWriter, WorldDbError};
use codec::*;
use environment::roads::*;
use environment::{PresetId, PresetKind, PresetLibrary};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use world::{CellCoord, WorldSpaceId};
pub const MAX_ROAD_WRITES: usize = 64;
pub const MAX_ROAD_DEPENDENCIES: usize = 1024;
pub const MAX_ROAD_INDEX_CELLS_PER_SPAN: usize = 4096;
pub const MAX_ROAD_INDEX_WRITES_PER_TRANSACTION: usize = 32768;
pub const MAX_ROAD_QUERY_MEMBERSHIPS: usize = 65536;
const MAX_ROAD_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRoad {
    pub space: WorldSpaceId,
    pub road: Road,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRoadKnot {
    pub road: RoadId,
    pub knot: RoadKnot,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRoadSpan {
    pub id: RoadSpanId,
    pub revision: u64,
    pub road: RoadId,
    pub start: RoadKnotId,
    pub end: RoadKnotId,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRoadJunction {
    pub space: WorldSpaceId,
    pub junction: RoadJunction,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RoadRecordKey {
    Profile(RoadProfileId),
    Road(RoadId),
    Knot(RoadKnotId),
    Span(RoadSpanId),
    Junction(RoadJunctionId),
}
impl RoadRecordKey {
    fn parts(self) -> (i64, [u8; 16], &'static str) {
        match self {
            Self::Profile(id) => (0, id.0, "road_profiles"),
            Self::Road(id) => (1, id.0, "roads"),
            Self::Knot(id) => (2, id.0, "road_knots"),
            Self::Span(id) => (3, id.0, "road_spans"),
            Self::Junction(id) => (4, id.0, "road_junctions"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RoadSourceRecord {
    Profile(CartTrackProfile),
    Road(SourceRoad),
    Knot(SourceRoadKnot),
    Span(SourceRoadSpan),
    Junction(SourceRoadJunction),
}
impl RoadSourceRecord {
    pub fn references(&self) -> Vec<RoadRecordKey> {
        match self {
            Self::Profile(_) => vec![],
            Self::Junction(j) => std::iter::once(RoadRecordKey::Profile(j.junction.profile))
                .chain(j.junction.knots.iter().copied().map(RoadRecordKey::Knot))
                .collect(),
            Self::Road(r) => vec![RoadRecordKey::Profile(r.road.profile)],
            Self::Knot(k) => vec![RoadRecordKey::Road(k.road)],
            Self::Span(s) => vec![
                RoadRecordKey::Road(s.road),
                RoadRecordKey::Knot(s.start),
                RoadRecordKey::Knot(s.end),
            ],
        }
    }

    pub fn key(&self) -> RoadRecordKey {
        match self {
            Self::Profile(p) => RoadRecordKey::Profile(p.id),
            Self::Road(r) => RoadRecordKey::Road(r.road.id),
            Self::Knot(k) => RoadRecordKey::Knot(k.knot.id),
            Self::Span(s) => RoadRecordKey::Span(s.id),
            Self::Junction(j) => RoadRecordKey::Junction(j.junction.id),
        }
    }
    pub fn revision(&self) -> u64 {
        match self {
            Self::Profile(p) => p.revision,
            Self::Road(r) => r.road.revision,
            Self::Knot(k) => k.knot.revision,
            Self::Span(s) => s.revision,
            Self::Junction(j) => j.junction.revision,
        }
    }
    pub fn set_revision(&mut self, value: u64) {
        match self {
            Self::Profile(p) => p.revision = value,
            Self::Road(r) => r.road.revision = value,
            Self::Knot(k) => k.knot.revision = value,
            Self::Span(s) => s.revision = value,
            Self::Junction(j) => j.junction.revision = value,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadRecordState {
    pub key: RoadRecordKey,
    /// None means this identity has never existed; Some with no record is a tombstone.
    pub revision: Option<u64>,
    pub record: Option<RoadSourceRecord>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadSourceWrite {
    pub key: RoadRecordKey,
    pub expected_revision: Option<u64>,
    /// None deletes; supplied payload revisions are replaced by the committed revision.
    pub record: Option<RoadSourceRecord>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadDependency {
    pub key: RoadRecordKey,
    pub revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadAffectedBounds {
    pub space: WorldSpaceId,
    pub bounds: RoadCellBounds,
}
/// Shared edits return selectors. Drain their indexed dependency pages before declaring previews current.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoadDependencySelector {
    Road(RoadId),
    Profile(RoadProfileId),
    GroundPreset(PresetId),
}
#[derive(Debug, Clone, PartialEq)]
pub struct RoadSourceCommit {
    pub revision: u64,
    pub records: Vec<RoadRecordState>,
    pub bounds: Vec<RoadAffectedBounds>,
    pub dependencies: Vec<RoadDependencySelector>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum RoadSourceWriteResult {
    Committed(RoadSourceCommit),
    Conflict(RoadRecordState),
    LibraryConflict { actual_revision: u64 },
}
#[derive(Debug, Clone)]
pub struct RoadReadSnapshot {
    /// Project road epoch for query/pagination freshness, not a geometry CAS or cell fingerprint.
    pub revision: u64,
    pub roads: RoadSnapshot,
}
impl RoadReadSnapshot {
    pub fn dependencies(&self) -> Vec<RoadDependency> {
        let mut result = BTreeMap::new();
        for j in &self.roads.junctions {
            result.insert(RoadRecordKey::Junction(j.id), j.revision);
        }
        for p in &self.roads.profiles {
            result.insert(RoadRecordKey::Profile(p.id), p.revision);
        }
        for r in &self.roads.roads {
            result.insert(RoadRecordKey::Road(r.id), r.revision);
        }
        for s in &self.roads.spans {
            result.insert(RoadRecordKey::Span(s.id), s.revision);
            for k in [&s.start, &s.end] {
                result.insert(RoadRecordKey::Knot(k.id), k.revision);
            }
        }
        result
            .into_iter()
            .map(|(key, revision)| RoadDependency { key, revision })
            .collect()
    }
}
#[derive(Debug, Clone)]
pub struct RoadDependencyPage {
    pub road_revision: u64,
    pub library_revision: u64,
    pub spans: Vec<(RoadSpanId, RoadAffectedBounds)>,
    pub next_cursor: Option<RoadSpanId>,
}
#[derive(Debug, Clone)]
pub struct RoadEnvironmentSnapshot {
    pub environment: crate::EnvironmentReadSnapshot,
    pub roads: RoadReadSnapshot,
}
/// Offline whole-project interchange; never used for viewport discovery.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoadDocument {
    pub records: Vec<RoadSourceRecord>,
}
fn invalid(message: impl Into<String>) -> WorldDbError {
    WorldDbError::Environment(format!("roads: {}", message.into()))
}
fn next_revision(value: u64) -> Result<u64, WorldDbError> {
    value
        .checked_add(1)
        .filter(|&n| n <= i64::MAX as u64)
        .ok_or(WorldDbError::IntegerOverflow)
}
fn epoch(c: &Connection) -> Result<u64, WorldDbError> {
    Ok(c.query_row(
        "SELECT revision FROM road_state WHERE singleton=1",
        [],
        |r| r.get::<_, i64>(0),
    )? as u64)
}
fn definition(
    c: &Connection,
    space: WorldSpaceId,
) -> Result<environment::EnvironmentDefinition, WorldDbError> {
    read_definition(c, space)?.ok_or_else(|| invalid("route world has no environment definition"))
}
