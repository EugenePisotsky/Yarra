//! Bounded refresh of a world-direction cloud atlas, independent of camera turns.
use bevy::prelude::*;
use std::time::Duration;

/// Azimuth × elevation panorama. Its rows follow the horizon: the former 512² stereographic
/// disk's square grid crossed thin distant clouds diagonally, and bilinear filtering rendered
/// them as sawtooth staircases. Twice that texel count at half the refresh rate traces the same
/// rays per second with ~5.7 texels per degree of azimuth (the disk had ~4.2).
pub const WIDTH: u32 = 2048;
pub const HEIGHT: u32 = 256;
/// Complete atlas refreshes per second, spread evenly over frames. The per-frame share
/// varies with frame rate; total work per second does not.
const REFRESH_HZ: f64 = 4.0;
/// After a hitch, refresh at most this many rows instead of catching up: 65,536 rays, as
/// one stripe of the former disk.
const MAX_ROWS: u32 = HEIGHT / 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Full,
    Rows { start: u32, count: u32 },
}
impl Region {
    /// Row ranges as (first, count). A refresh crossing the last row wraps to the top.
    pub fn ranges(self, height: u32) -> impl Iterator<Item = (u32, u32)> {
        let (start, count) = match self {
            Self::Full => (0, height),
            Self::Rows { start, count } => (start.min(height), count.min(height)),
        };
        let first = count.min(height - start);
        [(start, first), (0, count - first)]
            .into_iter()
            .filter(|(_, rows)| *rows > 0)
    }
}

#[derive(Default)]
pub struct Refresh {
    last_update: Option<Duration>,
    previous_camera: Option<Vec3>,
    cursor: u32,
    due: f64,
}
impl Refresh {
    /// Share of the current sweep already refreshed, for cross-fading complete images.
    pub fn progress(&self) -> f32 {
        self.cursor as f32 / HEIGHT as f32
    }

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
        let elapsed = self.last_update.map(|last| now.saturating_sub(last));
        self.last_update = Some(now);
        if invalidated || cut || elapsed.is_none_or(|e| e > Duration::from_millis(500)) {
            self.cursor = 0;
            self.due = 0.;
            return Some(Region::Full);
        }
        self.due += elapsed.unwrap().as_secs_f64() * REFRESH_HZ * f64::from(HEIGHT);
        let wanted = self.due.floor() as u32;
        let rows = wanted.min(MAX_ROWS);
        if rows == 0 {
            return None;
        }
        // No catch-up after a slow frame: drop the backlog instead of spreading a burst.
        self.due = if wanted > MAX_ROWS {
            self.due.fract()
        } else {
            self.due - f64::from(rows)
        };
        let start = self.cursor;
        self.cursor = (self.cursor + rows) % HEIGHT;
        Some(Region::Rows { start, count: rows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_work_is_even_per_frame_and_bounded_per_second() {
        for fps in [30, 60, 120, 240] {
            let mut refresh = Refresh::default();
            assert_eq!(
                refresh.next(Duration::ZERO, Vec3::ZERO, 20000., false),
                Some(Region::Full)
            );
            let mut rows = 0;
            let mut largest = 0;
            let mut expected_start = 0;
            for frame in 1..=fps * 10 {
                let now = Duration::from_secs_f64(f64::from(frame) / f64::from(fps));
                match refresh.next(now, Vec3::ZERO, 20000., false) {
                    Some(Region::Rows { start, count }) => {
                        assert_eq!(start, expected_start, "{fps} fps: rows are contiguous");
                        expected_start = (start + count) % HEIGHT;
                        rows += count;
                        largest = largest.max(count);
                    }
                    other => panic!("{fps} fps: unexpected {other:?}"),
                }
            }
            // Complete refreshes at a fixed rate, without stripe spikes. Very low frame
            // rates hit the per-frame bound first.
            let per_second = REFRESH_HZ as u32 * HEIGHT;
            let expected = (per_second * 10).min(MAX_ROWS * fps * 10);
            assert!(rows.abs_diff(expected) <= 200, "{fps} fps: {rows} rows");
            let even = per_second.div_ceil(fps) + 1;
            assert!(largest <= even, "{fps} fps: {largest} rows in one frame");
        }
    }

    #[test]
    fn wrapped_rows_split_into_two_ranges() {
        let ranges = |region: Region| region.ranges(HEIGHT).collect::<Vec<_>>();
        assert_eq!(ranges(Region::Full), [(0, HEIGHT)]);
        assert_eq!(
            ranges(Region::Rows {
                start: 10,
                count: 20
            }),
            [(10, 20)]
        );
        assert_eq!(
            ranges(Region::Rows {
                start: HEIGHT - 5,
                count: 20
            }),
            [(HEIGHT - 5, 5), (0, 15)]
        );
    }

    #[test]
    fn teleports_edits_and_resume_refresh_all_but_period_seams_and_hitches_do_not() {
        let mut refresh = Refresh::default();
        let mut camera = Vec3::new(19999., 10., 0.);
        refresh.next(Duration::ZERO, camera, 20000., false);
        camera.x = 1.;
        assert!(matches!(
            refresh.next(Duration::from_millis(10), camera, 20000., false),
            Some(Region::Rows { start: 0, .. })
        ));
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
        // A 300 ms hitch refreshes one bounded block, without catching up afterwards.
        assert_eq!(
            refresh.next(Duration::from_millis(2300), camera, 20000., false),
            Some(Region::Rows {
                start: 0,
                count: MAX_ROWS
            })
        );
        let Some(Region::Rows { start, count }) =
            refresh.next(Duration::from_millis(2310), camera, 20000., false)
        else {
            panic!("expected rows");
        };
        assert_eq!(start, MAX_ROWS);
        assert!(count < MAX_ROWS / 2, "backlog was not dropped: {count}");
    }
}
