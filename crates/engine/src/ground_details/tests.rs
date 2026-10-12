use super::*;
use vegetation::{DetailClumps, fixtures::full_coverage_page};

fn stones() -> DetailType {
    DetailType {
        key: "stones".into(),
        assets: vec!["stones.glb".into()],
        density: 2.0,
        scale: [0.8, 1.2],
        clumps: DetailClumps {
            size: 1.5,
            pull: 0.5,
            patches: 0.5,
        },
        sink: 0.3,
        align: 0.5,
        tilt: 0.3,
        distance: 40.0,
        shadows: false,
        on_paths: true,
    }
}

fn pieces() -> Vec<Piece> {
    (0..3)
        .map(|i| Piece {
            mesh: Handle::default(),
            material: Handle::default(),
            height: 0.1 * (i + 1) as f32,
            tags: None,
        })
        .collect()
}

#[test]
fn placement_follows_density_stays_on_its_page_and_repeats() {
    let detail = stones();
    let pieces = pieces();
    let page = full_coverage_page([32., -16.], 16., detail.id());
    let objects = place(&page, &page.fields[0], &detail, [0.; 2], &pieces);
    // Clumps thin their edges and vary in density, so somewhat under 2 per square metre.
    let expected = detail.density * page.size * page.size;
    assert!(
        (0.55 * expected..=1.05 * expected).contains(&(objects.len() as f32)),
        "{} of {expected}",
        objects.len()
    );
    for (piece, t) in &objects {
        let x = t.translation.x;
        let z = t.translation.z;
        assert!((32. ..48.).contains(&x) && (-16. ..0.).contains(&z));
        // Sunk by its share of the piece's height, on flat ground.
        assert!((t.translation.y + detail.sink * piece.height * t.scale.x).abs() < 1e-4);
        assert!((0.8..=1.2).contains(&t.scale.x));
    }
    let again = place(&page, &page.fields[0], &detail, [0.; 2], &pieces);
    assert_eq!(objects.len(), again.len());
    assert!(
        objects
            .iter()
            .zip(&again)
            .all(|(a, b)| a.1 == b.1 && std::ptr::eq(a.0, b.0))
    );
}

#[test]
fn objects_stay_on_the_world_lattice_when_the_render_origin_moves() {
    let detail = stones();
    let pieces = pieces();
    let here = full_coverage_page([0., 0.], 16., detail.id());
    let shifted = full_coverage_page([-64., 32.], 16., detail.id());
    let a = place(&here, &here.fields[0], &detail, [0.; 2], &pieces);
    let b = place(&shifted, &shifted.fields[0], &detail, [64., -32.], &pieces);
    assert_eq!(a.len(), b.len());
    for ((_, a), (_, b)) in a.iter().zip(&b) {
        assert!((a.translation - (b.translation + Vec3::new(64., 0., -32.))).length() < 1e-3);
    }
}

#[test]
fn no_coverage_grows_nothing() {
    let detail = stones();
    let mut page = full_coverage_page([0., 0.], 16., detail.id());
    page.fields[0].coverage = vec![0];
    assert!(place(&page, &page.fields[0], &detail, [0.; 2], &pieces()).is_empty());
}
