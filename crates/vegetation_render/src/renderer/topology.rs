//! Fixed index templates of the two blade LODs, as in Ghost of Tsushima's grass: pairs of
//! vertices at evenly spaced rows up the blade and one tip vertex. High detail has 9 rows
//! (19 vertices), low detail 3 (7 vertices). Vertex `2 * row + side` sits on a row; vertex
//! `2 * rows` is the tip. Twin types run one strip through two blades (`draw/geometry.wesl`),
//! so high detail has rows enough for two blades that lie over without visible kinks.
//!
//! A draw instance is a batch of [`BLADES_PER_BATCH`] blades, each strip's vertices after the
//! last's. Drawn one to an instance, a blade's few vertices took a quarter longer to shade on the
//! M2 Max (0.73 against 0.585 ms in the meadow).

/// Rows of vertex pairs below the tip.
pub(super) const HIGH_ROWS: u32 = 9;

pub(super) const LOW_ROWS: u32 = 3;
pub(super) const BLADES_PER_BATCH: u32 = 32;

/// Indices of one blade's strip.
pub(super) const HIGH_INDEX_COUNT: u32 = (HIGH_ROWS - 1) * 6 + 3;

pub(super) const LOW_INDEX_COUNT: u32 = (LOW_ROWS - 1) * 6 + 3;
pub(super) const HIGH_BATCH_INDEX_COUNT: u32 = BLADES_PER_BATCH * HIGH_INDEX_COUNT;
pub(super) const LOW_BATCH_INDEX_COUNT: u32 = BLADES_PER_BATCH * LOW_INDEX_COUNT;
pub(super) const HIGH_FIRST_INDEX: u32 = 0;
pub(super) const LOW_FIRST_INDEX: u32 = HIGH_BATCH_INDEX_COUNT;

fn append_batch(indices: &mut Vec<u16>, rows: u16) {
    let strip = 2 * rows + 1;

    for blade in 0..BLADES_PER_BATCH as u16 {
        let first = blade * strip;

        for row in 0..rows - 1 {
            let (left, right) = (first + 2 * row, first + 2 * row + 1);
            let (next_left, next_right) = (left + 2, right + 2);
            indices.extend_from_slice(&[left, right, next_right, left, next_right, next_left]);
        }

        let last = first + 2 * (rows - 1);
        indices.extend_from_slice(&[last, last + 1, first + 2 * rows]);
    }
}

pub(super) fn build_topology_indices() -> Vec<u16> {
    let mut indices = Vec::new();
    append_batch(&mut indices, HIGH_ROWS as u16);
    append_batch(&mut indices, LOW_ROWS as u16);
    debug_assert_eq!(
        indices.len(),
        (LOW_FIRST_INDEX + LOW_BATCH_INDEX_COUNT) as usize
    );
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_reference_only_their_own_vertices() {
        let indices = build_topology_indices();
        let high = &indices[..HIGH_BATCH_INDEX_COUNT as usize];
        let low = &indices[LOW_FIRST_INDEX as usize..];
        let last = |rows| (BLADES_PER_BATCH * (2 * rows + 1) - 1) as u16;
        assert_eq!(high.iter().max(), Some(&last(HIGH_ROWS)));
        assert_eq!(low.iter().max(), Some(&last(LOW_ROWS)));
        assert_eq!(HIGH_INDEX_COUNT, 51);
        assert_eq!(LOW_INDEX_COUNT, 15);

        // Each blade's strip keeps to its own vertices.
        for (template, rows, count) in [
            (high, HIGH_ROWS, HIGH_INDEX_COUNT),
            (low, LOW_ROWS, LOW_INDEX_COUNT),
        ] {
            for (blade, strip) in template.chunks(count as usize).enumerate() {
                let first = (blade as u32 * (2 * rows + 1)) as u16;
                assert!(
                    strip
                        .iter()
                        .all(|&i| (first..=first + 2 * rows as u16).contains(&i))
                );
            }
        }
    }
}
