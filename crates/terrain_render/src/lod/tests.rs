use super::*;
use world::{TerrainHeightfield, TerrainNode, WorldSpaceId};
fn key(level: u8, x: i32, z: i32) -> TerrainNodeKey {
    TerrainNodeKey {
        space: WorldSpaceId(1),
        level,
        x,
        z,
    }
}
fn metadata(root: TerrainNodeKey) -> BTreeMap<TerrainNodeKey, PatchMetadata> {
    let mut result = BTreeMap::new();
    let mut todo = vec![root];
    while let Some(key) = todo.pop() {
        result.insert(
            key,
            PatchMetadata {
                key,
                height_bounds: [0.0, 10.0],
                geometric_error: key.level as f32 * 0.25,
                resolution: 33,
            },
        );
        if let Some(children) = key.children().unwrap() {
            todo.extend(children);
        }
    }
    result
}
fn view(position: DVec3, target: DVec3) -> LodView {
    LodView {
        clip_from_world: DMat4::perspective_rh(1.0, 16.0 / 9.0, 0.1, 10000.0)
            * DMat4::look_at_rh(position, target, DVec3::Z),
        viewport: [2560, 1440],
        contact_position: position,
    }
}
fn settings() -> LodSettings {
    LodSettings {
        exact_radius: 0.0,
        contact_radius: 0.0,
        ..Default::default()
    }
}
fn check_cover(roots: &[TerrainNodeKey], result: &PlannedCover) {
    assert!(result.balanced);
    let expected: u64 = roots.iter().map(|k| 1_u64 << (2 * k.level)).sum();
    let actual: u64 = result.patches.keys().map(|k| 1_u64 << (2 * k.level)).sum();
    assert_eq!(actual, expected);
    let keys: Vec<_> = result.patches.keys().copied().collect();
    for (i, &a) in keys.iter().enumerate() {
        for &b in &keys[i + 1..] {
            assert!(!contains(a, b) && !contains(b, a));
            if adjacent(a, b).is_some() {
                assert!(a.level.abs_diff(b.level) <= 1);
            }
        }
    }
}
#[test]
fn stitched_topologies_cover_patch_without_holes_or_inverted_triangles() {
    for n in [3, 5, 33] {
        for mask in 0..16 {
            let indices = stitch_indices(n, StitchEdges(mask)).unwrap();
            let n = n as u32;
            let mut area = 0_i64;
            let mut edges = BTreeMap::<(u32, u32), usize>::new();
            for t in indices.chunks_exact(3) {
                let xy = |i: u32| ((i % n) as i64, (i / n) as i64);
                let (a, b, c) = (xy(t[0]), xy(t[1]), xy(t[2]));
                let cross = (b.1 - a.1) * (c.0 - a.0) - (b.0 - a.0) * (c.1 - a.1);
                assert!(cross > 0, "n={n} mask={mask} {t:?}");
                area += cross;
                for [a, b] in [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]] {
                    *edges.entry((a.min(b), a.max(b))).or_default() += 1;
                }
            }
            assert_eq!(area, 2 * i64::from(n - 1).pow(2));
            for ((a, b), count) in edges {
                assert!(count <= 2);
                if count == 1 {
                    assert!(
                        (a % n == 0 && b % n == 0)
                            || (a % n == n - 1 && b % n == n - 1)
                            || (a / n == 0 && b / n == 0)
                            || (a / n == n - 1 && b / n == n - 1),
                        "interior hole"
                    );
                }
            }
        }
    }
}
#[test]
fn fine_stitched_edge_matches_coarse_height_and_normal_vertices() {
    let mut leaves = Vec::new();
    for z in 0..2 {
        for x in 0..2 {
            let values: Vec<_> = (0..5)
                .flat_map(|j| {
                    (0..5).map(move |i| {
                        ((x * 4 + i) as f32 * 0.3).sin() + ((z * 4 + j) as f32 * 0.5).cos()
                    })
                })
                .collect();
            let normals = vec![[0.0, 1.0, 0.0]; 25];
            let field =
                TerrainHeightfield::from_heights_and_normals(5, &values, &normals, -2.0, 2.0)
                    .unwrap();
            leaves.push(TerrainNode::leaf(key(0, x, z), &field, 5).unwrap());
        }
    }
    let parent =
        TerrainNode::parent(key(1, 0, 0), std::array::from_fn(|i| Some(&leaves[i]))).unwrap();
    for i in 0..=2 {
        assert_eq!(
            leaves[0].heightfield.as_ref().unwrap().height_at(0, i * 2),
            parent.heightfield.as_ref().unwrap().height_at(0, i)
        );
        assert_eq!(
            leaves[0].heightfield.as_ref().unwrap().normal_at(0, i * 2),
            parent.heightfield.as_ref().unwrap().normal_at(0, i)
        );
    }
}
#[test]
fn missing_children_keep_parent_and_request_complete_replacement() {
    let root = key(2, -1, -1);
    let mut m = metadata(root);
    let child = root.children().unwrap().unwrap()[2];
    m.remove(&child);
    let v = view(
        DVec3::new(-10.0, 15.0, -10.0),
        DVec3::new(-10.0, 0.0, -10.0),
    );
    let plan = plan_cover(&[root], &m, &BTreeSet::new(), &v, 8.0, &settings()).unwrap();
    assert_eq!(plan.patches.len(), 1);
    assert!(plan.requests.contains(&child));
    check_cover(&[root], &plan);
}
#[test]
fn camera_motion_rotation_and_budget_keep_exact_balanced_coverage() {
    let roots = [key(4, -1, -1), key(4, 0, -1), key(4, -1, 0), key(4, 0, 0)];
    let metadata: BTreeMap<_, _> = roots.into_iter().flat_map(metadata).collect();
    let s = LodSettings {
        max_patches: 100,
        max_triangles: 100 * 2048,
        ..settings()
    };
    let mut previous = BTreeSet::new();
    for (eye, target) in [
        ([0., 15., 0.], [30., 0., 40.]),
        ([0., 15., 0.], [-60., 0., -40.]),
        ([800., 1000., 800.], [0., 0., 0.]),
        ([-90., 8., -90.], [20., 0., 0.]),
        ([0., 3000., 0.], [0., 0., 0.]),
    ] {
        let v = view(DVec3::from_array(eye), DVec3::from_array(target));
        let p = plan_cover(&roots, &metadata, &previous, &v, 8.0, &s).unwrap();
        check_cover(&roots, &p);
        assert!(p.patches.len() <= s.max_patches);
        assert!(p.stats.triangles <= s.max_triangles);
        assert!(p.stats.work <= s.max_work);
        previous = p.patches.keys().copied().collect();
    }
}
#[test]
fn elevated_valley_uses_three_dimensional_distance_and_orthographic_projection() {
    let root = key(3, 0, 0);
    let m = metadata(root);
    let mut v = view(DVec3::new(32., 1500., 32.), DVec3::new(32., 0., 32.));
    let high = plan_cover(
        &[root],
        &m,
        &BTreeSet::new(),
        &v,
        8.0,
        &LodSettings::default(),
    )
    .unwrap();
    assert_eq!(high.patches.len(), 1);
    v.contact_position.y = 8.0;
    v.clip_from_world = DMat4::orthographic_rh(-50., 50., -50., 50., 0.1, 2000.)
        * DMat4::look_at_rh(
            DVec3::new(32., 1500., 32.),
            DVec3::new(32., 0., 32.),
            DVec3::Z,
        );
    let near = plan_cover(
        &[root],
        &m,
        &BTreeSet::new(),
        &v,
        8.0,
        &LodSettings::default(),
    )
    .unwrap();
    assert!(near.patches.len() > high.patches.len());
    check_cover(&[root], &near);
    let bounds = m[&root].bounds(8.0);
    let e = v.projected_error(bounds, 1.0);
    let mut translated = v.clone();
    translated.clip_from_world =
        v.clip_from_world * DMat4::from_translation(DVec3::new(1000., 0., -1000.));
    let moved = bounds.map(|b| b - DVec3::new(1000., 0., -1000.));
    assert!((e - translated.projected_error(moved, 1.0)).abs() < 1e-9);
}
#[test]
fn sparse_roots_never_invent_ground_and_unready_balance_is_explicit() {
    let roots = [key(3, 0, 0), key(0, 8, 0)];
    let mut m = metadata(roots[0]);
    m.extend(metadata(roots[1]));
    let v = view(DVec3::new(0., 3000., 0.), DVec3::ZERO);
    let p = plan_cover(&roots, &m, &BTreeSet::new(), &v, 8.0, &settings()).unwrap();
    check_cover(&roots, &p);
    let coarse: BTreeMap<_, _> = roots.into_iter().map(|k| (k, m[&k].clone())).collect();
    let p = plan_cover(&roots, &coarse, &BTreeSet::new(), &v, 8.0, &settings()).unwrap();
    assert!(!p.balanced);
    assert!(!p.requests.is_empty());
}
#[test]
fn hysteresis_preserves_refinement_and_bad_budgets_fail_explicitly() {
    let root = key(1, 0, 0);
    let m = metadata(root);
    let mut v = view(DVec3::new(8., 100., 8.), DVec3::ZERO);
    // A horizontal look makes vertical terrain error affect projected height.
    v.clip_from_world = DMat4::orthographic_rh(-100., 100., -120., 120., 0.1, 2000.)
        * DMat4::look_at_rh(DVec3::new(8., 20., 100.), DVec3::new(8., 20., 0.), DVec3::Y);
    let before = plan_cover(&[root], &m, &BTreeSet::new(), &v, 8.0, &settings()).unwrap();
    let refined = root.children().unwrap().unwrap().into_iter().collect();
    let after = plan_cover(&[root], &m, &refined, &v, 8.0, &settings()).unwrap();
    assert_eq!(before.patches.len(), 1);
    assert_eq!(after.patches.len(), 4);
    let invalid = LodSettings {
        max_triangles: 1,
        ..settings()
    };
    assert!(plan_cover(&[root], &m, &refined, &v, 8.0, &invalid).is_err());
}

#[test]
fn stitching_error_refines_the_neighbour_even_when_its_body_is_flat() {
    let roots = [key(2, 0, 0), key(2, 1, 0)];
    let mut m: BTreeMap<_, _> = roots.into_iter().flat_map(metadata).collect();
    // The left root has fine ruts. The right root is flat, but it must refine too
    // wherever its stitched neighbour needs exact contact with those rut samples.
    for meta in m.values_mut() {
        if contains(roots[1], meta.key) {
            meta.geometric_error = 0.0;
        }
    }
    let v = view(DVec3::new(31.9, 5., 8.), DVec3::new(31.9, 0., 8.));
    let s = LodSettings {
        exact_radius: 2.0,
        contact_radius: 6.0,
        ..Default::default()
    };
    let p = plan_cover(&roots, &m, &BTreeSet::new(), &v, 8.0, &s).unwrap();
    check_cover(&roots, &p);
    assert!(!p.stats.contact_limited);
    assert!(!p.stats.budget_limited);
    assert!(p.patches.contains_key(&key(0, 4, 1)));
}
