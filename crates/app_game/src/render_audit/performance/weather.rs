//! F1 weather and time tools: force presets, follow the random sequence, accelerate its clock
//! and set or speed up the time of day. Weather and time are scene state like the camera: A/B
//! captures record them but never restore them.
use super::capture::CaptureSession;
use crate::render_audit::font;
use bevy::{prelude::*, ui::Pressed, ui_widgets::Button};
use engine::{AtmosphereState, GameDayClock, GameWeather, PrecipitationPresentation, WeatherKind};

// Clouds change region by region over ~40% of a transition; 10 s is for quick checks only.
const TRANSITIONS: [f32; 3] = [60.0, 10.0, 0.0];
const CLOCKS: [f32; 4] = [1.0, 10.0, 60.0, 0.0];
// A 20-minute authored day passes in 2 minutes at 10x and 20 seconds at 60x.
const DAY_CLOCKS: [f32; 4] = [1.0, 10.0, 60.0, 0.0];

#[derive(Component, Clone, Copy)]
pub(super) enum WeatherAction {
    Automatic,
    Preset(WeatherKind),
    Next,
    Transition,
    Clock,
    Authored,
    RainRendering,
    DayClock,
    Lightning,
    /// Move the time of day by this many hours.
    Hours(i32),
    /// Jump ahead to one of `world::atmosphere::PHASE_TIMES`.
    TimeOf(usize),
    /// Move to the day with the moon's next phase, by eighths of a month.
    MoonPhase,
}
#[derive(Component)]
struct WeatherStatus;

#[derive(Resource, Default)]
struct WeatherPanel {
    transition: usize,
    clock: usize,
    day_clock: usize,
    refreshed: f64,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<WeatherPanel>()
        .add_systems(Update, (pause_during_capture, actions).chain())
        .add_systems(PostUpdate, refresh);
}

pub(super) fn spawn(page: &mut ChildSpawnerCommands, button: impl Fn() -> (Node, BackgroundColor)) {
    page.spawn((
        Text::new("Weather overlays the authored atmosphere: clouds, fog, exposure and the shared grass/tree wind. Fog is drawn with the clouds, so Clouds Off also hides it. Presets blend from the current state and hold it; Auto follows the random sequence. The day clock sets how fast the time of day passes (stopped in profiles, repros and captures). Both clocks pause during A/B captures, which record weather and time but never restore them."),
        font(12.0),
    ));
    page.spawn((Text::new(""), font(13.0), WeatherStatus));
    page.spawn(Node {
        flex_wrap: FlexWrap::Wrap,
        column_gap: px(6),
        row_gap: px(6),
        ..default()
    })
    .with_children(|row| {
        let actions = WeatherKind::ALL
            .map(WeatherAction::Preset)
            .into_iter()
            .chain([
                WeatherAction::Automatic,
                WeatherAction::Next,
                WeatherAction::Transition,
                WeatherAction::Clock,
                WeatherAction::Authored,
                WeatherAction::RainRendering,
                WeatherAction::Lightning,
                WeatherAction::DayClock,
                WeatherAction::Hours(-1),
                WeatherAction::Hours(1),
                WeatherAction::MoonPhase,
            ])
            .chain((0..4).map(WeatherAction::TimeOf));
        for action in actions {
            let (mut node, color) = button();
            node.width = px(218);
            row.spawn((Button, action, node, color))
                .with_child((Text::new(""), font(13.0)));
        }
    });
}

// Weather is optional composition: the panel also runs in apps without GameWeatherPlugin.
fn pause_during_capture(
    session: Res<CaptureSession>,
    weather: Option<ResMut<GameWeather>>,
    clock: Option<ResMut<GameDayClock>>,
) {
    let recording = session.recording();
    if let Some(mut weather) = weather
        && weather.paused != recording
    {
        weather.paused = recording;
    }
    if let Some(mut clock) = clock
        && clock.paused != recording
    {
        clock.paused = recording;
    }
}

#[allow(clippy::too_many_arguments)] // Optional weather, time and lightning resources.
fn actions(
    clicks: Query<&WeatherAction, Added<Pressed>>,
    session: Res<CaptureSession>,
    atmosphere: Option<ResMut<AtmosphereState>>,
    weather: Option<ResMut<GameWeather>>,
    rain: Option<ResMut<PrecipitationPresentation>>,
    mut clock: Option<ResMut<GameDayClock>>,
    mut lightning: Option<ResMut<engine::GameLightning>>,
    mut panel: ResMut<WeatherPanel>,
) {
    let (Some(mut atmosphere), Some(mut weather), Some(mut rain)) = (atmosphere, weather, rain)
    else {
        return;
    };
    if session.recording() {
        return;
    }
    for action in &clicks {
        let profile = &atmosphere.profile;
        match *action {
            WeatherAction::Preset(kind) => {
                weather.request(profile, kind, TRANSITIONS[panel.transition]);
            }
            WeatherAction::Automatic => {
                let automatic = weather.runtime().is_some_and(|r| r.automatic);
                weather.set_automatic(profile, !automatic);
            }
            WeatherAction::Next => {
                weather.next_random(profile);
            }
            WeatherAction::Transition => {
                panel.transition = (panel.transition + 1) % TRANSITIONS.len();
            }
            WeatherAction::Clock => {
                panel.clock = (panel.clock + 1) % CLOCKS.len();
                weather.time_scale = CLOCKS[panel.clock];
            }
            WeatherAction::Authored => weather.clear(),
            WeatherAction::RainRendering => rain.enabled = !rain.enabled,
            WeatherAction::DayClock => {
                if let Some(clock) = clock.as_mut() {
                    // A measurement's held clock starts at 1x.
                    panel.day_clock = if clock.running {
                        (panel.day_clock + 1) % DAY_CLOCKS.len()
                    } else {
                        0
                    };
                    clock.running = true;
                    clock.time_scale = DAY_CLOCKS[panel.day_clock];
                }
            }
            WeatherAction::Lightning => {
                if let Some(lightning) = lightning.as_mut() {
                    lightning.strike_now();
                }
            }
            WeatherAction::Hours(hours) => atmosphere.advance(hours as f32 / 24.0),
            WeatherAction::TimeOf(i) => {
                let ahead = world::atmosphere::PHASE_TIMES[i] - atmosphere.phase;
                atmosphere.advance(ahead.rem_euclid(1.0));
            }
            WeatherAction::MoonPhase => {
                let age = atmosphere.evaluate(&atmosphere.profile).moon_age_days;
                let next = world::atmosphere::moon_phase(age) + 1;
                atmosphere.set_moon_age(next as f32 / 8.0 * world::atmosphere::SYNODIC_MONTH_DAYS);
            }
        }
        // Show the result immediately rather than at the next periodic refresh.
        panel.refreshed = f64::NEG_INFINITY;
    }
}

fn label(
    action: WeatherAction,
    weather: &GameWeather,
    rain: &PrecipitationPresentation,
    clock: Option<&GameDayClock>,
    panel: &WeatherPanel,
) -> String {
    let runtime = weather.runtime();
    match action {
        WeatherAction::Preset(kind) => {
            if runtime.is_some_and(|r| r.target() == kind) {
                format!("> {}", kind.label())
            } else {
                kind.label().into()
            }
        }
        WeatherAction::Automatic => format!(
            "Auto sequence: {}",
            if runtime.is_some_and(|r| r.automatic) {
                "on"
            } else {
                "off"
            }
        ),
        WeatherAction::Next => "Next random change".into(),
        WeatherAction::Transition => {
            let seconds = TRANSITIONS[panel.transition];
            if seconds > 0.0 {
                format!("Preset transition: {seconds:.0} s")
            } else {
                "Preset transition: instant".into()
            }
        }
        WeatherAction::Clock => {
            let scale = CLOCKS[panel.clock];
            if scale > 0.0 {
                format!("Weather clock: {scale:.0}x")
            } else {
                "Weather clock: stopped".into()
            }
        }
        WeatherAction::Authored => "Authored profile (weather off)".into(),
        WeatherAction::RainRendering => format!(
            "Rain rendering: {}",
            if rain.enabled { "on" } else { "off" }
        ),
        WeatherAction::DayClock => match clock {
            Some(clock) if clock.running && clock.time_scale > 0.0 => {
                format!("Day clock: {:.0}x", clock.time_scale)
            }
            Some(_) => "Day clock: stopped".into(),
            None => "Day clock: unavailable".into(),
        },
        WeatherAction::Lightning => "Lightning strike".into(),
        WeatherAction::Hours(hours) => format!("Time {hours:+} h"),
        WeatherAction::TimeOf(i) => {
            let (h, m) = engine::clock_time(world::atmosphere::PHASE_TIMES[i]);
            format!("{} ({h:02}:{m:02})", world::atmosphere::PHASE_NAMES[i])
        }
        WeatherAction::MoonPhase => "Moon: next phase".into(),
    }
}

fn time_status(clock: &GameDayClock, atmosphere: &AtmosphereState) -> String {
    let profile = &atmosphere.profile;
    let (h, m) = engine::clock_time(atmosphere.phase);
    let now = atmosphere.evaluate(profile);
    let elevation = |d: [f32; 3]| d[1].clamp(-1.0, 1.0).asin().to_degrees();
    let sun = elevation(now.direction_to_sun);
    let mode = if clock.paused {
        "paused for capture".to_string()
    } else if !clock.ticking() {
        "stopped".to_string()
    } else {
        format!("{:.0}x", clock.time_scale)
    };
    format!(
        "Day {} {h:02}:{m:02} | sun {sun:+.1}° | {} ({:.0}% lit) {:+.0}° | day {:.0} min | clock {mode}",
        atmosphere.day + 1,
        world::atmosphere::MOON_PHASE_NAMES[world::atmosphere::moon_phase(now.moon_age_days)],
        now.moon_lit * 100.0,
        elevation(now.direction_to_moon),
        profile.day_seconds / 60.0
    )
}

fn status(weather: &GameWeather, atmosphere: &AtmosphereState) -> String {
    let Some(runtime) = weather.runtime() else {
        return format!(
            "Weather: authored profile (start {:?}). Choose a preset or Auto to begin.",
            weather.start()
        );
    };
    let w = runtime.current();
    let profile = atmosphere.effective_profile();
    let visibility = atmosphere
        .weather_fog()
        .map_or(profile.visibility_metres, |fog| fog.visibility_metres);
    let mode = if weather.paused {
        "paused for capture".to_string()
    } else if let Some(hold) = runtime.hold_remaining() {
        format!(
            "auto, next change in {:.0}:{:02.0}",
            (hold / 60.0).floor(),
            hold % 60.0
        )
    } else if runtime.automatic {
        "auto, changing".into()
    } else {
        "manual hold".into()
    };
    format!(
        "Weather: {} {:.0}% | {mode} | seed {}\nClouds: coverage {:.2} | density {:.2} | thickness {:.0} m\nVisibility {:.1} km | fog grey {:.2} | exposure {:+.1} EV\nWind: strength x{:.2} | gusts x{:.2} | rate x{:.2}\nPrecipitation {:.2} | wetness {:.2}",
        runtime.target().label(),
        runtime.progress() * 100.0,
        weather.seed(),
        w.cloud_coverage,
        w.cloud_density,
        profile.clouds.thickness_metres,
        visibility / 1000.0,
        w.haze_grey,
        w.exposure_offset_ev,
        w.wind_strength,
        w.wind_gustiness,
        w.wind_rate,
        w.precipitation,
        runtime.wetness(),
    )
}

#[allow(clippy::too_many_arguments)] // Optional weather resources plus panel text queries.
fn refresh(
    time: Res<Time<Real>>,
    weather: Option<Res<GameWeather>>,
    clock: Option<Res<GameDayClock>>,
    atmosphere: Option<Res<AtmosphereState>>,
    rain: Option<Res<PrecipitationPresentation>>,
    mut panel: ResMut<WeatherPanel>,
    mut texts: Query<&mut Text>,
    status_text: Query<Entity, With<WeatherStatus>>,
    buttons: Query<(&WeatherAction, &Children)>,
) {
    let (Some(weather), Some(atmosphere), Some(rain)) = (weather, atmosphere, rain) else {
        return;
    };
    let now = time.elapsed_secs_f64();
    if now - panel.refreshed < 0.25 {
        return;
    }
    panel.refreshed = now;
    for entity in &status_text {
        if let Ok(mut text) = texts.get_mut(entity) {
            let mut value = status(&weather, &atmosphere);
            if let Some(clock) = clock.as_deref() {
                value = format!("{}\n{value}", time_status(clock, &atmosphere));
            }
            if text.0 != value {
                text.0 = value;
            }
        }
    }
    for (action, children) in &buttons {
        let value = label(*action, &weather, &rain, clock.as_deref(), &panel);
        for child in children {
            if let Ok(mut text) = texts.get_mut(*child)
                && text.0 != value
            {
                text.0 = value.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_buttons_change_the_target_and_labels() {
        let mut app = App::new();
        app.init_resource::<CaptureSession>()
            .init_resource::<engine::AtmosphereState>()
            .init_resource::<PrecipitationPresentation>()
            .insert_resource(GameWeather::new(engine::WeatherStart::Automatic, 1))
            .init_resource::<Time<Real>>();
        install(&mut app);
        let button = app
            .world_mut()
            .spawn((
                WeatherAction::Preset(WeatherKind::Storm),
                Pressed,
                children![Text::new("")],
            ))
            .id();
        app.update();
        let weather = app.world().resource::<GameWeather>();
        let runtime = weather.runtime().expect("a preset starts weather");
        assert_eq!(runtime.target(), WeatherKind::Storm);
        assert!(!runtime.automatic);
        let child = app.world().get::<Children>(button).unwrap()[0];
        assert_eq!(app.world().get::<Text>(child).unwrap().0, "> Storm");
    }
}
