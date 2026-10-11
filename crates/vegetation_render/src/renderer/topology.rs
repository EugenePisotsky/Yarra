//! Fixed index templates of the two blade LODs, as in Ghost of Tsushima's grass: pairs of
//! vertices at evenly spaced rows up the blade and one tip vertex. High detail has 7 rows
//! (15 vertices), low detail 3 (7 vertices). Vertex `2 * row + side` sits on a row; vertex
//! `2 * rows` is the tip.

/// Rows of vertex pairs below the tip.
pub(super) const HIGH_ROWS: u32 = 7;
pub(super) const LOW_ROWS: u32 = 3;
pub(super) const HIGH_INDEX_COUNT: u32 = (HIGH_ROWS - 1) * 6 + 3;
pub(super) const LOW_INDEX_COUNT: u32 = (LOW_ROWS - 1) * 6 + 3;
pub(super) const HIGH_FIRST_INDEX: u32 = 0;
pub(super) const LOW_FIRST_INDEX: u32 = HIGH_INDEX_COUNT;

fn append_blade(indices: &mut Vec<u16>, rows: u16) {
    for row in 0..rows - 1 {
        let (left, right) = (2 * row, 2 * row + 1);
        let (next_left, next_right) = (left + 2, right + 2);
        indices.extend_from_slice(&[left, right, next_right, left, next_right, next_left]);
    }
    indices.extend_from_slice(&[2 * (rows - 1), 2 * (rows - 1) + 1, 2 * rows]);
}

pub(super) fn build_topology_indices() -> Vec<u16> {
    let mut indices = Vec::new();
    append_blade(&mut indices, HIGH_ROWS as u16);
    append_blade(&mut indices, LOW_ROWS as u16);
    debug_assert_eq!(indices.len(), (LOW_FIRST_INDEX + LOW_INDEX_COUNT) as usize);
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_reference_only_their_own_vertices() {
        let indices = build_topology_indices();
        let high = &indices[..HIGH_INDEX_COUNT as usize];
        let low = &indices[LOW_FIRST_INDEX as usize..];
        assert_eq!(high.iter().max(), Some(&(2 * HIGH_ROWS as u16)));
        assert_eq!(low.iter().max(), Some(&(2 * LOW_ROWS as u16)));
        assert_eq!(HIGH_INDEX_COUNT, 39);
        assert_eq!(LOW_INDEX_COUNT, 15);
    }
}
