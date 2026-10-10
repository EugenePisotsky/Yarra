//! Distance-driven terrain cover planning. IO/upload readiness is deliberately separate:
//! a caller stages the complete result and retains its previous cover until ready.
use bevy::math::{DMat4, DVec3};
use std::collections::{BTreeMap, BTreeSet};
use world::TerrainNodeKey;

mod allocation;
mod neighbours;
use neighbours::EDGES;
mod mesh;
pub(crate) use mesh::stitch_indices;
pub use mesh::{StitchEdges, build_patch_mesh};
mod morph;
pub use morph::{MorphMesh, MorphSource, build_morph_mesh, common_cover, touches};
pub mod contact;
use contact::{ContactRegion, patch_error};
#[cfg(test)]
mod tests;

/// Node sets and maps with Bevy's fixed foldhash: SipHash lookups were a large part of a
/// full-budget plan. Iteration order never decides a result; ordered covers stay BTree.
type KeySet = bevy::platform::collections::HashSet<TerrainNodeKey>;
type KeyMap<V> = bevy::platform::collections::HashMap<TerrainNodeKey, V>;

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
    /// A rotation-independent screen-error bound. Detail must already exist behind
    /// the camera: frustum-driven coarsening otherwise exposes a coarse cover for
    /// seconds after a turn, even when all fine meshes are cached.
    ///
    /// Row lengths recover projection scale independently of camera rotation. In
    /// perspective, use 3D distance to the bounds and the worst point in the view
    /// cone (including its corners). Orthographic error is independent of distance.
    pub fn distance_error(&self, bounds: [DVec3; 2], error: f32) -> f64 {
        if error == 0.0 {
            return 0.0;
        }
        let rows = self.clip_from_world.transpose();
        let x = rows.x_axis.truncate().length();
        let y = rows.y_axis.truncate().length();
        let w = rows.w_axis.truncate().length();
        let pixels = (x * self.viewport[0] as f64).max(y * self.viewport[1] as f64) * 0.5;
        let e = f64::from(error);
        if w < 1e-9 {
            return e * pixels / rows.w_axis.w.abs().max(1e-9);
        }
        let cosine = (1.0 + (w / x).powi(2) + (w / y).powi(2)).sqrt().recip();
        let depth = self.distance(bounds) * cosine - e;
        if depth <= 1e-9 {
            return f64::INFINITY;
        }
        e * pixels / (w * cosine * depth)
    }

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
        if !error.is_finite() {
            return f64::INFINITY;
        }
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
            // Desktop views across a whole island need about 1,100 patches (2.3M triangles)
            // for the 2 px target. Phones keep the earlier budget until measured there.
            max_patches: if cfg!(target_os = "ios") { 512 } else { 2048 },
            max_triangles: if cfg!(target_os = "ios") {
                1_048_576
            } else {
                4_194_304
            },
            max_work: 1_000_000,
            // Each plan refines at most a quarter of this many nodes before its morph, so
            // it sets how quickly a large cover arrives after a teleport or start.
            max_requests: if cfg!(target_os = "ios") { 128 } else { 512 },
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
    /// Missing metadata, most urgent first: callers that load only a prefix must keep this
    /// order, or a large visual demand can starve the terrain under an actor.
    pub requests: Vec<TerrainNodeKey>,
    pub stats: CoverStats,
    /// False means the initial sparse root cover still needs balancing before drawing.
    pub balanced: bool,
}

struct Planner<'a> {
    metadata: &'a BTreeMap<TerrainNodeKey, PatchMetadata>,
    settings: &'a LodSettings,
    /// Ordered for deterministic passes; `members` holds the same nodes for lookups.
    cover: BTreeSet<TerrainNodeKey>,
    members: KeySet,
    /// Strict ancestors of `cover`; the planner only splits, so this only grows.
    interior: KeySet,
    requests: Vec<TerrainNodeKey>,
    requested: KeySet,
    stats: CoverStats,
}
impl Planner<'_> {
    /// Demand is refined in priority order, so requests are recorded in that order.
    fn request(&mut self, key: TerrainNodeKey) {
        if self.requested.contains(&key) {
            return;
        }
        if self.requests.len() < self.settings.max_requests {
            self.requests.push(key);
            self.requested.insert(key);
        } else {
            self.stats.budget_limited = true;
        }
    }
    fn spend(&mut self) -> bool {
        if self.stats.work >= self.settings.max_work {
            self.stats.budget_limited = true;
            false
        } else {
            self.stats.work += 1;
            true
        }
    }
    // Leave enough work for the final seam pass (one lookup per patch edge). A
    // work-limited quality plan must still return a balanced, drawable cover.
    fn spend_selection(&mut self) -> bool {
        if self.stats.work + EDGES.len() * self.cover.len() + 1 >= self.settings.max_work {
            self.stats.budget_limited = true;
            false
        } else {
            self.spend()
        }
    }
    fn split(&mut self, key: TerrainNodeKey) -> bool {
        let Some(children) = key.children().ok().flatten() else {
            return false;
        };
        let mut ready = true;
        for child in children {
            if !self.metadata.contains_key(&child) {
                self.request(child);
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
        self.members.remove(&key);
        self.cover.extend(children);
        self.members.extend(children);
        self.interior.insert(key);
        self.stats.triangles = triangles;
        true
    }
    fn refine(&mut self, key: TerrainNodeKey) -> Vec<TerrainNodeKey> {
        // Compute the complete balancing dependency group before changing ownership.
        // Admission is atomic; independently splitting/coarsening neighbours can cycle.
        let mut group = BTreeSet::from([key]);
        let mut todo = vec![key];
        while let Some(candidate) = todo.pop() {
            for edge in EDGES {
                if !self.spend_selection() {
                    return vec![];
                }
                if let Some(other) =
                    neighbours::coarser(&self.members, &self.interior, candidate, edge)
                    && other.level > candidate.level
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
                    self.request(child);
                }
                children.push(child);
            }
        }
        let next_count = self.cover.len() + 3 * group.len();
        if next_count > self.settings.max_patches
            || triangles > self.settings.max_triangles
            || self.stats.work + EDGES.len() * next_count >= self.settings.max_work
        {
            self.stats.budget_limited = true;
            return vec![];
        }
        if !ready {
            return vec![];
        }
        for parent in group {
            self.cover.remove(&parent);
            self.members.remove(&parent);
            self.interior.insert(parent);
        }
        self.cover.extend(children.iter().copied());
        self.members.extend(children.iter().copied());
        self.stats.triangles = triangles;
        children
    }
}

/// Every result covers exactly the root forest. Distance chooses detail in every
/// direction; the renderer's frustum culling decides which resident patches draw.
pub fn plan_cover(
    roots: &[TerrainNodeKey],
    metadata: &BTreeMap<TerrainNodeKey, PatchMetadata>,
    previous: &BTreeSet<TerrainNodeKey>,
    view: &LodView,
    cell_size: f64,
    settings: &LodSettings,
) -> Result<PlannedCover, String> {
    plan_cover_with_contacts(roots, metadata, previous, view, cell_size, settings, &[])
}

pub fn plan_cover_with_contacts(
    roots: &[TerrainNodeKey],
    metadata: &BTreeMap<TerrainNodeKey, PatchMetadata>,
    previous: &BTreeSet<TerrainNodeKey>,
    view: &LodView,
    cell_size: f64,
    settings: &LodSettings,
    contacts: &[ContactRegion],
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
        || contacts.len() > contact::MAX_CONTACT_REGIONS
        || contacts.iter().any(|r| !r.validate())
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
        members: roots.iter().copied().collect(),
        interior: neighbours::interior_of(roots),
        requests: Vec::new(),
        requested: KeySet::new(),
        stats: CoverStats {
            triangles,
            ..Default::default()
        },
    };
    // Sparse root forests may start unbalanced. Prepare their minimum balanced
    // cover before publishing anything; a missing child is pending, never absent.
    let mut balanced = true;
    // Nodes that must split but whose children are still loading. Scanning on requests
    // every one of them in this plan, rather than one per plan.
    let mut waiting = KeySet::new();
    // Every unbalanced pair is seen from its finer node, as a coarser neighbour.
    'balance: loop {
        let keys: Vec<_> = p.cover.iter().copied().collect();
        for &a in &keys {
            for edge in EDGES {
                if !p.spend() {
                    balanced = false;
                    break 'balance;
                }
                if let Some(coarse) = neighbours::coarser(&p.members, &p.interior, a, edge)
                    && coarse.level > a.level + 1
                    && !waiting.contains(&coarse)
                {
                    let loading = coarse
                        .children()
                        .ok()
                        .flatten()
                        .is_some_and(|c| c.iter().any(|k| !metadata.contains_key(k)));
                    if p.split(coarse) {
                        continue 'balance;
                    }
                    if !loading {
                        balanced = false;
                        break 'balance;
                    }
                    waiting.insert(coarse);
                }
            }
        }
        break;
    }
    balanced &= waiting.is_empty();
    if balanced {
        allocation::refine(&mut p, previous, view, cell_size, contacts);
    }
    let mut patches: BTreeMap<_, _> = p
        .cover
        .iter()
        .map(|&k| (k, StitchEdges::default()))
        .collect();
    if balanced {
        // A fine patch stitches every edge it shares with a patch one level coarser.
        let keys: Vec<_> = p.cover.iter().copied().collect();
        'seams: for &a in &keys {
            for edge in EDGES {
                if !p.spend() {
                    balanced = false;
                    break 'seams;
                }
                if neighbours::coarser(&p.members, &p.interior, a, edge)
                    .is_some_and(|b| b.level == a.level + 1)
                {
                    patches.get_mut(&a).unwrap().insert(edge);
                }
            }
        }
    }
    for key in patches.keys() {
        let m = &metadata[key];
        let bounds = m.bounds(cell_size);
        // Stitched patches use some parent samples along the edge. Include that error
        // when reporting contact/quality, even though the body uses finer samples.
        let error = patch_error(*key, patches[key], metadata);
        if view.visible(bounds) {
            p.stats.maximum_visible_error = p
                .stats
                .maximum_visible_error
                .max(view.projected_error(bounds, error));
        }
        p.stats.contact_limited |= (view.distance(bounds) < settings.exact_radius && key.level > 0)
            || (view.distance(bounds) < settings.contact_radius
                && error > settings.contact_tolerance)
            || contacts
                .iter()
                .any(|r| r.needs_refinement(bounds, key.level, error));
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
#[cfg(test)]
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
