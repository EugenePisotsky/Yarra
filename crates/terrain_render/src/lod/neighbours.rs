//! Edge neighbours in a cover of disjoint quadtree nodes, found by walking the tree instead of
//! testing every pair. A neighbour at least as coarse as a node is an ancestor-or-self of the
//! same-level square across the edge. Finer neighbours are that square's descendants along the
//! shared edge, reached only through `interior` nodes (strict ancestors of cover nodes).
use super::{StitchEdges, contains};
use std::collections::{BTreeSet, HashSet};
use world::TerrainNodeKey;

pub(super) const EDGES: [u8; 4] = [
    StitchEdges::WEST,
    StitchEdges::EAST,
    StitchEdges::SOUTH,
    StitchEdges::NORTH,
];

/// Strict ancestors of every node in `cover`.
pub(super) fn interior_of<'a>(
    cover: impl IntoIterator<Item = &'a TerrainNodeKey>,
) -> HashSet<TerrainNodeKey> {
    let mut interior = HashSet::new();
    for key in cover {
        let mut node = *key;
        while let Some(parent) = node.parent().ok().flatten() {
            if !interior.insert(parent) {
                break;
            }
            node = parent;
        }
    }
    interior
}

/// The same-level square across `edge`.
fn across(key: TerrainNodeKey, edge: u8) -> Option<TerrainNodeKey> {
    let (dx, dz) = match edge {
        StitchEdges::WEST => (-1, 0),
        StitchEdges::EAST => (1, 0),
        StitchEdges::SOUTH => (0, -1),
        _ => (0, 1),
    };
    let square = TerrainNodeKey {
        x: key.x.checked_add(dx)?,
        z: key.z.checked_add(dz)?,
        ..key
    };
    square.cell_bounds().ok()?;
    Some(square)
}

/// The cover node at least as coarse as `key` touching its `edge`, if there is one.
pub(super) fn coarser(
    cover: &BTreeSet<TerrainNodeKey>,
    key: TerrainNodeKey,
    edge: u8,
) -> Option<TerrainNodeKey> {
    let mut square = across(key, edge)?;
    loop {
        // A shared ancestor also contains `key`, so it cannot be in a disjoint cover.
        if contains(square, key) {
            return None;
        }
        if cover.contains(&square) {
            return Some(square);
        }
        square = square.parent().ok().flatten()?;
    }
}

/// Cover nodes finer than `key` touching its `edge`, appended to `out`.
pub(super) fn finer(
    cover: &BTreeSet<TerrainNodeKey>,
    interior: &HashSet<TerrainNodeKey>,
    key: TerrainNodeKey,
    edge: u8,
    out: &mut Vec<TerrainNodeKey>,
) {
    let Some(square) = across(key, edge) else {
        return;
    };
    let mut stack = vec![square];
    while let Some(node) = stack.pop() {
        if !interior.contains(&node) {
            continue;
        }
        // The two children on the side facing `key`.
        let child = |dx, dz| TerrainNodeKey {
            level: node.level - 1,
            x: node.x * 2 + dx,
            z: node.z * 2 + dz,
            ..node
        };
        let facing = match edge {
            StitchEdges::WEST => [child(1, 0), child(1, 1)],
            StitchEdges::EAST => [child(0, 0), child(0, 1)],
            StitchEdges::SOUTH => [child(0, 1), child(1, 1)],
            _ => [child(0, 0), child(1, 0)],
        };
        for child in facing {
            if cover.contains(&child) {
                out.push(child);
            } else {
                stack.push(child);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lod::adjacent;
    use world::WorldSpaceId;

    /// Random disjoint covers: split random nodes of a root square, unbalanced on purpose.
    fn random_cover(seed: u64, splits: usize) -> BTreeSet<TerrainNodeKey> {
        let root = TerrainNodeKey {
            space: WorldSpaceId(1),
            level: 6,
            x: -1,
            z: 0,
        };
        let mut cover = BTreeSet::from([root]);
        let mut state = seed;
        for _ in 0..splits {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let candidates: Vec<_> = cover.iter().copied().filter(|k| k.level > 0).collect();
            let key = candidates[(state >> 33) as usize % candidates.len()];
            cover.remove(&key);
            cover.extend(key.children().unwrap().unwrap());
        }
        cover
    }

    #[test]
    fn tree_walks_find_exactly_the_adjacent_pairs() {
        for seed in 0..40 {
            let cover = random_cover(seed, 5 + seed as usize * 3);
            let interior = interior_of(&cover);
            for &key in &cover {
                for edge in EDGES {
                    let mut found = Vec::new();
                    if let Some(c) = coarser(&cover, key, edge) {
                        found.push(c);
                    }
                    finer(&cover, &interior, key, edge, &mut found);
                    found.sort();
                    let expected: Vec<_> = cover
                        .iter()
                        .copied()
                        .filter(|&other| other != key && adjacent(key, other) == Some(edge))
                        .collect();
                    assert_eq!(found, expected, "seed {seed} key {key:?} edge {edge}");
                    for other in &found {
                        assert!(
                            (other.level >= key.level)
                                == (coarser(&cover, key, edge) == Some(*other))
                        );
                    }
                }
            }
        }
    }
}
