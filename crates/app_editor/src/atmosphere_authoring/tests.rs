use super::*;
use crate::editing::EditorObjectWorkingSet;

#[test]
fn displaying_a_running_clock_does_not_pause_or_quantize_it() {
    let context = egui::Context::default();
    let mut controls = Controls::new(&AtmosphereProfile::default());
    controls.phase = 0.99999;
    controls.playing = true;
    controls.clouds_playing = true;
    controls.cloud_speed = 10.;
    for _ in 0..120 {
        let phase = controls.phase;
        let _ = context.run_ui(Default::default(), |ui| {
            preview_hour(ui, &mut controls);
        });
        assert!(controls.playing, "rendering an idle slider stopped Play");
        assert_eq!(controls.phase, phase);
        controls.advance(1. / 60., 1200.);
    }
    assert!((controls.phase - 0.0333233).abs() < 0.00001);
    assert!((controls.cloud_seconds - 20.).abs() < 0.00001);
    controls.playing = false;
    let phase = controls.phase;
    controls.advance(1., 1200.);
    assert_eq!(controls.phase, phase);
    assert!((controls.cloud_seconds - 30.).abs() < 0.00001);
    controls.clouds_playing = false;
    controls.playing = true;
    controls.advance(1., 1200.);
    assert!(controls.phase > phase);
    assert!((controls.cloud_seconds - 30.).abs() < 0.00001);
}

#[test]
fn preview_is_transient_and_color_drag_is_one_undo_across_a_save() {
    let mut dense = DenseDomainWorkingSets::default();
    let source = world_db::WorldSpaceRecord {
        id: WorldSpaceId(1),
        name: "test".into(),
        cell_size: 32.0,
        minimum_y: 0.0,
        maximum_y: 1.0,
        atmosphere: Default::default(),
        atmosphere_revision: 1,
    };
    dense.atmospheres.pin(&source);
    let mut controls = Controls::new(&source.atmosphere);
    controls.phase = 0.75;
    controls.playing = true;
    controls.clouds_playing = true;
    controls.cloud_seconds = 123.;
    assert_eq!(dense.dirty_count(), 0);
    let before = source.atmosphere.clone();
    dense.atmospheres.gesture = Some((source.id, before.clone()));
    for strength in [1000.0, 2000.0, 3500.0] {
        dense
            .atmospheres
            .entries
            .get_mut(&source.id)
            .unwrap()
            .current
            .phases[1]
            .ambient_lux = strength;
    }
    assert!(dense.atmospheres.journal().is_empty());
    let (id, before, after) = dense.atmospheres.finish_gesture().unwrap();
    let mut history = EditorHistory::default();
    history.record_atmosphere(id, before.clone(), after.clone());
    assert_eq!(history.undo_len(), 1);
    assert_eq!(dense.atmospheres.journal().len(), 1);
    dense
        .atmospheres
        .complete(Ok(world_db::AtmosphereWriteResult::Committed(vec![(
            id,
            2,
            after.clone(),
        )])));
    assert_eq!(dense.dirty_count(), 0);
    let mut objects = EditorObjectWorkingSet::default();
    assert!(history.undo(&mut objects, &mut dense));
    assert_eq!(dense.dirty_count(), 1);
    assert_eq!(dense.atmospheres.entries[&id].current, before);
    assert!(history.redo(&mut objects, &mut dense));
    assert_eq!(dense.dirty_count(), 0);
    assert_eq!(controls.phase, 0.75);
    assert_eq!(controls.cloud_seconds, 123.);
}
#[test]
fn recovery_and_conflict_keep_the_local_profile() {
    let mut set = working::WorkingSet::default();
    let mut changed = AtmosphereProfile::default();
    changed.exposure_ev100 = 12.0;
    changed.clouds = world::clouds::CloudSettings::overcast();
    let id = WorldSpaceId(1);
    let snapshot = working::Snapshot {
        space: id,
        revision: 1,
        base: Default::default(),
        current: changed.clone(),
    };
    let encoded = ron::to_string(&vec![snapshot]).unwrap();
    assert_eq!(set.restore(ron::from_str(&encoded).unwrap()), 1);
    assert!(!set.complete(Ok(world_db::AtmosphereWriteResult::Conflict {
        space: id,
        revision: 2,
        actual: AtmosphereProfile::default()
    })));
    assert_eq!(set.entries[&id].current, changed);
    set.resolve_conflict(true);
    assert_eq!(set.entries[&id].revision, 2);
    assert_eq!(set.dirty_count(), 1);
}
