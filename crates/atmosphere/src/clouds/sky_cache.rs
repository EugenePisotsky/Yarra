//! Bounded refresh of a world-direction cloud atlas, independent of camera turns.
use bevy::prelude::*;
use std::time::Duration;

pub const SIZE: u32 = 512;
const STRIPES: u32 = 4;
// At most eight complete atlas refreshes/second, spread over separate frames.
const INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / (8 * STRIPES as u64));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Full,
    Stripe(u32),
}
impl Region {
    pub fn rows(self, height: u32) -> (u32, u32) {
        match self {
            Self::Full => (0, height),
            Self::Stripe(i) => (i * height / STRIPES, height / STRIPES),
        }
    }
}

#[derive(Default)]
pub struct Refresh {
    last_update: Option<Duration>,
    previous_camera: Option<Vec3>,
    stripe: u32,
}
impl Refresh {
    pub fn next(
        &mut self,
        now: Duration,
        camera: Vec3,
        period: f32,
        invalidated: bool,
    ) -> Option<Region> {
        // Canonical X/Z wrap at the cloud field period. A floating-origin rebase
        // or crossing that seam is not a camera cut.
        let cut = self.previous_camera.is_some_and(|previous| {
            let mut delta = camera - previous;
            if period > 0. {
                delta.x = (delta.x + period * 0.5).rem_euclid(period) - period * 0.5;
                delta.z = (delta.z + period * 0.5).rem_euclid(period) - period * 0.5;
            }
            delta.length_squared() > 50. * 50.
        });
        self.previous_camera = Some(camera);
        let stale = self
            .last_update
            .is_none_or(|last| now.saturating_sub(last) > Duration::from_millis(500));
        if invalidated || cut || stale {
            self.last_update = Some(now);
            self.stripe = 0;
            return Some(Region::Full);
        }
        if now.saturating_sub(self.last_update.unwrap()) < INTERVAL {
            return None;
        }
        // No catch-up loop after a slow frame: never submit several stale updates.
        self.last_update = Some(now);
        let region = Region::Stripe(self.stripe);
        self.stripe = (self.stripe + 1) % STRIPES;
        Some(region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_budget_is_bounded_independently_of_frame_rate() {
        for fps in [30, 60, 120, 240] {
            let mut refresh = Refresh::default();
            let mut stripes = 0;
            assert_eq!(
                refresh.next(Duration::ZERO, Vec3::ZERO, 20000., false),
                Some(Region::Full)
            );
            for frame in 1..=fps * 10 {
                let now = Duration::from_secs_f64(f64::from(frame) / f64::from(fps));
                if let Some(region) = refresh.next(now, Vec3::ZERO, 20000., false) {
                    assert_eq!(region, Region::Stripe(stripes % STRIPES));
                    stripes += 1;
                }
            }
            assert!((280..=320).contains(&stripes), "{fps} fps: {stripes}");
        }
    }

    #[test]
    fn teleports_edits_and_resume_refresh_all_but_period_seams_do_not() {
        let mut refresh = Refresh::default();
        let mut camera = Vec3::new(19999., 10., 0.);
        refresh.next(Duration::ZERO, camera, 20000., false);
        camera.x = 1.;
        assert_eq!(
            refresh.next(Duration::from_millis(10), camera, 20000., false),
            None
        );
        camera.y += 100.;
        assert_eq!(
            refresh.next(Duration::from_millis(20), camera, 20000., false),
            Some(Region::Full)
        );
        assert_eq!(
            refresh.next(Duration::from_millis(21), camera, 20000., true),
            Some(Region::Full)
        );
        assert_eq!(
            refresh.next(Duration::from_secs(2), camera, 20000., false),
            Some(Region::Full)
        );
        assert_eq!(
            refresh.next(Duration::from_millis(2100), camera, 20000., false),
            Some(Region::Stripe(0))
        );
        // A delayed frame performs one stripe, without catching up several passes.
        assert_eq!(
            refresh.next(Duration::from_millis(2400), camera, 20000., false),
            Some(Region::Stripe(1))
        );
    }
}
