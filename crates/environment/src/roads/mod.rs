//! Bounded road query contracts. These are source records, not baked paint or navigation data.
//! Persistence/editor integration is separate; a snapshot must contain every span whose
//! conservative influence bounds intersect its declared window, including off-window knots.
mod geometry;
use crate::{ChannelId, PresetId, ValidationError, unit};
pub use geometry::{influence_bounds, split_span};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use world::{CellCoord, WorldSpaceId};

macro_rules! road_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(pub [u8; 16]);
    };
}
road_id!(RoadId);
road_id!(RoadProfileId);
road_id!(RoadKnotId);
road_id!(RoadSpanId);
road_id!(RoadJunctionId);
pub const MAX_ROAD_RECORDS: usize = 64;
pub const MAX_ROAD_SPANS: usize = 256;
pub const MAX_ROAD_QUERY_CELLS: usize = 576;

/// Explicit at-grade connection. Member knots retain independent curve handles.
/// The first policy is a worn clearing, using one shared cart style for all arms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadJunction {
    pub id: RoadJunctionId,
    pub revision: u64,
    pub position: RoadPoint,
    pub radius: f32,
    pub profile: RoadProfileId,
    pub seed: u32,
    pub knots: Vec<RoadKnotId>,
}
impl RoadJunction {
    pub fn validate(&self, size: f32) -> Result<(), ValidationError> {
        if self.revision == 0
            || !range(self.radius, 1.0, 16.0)
            || !(2..=4).contains(&self.knots.len())
            || self.knots.iter().collect::<BTreeSet<_>>().len() != self.knots.len()
            || !range(size, 0.01, 65536.0)
            || self
                .position
                .local
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0 || *v >= f64::from(size))
        {
            return Err(invalid(
                "junction needs 2–4 distinct road points and a radius between 1 and 16 m",
            ));
        }
        Ok(())
    }
    pub fn bounds(&self, size: f32) -> Result<RoadCellBounds, ValidationError> {
        self.validate(size)?;
        let point = |sign: f64| {
            RoadPoint::from_relative(
                self.position.cell,
                self.position
                    .local
                    .map(|v| v + sign * f64::from(self.radius)),
                f64::from(size),
            )
            .map(|p| p.cell)
        };
        Ok(RoadCellBounds {
            minimum: point(-1.0)?,
            maximum: point(1.0)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RoadPoint {
    pub cell: CellCoord,
    pub local: [f64; 2],
}
impl RoadPoint {
    pub fn relative_to(self, origin: CellCoord, size: f64) -> [f64; 2] {
        [
            (i64::from(self.cell.x) - i64::from(origin.x)) as f64 * size + self.local[0],
            (i64::from(self.cell.z) - i64::from(origin.z)) as f64 * size + self.local[1],
        ]
    }
    pub fn from_relative(
        origin: CellCoord,
        point: [f64; 2],
        size: f64,
    ) -> Result<Self, ValidationError> {
        if !size.is_finite() || size <= 0.0 || point.iter().any(|p| !p.is_finite()) {
            return Err(invalid("point"));
        }
        let offsets = point.map(|p| (p / size).floor());
        let x = f64::from(origin.x) + offsets[0];
        let z = f64::from(origin.z) + offsets[1];
        if [x, z]
            .iter()
            .any(|p| *p < f64::from(i32::MIN) || *p > f64::from(i32::MAX))
        {
            return Err(invalid("coordinate range"));
        }
        Ok(Self {
            cell: CellCoord {
                x: x as i32,
                z: z as i32,
            },
            local: [point[0] - offsets[0] * size, point[1] - offsets[1] * size],
        })
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadKnot {
    pub id: RoadKnotId,
    pub revision: u64,
    pub position: RoadPoint,
    /// Cubic handle offsets in metres from this knot, including direction.
    pub incoming: [f64; 2],
    pub outgoing: [f64; 2],
    /// Full corridor width. Changes shoulders, not cart wheel spacing.
    pub width: f32,
}
impl RoadKnot {
    pub fn validate(&self, size: f32, profile: &CartTrackProfile) -> Result<(), ValidationError> {
        profile.validate()?;
        if !range(self.width, profile.minimum_width(), 64.0) {
            return Err(invalid(
                "corridor width must fit the style's minimum road width and be at most 64 m",
            ));
        }
        if self.revision == 0
            || !range(size, 0.01, 65536.0)
            || self
                .position
                .local
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0 || *v >= f64::from(size))
            || self
                .incoming
                .iter()
                .chain(&self.outgoing)
                .any(|v| !v.is_finite() || v.abs() > 512.0)
        {
            return Err(invalid("knot"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadSpan {
    pub id: RoadSpanId,
    pub revision: u64,
    pub road: RoadId,
    pub start: RoadKnot,
    pub end: RoadKnot,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TravelDirection {
    Bidirectional,
    Forward,
    Reverse,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RoadTravelMode {
    Foot,
    Mounted,
    Cart,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Road {
    pub id: RoadId,
    pub revision: u64,
    pub name: String,
    pub profile: RoadProfileId,
    pub seed: u32,
    pub enabled: bool,
    pub order: i32,
    pub direction: TravelDirection,
    pub travel_modes: Vec<RoadTravelMode>,
}
impl Road {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.revision == 0
            || !name(&self.name)
            || self.travel_modes.is_empty()
            || self.travel_modes.len() > 3
            || self.travel_modes.iter().collect::<BTreeSet<_>>().len() != self.travel_modes.len()
        {
            return Err(invalid("road metadata"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CartTrackProfile {
    pub id: RoadProfileId,
    pub revision: u64,
    pub name: String,
    /// Reuses a Ground preset; road geometry supplies its spatial influence.
    pub ground: PresetId,
    pub vegetation_channel: ChannelId,
    pub track_spacing: f32,
    pub track_width: f32,
    pub edge_softness: f32,
    pub center_ground: f32,
    pub center_retention: f32,
    pub shoulder_ground: f32,
    pub shoulder_retention: f32,
    pub track_retention: f32,
    pub edge_variation: f32,
    pub edge_patch_size: f32,
    /// Fraction of otherwise worn coverage restored in noise-selected patches.
    pub breakup: f32,
    pub breakup_patch_size: f32,
    pub relief: RoadRelief,
}

/// Additional depression relative to the unmodified source heightfield, in metres.
/// Variation is a fraction of depth, keeping both effects nonnegative at every sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadRelief {
    pub road_depth: f32,
    pub track_depth: f32,
    pub shoulder_falloff: f32,
    pub rut_roundness: f32,
    pub road_variation: f32,
    pub road_variation_length: f32,
    pub track_variation: f32,
    pub track_variation_length: f32,
}
impl Default for RoadRelief {
    fn default() -> Self {
        Self {
            road_depth: 0.0,
            track_depth: 0.0,
            shoulder_falloff: 0.75,
            rut_roundness: 1.0,
            road_variation: 0.2,
            road_variation_length: 6.0,
            track_variation: 0.25,
            track_variation_length: 3.0,
        }
    }
}
impl RoadRelief {
    pub fn enabled(&self) -> bool {
        self.road_depth > 0.0 || self.track_depth > 0.0
    }
    pub fn validate(&self) -> Result<(), ValidationError> {
        for (value, min, max, message) in [
            (
                self.road_depth,
                0.0,
                2.0,
                "whole-road depth must be between 0 and 2 m",
            ),
            (
                self.track_depth,
                0.0,
                0.5,
                "additional track depth must be between 0 and 0.5 m",
            ),
            (
                self.shoulder_falloff,
                0.05,
                8.0,
                "terrain shoulder falloff must be between 0.05 and 8 m",
            ),
            (
                self.rut_roundness,
                0.0,
                1.0,
                "rut roundness must be between 0 and 1",
            ),
            (
                self.road_variation,
                0.0,
                1.0,
                "road depth variation must be between 0 and 1",
            ),
            (
                self.track_variation,
                0.0,
                1.0,
                "track depth variation must be between 0 and 1",
            ),
            (
                self.road_variation_length,
                0.5,
                64.0,
                "road variation length must be between 0.5 and 64 m",
            ),
            (
                self.track_variation_length,
                0.5,
                64.0,
                "track variation length must be between 0.5 and 64 m",
            ),
        ] {
            if !range(value, min, max) {
                return Err(invalid(message));
            }
        }
        Ok(())
    }
}
impl CartTrackProfile {
    pub fn validate_relief_detail(&self, height_step: f32) -> Result<(), &'static str> {
        let r = &self.relief;
        if !r.enabled() {
            return Ok(());
        }
        if !height_step.is_finite() || height_step <= 0.0 {
            return Err("invalid terrain height grid");
        }
        if r.road_depth > 0.0 && r.shoulder_falloff < height_step {
            return Err(
                "terrain shoulder falloff must span at least one height sample; broaden it or increase terrain resolution",
            );
        }
        if r.track_depth > 0.0 && self.track_width < 2.0 * height_step {
            return Err(
                "wheel ruts need at least two terrain height intervals across their width; widen tracks or increase terrain resolution",
            );
        }
        if (r.road_depth > 0.0
            && r.road_variation > 0.0
            && r.road_variation_length < 4.0 * height_step)
            || (r.track_depth > 0.0
                && r.track_variation > 0.0
                && r.track_variation_length < 4.0 * height_step)
        {
            return Err("depth variation length must span at least four terrain height intervals");
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.relief.validate()?;
        if self.revision == 0 || !name(&self.name) {
            return Err(invalid(
                "style needs a revision and a nonempty name of at most 256 bytes",
            ));
        }
        for (value, min, max, message) in [
            (
                self.track_width,
                0.05,
                8.0,
                "track width must be between 0.05 and 8 m",
            ),
            (
                self.track_spacing,
                0.1,
                16.0,
                "wheel spacing must be between 0.1 and 16 m",
            ),
            (
                self.edge_softness,
                0.01,
                4.0,
                "edge softness must be between 0.01 and 4 m",
            ),
            (
                self.edge_variation,
                0.0,
                4.0,
                "edge variation must be between 0 and 4 m",
            ),
            (
                self.edge_patch_size,
                0.25,
                64.0,
                "edge patch size must be between 0.25 and 64 m",
            ),
            (
                self.breakup_patch_size,
                0.25,
                64.0,
                "breakup patch size must be between 0.25 and 64 m",
            ),
        ] {
            if !range(value, min, max) {
                return Err(invalid(message));
            }
        }
        if self.track_spacing <= self.track_width {
            return Err(invalid("wheel spacing must be greater than track width"));
        }
        if self.edge_softness > self.track_width {
            return Err(invalid(
                "edge softness must not exceed track width; reduce softness or increase track width",
            ));
        }
        if self.edge_variation > self.track_width * 0.4 {
            return Err(invalid("edge variation must not exceed 40% of track width"));
        }
        for (value, message) in [
            (
                self.center_ground,
                "ground exposure at center must be between 0 and 1",
            ),
            (
                self.center_retention,
                "grass retained at center must be between 0 and 1",
            ),
            (
                self.shoulder_ground,
                "ground exposure at shoulders must be between 0 and 1",
            ),
            (
                self.shoulder_retention,
                "grass retained at shoulders must be between 0 and 1",
            ),
            (
                self.track_retention,
                "grass retained in tracks must be between 0 and 1",
            ),
            (self.breakup, "unworn patches must be between 0 and 1"),
        ] {
            if !unit(value) {
                return Err(invalid(message));
            }
        }
        Ok(())
    }
    pub fn minimum_width(&self) -> f32 {
        self.track_spacing + self.track_width + 2.0 * (self.edge_softness + self.edge_variation)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadCellBounds {
    pub minimum: CellCoord,
    pub maximum: CellCoord,
}
impl RoadCellBounds {
    pub fn contains(self, cell: CellCoord) -> bool {
        cell.x >= self.minimum.x
            && cell.x <= self.maximum.x
            && cell.z >= self.minimum.z
            && cell.z <= self.maximum.z
    }
    pub fn intersects(self, other: Self) -> bool {
        self.minimum.x <= other.maximum.x
            && self.maximum.x >= other.minimum.x
            && self.minimum.z <= other.maximum.z
            && self.maximum.z >= other.minimum.z
    }
    pub fn cell_count(self) -> Option<usize> {
        let x = i64::from(self.maximum.x) - i64::from(self.minimum.x) + 1;
        let z = i64::from(self.maximum.z) - i64::from(self.minimum.z) + 1;
        if x <= 0 || z <= 0 {
            return None;
        }
        usize::try_from(x.checked_mul(z)?).ok()
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadSnapshot {
    pub space: WorldSpaceId,
    pub cell_size: f32,
    pub loaded_bounds: RoadCellBounds,
    /// Truncated reads are never accepted as empty or complete road coverage.
    pub truncated: bool,
    pub roads: Vec<Road>,
    pub profiles: Vec<CartTrackProfile>,
    pub spans: Vec<RoadSpan>,
    pub junctions: Vec<RoadJunction>,
}
impl RoadSnapshot {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.truncated
            || !range(self.cell_size, 0.01, 65536.0)
            || self
                .loaded_bounds
                .cell_count()
                .is_none_or(|n| n > MAX_ROAD_QUERY_CELLS)
            || self.roads.len() > MAX_ROAD_RECORDS
            || self.profiles.len() > MAX_ROAD_RECORDS
            || self.spans.len() > MAX_ROAD_SPANS
            || self.junctions.len() > MAX_ROAD_RECORDS
        {
            return Err(invalid("snapshot bounds or completeness"));
        }
        let mut profiles = BTreeMap::new();
        for p in &self.profiles {
            p.validate()?;
            if profiles.insert(p.id, p).is_some() {
                return Err(invalid("duplicate profile"));
            }
        }
        let mut roads = BTreeMap::new();
        for r in &self.roads {
            r.validate()?;
            if roads.insert(r.id, r).is_some() || !profiles.contains_key(&r.profile) {
                return Err(invalid("road metadata or profile reference"));
            }
        }
        let mut spans = BTreeSet::new();
        let mut knots = BTreeMap::new();
        let mut directions = BTreeSet::new();
        for span in &self.spans {
            let Some(road) = roads.get(&span.road) else {
                return Err(invalid("span road reference"));
            };
            if span.revision == 0 || !spans.insert(span.id) || span.start.id == span.end.id {
                return Err(invalid("span identity"));
            }
            let profile = profiles[&road.profile];
            for (knot, is_start) in [(&span.start, true), (&span.end, false)] {
                knot.validate(self.cell_size, profile)?;
                if let Some((owner, previous)) = knots.insert(knot.id, (span.road, knot))
                    && (owner != span.road || previous != knot)
                {
                    return Err(invalid(
                        "inconsistent shared knot; explicit junction required between roads",
                    ));
                }
                if !directions.insert((span.road, knot.id, is_start)) {
                    return Err(invalid("branch requires an explicit junction"));
                }
            }
            let delta = span
                .end
                .position
                .relative_to(span.start.position.cell, f64::from(self.cell_size));
            if (delta[0] - span.start.position.local[0])
                .hypot(delta[1] - span.start.position.local[1])
                > 1024.0
            {
                return Err(invalid("span length; insert intermediate knots"));
            }
            influence_bounds(span, profile, self.cell_size)?;
        }
        let mut junction_ids = BTreeSet::new();
        let mut connected = BTreeSet::new();
        for junction in &self.junctions {
            junction.validate(self.cell_size)?;
            if !junction_ids.insert(junction.id) || !profiles.contains_key(&junction.profile) {
                return Err(invalid("duplicate junction or missing junction style"));
            }
            let mut owners = BTreeSet::new();
            let mut arms = 0;
            for id in &junction.knots {
                let Some((owner, knot)) = knots.get(id) else {
                    return Err(invalid("junction is missing an incident road point"));
                };
                if !connected.insert(*id)
                    || !owners.insert(*owner)
                    || knot.position != junction.position
                    || roads[owner].profile != junction.profile
                    || knot.width > junction.radius
                {
                    return Err(invalid(
                        "junction points must belong to different roads, share its position and style, and fit its radius",
                    ));
                }
                arms += self
                    .spans
                    .iter()
                    .filter(|s| s.start.id == *id || s.end.id == *id)
                    .count();
            }
            if !(2..=4).contains(&arms) {
                return Err(invalid("junction supports two to four incident arms"));
            }
        }
        Ok(())
    }
}
fn name(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 256
}
fn range(v: f32, min: f32, max: f32) -> bool {
    v.is_finite() && (min..=max).contains(&v)
}
fn invalid(what: &'static str) -> ValidationError {
    ValidationError::Road(what)
}
