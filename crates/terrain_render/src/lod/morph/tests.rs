use super::*;
use crate::lod::{adjacent, build_patch_mesh};
use bevy::mesh::VertexAttributeValues;
use world::WorldSpaceId;

fn key(level: u8, x: i32, z: i32) -> TerrainNodeKey {
    TerrainNodeKey {
        space: WorldSpaceId(1),
        level,
        x,
        z,
    }
}
fn source(key: TerrainNodeKey, edges: StitchEdges, n: u16) -> MorphSource {
    let mut heights = Vec::new();
    let mut normals = Vec::new();
    for z in 0..n {
        for x in 0..n {
            let px = rect(key)[0] as f32 * 4.
                + x as f32 * (1_u32 << key.level) as f32 * 4. / (n - 1) as f32;
            let pz = rect(key)[1] as f32 * 4.
                + z as f32 * (1_u32 << key.level) as f32 * 4. / (n - 1) as f32;
            heights.push((px * 0.7).sin() + (pz * 0.4).cos());
            normals.push(
                Vec3::new(-0.7 * (px * 0.7).cos(), 1., 0.4 * (pz * 0.4).sin())
                    .normalize()
                    .to_array(),
            );
        }
    }
    MorphSource {
        key,
        edges,
        field: TerrainHeightfield::from_heights_and_normals(n, &heights, &normals, -2., 2.)
            .unwrap(),
    }
}
fn positions(mesh: &Mesh, weight: f32) -> Vec<Vec3> {
    let Some(VertexAttributeValues::Float32x3(p)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        panic!()
    };
    p.iter()
        .enumerate()
        .map(|(i, &p)| {
            Vec3::from_array(p)
                + mesh
                    .morph_targets()
                    .map_or(Vec3::ZERO, |d| d[i].position * weight)
        })
        .collect()
}
fn triangles(mesh: &Mesh, weight: f32, key: TerrainNodeKey) -> Vec<[[i64; 3]; 3]> {
    let offset = Vec3::new(rect(key)[0] as f32 * 4., 0., rect(key)[1] as f32 * 4.);
    let p: Vec<_> = positions(mesh, weight)
        .into_iter()
        .map(|p| p + offset)
        .collect();
    let Some(Indices::U32(indices)) = mesh.indices() else {
        panic!()
    };
    indices
        .chunks_exact(3)
        .filter_map(|i| {
            let a = p[i[0] as usize];
            let b = p[i[1] as usize];
            let c = p[i[2] as usize];
            let cross = (b.z - a.z) * (c.x - a.x) - (b.x - a.x) * (c.z - a.z);
            assert!(
                cross >= -1e-5,
                "inverted triangle {a:?} {b:?} {c:?}, t={weight}"
            );
            if cross.abs() < 1e-5 {
                return None;
            }
            let mut tri =
                [a, b, c].map(|p| p.to_array().map(|v| (v as f64 * 10000.).round() as i64));
            tri.sort();
            Some(tri)
        })
        .collect()
}
fn check(
    old: BTreeMap<TerrainNodeKey, StitchEdges>,
    new: BTreeMap<TerrainNodeKey, StitchEdges>,
    n: u16,
) {
    let common = common_cover(&old, &new).unwrap();
    let sources = |cover: &BTreeMap<_, _>| {
        cover
            .iter()
            .map(|(&k, &e)| source(k, e, n))
            .collect::<Vec<_>>()
    };
    let a = sources(&old);
    let b = sources(&new);
    let meshes: Vec<_> = common
        .iter()
        .map(|&k| (k, build_morph_mesh(k, 4., &a, &b).unwrap()))
        .collect();
    for (weight, endpoint) in [(0., &a), (1., &b)] {
        let mut actual: Vec<_> = meshes
            .iter()
            .flat_map(|(k, m)| triangles(&m.mesh, weight, *k))
            .collect();
        let mut expected: Vec<_> = endpoint
            .iter()
            .flat_map(|s| {
                triangles(
                    &build_patch_mesh(&s.field, 4. * (1_u32 << s.key.level) as f32, s.edges)
                        .unwrap(),
                    0.,
                    s.key,
                )
            })
            .collect();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "endpoint {weight}");
    }
    for t in [0., 0.125, 0.25, 0.5, 0.75, 0.875, 1.] {
        let mut edges = BTreeMap::new();
        let mut shared_normals = BTreeMap::<[i64; 3], Vec3>::new();
        let mut area = 0_i128;
        for (key, m) in &meshes {
            let Some(VertexAttributeValues::Float32x3(normals)) =
                m.mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
            else {
                panic!()
            };
            let offset = Vec3::new(rect(*key)[0] as f32 * 4., 0., rect(*key)[1] as f32 * 4.);
            for (i, p) in positions(&m.mesh, t).into_iter().enumerate() {
                let normal =
                    Vec3::from_array(normals[i]) + m.mesh.morph_targets().unwrap()[i].normal * t;
                let canonical = (p + offset)
                    .to_array()
                    .map(|v| (v as f64 * 10000.).round() as i64);
                if let Some(previous) = shared_normals.insert(canonical, normal) {
                    assert!(
                        previous.distance(normal) < 1e-5,
                        "normal seam at {canonical:?}, t={t}"
                    );
                }
                assert!(p.cmpge(m.bounds[0] - Vec3::splat(1e-5)).all());
                assert!(p.cmple(m.bounds[1] + Vec3::splat(1e-5)).all());
            }
            for tri in triangles(&m.mesh, t, *key) {
                let [a, b, c] = tri;
                area += ((b[2] - a[2]) as i128 * (c[0] - a[0]) as i128
                    - (b[0] - a[0]) as i128 * (c[2] - a[2]) as i128)
                    .abs();
                for [mut a, mut b] in [[a, b], [b, c], [c, a]] {
                    if a > b {
                        std::mem::swap(&mut a, &mut b);
                    }
                    *edges.entry((a, b)).or_insert(0) += 1;
                }
            }
        }
        let expected_area: i128 = old
            .keys()
            .map(|k| (1_i128 << (2 * k.level)) * 16 * 2 * 10000_i128.pow(2))
            .sum();
        assert_eq!(area, expected_area, "area at t={t}");
        // Single edges must lie on the outer domain, never a patch interior.
        for ((a, b), count) in edges {
            assert!(count <= 2);
            if count == 1 {
                let bounds = old
                    .keys()
                    .map(|&k| rect(k))
                    .fold([i64::MAX, i64::MAX, i64::MIN, i64::MIN], |b, r| {
                        [
                            b[0].min(r[0]),
                            b[1].min(r[1]),
                            b[2].max(r[2]),
                            b[3].max(r[3]),
                        ]
                    })
                    .map(|v| v * 40000);
                assert!(
                    (a[0] == bounds[0] && b[0] == bounds[0])
                        || (a[0] == bounds[2] && b[0] == bounds[2])
                        || (a[2] == bounds[1] && b[2] == bounds[1])
                        || (a[2] == bounds[3] && b[2] == bounds[3]),
                    "interior crack {a:?} {b:?} at {t}"
                );
            }
        }
    }
}
#[test]
fn every_stitch_change_morphs_without_inversion_and_reproduces_endpoints() {
    for old in 0..16 {
        for new in 0..16 {
            check(
                BTreeMap::from([(key(0, -1, -1), StitchEdges(old))]),
                BTreeMap::from([(key(0, -1, -1), StitchEdges(new))]),
                5,
            );
        }
    }
}
fn cover(keys: BTreeSet<TerrainNodeKey>) -> BTreeMap<TerrainNodeKey, StitchEdges> {
    keys.iter()
        .map(|&k| {
            let mut e = StitchEdges::default();
            for &other in &keys {
                if other.level > k.level
                    && let Some(edge) = adjacent(k, other)
                {
                    e.insert(edge);
                }
            }
            (k, e)
        })
        .collect()
}
#[test]
fn multilevel_mixed_refinement_and_collapse_match_both_covers() {
    let roots = [key(2, -1, -1), key(2, 0, -1), key(2, -1, 0), key(2, 0, 0)];
    let coarse = cover(roots.into_iter().collect());
    let fine = cover(
        (-4..4)
            .flat_map(|z| (-4..4).map(move |x| key(0, x, z)))
            .collect(),
    );
    check(coarse.clone(), fine.clone(), 3);
    check(fine, coarse, 5);
    let mut a = BTreeSet::new();
    let mut b = BTreeSet::new();
    for root in roots {
        for child in root.children().unwrap().unwrap() {
            if child.x < 0 {
                a.extend(child.children().unwrap().unwrap());
                b.insert(child);
            } else {
                b.extend(child.children().unwrap().unwrap());
                a.insert(child);
            }
        }
    }
    check(cover(a), cover(b), 5);
}
#[test]
fn different_domains_are_rejected() {
    assert!(
        common_cover(
            &BTreeMap::from([(key(0, 0, 0), StitchEdges(0))]),
            &BTreeMap::new()
        )
        .is_err()
    );
}

#[test]
fn irregular_balanced_covers_keep_shared_edges_and_normals_closed() {
    let random_cover = |mut seed: u32| {
        let mut keys = BTreeSet::new();
        let mut pending = vec![key(3, -1, -1), key(3, 0, -1), key(3, -1, 0), key(3, 0, 0)];
        while let Some(k) = pending.pop() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            if k.level > 0 && !seed.is_multiple_of(3) {
                pending.extend(k.children().unwrap().unwrap());
            } else {
                keys.insert(k);
            }
        }
        loop {
            let split = keys.iter().copied().find(|&a| {
                keys.iter()
                    .any(|&b| a.level > b.level + 1 && adjacent(a, b).is_some())
            });
            let Some(k) = split else {
                break;
            };
            keys.remove(&k);
            keys.extend(k.children().unwrap().unwrap());
        }
        cover(keys)
    };
    for seed in 0..8 {
        check(random_cover(seed), random_cover(seed + 128), 5);
    }
}
