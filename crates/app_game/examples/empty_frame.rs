//! Bounded empty-frame comparison using the game's actual presentation code.
//! No world, atmosphere, grass or audit panel is installed.
//! --plain compares a stock Bevy camera; --direct uses the game's direct path.
//! --probes, --shadows and --prepass independently add the game's timing probes,
//! three-cascade directional shadows and depth prepass to isolate fixed overhead.
//! --temporal-bypass retains Temporal's inputs/output path but skips reconstruction.
#![allow(dead_code)] // The shared module also exposes settings to the full game.

#[path = "../src/frame_pacing.rs"]
mod frame_pacing;
#[path = "../src/game_render.rs"]
mod game_render;
#[path = "../src/render_audit/timing.rs"]
mod timing;

use bevy::{
    camera::Hdr,
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use game_render::{GameRenderPlugin, GameRenderSettings, RenderPath};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let value = |flag: &str| {
        args.iter()
            .position(|s| s == flag)
            .map(|i| args.get(i + 1).expect("flag requires a value").as_str())
    };
    let scale: f32 = value("--scale")
        .unwrap_or("1")
        .parse()
        .expect("numeric scale");
    assert!(scale.is_finite() && scale > 0.0 && scale <= 1.0);
    let msaa = match value("--msaa").unwrap_or("4") {
        "1" => Msaa::Off,
        "4" => Msaa::Sample4,
        _ => panic!("--msaa expects 1 or 4"),
    };
    let plain = args.iter().any(|s| s == "--plain");
    let direct = args.iter().any(|s| s == "--direct");
    let probes = args.iter().any(|s| s == "--probes");
    let shadows = args.iter().any(|s| s == "--shadows");
    let prepass = args.iter().any(|s| s == "--prepass");
    let bypass = args.iter().any(|s| s == "--temporal-bypass");
    let settings = GameRenderSettings {
        upscaler: match value("--upscaler").unwrap_or("auto") {
            "auto" => upscaling::UpscaleMethod::Auto,
            "linear" => upscaling::UpscaleMethod::Linear,
            "metalfx-spatial" => upscaling::UpscaleMethod::MetalFxSpatial,
            "metalfx-temporal" => upscaling::UpscaleMethod::MetalFxTemporal,
            _ => panic!("invalid --upscaler"),
        },
        direct_temporal_output: !args.iter().any(|a| a == "--temporal-standard-output"),
        resolution_scale: scale,
        msaa,
        temporal_debug: if bypass {
            upscaling::temporal::TemporalDebug::Bypass
        } else {
            default()
        },
        render_path: if direct {
            RenderPath::Direct
        } else {
            RenderPath::Composite
        },
        ..default()
    };
    assert!(
        !bypass || settings.upscaler == upscaling::UpscaleMethod::MetalFxTemporal,
        "--temporal-bypass requires --upscaler metalfx-temporal"
    );
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Yarra empty-frame diagnostic".into(),
            resolution: WindowResolution::new(3456, 1942),
            present_mode: PresentMode::AutoVsync,
            resizable: false,
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::srgb(0.2, 0.25, 0.3)))
    .insert_resource(upscaling::UpscalingDiagnostics { temporal_timing: args.iter().any(|a| a == "--metalfx-timing-log") })
    .insert_resource(settings)
    .init_resource::<Measurement>()
    .add_systems(
        Startup,
        move |mut commands: Commands, mut window: Single<&mut Window>| {
            window.resolution.set_physical_resolution(3456, 1942);
            let mut camera = commands.spawn((Camera3d::default(), Hdr, msaa, engine::WorldViewCamera));
            if prepass {
                camera.insert(bevy::core_pipeline::prepass::DepthPrepass);
            }
            if shadows {
                commands.spawn((
                    DirectionalLight { illuminance: 128_000.0, shadow_maps_enabled: true, ..default() },
                    bevy::light::CascadeShadowConfigBuilder {
                        num_cascades: 3,
                        first_cascade_far_bound: 20.0,
                        maximum_distance: 80.0,
                        ..default()
                    }.build(),
                    Transform::from_xyz(1.0, 1.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
                ));
            }
            warn!(
                "EMPTY_FRAME start plain={plain} direct={direct} scale={scale} probes={probes} shadows={shadows} prepass={prepass} msaa={}",
                msaa.samples()
            );
        },
    )
    .add_systems(Update, measure);
    if !plain {
        app.add_plugins(GameRenderPlugin);
    }
    if probes {
        app.add_systems(Last, stamp_frame.before(timing::CollectTimings))
            .add_plugins(timing::TimingPlugin {
                log: args.iter().any(|a| a == "--timing-log"),
                gpu_off: args.iter().any(|a| a == "--gpu-timing-off"),
            });
    }
    frame_pacing::install(
        &mut app,
        frame_pacing::FrameRate::new(
            value("--fps")
                .unwrap_or("0")
                .parse()
                .expect("numeric --fps"),
        ),
        args.iter().any(|a| a == "--frame-pacing-timer"),
    );
    app.run();
}

#[derive(Resource, Default)]
struct Measurement {
    started: Option<f64>,
    frames: u64,
}

fn measure(
    time: Res<Time<Real>>,
    window: Single<&Window>,
    camera: Single<(&Msaa, Option<&upscaling::UpscaleStatus>), With<engine::WorldViewCamera>>,
    mut measured: ResMut<Measurement>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs_f64();
    if now < 8.0 {
        return;
    }
    let unix_ms = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    };
    let Some(start) = measured.started else {
        measured.started = Some(now);
        warn!(
            "EMPTY_FRAME event=measure_start unix_ms={} surface={:?} focused={} actual_msaa={} upscaler={:?}",
            unix_ms(),
            window.physical_size(),
            window.focused,
            camera.0.samples(),
            camera.1,
        );
        return;
    };
    measured.frames += 1;
    if now - start >= 8.0 {
        warn!(
            "EMPTY_FRAME event=complete unix_ms={} frames={} seconds={:.3} fps={:.2} focused={}",
            unix_ms(),
            measured.frames,
            now - start,
            measured.frames as f64 / (now - start),
            window.focused
        );
        exit.write(AppExit::Success);
    }
}

fn stamp_frame(frame: Res<bevy::diagnostic::FrameCount>, mut stamp: ResMut<timing::Stamp>) {
    stamp.frame = frame.0;
}
