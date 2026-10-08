//! Lightning strikes in game storms (`atmosphere::lightning`): strikes come at random, as often as
//! the current weather calls for, a few kilometres around the camera; F1 and look captures can
//! force one. Editor workspaces and studies never see them.
use crate::{ApplyAtmosphere, AtmosphereOwner, AtmosphereState, GameWeather, WorldViewCamera};
use atmosphere::lightning::{self, Strike};
use bevy::prelude::*;

/// Nearest and farthest strikes, metres from the camera.
const NEAREST: f32 = 1500.0;
const FARTHEST: f32 = 7000.0;

#[derive(Resource, Default)]
pub struct GameLightning {
    strike: Option<Strike>,
    /// Seconds of storm time; stands still while the weather clock is paused.
    clock: f64,
    next: Option<f64>,
    random: u32,
    request: Option<Request>,
    /// Seconds into the strike it is held at, for a still.
    held: Option<f32>,
}

#[derive(Clone, Copy)]
struct Request {
    /// Seconds into the strike to hold it at, for a still; `None` lets it run.
    hold: Option<f32>,
}

impl GameLightning {
    /// A strike now, a few kilometres ahead of the camera.
    pub fn strike_now(&mut self) {
        self.request = Some(Request { hold: None });
    }
    /// A strike ahead of the camera held `age` seconds in, for look captures.
    pub fn hold(&mut self, age: f32) {
        self.request = Some(Request { hold: Some(age) });
    }
    /// The age a strike is held at, if any.
    pub fn holding(&self) -> Option<f32> {
        self.request.and_then(|r| r.hold).or(self.held)
    }
    /// No strike, and none held.
    pub fn clear(&mut self) {
        self.strike = None;
        self.request = None;
        self.held = None;
    }
    fn random(&mut self) -> f32 {
        // xorshift: plenty for spacing strikes.
        let mut x = self.random.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.random = x;
        x as f32 / u32::MAX as f32
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<GameLightning>().add_systems(
        PostUpdate,
        strike
            .after(crate::weather::advance_weather)
            .before(ApplyAtmosphere),
    );
}

#[allow(clippy::too_many_arguments)] // Weather, camera, time and the strike state.
fn strike(
    time: Res<Time>,
    weather: Res<GameWeather>,
    mut atmosphere: ResMut<AtmosphereState>,
    camera: Query<&GlobalTransform, With<WorldViewCamera>>,
    sea: Option<Res<atmosphere::SeaSurface>>,
    mut state: ResMut<GameLightning>,
) {
    if atmosphere.owner != AtmosphereOwner::Game || !atmosphere.profile.outdoor {
        if atmosphere.lightning.is_some() {
            atmosphere.lightning = None;
        }
        state.strike = None;
        return;
    }
    let Some(camera) = camera.iter().next() else {
        return;
    };
    let state = &mut *state;
    if state.random == 0 {
        state.random = (weather.seed() as u32) | 1;
    }
    if state.held.is_none() && !weather.paused {
        state.clock += f64::from(time.delta_secs().min(0.1));
    }
    let rate = weather
        .runtime()
        .map_or(0.0, |r| r.current().lightning_per_minute());
    let base = atmosphere.effective_profile().clouds.base_metres;
    let ground_level = sea.and_then(|s| s.level).unwrap_or(0.0);
    let start = |state: &mut GameLightning, ahead: bool| {
        let forward = camera
            .forward()
            .as_vec3()
            .with_y(0.0)
            .normalize_or(Vec3::NEG_Z);
        let angle = if ahead {
            (state.random() - 0.5) * 0.5
        } else {
            state.random() * std::f32::consts::TAU
        };
        let direction = Quat::from_rotation_y(angle) * forward;
        let distance = if ahead {
            2500.0 + 1500.0 * state.random()
        } else {
            NEAREST + (FARTHEST - NEAREST) * state.random()
        };
        let seed = (state.random() * u32::MAX as f32) as u32;
        let point = camera.translation() + direction * distance;
        state.strike = Some(Strike {
            seed,
            ground: Vec3::new(point.x, ground_level, point.z),
            base: base.max(ground_level + 200.0),
            started: state.clock,
        });
    };
    if let Some(request) = state.request.take() {
        start(state, true);
        state.held = request.hold;
    }
    if state.strike.is_none() && state.held.is_none() && rate > 0.0 {
        let next = match state.next {
            Some(next) => next,
            None => {
                // Exponential waits: strikes come at random, at the weather's rate.
                let wait = -(1.0 - state.random()).max(1e-6).ln() * 60.0 / rate;
                *state.next.insert(state.clock + f64::from(wait))
            }
        };
        if state.clock >= next {
            state.next = None;
            start(state, false);
        }
    }
    if rate <= 0.0 {
        state.next = None;
    }
    let flash = state.strike.and_then(|strike| {
        let now = state
            .held
            .map_or(state.clock, |age| strike.started + f64::from(age));
        lightning::flash(&strike, now)
    });
    if flash.is_none() && state.held.is_none() {
        state.strike = None;
    }
    if atmosphere.lightning != flash {
        atmosphere.lightning = flash;
    }
}
