//! Page samples kept in place across scene revisions. A page whose content key is unchanged
//! keeps its surface and coverage ranges, so a repack uploads only pages that arrived or
//! changed. Walking repacked about ten times a second, re-uploading ~8 MB each time.
use super::packing::PageLayout;
use bevy::platform::collections::{HashMap, HashSet};
use vegetation::VegetationFieldPage;

/// First-fit ranges of a buffer, in elements.
#[derive(Debug, Default)]
struct RangeAllocator {
    /// Free (start, length) ranges, sorted and never adjacent.
    free: Vec<(u32, u32)>,
}

impl RangeAllocator {
    fn with_capacity(capacity: u32) -> Self {
        Self {
            free: if capacity > 0 {
                vec![(0, capacity)]
            } else {
                Vec::new()
            },
        }
    }

    fn allocate(&mut self, length: u32) -> Option<u32> {
        if length == 0 {
            return Some(0);
        }
        let index = self.free.iter().position(|&(_, free)| free >= length)?;
        let (start, free) = self.free[index];
        if free == length {
            self.free.remove(index);
        } else {
            self.free[index] = (start + length, free - length);
        }
        Some(start)
    }

    fn release(&mut self, start: u32, length: u32) {
        if length == 0 {
            return;
        }
        let index = self.free.partition_point(|&(s, _)| s < start);
        self.free.insert(index, (start, length));
        // Merge with the following range, then with the preceding one.
        if index + 1 < self.free.len() && start + length == self.free[index + 1].0 {
            self.free[index].1 += self.free[index + 1].1;
            self.free.remove(index + 1);
        }
        if index > 0 && self.free[index - 1].0 + self.free[index - 1].1 == start {
            self.free[index - 1].1 += self.free[index].1;
            self.free.remove(index);
        }
    }
}

struct Resident {
    layout: PageLayout,
    surface_samples: u32,
    coverage_samples: Vec<u32>,
}

fn surface_samples(page: &VegetationFieldPage) -> u32 {
    page.surface.heights.len() as u32
}

fn coverage_samples(page: &VegetationFieldPage) -> Vec<u32> {
    page.fields
        .iter()
        .map(|f| f.coverage.len() as u32)
        .collect()
}

/// Where each page's samples go, and which pages must be written.
#[derive(Debug, PartialEq)]
pub(super) struct SlotPlan {
    pub(super) layouts: Vec<PageLayout>,
    /// Indices of pages whose samples are not in the buffers yet.
    pub(super) uploads: Vec<usize>,
}

/// Required capacities, in elements, when the current buffers cannot hold the scene.
#[derive(Debug, PartialEq)]
pub(super) struct Regrow {
    pub(super) surface_samples: u64,
    pub(super) coverage_samples: u64,
}

#[derive(Default)]
pub(super) struct PageSlots {
    pages: HashMap<u64, Resident>,
    surfaces: RangeAllocator,
    coverage: RangeAllocator,
}

impl PageSlots {
    /// Places `pages` in buffers holding `capacity` (surface, coverage) samples. `keys` are
    /// content keys, one per page: an equal key promises identical samples. Without keys
    /// (or with repeated ones) every page is packed afresh, in order, as before.
    pub(super) fn place(
        &mut self,
        pages: &[VegetationFieldPage],
        keys: &[u64],
        capacity: (u32, u32),
    ) -> Result<SlotPlan, Regrow> {
        let keyed =
            keys.len() == pages.len() && keys.iter().collect::<HashSet<_>>().len() == keys.len();
        if !keyed {
            self.reset(capacity);
        } else {
            let current: HashSet<_> = keys.iter().collect();
            let departed: Vec<_> = self
                .pages
                .keys()
                .filter(|key| !current.contains(key))
                .copied()
                .collect();
            for key in departed {
                let resident = self.pages.remove(&key).unwrap();
                self.release(&resident);
            }
        }
        let mut layouts = Vec::with_capacity(pages.len());
        let mut uploads = Vec::new();
        for (index, page) in pages.iter().enumerate() {
            let surfaces = surface_samples(page);
            let coverage = coverage_samples(page);
            if keyed && let Some(resident) = self.pages.get(&keys[index]) {
                if resident.surface_samples == surfaces && resident.coverage_samples == coverage {
                    layouts.push(resident.layout.clone());
                    continue;
                }
                // Same key, different shape: never read another page's samples.
                let resident = self.pages.remove(&keys[index]).unwrap();
                self.release(&resident);
            }
            let Some(layout) = self.allocate(surfaces, &coverage) else {
                return Err(self.regrow(pages));
            };
            if keyed {
                self.pages.insert(
                    keys[index],
                    Resident {
                        layout: layout.clone(),
                        surface_samples: surfaces,
                        coverage_samples: coverage,
                    },
                );
            }
            layouts.push(layout);
            uploads.push(index);
        }
        Ok(SlotPlan { layouts, uploads })
    }

    /// Forgets every page, for buffers of `capacity` (surface, coverage) samples.
    pub(super) fn reset(&mut self, capacity: (u32, u32)) {
        *self = Self {
            pages: HashMap::default(),
            surfaces: RangeAllocator::with_capacity(capacity.0),
            coverage: RangeAllocator::with_capacity(capacity.1),
        };
    }

    fn allocate(&mut self, surfaces: u32, coverage: &[u32]) -> Option<PageLayout> {
        let surface = self.surfaces.allocate(surfaces)?;
        let mut offsets = Vec::with_capacity(coverage.len());
        for &samples in coverage {
            match self.coverage.allocate(samples) {
                Some(offset) => offsets.push(offset),
                None => {
                    self.surfaces.release(surface, surfaces);
                    for (&offset, &samples) in offsets.iter().zip(coverage) {
                        self.coverage.release(offset, samples);
                    }
                    return None;
                }
            }
        }
        Some(PageLayout {
            surface,
            coverage: offsets,
        })
    }

    fn release(&mut self, resident: &Resident) {
        self.surfaces
            .release(resident.layout.surface, resident.surface_samples);
        for (&offset, &samples) in resident
            .layout
            .coverage
            .iter()
            .zip(&resident.coverage_samples)
        {
            self.coverage.release(offset, samples);
        }
    }

    /// Capacity for the whole scene with room for pages to come and go before the next
    /// regrowth. Resident ranges are forgotten: the caller replaces both buffers.
    fn regrow(&mut self, pages: &[VegetationFieldPage]) -> Regrow {
        let surfaces: u64 = pages.iter().map(|p| u64::from(surface_samples(p))).sum();
        let coverage: u64 = pages
            .iter()
            .flat_map(|p| p.fields.iter().map(|f| f.coverage.len() as u64))
            .sum();
        self.pages.clear();
        Regrow {
            surface_samples: surfaces + surfaces / 2,
            coverage_samples: coverage + coverage / 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freed_ranges_merge_and_are_reused_first_fit() {
        let mut ranges = RangeAllocator::with_capacity(10);
        let a = ranges.allocate(3).unwrap();
        let b = ranges.allocate(3).unwrap();
        let c = ranges.allocate(3).unwrap();
        assert_eq!((a, b, c), (0, 3, 6));
        assert_eq!(ranges.allocate(2), None);
        ranges.release(a, 3);
        ranges.release(c, 3);
        assert_eq!(ranges.free, vec![(0, 3), (6, 4)]);
        ranges.release(b, 3);
        assert_eq!(ranges.free, vec![(0, 10)]);
        assert_eq!(ranges.allocate(10), Some(0));
        assert_eq!(ranges.allocate(0), Some(0));
    }

    fn pages(count: usize) -> Vec<VegetationFieldPage> {
        let page = vegetation::fixtures::reference_scene().pages[0].clone();
        (0..count)
            .map(|i| {
                let mut page = page.clone();
                page.origin_xz[0] += i as f32 * page.size;
                page
            })
            .collect()
    }

    fn sizes(page: &VegetationFieldPage) -> (u32, u32) {
        (
            surface_samples(page),
            coverage_samples(page).iter().sum::<u32>(),
        )
    }

    #[test]
    fn unchanged_pages_keep_their_samples_and_only_arrivals_upload() {
        let pages = pages(4);
        let (surface, coverage) = sizes(&pages[0]);
        let capacity = (surface * 5, coverage * 5);
        let mut slots = PageSlots::default();
        slots.reset(capacity);
        let first = slots.place(&pages[..3], &[1, 2, 3], capacity).unwrap();
        assert_eq!(first.uploads, vec![0, 1, 2]);
        // Page 1 leaves, page 4 arrives, page 3 moves in the list: only page 4 uploads, into
        // the range page 1 freed, and the others keep their ranges.
        let next = slots
            .place(
                &[pages[2].clone(), pages[0].clone(), pages[3].clone()],
                &[3, 1, 4],
                capacity,
            )
            .unwrap();
        assert_eq!(next.uploads, vec![2]);
        assert_eq!(next.layouts[0], first.layouts[2]);
        assert_eq!(next.layouts[1], first.layouts[0]);
        assert_eq!(next.layouts[2].surface, first.layouts[1].surface);
    }

    #[test]
    fn unkeyed_scenes_pack_every_page_in_order() {
        let pages = pages(3);
        let (surface, coverage) = sizes(&pages[0]);
        let capacity = (surface * 4, coverage * 4);
        let mut slots = PageSlots::default();
        slots.reset(capacity);
        slots.place(&pages, &[7, 8, 9], capacity).unwrap();
        for keys in [&[][..], &[5, 5, 6][..]] {
            let plan = slots.place(&pages, keys, capacity).unwrap();
            assert_eq!(plan.uploads, vec![0, 1, 2]);
            // The same compact layout as the reference packer.
            assert_eq!(plan.layouts[0].surface, 0);
            assert_eq!(plan.layouts[1].surface, surface);
            assert_eq!(plan.layouts[2].surface, 2 * surface);
        }
    }

    #[test]
    fn a_full_buffer_asks_for_room_with_headroom() {
        let pages = pages(3);
        let (surface, coverage) = sizes(&pages[0]);
        let mut slots = PageSlots::default();
        let small = (surface * 2, coverage * 2);
        slots.reset(small);
        let regrow = slots.place(&pages, &[1, 2, 3], small).unwrap_err();
        assert_eq!(
            regrow,
            Regrow {
                surface_samples: u64::from(surface) * 3 * 3 / 2,
                coverage_samples: u64::from(coverage) * 3 * 3 / 2,
            }
        );
        let large = (
            regrow.surface_samples as u32,
            regrow.coverage_samples as u32,
        );
        slots.reset(large);
        assert_eq!(
            slots.place(&pages, &[1, 2, 3], large).unwrap().uploads,
            vec![0, 1, 2]
        );
    }
}
