//! Which meshes each view's instances come from: shadow LODs, cascade tests and hiding.
use super::*;
use crate::object_lod::ScreenSpaceLodVariant;
use bevy::{asset::uuid::Uuid, ecs::system::RunSystemOnce};

/// Three mesh LODs and an impostor, as streamed trees have.
fn tree_lod() -> ScreenSpaceLod {
    let variants = (0..4u8)
        .map(|lod| ScreenSpaceLodVariant {
            lod,
            scene: (lod < 3).then(Handle::default),
            minimum_screen_height: [0.5, 0.25, 0.1, 0.0][lod as usize],
        })
        .collect();
    ScreenSpaceLod::new(variants, 14.0)
}

fn casts(lod: &ScreenSpaceLod, bias: usize) -> Vec<(usize, i32)> {
    let mut casts = vec![(9, 9)];
    shadow_scenes(lod, bias, &mut casts);
    casts
}

#[test]
fn shadows_come_from_the_drawn_lod_or_a_coarser_mesh() {
    let mut lod = tree_lod();
    lod.set_timed(0, None);
    assert_eq!(casts(&lod, 0), [(0, 0)]);
    assert_eq!(casts(&lod, 1), [(1, 0)]);
    // Never coarser than the last mesh LOD; the impostor casts nothing.
    assert_eq!(casts(&lod, 5), [(2, 0)]);
    lod.set_timed(3, None);
    assert_eq!(casts(&lod, 0), []);
}

#[test]
fn fading_shadows_dither_unless_both_lods_cast_one_scene() {
    let mut lod = tree_lod();
    // Halfway from LOD0 to LOD1: level 8 fading out, -8 fading in.
    lod.set_timed(1, Some((0, 0.5)));
    assert_eq!(casts(&lod, 0), [(0, 8), (1, -8)]);
    assert_eq!(casts(&lod, 1), [(1, 8), (2, -8)]);
    // From LOD1 to LOD2 one LOD coarser, both cast LOD2: whole, so the shadow holds.
    lod.set_timed(2, Some((1, 0.5)));
    assert_eq!(casts(&lod, 1), [(2, 0)]);
    // Into the impostor only the outgoing mesh casts, dissolving.
    lod.set_timed(3, Some((2, 0.25)));
    assert_eq!(casts(&lod, 0), [(2, 4)]);
}

#[test]
fn objects_without_an_impostor_cast_as_entities() {
    let variants = (0..2u8)
        .map(|lod| ScreenSpaceLodVariant {
            lod,
            scene: Some(Handle::default()),
            minimum_screen_height: 0.0,
        })
        .collect();
    assert_eq!(casts(&ScreenSpaceLod::new(variants, 2.0), 0), []);
}

/// A cascade as Bevy builds one (`calculate_cascade`, reverse Z): 20 m across, looking down
/// -Z from the origin, 100 m deep.
fn cascade() -> Frustum {
    let clip_from_world = Mat4::from_cols(
        Vec4::new(0.1, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.1, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.01, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 1.0),
    );
    Frustum(ViewFrustum::from_clip_from_world(&clip_from_world))
}

#[test]
fn cascades_ignore_only_their_near_plane() {
    let frustum = cascade();
    let touches = |x, z, radius| in_cascade(&frustum, Vec3A::new(x, 0.0, z), radius);
    assert!(touches(0.0, -50.0, 1.0));
    assert!(!touches(12.0, -50.0, 1.0), "beside the cascade");
    assert!(touches(10.5, -50.0, 1.0), "reaching into it");
    assert!(!touches(0.0, -102.0, 1.0), "past its far plane");
    assert!(
        touches(0.0, 30.0, 1.0),
        "between the light and the cascade it still shades it"
    );
}

fn form(index: u128) -> Form {
    (
        AssetId::Uuid {
            uuid: Uuid::from_u128(index),
        },
        AssetId::Uuid {
            uuid: Uuid::from_u128(index),
        },
    )
}

#[test]
fn flatten_packs_groups_after_earlier_views_and_keeps_allocations() {
    let mut groups = Groups::default();
    groups.insert(form(1), vec![TreeInstance::default(); 3]);
    groups.insert(form(2), vec![TreeInstance::default(); 2]);
    groups.insert(form(3), Vec::new());
    let mut instances = vec![TreeInstance::default(); 4];
    let mut drawn = flatten(&mut groups, &mut instances);
    drawn.sort_by_key(|group| group.first);
    let spans: Vec<_> = drawn.iter().map(|g| (g.first, g.count)).collect();
    assert!(
        spans == [(4, 3), (7, 2)] || spans == [(4, 2), (6, 3)],
        "{spans:?}"
    );
    assert_eq!(instances.len(), 9);
    // Empty groups go; the others stay, emptied, for the next frame.
    assert_eq!(groups.len(), 2);
    assert!(
        groups
            .values()
            .all(|list| list.is_empty() && list.capacity() > 0)
    );
}

#[test]
fn flatten_draws_at_most_its_draw_entities() {
    let mut groups: Groups = (0..DRAWS as u128 + 3)
        .map(|index| (form(index), vec![TreeInstance::default()]))
        .collect();
    let mut instances = Vec::new();
    assert_eq!(flatten(&mut groups, &mut instances).len(), DRAWS);
    assert_eq!(instances.len(), DRAWS);
    assert!(groups.values().all(Vec::is_empty));
}

/// One tree in front of the world view, LOD0 drawn, casting from LOD1 (`shadow_lod` 1), whose
/// scene timed LOD hides; `hidden` hides both meshes themselves, as F1 Objects off does.
fn gathered(hidden: bool) -> (usize, usize) {
    let mut world = World::new();
    world.insert_resource(TreeInstancing {
        enabled: true,
        shadow_lod: 1,
    });
    world.init_resource::<TreeInstanceFrame>();
    let camera = world
        .spawn((Camera::default(), cascade(), crate::WorldViewCamera))
        .id();
    world.spawn((
        DirectionalLight {
            shadow_maps_enabled: true,
            ..default()
        },
        CascadesFrusta {
            frusta: [(camera, vec![cascade()])].into_iter().collect(),
        },
        ViewVisibility::VISIBLE,
    ));
    let mut lod = tree_lod();
    lod.set_timed(0, None);
    let transform = GlobalTransform::from_xyz(0.0, 0.0, -50.0);
    let object = world
        .spawn((lod, transform, InheritedVisibility::VISIBLE))
        .id();
    for (index, drawn) in [(0, true), (1, false)] {
        let scene = world.spawn((LodScene(index), ChildOf(object))).id();
        world.spawn((
            Mesh3d(Handle::default()),
            MeshMaterial3d::<TreeWindMaterial>(Handle::default()),
            transform,
            if hidden {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            },
            if drawn && !hidden {
                InheritedVisibility::VISIBLE
            } else {
                InheritedVisibility::HIDDEN
            },
            TIMED_RANGE,
            ChildOf(scene),
        ));
    }
    world.run_system_once(collect).unwrap();
    let frame = world.resource::<TreeInstanceFrame>();
    let count = |groups: &[Group]| groups.iter().map(|g| g.count as usize).sum();
    let cascades = frame.cascades.values().map(|g| count(g)).sum();
    (count(&frame.groups), cascades)
}

#[test]
fn hidden_objects_cast_nothing_but_hidden_lod_scenes_do() {
    assert_eq!(gathered(false), (1, 1), "LOD0 drawn, LOD1 casting");
    assert_eq!(gathered(true), (0, 0));
}
