//! Renderer-independent weather: presets, a weighted random sequence and continuous blending.
//! The authored atmosphere profile stays the baseline; weather overlays clouds, haze, exposure
//! and wind. No ECS or wall clock: callers advance the runtime with elapsed seconds.
use crate::atmosphere::AtmosphereProfile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WeatherKind {
    Clear,
    Scattered,
    Overcast,
    Rain,
    Storm,
}
impl WeatherKind {
    pub const ALL: [Self; 5] = [
        Self::Clear,
        Self::Scattered,
        Self::Overcast,
        Self::Rain,
        Self::Storm,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Clear => "Clear",
            Self::Scattered => "Scattered",
            Self::Overcast => "Overcast",
            Self::Rain => "Rain",
            Self::Storm => "Storm",
        }
    }
    pub fn index(self) -> usize {
        self as usize
    }
    /// Built-in default for this state; worlds author their own in `WeatherSettings`.
    pub fn preset(self) -> WeatherParams {
        let p = |cloud_coverage,
                 cloud_density,
                 cloud_thickness_scale,
                 cloud_erosion,
                 visibility_scale,
                 haze_grey,
                 exposure_offset_ev,
                 [wind_strength, wind_gustiness, wind_rate]: [f32; 3],
                 precipitation| WeatherParams {
            cloud_coverage,
            cloud_density,
            cloud_thickness_scale,
            cloud_erosion,
            visibility_scale,
            haze_grey,
            exposure_offset_ev,
            wind_strength,
            wind_gustiness,
            wind_rate,
            precipitation,
        };
        // Scattered matches the default authored cloud layer and unscaled wind, so the
        // existing look is one point of the sequence. Negative EV offsets approximate the
        // exposure a photographer would open under cloud; there is no automatic exposure.
        // Keep them small: Bevy's ambient term is already a large share of clear-day light, so
        // -2 EV made rain-lit PBR surfaces brighter than sunlit ones.
        match self {
            Self::Clear => p(0.18, 0.6, 1.0, 0.35, 1.0, 0.0, 0.0, [0.7, 0.7, 0.85], 0.0),
            Self::Scattered => p(0.48, 0.8, 1.0, 0.3, 1.0, 0.0, 0.0, [1.0, 1.0, 1.0], 0.0),
            Self::Overcast => p(0.92, 1.2, 0.7, 0.15, 0.55, 0.5, -0.8, [1.2, 1.0, 1.1], 0.0),
            Self::Rain => p(
                0.96,
                1.6,
                0.85,
                0.1,
                0.2,
                0.75,
                -1.0,
                [1.45, 1.0, 1.25],
                0.6,
            ),
            Self::Storm => p(1.0, 2.2, 1.1, 0.05, 0.08, 0.9, -0.9, [1.9, 1.0, 1.5], 1.0),
        }
    }
    /// The preset closest to an authored cloud layer, used to start the sequence without a jump.
    pub fn nearest_to(profile: &AtmosphereProfile) -> Self {
        if !profile.clouds.enabled {
            return Self::Clear;
        }
        let coverage = profile.clouds.coverage;
        Self::ALL
            .into_iter()
            .filter(|k| profile.weather.preset(*k).precipitation == 0.0)
            .min_by(|a, b| {
                let da = (profile.weather.preset(*a).cloud_coverage - coverage).abs();
                let db = (profile.weather.preset(*b).cloud_coverage - coverage).abs();
                da.total_cmp(&db)
            })
            .unwrap_or(Self::Clear)
    }
}

/// Continuous weather values. Scales are relative to the authored profile; the rest are absolute.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WeatherParams {
    pub cloud_coverage: f32,
    pub cloud_density: f32,
    pub cloud_thickness_scale: f32,
    pub cloud_erosion: f32,
    /// Visibility relative to the authored clear-weather visibility; see [`Self::fog`].
    pub visibility_scale: f32,
    /// 0 keeps the authored fog tint and skylight colour; 1 is neutral grey under a cloud deck.
    pub haze_grey: f32,
    /// Added to the authored daytime EV100. Negative brightens.
    pub exposure_offset_ev: f32,
    /// Multipliers of the baseline vegetation wind amplitude, gust layer and wave rate.
    pub wind_strength: f32,
    pub wind_gustiness: f32,
    pub wind_rate: f32,
    /// 0..1 precipitation intensity.
    pub precipitation: f32,
}

const NEUTRAL_HAZE_SRGB: [f32; 3] = [0.78, 0.80, 0.82];
/// Keep one frame's wave advance below the grass history-reset threshold at the 0.1 s hitch clamp.
pub const MAX_WIND_RATE: f32 = 2.0;

impl WeatherParams {
    pub fn lerp(self, to: Self, t: f32) -> Self {
        let m = |a: f32, b: f32| a + (b - a) * t;
        Self {
            cloud_coverage: m(self.cloud_coverage, to.cloud_coverage),
            cloud_density: m(self.cloud_density, to.cloud_density),
            cloud_thickness_scale: m(self.cloud_thickness_scale, to.cloud_thickness_scale),
            cloud_erosion: m(self.cloud_erosion, to.cloud_erosion),
            visibility_scale: m(self.visibility_scale, to.visibility_scale),
            haze_grey: m(self.haze_grey, to.haze_grey),
            exposure_offset_ev: m(self.exposure_offset_ev, to.exposure_offset_ev),
            wind_strength: m(self.wind_strength, to.wind_strength),
            wind_gustiness: m(self.wind_gustiness, to.wind_gustiness),
            wind_rate: m(self.wind_rate, to.wind_rate),
            precipitation: m(self.precipitation, to.precipitation),
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let valid = [
            (self.cloud_coverage, 0., 1.),
            (self.cloud_density, 0., 3.),
            (self.cloud_thickness_scale, 0.1, 4.),
            (self.cloud_erosion, 0., 1.),
            (self.visibility_scale, 0.01, 1.),
            (self.haze_grey, 0., 1.),
            (self.exposure_offset_ev, -6., 6.),
            (self.wind_strength, 0., 3.),
            (self.wind_gustiness, 0., 3.),
            (self.wind_rate, 0., MAX_WIND_RATE),
            (self.precipitation, 0., 1.),
        ]
        .iter()
        .all(|&(v, lo, hi)| v.is_finite() && (lo..=hi).contains(&v));
        if valid {
            Ok(())
        } else {
            Err("Weather values are outside their physical range")
        }
    }

    /// The authored profile with this weather's clouds and exposure. Cloud altitude, size, seed
    /// and drift, sun path, palettes and the clear-air haze stay authored. The result validates
    /// whenever the input does.
    pub fn apply(&self, profile: &AtmosphereProfile) -> AtmosphereProfile {
        let mut p = profile.clone();
        p.clouds.enabled = true;
        p.clouds.coverage = self.cloud_coverage.clamp(0., 1.);
        p.clouds.density = self.cloud_density.clamp(0., 3.);
        p.clouds.erosion = self.cloud_erosion.clamp(0., 1.);
        p.clouds.thickness_metres =
            (profile.clouds.thickness_metres * self.cloud_thickness_scale).clamp(100., 3000.);
        // Night keeps its authored exposure: the moon is already hidden by the same clouds.
        p.exposure_ev100 = (profile.exposure_ev100 + self.exposure_offset_ev).clamp(0., 20.);
        // Skylight under a cloud deck is grey. Blue clear-sky ambient on pale surfaces reads as
        // fog even at arm's length once the sun is gone.
        let grey = self.haze_grey.clamp(0., 1.) * 0.7;
        for phase in &mut p.phases {
            let mean = phase.ambient_srgb.iter().sum::<f32>() / 3.;
            for c in &mut phase.ambient_srgb {
                *c += (mean - *c) * grey;
            }
        }
        p
    }

    /// Fog added in front of the scene for reduced visibility. The authored visibility stays in
    /// the physical sky model, whose lookup tables divide transmittances that underflow once
    /// visibility drops to a few kilometres; weather supplies only the extra extinction.
    pub fn fog(&self, profile: &AtmosphereProfile) -> WeatherFog {
        let authored = profile.visibility_metres.clamp(50., 100_000.);
        let visibility = (authored * self.visibility_scale.clamp(0.01, 1.)).max(50.);
        let grey = self.haze_grey.clamp(0., 1.);
        let haze = crate::atmosphere::linear_rgb(profile.haze_srgb);
        let neutral = crate::atmosphere::linear_rgb(NEUTRAL_HAZE_SRGB);
        WeatherFog {
            visibility_metres: visibility,
            extinction: (VISIBILITY_EXTINCTION / visibility - VISIBILITY_EXTINCTION / authored)
                .max(0.),
            tint_linear: std::array::from_fn(|i| haze[i] + (neutral[i] - haze[i]) * grey),
        }
    }
}

/// Both ends of the current change with its linear progress. The cloud field changes region by
/// region within it, so renderers need more than the blended values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherTransition {
    pub from: WeatherParams,
    pub to: WeatherParams,
    /// 0..1, 1 once settled.
    pub progress: f32,
}

/// Extinction per metre at which 2% contrast remains after one visibility distance.
const VISIBILITY_EXTINCTION: f32 = 3.912;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherFog {
    /// Total visibility, including the authored clear-air haze.
    pub visibility_metres: f32,
    /// Extra extinction per metre beyond the authored haze.
    pub extinction: f32,
    pub tint_linear: [f32; 3],
}

/// Hard-coded default sequence until the editor authors one. Indexed by `WeatherKind::index`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherSchedule {
    /// Seconds a settled state is held before the next one is chosen, as [min, max].
    pub hold_seconds: [[f32; 2]; 5],
    /// Relative weight of each next state, indexed [current][next]. Zero forbids that change.
    pub next_weights: [[f32; 5]; 5],
    /// Duration of an automatic change, as [min, max].
    pub transition_seconds: [f32; 2],
}
impl Default for WeatherSchedule {
    fn default() -> Self {
        Self {
            hold_seconds: [
                [240., 600.],
                [180., 480.],
                [120., 360.],
                [120., 300.],
                [60., 180.],
            ],
            // Weather moves through neighbouring states: no clear sky straight into a storm.
            next_weights: [
                [0.0, 0.7, 0.3, 0.0, 0.0],
                [0.35, 0.0, 0.45, 0.2, 0.0],
                [0.15, 0.4, 0.0, 0.45, 0.0],
                [0.0, 0.25, 0.5, 0.0, 0.25],
                [0.0, 0.0, 0.2, 0.8, 0.0],
            ],
            transition_seconds: [40., 90.],
        }
    }
}
impl WeatherSchedule {
    pub fn validate(&self) -> Result<(), &'static str> {
        let range = |[lo, hi]: [f32; 2]| lo.is_finite() && hi.is_finite() && 0. <= lo && lo <= hi;
        let holds = self.hold_seconds.iter().all(|&h| range(h) && h[1] > 0.);
        let weights = self.next_weights.iter().all(|row| {
            row.iter().all(|w| w.is_finite() && *w >= 0.) && row.iter().sum::<f32>() > 0.
        });
        if holds && weights && range(self.transition_seconds) {
            Ok(())
        } else {
            Err(
                "Weather schedule needs ordered non-negative durations and a positive weight per state",
            )
        }
    }
}

/// Authored weather for one world: the preset of each state and the random sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherSettings {
    /// Indexed by `WeatherKind::index`.
    pub presets: [WeatherParams; 5],
    pub schedule: WeatherSchedule,
}
impl Default for WeatherSettings {
    fn default() -> Self {
        Self {
            presets: WeatherKind::ALL.map(WeatherKind::preset),
            schedule: WeatherSchedule::default(),
        }
    }
}
impl WeatherSettings {
    pub fn preset(&self, kind: WeatherKind) -> WeatherParams {
        self.presets[kind.index()]
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        for preset in &self.presets {
            preset.validate()?;
        }
        self.schedule.validate()
    }
}

/// Small deterministic generator; weather needs variety, not statistical quality.
#[derive(Debug, Clone)]
struct SplitMix64(u64);
impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, [lo, hi]: [f32; 2]) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// Wetting and drying time constants at full precipitation, in seconds.
const WETTING_SECONDS: f32 = 90.;
const DRYING_SECONDS: f32 = 240.;

/// Weather state and automatic sequence. Manual requests blend from the current values, so a
/// change can start mid-transition without a jump.
#[derive(Debug, Clone)]
pub struct WeatherRuntime {
    settings: WeatherSettings,
    rng: SplitMix64,
    from: WeatherParams,
    target: WeatherKind,
    elapsed: f32,
    duration: f32,
    hold_remaining: f32,
    /// Choose the next state automatically once the current one has been held.
    pub automatic: bool,
    wetness: f32,
}
impl WeatherRuntime {
    /// Settled at `initial`. Invalid settings fall back to the defaults.
    pub fn new(
        settings: WeatherSettings,
        seed: u64,
        initial: WeatherKind,
        automatic: bool,
    ) -> Self {
        let settings = if settings.validate().is_ok() {
            settings
        } else {
            WeatherSettings::default()
        };
        let mut runtime = Self {
            rng: SplitMix64(seed),
            from: settings.preset(initial),
            target: initial,
            elapsed: 0.,
            duration: 0.,
            hold_remaining: 0.,
            automatic,
            wetness: 0.,
            settings,
        };
        runtime.hold_remaining = runtime.draw_hold(initial);
        runtime
    }

    fn draw_hold(&mut self, kind: WeatherKind) -> f32 {
        self.rng
            .range(self.settings.schedule.hold_seconds[kind.index()])
    }

    pub fn settings(&self) -> &WeatherSettings {
        &self.settings
    }

    /// Adopt edited settings without restarting. Invalid settings are ignored; an edited
    /// target preset applies immediately, as an authoring change should.
    pub fn set_settings(&mut self, settings: WeatherSettings) {
        if settings.validate().is_ok() {
            if self.progress() >= 1. {
                self.from = settings.preset(self.target);
            }
            self.settings = settings;
        }
    }

    pub fn target(&self) -> WeatherKind {
        self.target
    }
    pub fn wetness(&self) -> f32 {
        self.wetness
    }
    /// 1 once the target is reached.
    pub fn progress(&self) -> f32 {
        if self.duration <= 0. {
            1.
        } else {
            (self.elapsed / self.duration).clamp(0., 1.)
        }
    }
    /// Remaining hold before the next automatic change, once settled.
    pub fn hold_remaining(&self) -> Option<f32> {
        (self.automatic && self.progress() >= 1.).then_some(self.hold_remaining)
    }

    pub fn transition(&self) -> WeatherTransition {
        WeatherTransition {
            from: self.from,
            to: self.settings.preset(self.target),
            progress: self.progress(),
        }
    }

    pub fn current(&self) -> WeatherParams {
        let t = self.progress();
        let to = self.settings.preset(self.target);
        let mut params = self.from.lerp(to, smoothstep(t));
        // Rain starts once the cloud deck has built and stops before it clears.
        let rain_t = if to.precipitation > self.from.precipitation {
            smoothstep((t - 0.4) / 0.6)
        } else {
            smoothstep(t / 0.6)
        };
        params.precipitation =
            self.from.precipitation + (to.precipitation - self.from.precipitation) * rain_t;
        params
    }

    /// Blend from the current values to `kind`. Zero seconds applies it immediately.
    pub fn request(&mut self, kind: WeatherKind, transition_seconds: f32) {
        let duration = transition_seconds.max(0.);
        self.from = if duration > 0. {
            self.current()
        } else {
            self.settings.preset(kind)
        };
        self.target = kind;
        self.elapsed = 0.;
        self.duration = duration;
        self.hold_remaining = self.draw_hold(kind);
    }

    /// Choose the next state from the schedule weights of the current target.
    pub fn next_random(&mut self) -> WeatherKind {
        let weights = self.settings.schedule.next_weights[self.target.index()];
        let mut pick = self.rng.next_f32() * weights.iter().sum::<f32>();
        let mut next = self.target;
        for kind in WeatherKind::ALL {
            let weight = weights[kind.index()];
            if weight > 0. {
                next = kind;
                if pick < weight {
                    break;
                }
                pick -= weight;
            }
        }
        let seconds = self.rng.range(self.settings.schedule.transition_seconds);
        self.request(next, seconds);
        next
    }

    pub fn advance(&mut self, seconds: f32) {
        if !seconds.is_finite() || seconds <= 0. {
            return;
        }
        let precipitation = self.current().precipitation;
        // Exact for constant precipitation over the step, so accelerated clocks stay stable.
        if precipitation > 0.01 {
            let k = precipitation / WETTING_SECONDS;
            self.wetness += (1. - self.wetness) * (1. - (-k * seconds).exp());
        } else {
            self.wetness *= (-seconds / DRYING_SECONDS).exp();
        }
        self.wetness = self.wetness.clamp(0., 1.);
        let mut remaining = seconds;
        // Bounded: each iteration either consumes the step or starts a positive-length change.
        for _ in 0..8 {
            if self.progress() < 1. {
                let step = remaining.min(self.duration - self.elapsed);
                self.elapsed += step;
                remaining -= step;
                if self.elapsed >= self.duration {
                    self.from = self.settings.preset(self.target);
                    self.elapsed = 0.;
                    self.duration = 0.;
                }
            }
            if remaining <= 0. || !self.automatic {
                return;
            }
            if remaining < self.hold_remaining {
                self.hold_remaining -= remaining;
                return;
            }
            remaining -= self.hold_remaining;
            self.next_random();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_and_ordered_by_severity() {
        for kind in WeatherKind::ALL {
            assert!(kind.preset().validate().is_ok(), "{kind:?}");
        }
        let coverage: Vec<_> = WeatherKind::ALL
            .iter()
            .map(|k| k.preset().cloud_coverage)
            .collect();
        assert!(coverage.windows(2).all(|w| w[0] < w[1]));
        assert!(WeatherSchedule::default().validate().is_ok());
    }

    #[test]
    fn overlay_keeps_authored_layout_and_validates() {
        let profile = AtmosphereProfile {
            clouds: crate::clouds::CloudSettings::scattered(),
            ..Default::default()
        };
        for kind in WeatherKind::ALL {
            let applied = kind.preset().apply(&profile);
            assert!(applied.validate().is_ok(), "{kind:?}");
            assert_eq!(applied.clouds.base_metres, profile.clouds.base_metres);
            assert_eq!(applied.clouds.size_metres, profile.clouds.size_metres);
            assert_eq!(applied.clouds.seed, profile.clouds.seed);
            assert_eq!(applied.clouds.wind_degrees, profile.clouds.wind_degrees);
            assert_eq!(applied.night, profile.night);
            for (a, b) in applied.phases.iter().zip(&profile.phases) {
                assert_eq!(
                    (a.sun_srgb, a.sun_lux, a.ambient_lux),
                    (b.sun_srgb, b.sun_lux, b.ambient_lux)
                );
            }
            // The sky model keeps authored clear air; weather visibility is separate fog.
            assert_eq!(applied.visibility_metres, profile.visibility_metres);
            assert_eq!(applied.haze_srgb, profile.haze_srgb);
        }
        let scattered = WeatherKind::Scattered.preset().apply(&profile);
        assert_eq!(scattered.phases, profile.phases);
        assert_eq!(scattered.clouds, profile.clouds);
        assert_eq!(scattered.exposure_ev100, profile.exposure_ev100);
        assert_eq!(WeatherKind::Scattered.preset().fog(&profile).extinction, 0.);
        assert_eq!(WeatherKind::nearest_to(&profile), WeatherKind::Scattered);
    }

    #[test]
    fn fog_adds_only_the_extinction_missing_from_authored_haze() {
        let profile = AtmosphereProfile::default();
        let fog = WeatherKind::Storm.preset().fog(&profile);
        let visibility = profile.visibility_metres * WeatherKind::Storm.preset().visibility_scale;
        assert_eq!(fog.visibility_metres, visibility);
        let total = fog.extinction + 3.912 / profile.visibility_metres;
        assert!((total * visibility - 3.912).abs() < 1e-4);
        let clear = WeatherKind::Clear.preset().fog(&profile);
        assert_eq!(clear.extinction, 0.);
        assert_eq!(
            clear.tint_linear,
            crate::atmosphere::linear_rgb(profile.haze_srgb)
        );
        let mut hazy = profile.clone();
        hazy.visibility_metres = 500.;
        assert!(WeatherKind::Rain.preset().fog(&hazy).visibility_metres >= 50.);
    }

    #[test]
    fn transitions_are_continuous_including_mid_transition_requests() {
        let mut w = WeatherRuntime::new(default_schedule(), 1, WeatherKind::Clear, false);
        assert_eq!(w.current(), WeatherKind::Clear.preset());
        w.request(WeatherKind::Overcast, 60.);
        let mut previous = w.current();
        for step in 0..200 {
            if step == 50 {
                // Redirect halfway: the next value continues from the blended state.
                w.request(WeatherKind::Storm, 30.);
                assert_eq!(w.current(), previous);
            }
            w.advance(0.5);
            let now = w.current();
            assert!((now.cloud_coverage - previous.cloud_coverage).abs() < 0.03);
            assert!((now.visibility_scale - previous.visibility_scale).abs() < 0.05);
            previous = now;
        }
        assert_eq!(w.current(), WeatherKind::Storm.preset());
        assert_eq!(w.progress(), 1.);
        assert_eq!(
            w.hold_remaining(),
            None,
            "manual weather holds indefinitely"
        );
    }

    #[test]
    fn rain_waits_for_the_cloud_deck_and_stops_first() {
        let mut w = WeatherRuntime::new(default_schedule(), 1, WeatherKind::Overcast, false);
        w.request(WeatherKind::Rain, 100.);
        w.advance(35.);
        assert_eq!(w.current().precipitation, 0.);
        assert!(w.current().cloud_coverage > WeatherKind::Overcast.preset().cloud_coverage);
        w.advance(65.);
        assert_eq!(
            w.current().precipitation,
            WeatherKind::Rain.preset().precipitation
        );
        w.request(WeatherKind::Scattered, 100.);
        w.advance(60.);
        assert_eq!(w.current().precipitation, 0.);
        assert!(w.current().cloud_coverage > WeatherKind::Scattered.preset().cloud_coverage);
    }

    #[test]
    fn wetness_accumulates_in_rain_and_dries_independently_of_step_size() {
        let run = |step: f32| {
            let mut w = WeatherRuntime::new(default_schedule(), 1, WeatherKind::Storm, false);
            let mut t = 0.;
            while t < 180. {
                w.advance(step);
                t += step;
            }
            let wet = w.wetness();
            w.request(WeatherKind::Clear, 0.);
            let mut t = 0.;
            while t < 240. {
                w.advance(step);
                t += step;
            }
            (wet, w.wetness())
        };
        let (fine_wet, fine_dry) = run(1. / 120.);
        let (coarse_wet, coarse_dry) = run(6.);
        assert!(fine_wet > 0.8 && fine_wet <= 1.);
        assert!((fine_dry - fine_wet * (-1_f32).exp()).abs() < 0.01);
        assert!((fine_wet - coarse_wet).abs() < 0.01);
        assert!((fine_dry - coarse_dry).abs() < 0.01);
    }

    #[test]
    fn automatic_sequence_is_deterministic_and_follows_allowed_changes() {
        let schedule = WeatherSchedule::default();
        let sequence = |seed| {
            let settings = WeatherSettings {
                schedule: schedule.clone(),
                ..default_schedule()
            };
            let mut w = WeatherRuntime::new(settings, seed, WeatherKind::Clear, true);
            let mut seen = vec![WeatherKind::Clear];
            for _ in 0..20_000 {
                w.advance(5.);
                if *seen.last().unwrap() != w.target() {
                    seen.push(w.target());
                }
            }
            seen
        };
        let a = sequence(42);
        assert_eq!(a, sequence(42));
        assert_ne!(a, sequence(43));
        assert!(a.len() > 50);
        for pair in a.windows(2) {
            let weight = schedule.next_weights[pair[0].index()][pair[1].index()];
            assert!(weight > 0., "{:?} -> {:?}", pair[0], pair[1]);
        }
        for kind in WeatherKind::ALL {
            assert!(a.contains(&kind), "{kind:?} never occurs");
        }
    }

    #[test]
    fn large_steps_are_bounded_and_manual_mode_never_changes_target() {
        let mut w = WeatherRuntime::new(default_schedule(), 7, WeatherKind::Rain, false);
        w.advance(1.0e9);
        assert_eq!(w.target(), WeatherKind::Rain);
        w.advance(f32::NAN);
        w.advance(-1.);
        assert!(w.current().validate().is_ok());
        w.automatic = true;
        w.advance(1.0e9);
        assert!(w.current().validate().is_ok());
    }

    #[test]
    fn invalid_settings_fall_back() {
        let mut settings = WeatherSettings::default();
        settings.schedule.next_weights[0] = [0.; 5];
        assert!(settings.validate().is_err());
        let w = WeatherRuntime::new(settings.clone(), 1, WeatherKind::Clear, true);
        assert_eq!(w.settings(), &WeatherSettings::default());
        settings = WeatherSettings::default();
        settings.presets[0].cloud_coverage = 2.;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn authored_presets_drive_the_runtime_and_live_edits_apply_without_restarting() {
        let mut settings = WeatherSettings::default();
        settings.presets[WeatherKind::Rain.index()].cloud_coverage = 0.7;
        let mut w = WeatherRuntime::new(settings.clone(), 3, WeatherKind::Rain, false);
        assert_eq!(w.current().cloud_coverage, 0.7);
        assert_eq!(w.transition().to.cloud_coverage, 0.7);
        w.advance(30.);
        let wetness = w.wetness();
        settings.presets[WeatherKind::Rain.index()].cloud_coverage = 0.8;
        w.set_settings(settings.clone());
        assert_eq!(w.current().cloud_coverage, 0.8);
        assert_eq!(w.wetness(), wetness, "an edit is not a restart");
        let mut invalid = settings;
        invalid.presets[0].precipitation = -1.;
        w.set_settings(invalid);
        assert_eq!(w.current().cloud_coverage, 0.8);
    }

    fn default_schedule() -> WeatherSettings {
        WeatherSettings::default()
    }
}
