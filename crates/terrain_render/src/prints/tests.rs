use super::*;

const N: i64 = SIDE as i64 / TILE;

fn press(source: u64, at: Vec2, depth: f32) -> GroundPress {
    GroundPress {
        source,
        heel: at,
        ball: at + Vec2::new(0.0, 0.15),
        radii: [0.04, 0.045],
        depth: [depth; 2],
    }
}

fn presses(focus: Vec2, list: Vec<GroundPress>) -> GroundPresses {
    GroundPresses {
        space: Some(WorldSpaceId(1)),
        focus: Some(Vec3::new(focus.x, 0.0, focus.y)),
        origin: DVec2::new(1000.0, -2000.0),
        presses: list,
    }
}

fn level_tiles(frame: &PrintFrame, level: u32) -> Vec<[u32; 4]> {
    frame
        .tiles
        .iter()
        .filter(|t| t[3] & 1 == level)
        .copied()
        .collect()
}

#[test]
fn entered_tiles_are_exactly_the_new_ones() {
    let moves = [
        [1, 0],
        [0, -1],
        [3, 2],
        [-2, 5],
        [-7, -7],
        [N - 1, 1],
        [N, 0],
    ];

    for [dx, dz] in moves {
        let previous = [40, -60];
        let window = [previous[0] + dx, previous[1] + dz];
        let rects = entered(Some(previous), window);
        let mut covered = HashSet::new();

        for r in &rects {
            for z in r[1]..r[3] {
                for x in r[0]..r[2] {
                    assert!(covered.insert((x, z)), "rectangles overlap at {x}, {z}");
                }
            }
        }

        let held = |x: i64, z: i64, w: [i64; 2]| {
            (w[0]..w[0] + N).contains(&x) && (w[1]..w[1] + N).contains(&z)
        };

        for z in window[1]..window[1] + N {
            for x in window[0]..window[0] + N {
                assert_eq!(
                    covered.contains(&(x, z)),
                    !held(x, z, previous),
                    "move {dx}, {dz} at tile {x}, {z}"
                );
            }
        }

        assert_eq!(
            covered.len() as i64,
            N * N - (N - dx.abs()).max(0) * (N - dz.abs()).max(0)
        );
    }

    assert_eq!(entered(None, [0, 0]), vec![[0, 0, N, N]]);
}

#[test]
fn a_held_foot_logs_once_and_again_when_it_presses_deeper_or_touches_again() {
    let mut prints = GroundPrints::default();
    let at = Vec2::new(3.0, 4.0);

    prints.update(&presses(at, vec![press(7, at, 0.05)]));
    prints.update(&presses(at, vec![press(7, at, 0.051)]));
    assert_eq!(prints.log.len(), 1, "a foot held still is logged once");

    prints.update(&presses(at, vec![press(7, at, 0.06)]));
    assert_eq!(prints.log.len(), 2, "pressing deeper is logged");

    prints.update(&presses(at, vec![press(7, at + Vec2::X * 0.02, 0.06)]));
    assert_eq!(prints.log.len(), 3, "moving is logged");

    prints.update(&presses(at, vec![press(7, at, -0.01)]));
    prints.update(&presses(at, vec![press(7, at, 0.06)]));
    assert_eq!(prints.log.len(), 4, "a lifted foot logs its next touch");

    prints.update(&presses(at, vec![press(8, at, f32::NAN)]));
    assert_eq!(prints.log.len(), 4, "a press that is not finite is ignored");
}

#[test]
fn the_first_frame_starts_both_windows_undisturbed_and_takes_its_presses() {
    let mut prints = GroundPrints::default();
    let at = Vec2::new(3.0, 4.0);
    prints.update(&presses(at, vec![press(1, at, 0.05)]));
    let frame = &prints.frame;

    for level in 0..LEVELS as u32 {
        let tiles = level_tiles(frame, level);
        assert_eq!(tiles.len() as i64, N * N);
        assert!(
            tiles.iter().all(|t| t[3] & 2 != 0),
            "every tile starts undisturbed"
        );
        assert!(
            tiles.iter().any(|t| t[3] >> 2 == 1),
            "level {level} takes the press"
        );
    }

    assert_eq!(frame.stamps.len(), 1);
    let stamp = frame.stamps[0];
    assert_eq!(
        [stamp[0], stamp[1]],
        [at.x, at.y],
        "stamps are in render space"
    );
}

#[test]
fn windows_report_their_corner_in_render_space_and_their_toroidal_texel() {
    let mut prints = GroundPrints::default();
    let focus = Vec2::new(3.0, 4.0);
    let list = presses(focus, Vec::new());
    prints.update(&list);

    let windows = prints.windows.unwrap();

    for ((w, first), texel) in prints.frame.windows.iter().zip(windows).zip(TEXEL_METRES) {
        let corner = Vec2::new(f32::from_bits(w[0]), f32::from_bits(w[1]));
        let expected =
            (DVec2::new(first[0] as f64, first[1] as f64) * texel - list.origin).as_vec2();
        assert!((corner - expected).length() < 1e-3);
        assert_eq!(w[2], first[0].rem_euclid(i64::from(SIDE)) as u32);
        assert_eq!(w[3], first[1].rem_euclid(i64::from(SIDE)) as u32);

        // The focus lies within a tile of the window's centre.
        let centre = corner + Vec2::splat(SIDE as f32 * texel as f32 / 2.0);
        assert!((centre - focus).abs().max_element() <= (TILE as f64 * texel) as f32);
    }
}

#[test]
fn ground_a_window_returns_to_takes_its_logged_presses_again() {
    let mut prints = GroundPrints::default();
    let start = Vec2::new(0.0, 0.0);
    let print = Vec2::new(0.0, -15.0);
    prints.update(&presses(start, vec![press(1, print, 0.05)]));

    // Walk away until the fine window no longer holds the print, then come back a tile at a time.
    let away = Vec2::new(0.0, 40.0);
    prints.update(&presses(away, Vec::new()));
    let tile = (TILE as f64 * TEXEL_METRES[0]) as f32;
    let mut focus = away;
    let mut taken = false;

    while focus.y > start.y {
        focus.y -= tile;
        prints.update(&presses(focus, Vec::new()));
        let fine = level_tiles(&prints.frame, 0);
        assert!(
            fine.iter().all(|t| t[3] & 2 != 0),
            "only entering tiles are listed"
        );
        taken |= fine.iter().any(|t| t[3] >> 2 > 0);
    }

    assert!(taken, "the fine window took the print again");
    assert_eq!(prints.log.len(), 1);
}

#[test]
fn a_tile_takes_its_stamps_oldest_first_and_each_tile_is_listed_once() {
    let mut prints = GroundPrints::default();
    let at = Vec2::new(1.0, 1.0);
    prints.update(&presses(at, vec![press(1, at, 0.03)]));
    prints.update(&presses(at, vec![press(1, at, 0.05), press(2, at, 0.04)]));
    let frame = &prints.frame;
    let mut seen = HashSet::new();

    for t in &frame.tiles {
        assert!(seen.insert((t[3] & 1, t[0], t[1])), "tile listed twice");
        let list = &frame.indices[t[2] as usize..(t[2] + (t[3] >> 2)) as usize];
        assert!(list.windows(2).all(|w| w[0] < w[1]), "stamps out of order");
    }

    assert!(frame.tiles.iter().all(|t| t[3] & 2 == 0), "nothing entered");
    assert_eq!(frame.stamps.len(), 2, "only this frame's presses are drawn");
}

#[test]
fn another_world_space_starts_over() {
    let mut prints = GroundPrints::default();
    let at = Vec2::new(1.0, 1.0);
    prints.update(&presses(at, vec![press(1, at, 0.03)]));
    let mut other = presses(at, Vec::new());
    other.space = Some(WorldSpaceId(2));
    prints.update(&other);
    assert!(prints.log.is_empty());
    assert_eq!(level_tiles(&prints.frame, 0).len() as i64, N * N);
}
