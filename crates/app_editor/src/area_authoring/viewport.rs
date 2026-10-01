//! The Areas tool in the viewport: drawing a new shape corner by corner, selecting, moving
//! and adding corners, and the outlines that show where the areas are.
use super::*;
use crate::{shell::EditorInputCapture, workspaces::world::EditorOverlayGizmos};
use bevy::{ecs::system::SystemParam, window::PrimaryWindow};
use engine::{StreamedTerrainSurface, WorldViewCamera};
use world::MAX_GAMEPLAY_AREA_POINTS;

/// Pointer distances, in logical pixels, within which a corner or an edge is picked.
const CORNER_PICK: f32 = 12.0;
const EDGE_PICK: f32 = 10.0;
/// Outlines are drawn for areas within this distance of the camera.
const DRAW_RANGE: f64 = 800.0;
/// How far above the ground outlines float.
const LIFT: f32 = 0.15;

#[derive(SystemParam)]
pub(super) struct Input<'w, 's> {
    window: Single<'w, 's, &'static Window, With<PrimaryWindow>>,
    camera: Single<'w, 's, (&'static Camera, &'static GlobalTransform), With<WorldViewCamera>>,
    terrain: Query<'w, 's, (Entity, &'static StreamedTerrainSurface)>,
    origin: Res<'w, WorldOrigin>,
    catalog: Res<'w, WorldCatalog>,
    workspace: Res<'w, State<EditorWorkspace>>,
    mode: Res<'w, PreviewModeState>,
    tools: Res<'w, EditorToolRegistry>,
    capture: Res<'w, EditorInputCapture>,
    buttons: Res<'w, ButtonInput<MouseButton>>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    publication: Res<'w, RuntimePublicationState>,
    save: Res<'w, EditorSaveCoordinator>,
    project: Res<'w, ProjectEditorStore>,
}

/// A shape placed in the view: its corners, the line through them following the ground, and
/// the lowest and highest ground under the corners.
struct Trace {
    corners: Vec<Vec3>,
    line: Vec<Vec3>,
    ground: [f32; 2],
}

impl Input<'_, '_> {
    fn active(&self) -> bool {
        *self.workspace.get() == EditorWorkspace::World
            && self.mode.active() == Some(EditorPreviewMode::Authoring)
            && tool_active(&self.tools)
    }

    fn ground(&self, render: Vec3) -> Option<f32> {
        engine::sample_resident_terrain_surface(
            &self.origin,
            self.terrain.iter().map(|(_, surface)| surface),
            [render.x, render.z],
        )
        .map(|sample| sample.height)
    }

    /// The world ground position under the pointer.
    fn pointed(&self, cursor: Vec2) -> Option<[f64; 2]> {
        let ray = self
            .camera
            .0
            .viewport_to_world(self.camera.1, cursor)
            .ok()?;
        let (_, hit) = engine::raycast_resident_terrain(&self.origin, self.terrain.iter(), ray)?;
        let (_, world) = self.origin.to_world(&self.catalog, hit)?;
        Some([world[0], world[2]])
    }

    fn screen(&self, position: Vec3) -> Option<Vec2> {
        self.camera
            .0
            .world_to_viewport(self.camera.1, position)
            .ok()
    }

    /// Places world ground points in the view. Terrain is only loaded near the camera, so a
    /// corner without ground under it borrows the average height of those that have it;
    /// a shape with no loaded corner is not placed at all.
    fn trace(&self, space: WorldSpaceId, points: &[[f64; 2]], closed: bool) -> Option<Trace> {
        let flat: Vec<Vec3> = points
            .iter()
            .map(|p| {
                self.origin
                    .to_render(&self.catalog, space, [p[0], 0., p[1]])
            })
            .collect::<Option<_>>()?;
        let heights: Vec<Option<f32>> = flat.iter().map(|p| self.ground(*p)).collect();
        let known: Vec<f32> = heights.iter().flatten().copied().collect();
        if known.is_empty() {
            return None;
        }
        let average = known.iter().sum::<f32>() / known.len() as f32;
        let ground = known
            .iter()
            .fold([f32::INFINITY, f32::NEG_INFINITY], |[low, high], h| {
                [low.min(*h), high.max(*h)]
            });
        let corners: Vec<Vec3> = flat
            .iter()
            .zip(&heights)
            .map(|(p, h)| p.with_y(h.unwrap_or(average) + LIFT))
            .collect();
        // Long edges are split so the line follows the ground between the corners.
        let mut line = Vec::new();
        let edges = if closed {
            corners.len()
        } else {
            corners.len().saturating_sub(1)
        };
        for i in 0..edges {
            let (a, b) = (corners[i], corners[(i + 1) % corners.len()]);
            let steps = ((a.xz().distance(b.xz()) / 4.0).ceil() as usize).clamp(1, 16);
            line.push(a);
            for step in 1..steps {
                let p = a.lerp(b, step as f32 / steps as f32);
                line.push(self.ground(p).map_or(p, |y| p.with_y(y + LIFT)));
            }
            if i + 1 == edges {
                line.push(b);
            }
        }
        if line.is_empty() {
            line.extend(&corners);
        }
        Some(Trace {
            corners,
            line,
            ground,
        })
    }

    /// Whether an area is close enough to the camera to be worth drawing.
    fn near(&self, area: &GameplayArea) -> bool {
        let Some((_, camera)) = self
            .origin
            .to_world(&self.catalog, self.camera.1.translation())
        else {
            return false;
        };
        let [low, high] = area.bounds();
        let away = |value: f64, low: f64, high: f64| (low - value).max(value - high).max(0.);
        away(camera[0], low[0], high[0]).hypot(away(camera[2], low[1], high[1])) < DRAW_RANGE
    }
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let d = b - a;
    let t = ((p - a).dot(d) / d.length_squared().max(0.0001)).clamp(0.0, 1.0);
    p.distance(a + t * d)
}

/// The corner within reach of the pointer, nearest first.
pub(super) fn pick_corner(corners: &[Option<Vec2>], cursor: Vec2) -> Option<usize> {
    corners
        .iter()
        .enumerate()
        .filter_map(|(i, corner)| Some((i, corner.as_ref()?.distance(cursor))))
        .filter(|(_, distance)| *distance < CORNER_PICK)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The edge within reach of the pointer, as the index of the corner it starts from.
pub(super) fn pick_edge(corners: &[Option<Vec2>], cursor: Vec2) -> Option<usize> {
    (0..corners.len())
        .filter_map(|i| {
            let (a, b) = (corners[i]?, corners[(i + 1) % corners.len()]?);
            Some((i, distance_to_segment(cursor, a, b)))
        })
        .filter(|(_, distance)| *distance < EDGE_PICK)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The smallest area of a world whose shape covers a ground position: the one on top when
/// areas are nested.
pub(super) fn area_at(
    areas: &[GameplayArea],
    space: WorldSpaceId,
    [x, z]: [f64; 2],
) -> Option<&GameplayArea> {
    let extent = |area: &GameplayArea| {
        let [low, high] = area.bounds();
        (high[0] - low[0]) * (high[1] - low[1])
    };
    areas
        .iter()
        .filter(|area| area.space == space && area.covers(x, z))
        .min_by(|a, b| extent(a).total_cmp(&extent(b)))
}

/// What the pointer and keys did this frame, already resolved against the view.
#[derive(Clone, Copy, Default)]
pub(super) struct Gesture {
    /// World ground position under the pointer.
    pub hit: Option<[f64; 2]>,
    /// The left button went down this frame, and whether it is down now.
    pub click: bool,
    pub held: bool,
    /// Enter: close the shape being drawn.
    pub close: bool,
    /// Backspace or Delete: take back the last corner drawn, or remove the picked one.
    pub back: bool,
    /// The corner and the edge of the selected area within reach of the pointer.
    pub corner: Option<usize>,
    pub edge: Option<usize>,
    /// The pointer is on the first corner of the shape being drawn.
    pub on_first: bool,
}

/// Carries out one frame of the tool: dragging, drawing, corner edits and selection.
pub(super) fn act(
    state: &mut AreaToolState,
    dense: &mut DenseDomainWorkingSets,
    history: &mut EditorHistory,
    space: WorldSpaceId,
    gesture: Gesture,
) {
    let areas = dense.areas.areas().clone();
    state.hover = gesture.hit;

    if let Some((corner, offset)) = state.dragging {
        if let (Some(hit), Some(index)) = (gesture.hit, state.selected_index(&areas)) {
            let point = [hit[0] + offset[0], hit[1] + offset[1]];
            if areas[index].points.get(corner).is_some_and(|p| *p != point) {
                let moved = with_area(&areas, index, |area| area.points[corner] = point);
                state.edit(dense, moved);
            }
        }
        if !gesture.held {
            state.finish(dense, history, false);
        }
        return;
    }

    if let Some(draft) = &mut state.draft {
        if gesture.back {
            draft.pop();
        }
        if draft.len() >= 3 && (gesture.close || gesture.click && gesture.on_first) {
            let name = unused_name(&areas);
            let mut next = areas.to_vec();
            next.push(GameplayArea {
                name: name.clone(),
                space,
                points: state.draft.take().unwrap_or_default(),
                height: None,
            });
            if state.commit(dense, history, next) {
                state.selected = Some(name);
                state.corner = None;
            }
        } else if gesture.click
            && let Some(point) = gesture.hit
        {
            if draft.len() < MAX_GAMEPLAY_AREA_POINTS {
                draft.push(point);
            } else {
                state.status = Some("An area has at most 64 corners".into());
            }
        }
        return;
    }

    let selected = state.selected_index(&areas);
    if gesture.back
        && let (Some(index), Some(corner)) = (selected, state.corner)
        && areas[index].points.len() > 3
        && corner < areas[index].points.len()
    {
        let fewer = with_area(&areas, index, |area| {
            area.points.remove(corner);
        });
        state.commit(dense, history, fewer);
        state.corner = None;
        return;
    }
    if !gesture.click {
        return;
    }
    if let Some(index) = selected {
        let area = &areas[index];
        if let Some(corner) = gesture.corner.filter(|&c| c < area.points.len()) {
            // Keep the corner where it is under the pointer instead of snapping to it.
            let at = area.points[corner];
            let offset = gesture
                .hit
                .map_or([0.; 2], |hit| [at[0] - hit[0], at[1] - hit[1]]);
            state.corner = Some(corner);
            state.dragging = Some((corner, offset));
            return;
        }
        if let (Some(after), Some(point)) = (gesture.edge, gesture.hit) {
            if area.points.len() >= MAX_GAMEPLAY_AREA_POINTS {
                state.status = Some("An area has at most 64 corners".into());
                return;
            }
            let more = with_area(&areas, index, |area| area.points.insert(after + 1, point));
            // The new corner is already in hand: adding it and placing it is one step.
            if state.edit(dense, more) {
                state.corner = Some(after + 1);
                state.dragging = Some((after + 1, [0.; 2]));
            }
            return;
        }
    }
    let picked = gesture
        .hit
        .and_then(|hit| area_at(&areas, space, hit))
        .map(|area| area.name.clone());
    if picked != state.selected {
        state.corner = None;
        state.selected = picked;
    }
}

pub(super) fn input(
    input: Input,
    mut dense: ResMut<DenseDomainWorkingSets>,
    mut history: ResMut<EditorHistory>,
    mut state: ResMut<AreaToolState>,
) {
    if let Some(space) = input.origin.space()
        && state.space != Some(space)
    {
        // Another world: what was selected or half drawn belongs to the one left behind.
        state.finish(&mut dense, &mut history, false);
        state.space = Some(space);
        state.selected = None;
        state.corner = None;
        state.draft = None;
    }
    let active = input.active();
    let enabled = active && input.window.focused;
    let typing = input.capture.wants_keyboard;
    let pressed = |key| !typing && input.keys.just_pressed(key);
    if enabled && pressed(KeyCode::Escape) {
        state.finish(&mut dense, &mut history, true);
        state.draft = None;
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
    if (!enabled || blocked) && state.dragging.is_some() {
        state.finish(&mut dense, &mut history, false);
    }
    if !active {
        state.hover = None;
        state.draft = None;
        return;
    }
    // A shape half drawn survives looking away from the editor or a save.
    if !enabled || blocked {
        return;
    }
    let Some(space) = input.origin.space() else {
        return;
    };
    let cursor = input.window.cursor_position();
    let mut gesture = Gesture {
        hit: cursor.and_then(|cursor| input.pointed(cursor)),
        click: input.buttons.just_pressed(MouseButton::Left),
        held: input.buttons.pressed(MouseButton::Left),
        close: pressed(KeyCode::Enter),
        back: pressed(KeyCode::Backspace) || pressed(KeyCode::Delete),
        ..default()
    };
    // What is within reach of the pointer only matters on the frame of a click.
    if let (true, Some(cursor)) = (gesture.click && state.dragging.is_none(), cursor) {
        if let (Some(draft), Some(space)) = (&state.draft, state.space) {
            gesture.on_first = draft.first().is_some_and(|first| {
                input
                    .trace(space, &[*first], false)
                    .and_then(|trace| input.screen(trace.corners[0]))
                    .is_some_and(|first| first.distance(cursor) < CORNER_PICK)
            });
        } else if let Some(index) = state.selected_index(dense.areas.areas())
            && let area = &dense.areas.areas()[index]
            && let Some(trace) = input.trace(area.space, &area.points, true)
        {
            let corners: Vec<Option<Vec2>> =
                trace.corners.iter().map(|p| input.screen(*p)).collect();
            gesture.corner = pick_corner(&corners, cursor);
            gesture.edge = pick_edge(&corners, cursor);
        }
    }
    act(&mut state, &mut dense, &mut history, space, gesture);
}

fn cross(gizmos: &mut Gizmos<EditorOverlayGizmos>, at: Vec3, color: Color, radius: f32) {
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        gizmos.line(at - axis * radius, at + axis * radius, color);
    }
}

pub(super) fn draw(
    input: Input,
    dense: Res<DenseDomainWorkingSets>,
    mut state: ResMut<AreaToolState>,
    mut gizmos: Gizmos<EditorOverlayGizmos>,
) {
    if !input.active() {
        return;
    }
    let Some(space) = input.origin.space() else {
        return;
    };
    let resting = Color::srgb(0.45, 0.95, 0.55);
    let chosen = Color::srgb(1.0, 0.55, 0.1);
    let mut ground = None;
    for area in dense.areas.areas().iter() {
        if area.space != space || !input.near(area) {
            continue;
        }
        let Some(trace) = input.trace(area.space, &area.points, true) else {
            continue;
        };
        let selected = state.selected.as_deref() == Some(area.name.as_str());
        let color = if selected { chosen } else { resting };
        gizmos.linestrip(trace.line.iter().copied(), color);
        if selected {
            ground = Some(trace.ground);
            for (i, corner) in trace.corners.iter().enumerate() {
                let picked = state.corner == Some(i);
                cross(
                    &mut gizmos,
                    *corner,
                    if picked { Color::WHITE } else { color },
                    if picked { 0.3 } else { 0.2 },
                );
            }
        }
        if let Some([low, high]) = area.height {
            // The height range as a fence: the outline at both heights, joined at the corners.
            let faded = color.with_alpha(0.45);
            for y in [low, high] {
                let ring = trace.corners.iter().chain(trace.corners.first());
                gizmos.linestrip(ring.map(|p| p.with_y(y)), faded);
            }
            for corner in &trace.corners {
                gizmos.line(corner.with_y(low), corner.with_y(high), faded);
            }
        }
    }
    if state.selected.is_some() {
        state.ground = ground.or(state.ground);
    } else {
        state.ground = None;
    }
    let Some(draft) = &state.draft else {
        return;
    };
    let mut points = draft.clone();
    points.extend(state.hover);
    if let Some(trace) = input.trace(space, &points, false) {
        gizmos.linestrip(trace.line.iter().copied(), chosen);
        for (i, corner) in trace.corners.iter().take(draft.len()).enumerate() {
            // The first corner is the one to click to close the shape.
            cross(
                &mut gizmos,
                *corner,
                chosen,
                if i == 0 { 0.35 } else { 0.2 },
            );
        }
        if draft.len() >= 2
            && let (Some(last), Some(first)) = (trace.corners.last(), trace.corners.first())
        {
            gizmos.line(*last, *first, chosen.with_alpha(0.35));
        }
    }
}
