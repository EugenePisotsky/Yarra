//! F1 weather tools: force presets, follow the random sequence and accelerate its clock.
//! Weather is scene state like camera and time: A/B captures record it but never restore it.
use super::capture::CaptureSession;
use crate::render_audit::font;
use bevy::prelude::*;
use engine::{AtmosphereState, GameWeather, WeatherKind};

// Clouds change region by region over ~40% of a transition; 10 s is for quick checks only.
const TRANSITIONS: [f32; 3] = [60.0, 10.0, 0.0];
const CLOCKS: [f32; 4] = [1.0, 10.0, 60.0, 0.0];

#[derive(Component, Clone, Copy)]
pub(super) enum WeatherAction {
    Automatic,
    Preset(WeatherKind),
    Next,
    Transition,
    Clock,
    Authored,
}
#[derive(Component)]
struct WeatherStatus;

#[derive(Resource, Default)]
struct WeatherPanel {
    transition: usize,
    clock: usize,
    refreshed: f64,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<WeatherPanel>()
        .add_systems(Update, (pause_during_capture, actions).chain())
        .add_systems(PostUpdate, refresh);
}

pub(super) fn spawn(page: &mut ChildSpawnerCommands, button: impl Fn() -> (Node, BackgroundColor)) {
    page.spawn((
        Text::new("Weather overlays the authored atmosphere: clouds, fog, exposure and the shared grass/tree wind. Fog is drawn with the clouds, so Clouds Off also hides it. Presets blend from the current state and hold it; Auto follows the random sequence. The clock pauses during A/B captures, which record weather but never restore it."),
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
            ]);
        for action in actions {
            let (mut node, color) = button();
            node.width = px(218);
            row.spawn((Button, action, node, color))
                .with_child((Text::new(""), font(13.0)));
        }
    });
}

// Weather is optional composition: the panel also runs in apps without GameWeatherPlugin.
fn pause_during_capture(session: Res<CaptureSession>, weather: Option<ResMut<GameWeather>>) {
    let Some(mut weather) = weather else {
        return;
    };
    let recording = session.recording();
    if weather.paused != recording {
        weather.paused = recording;
    }
}

fn actions(
    clicks: Query<(&Interaction, &WeatherAction), Changed<Interaction>>,
    session: Res<CaptureSession>,
    atmosphere: Option<Res<AtmosphereState>>,
    weather: Option<ResMut<GameWeather>>,
    mut panel: ResMut<WeatherPanel>,
) {
    let (Some(atmosphere), Some(mut weather)) = (atmosphere, weather) else {
        return;
    };
    if session.recording() {
        return;
    }
    let profile = &atmosphere.profile;
    for (_, action) in clicks.iter().filter(|(i, _)| **i == Interaction::Pressed) {
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
        }
        // Show the result immediately rather than at the next periodic refresh.
        panel.refreshed = f64::NEG_INFINITY;
    }
}

fn label(action: WeatherAction, weather: &GameWeather, panel: &WeatherPanel) -> String {
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
    }
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
        "Weather: {} {:.0}% | {mode} | seed {}\nClouds: coverage {:.2} | density {:.2} | thickness {:.0} m\nVisibility {:.1} km | fog grey {:.2} | exposure {:+.1} EV\nWind: strength x{:.2} | gusts x{:.2} | rate x{:.2}\nPrecipitation {:.2} | wetness {:.2} (rain rendering not yet implemented)",
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

fn refresh(
    time: Res<Time<Real>>,
    weather: Option<Res<GameWeather>>,
    atmosphere: Option<Res<AtmosphereState>>,
    mut panel: ResMut<WeatherPanel>,
    mut texts: Query<&mut Text>,
    status_text: Query<Entity, With<WeatherStatus>>,
    buttons: Query<(&WeatherAction, &Children)>,
) {
    let (Some(weather), Some(atmosphere)) = (weather, atmosphere) else {
        return;
    };
    let now = time.elapsed_secs_f64();
    if now - panel.refreshed < 0.25 {
        return;
    }
    panel.refreshed = now;
    for entity in &status_text {
        if let Ok(mut text) = texts.get_mut(entity) {
            let value = status(&weather, &atmosphere);
            if text.0 != value {
                text.0 = value;
            }
        }
    }
    for (action, children) in &buttons {
        let value = label(*action, &weather, &panel);
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
            .insert_resource(GameWeather::new(engine::WeatherStart::Automatic, 1))
            .init_resource::<Time<Real>>();
        install(&mut app);
        let button = app
            .world_mut()
            .spawn((
                WeatherAction::Preset(WeatherKind::Storm),
                Interaction::Pressed,
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
