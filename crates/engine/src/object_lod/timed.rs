//! LOD changes over time, for objects whose last LOD is an impostor (trees and shrubs).
//!
//! A distance band crossfade keeps a tree half dissolved for as long as the camera takes to
//! cross the band, and Bevy's shadow pass does not dither, so shadows switched at once.
//! Here the distance only decides which representation draws (with a little hysteresis);
//! a change dissolves the old one into the new over [`FADE_SECONDS`] at any speed. Fading
//! meshes carry their dither level in their mesh tag, which the tree shaders read in every
//! pass, shadows included (`shaders/tree_wind.wgsl`). The impostor's level goes to its
//! instance's slot in [`ImpostorFades`], found by world position. An object whose cell has
//! just loaded fades in from its impostor, which drew while it was missing.
use super::{ForcedLod, LodProjection};
use crate::tree_impostor::ImpostorFades;
use bevy::{camera::visibility::VisibilityRange, math::DVec2};

pub(crate) const FADE_SECONDS: f32 = 0.6;
/// A representation keeps drawing until the camera is this share past its band.
const HYSTERESIS: f32 = 0.04;
/// Mesh tags carry 64 + the dither level; 0 means no level (distance crossfades).
pub(super) const TAG_BIAS: i32 = 64;

/// Always drawn as far as visibility ranges go, but with a crossfade margin, so Bevy's main
/// and prepass pipelines include the dither the mesh tag drives. It never changes, so it
/// never makes Bevy rebuild its range table.
pub(super) const TIMED_RANGE: VisibilityRange = VisibilityRange {
    start_margin: 0.0..1e-3,
    end_margin: f32::MAX / 2.0..f32::MAX,
    use_aabb: false,
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade {
    from: usize,
    to: usize,
    progress: f32,
}

/// One band per representation that has one, by variant index: where the camera's
/// distance to the root selects it, between the midpoints of the crossfades the distance
/// LOD would use.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Band {
    index: usize,
    start: f32,
    end: f32,
}

#[derive(Debug, Default)]
pub(crate) struct TimedLod {
    shown: Option<usize>,
    fade: Option<Fade>,
    bands: Vec<Band>,
    /// The impostor instance's slot, and the [`ImpostorFades`] generation it was looked up in.
    slot: Option<u32>,
    searched: Option<u64>,
    /// Levels last written, by variant index.
    applied: Option<Vec<Option<i32>>>,
}

impl TimedLod {
    pub(crate) fn fading(&self) -> bool {
        self.fade.is_some()
    }

    /// Recomputes the bands, after the projection or the hand-off changed.
    pub(super) fn set_bands(
        &mut self,
        projection: &LodProjection,
        thresholds: &[f32],
        height: f32,
        farthest: f32,
    ) {
        self.bands = (0..thresholds.len())
            .filter_map(|index| {
                let range = projection.range(thresholds, height, index, farthest);
                (range.end_margin.end > 0.0).then_some(Band {
                    index,
                    start: (range.start_margin.start + range.start_margin.end) * 0.5,
                    end: if range.end_margin.end >= f32::MAX {
                        f32::INFINITY
                    } else {
                        (range.end_margin.start + range.end_margin.end) * 0.5
                    },
                })
            })
            .collect();
    }

    pub(super) fn has_bands(&self) -> bool {
        !self.bands.is_empty()
    }

    /// The representation the camera's distance asks for, keeping the current one until
    /// the distance is clearly past its band.
    fn target(&self, distance: f32) -> Option<usize> {
        let containing = self
            .bands
            .iter()
            .find(|b| distance >= b.start && distance < b.end)
            .map(|b| b.index);
        let current = self.fade.map(|f| f.to).or(self.shown);
        match current.and_then(|c| self.bands.iter().find(|b| b.index == c)) {
            Some(band)
                if distance <= band.end * (1.0 + HYSTERESIS)
                    && distance >= band.start * (1.0 - HYSTERESIS) =>
            {
                current
            }
            _ => containing,
        }
    }

    /// Advances by `dt` seconds towards the representation `forced` or the distance picks.
    pub(super) fn step(&mut self, forced: ForcedLod, distance: f32, impostor: usize, dt: f32) {
        let target = match forced {
            ForcedLod::Auto => self.target(distance),
            ForcedLod::Only(index) => {
                self.shown = Some(index);
                self.fade = None;
                return;
            }
            ForcedLod::Nothing => {
                self.shown = None;
                self.fade = None;
                return;
            }
        };
        match self.fade {
            Some(mut fade) => {
                if target == Some(fade.from) {
                    // The camera turned back: dissolve the other way from where it got to.
                    fade = Fade {
                        from: fade.to,
                        to: fade.from,
                        progress: 1.0 - fade.progress,
                    };
                }
                fade.progress += dt / FADE_SECONDS;
                if fade.progress >= 1.0 {
                    self.shown = Some(fade.to);
                    self.fade = None;
                } else {
                    self.fade = Some(fade);
                }
            }
            None if target != self.shown => {
                // A newly resident object's impostor drew until now: dissolve from it.
                let from = self.shown.or_else(|| {
                    (self.slot.is_some() && self.applied.is_none()).then_some(impostor)
                });
                match (from, target) {
                    (Some(from), Some(to)) if from != to => {
                        self.fade = Some(Fade {
                            from,
                            to,
                            progress: 0.0,
                        });
                    }
                    _ => self.shown = target,
                }
            }
            None => {}
        }
    }

    /// Bevy's dither level for variant `index`: from 0 to 16 while fading out, from -16 to
    /// 0 while fading in, 0 while shown, `None` while hidden.
    pub(super) fn level(&self, index: usize) -> Option<i32> {
        match self.fade {
            Some(f) => {
                let step = (f.progress.clamp(0.0, 1.0) * 16.0).round() as i32;
                if index == f.from {
                    Some(step)
                } else if index == f.to {
                    Some(step - 16)
                } else {
                    None
                }
            }
            None => (self.shown == Some(index)).then_some(0),
        }
    }

    /// Levels of every variant, if they differ from those last written.
    pub(super) fn changed_levels(&mut self, variants: usize) -> Option<Vec<Option<i32>>> {
        let levels: Vec<_> = (0..variants).map(|i| self.level(i)).collect();
        if self.applied.as_ref() == Some(&levels) {
            return None;
        }
        self.applied = Some(levels.clone());
        Some(levels)
    }

    /// Finds the impostor instance's slot once the drawn batches change.
    pub(super) fn link(&mut self, fades: &ImpostorFades, world_xz: Option<DVec2>) {
        if self.searched == Some(fades.generation()) {
            return;
        }
        let Some(world_xz) = world_xz else {
            return;
        };
        self.searched = Some(fades.generation());
        let slot = fades.find(world_xz);
        if slot != self.slot {
            self.slot = slot;
            // Rewrite every level, including the new slot's.
            self.applied = self.applied.take().map(|mut levels| {
                levels.iter_mut().for_each(|l| *l = Some(i32::MIN));
                levels
            });
        }
    }

    pub(super) fn slot(&self) -> Option<u32> {
        self.slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lod() -> TimedLod {
        TimedLod {
            bands: vec![
                Band {
                    index: 0,
                    start: 0.0,
                    end: 40.0,
                },
                Band {
                    index: 1,
                    start: 40.0,
                    end: 90.0,
                },
                Band {
                    index: 3,
                    start: 90.0,
                    end: f32::INFINITY,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_switch_dissolves_over_the_fade_time_whatever_the_speed() {
        let quarter = FADE_SECONDS / 4.0;
        let mut lod = lod();
        lod.step(ForcedLod::Auto, 30.0, 3, 0.016);
        assert_eq!(lod.shown, Some(0));
        assert_eq!(lod.level(0), Some(0));
        // Past the band and its hysteresis: LOD0 fades out, LOD1 in, from the next frame.
        lod.step(ForcedLod::Auto, 43.0, 3, quarter);
        let fade = lod.fade.unwrap();
        assert_eq!((fade.from, fade.to), (0, 1));
        assert_eq!((lod.level(0), lod.level(1)), (Some(0), Some(-16)));
        lod.step(ForcedLod::Auto, 43.0, 3, quarter);
        assert_eq!(lod.level(0), Some(4));
        assert_eq!(lod.level(1), Some(-12));
        assert_eq!(lod.level(3), None);
        for _ in 0..3 {
            lod.step(ForcedLod::Auto, 43.0, 3, quarter);
        }
        assert_eq!(lod.shown, Some(1));
        assert!(!lod.fading());
        assert_eq!(lod.level(0), None);
    }

    #[test]
    fn hysteresis_keeps_a_representation_near_its_boundary() {
        let mut lod = lod();
        lod.step(ForcedLod::Auto, 39.0, 3, 0.016);
        lod.step(ForcedLod::Auto, 41.0, 3, 0.016);
        assert_eq!((lod.shown, lod.fading()), (Some(0), false));
    }

    #[test]
    fn turning_back_reverses_the_dissolve_from_where_it_got() {
        let mut lod = lod();
        lod.step(ForcedLod::Auto, 30.0, 3, 0.016);
        for _ in 0..3 {
            lod.step(ForcedLod::Auto, 95.0, 3, FADE_SECONDS / 4.0);
        }
        lod.step(ForcedLod::Auto, 30.0, 3, 0.0);
        let fade = lod.fade.unwrap();
        assert_eq!((fade.from, fade.to), (3, 0));
        assert!((fade.progress - 0.5).abs() < 1e-5);
    }

    #[test]
    fn a_newly_resident_object_fades_in_from_its_impostor() {
        let mut lod = lod();
        lod.slot = Some(7);
        lod.step(ForcedLod::Auto, 60.0, 3, 0.016);
        let fade = lod.fade.unwrap();
        assert_eq!((fade.from, fade.to), (3, 1));
    }

    #[test]
    fn forcing_shows_one_representation_at_once() {
        let mut lod = lod();
        lod.step(ForcedLod::Only(3), 10.0, 3, 0.016);
        assert_eq!((lod.level(3), lod.level(0)), (Some(0), None));
        lod.step(ForcedLod::Nothing, 10.0, 3, 0.016);
        assert_eq!((lod.level(3), lod.level(0)), (None, None));
    }
}
