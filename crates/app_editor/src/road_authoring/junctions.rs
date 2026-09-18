//! Atomic junction gestures, reusing road history and persistence.
use super::*;
use std::collections::BTreeMap;
use world_db::SourceRoadJunction;
fn change(record: RoadSourceRecord) -> RoadChange {
    RoadChange {
        key: record.key(),
        record: Some(record),
    }
}
fn knot(w: &working::RoadWorkingSet, id: RoadKnotId) -> Result<world_db::SourceRoadKnot, String> {
    match w.get(RoadRecordKey::Knot(id)) {
        Some(RoadSourceRecord::Knot(k)) => Ok(k.clone()),
        _ => Err("Choose a loaded road point".into()),
    }
}
pub(super) fn at(w: &working::RoadWorkingSet, id: RoadKnotId) -> Option<&SourceRoadJunction> {
    w.entries.values().find_map(|e| match &e.current {
        Some(RoadSourceRecord::Junction(j)) if j.junction.knots.contains(&id) => Some(j),
        _ => None,
    })
}
fn incident(w: &working::RoadWorkingSet, id: RoadKnotId) -> usize {
    w.entries
        .values()
        .filter(
            |e| matches!(&e.current,Some(RoadSourceRecord::Span(s)) if s.start==id || s.end==id),
        )
        .count()
}
pub(super) fn ready(w: &working::RoadWorkingSet, id: RoadKnotId) -> bool {
    commands::complete(w, id)
        && at(w, id).is_none_or(|j| j.junction.knots.iter().all(|k| commands::complete(w, *k)))
}
pub(super) fn join(
    w: &working::RoadWorkingSet,
    source: RoadKnotId,
    target: RoadKnotId,
    size: f32,
) -> Result<Vec<RoadChange>, String> {
    if source == target {
        return Err("Choose a point on another road".into());
    }
    if !ready(w, source) || !ready(w, target) {
        return Err("Wait for all adjoining curves to load".into());
    }
    if at(w, source).is_some() {
        return Err("Disconnect this point before joining it elsewhere".into());
    }
    if !(1..=2).contains(&incident(w, source)) {
        return Err("Choose a point with one or two adjoining road sections".into());
    }
    let mut a = knot(w, source)?;
    let b = knot(w, target)?;
    let road = |id| match w.get(RoadRecordKey::Road(id)) {
        Some(RoadSourceRecord::Road(r)) => Ok(r.clone()),
        _ => Err("Road metadata is not loaded".to_string()),
    };
    let ar = road(a.road)?;
    let br = road(b.road)?;
    if ar.space != br.space || ar.road.profile != br.road.profile {
        return Err(
            "First-version junctions connect roads in the same world using the same style".into(),
        );
    }
    let mut j = at(w, target)
        .cloned()
        .unwrap_or_else(|| SourceRoadJunction {
            space: br.space,
            junction: RoadJunction {
                id: RoadJunctionId(*uuid::Uuid::new_v4().as_bytes()),
                revision: 1,
                position: b.knot.position,
                radius: b.knot.width.max(a.knot.width).max(1.0),
                profile: br.road.profile,
                seed: br.road.seed,
                knots: vec![target],
            },
        });
    if j.junction
        .knots
        .iter()
        .any(|id| knot(w, *id).is_ok_and(|k| k.road == a.road))
    {
        return Err("A road can use only one point in this junction".into());
    }
    j.junction.knots.push(source);
    j.junction.knots.sort();
    j.junction.radius = j.junction.radius.max(a.knot.width);
    j.junction.validate(size).map_err(|e| e.to_string())?;
    if j.junction
        .knots
        .iter()
        .map(|id| incident(w, *id))
        .sum::<usize>()
        > 4
    {
        return Err("This junction already has four arms".into());
    }
    a.knot.position = b.knot.position;
    Ok(vec![
        change(RoadSourceRecord::Knot(a)),
        change(RoadSourceRecord::Junction(j)),
    ])
}
pub(super) fn branch(
    w: &working::RoadWorkingSet,
    target: RoadKnotId,
    size: f32,
    end: RoadPoint,
) -> Result<(RoadId, RoadKnotId, Vec<RoadChange>), String> {
    if !ready(w, target) {
        return Err("Wait for adjoining curves to load".into());
    }
    let a = knot(w, target)?;
    let Some(RoadSourceRecord::Road(r)) = w.get(RoadRecordKey::Road(a.road)) else {
        return Err("Road metadata is not loaded".into());
    };
    let (road, end_id, changes) =
        commands::create(w, r.space, size, r.road.profile, a.knot.position, end)?;
    let start_id = changes
        .iter()
        .find_map(|c| match &c.record {
            Some(RoadSourceRecord::Span(s)) => Some(s.start),
            _ => None,
        })
        .unwrap();
    let mut draft = working::RoadWorkingSet {
        entries: w.entries.clone(),
        complete_knots: w.complete_knots.clone(),
        ..Default::default()
    };
    draft.apply(&changes)?;
    let joined = join(&draft, start_id, target, size)?;
    let merged = changes
        .into_iter()
        .chain(joined)
        .map(|c| (c.key, c))
        .collect::<BTreeMap<_, _>>();
    Ok((road, end_id, merged.into_values().collect()))
}
pub(super) fn detach(
    w: &working::RoadWorkingSet,
    id: RoadKnotId,
) -> Result<Vec<RoadChange>, String> {
    if !ready(w, id) {
        return Err("Wait for adjoining curves to load".into());
    }
    let mut j = at(w, id)
        .cloned()
        .ok_or("This point is not in a junction")?;
    j.junction.knots.retain(|k| *k != id);
    let key = RoadRecordKey::Junction(j.junction.id);
    Ok(vec![RoadChange {
        key,
        record: (j.junction.knots.len() >= 2).then_some(RoadSourceRecord::Junction(j)),
    }])
}
pub(super) fn move_point(
    w: &working::RoadWorkingSet,
    primary: RoadChange,
) -> Result<Vec<RoadChange>, String> {
    let Some(RoadSourceRecord::Knot(k)) = &primary.record else {
        return Err("Expected a road point".into());
    };
    let Some(mut j) = at(w, k.knot.id).cloned() else {
        return Ok(vec![primary]);
    };
    if !ready(w, k.knot.id) {
        return Err("Wait for junction members to load".into());
    }
    j.junction.position = k.knot.position;
    let mut changes = vec![];
    for id in &j.junction.knots {
        if *id == k.knot.id {
            continue;
        }
        let mut other = knot(w, *id)?;
        other.knot.position = k.knot.position;
        changes.push(change(RoadSourceRecord::Knot(other)));
    }
    changes.push(change(RoadSourceRecord::Junction(j)));
    changes.push(primary);
    Ok(changes)
}
pub(super) fn drag_before(w: &working::RoadWorkingSet, id: RoadKnotId) -> Vec<RoadChange> {
    let mut keys = vec![RoadRecordKey::Knot(id)];
    if let Some(j) = at(w, id) {
        keys.push(RoadRecordKey::Junction(j.junction.id));
        keys.extend(
            j.junction
                .knots
                .iter()
                .filter(|k| **k != id)
                .map(|k| RoadRecordKey::Knot(*k)),
        );
    }
    w.changes(keys)
}

pub(super) fn inspector(
    ui: &mut egui::Ui,
    state: &mut RoadToolState,
    dense: &mut DenseDomainWorkingSets,
    history: &mut EditorHistory,
    knot: RoadKnotId,
    size: f32,
) {
    ui.horizontal(|ui| {
        if ui.button("Branch from point").clicked() {
            state.branching=true;state.connecting=false;state.creating=false;state.extending=false;state.first=None;
            state.status=Some("Click the new branch endpoint. The branch shares this road's style.".into());
        }
        if ui.button("Connect to point…").clicked() {
            state.connecting=true;state.branching=false;state.creating=false;state.extending=false;state.first=None;
            state.status=Some("Click a point on another road using the same style. Split its curve first if needed.".into());
        }
    });
    let Some(source) = at(&dense.roads, knot).cloned() else {
        return;
    };
    ui.label(format!(
        "Junction · {} connected roads",
        source.junction.knots.len()
    ));
    ui.weak("Moving this point moves every connected point. Choose a road below to edit its tangent handles.");
    for id in &source.junction.knots {
        let Some(RoadSourceRecord::Knot(k)) = dense.roads.get(RoadRecordKey::Knot(*id)) else {
            continue;
        };
        let Some(RoadSourceRecord::Road(r)) = dense.roads.get(RoadRecordKey::Road(k.road)) else {
            continue;
        };
        if ui
            .selectable_label(state.knot == Some(*id), &r.road.name)
            .clicked()
        {
            state.knot = Some(*id);
            state.road = Some(k.road);
            state.span = dense.roads.entries.values().find_map(|e| match &e.current {
                Some(RoadSourceRecord::Span(s)) if s.start == *id || s.end == *id => Some(s.id),
                _ => None,
            });
        }
    }
    let key = ui.make_persistent_id(("junction-radius", source.junction.id.0));
    let mut draft = ui
        .data_mut(|d| d.get_temp::<(f32, f32)>(key))
        .unwrap_or((source.junction.radius, source.junction.radius));
    if draft.0 != source.junction.radius {
        draft = (source.junction.radius, source.junction.radius);
    }
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut draft.1).speed(0.05).range(1.0..=16.0).suffix(" m"));
        ui.label("Blend radius");
        if ui.add_enabled(draft.1!=source.junction.radius,egui::Button::new("Apply radius")).clicked() {
            let mut next=source.clone();next.junction.radius=draft.1;
            let valid=next.junction.knots.iter().all(|id|matches!(dense.roads.get(RoadRecordKey::Knot(*id)),Some(RoadSourceRecord::Knot(k)) if k.knot.width<=next.junction.radius));
            let changes=next.junction.validate(size).map_err(|e|e.to_string()).and_then(|_|if valid{Ok(vec![change(RoadSourceRecord::Junction(next))])}else{Err("Junction radius must be at least the widest connected corridor".into())});
            if state.commit(dense,history,changes){draft.0=draft.1;}
        }
    });
    ui.data_mut(|d| d.insert_temp(key, draft));
    if ui.button("Disconnect this road point").clicked() {
        let changes = detach(&dense.roads, knot);
        if state.commit(dense, history, changes) {
            state.status =
                Some("Point disconnected. Move it away to remove the unsupported overlap.".into());
        }
    }
}
