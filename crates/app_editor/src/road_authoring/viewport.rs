use super::*;
use crate::{
    preview::{EditorPreviewMode, PreviewModeState},
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::EditorInputCapture,
    workspaces::world_impl::EditorOverlayGizmos,
};
use bevy::{
    ecs::system::SystemParam,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    window::PrimaryWindow,
};
use engine::{StreamedTerrainSurface, WorldOrigin, WorldViewCamera};
#[derive(Clone, Copy, PartialEq)]
enum Handle {
    Point,
    Incoming,
    Outgoing,
    Width,
}
pub(super) struct Drag {
    before: world_db::SourceRoadKnot,
    connected_before: Vec<RoadChange>,
    handle: Handle,
    start: RoadPoint,
}
#[derive(SystemParam)]
pub(super) struct Input<'w, 's> {
    window: Single<'w, 's, &'static Window, With<PrimaryWindow>>,
    camera: Single<'w, 's, (&'static Camera, &'static GlobalTransform), With<WorldViewCamera>>,
    terrain: Query<'w, 's, (Entity, &'static StreamedTerrainSurface)>,
    origin: Res<'w, WorldOrigin>,
    workspace: Res<'w, State<EditorWorkspace>>,
    mode: Res<'w, PreviewModeState>,
    tools: Res<'w, EditorToolRegistry>,
    capture: Res<'w, EditorInputCapture>,
    buttons: Res<'w, ButtonInput<MouseButton>>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    publication: Res<'w, RuntimePublicationState>,
    save: Res<'w, EditorSaveCoordinator>,
    project: Res<'w, ProjectEditorStore>,
    presets: Res<'w, crate::workspaces::presets::PresetAuthoringState>,
}
fn active(input: &Input) -> bool {
    *input.workspace.get() == EditorWorkspace::World
        && input.mode.active() == Some(EditorPreviewMode::Authoring)
        && input
            .tools
            .active(EditorWorkspace::World)
            .is_some_and(|t| t.id == ROAD_TOOL.id)
}
fn finish(
    state: &mut RoadToolState,
    dense: &mut DenseDomainWorkingSets,
    history: &mut EditorHistory,
    cancel: bool,
) {
    if let Some(drag) = state.drag.take() {
        dense.gesture_active = false;
        let before = if drag.connected_before.is_empty() {
            vec![RoadChange {
                key: RoadRecordKey::Knot(drag.before.knot.id),
                record: Some(RoadSourceRecord::Knot(drag.before)),
            }]
        } else {
            drag.connected_before
        };
        if cancel {
            if let Err(e) = dense.roads.apply(&before) {
                state.status = Some(e);
            }
        } else {
            let after = dense.roads.changes(before.iter().map(|c| c.key));
            history.record_roads(before, after);
        }
    }
}
pub(super) fn input(
    input: Input,
    mut raycast: MeshRayCast,
    mut dense: ResMut<DenseDomainWorkingSets>,
    mut history: ResMut<EditorHistory>,
    mut state: ResMut<RoadToolState>,
) {
    let enabled = active(&input) && input.window.focused;
    if input.keys.just_pressed(KeyCode::Escape) {
        finish(&mut state, &mut dense, &mut history, true);
        state.creating = false;
        state.extending = false;
        state.branching = false;
        state.connecting = false;
        state.first = None;
        state.status = None;
        return;
    }
    let blocked = input.capture.wants_pointer
        || input.buttons.pressed(MouseButton::Right)
        || input.buttons.pressed(MouseButton::Middle)
        || input.publication.active()
        || input.save.active()
        || input.project.save_in_flight()
        || dense.saving()
        || dense.has_any_conflict()
        || input.presets.dirty()
        || [
            KeyCode::SuperLeft,
            KeyCode::SuperRight,
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
            KeyCode::AltLeft,
            KeyCode::AltRight,
        ]
        .iter()
        .any(|k| input.keys.pressed(*k));
    if !enabled || blocked {
        finish(&mut state, &mut dense, &mut history, false);
    }
    if !enabled {
        state.hover = None;
        state.creating = false;
        state.extending = false;
        state.branching = false;
        state.connecting = false;
        state.first = None;
        return;
    }
    if blocked {
        return;
    }
    let Some(space) = input.origin.space() else {
        if !input.buttons.pressed(MouseButton::Left) {
            finish(&mut state, &mut dense, &mut history, false);
        }
        return;
    };
    if state.space != Some(space) {
        finish(&mut state, &mut dense, &mut history, false);
        state.space = Some(space);
        state.road = None;
        state.knot = None;
        state.span = None;
        state.creating = false;
        state.extending = false;
        state.branching = false;
        state.connecting = false;
        state.first = None;
        state.hover = None;
    }
    let Some(size) = dense.definition(space).map(|d| d.cell_size) else {
        if !input.buttons.pressed(MouseButton::Left) {
            finish(&mut state, &mut dense, &mut history, false);
        }
        return;
    };
    let Some(cursor) = input.window.cursor_position() else {
        if !input.buttons.pressed(MouseButton::Left) {
            finish(&mut state, &mut dense, &mut history, false);
        }
        return;
    };
    let Ok(ray) = input.camera.0.viewport_to_world(input.camera.1, cursor) else {
        if !input.buttons.pressed(MouseButton::Left) {
            finish(&mut state, &mut dense, &mut history, false);
        }
        return;
    };
    let filter = |e| {
        input
            .terrain
            .get(e)
            .is_ok_and(|(_, s)| s.key.space == space)
    };
    let settings = MeshRayCastSettings::default()
        .with_filter(&filter)
        .always_early_exit();
    let hit = raycast
        .cast_ray(ray, &settings)
        .first()
        .map(|(_, h)| h.point);
    let Some(hit) = hit else {
        state.hover = None;
        if !input.buttons.pressed(MouseButton::Left) {
            finish(&mut state, &mut dense, &mut history, false);
        }
        return;
    };
    let Ok(point) = RoadPoint::from_relative(
        input.origin.cell(),
        [hit.x as f64, hit.z as f64],
        size as f64,
    ) else {
        if !input.buttons.pressed(MouseButton::Left) {
            finish(&mut state, &mut dense, &mut history, false);
        }
        return;
    };
    state.hover = Some(point);
    if state.drag.is_some() {
        update_drag(
            &mut state,
            &mut dense,
            &mut history,
            point,
            size,
            !input.buttons.pressed(MouseButton::Left),
        );
        return;
    }
    if !input.buttons.just_pressed(MouseButton::Left) {
        return;
    }
    if state.creating {
        if !state.can_create_at(space, point.cell) {
            state.status = Some("Wait for nearby road controls to load".into());
            return;
        }
        if let Some(start) = state.first {
            let Some(style) = state.style else { return };
            match commands::create(&dense.roads, space, size, style, start, point) {
                Ok((road, knot, changes)) => {
                    let span = changes.iter().find_map(|c| {
                        if let RoadRecordKey::Span(id) = c.key {
                            Some(id)
                        } else {
                            None
                        }
                    });
                    if state.commit(&mut dense, &mut history, Ok(changes)) {
                        state.road = Some(road);
                        state.knot = Some(knot);
                        state.span = span;
                        state.creating = false;
                        state.first = None;
                    }
                }
                Err(e) => state.status = Some(e),
            }
        } else {
            state.first = Some(point);
            state.status = Some("Click the end of the first curve".into());
        }
        return;
    }
    if state.branching {
        if let Some(knot) = state.knot {
            match junctions::branch(&dense.roads, knot, size, point) {
                Ok((road, end, changes)) => {
                    let span = changes.iter().find_map(|c| match &c.record {
                        Some(RoadSourceRecord::Span(s)) if s.road == road => Some(s.id),
                        _ => None,
                    });
                    if state.commit(&mut dense, &mut history, Ok(changes)) {
                        state.road = Some(road);
                        state.knot = Some(end);
                        state.span = span;
                        state.branching = false;
                    }
                }
                Err(e) => state.status = Some(e),
            }
        }
        return;
    }
    if state.extending {
        if let Some(knot) = state.knot {
            match commands::extend(&dense.roads, knot, size, point) {
                Ok((id, changes)) => {
                    let span = changes.iter().find_map(|c| {
                        if let RoadRecordKey::Span(id) = c.key {
                            Some(id)
                        } else {
                            None
                        }
                    });
                    if state.commit(&mut dense, &mut history, Ok(changes)) {
                        state.knot = Some(id);
                        state.span = span;
                        state.extending = false;
                    }
                }
                Err(e) => state.status = Some(e),
            }
        }
        return;
    }
    let mut picks = vec![];
    for e in dense.roads.entries.values() {
        let Some(RoadSourceRecord::Knot(k)) = &e.current else {
            continue;
        };
        let Some(RoadSourceRecord::Road(r)) = dense.roads.get(RoadRecordKey::Road(k.road)) else {
            continue;
        };
        if r.space != space {
            continue;
        }
        for (handle, p) in handles(
            &k.knot,
            !state.connecting && state.knot == Some(k.knot.id),
            size,
        ) {
            let Some(p) = project_point(&input, p, size) else {
                continue;
            };
            let Ok(screen) = input.camera.0.world_to_viewport(input.camera.1, p) else {
                continue;
            };
            let distance = screen.distance(cursor);
            if distance < 12.0 {
                picks.push((distance, k.clone(), handle));
            }
        }
    }
    picks.sort_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, k, handle)) = picks.into_iter().next() {
        if state.connecting {
            if let Some(source) = state.knot {
                let changes = junctions::join(&dense.roads, source, k.knot.id, size);
                if state.commit(&mut dense, &mut history, changes) {
                    state.connecting = false;
                }
            }
            return;
        }
        state.road = Some(k.road);
        state.knot = Some(k.knot.id);
        state.span = dense.roads.entries.values().find_map(|e| {
            if let Some(RoadSourceRecord::Span(s)) = &e.current {
                (s.start == k.knot.id || s.end == k.knot.id).then_some(s.id)
            } else {
                None
            }
        });
        if junctions::ready(&dense.roads, k.knot.id) {
            let connected_before = if handle == Handle::Point {
                junctions::drag_before(&dense.roads, k.knot.id)
            } else {
                vec![]
            };
            state.drag = Some(Drag {
                connected_before,
                before: k,
                handle,
                start: point,
            });
            dense.gesture_active = true;
        } else {
            state.status = Some("Loading adjoining curves; drag again when ready".into());
        }
        return;
    }
    if state.connecting {
        state.status =
            Some("Click a road point; split a curve first to create a connection point".into());
        return;
    }
    let mut nearest: Option<(f32, RoadSpanId, RoadId)> = None;
    for e in dense.roads.entries.values() {
        let Some(RoadSourceRecord::Span(s)) = &e.current else {
            continue;
        };
        let Some(RoadSourceRecord::Road(r)) = dense.roads.get(RoadRecordKey::Road(s.road)) else {
            continue;
        };
        if r.space != space {
            continue;
        }
        let Some(span) = commands::span(&dense.roads, s.id) else {
            continue;
        };
        let controls = span.control_points(input.origin.cell(), size);
        let mut last = None;
        for i in 0..=48 {
            let point = RoadPoint::from_relative(
                input.origin.cell(),
                cubic(controls, i as f64 / 48.0),
                size as f64,
            )
            .ok();
            let screen = point
                .and_then(|p| project_point(&input, p, size))
                .and_then(|p| input.camera.0.world_to_viewport(input.camera.1, p).ok());
            if let (Some(a), Some(b)) = (last, screen) {
                let distance = distance_to_segment(cursor, a, b);
                if distance < 10.0 && nearest.is_none_or(|v| distance < v.0) {
                    nearest = Some((distance, s.id, s.road));
                }
            }
            last = screen;
        }
    }
    if let Some((_, id, road)) = nearest {
        state.span = Some(id);
        state.road = Some(road);
        state.knot = None;
    }
}
fn update_drag(
    state: &mut RoadToolState,
    dense: &mut DenseDomainWorkingSets,
    history: &mut EditorHistory,
    point: RoadPoint,
    size: f32,
    released: bool,
) {
    // Apply the final cursor position before recording the released gesture.
    let Some(drag) = &state.drag else { return };
    let result = drag_change(drag, point, size, &dense.roads)
        .and_then(|change| {
            if drag.handle == Handle::Point {
                junctions::move_point(&dense.roads, change)
            } else {
                Ok(vec![change])
            }
        })
        .and_then(|changes| dense.roads.apply(&changes));
    state.status = result.err();
    if released {
        finish(state, dense, history, false);
    }
}
fn drag_change(
    drag: &Drag,
    point: RoadPoint,
    size: f32,
    working: &working::RoadWorkingSet,
) -> Result<RoadChange, String> {
    let mut next = drag.before.clone();
    let now = point.relative_to(drag.start.cell, size as f64);
    let delta = [now[0] - drag.start.local[0], now[1] - drag.start.local[1]];
    match drag.handle {
        Handle::Point => {
            let p = next.knot.position;
            next.knot.position = match RoadPoint::from_relative(
                p.cell,
                [p.local[0] + delta[0], p.local[1] + delta[1]],
                size as f64,
            ) {
                Ok(p) => p,
                Err(e) => {
                    return Err(e.to_string());
                }
            };
        }
        Handle::Incoming | Handle::Outgoing => {
            let (own, other) = if drag.handle == Handle::Incoming {
                (&mut next.knot.incoming, &mut next.knot.outgoing)
            } else {
                (&mut next.knot.outgoing, &mut next.knot.incoming)
            };
            own[0] += delta[0];
            own[1] += delta[1];
            let len = own[0].hypot(own[1]);
            let opposite = other[0].hypot(other[1]);
            if len < 0.05 {
                return Err("Keep the curve handle longer than 5 cm".into());
            }
            if opposite > 0.0 {
                *other = own.map(|v| -v / len * opposite);
            }
        }
        Handle::Width => {
            let n = normal(&next.knot);
            next.knot.width =
                (next.knot.width + 2.0 * (delta[0] * n[0] + delta[1] * n[1]) as f32).min(64.0);
        }
    }
    let Some(RoadSourceRecord::Road(r)) = working.get(RoadRecordKey::Road(next.road)) else {
        return Err("The road reference is not loaded".into());
    };
    let Some(RoadSourceRecord::Profile(p)) = working.get(RoadRecordKey::Profile(r.road.profile))
    else {
        return Err("The road reference is not loaded".into());
    };
    next.knot.width = next.knot.width.max(p.minimum_width());
    next.knot.validate(size, p).map_err(|e| e.to_string())?;
    if junctions::at(working, next.knot.id).is_some_and(|j| next.knot.width > j.junction.radius) {
        return Err("Increase the junction blend radius before widening this point".into());
    }
    Ok(RoadChange {
        key: RoadRecordKey::Knot(next.knot.id),
        record: Some(RoadSourceRecord::Knot(next)),
    })
}
fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let d = b - a;
    let t = ((p - a).dot(d) / d.length_squared().max(0.0001)).clamp(0.0, 1.0);
    p.distance(a + t * d)
}
fn normal(k: &RoadKnot) -> [f64; 2] {
    let d = if k.outgoing[0].hypot(k.outgoing[1]) > 0.01 {
        k.outgoing
    } else {
        k.incoming.map(|v| -v)
    };
    let len = d[0].hypot(d[1]).max(0.001);
    [-d[1] / len, d[0] / len]
}
fn handles(k: &RoadKnot, selected: bool, size: f32) -> Vec<(Handle, RoadPoint)> {
    let mut result = vec![(Handle::Point, k.position)];
    if selected {
        for (h, d) in [
            (Handle::Incoming, k.incoming),
            (Handle::Outgoing, k.outgoing),
            (Handle::Width, normal(k).map(|v| v * k.width as f64 / 2.0)),
        ] {
            if d[0].hypot(d[1]) < 0.01 {
                continue;
            }
            if let Ok(p) = RoadPoint::from_relative(
                k.position.cell,
                [k.position.local[0] + d[0], k.position.local[1] + d[1]],
                size as f64,
            ) {
                result.push((h, p));
            }
        }
    }
    result
}
fn project_point(input: &Input, p: RoadPoint, size: f32) -> Option<Vec3> {
    let (_, terrain) = input
        .terrain
        .iter()
        .find(|(_, s)| Some(s.key.space) == input.origin.space() && s.key.cell == p.cell)?;
    let y = terrain
        .heightfield
        .sample(p.local.map(|v| v as f32), size)
        .height;
    let local = p.relative_to(input.origin.cell(), size as f64);
    Some(Vec3::new(local[0] as f32, y + 0.12, local[1] as f32))
}
fn cubic(p: [[f64; 2]; 4], t: f64) -> [f64; 2] {
    let u = 1.0 - t;
    [0, 1].map(|i| {
        u * u * u * p[0][i]
            + 3.0 * u * u * t * p[1][i]
            + 3.0 * u * t * t * p[2][i]
            + t * t * t * p[3][i]
    })
}
fn cross(g: &mut Gizmos<EditorOverlayGizmos>, p: Vec3, color: Color, r: f32) {
    g.line(p - Vec3::X * r, p + Vec3::X * r, color);
    g.line(p - Vec3::Z * r, p + Vec3::Z * r, color);
    g.line(p - Vec3::Y * r, p + Vec3::Y * r, color);
}
pub(super) fn draw(
    input: Input,
    dense: Res<DenseDomainWorkingSets>,
    state: Res<RoadToolState>,
    mut gizmos: Gizmos<EditorOverlayGizmos>,
) {
    if !active(&input) {
        return;
    }
    let Some(space) = input.origin.space() else {
        return;
    };
    let Some(size) = dense.definition(space).map(|d| d.cell_size) else {
        return;
    };
    let cyan = Color::srgb(0.15, 0.85, 1.0);
    let orange = Color::srgb(1.0, 0.55, 0.1);
    for e in dense.roads.entries.values() {
        match &e.current {
            Some(RoadSourceRecord::Junction(j)) if j.space == space => {
                let color = Color::srgb(1.0, 0.35, 0.8);
                if let Some(p) = project_point(&input, j.junction.position, size) {
                    cross(&mut gizmos, p, color, 0.28);
                }
                let mut previous = None;
                for i in 0..=48 {
                    let angle = std::f64::consts::TAU * i as f64 / 48.0;
                    let p = RoadPoint::from_relative(
                        j.junction.position.cell,
                        [
                            j.junction.position.local[0]
                                + angle.cos() * f64::from(j.junction.radius),
                            j.junction.position.local[1]
                                + angle.sin() * f64::from(j.junction.radius),
                        ],
                        f64::from(size),
                    )
                    .ok()
                    .and_then(|p| project_point(&input, p, size));
                    if let (Some(a), Some(b)) = (previous, p) {
                        gizmos.line(a, b, color);
                    }
                    previous = p;
                }
            }
            Some(RoadSourceRecord::Span(s)) => {
                let Some(RoadSourceRecord::Road(r)) = dense.roads.get(RoadRecordKey::Road(s.road))
                else {
                    continue;
                };
                if r.space != space {
                    continue;
                }
                let Some(span) = commands::span(&dense.roads, s.id) else {
                    continue;
                };
                let p = span.control_points(input.origin.cell(), size);
                let mut last = None;
                let color = if state.span == Some(s.id) {
                    orange
                } else if state.road == Some(s.road) {
                    cyan
                } else {
                    Color::srgb(0.85, 0.85, 0.55)
                };
                for i in 0..=48 {
                    let next = RoadPoint::from_relative(
                        input.origin.cell(),
                        cubic(p, i as f64 / 48.0),
                        size as f64,
                    )
                    .ok()
                    .and_then(|p| project_point(&input, p, size));
                    if let (Some(a), Some(b)) = (last, next) {
                        gizmos.line(a, b, color);
                    }
                    last = next;
                }
            }
            Some(RoadSourceRecord::Knot(k)) => {
                let Some(RoadSourceRecord::Road(r)) = dense.roads.get(RoadRecordKey::Road(k.road))
                else {
                    continue;
                };
                if r.space != space {
                    continue;
                }
                let center = project_point(&input, k.knot.position, size);
                for (handle, p) in handles(&k.knot, state.knot == Some(k.knot.id), size) {
                    if let Some(p) = project_point(&input, p, size) {
                        let color = if handle == Handle::Width {
                            orange
                        } else {
                            cyan
                        };
                        cross(
                            &mut gizmos,
                            p,
                            color,
                            if handle == Handle::Point { 0.16 } else { 0.12 },
                        );
                        if handle != Handle::Point
                            && let Some(a) = center
                        {
                            gizmos.line(a, p, color);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let first = state.first.or_else(|| {
        if state.branching || state.extending {
            state
                .knot
                .and_then(|id| match dense.roads.get(RoadRecordKey::Knot(id)) {
                    Some(RoadSourceRecord::Knot(k)) => Some(k.knot.position),
                    _ => None,
                })
        } else {
            None
        }
    });
    if let Some(first) = first.and_then(|p| project_point(&input, p, size)) {
        cross(&mut gizmos, first, orange, 0.2);
        if let Some(last) = state.hover.and_then(|p| project_point(&input, p, size)) {
            gizmos.line(first, last, orange);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::EditorObjectWorkingSet;
    #[test]
    fn release_uses_last_ground_hit_and_one_undo_restores_the_start() {
        let mut dense = DenseDomainWorkingSets::default();
        let style = commands::default_profile(
            environment::fixtures::DRY_GROUND,
            environment::ChannelId([1; 16]),
        );
        dense
            .roads
            .apply(&[RoadChange {
                key: RoadRecordKey::Profile(style.id),
                record: Some(RoadSourceRecord::Profile(style.clone())),
            }])
            .unwrap();
        let start = RoadPoint::from_relative(CellCoord::ZERO, [1.0, 1.0], 8.0).unwrap();
        let end = RoadPoint::from_relative(CellCoord::ZERO, [12.0, 1.0], 8.0).unwrap();
        let (_, id, changes) =
            commands::create(&dense.roads, WorldSpaceId(1), 8.0, style.id, start, end).unwrap();
        dense.roads.apply(&changes).unwrap();
        let RoadSourceRecord::Knot(before) =
            dense.roads.get(RoadRecordKey::Knot(id)).unwrap().clone()
        else {
            panic!()
        };
        let mut state = RoadToolState {
            drag: Some(Drag {
                before: before.clone(),
                connected_before: vec![],
                handle: Handle::Point,
                start: end,
            }),
            ..Default::default()
        };
        dense.gesture_active = true;
        let mut history = EditorHistory::default();
        let hit = RoadPoint::from_relative(CellCoord::ZERO, [15.0, 3.0], 8.0).unwrap();
        update_drag(&mut state, &mut dense, &mut history, hit, 8.0, true);
        assert!(!dense.gesture_active);
        assert!(state.drag.is_none());
        let Some(RoadSourceRecord::Knot(after)) = dense.roads.get(RoadRecordKey::Knot(id)) else {
            panic!()
        };
        assert_eq!(after.knot.position, hit);
        let mut objects = EditorObjectWorkingSet::default();
        assert!(history.undo(&mut objects, &mut dense));
        assert_eq!(
            dense.roads.get(RoadRecordKey::Knot(id)),
            Some(&RoadSourceRecord::Knot(before))
        );
        assert!(!history.undo(&mut objects, &mut dense));
    }
    #[test]
    fn cancelling_a_junction_drag_restores_all_members_without_an_undo_entry() {
        let mut dense = DenseDomainWorkingSets::default();
        let style = commands::default_profile(
            environment::fixtures::DRY_GROUND,
            environment::ChannelId([1; 16]),
        );
        dense
            .roads
            .apply(&[RoadChange {
                key: RoadRecordKey::Profile(style.id),
                record: Some(RoadSourceRecord::Profile(style.clone())),
            }])
            .unwrap();
        let point = |x, z| RoadPoint::from_relative(CellCoord::ZERO, [x, z], 8.0).unwrap();
        let (_, id, changes) = commands::create(
            &dense.roads,
            WorldSpaceId(1),
            8.0,
            style.id,
            point(-12.0, 0.0),
            point(0.0, 0.0),
        )
        .unwrap();
        dense.roads.apply(&changes).unwrap();
        let (_, _, changes) = junctions::branch(&dense.roads, id, 8.0, point(0.0, 12.0)).unwrap();
        dense.roads.apply(&changes).unwrap();
        let initial = dense.roads.writes();
        let RoadSourceRecord::Knot(before) =
            dense.roads.get(RoadRecordKey::Knot(id)).unwrap().clone()
        else {
            panic!()
        };
        let mut state = RoadToolState {
            drag: Some(Drag {
                before,
                connected_before: junctions::drag_before(&dense.roads, id),
                handle: Handle::Point,
                start: point(0.0, 0.0),
            }),
            ..Default::default()
        };
        let mut history = EditorHistory::default();
        dense.gesture_active = true;
        update_drag(
            &mut state,
            &mut dense,
            &mut history,
            point(2.0, 1.0),
            8.0,
            false,
        );
        assert_ne!(dense.roads.writes(), initial);
        finish(&mut state, &mut dense, &mut history, true);
        assert_eq!(dense.roads.writes(), initial);
        assert!(!history.undo(&mut EditorObjectWorkingSet::default(), &mut dense));
    }
}
