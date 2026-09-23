#[cfg(target_os = "ios")]
use bevy::window::{MonitorSelection, ScreenEdge, WindowMode};
use bevy::{
    asset::AssetPlugin,
    log::LogPlugin,
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use engine::{MinimalGamePlugin, StreamedTerrainSurface, WorldVegetationPlugin};
use vegetation_render::VegetationDebugSettings;

#[cfg(target_os = "macos")]
mod camera_input_trace;
mod frame_pacing;
mod game_render;
mod launch;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod metal_capture;
mod profile;
mod render_audit;
mod repro;
mod runtime_settings;

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
    let canopy = match runtime_settings::load_canopy(&options.canopy_path()) {
        Ok(look) => look,
        Err(error) if options.canopy_path.is_some() => return Err(error),
        Err(error) => {
            eprintln!("Canopy look: {error}; using defaults");
            default()
        }
    };
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
        .insert_resource(engine::TerrainLodPreview {
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

    if options.streaming_smoke {
        app.add_plugins(engine::StreamingSmokePlugin);
    }

    frame_pacing::install(
        &mut app,
        frame_pacing::FrameRate::new(options.fps),
        options.timer_pacing,
    );
    #[cfg(target_os = "macos")]
    camera_input_trace::install(&mut app);
    app.add_plugins((WorldVegetationPlugin, engine::TreeWindPlugin))
        .configure_sets(
            Update,
            engine::WorldVegetationSystems.after(runtime_settings::RuntimeSettingsApply),
        );

    app.world_mut()
        .resource_mut::<vegetation_render::VegetationLighting>()
        .canopy = canopy;
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .density_mode = options.density;
    app.add_systems(
        Update,
        draw_streamed_terrain_page_diagnostics.run_if(runtime_settings::page_gizmos_enabled),
    );

    if options.vertex_reference {
        app.world_mut()
            .resource_mut::<vegetation_render::VegetationBladePreparation>()
            .enabled = false;
    }
    if options.candidate_reference {
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .candidate_cache_enabled = false;
    }
    if options.placement_reference {
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .early_rejection = false;
    }
    app.world_mut()
        .resource_mut::<terrain_render::TerrainPreparedSettings>()
        .enabled = !options.terrain_reference;
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .gpu_counters_enabled = options.counters;
    if options.terrain_universal {
        app.world_mut()
            .resource_mut::<terrain_render::TerrainPreparedSettings>()
            .prefer_native_astc = false;
    }
    if options.msaa_store_reference {
        app.add_systems(
            PostStartup,
            |mut commands: Commands, cameras: Query<Entity, With<engine::WorldViewCamera>>| {
                for camera in &cameras {
                    commands
                        .entity(camera)
                        .insert(engine::MsaaColorStorePolicy::Preserve);
                }
            },
        );
    }
    if options.terrain_procedural {
        app.world_mut()
            .resource_mut::<terrain_render::TerrainCacheSettings>()
            .enabled = false;
    }
    app.insert_resource(options.clouds);
    app.add_plugins(runtime_settings::RuntimeSettingsPlugin);
    // Explicit precedence: launch defaults, reproduction, then profile settings.
    profile::install(&mut app);
    repro::install(&mut app);
    profile::apply_runtime_settings(&mut app);
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

fn draw_streamed_terrain_page_diagnostics(
    terrain_pages: Query<(&Transform, &StreamedTerrainSurface)>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut gizmos: Gizmos,
) {
    let Some(camera) = cameras.iter().next() else {
        return;
    };
    let camera_xz = camera.translation.xz();

    for (transform, page) in &terrain_pages {
        let center_xz = transform.translation.xz();
        if center_xz.distance_squared(camera_xz) > 96.0_f32.powi(2) {
            continue;
        }

        let resolution = usize::from(page.heightfield.resolution);
        let intervals = resolution - 1;
        let color = if (page.key.cell.x ^ page.key.cell.z) & 1 == 0 {
            Color::srgba(0.1, 0.95, 1.0, 0.9)
        } else {
            Color::srgba(1.0, 0.2, 0.85, 0.9)
        };
        let point = |x: usize, z: usize| {
            let u = x as f32 / intervals as f32;
            let v = z as f32 / intervals as f32;
            Vec3::new(
                transform.translation.x + (u - 0.5) * page.cell_size,
                page.heightfield.height_at(x, z) + 0.035,
                transform.translation.z + (v - 0.5) * page.cell_size,
            )
        };

        for index in 0..intervals {
            gizmos.line(point(index, 0), point(index + 1, 0), color);
            gizmos.line(point(index, intervals), point(index + 1, intervals), color);
            gizmos.line(point(0, index), point(0, index + 1), color);
            gizmos.line(point(intervals, index), point(intervals, index + 1), color);
        }

        let sample = page
            .heightfield
            .sample([page.cell_size * 0.5; 2], page.cell_size);
        let root = Vec3::new(
            transform.translation.x,
            sample.height + 0.045,
            transform.translation.z,
        );
        gizmos.line(
            root,
            root + Vec3::from_array(sample.normal) * 0.75,
            Color::srgb(1.0, 0.95, 0.15),
        );
    }
}

fn game_window() -> Window {
    let window = Window {
        title: "Yarra — SQLite World Streaming".into(),
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
