use super::working::{RoadChange, RoadWorkingSet};
use environment::{ChannelId, PresetId, roads::*};
use world::WorldSpaceId;
use world_db::*;
fn id() -> [u8; 16] {
    *uuid::Uuid::new_v4().as_bytes()
}
fn change(r: RoadSourceRecord) -> RoadChange {
    RoadChange {
        key: r.key(),
        record: Some(r),
    }
}
pub(crate) fn default_profile(ground: PresetId, channel: ChannelId) -> CartTrackProfile {
    CartTrackProfile {
        id: RoadProfileId(id()),
        revision: 1,
        name: "Worn cart track".into(),
        ground,
        vegetation_channel: channel,
        track_spacing: 1.55,
        track_width: 0.65,
        edge_softness: 0.15,
        center_ground: 0.0,
        center_retention: 1.0,
        shoulder_ground: 0.15,
        shoulder_retention: 0.55,
        track_retention: 0.0,
        edge_variation: 0.08,
        edge_patch_size: 2.0,
        breakup: 0.7,
        breakup_patch_size: 1.2,
        relief: Default::default(),
    }
}
pub(super) fn create(
    working: &RoadWorkingSet,
    space: WorldSpaceId,
    size: f32,
    style: RoadProfileId,
    start: RoadPoint,
    end: RoadPoint,
) -> Result<(RoadId, RoadKnotId, Vec<RoadChange>), String> {
    let d = end.relative_to(start.cell, size as f64);
    let delta = [d[0] - start.local[0], d[1] - start.local[1]];
    if delta[0].hypot(delta[1]) < 2.0 {
        return Err("Place the second point at least 2 m away".into());
    }
    let Some(RoadSourceRecord::Profile(profile)) = working.get(RoadRecordKey::Profile(style))
    else {
        return Err("Choose a loaded road style first".into());
    };
    profile.validate().map_err(|e| e.to_string())?;
    let mut changes = vec![];
    let road = Road {
        id: RoadId(id()),
        revision: 1,
        name: "Cart road".into(),
        profile: profile.id,
        seed: u32::from_le_bytes(id()[..4].try_into().unwrap()),
        enabled: true,
        order: 0,
        direction: TravelDirection::Bidirectional,
        travel_modes: vec![
            RoadTravelMode::Foot,
            RoadTravelMode::Mounted,
            RoadTravelMode::Cart,
        ],
    };
    let a = RoadKnot {
        id: RoadKnotId(id()),
        revision: 1,
        position: start,
        incoming: [0.0; 2],
        outgoing: delta.map(|v| v / 3.0),
        width: 3.5f32.max(profile.minimum_width()),
    };
    let b = RoadKnot {
        id: RoadKnotId(id()),
        revision: 1,
        position: end,
        incoming: delta.map(|v| -v / 3.0),
        outgoing: [0.0; 2],
        width: a.width,
    };
    a.validate(size, profile).map_err(|e| e.to_string())?;
    b.validate(size, profile).map_err(|e| e.to_string())?;
    changes.push(change(RoadSourceRecord::Road(SourceRoad {
        space,
        road: road.clone(),
    })));
    changes.extend([a.clone(), b.clone()].into_iter().map(|knot| {
        change(RoadSourceRecord::Knot(SourceRoadKnot {
            road: road.id,
            knot,
        }))
    }));
    changes.push(change(RoadSourceRecord::Span(SourceRoadSpan {
        id: RoadSpanId(id()),
        revision: 1,
        road: road.id,
        start: a.id,
        end: b.id,
    })));
    Ok((road.id, b.id, changes))
}
pub(super) fn span(w: &RoadWorkingSet, id: RoadSpanId) -> Option<RoadSpan> {
    let RoadSourceRecord::Span(s) = w.get(RoadRecordKey::Span(id))? else {
        return None;
    };
    let RoadSourceRecord::Knot(a) = w.get(RoadRecordKey::Knot(s.start))? else {
        return None;
    };
    let RoadSourceRecord::Knot(b) = w.get(RoadRecordKey::Knot(s.end))? else {
        return None;
    };
    Some(RoadSpan {
        id: s.id,
        revision: s.revision,
        road: s.road,
        start: a.knot.clone(),
        end: b.knot.clone(),
    })
}
pub(super) fn complete(w: &RoadWorkingSet, id: RoadKnotId) -> bool {
    w.complete_knots.contains(&id)
        || w.entries
            .get(&RoadRecordKey::Knot(id))
            .is_some_and(|e| e.base.revision.is_none())
}
pub(super) fn split(
    w: &RoadWorkingSet,
    key: RoadSpanId,
    size: f32,
) -> Result<Vec<RoadChange>, String> {
    let s = span(w, key).ok_or("Select a loaded curve first")?;
    if !complete(w, s.start.id) || !complete(w, s.end.id) {
        return Err("Wait for adjoining spans to load".into());
    }
    let (a, b) =
        split_span(&s, size, 0.5, RoadKnotId(id()), RoadSpanId(id())).map_err(|e| e.to_string())?;
    let mut changes = [a.start.clone(), a.end.clone(), b.end.clone()]
        .into_iter()
        .map(|knot| {
            change(RoadSourceRecord::Knot(SourceRoadKnot {
                road: s.road,
                knot,
            }))
        })
        .collect::<Vec<_>>();
    changes.extend([a, b].into_iter().map(|s| {
        change(RoadSourceRecord::Span(SourceRoadSpan {
            id: s.id,
            revision: s.revision,
            road: s.road,
            start: s.start.id,
            end: s.end.id,
        }))
    }));
    Ok(changes)
}
pub(super) fn extend(
    w: &RoadWorkingSet,
    key: RoadKnotId,
    size: f32,
    end: RoadPoint,
) -> Result<(RoadKnotId, Vec<RoadChange>), String> {
    if super::junctions::at(w, key).is_some() {
        return Err("Use Branch from point at a connected junction".into());
    }
    if !complete(w, key) {
        return Err("Wait for adjoining spans to load".into());
    }
    let Some(RoadSourceRecord::Knot(a)) = w.get(RoadRecordKey::Knot(key)) else {
        return Err("Select an endpoint".into());
    };
    if w.entries
        .values()
        .any(|e| matches!(&e.current,Some(RoadSourceRecord::Span(s)) if s.start==key))
    {
        return Err("Select the last endpoint of the road to extend it".into());
    }
    let mut a = a.clone();
    let d = end.relative_to(a.knot.position.cell, size as f64);
    let delta = [
        d[0] - a.knot.position.local[0],
        d[1] - a.knot.position.local[1],
    ];
    let distance = delta[0].hypot(delta[1]);
    if distance < 2.0 {
        return Err("Extend by at least 2 m".into());
    }
    let tangent = a.knot.incoming;
    let len = tangent[0].hypot(tangent[1]);
    a.knot.outgoing = if len > 0.01 {
        tangent.map(|v| -v / len * distance / 3.0)
    } else {
        delta.map(|v| v / 3.0)
    };
    let b = RoadKnot {
        id: RoadKnotId(id()),
        revision: 1,
        position: end,
        incoming: delta.map(|v| -v / 3.0),
        outgoing: [0.0; 2],
        width: a.knot.width,
    };
    let changes = vec![
        change(RoadSourceRecord::Knot(a.clone())),
        change(RoadSourceRecord::Knot(SourceRoadKnot {
            road: a.road,
            knot: b.clone(),
        })),
        change(RoadSourceRecord::Span(SourceRoadSpan {
            id: RoadSpanId(id()),
            revision: 1,
            road: a.road,
            start: key,
            end: b.id,
        })),
    ];
    Ok((b.id, changes))
}
pub(super) fn delete_span(w: &RoadWorkingSet, key: RoadSpanId) -> Result<Vec<RoadChange>, String> {
    let s = span(w, key).ok_or("Select a curve to remove")?;
    let mut changes = vec![RoadChange {
        key: RoadRecordKey::Span(key),
        record: None,
    }];
    for id in [s.start.id, s.end.id] {
        if !complete(w, id) {
            return Err("Wait for adjoining spans to load".into());
        }
        let count = w
            .entries
            .values()
            .filter(
                |e| matches!(&e.current,Some(RoadSourceRecord::Span(s)) if s.start==id||s.end==id),
            )
            .count();
        if count == 1 {
            if super::junctions::at(w, id).is_some() {
                changes.extend(super::junctions::detach(w, id)?);
            }
            changes.push(RoadChange {
                key: RoadRecordKey::Knot(id),
                record: None,
            });
        }
    }
    Ok(changes)
}
