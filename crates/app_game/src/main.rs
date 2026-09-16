#[cfg(target_os = "ios")]
use bevy::window::{MonitorSelection, ScreenEdge, WindowMode};
use bevy::{
    asset::AssetPlugin,
    diagnostic::{DiagnosticPath, DiagnosticsStore},
    log::LogPlugin,
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use engine::{
    MinimalGamePlugin, StreamedTerrainSurface, StreamedVegetationFieldPage, WorldCatalog,
    WorldOrigin,
};
use vegetation::{VegetationFieldPage, VegetationScene, VegetationSurfaceField};
use vegetation_render::{
    VegetationDebugMode, VegetationDebugScene, VegetationDebugSettings, VegetationDiagnostics,
    VegetationDiagnosticsSnapshot, VegetationRenderPlugin, VegetationWind,
};

mod game_render;
mod grass_bands;
mod grass_field;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod metal_capture;
mod profile;
mod render_audit;

fn main() {
    let asset_root = resolve_asset_root();
    let runtime_database = runtime_database_path(&asset_root);
    let vegetation_v2_debug = vegetation_v2_debug_enabled();
    let mut app = App::new();
    app.insert_resource(ClearColor(Color::srgb(0.055, 0.065, 0.075)))
        .add_plugins(
            DefaultPlugins
                .set(game_log_plugin())
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

    app.add_plugins(VegetationRenderPlugin)
        .insert_resource(
            VegetationDebugScene::new(VegetationScene {
                catalog: vegetation::fixtures::reference_catalog(),
                pages: Vec::new(),
            })
            .expect("empty vegetation runtime scene is valid"),
        )
        .add_systems(Update, conform_vegetation_debug_to_streamed_terrain);

    grass_bands::install(&mut app);
    grass_field::install(&mut app);

    #[cfg(target_os = "ios")]
    warn!(
        "iOS ground + grass baseline: flat terrain rendering and production vegetation LOD enabled"
    );

    if vegetation_v2_debug && !cfg!(target_os = "ios") {
        app.add_systems(Startup, setup_vegetation_debug_legend)
            .add_systems(
                Update,
                (
                    draw_streamed_terrain_page_diagnostics,
                    update_vegetation_debug_legend,
                ),
            );
    }
    if std::env::args_os().any(|argument| argument == "--grass-vertex-reference") {
        app.world_mut()
            .resource_mut::<vegetation_render::VegetationBladePreparation>()
            .enabled = false;
    }
    if std::env::args_os().any(|argument| argument == "--grass-candidate-reference") {
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .candidate_cache_enabled = false;
    }
    if std::env::args_os().any(|argument| argument == "--grass-placement-reference") {
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .early_rejection = false;
    }
    app.world_mut()
        .resource_mut::<terrain_render::TerrainPreparedSettings>()
        .enabled = terrain_prepared_enabled();
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .gpu_counters_enabled = std::env::args_os().any(|arg| arg == "--grass-counters");
    if std::env::args_os().any(|argument| argument == "--terrain-prepared-universal") {
        app.world_mut()
            .resource_mut::<terrain_render::TerrainPreparedSettings>()
            .prefer_native_astc = false;
    }
    if std::env::args_os().any(|argument| argument == "--msaa-store-reference") {
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
    if std::env::args_os().any(|argument| argument == "--terrain-procedural") {
        app.world_mut()
            .resource_mut::<terrain_render::TerrainCacheSettings>()
            .enabled = false;
    }
    if std::env::args_os()
        .any(|argument| argument == "--render-audit" || argument == "--render-repro")
    {
        app.add_plugins(render_audit::RenderAuditPlugin);
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    metal_capture::install(&mut app);
    app.run();
}

// --terrain-prepared remains accepted for existing capture commands; prepared is now normal.
fn terrain_prepared_enabled() -> bool {
    !std::env::args_os().any(|arg| arg == "--terrain-reference")
}

#[derive(Component)]
struct VegetationDebugLegend;

fn game_log_plugin() -> LogPlugin {
    let mut plugin = LogPlugin::default();
    if cfg!(target_os = "ios") {
        plugin
            .filter
            .push_str(",winit::platform_impl::ios::app_state=error");
    }
    #[cfg(target_os = "ios")]
    if std::env::args_os().any(|arg| arg == "--render-console" || arg == "--render-repro") {
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

const VEGETATION_SCHEDULE_GPU: DiagnosticPath =
    DiagnosticPath::const_new("render/vegetation_v2_schedule/elapsed_gpu");
const VEGETATION_GENERATE_GPU: DiagnosticPath =
    DiagnosticPath::const_new("render/vegetation_v2_generate/elapsed_gpu");
const VEGETATION_FINALIZE_GPU: DiagnosticPath =
    DiagnosticPath::const_new("render/vegetation_v2_finalize/elapsed_gpu");
const VEGETATION_DRAW_GPU: DiagnosticPath =
    DiagnosticPath::const_new("render/vegetation_v2_draw/elapsed_gpu");
const VEGETATION_SCHEDULE_CPU: DiagnosticPath =
    DiagnosticPath::const_new("render/vegetation_v2_schedule/elapsed_cpu");
const VEGETATION_GENERATE_CPU: DiagnosticPath =
    DiagnosticPath::const_new("render/vegetation_v2_generate/elapsed_cpu");
const VEGETATION_FINALIZE_CPU: DiagnosticPath =
    DiagnosticPath::const_new("render/vegetation_v2_finalize/elapsed_cpu");
const VEGETATION_DRAW_CPU: DiagnosticPath =
    DiagnosticPath::const_new("render/vegetation_v2_draw/elapsed_cpu");

fn setup_vegetation_debug_legend(
    mut commands: Commands,
    diagnostics: Res<VegetationDiagnostics>,
    wind: Res<VegetationWind>,
    render_diagnostics: Res<DiagnosticsStore>,
) {
    commands.spawn((
        Text::new(vegetation_debug_legend(
            VegetationDebugSettings::default(),
            wind.enabled,
            diagnostics.snapshot(),
            &vegetation_pass_timing_line(&render_diagnostics),
        )),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0.025, 0.03, 0.04, 0.82)),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(14),
            left: px(14),
            padding: UiRect::all(px(8)),
            ..default()
        },
        VegetationDebugLegend,
        Name::new("Vegetation V2 diagnostic legend"),
    ));
}

fn update_vegetation_debug_legend(
    settings: Res<VegetationDebugSettings>,
    wind: Res<VegetationWind>,
    diagnostics: Res<VegetationDiagnostics>,
    render_diagnostics: Res<DiagnosticsStore>,
    mut legend: Single<&mut Text, With<VegetationDebugLegend>>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
) {
    *elapsed += time.delta_secs();
    if !settings.is_changed() && *elapsed < 0.25 {
        return;
    }
    *elapsed = 0.0;
    **legend = Text::new(vegetation_debug_legend(
        *settings,
        wind.enabled,
        diagnostics.snapshot(),
        &vegetation_pass_timing_line(&render_diagnostics),
    ));
}

fn vegetation_debug_legend(
    settings: VegetationDebugSettings,
    wind_enabled: bool,
    diagnostics: VegetationDiagnosticsSnapshot,
    pass_timing_line: &str,
) -> String {
    let description = match settings.mode {
        VegetationDebugMode::ProceduralGeometry => {
            "V2: classify-once indexed procedural LOD\nFixed-budget single/split topology + stable population LOD | X: diagnostics"
        }
        VegetationDebugMode::AcceptedSpecies => {
            "V2: accepted roots by species\nOnly candidates emitted by placement are shown | X: next"
        }
        VegetationDebugMode::ParentLinks => {
            "V2: accepted parent/child links\nYellow: shared parent | Cyan: accepted child link | X: next"
        }
        VegetationDebugMode::CandidateOutcomes => {
            "V2: all page-owned candidates (more than emitted grass)\nGreen: accepted | Blue: density-thinned | Orange: coverage/competition rejected | Magenta: invalid surface | X: next"
        }
        VegetationDebugMode::GroupStructure => {
            "V2: unified group samples\nAccepted roots link to explicit parent or analytic Voronoi centre; color is stable group identity | X: next"
        }
    };
    let eligible = diagnostics.eligible_instances;
    let emitted = diagnostics.emitted_instances;
    let dropped = diagnostics.capacity_dropped_instances;
    let budget_status = if dropped.into_iter().sum::<u32>() == 0 {
        "OK"
    } else {
        "VIOLATION"
    };
    let density_mode = settings.density_mode.label();
    let lighting_mode = settings.lighting_mode.label();
    let far_width_mode = if settings.far_width_compensation {
        "LOD + subpixel"
    } else {
        "authored"
    };
    let wind_mode = if wind_enabled { "strong" } else { "off" };
    format!(
        "{description}\n\
         LOD density: {density_mode} | O: balanced/full/authored\n\
         Far coverage: {far_width_mode} | K: toggle\n\
         Lighting: {lighting_mode} | L: rounded/legacy/unlit/minimal-vertex\n\
         Wind: {wind_mode} | I: toggle\n\
         Profile: {} | P: full/draw-frozen/compute/schedule\n\
         Source: {} pages | {} work items | repacks {} | reallocs {} | upload/reserved {:.2}/{:.2} MiB | revision {}\n\
         GPU: {}/{} scheduled | {} lanes / {} candidate evaluations | sample {}\n\
         {pass_timing_line}\n\
         Draw: {} indices | {} topology vertex inputs | {:.2} MiB instance arena\n\
         Capacity S-H/S-L/P-H/P-L: {}/{}/{}/{}\n\
         Budget: {budget_status} | eligible S-H/S-L/P-H/P-L: {}/{}/{}/{} | emitted: {}/{}/{}/{} | capacity drop: {}/{}/{}/{}",
        settings.profile_mode.label(),
        diagnostics.source_pages,
        diagnostics.source_work_items,
        diagnostics.source_repacks,
        diagnostics.source_buffer_reallocations,
        diagnostics.source_uploaded_bytes as f64 / (1024.0 * 1024.0),
        diagnostics.source_buffer_capacity_bytes as f64 / (1024.0 * 1024.0),
        diagnostics.scene_revision,
        diagnostics.scheduled_work_items,
        diagnostics.source_work_items,
        diagnostics.dispatched_candidate_lanes,
        diagnostics.candidate_evaluations,
        diagnostics.gpu_samples,
        diagnostics.submitted_indices,
        diagnostics.topology_vertex_inputs,
        diagnostics.procedural_instance_bytes as f64 / (1024.0 * 1024.0),
        diagnostics.topology_instance_capacities[0],
        diagnostics.topology_instance_capacities[1],
        diagnostics.topology_instance_capacities[2],
        diagnostics.topology_instance_capacities[3],
        eligible[0],
        eligible[1],
        eligible[2],
        eligible[3],
        emitted[0],
        emitted[1],
        emitted[2],
        emitted[3],
        dropped[0],
        dropped[1],
        dropped[2],
        dropped[3],
    )
}

fn vegetation_pass_timing_line(diagnostics: &DiagnosticsStore) -> String {
    let gpu = [
        smoothed_diagnostic(diagnostics, &VEGETATION_SCHEDULE_GPU),
        smoothed_diagnostic(diagnostics, &VEGETATION_GENERATE_GPU),
        smoothed_diagnostic(diagnostics, &VEGETATION_FINALIZE_GPU),
        smoothed_diagnostic(diagnostics, &VEGETATION_DRAW_GPU),
    ];
    let cpu = [
        smoothed_diagnostic(diagnostics, &VEGETATION_SCHEDULE_CPU),
        smoothed_diagnostic(diagnostics, &VEGETATION_GENERATE_CPU),
        smoothed_diagnostic(diagnostics, &VEGETATION_FINALIZE_CPU),
        smoothed_diagnostic(diagnostics, &VEGETATION_DRAW_CPU),
    ];
    let gpu_line = if gpu.into_iter().any(|value| value.is_some()) {
        format!(
            "Pass GPU ms S/G/F/D: {}/{}/{}/{}",
            format_diagnostic(gpu[0]),
            format_diagnostic(gpu[1]),
            format_diagnostic(gpu[2]),
            format_diagnostic(gpu[3]),
        )
    } else {
        "Pass GPU ms S/G/F/D: unavailable on this backend".to_owned()
    };
    if cpu.into_iter().any(|value| value.is_some()) {
        format!(
            "{gpu_line} | CPU record: {}/{}/{}/{}",
            format_diagnostic(cpu[0]),
            format_diagnostic(cpu[1]),
            format_diagnostic(cpu[2]),
            format_diagnostic(cpu[3]),
        )
    } else {
        gpu_line
    }
}

fn smoothed_diagnostic(diagnostics: &DiagnosticsStore, path: &DiagnosticPath) -> Option<f64> {
    diagnostics
        .get(path)
        .and_then(|diagnostic| diagnostic.smoothed())
}

fn format_diagnostic(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| format!("{value:.3}"))
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

fn conform_vegetation_debug_to_streamed_terrain(
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    field_trial: Res<grass_field::GrassFieldTrial>,
    terrain_pages: Query<(Entity, &StreamedTerrainSurface)>,
    field_pages: Query<(Entity, &StreamedVegetationFieldPage)>,
    mut vegetation_scene: ResMut<VegetationDebugScene>,
    mut previous_pages: Local<Option<Vec<(Entity, Entity)>>>,
    mut previous_origin: Local<Option<(i64, i32, i32)>>,
) {
    let Some(active_space) = origin.space() else {
        return;
    };
    let terrain: std::collections::HashMap<_, _> = terrain_pages
        .iter()
        .filter(|(_, p)| p.key.space == active_space)
        .map(|(entity, p)| {
            (
                (p.key.space.0, p.key.cell.x, p.key.cell.z, p.key.lod),
                entity,
            )
        })
        .collect();
    let signature = paired_vegetation_pages(
        field_pages
            .iter()
            .filter(|(_, p)| p.key.space == active_space)
            .map(|(entity, p)| {
                (
                    entity,
                    (p.key.space.0, p.key.cell.x, p.key.cell.z, p.key.lod),
                )
            }),
        &terrain,
    );
    let frame = (active_space.0, origin.cell().x, origin.cell().z);
    if previous_pages.as_ref() == Some(&signature)
        && *previous_origin == Some(frame)
        && !field_trial.is_changed()
        && !catalog.is_changed()
    {
        return;
    }
    let Some(mut catalog) = catalog.vegetation().cloned() else {
        return;
    };
    if !field_trial.enabled {
        catalog = field_trial.baseline.clone();
    }

    let pages = signature
        .iter()
        .map(|&(field_entity, terrain_entity)| {
            let (_, fields) = field_pages.get(field_entity).unwrap();
            let (_, terrain) = terrain_pages.get(terrain_entity).unwrap();
            let cell_origin = fields.key.cell.origin(fields.cell_size);
            let render_origin = origin.cell().origin(fields.cell_size);
            let resolution = terrain.heightfield.resolution;
            let sample_count = usize::from(resolution).pow(2);
            VegetationFieldPage::from_data(
                [
                    (cell_origin[0] - render_origin[0]) as f32,
                    (cell_origin[1] - render_origin[1]) as f32,
                ],
                fields.cell_size,
                VegetationSurfaceField {
                    resolution,
                    heights: (0..sample_count)
                        .map(|index| {
                            terrain.heightfield.height_at(
                                index % usize::from(resolution),
                                index / usize::from(resolution),
                            )
                        })
                        .collect(),
                    normals_oct: terrain.heightfield.normals_oct.clone(),
                    validity: vec![u8::MAX; sample_count],
                },
                fields.data.clone(),
            )
        })
        .collect();
    *previous_pages = Some(signature);
    *previous_origin = Some(frame);
    let scene = VegetationScene { catalog, pages };
    vegetation_scene
        .replace(scene)
        .expect("resident vegetation and terrain pages form a valid diagnostic scene");
}

// Only terrain actually joined to a vegetation page can invalidate the grass scene.
// Far terrain streaming must not flush placement caches for every resident grass field.
fn paired_vegetation_pages(
    fields: impl Iterator<Item = (Entity, (i64, i32, i32, u8))>,
    terrain: &std::collections::HashMap<(i64, i32, i32, u8), Entity>,
) -> Vec<(Entity, Entity)> {
    let mut paired: Vec<_> = fields
        .filter_map(|(entity, key)| terrain.get(&key).map(|&surface| (key, entity, surface)))
        .collect();
    paired.sort_by_key(|&(key, entity, _)| (key, entity));
    paired
        .into_iter()
        .map(|(_, field, surface)| (field, surface))
        .collect()
}

#[cfg(test)]
mod grass_scene_tests {
    use super::*;
    #[test]
    fn unrelated_terrain_does_not_invalidate_grass_but_joined_terrain_does() {
        let mut world = World::new();
        let field = world.spawn_empty().id();
        let surface = world.spawn_empty().id();
        let unrelated = world.spawn_empty().id();
        let replacement = world.spawn_empty().id();
        let key = (1, 0, 0, 0);
        let mut terrain = std::collections::HashMap::from([(key, surface)]);
        let signature = |terrain: &_| paired_vegetation_pages([(field, key)].into_iter(), terrain);
        let original = signature(&terrain);
        terrain.insert((1, 8, 0, 2), unrelated);
        assert_eq!(signature(&terrain), original);
        terrain.insert(key, replacement);
        assert_ne!(signature(&terrain), original);
        terrain.remove(&key);
        assert!(signature(&terrain).is_empty());
    }
}

fn vegetation_v2_debug_enabled() -> bool {
    std::env::args_os().any(|argument| argument == "--vegetation-v2-debug")
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

fn runtime_database_path(asset_root: &std::path::Path) -> std::path::PathBuf {
    let mut arguments = std::env::args_os();
    while let Some(argument) = arguments.next() {
        if argument == "--world-db" {
            return arguments
                .next()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| panic!("--world-db requires a database path"));
        }
    }
    asset_root.join("generated/demo.runtime.sqlite")
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
