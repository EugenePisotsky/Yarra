//! Game time of day: advances the atmosphere's day phase at the authored day length, counting
//! the days that move the moon. Editor workspaces own the phase themselves and are left alone.
use crate::{ActiveWorldSpace, ApplyAtmosphere, AtmosphereOwner, AtmosphereState};
use bevy::prelude::*;

#[derive(Resource)]
pub struct GameDayClock {
    /// Day phase to start at instead of the authored one, 0..1.
    start: Option<f32>,
    started: bool,
    /// Whether time passes; measurements and captures hold the time they start at.
    pub running: bool,
    /// Freeze the clock, e.g. during an A/B capture.
    pub paused: bool,
    /// Clock multiplier for watching the day go by.
    pub time_scale: f32,
}
impl GameDayClock {
    pub fn new(start: Option<f32>, running: bool) -> Self {
        assert!(
            start.is_none_or(|phase| (0.0..1.0).contains(&phase)),
            "day phase must be within 0..1"
        );
        Self {
            start,
            started: false,
            running,
            paused: false,
            time_scale: 1.0,
        }
    }
    /// Whether time passes now.
    pub fn ticking(&self) -> bool {
        self.running && !self.paused && self.time_scale > 0.0
    }
}

/// Hours and minutes of a day phase, rounded down to the minute.
pub fn clock_time(phase: f32) -> (u32, u32) {
    let minutes = (phase.rem_euclid(1.0) * 1440.0) as u32 % 1440;
    (minutes / 60, minutes % 60)
}

pub struct GameDayClockPlugin;
impl Plugin for GameDayClockPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GameDayClock>() {
            app.insert_resource(GameDayClock::new(None, true));
        }
        app.add_systems(
            PostUpdate,
            advance_day
                .after(crate::world_streaming::sync_world_atmosphere)
                .before(ApplyAtmosphere),
        );
    }
}

fn advance_day(
    time: Res<Time>,
    active: Option<Res<ActiveWorldSpace>>,
    mut clock: ResMut<GameDayClock>,
    mut atmosphere: ResMut<AtmosphereState>,
) {
    if atmosphere.owner != AtmosphereOwner::Game {
        return;
    }
    // The authored phase arrives with the first active world space.
    if !clock.started {
        if active.is_none_or(|a| a.current().is_none()) {
            return;
        }
        clock.started = true;
        if let Some(phase) = clock.start {
            atmosphere.phase = phase;
        }
        return;
    }
    if !clock.ticking() || !atmosphere.profile.outdoor {
        return;
    }
    let days = time.delta_secs() * clock.time_scale.min(10_000.0) / atmosphere.profile.day_seconds;
    atmosphere.advance(days);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use world::WorldSpaceId;

    fn app(clock: GameDayClock) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(clock)
            .insert_resource(AtmosphereState::default())
            .insert_resource(ActiveWorldSpace::current_for_tests(WorldSpaceId(1)))
            .configure_sets(PostUpdate, ApplyAtmosphere)
            .add_systems(PostUpdate, advance_day.before(ApplyAtmosphere));
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(100),
        ));
        app
    }

    #[test]
    fn the_day_passes_at_the_authored_length_from_the_chosen_start() {
        let mut app = app(GameDayClock::new(Some(0.7), true));
        app.update();
        let state = app.world().resource::<AtmosphereState>();
        assert_eq!(state.phase, 0.7, "the start is applied before time passes");
        let day = state.profile.day_seconds;
        app.world_mut().resource_mut::<GameDayClock>().time_scale = 60.0;
        app.update();
        let phase = app.world().resource::<AtmosphereState>().phase;
        assert!((phase - (0.7 + 6.0 / day)).abs() < 1e-5, "{phase}");
        assert_eq!(clock_time(0.75), (18, 0));
        assert_eq!(clock_time(0.999_999), (23, 59));
    }

    #[test]
    fn captures_measurements_and_editors_hold_the_time() {
        let held = |clock: GameDayClock, owner: AtmosphereOwner| {
            let mut app = app(clock);
            app.world_mut().resource_mut::<AtmosphereState>().owner = owner;
            app.world_mut().resource_mut::<AtmosphereState>().phase = 0.3;
            for _ in 0..4 {
                app.update();
            }
            app.world().resource::<AtmosphereState>().phase == 0.3
        };
        assert!(held(GameDayClock::new(None, false), AtmosphereOwner::Game));
        let mut paused = GameDayClock::new(None, true);
        paused.paused = true;
        assert!(held(paused, AtmosphereOwner::Game));
        assert!(held(
            GameDayClock::new(Some(0.9), true),
            AtmosphereOwner::Editor
        ));
        assert!(!held(GameDayClock::new(None, true), AtmosphereOwner::Game));
    }
}
