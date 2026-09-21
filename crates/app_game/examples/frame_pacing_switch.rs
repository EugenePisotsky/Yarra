//! Bounded native transition check using the game's production pacing module.
//! cargo run --release -p yarra-app-game --example frame_pacing_switch
//! Set MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 for native presentation evidence.
#![allow(dead_code)] // Shared production module includes the panel's label helpers.
#[path = "../src/frame_pacing.rs"]
mod frame_pacing;

use bevy::{
    prelude::*,
    window::{PresentMode, PrimaryWindow, WindowResolution},
};
use frame_pacing::{FramePacing, FrameRate};
use std::time::Instant;

const RATES: [u32; 6] = [0, 60, 30, 120, 0, 60];
const SETTLE: f64 = 2.0;
const SAMPLE: f64 = 3.0;

#[derive(Resource, Default)]
struct Run {
    phase: usize,
    started: Option<Instant>,
    last: Option<Instant>,
    samples: Vec<f64>,
    measuring: bool,
    max_interval_ms: f64,
}

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Yarra FPS transition check".into(),
            resolution: WindowResolution::new(640, 360),
            present_mode: PresentMode::AutoVsync,
            ..default()
        }),
        ..default()
    }))
    .init_resource::<Run>()
    .add_systems(Startup, |mut commands: Commands| {
        commands.spawn((Camera3d::default(), Msaa::Off));
    })
    .add_systems(Update, advance);
    frame_pacing::install(&mut app, frame_pacing::FrameRate::default(), false);
    frame_pacing::set_fps(&mut app, RATES[0]);
    app.run();
}

fn advance(
    mut run: ResMut<Run>,
    mut pacing: ResMut<FramePacing>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut exit: MessageWriter<AppExit>,
) {
    if run.phase == RATES.len() {
        return;
    }
    let now = Instant::now();
    let Some(start) = run.started else {
        run.started = Some(now);
        run.last = Some(now);
        warn!(
            "FPS_SWITCH phase={} target={} event=settle",
            run.phase, RATES[run.phase]
        );
        return;
    };
    let elapsed = now.duration_since(start).as_secs_f64();
    let interval = now
        .duration_since(run.last.replace(now).unwrap())
        .as_secs_f64()
        * 1000.0;
    run.max_interval_ms = run.max_interval_ms.max(interval);
    if elapsed >= SETTLE {
        if !run.measuring {
            run.measuring = true;
            warn!(
                "FPS_SWITCH phase={} target={} event=measure focused={}",
                run.phase, RATES[run.phase], window.focused
            );
        } else {
            run.samples.push(interval);
        }
    }
    if elapsed < SETTLE + SAMPLE {
        return;
    }
    run.samples.sort_by(f64::total_cmp);
    let count = run.samples.len();
    let mean = run.samples.iter().sum::<f64>() / count as f64;
    let median = run.samples[count / 2];
    let p95 = run.samples[(count as f64 * 0.95) as usize];
    let target = RATES[run.phase];
    warn!(
        "FPS_SWITCH phase={} target={target} event=complete frames={count} fps={:.3} median_ms={median:.3} p95_ms={p95:.3} transition_max_ms={:.3} focused={}",
        run.phase,
        1000.0 / mean,
        run.max_interval_ms,
        window.focused
    );
    assert!(count >= 30, "event loop stalled during transition");
    if run.phase > 0 {
        assert!(run.max_interval_ms < 250.0, "transition stalled");
    }
    // This native probe targets the user's 120 Hz ProMotion display. Unlike this
    // assertion, the production cap also supports slower displays / OS restrictions.
    if target != 0 {
        assert!(
            (1000.0 / mean - f64::from(target)).abs() < f64::from(target) * 0.1,
            "unexpected update rate after changing to {target}: {}",
            1000.0 / mean
        );
    }
    run.phase += 1;
    if run.phase == RATES.len() {
        exit.write(AppExit::Success);
        return;
    }
    pacing.rate = FrameRate::new(RATES[run.phase]);
    run.started = Some(now);
    run.measuring = false;
    run.samples.clear();
    run.max_interval_ms = 0.0;
    warn!(
        "FPS_SWITCH phase={} target={} event=settle",
        run.phase, RATES[run.phase]
    );
}
