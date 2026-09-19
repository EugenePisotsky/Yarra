//! Body and stitched-edge demands share one queue. A visual error, including an
//! infinite camera-plane bound, never outranks terrain needed by a contact consumer.
use super::*;
use contact::ContactPriority;
use std::{cmp::Ordering, collections::BinaryHeap};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Priority(u8, f64);
impl Eq for Priority {}
impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .cmp(&other.0)
            .then_with(|| self.1.total_cmp(&other.1))
    }
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Candidate(Priority, std::cmp::Reverse<TerrainNodeKey>, u64);

struct Demand<'a> {
    previous: &'a BTreeSet<TerrainNodeKey>,
    view: &'a LodView,
    cell_size: f64,
    contacts: &'a [ContactRegion],
}
impl Demand<'_> {
    fn priority(&self, p: &Planner, key: TerrainNodeKey, error: f32) -> Priority {
        let bounds = p.metadata[&key].bounds(self.cell_size);
        let distance = self.view.distance(bounds);
        let near = 1. / (1. + distance);
        if let Some(priority) = self
            .contacts
            .iter()
            .filter(|r| r.needs_refinement(bounds, key.level, error))
            .map(|r| r.priority)
            .max()
        {
            return Priority(
                match priority {
                    ContactPriority::Actor => 4,
                    ContactPriority::Vegetation => 3,
                },
                near,
            );
        }
        if (distance < p.settings.exact_radius && (key.level > 0 || error > 0.))
            || (distance < p.settings.contact_radius && error > p.settings.contact_tolerance)
        {
            return Priority(2, near);
        }
        if !self.view.visible(bounds) {
            return Priority(0, 0.);
        }
        let refined = self.previous.iter().any(|&k| k != key && contains(key, k));
        let threshold = if refined {
            p.settings.collapse_pixels
        } else {
            p.settings.refine_pixels
        };
        let error = self.view.projected_error(bounds, error) / threshold;
        if error > 1. {
            Priority(1, error)
        } else {
            Priority(0, 0.)
        }
    }

    fn score(&self, p: &mut Planner, key: TerrainNodeKey) -> Result<Priority, ()> {
        if key.level == 0 {
            return Ok(Priority(0, 0.));
        }
        let mut priority = self.priority(p, key, p.metadata[&key].geometric_error);
        // Refining this coarse neighbour may be needed to certify a fine patch,
        // even when the coarse patch's own body doesn't intersect the consumer.
        for fine in p.cover.iter().copied().collect::<Vec<_>>() {
            if !p.spend_selection() {
                return Err(());
            }
            if fine.level + 1 == key.level && adjacent(fine, key).is_some() {
                let error = patch_error(fine, StitchEdges(1), p.metadata);
                priority = priority.max(self.priority(p, fine, error));
            }
        }
        Ok(priority)
    }
}

pub(super) fn refine(
    p: &mut Planner,
    previous: &BTreeSet<TerrainNodeKey>,
    view: &LodView,
    cell_size: f64,
    contacts: &[ContactRegion],
) {
    let demand = Demand {
        previous,
        view,
        cell_size,
        contacts,
    };
    let mut queue = BinaryHeap::new();
    let mut versions = BTreeMap::<TerrainNodeKey, u64>::new();
    let mut changed = p.cover.clone();
    loop {
        for key in changed {
            let Ok(priority) = demand.score(p, key) else {
                return;
            };
            let version = versions.entry(key).or_default();
            *version += 1;
            if priority.0 != 0 {
                queue.push(Candidate(priority, std::cmp::Reverse(key), *version));
            }
        }
        let Some(Candidate(_, std::cmp::Reverse(key), version)) = queue.pop() else {
            break;
        };
        changed = BTreeSet::new();
        if !p.spend_selection() {
            break;
        }
        if !p.cover.contains(&key) || versions.get(&key) != Some(&version) {
            continue;
        }
        let children = p.refine(key);
        if children.is_empty() {
            continue;
        }
        // Only split children and their neighbours can have changed body/seam
        // demand. Refresh those entries rather than rescanning every pair after
        // every split; versioned heap entries discard stale higher priorities.
        changed.extend(children.iter().copied());
        let current: Vec<_> = p.cover.iter().copied().collect();
        for child in children {
            for &other in &current {
                if !p.spend_selection() {
                    return;
                }
                if adjacent(child, other).is_some() {
                    changed.insert(other);
                }
            }
        }
    }
}
