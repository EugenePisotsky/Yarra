use super::*;
use crate::editing::EditorObjectWorkingSet;
use world_db::{GameplayAreasRecord, GameplayAreasWriteResult};

fn area(name: &str, points: &[[f64; 2]]) -> GameplayArea {
    GameplayArea {
        name: name.into(),
        space: WorldSpaceId(1),
        points: points.to_vec(),
        height: None,
    }
}
fn yard() -> GameplayArea {
    area("yard", &[[0., 0.], [20., 0.], [20., 20.], [0., 20.]])
}
fn loaded(areas: Vec<GameplayArea>) -> DenseDomainWorkingSets {
    let mut dense = DenseDomainWorkingSets::default();
    dense.areas.pin(&GameplayAreasRecord {
        revision: 1,
        areas: areas.into(),
    });
    dense
}

#[test]
fn a_drag_is_one_undo_step_that_survives_a_save() {
    let mut dense = loaded(vec![yard()]);
    let mut history = EditorHistory::default();
    let mut state = AreaToolState::default();
    assert_eq!(dense.dirty_count(), 0);
    for x in [21., 24., 30.] {
        let moved = with_area(dense.areas.areas(), 0, |area| area.points[1] = [x, 0.]);
        assert!(state.edit(&mut dense, moved));
        assert!(dense.gesture_active);
    }
    assert_eq!(history.undo_len(), 0);
    state.finish(&mut dense, &mut history, false);
    assert!(!dense.gesture_active);
    assert_eq!((history.undo_len(), dense.dirty_count()), (1, 1));

    let saved = dense.areas.areas().clone();
    assert!(dense.areas.complete(Ok(GameplayAreasWriteResult::Committed(
        GameplayAreasRecord {
            revision: 2,
            areas: saved.clone(),
        }
    ))));
    assert_eq!(dense.dirty_count(), 0);
    let mut objects = EditorObjectWorkingSet::default();
    assert!(history.undo(&mut objects, &mut dense));
    assert_eq!(dense.areas.areas()[0], yard());
    assert_eq!(dense.dirty_count(), 1);
    assert!(history.redo(&mut objects, &mut dense));
    assert_eq!(dense.areas.areas(), &saved);
    assert_eq!(dense.dirty_count(), 0);
}

#[test]
fn a_cancelled_gesture_and_a_refused_edit_leave_the_set_alone() {
    let mut dense = loaded(vec![yard()]);
    let mut history = EditorHistory::default();
    let mut state = AreaToolState::default();
    let moved = with_area(dense.areas.areas(), 0, |area| area.points[0] = [-5., -5.]);
    assert!(state.edit(&mut dense, moved));
    state.finish(&mut dense, &mut history, true);
    assert_eq!(dense.areas.areas()[0], yard());
    assert_eq!((history.undo_len(), dense.dirty_count()), (0, 0));

    // A name gameplay content could not refer to, and a name used twice.
    let renamed = with_area(dense.areas.areas(), 0, |area| area.name = "The Yard".into());
    assert!(!state.commit(&mut dense, &mut history, renamed));
    assert!(state.status.as_deref().unwrap().contains("lowercase"));
    assert!(!state.commit(&mut dense, &mut history, vec![yard(), yard()]));
    assert_eq!(dense.areas.areas().len(), 1);
    assert_eq!(history.undo_len(), 0);

    // Nothing can be edited before the project's record has been read.
    let mut unloaded = DenseDomainWorkingSets::default();
    assert!(!state.commit(&mut unloaded, &mut history, vec![yard()]));
}

#[test]
fn unsaved_work_is_recovered_and_kept_through_a_conflict() {
    let mut dense = loaded(vec![yard()]);
    let mut history = EditorHistory::default();
    let mut state = AreaToolState::default();
    let gate = area("guard/gate_post", &[[2., 2.], [4., 2.], [3., 5.]]);
    assert!(state.commit(&mut dense, &mut history, vec![yard(), gate.clone()]));
    let journal = ron::to_string(&dense.areas.journal().unwrap()).unwrap();

    // A restart: the journal is read before or after the project record.
    let mut recovered = working::WorkingSet::default();
    assert!(recovered.restore(ron::from_str(&journal).unwrap()));
    recovered.pin(&GameplayAreasRecord {
        revision: 1,
        areas: vec![yard()].into(),
    });
    assert_eq!(recovered.areas()[1], gate);
    assert_eq!(recovered.dirty_count(), 1);
    // Clean state has nothing to journal.
    assert!(loaded(vec![yard()]).areas.journal().is_none());

    // Someone else saved first: the local version stays until the user decides.
    let theirs = GameplayAreasRecord {
        revision: 5,
        areas: vec![area("camp", &[[0., 0.], [1., 0.], [0., 1.]])].into(),
    };
    assert!(!recovered.complete(Ok(GameplayAreasWriteResult::Conflict(theirs.clone()))));
    assert!(recovered.apply(vec![yard()].into()).is_err());
    recovered.resolve_conflict(true);
    assert_eq!((recovered.areas().len(), recovered.dirty_count()), (2, 1));
    assert!(!recovered.complete(Ok(GameplayAreasWriteResult::Conflict(theirs.clone()))));
    recovered.resolve_conflict(false);
    assert_eq!(recovered.areas(), &theirs.areas);
    assert_eq!(recovered.dirty_count(), 0);
}

#[test]
fn picking_prefers_the_nearest_corner_the_nearest_edge_and_the_inner_area() {
    let corners = [
        Some(Vec2::new(100., 100.)),
        Some(Vec2::new(200., 100.)),
        None, // behind the camera
        Some(Vec2::new(100., 200.)),
    ];
    assert_eq!(
        viewport::pick_corner(&corners, Vec2::new(195., 104.)),
        Some(1)
    );
    assert_eq!(viewport::pick_corner(&corners, Vec2::new(150., 100.)), None);
    assert_eq!(
        viewport::pick_edge(&corners, Vec2::new(150., 104.)),
        Some(0)
    );
    assert_eq!(viewport::pick_edge(&corners, Vec2::new(96., 150.)), Some(3));
    // Edges to the unseen corner cannot be picked.
    assert_eq!(viewport::pick_edge(&corners, Vec2::new(200., 150.)), None);

    let mut other_world = area("other", &[[0., 0.], [9., 0.], [9., 9.], [0., 9.]]);
    other_world.space = WorldSpaceId(2);
    let areas = [
        yard(),
        area("yard/well", &[[8., 8.], [12., 8.], [12., 12.], [8., 12.]]),
        other_world,
    ];
    let at = |point| viewport::area_at(&areas, WorldSpaceId(1), point).map(|a| a.name.as_str());
    assert_eq!(at([10., 10.]), Some("yard/well"));
    assert_eq!(at([2., 2.]), Some("yard"));
    assert_eq!(at([40., 2.]), None);
    assert_eq!(unused_name(&areas), "area-1");
    assert_eq!(unused_name(&[area("area-1", &yard().points)]), "area-2");
}

/// One frame of the tool with the pointer over a ground position.
fn frame(
    state: &mut AreaToolState,
    dense: &mut DenseDomainWorkingSets,
    history: &mut EditorHistory,
    gesture: viewport::Gesture,
) {
    viewport::act(state, dense, history, WorldSpaceId(1), gesture);
}
fn click(hit: [f64; 2]) -> viewport::Gesture {
    viewport::Gesture {
        hit: Some(hit),
        click: true,
        held: true,
        ..Default::default()
    }
}

#[test]
fn a_shape_is_drawn_corner_by_corner_and_closed_into_a_named_area() {
    let mut dense = loaded(vec![]);
    let mut history = EditorHistory::default();
    let mut state = AreaToolState {
        draft: Some(vec![]),
        ..Default::default()
    };
    let enter = viewport::Gesture {
        close: true,
        ..Default::default()
    };
    for point in [[0., 0.], [10., 0.]] {
        frame(&mut state, &mut dense, &mut history, click(point));
        // Two corners are not a shape yet.
        frame(&mut state, &mut dense, &mut history, enter);
    }
    // A misplaced third corner is taken back before the shape is closed.
    frame(&mut state, &mut dense, &mut history, click([99., 99.]));
    let back = viewport::Gesture {
        back: true,
        ..Default::default()
    };
    frame(&mut state, &mut dense, &mut history, back);
    frame(&mut state, &mut dense, &mut history, enter);
    assert_eq!(state.draft.as_ref().map(Vec::len), Some(2));
    assert!(dense.areas.areas().is_empty());

    frame(&mut state, &mut dense, &mut history, click([10., 10.]));
    frame(&mut state, &mut dense, &mut history, click([0., 10.]));
    let on_first = viewport::Gesture {
        on_first: true,
        ..click([0.2, 0.1])
    };
    frame(&mut state, &mut dense, &mut history, on_first);
    assert!(state.draft.is_none());
    assert_eq!(state.selected.as_deref(), Some("area-1"));
    assert_eq!(
        dense.areas.areas()[0],
        area("area-1", &[[0., 0.], [10., 0.], [10., 10.], [0., 10.]])
    );
    // Drawing a whole area is one step to undo.
    assert_eq!(history.undo_len(), 1);
    assert!(history.undo(&mut EditorObjectWorkingSet::default(), &mut dense));
    assert!(dense.areas.areas().is_empty());
}

#[test]
fn corners_are_dragged_added_on_an_edge_and_removed() {
    let mut dense = loaded(vec![yard()]);
    let mut history = EditorHistory::default();
    let mut state = AreaToolState::default();
    // A click on the ground selects what is there; elsewhere it deselects.
    frame(&mut state, &mut dense, &mut history, click([5., 5.]));
    assert_eq!(state.selected.as_deref(), Some("yard"));
    frame(&mut state, &mut dense, &mut history, click([50., 5.]));
    assert_eq!(state.selected, None);
    frame(&mut state, &mut dense, &mut history, click([5., 5.]));

    // Grab corner 1 slightly off its centre: it keeps that offset while it moves.
    let grab = viewport::Gesture {
        corner: Some(1),
        ..click([19.5, 0.5])
    };
    frame(&mut state, &mut dense, &mut history, grab);
    assert_eq!(dense.areas.areas()[0], yard());
    let drag = |hit| viewport::Gesture {
        hit: Some(hit),
        held: true,
        ..Default::default()
    };
    frame(&mut state, &mut dense, &mut history, drag([24.5, 0.5]));
    frame(&mut state, &mut dense, &mut history, drag([29.5, 2.5]));
    assert!(dense.gesture_active);
    let release = viewport::Gesture {
        hit: Some([29.5, 2.5]),
        ..Default::default()
    };
    frame(&mut state, &mut dense, &mut history, release);
    assert!(!dense.gesture_active && state.dragging.is_none());
    assert_eq!(dense.areas.areas()[0].points[1], [30., 2.]);
    assert_eq!(history.undo_len(), 1);

    // A click on the edge after corner 2 puts a corner there, already in hand.
    let on_edge = viewport::Gesture {
        edge: Some(2),
        ..click([10., 20.])
    };
    frame(&mut state, &mut dense, &mut history, on_edge);
    frame(&mut state, &mut dense, &mut history, drag([10., 26.]));
    frame(
        &mut state,
        &mut dense,
        &mut history,
        viewport::Gesture::default(),
    );
    assert_eq!(dense.areas.areas()[0].points.len(), 5);
    assert_eq!(dense.areas.areas()[0].points[3], [10., 26.]);
    assert_eq!((history.undo_len(), state.corner), (2, Some(3)));

    // Delete removes the corner picked last, down to a triangle and no further.
    let back = viewport::Gesture {
        back: true,
        ..Default::default()
    };
    frame(&mut state, &mut dense, &mut history, back);
    assert_eq!(dense.areas.areas()[0].points.len(), 4);
    assert_eq!(state.corner, None);
    state.corner = Some(0);
    frame(&mut state, &mut dense, &mut history, back);
    state.corner = Some(0);
    frame(&mut state, &mut dense, &mut history, back);
    assert_eq!(dense.areas.areas()[0].points.len(), 3);

    // A drag that is cancelled leaves no trace.
    let steps = history.undo_len();
    let before = dense.areas.areas().clone();
    let grab = viewport::Gesture {
        corner: Some(0),
        ..click([0., 0.])
    };
    frame(&mut state, &mut dense, &mut history, grab);
    frame(&mut state, &mut dense, &mut history, drag([-8., -8.]));
    state.finish(&mut dense, &mut history, true);
    assert_eq!(dense.areas.areas(), &before);
    assert_eq!(history.undo_len(), steps);
}

#[test]
fn leaving_the_world_workspace_mid_gesture_ends_it_so_saving_can_go_on() {
    let mut app = App::new();
    app.add_plugins(bevy::state::app::StatesPlugin)
        .init_state::<EditorWorkspace>()
        .insert_resource(loaded(vec![yard()]))
        .init_resource::<EditorHistory>()
        .init_resource::<AreaToolState>()
        .add_systems(Update, end_gesture_away);
    let world = app.world_mut();
    let moved = with_area(
        world.resource::<DenseDomainWorkingSets>().areas.areas(),
        0,
        |a| {
            a.points[1] = [30., 0.];
        },
    );
    world.resource_scope(|world, mut state: Mut<AreaToolState>| {
        assert!(state.edit(&mut world.resource_mut::<DenseDomainWorkingSets>(), moved));
    });
    // Still in the World workspace, the window ends it; nothing else does.
    app.update();
    assert!(
        app.world()
            .resource::<DenseDomainWorkingSets>()
            .gesture_active
    );
    app.world_mut()
        .resource_mut::<NextState<EditorWorkspace>>()
        .set(EditorWorkspace::Presets);
    app.update();
    let world = app.world();
    assert!(!world.resource::<DenseDomainWorkingSets>().gesture_active);
    assert_eq!(world.resource::<EditorHistory>().undo_len(), 1);
    assert_eq!(
        world.resource::<DenseDomainWorkingSets>().areas.areas()[0].points[1],
        [30., 0.]
    );
}
