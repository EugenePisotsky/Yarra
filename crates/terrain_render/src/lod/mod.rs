//! Camera-driven terrain cover planning. IO/upload readiness is deliberately separate:
//! a caller stages the complete result and retains its previous cover until ready.
use bevy::math::{DMat4, DVec3};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use world::TerrainNodeKey;

mod mesh;
pub use mesh::{StitchEdges, build_patch_mesh, stitch_indices};
#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct PatchMetadata {
    pub key: TerrainNodeKey,
    pub height_bounds: [f32; 2],
    pub geometric_error: f32,
    pub resolution: u16,
}
impl PatchMetadata {
    pub fn bounds(&self, cell_size: f64) -> [DVec3; 2] {
        let span = (1_u64 << self.key.level) as f64 * cell_size;
        let x = self.key.x as f64 * span;
        let z = self.key.z as f64 * span;
        [
            DVec3::new(x, self.height_bounds[0] as f64, z),
            DVec3::new(x + span, self.height_bounds[1] as f64, z + span),
        ]
    }
    fn triangles(&self) -> usize {
        2 * usize::from(self.resolution - 1).pow(2)
    }
}

/// Canonical world coordinates use f64; render-origin changes do not alter selection.
#[derive(Clone, Debug, PartialEq)]
pub struct LodView {
    pub clip_from_world: DMat4,
    pub viewport: [u32; 2],
    pub contact_position: DVec3,
}
impl LodView {
    pub fn visible(&self, bounds: [DVec3; 2]) -> bool {
        let corners = corners(bounds).map(|p| self.clip_from_world * p.extend(1.0));
        // WebGPU clip space: -w <= x,y <= w and 0 <= z <= w, including reverse Z.
        !(0..6).any(|plane| {
            corners.iter().all(|p| match plane {
                0 => p.x < -p.w,
                1 => p.x > p.w,
                2 => p.y < -p.w,
                3 => p.y > p.w,
                4 => p.z < 0.0,
                _ => p.z > p.w,
            })
        })
    }
    /// Bound the screen displacement of a vertical height error, including changes in
    /// perspective W. Using XZ distance alone badly over-refines a valley below a summit.
    pub fn projected_error(&self, bounds: [DVec3; 2], error: f32) -> f64 {
        if error == 0.0 {
            return 0.0;
        }
        let e = error as f64;
        let vertical = self.clip_from_world.y_axis;
        let w = corners(bounds)
            .into_iter()
            .map(|p| (self.clip_from_world * p.extend(1.0)).w)
            .fold(f64::INFINITY, f64::min)
            - e * vertical.w.abs();
        if w <= 1e-9 {
            return f64::INFINITY;
        }
        let x = (vertical.x.abs() + vertical.w.abs()) * self.viewport[0] as f64;
        let y = (vertical.y.abs() + vertical.w.abs()) * self.viewport[1] as f64;
        e * x.max(y) * 0.5 / w
    }
    fn distance(&self, bounds: [DVec3; 2]) -> f64 {
        self.contact_position
            .distance(self.contact_position.clamp(bounds[0], bounds[1]))
    }
}
fn corners(bounds: [DVec3; 2]) -> [DVec3; 8] {
    std::array::from_fn(|i| {
        DVec3::new(
            bounds[i & 1].x,
            bounds[(i >> 1) & 1].y,
            bounds[(i >> 2) & 1].z,
        )
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct LodSettings {
    pub refine_pixels: f64,
    pub collapse_pixels: f64,
    pub exact_radius: f64,
    pub contact_radius: f64,
    pub contact_tolerance: f32,
    pub max_patches: usize,
    pub max_triangles: usize,
    pub max_work: usize,
    pub max_requests: usize,
}
impl Default for LodSettings {
    fn default() -> Self {
        Self {
            refine_pixels: 2.0,
            collapse_pixels: 1.0,
            exact_radius: 8.0,
            contact_radius: 96.0,
            contact_tolerance: 0.01,
            max_patches: 512,
            max_triangles: 1_048_576,
            max_work: 1_000_000,
            max_requests: 128,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct CoverStats {
    pub work: usize,
    pub triangles: usize,
    pub budget_limited: bool,
    pub maximum_visible_error: f64,
    pub contact_limited: bool,
}
#[derive(Clone, Debug)]
pub struct PlannedCover {
    pub patches: BTreeMap<TerrainNodeKey, StitchEdges>,
    pub requests: BTreeSet<TerrainNodeKey>,
    pub stats: CoverStats,
    /// False means the initial sparse root cover still needs balancing before drawing.
    pub balanced: bool,
}

struct Planner<'a> {
    metadata: &'a BTreeMap<TerrainNodeKey, PatchMetadata>,
    settings: &'a LodSettings,
    cover: BTreeSet<TerrainNodeKey>,
    requests: BTreeSet<TerrainNodeKey>,
    stats: CoverStats,
}
impl Planner<'_> {
    fn spend(&mut self) -> bool {
        if self.stats.work >= self.settings.max_work {
            self.stats.budget_limited = true;
            false
        } else {
            self.stats.work += 1;
            true
        }
    }
    fn split(&mut self, key: TerrainNodeKey) -> bool {
        let Some(children) = key.children().ok().flatten() else {
            return false;
        };
        let mut ready = true;
        for child in children {
            if !self.metadata.contains_key(&child) {
                if self.requests.len() < self.settings.max_requests {
                    self.requests.insert(child);
                } else {
                    self.stats.budget_limited = true;
                }
                ready = false;
            }
        }
        if !ready {
            return false;
        }
        let triangles = self.stats.triangles - self.metadata[&key].triangles()
            + children
                .iter()
                .map(|k| self.metadata[k].triangles())
                .sum::<usize>();
        if self.cover.len() + 3 > self.settings.max_patches
            || triangles > self.settings.max_triangles
        {
            self.stats.budget_limited = true;
            return false;
        }
        self.cover.remove(&key);
        self.cover.extend(children);
        self.stats.triangles = triangles;
        true
    }
    fn refine(&mut self, key: TerrainNodeKey) -> Vec<TerrainNodeKey> {
        // Compute the complete balancing dependency group before changing ownership.
        // Admission is atomic; independently splitting/coarsening neighbours can cycle.
        let mut group = BTreeSet::from([key]);
        let mut todo = vec![key];
        let current: Vec<_> = self.cover.iter().copied().collect();
        while let Some(candidate) = todo.pop() {
            for &other in &current {
                if !self.spend() {
                    return vec![];
                }
                if other.level > candidate.level
                    && adjacent(candidate, other).is_some()
                    && group.insert(other)
                {
                    todo.push(other);
                }
            }
        }
        let mut ready = true;
        let mut children = Vec::new();
        let mut triangles = self.stats.triangles;
        for parent in &group {
            triangles -= self.metadata[parent].triangles();
            for child in parent.children().unwrap().unwrap() {
                if let Some(m) = self.metadata.get(&child) {
                    triangles += m.triangles();
                } else {
                    ready = false;
                    if self.requests.len() < self.settings.max_requests {
                        self.requests.insert(child);
                    } else {
                        self.stats.budget_limited = true;
                    }
                }
                children.push(child);
            }
        }
        if self.cover.len() + 3 * group.len() > self.settings.max_patches
            || triangles > self.settings.max_triangles
        {
            self.stats.budget_limited = true;
            return vec![];
        }
        if !ready {
            return vec![];
        }
        for parent in group {
            self.cover.remove(&parent);
        }
        self.cover.extend(children.iter().copied());
        self.stats.triangles = triangles;
        children
    }
}

/// Every result covers exactly the root forest, including off-screen ground. Frustum
/// tests choose refinement priorities, not whether a fallback exists after a teleport.
pub fn plan_cover(
    roots: &[TerrainNodeKey],
    metadata: &BTreeMap<TerrainNodeKey, PatchMetadata>,
    previous: &BTreeSet<TerrainNodeKey>,
    view: &LodView,
    cell_size: f64,
    settings: &LodSettings,
) -> Result<PlannedCover, String> {
    if !cell_size.is_finite()
        || cell_size <= 0.0
        || settings.max_work == 0
        || settings.max_requests < 4
        || settings.refine_pixels <= settings.collapse_pixels
        || settings.collapse_pixels <= 0.0
        || !settings.refine_pixels.is_finite()
        || !settings.collapse_pixels.is_finite()
        || !settings.exact_radius.is_finite()
        || !settings.contact_radius.is_finite()
        || settings.exact_radius < 0.0
        || settings.contact_radius < settings.exact_radius
        || !settings.contact_tolerance.is_finite()
        || settings.contact_tolerance < 0.0
        || view.viewport.contains(&0)
        || !view.clip_from_world.is_finite()
        || !view.contact_position.is_finite()
    {
        return Err("invalid terrain LOD profile/view".into());
    }
    for (key, m) in metadata {
        key.cell_bounds().map_err(|e| e.to_string())?;
        if *key != m.key
            || m.resolution < 3
            || !(m.resolution - 1).is_power_of_two()
            || m.resolution > 257
            || !m.geometric_error.is_finite()
            || m.geometric_error < 0.0
            || !m.height_bounds.iter().all(|v| v.is_finite())
            || m.height_bounds[0] > m.height_bounds[1]
        {
            return Err("invalid terrain LOD metadata".into());
        }
    }
    for (i, &a) in roots.iter().enumerate() {
        if !metadata.contains_key(&a) {
            return Err("missing terrain root metadata".into());
        }
        if roots[..i].iter().any(|&b| contains(a, b) || contains(b, a)) {
            return Err("overlapping terrain roots".into());
        }
    }
    let triangles = roots.iter().map(|k| metadata[k].triangles()).sum();
    if roots.len() > settings.max_patches || triangles > settings.max_triangles {
        return Err("coarse terrain cover exceeds profile budget".into());
    }
    let mut p = Planner {
        metadata,
        settings,
        cover: roots.iter().copied().collect(),
        requests: BTreeSet::new(),
        stats: CoverStats {
            triangles,
            ..Default::default()
        },
    };
    let score = |key: TerrainNodeKey| {
        let m = &metadata[&key];
        let bounds = m.bounds(cell_size);
        let distance = view.distance(bounds);
        if distance < settings.exact_radius && key.level > 0 {
            return f64::INFINITY;
        }
        if distance < settings.contact_radius && m.geometric_error > settings.contact_tolerance {
            return f64::MAX;
        }
        if !view.visible(bounds) {
            return 0.0;
        }
        let refined = previous.iter().any(|&k| k != key && contains(key, k));
        view.projected_error(bounds, m.geometric_error)
            / if refined {
                settings.collapse_pixels
            } else {
                settings.refine_pixels
            }
    };
    // Sparse root forests may start unbalanced. Prepare their minimum balanced
    // cover before publishing anything; a missing child is pending, never absent.
    let mut balanced = true;
    'balance: loop {
        let keys: Vec<_> = p.cover.iter().copied().collect();
        for (i, &a) in keys.iter().enumerate() {
            for &b in &keys[i + 1..] {
                if !p.spend() {
                    balanced = false;
                    break 'balance;
                }
                if adjacent(a, b).is_some() && a.level.abs_diff(b.level) > 1 {
                    let coarse = if a.level > b.level { a } else { b };
                    if !p.split(coarse) {
                        balanced = false;
                        break 'balance;
                    }
                    continue 'balance;
                }
            }
        }
        break;
    }
    if balanced {
        let mut queue: BinaryHeap<_> = p.cover.iter().map(|&k| Candidate(score(k), k)).collect();
        while let Some(Candidate(priority, key)) = queue.pop() {
            if p.stats.work + p.cover.len() * p.cover.len() >= settings.max_work {
                p.stats.budget_limited = true;
                break;
            }
            if !p.spend() {
                break;
            }
            if key.level == 0 || priority <= 1.0 || !p.cover.contains(&key) {
                continue;
            }
            for child in p.refine(key) {
                queue.push(Candidate(score(child), child));
            }
        }
    }
    if balanced {
        let mut blocked = BTreeSet::new();
        'edges: loop {
            let keys: Vec<_> = p.cover.iter().copied().collect();
            // Reserve a complete final seam pass even if quality refinement runs out
            // of work. Returning an unbalanced half-built seam set is never useful.
            if p.stats.work + 2 * keys.len() * keys.len() >= settings.max_work {
                p.stats.budget_limited = true;
                break;
            }
            for (i, &a) in keys.iter().enumerate() {
                for &b in &keys[i + 1..] {
                    if !p.spend() {
                        break 'edges;
                    }
                    if a.level.abs_diff(b.level) != 1 || adjacent(a, b).is_none() {
                        continue;
                    }
                    let (fine, coarse) = if a.level < b.level { (a, b) } else { (b, a) };
                    if blocked.contains(&coarse) {
                        continue;
                    }
                    let bounds = metadata[&fine].bounds(cell_size);
                    let error = fine
                        .parent()
                        .ok()
                        .flatten()
                        .and_then(|k| metadata.get(&k))
                        .map_or(metadata[&fine].geometric_error, |m| m.geometric_error);
                    let contact = view.distance(bounds) < settings.contact_radius
                        && error > settings.contact_tolerance;
                    let exact = view.distance(bounds) < settings.exact_radius && error > 0.0;
                    let visible = view.visible(bounds)
                        && view.projected_error(bounds, error) > settings.refine_pixels;
                    if contact || exact || visible {
                        if !p.refine(coarse).is_empty() {
                            continue 'edges;
                        }
                        blocked.insert(coarse);
                    }
                }
            }
            break;
        }
    }
    let mut patches: BTreeMap<_, _> = p
        .cover
        .iter()
        .map(|&k| (k, StitchEdges::default()))
        .collect();
    if balanced {
        let keys: Vec<_> = p.cover.iter().copied().collect();
        for (i, &a) in keys.iter().enumerate() {
            for &b in &keys[i + 1..] {
                if !p.spend() {
                    balanced = false;
                    break;
                }
                if a.level + 1 == b.level
                    && let Some(edge) = adjacent(a, b)
                {
                    patches.get_mut(&a).unwrap().insert(edge);
                }
                if b.level + 1 == a.level
                    && let Some(edge) = adjacent(b, a)
                {
                    patches.get_mut(&b).unwrap().insert(edge);
                }
            }
            if !balanced {
                break;
            }
        }
    }
    for key in patches.keys() {
        let m = &metadata[key];
        let bounds = m.bounds(cell_size);
        // Stitched patches use some parent samples along the edge. Include that error
        // when reporting contact/quality, even though the body uses finer samples.
        let error = if patches[key].0 != 0 {
            key.parent()
                .ok()
                .flatten()
                .and_then(|k| metadata.get(&k))
                .map_or(m.geometric_error, |m| m.geometric_error)
        } else {
            m.geometric_error
        };
        if view.visible(bounds) {
            p.stats.maximum_visible_error = p
                .stats
                .maximum_visible_error
                .max(view.projected_error(bounds, error));
        }
        p.stats.contact_limited |= (view.distance(bounds) < settings.exact_radius && key.level > 0)
            || (view.distance(bounds) < settings.contact_radius
                && error > settings.contact_tolerance);
    }
    Ok(PlannedCover {
        patches,
        requests: p.requests,
        stats: p.stats,
        balanced,
    })
}

pub fn contains(parent: TerrainNodeKey, child: TerrainNodeKey) -> bool {
    if parent.space != child.space || parent.level < child.level {
        return false;
    }
    let shift = u32::from(parent.level - child.level);
    child.x.checked_shr(shift) == Some(parent.x) && child.z.checked_shr(shift) == Some(parent.z)
}
/// Edge of A touching B, with positive edge length (corner-only contacts excluded).
fn adjacent(a: TerrainNodeKey, b: TerrainNodeKey) -> Option<u8> {
    if a.space != b.space {
        return None;
    }
    let rect = |k: TerrainNodeKey| {
        let s = 1_i64 << k.level;
        [
            k.x as i64 * s,
            k.z as i64 * s,
            (k.x as i64 + 1) * s,
            (k.z as i64 + 1) * s,
        ]
    };
    let a = rect(a);
    let b = rect(b);
    if a[1].max(b[1]) < a[3].min(b[3]) {
        if a[0] == b[2] {
            return Some(StitchEdges::WEST);
        }
        if a[2] == b[0] {
            return Some(StitchEdges::EAST);
        }
    }
    if a[0].max(b[0]) < a[2].min(b[2]) {
        if a[1] == b[3] {
            return Some(StitchEdges::SOUTH);
        }
        if a[3] == b[1] {
            return Some(StitchEdges::NORTH);
        }
    }
    None
}
#[derive(PartialEq)]
struct Candidate(f64, TerrainNodeKey);
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .total_cmp(&other.0)
            .then_with(|| other.1.cmp(&self.1))
    }
}
