//! Fixed index templates shared by procedural draws and GPU regression fixtures.

pub(super) const MAX_RENDER_SECTIONS: u8 = 8;
pub(super) const MAX_LOW_RENDER_SECTIONS: u8 = 3;
pub(super) const DIAGNOSTIC_INDEX_COUNT: u32 = 6;
pub(super) const SINGLE_HIGH_INDEX_COUNT: u32 = 48;
pub(super) const SINGLE_LOW_INDEX_COUNT: u32 = 18;
pub(super) const SPLIT_HIGH_INDEX_COUNT: u32 = 39;
pub(super) const SPLIT_LOW_INDEX_COUNT: u32 = 9;
pub(super) const DIAGNOSTIC_FIRST_INDEX: u32 = 0;
pub(super) const SINGLE_HIGH_FIRST_INDEX: u32 = DIAGNOSTIC_FIRST_INDEX + DIAGNOSTIC_INDEX_COUNT;
pub(super) const SINGLE_LOW_FIRST_INDEX: u32 = SINGLE_HIGH_FIRST_INDEX + SINGLE_HIGH_INDEX_COUNT;
pub(super) const SPLIT_HIGH_FIRST_INDEX: u32 = SINGLE_LOW_FIRST_INDEX + SINGLE_LOW_INDEX_COUNT;
pub(super) const SPLIT_LOW_FIRST_INDEX: u32 = SPLIT_HIGH_FIRST_INDEX + SPLIT_HIGH_INDEX_COUNT;

fn topology_vertex(blade: u16, row: u16, right: bool) -> u16 {
    debug_assert!(blade < 2 && row < 16);
    (blade << 5) | (row << 1) | u16::from(right)
}

fn append_strip_indices(indices: &mut Vec<u16>, blade: u16, section_count: u16) {
    for section in 0..section_count {
        let left = topology_vertex(blade, section, false);
        let right = topology_vertex(blade, section, true);
        let next_left = topology_vertex(blade, section + 1, false);
        let next_right = topology_vertex(blade, section + 1, true);
        indices.extend_from_slice(&[left, right, next_right, left, next_right, next_left]);
    }
}

pub(super) fn build_topology_indices() -> Vec<u16> {
    let mut indices = vec![0, 1, 2, 0, 2, 3];
    append_strip_indices(&mut indices, 0, u16::from(MAX_RENDER_SECTIONS));
    append_strip_indices(&mut indices, 0, u16::from(MAX_LOW_RENDER_SECTIONS));

    // Reconstruct the talk's folded strip: both halves reuse the same base edge.
    // Main: four sections and a shared tip. Companion: three paired rows after the base;
    // its high tip pair coincides. The far template retains the previous bent main plus companion triangle.
    let folded_vertex =
        |blade, row, right| topology_vertex(if row == 0 { 0 } else { blade }, row, right);
    {
        let main_sections = 4;
        for section in 0..main_sections - 1 {
            let a = folded_vertex(0, section, false);
            let b = folded_vertex(0, section, true);
            let c = folded_vertex(0, section + 1, false);
            let d = folded_vertex(0, section + 1, true);
            indices.extend_from_slice(&[a, b, d, a, d, c]);
        }
        indices.extend_from_slice(&[
            folded_vertex(0, main_sections - 1, false),
            folded_vertex(0, main_sections - 1, true),
            folded_vertex(0, main_sections, false),
        ]);
        for section in 0..3 {
            let a = folded_vertex(1, section, false);
            let b = folded_vertex(1, section, true);
            let c = folded_vertex(1, section + 1, false);
            let d = folded_vertex(1, section + 1, true);
            indices.extend_from_slice(&[a, b, d, a, d, c]);
        }
    }
    // A bent main blade (root, two shoulder edges, tip) plus the short companion triangle.
    // Production retention pays for the third triangle before emission; no arena grows.
    indices.extend_from_slice(&[
        topology_vertex(0, 0, false),
        topology_vertex(0, 1, false),
        topology_vertex(0, 1, true),
        topology_vertex(0, 1, false),
        topology_vertex(0, 2, false),
        topology_vertex(0, 1, true),
        topology_vertex(1, 0, false),
        topology_vertex(1, 0, true),
        topology_vertex(1, 1, false),
    ]);
    debug_assert_eq!(
        indices.len(),
        (SPLIT_LOW_FIRST_INDEX + SPLIT_LOW_INDEX_COUNT) as usize
    );
    indices
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::gpu_types::{PROCEDURAL_INSTANCE_CAPACITY, ProceduralInstanceGpu};
    use std::collections::HashSet;
    use std::mem::size_of;

    #[test]
    fn topology_indices_enforce_fixed_per_unit_vertex_budgets() {
        let indices = build_topology_indices();
        let ranges = [
            (DIAGNOSTIC_FIRST_INDEX, DIAGNOSTIC_INDEX_COUNT, 4_usize),
            (SINGLE_HIGH_FIRST_INDEX, SINGLE_HIGH_INDEX_COUNT, 18),
            (SINGLE_LOW_FIRST_INDEX, SINGLE_LOW_INDEX_COUNT, 8),
            (SPLIT_HIGH_FIRST_INDEX, SPLIT_HIGH_INDEX_COUNT, 15),
            (SPLIT_LOW_FIRST_INDEX, SPLIT_LOW_INDEX_COUNT, 7),
        ];
        for (first, count, expected_unique) in ranges {
            let range = first as usize..(first + count) as usize;
            assert_eq!(
                indices[range].iter().copied().collect::<HashSet<_>>().len(),
                expected_unique
            );
        }
        assert!(SPLIT_HIGH_INDEX_COUNT <= SINGLE_HIGH_INDEX_COUNT);
        assert!(SPLIT_LOW_INDEX_COUNT <= SINGLE_LOW_INDEX_COUNT);
        assert!(
            u64::from(PROCEDURAL_INSTANCE_CAPACITY) * size_of::<ProceduralInstanceGpu>() as u64
                <= 26 * 1024 * 1024
        );
    }

    #[test]
    fn folded_pair_cpu_gpu_index_ranges_and_triangle_budget_agree() {
        let compute = include_str!("../../../../assets/shaders/vegetation_debug_compute.wgsl");
        for (name, value) in [
            ("SPLIT_HIGH_INDEX_COUNT", SPLIT_HIGH_INDEX_COUNT),
            ("SPLIT_LOW_INDEX_COUNT", SPLIT_LOW_INDEX_COUNT),
            ("SPLIT_HIGH_FIRST_INDEX", SPLIT_HIGH_FIRST_INDEX),
            ("SPLIT_LOW_FIRST_INDEX", SPLIT_LOW_FIRST_INDEX),
        ] {
            assert!(compute.contains(&format!("const {name}: u32 = {value}u;")));
        }
        let retention = |source: &str| -> f32 {
            source
                .split("const SPLIT_LOW_DENSITY_BUDGET_SCALE: f32 = ")
                .nth(1)
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        let density = retention(compute);
        assert_eq!(
            density,
            retention(include_str!(
                "../../../../assets/shaders/vegetation_blade.wgsl"
            ))
        );
        assert!(density * SPLIT_LOW_INDEX_COUNT as f32 <= 0.65 * 9.0 + 1e-6);
    }

    #[test]
    fn folded_near_base_and_original_far_topology_are_preserved() {
        let indices = build_topology_indices();
        let high = &indices[SPLIT_HIGH_FIRST_INDEX as usize..SPLIT_LOW_FIRST_INDEX as usize];
        assert!(
            high.chunks_exact(3)
                .any(|triangle| triangle.contains(&0) && triangle.contains(&1))
        );
        assert!(!high.contains(&topology_vertex(1, 0, false)));
        assert!(!high.contains(&topology_vertex(1, 0, true)));
        let low = &indices[SPLIT_LOW_FIRST_INDEX as usize..];
        assert_eq!(low, &[0, 2, 3, 2, 4, 3, 32, 33, 34]);
    }
}
