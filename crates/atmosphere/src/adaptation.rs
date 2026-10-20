//! Eye adaptation in the game: Bevy's auto exposure follows part of a change in the scene's
//! brightness, around the profile's exposure, so forest shade stays readable and open ground in
//! sun calms down while the forest still reads darker than the beach.
//!
//! Bevy meters the average log2 luminance of the exposed image (the 10-90% band of its
//! histogram) and corrects the exposure by `curve(average) - average` stops. The curve here
//! makes that correction [`FOLLOW`] times the distance from [`REFERENCE`], clamped. Measured at
//! EV 13 with no tonemapping, the start view's beach meters -2.4, the forest edge -3.2, the
//! pine forest -3.9, the spruce forest -5.5 and the summit, mostly sky, -1.3. The sky counts
//! for less than the land ([`metering_mask`]).
use bevy::{
    asset::RenderAssetUsages,
    curve::cubic_splines::LinearSpline,
    post_process::auto_exposure::{
        AutoExposure, AutoExposureCompensationCurve, AutoExposurePlugin,
    },
    prelude::*,
    render::{
        RenderApp,
        render_resource::{Extent3d, TextureDimension, TextureFormat},
    },
};

/// Share of a change in metered brightness the exposure follows; the rest stays visible.
pub(crate) const FOLLOW: f32 = 0.5;

/// Metered log2 exposed luminance left as authored, a stop below a forest edge in sun at EV 13
/// (-3.25), so daylight shows half a stop darker than it meters: the game read too bright in
/// sun and under cloud. These constants are binary fractions, so the curve's segments meet
/// exactly, as Bevy requires.
pub(crate) const REFERENCE: f32 = -4.25;

/// The most the exposure brightens (deep shade) and darkens (bright open ground), in stops.
pub(crate) const BRIGHTEN: f32 = 1.5;

pub(crate) const DARKEN: f32 = 1.0;

/// The most it brightens at full night. A moonless night meters far below [`REFERENCE`] (dark
/// grass, a near-black sky; snow is in the top tenth Bevy leaves out), so the full day value
/// lifted snow to lavender white; a moonlit night meters brighter and kept snow grey.
pub(crate) const NIGHT_BRIGHTEN: f32 = 0.5;

/// Curves between [`BRIGHTEN`] and [`NIGHT_BRIGHTEN`], a quarter stop apart, picked by the
/// night's adaptation; Bevy's adaptation speed hides each step.
const NIGHT_STEPS: usize = 5;

/// Stops per second towards a brighter scene, which eyes adapt to quickly, and towards a
/// darker one, which takes longer.
const TO_BRIGHT: f32 = 2.0;

const TO_DARK: f32 = 0.75;

/// Metering weight of the top of the frame (mostly sky), rising to 1 at [`LAND_FROM`] of its
/// height.
const SKY_WEIGHT: f32 = 0.3;

const LAND_FROM: f32 = 0.4;

#[derive(Resource)]
pub(crate) struct Adaptation {
    /// Day to night, see [`NIGHT_STEPS`].
    curves: Vec<Handle<AutoExposureCompensationCurve>>,
    /// No correction. Bevy keeps adapting a view whose `AutoExposure` was removed, so turning
    /// adaptation off swaps the curve instead.
    identity: Handle<AutoExposureCompensationCurve>,
    mask: Handle<Image>,
}

impl Adaptation {
    /// `night` is the atmosphere's adaptation, 0 by day and 1 at night.
    pub(crate) fn settings(&self, adapt: bool, night: f32) -> AutoExposure {
        AutoExposure {
            speed_brighten: TO_BRIGHT,
            speed_darken: TO_DARK,
            metering_mask: self.mask.clone(),
            compensation_curve: self.curve(adapt, night).clone(),
            ..default()
        }
    }

    pub(crate) fn curve(&self, adapt: bool, night: f32) -> &Handle<AutoExposureCompensationCurve> {
        if !adapt {
            return &self.identity;
        }

        let step = (night.clamp(0.0, 1.0) * (NIGHT_STEPS - 1) as f32).round() as usize;
        &self.curves[step]
    }
}

pub(crate) fn plugin(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    app.add_plugins(AutoExposurePlugin)
        .add_systems(Startup, setup);
}

fn setup(
    mut commands: Commands,
    mut curves: ResMut<Assets<AutoExposureCompensationCurve>>,
    mut images: ResMut<Assets<Image>>,
) {
    let identity = [Vec2::splat(-8.0), Vec2::splat(8.0)];
    let night: Result<Vec<_>, _> = (0..NIGHT_STEPS)
        .map(|step| {
            AutoExposureCompensationCurve::from_curve(LinearSpline::new(compensation_points(
                night_brighten(step),
            )))
        })
        .collect();
    let (Ok(night), Ok(identity)) = (
        night,
        AutoExposureCompensationCurve::from_curve(LinearSpline::new(identity)),
    ) else {
        error!("auto exposure: invalid compensation curve");
        return;
    };
    commands.insert_resource(Adaptation {
        curves: night.into_iter().map(|curve| curves.add(curve)).collect(),
        identity: curves.add(identity),
        mask: images.add(metering_mask()),
    });
}

/// The brightening limit of night step `step`: [`BRIGHTEN`] at 0, [`NIGHT_BRIGHTEN`] at the last.
fn night_brighten(step: usize) -> f32 {
    BRIGHTEN + (NIGHT_BRIGHTEN - BRIGHTEN) * step as f32 / (NIGHT_STEPS - 1) as f32
}

/// Bevy's compensation curve over its default metering range of -8..8: `average + correction`
/// where the correction is `FOLLOW * (REFERENCE - average)` within `-DARKEN..brighten`.
pub(crate) fn compensation_points(brighten: f32) -> [Vec2; 4] {
    let brightest_kept = REFERENCE + DARKEN / FOLLOW;
    let darkest_kept = REFERENCE - brighten / FOLLOW;
    [
        Vec2::new(-8.0, -8.0 + brighten),
        Vec2::new(darkest_kept, darkest_kept + brighten),
        Vec2::new(brightest_kept, brightest_kept - DARKEN),
        Vec2::new(8.0, 8.0 - DARKEN),
    ]
}

/// One column, top to bottom: the sky band weighs [`SKY_WEIGHT`], the land below fully.
fn metering_mask() -> Image {
    const ROWS: u32 = 64;
    let data = (0..ROWS)
        .map(|row| {
            let t = (row as f32 + 0.5) / ROWS as f32 / LAND_FROM;
            let weight = SKY_WEIGHT + (1.0 - SKY_WEIGHT) * t.clamp(0.0, 1.0);
            (weight * 255.0).round() as u8
        })
        .collect();
    Image::new(
        Extent3d {
            width: 1,
            height: ROWS,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The correction the curve asks for at a metered average.
    fn correction(average: f32) -> f32 {
        correction_with(BRIGHTEN, average)
    }

    fn correction_with(brighten: f32, average: f32) -> f32 {
        let points = compensation_points(brighten);
        let i = points
            .windows(2)
            .position(|w| average <= w[1].x)
            .unwrap_or(points.len() - 2);
        let (a, b) = (points[i], points[i + 1]);
        a.y + (b.y - a.y) * (average - a.x) / (b.x - a.x) - average
    }

    #[test]
    fn exposure_follows_half_the_change_within_its_limits() {
        assert!(correction(REFERENCE).abs() < 1e-5);

        // The beach a stop brighter than the forest edge darkens by half a stop.
        assert!((correction(REFERENCE + 1.0) + 0.5).abs() < 1e-5);
        assert!((correction(REFERENCE - 2.0) - 1.0).abs() < 1e-5);
        assert!((correction(REFERENCE - 6.0) - BRIGHTEN).abs() < 1e-5);
        assert!((correction(REFERENCE + 5.0) + DARKEN).abs() < 1e-5);
        let points = compensation_points(BRIGHTEN);
        assert!(points.windows(2).all(|w| w[1].x > w[0].x));
        assert!(AutoExposureCompensationCurve::from_curve(LinearSpline::new(points)).is_ok());
    }

    #[test]
    fn night_brightens_less_and_daylight_is_unchanged() {
        assert_eq!(night_brighten(0), BRIGHTEN);
        assert_eq!(night_brighten(NIGHT_STEPS - 1), NIGHT_BRIGHTEN);

        for step in 0..NIGHT_STEPS {
            let brighten = night_brighten(step);

            // Quarter stops: binary fractions, so the curve's segments meet exactly.
            assert_eq!((brighten * 4.0).fract(), 0.0);
            let points = compensation_points(brighten);
            assert!(points.windows(2).all(|w| w[1].x > w[0].x));
            assert!(AutoExposureCompensationCurve::from_curve(LinearSpline::new(points)).is_ok());

            // Bright ground darkens alike day and night; only the lift of dark scenes shrinks.
            assert!((correction_with(brighten, REFERENCE + 5.0) + DARKEN).abs() < 1e-5);
            assert!((correction_with(brighten, -8.0) - brighten).abs() < 1e-5);
        }
    }

    #[test]
    fn the_sky_counts_for_less_than_the_land() {
        let mask = metering_mask();
        let data = mask.data.unwrap();
        assert!((70..90).contains(&data[0]));
        assert_eq!(*data.last().unwrap(), 255);
        assert!(data.windows(2).all(|w| w[1] >= w[0]));
    }
}
