//! The instance pool: every batch of every resident far-object block (and LOD lab tree) holds
//! a contiguous range of instances, placed relative to the batch's origin, so a rebase moves
//! only origins. The main world writes into data shared with the render world, which uploads
//! what changed ([`PoolData::take_uploads`]).
use super::super::impostor::ImpostorBatch;
use super::forms::TreeForms;
use super::{ALIVE, FORCED_MASK, FORCED_NOTHING, GpuInstance, MAX_INSTANCES};
use bevy::prelude::*;

use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Mutex},
};

/// What one batch's instances draw, for the LOD lab: `None` lets the distance pick;
/// `Some(n)` forces representation `n` (mesh LODs, then the impostor) at every distance,
/// `Some(usize::MAX)` none.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TreeOverride(pub(crate) Option<usize>);

impl TreeOverride {
    fn flags(self) -> u32 {
        match self.0 {
            None => 0,
            Some(n) if n < FORCED_NOTHING as usize - 1 => n as u32 + 1,
            Some(_) => FORCED_NOTHING,
        }
    }
}

/// The pool as the GPU sees it, shared by both worlds.
#[derive(Default)]
pub(super) struct PoolData {
    pub(super) instances: Vec<GpuInstance>,
    /// Each origin slot's render-space position, and 1 while its batch is visible.
    pub(super) origins: Vec<[f32; 4]>,
    /// Instance ranges written since the last upload, and ranges whose fade state restarts.
    dirty: Vec<Range<u32>>,
    restart: Vec<Range<u32>>,
    origins_dirty: bool,
}

/// What the render world uploads this frame.
pub(super) struct PoolUploads {
    pub(super) instances: Vec<(u32, Vec<GpuInstance>)>,
    pub(super) restart: Vec<Range<u32>>,
    pub(super) origins: Option<Vec<[f32; 4]>>,
    pub(super) len: u32,
}

impl PoolData {
    /// Takes every change since the last call; `everything` asks for the whole pool (a new
    /// buffer).
    pub(super) fn take_uploads(&mut self, everything: bool) -> PoolUploads {
        let len = self.instances.len() as u32;
        let whole = std::iter::once(0..len).collect::<Vec<_>>();
        let ranges = if everything {
            whole.clone()
        } else {
            merge(std::mem::take(&mut self.dirty))
        };
        self.dirty.clear();
        let restart = if everything {
            whole
        } else {
            merge(std::mem::take(&mut self.restart))
        };
        self.restart.clear();
        let origins = (everything || self.origins_dirty).then(|| self.origins.clone());
        self.origins_dirty = false;
        PoolUploads {
            instances: ranges
                .into_iter()
                .filter(|r| r.start < r.end)
                .map(|r| {
                    (
                        r.start,
                        self.instances[r.start as usize..r.end as usize].to_vec(),
                    )
                })
                .collect(),
            restart,
            origins,
            len,
        }
    }

    fn write(&mut self, start: u32, instances: &[GpuInstance], restart: bool) {
        let end = start + instances.len() as u32;

        if self.instances.len() < end as usize {
            self.instances.resize(end as usize, GpuInstance::default());
        }

        self.instances[start as usize..end as usize].copy_from_slice(instances);
        self.dirty.push(start..end);

        if restart {
            self.restart.push(start..end);
        }
    }

    fn set_origin(&mut self, slot: u32, origin: [f32; 4]) {
        let slot = slot as usize;

        if self.origins.len() <= slot {
            self.origins.resize(slot + 1, [0.0; 4]);
        }

        if self.origins[slot] != origin {
            self.origins[slot] = origin;
            self.origins_dirty = true;
        }
    }
}

/// Sorted, overlapping or touching ranges joined.
fn merge(mut ranges: Vec<Range<u32>>) -> Vec<Range<u32>> {
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<u32>> = Vec::with_capacity(ranges.len());

    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }

    merged
}

/// A batch's place in the pool.
#[derive(Clone, Debug)]
pub(super) struct Batch {
    range: Range<u32>,
    origin: u32,
    pub(super) form: u32,
    pub(super) flags: u32,
    /// Its origin (render space) and 1 while visible.
    pub(super) position: [f32; 4],
    /// Its instances' roots relative to the origin, and the range of their scales.
    pub(super) roots: (Vec3, Vec3),
    pub(super) scales: (f32, f32),
}

#[derive(Resource)]
pub(crate) struct TreePool {
    data: Arc<Mutex<PoolData>>,
    batches: HashMap<Entity, Batch>,
    /// Free instance ranges, `[start, end)`, sorted, and the pool's length.
    free: Vec<Range<u32>>,
    len: u32,
    free_origins: Vec<u32>,
    origins: u32,
    /// Instances per form.
    pub(super) counts: Vec<u32>,
    /// Counts batches coming and going, so the draw layout follows.
    pub(super) revision: u64,
}

impl Default for TreePool {
    fn default() -> Self {
        Self {
            data: Arc::new(Mutex::new(PoolData::default())),
            batches: HashMap::new(),
            free: Vec::new(),
            len: 0,
            free_origins: Vec::new(),
            origins: 0,
            counts: Vec::new(),
            revision: 0,
        }
    }
}

impl TreePool {
    pub(super) fn shared(&self) -> Arc<Mutex<PoolData>> {
        self.data.clone()
    }

    pub(super) fn batches(&self) -> impl Iterator<Item = &Batch> {
        self.batches.values()
    }

    fn allocate(&mut self, count: u32) -> Option<u32> {
        if let Some(i) = self.free.iter().position(|r| r.end - r.start >= count) {
            let start = self.free[i].start;
            self.free[i].start += count;

            if self.free[i].is_empty() {
                self.free.remove(i);
            }

            return Some(start);
        }

        // Grow, taking a free range at the end along.
        let start = match self.free.last() {
            Some(last) if last.end == self.len => self.free.pop().unwrap().start,
            _ => self.len,
        };
        let end = start
            .checked_add(count)
            .filter(|&end| end <= MAX_INSTANCES)?;
        self.len = self.len.max(end);
        Some(start)
    }

    fn release(&mut self, range: Range<u32>) {
        self.free.push(range);
        self.free = merge(std::mem::take(&mut self.free));
    }

    fn origin_slot(&mut self) -> u32 {
        self.free_origins.pop().unwrap_or_else(|| {
            self.origins += 1;
            self.origins - 1
        })
    }
}

fn origin_of(transform: &GlobalTransform, visible: bool) -> [f32; 4] {
    transform
        .translation()
        .extend(f32::from(u8::from(visible)))
        .to_array()
}

/// Gives every new batch whose form is known its range of the pool.
#[allow(clippy::type_complexity)]
pub(super) fn attach(
    mut pool: ResMut<TreePool>,
    forms: Res<TreeForms>,
    batches: Query<
        (
            Entity,
            &ImpostorBatch,
            &GlobalTransform,
            &InheritedVisibility,
            Option<&TreeOverride>,
        ),
        Added<ImpostorBatch>,
    >,
) {
    let pool = pool.as_mut();
    for (entity, batch, transform, visible, forced) in &batches {
        let Some(form) = forms.index(&batch.form.impostor) else {
            continue; // not drawable (see `forms::register`)
        };
        let count = batch.instances.len() as u32;
        let Some(start) = pool.allocate(count) else {
            warn_once!("GPU trees: the pool is full; a batch of {count} is not drawn");
            continue;
        };
        let origin = pool.origin_slot();
        let flags = ALIVE | forced.map_or(0, |f| f.flags());
        let instances: Vec<GpuInstance> = batch
            .instances
            .iter()
            .map(|instance| GpuInstance {
                position_yaw: instance.translation.extend(instance.yaw).to_array(),
                scale: instance.scale,
                form,
                origin,
                flags,
            })
            .collect();

        {
            let mut data = pool.data.lock().unwrap();
            data.write(start, &instances, true);
            data.set_origin(origin, origin_of(transform, visible.get()));
        }

        if pool.counts.len() <= form as usize {
            pool.counts.resize(form as usize + 1, 0);
        }

        pool.counts[form as usize] += count;
        let roots = batch
            .instances
            .iter()
            .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), i| {
                (lo.min(i.translation), hi.max(i.translation))
            });
        let scales = batch
            .instances
            .iter()
            .fold((f32::INFINITY, 0.0_f32), |(lo, hi), i| {
                (lo.min(i.scale), hi.max(i.scale))
            });
        pool.batches.insert(
            entity,
            Batch {
                range: start..start + count,
                origin,
                form,
                flags,
                position: origin_of(transform, visible.get()),
                roots,
                scales,
            },
        );
        pool.revision += 1;
    }
}

/// Batches leaving free their range; their instances stop drawing at once.
pub(super) fn release(removed: On<Remove<ImpostorBatch>>, mut pool: Option<ResMut<TreePool>>) {
    let Some(pool) = pool.as_deref_mut() else {
        return;
    };
    let Some(batch) = pool.batches.remove(&removed.entity) else {
        return;
    };
    let dead = vec![GpuInstance::default(); batch.range.len()];
    pool.data
        .lock()
        .unwrap()
        .write(batch.range.start, &dead, false);
    pool.counts[batch.form as usize] -= batch.range.len() as u32;
    pool.free_origins.push(batch.origin);
    pool.release(batch.range);
    pool.revision += 1;
}

/// Origins follow their batches (rebases, visibility); overrides rewrite their instances.
#[allow(clippy::type_complexity)]
pub(super) fn follow(
    mut pool: ResMut<TreePool>,
    batches: Query<
        (
            Entity,
            &ImpostorBatch,
            &GlobalTransform,
            &InheritedVisibility,
            Option<&TreeOverride>,
        ),
        Or<(
            Changed<GlobalTransform>,
            Changed<InheritedVisibility>,
            Changed<TreeOverride>,
        )>,
    >,
) {
    let pool = pool.as_mut();
    for (entity, batch, transform, visible, forced) in &batches {
        let Some(placed) = pool.batches.get_mut(&entity) else {
            continue;
        };
        let position = origin_of(transform, visible.get());
        let flags = ALIVE | forced.map_or(0, |f| f.flags());
        let mut data = pool.data.lock().unwrap();

        if placed.position != position {
            placed.position = position;
            data.set_origin(placed.origin, position);
        }

        if placed.flags != flags {
            placed.flags = flags;
            let mut instances =
                data.instances[placed.range.start as usize..placed.range.end as usize].to_vec();

            for (instance, source) in instances.iter_mut().zip(&batch.instances) {
                instance.flags = (instance.flags & !FORCED_MASK) | (flags & FORCED_MASK);
                debug_assert_eq!(instance.scale, source.scale);
            }

            data.write(placed.range.start, &instances, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freed_ranges_are_reused_and_joined() {
        let mut pool = TreePool::default();
        assert_eq!(pool.allocate(10), Some(0));
        assert_eq!(pool.allocate(5), Some(10));
        assert_eq!(pool.allocate(5), Some(15));
        pool.release(0..10);
        pool.release(15..20);

        // The end's free range grows in place; the first fits a small batch.
        assert_eq!(pool.allocate(8), Some(0));
        assert_eq!(pool.allocate(12), Some(15));
        assert_eq!(pool.len, 27);

        // 8..10 stayed free; giving back 0..8 joins them.
        pool.release(0..8);
        assert_eq!(pool.free, std::iter::once(0..10).collect::<Vec<_>>());
        assert_eq!(pool.allocate(MAX_INSTANCES), None);
    }

    #[test]
    fn uploads_are_merged_and_taken_once() {
        let mut data = PoolData::default();
        let one = |flags| GpuInstance { flags, ..default() };
        data.write(0, &[one(ALIVE); 4], true);
        data.write(2, &[one(ALIVE | 3); 4], false);
        data.set_origin(1, [1.0, 2.0, 3.0, 1.0]);
        let uploads = data.take_uploads(false);
        assert_eq!(uploads.len, 6);
        assert_eq!(uploads.instances.len(), 1);
        assert_eq!(uploads.instances[0].0, 0);
        assert_eq!(uploads.instances[0].1.len(), 6);
        assert_eq!(uploads.restart, std::iter::once(0..4).collect::<Vec<_>>());
        assert_eq!(uploads.origins.as_deref().map(<[_]>::len), Some(2));
        let again = data.take_uploads(false);
        assert!(again.instances.is_empty() && again.restart.is_empty());
        assert!(again.origins.is_none());
        assert_eq!(data.take_uploads(true).instances[0].1.len(), 6);
    }

    #[test]
    fn overrides_force_a_representation_or_nothing() {
        assert_eq!(TreeOverride(None).flags(), 0);
        assert_eq!(TreeOverride(Some(0)).flags(), 1);
        assert_eq!(TreeOverride(Some(3)).flags(), 4);
        assert_eq!(TreeOverride(Some(usize::MAX)).flags(), FORCED_NOTHING);
    }
}
