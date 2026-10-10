#[cfg(target_os = "ios")]
use bevy::window::{MonitorSelection, ScreenEdge, WindowMode};
use bevy::{
    asset::AssetPlugin,
    log::LogPlugin,
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use engine::{MinimalGamePlugin, WorldVegetationPlugin};

#[cfg(target_os = "macos")]
mod camera_input_trace;
mod frame_pacing;
mod game_render;
mod launch;
mod lod_lab;
mod look_capture;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod metal_capture;
mod profile;
mod render_audit;
mod repro;
mod runtime_settings;
mod story;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let options = launch::LaunchOptions::parse(std::env::args_os().skip(1))?;
    if options.help {
        print!("{}", launch::LaunchOptions::help());
        return Ok(());
    }
    let start_view = engine::WorldStartView::load(options.start_view.as_deref())?;
    let asset_root = resolve_asset_root();
    let runtime_database = options
        .world_db
        .clone()
        .unwrap_or_else(|| asset_root.join(world::DEFAULT_RUNTIME_DATABASE));
    let mut app = App::new();
    app.insert_resource(options.clone())
        .insert_resource(start_view)
        .insert_resource(engine::WorldDebugControls {
            world_switch: options.debug_world_switch,
        })
        .insert_resource(engine::TreeInstancing {
            shadow_lod: options.tree_shadow_lod,
            ..default()
        })
        .insert_resource(engine::TerrainHierarchy {
            enabled: !options.terrain_legacy,
            ..default()
        })
        .insert_resource(
            options
                .prepared_blades
                .map(vegetation_render::VegetationPreparationCapacity)
                .unwrap_or_default(),
        )
        .insert_resource(upscaling::UpscalingDiagnostics {
            temporal_timing: options.metalfx_timing,
        })
        .insert_resource(game_render::GameRenderSettings {
            upscaler: options.upscaler,
            direct_temporal_output: !options.temporal_standard_output,
            ..default()
        })
        .insert_resource(ClearColor(Color::srgb(0.055, 0.065, 0.075)))
        .add_plugins(
            DefaultPlugins
                .set(game_log_plugin(&options))
                .set(engine::tree_gltf_plugin())
                .set(AssetPlugin {
                    file_path: asset_root.to_string_lossy().into_owned(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(game_window()),
                    ..default()
                }),
        )
        .add_plugins(MinimalGamePlugin::new(runtime_database))
        .add_plugins(game_render::GameRenderPlugin);

    if options.mode == launch::RunMode::Smoke {
        app.add_plugins(engine::StreamingSmokePlugin);
    }

    frame_pacing::install(
        &mut app,
        frame_pacing::FrameRate::new(options.fps),
        options.timer_pacing,
    );
    #[cfg(target_os = "macos")]
    camera_input_trace::install(&mut app);
    // Variety between sessions; the seed is logged when weather starts.
    let weather_seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    app.insert_resource(engine::GameWeather::new(options.weather, weather_seed))
        .add_plugins(engine::GameWeatherPlugin)
        .insert_resource(engine::GameDayClock::new(options.time, options.day_clock))
        .add_plugins(engine::GameDayClockPlugin);
    app.add_plugins((
        WorldVegetationPlugin,
        engine::TreeWindPlugin,
        engine::OceanPlugin,
    ))
    .configure_sets(
        Update,
        engine::WorldVegetationSystems.after(runtime_settings::RuntimeSettingsApply),
    );

    runtime_settings::apply_launch_options(&mut app)?;
    if let Some(source) = &options.story {
        story::install(&mut app, source)?;
    }
    app.add_plugins(runtime_settings::RuntimeSettingsPlugin);
    // Explicit precedence: launch defaults, reproduction, then profile settings.
    profile::install(&mut app);
    repro::install(&mut app);
    profile::apply_runtime_settings(&mut app);
    lod_lab::install(&mut app)?;
    look_capture::install(&mut app);
    render_audit::install(&mut app);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    metal_capture::install(&mut app);
    app.run();
    Ok(())
}

fn game_log_plugin(_options: &launch::LaunchOptions) -> LogPlugin {
    let mut plugin = LogPlugin::default();
    if cfg!(target_os = "ios") {
        plugin
            .filter
            .push_str(",winit::platform_impl::ios::app_state=error");
    }
    #[cfg(target_os = "ios")]
    if _options.console {
        // Bevy's iOS OSLog layer is visible in Xcode but not devicectl --console.
        // Opt-in stderr output keeps audit settings and thermal transitions with HUD packets.
        plugin.custom_layer = |_| {
            Some(Box::new(
                bevy::log::tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::io::stderr),
            ))
        };
    }
    plugin
}

fn game_window() -> Window {
    let window = Window {
        title: "Yarra".into(),
        resolution: WindowResolution::new(1280, 720),
        present_mode: PresentMode::AutoVsync,
        ..default()
    };

    #[cfg(target_os = "ios")]
    let window = {
        let mut window = window;
        // Windowed mode causes winit to use the desktop-sized resolution above
        // as the actual UIWindow size. Fullscreen uses the device surface.
        window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Primary);
        window.resizable = false;
        window.recognize_pinch_gesture = true;
        window.recognize_pan_gesture = Some((2, 2));
        window.prefers_home_indicator_hidden = true;
        window.prefers_status_bar_hidden = true;
        window.preferred_screen_edges_deferring_system_gestures = ScreenEdge::Bottom;
        window
    };

    window
}

fn resolve_asset_root() -> std::path::PathBuf {
    let mut candidates = Vec::new();
    if let Ok(current_directory) = std::env::current_dir() {
        candidates.push(current_directory.join("assets"));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(executable_directory) = executable.parent()
    {
        candidates.push(executable_directory.join("assets"));
        candidates.push(executable_directory.join("../Resources/assets"));
    }
    candidates.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets"));

    candidates
        .into_iter()
        .find(|candidate| candidate.is_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("assets"))
}
