//! Optional canopy work never runs inside synchronous scene packing.
use super::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    revision: u64,
    origin: [u32; 2],
}

#[derive(Default)]
struct Request {
    latest: Option<Key>,
    changed_at: Option<Instant>,
    published: Option<Key>,
}
impl Request {
    fn set(&mut self, key: Option<Key>, now: Instant) -> bool {
        if self.latest == key {
            return false;
        }
        // A stale coordinate frame must never shade the newly rebased world.
        let clear = key.is_none() || self.latest.map(|k| k.origin) != key.map(|k| k.origin);
        self.latest = key;
        self.changed_at = Some(now);
        if clear {
            self.published = None;
        }
        clear
    }
    fn should_start(&self, now: Instant) -> bool {
        self.latest.is_some()
            && self.latest != self.published
            && self
                .changed_at
                .is_some_and(|t| now.duration_since(t) >= SETTLE)
    }
    fn accept(&mut self, key: Key) -> bool {
        if self.latest != Some(key) {
            return false;
        }
        self.published = Some(key);
        true
    }
}

pub(super) struct CanopyBoundary {
    pub buffer: Buffer,
    request: Request,
    task: Option<Task<(Key, Vec<f32>)>>,
}
impl CanopyBoundary {
    pub fn new(device: &RenderDevice) -> Self {
        Self {
            buffer: dummy_storage(device, "empty canopy boundary"),
            request: Request::default(),
            task: None,
        }
    }

    /// Poll once; never wait. Returning true means the draw binding needs the new buffer.
    pub fn update(
        &mut self,
        scene: &VegetationDebugScene,
        lighting: &VegetationLighting,
        device: &RenderDevice,
        queue: &RenderQueue,
    ) -> bool {
        let enabled = lighting.canopy.enabled
            && lighting.canopy.strength > 0.0
            && lighting.canopy.blade_amount > 0.0
            && !scene.scene().pages.is_empty();
        let key = enabled.then_some(Key {
            revision: scene.revision(),
            origin: lighting.canopy_origin.map(f32::to_bits),
        });
        let now = Instant::now();
        if self.request.set(key, now) {
            // Width zero marks an unavailable field, even when the allocation is retained.
            queue.write_buffer(&self.buffer, 0, &[0; 16]);
        }
        let mut rebound = false;
        if let Some((finished, values)) = self.task.as_mut().and_then(check_ready) {
            self.task = None;
            if self.request.accept(finished) {
                if let Some((buffer, _)) = update_storage(
                    device,
                    queue,
                    &self.buffer,
                    self.buffer.size(),
                    "canopy boundary",
                    bytemuck::cast_slice(&values),
                ) {
                    self.buffer = buffer;
                    rebound = true;
                }
            }
        }
        // Coalesce streaming bursts. One job may finish after disabling; its result is discarded.
        // Keeping that task until completion prevents overlapping jobs after rapid toggles.
        if self.task.is_none() && self.request.should_start(now) {
            let key = self.request.latest.unwrap();
            let source = scene.scene().clone();
            self.task = Some(AsyncComputeTaskPool::get().spawn(async move {
                let field = crate::canopy_coverage::BoundaryField::for_scene(
                    &source.catalog,
                    &source.pages,
                );
                (key, field.gpu_values())
            }));
        }
        rebound
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_canopy_never_schedules_or_accepts_a_late_result() {
        let now = Instant::now();
        let key = Key {
            revision: 1,
            origin: [0, 0],
        };
        let mut request = Request::default();
        request.set(None, now);
        assert!(!request.should_start(now + SETTLE));
        request.set(Some(key), now);
        assert!(request.should_start(now + SETTLE));
        assert!(request.set(None, now + SETTLE));
        assert!(!request.should_start(now + SETTLE * 2));
        assert!(!request.accept(key));
    }
    #[test]
    fn streaming_bursts_coalesce_and_old_results_cannot_replace_the_latest_field() {
        let now = Instant::now();
        let a = Key {
            revision: 1,
            origin: [0, 0],
        };
        let b = Key { revision: 2, ..a };
        let mut request = Request::default();
        request.set(Some(a), now);
        assert!(!request.should_start(now + SETTLE / 2));
        assert!(request.accept(a));
        assert!(!request.should_start(now + SETTLE));
        assert!(!request.set(Some(b), now + SETTLE));
        assert_eq!(request.published, Some(a));
        assert!(!request.accept(a));
        assert!(!request.should_start(now + SETTLE * 3 / 2));
        assert!(request.should_start(now + SETTLE * 2));
        let rebased = Key {
            revision: 3,
            origin: [32.0f32.to_bits(), 0],
        };
        assert!(request.set(Some(rebased), now + SETTLE * 2));
        assert_eq!(request.published, None);
    }
}
