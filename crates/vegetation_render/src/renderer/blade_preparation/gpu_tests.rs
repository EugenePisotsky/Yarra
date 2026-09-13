//! Real Bevy pipeline/readback coverage for preparation, reference, and bounded overflow.
use super::*;
use crate::*;
use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    render::{
        RenderApp, RenderPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{CachedPipelineState, TextureUsages},
    },
    time::TimeUpdateStrategy,
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Resource, Default, Clone)]
struct Pixels(Arc<Mutex<Vec<u8>>>);

#[test]
#[ignore = "requires a native GPU; run explicitly when changing blade preparation"]
fn prepared_blades_match_reference_with_wind_msaa_and_overflow() {
    let mut app = test_app();
    settled_pixels(&mut app);
    let stats = snapshot(&app);
    assert!(stats.prepared_blades > 0, "{stats:?}");
    assert!(stats.emitted_instances.iter().sum::<u32>() > 100);
    // Keep the same emitted instances for exact A/B comparisons, including compaction order.
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .profile_mode = VegetationProfileMode::DrawFrozen;
    let mut previous = Vec::new();
    for variant in 0..4 {
        {
            let mut wind = app.world_mut().resource_mut::<VegetationWind>();
            wind.enabled = variant != 0;
            wind.phase_seconds = variant as f32 * 1.73;
        }
        if variant == 2 {
            let world = app.world_mut();
            let mut cameras = world.query_filtered::<(&mut Msaa, &mut Transform), With<Camera3d>>();
            for (mut msaa, mut transform) in cameras.iter_mut(world) {
                *msaa = Msaa::Sample4;
                *transform = Transform::from_xyz(13.0, 5.0, 18.0)
                    .looking_at(Vec3::new(12.0, 0.0, 8.0), Vec3::Y);
            }
        }
        if variant == 3 {
            // Force almost every blade through the procedural fallback, including paired bins.
            replace_arena(&mut app, 3);
        }
        app.world_mut()
            .resource_mut::<VegetationBladePreparation>()
            .enabled = true;
        let prepared = settled_pixels(&mut app);
        let stats = snapshot(&app);
        assert!(stats.blade_preparation_enabled);
        if variant == 3 {
            assert!(stats.prepared_blades <= 3 && stats.preparation_fallback_blades > 100);
        } else {
            assert_eq!(stats.preparation_fallback_blades, 0);
            let [single_high, single_low, paired_high, paired_low] = stats.emitted_instances;
            assert_eq!(
                stats.prepared_blades,
                single_high + single_low + 2 * (paired_high + paired_low),
                "paired topology prepares both blades"
            );
        }
        let dispatches = stats.blade_preparation_dispatches;
        settled_pixels(&mut app);
        assert_eq!(
            snapshot(&app).blade_preparation_dispatches,
            dispatches,
            "unchanged wind/camera must reuse preparation"
        );
        app.world_mut()
            .resource_mut::<VegetationBladePreparation>()
            .enabled = false;
        let reference = settled_pixels(&mut app);
        assert!(!snapshot(&app).blade_preparation_enabled);
        let errors: Vec<_> = prepared
            .iter()
            .zip(&reference)
            .map(|(a, b)| a.abs_diff(*b))
            .collect();
        let maximum = errors.iter().copied().max().unwrap();
        let mean = errors.iter().map(|&e| f64::from(e)).sum::<f64>() / errors.len() as f64;
        eprintln!(
            "grass prepared/reference variant {variant}: max byte error={maximum}, mean={mean:.6}, prepared={}, fallback={}",
            stats.prepared_blades, stats.preparation_fallback_blades
        );
        let edge_pixels = errors
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, e)| e.iter().any(|&e| e > 2))
            .map(|(pixel, _)| pixel)
            .collect::<Vec<_>>();
        // Compute and vertex-stage float contraction can move a thin silhouette over one MSAA
        // sample. Keep strict interior matching; allow only a few pixels next to the clear color.
        // The observed case is one pixel (mean byte error 0.00013), not a different blade shape.
        let sparse_msaa_edges = variant >= 2
            && maximum <= 32
            && mean < 0.001
            && edge_pixels.len() <= 4
            && edge_pixels.iter().all(|&pixel| {
                [
                    pixel.checked_sub(1),
                    (pixel + 1 < 256 * 256).then_some(pixel + 1),
                    pixel.checked_sub(256),
                    (pixel + 256 < 256 * 256).then_some(pixel + 256),
                ]
                .into_iter()
                .flatten()
                .any(|neighbor| {
                    prepared[neighbor * 4..neighbor * 4 + 4] == prepared[..4]
                        && reference[neighbor * 4..neighbor * 4 + 4] == reference[..4]
                })
            });
        assert!(
            (maximum <= 2 && mean < 0.05) || sparse_msaa_edges,
            "preparation changed rendered grass: mean={mean}, max={maximum}, edge pixels={}",
            edge_pixels.len(),
        );
        // Nonempty output and genuinely different wind/camera cases, not comparisons of clears.
        let first = &prepared[0..4];
        assert!(prepared.chunks_exact(4).filter(|p| *p != first).count() > 500);
        assert!(prepared != previous);
        previous = prepared;
    }
    // Resume placement and change the source. This must regenerate and refresh preparation.
    app.world_mut()
        .resource_mut::<VegetationBladePreparation>()
        .enabled = true;
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .profile_mode = VegetationProfileMode::Full;
    let before = snapshot(&app).generation_dispatches;
    let mut scene = vegetation::fixtures::reference_scene();
    scene.pages.truncate(1);
    app.world_mut()
        .resource_mut::<VegetationDebugScene>()
        .replace(scene)
        .unwrap();
    settled_pixels(&mut app);
    assert!(snapshot(&app).generation_dispatches > before);
    for mode in [
        VegetationProfileMode::ComputeOnly,
        VegetationProfileMode::ScheduleOnly,
        VegetationProfileMode::Disabled,
        VegetationProfileMode::Full,
    ] {
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .profile_mode = mode;
        settled_pixels(&mut app);
        assert_eq!(
            snapshot(&app).blade_preparation_enabled,
            mode == VegetationProfileMode::Full
        );
    }
}

fn snapshot(app: &App) -> VegetationDiagnosticsSnapshot {
    app.world().resource::<VegetationDiagnostics>().snapshot()
}

fn settled_pixels(app: &mut App) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut ready_frames = 0;
    loop {
        assert!(
            Instant::now() < deadline,
            "vegetation shader/readback timed out"
        );
        app.update();
        assert!(
            app.should_exit().is_none(),
            "renderer requested exit during GPU comparison"
        );
        let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
        for pipeline in cache.pipelines() {
            if let CachedPipelineState::Err(error) = &pipeline.state {
                panic!("GPU pipeline: {error}");
            }
        }
        ready_frames = if cache.waiting_pipelines().count() == 0 {
            ready_frames + 1
        } else {
            0
        };
        // Also refresh the 30-frame asynchronous workload counters.
        if ready_frames >= 65 {
            let pixels = app.world().resource::<Pixels>().0.lock().unwrap().clone();
            if !pixels.is_empty() {
                return pixels;
            }
        }
        std::thread::sleep(Duration::from_millis(3));
    }
}

fn replace_arena(app: &mut App, blades: u64) {
    app.sub_app_mut(RenderApp).world_mut().resource_scope(
        |world, mut preparation: Mut<BladePreparation>| {
            let device = world.resource::<RenderDevice>();
            preparation.arena = device.create_buffer(&BufferDescriptor {
                label: Some("test bounded overflow"),
                size: PROCEDURAL_INSTANCE_CAPACITY as u64 * 4 + blades * BLADE_BYTES,
                usage: BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            preparation.bind_group = None;
            preparation.last_key = None;
            let cache = world.resource::<PipelineCache>();
            let layout =
                cache.get_bind_group_layout(&world.resource::<VegetationPipelines>().draw_layout);
            let buffers = world.resource::<VegetationBuffers>();
            let group = create_draw_bind_group(
                device,
                &layout,
                &buffers.procedural_instances,
                &buffers.diagnostic_instances,
                &buffers.species,
                &buffers.camera,
                &buffers.debug_config,
                &preparation.arena,
            );
            world.resource_mut::<VegetationBuffers>().draw_bind_group = group;
        },
    );
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, pixels: Res<Pixels>) {
    let mut target = Image::new_target_texture(256, 256, TextureFormat::Bgra8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = images.add(target);
    let pixels = pixels.0.clone();
    commands.spawn(Readback::texture(target.clone())).observe(
        move |event: On<ReadbackComplete>| {
            *pixels.lock().unwrap() = event.data.clone();
        },
    );
    commands.spawn((
        Camera3d::default(),
        RenderTarget::Image(target.into()),
        Msaa::Off,
        Transform::from_xyz(12.0, 7.0, 20.0).looking_at(Vec3::new(12.0, 0.0, 8.0), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_rotation(Quat::from_rotation_x(-1.0)),
    ));
}

fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: format!("{}/../../assets", env!("CARGO_MANIFEST_DIR")),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<PipelinedRenderingPlugin>(),
    )
    .add_plugins(VegetationRenderPlugin)
    // Counters are opt-in in the renderer, but these tests assert on GPU readbacks.
    .insert_resource(VegetationDebugSettings {
        gpu_counters_enabled: true,
        ..default()
    })
    .insert_resource(VegetationDebugScene::reference())
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO))
    .init_resource::<Pixels>()
    .add_systems(Startup, setup);
    let deadline = Instant::now() + Duration::from_secs(45);
    while app.plugins_state() != PluginsState::Ready {
        assert!(Instant::now() < deadline, "GPU startup timed out");
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    app
}

#[test]
#[ignore = "requires a native GPU; run when changing candidate caching"]
fn candidate_cache_preserves_population_images_and_source_lifetime() {
    let mut app = test_app();
    settled_pixels(&mut app);
    assert_eq!(
        snapshot(&app).candidate_cache_ready_items,
        snapshot(&app).candidate_cache_planned_items
    );
    assert!(snapshot(&app).candidate_cache_ready_items > 0);
    let builds = snapshot(&app).candidate_cache_builds;
    for (variant, density) in [
        VegetationDensityMode::Balanced,
        VegetationDensityMode::FullReference,
        VegetationDensityMode::Authored,
    ]
    .into_iter()
    .enumerate()
    {
        let world = app.world_mut();
        let mut cameras = world.query_filtered::<(&mut Msaa, &mut Transform), With<Camera3d>>();
        for (mut msaa, mut transform) in cameras.iter_mut(world) {
            *msaa = Msaa::Sample4;
            *transform = Transform::from_xyz(12.0 + variant as f32, 3.0, 18.0)
                .looking_at(Vec3::new(12.0, 0.0, 8.0), Vec3::Y);
        }
        app.world_mut()
            .resource_mut::<VegetationWind>()
            .phase_seconds = 1.73 + variant as f32;
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .density_mode = density;
        compare_candidate_images(&mut app);
        assert_eq!(
            snapshot(&app).candidate_cache_builds,
            builds,
            "movement, density and wind must reuse stable acceptance"
        );
    }
    // Repeated unload/reload, different surface validity and peer coverage must invalidate cache.
    for revision in 0..3 {
        let mut scene = vegetation::fixtures::reference_scene();
        if revision != 1 {
            scene.pages.truncate(1);
        }
        if revision == 2 {
            scene.pages[0].fields[0].coverage.fill(0);
            for value in scene.pages[0].surface.validity.iter_mut().step_by(3) {
                *value = 0;
            }
        }
        app.world_mut()
            .resource_mut::<VegetationDebugScene>()
            .replace(scene)
            .unwrap();
        compare_candidate_images(&mut app);
        assert!(snapshot(&app).candidate_cache_builds > builds);
    }
    // Force every entry to the reference fallback without changing source or camera.
    app.sub_app_mut(RenderApp).world_mut().resource_scope(
        |world, mut cache: Mut<candidate_cache::CandidateCache>| {
            world.resource::<RenderQueue>().write_buffer(
                &cache.entries,
                0,
                &vec![0; cache.entries.size() as usize],
            );
            cache.serial += 1;
        },
    );
    compare_candidate_images(&mut app);
    for mode in [
        VegetationDebugMode::AcceptedSpecies,
        VegetationDebugMode::CandidateOutcomes,
        VegetationDebugMode::ProceduralGeometry,
    ] {
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .mode = mode;
        compare_candidate_images(&mut app);
    }
}

fn compare_candidate_images(app: &mut App) {
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .candidate_cache_enabled = true;
    let cached = settled_pixels(app);
    let cached_stats = snapshot(app);
    let production = app.world().resource::<VegetationDebugSettings>().mode
        == VegetationDebugMode::ProceduralGeometry;
    let cached_instances = production.then(|| generated_instances(app));
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .candidate_cache_enabled = false;
    let reference = settled_pixels(app);
    let reference_stats = snapshot(app);
    if let Some(instances) = cached_instances {
        assert_eq!(
            instances,
            generated_instances(app),
            "cache changed generated instance data"
        );
    }
    assert_eq!(
        cached_stats.emitted_instances, reference_stats.emitted_instances,
        "cache changed population"
    );
    assert_eq!(cached_stats.capacity_dropped_instances, [0; 4]);
    assert_eq!(reference_stats.capacity_dropped_instances, [0; 4]);
    let errors: Vec<_> = cached
        .iter()
        .zip(&reference)
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let maximum = *errors.iter().max().unwrap();
    let mean = errors.iter().map(|&v| f64::from(v)).sum::<f64>() / errors.len() as f64;
    eprintln!(
        "candidate cache/reference: max={maximum}, mean={mean:.6}, population={:?}, evaluations={}/{}",
        cached_stats.emitted_instances,
        cached_stats.candidate_evaluations,
        reference_stats.candidate_evaluations
    );
    // Compaction changes append order. Exact instance multisets above separate data changes
    // from different winners at equal-depth MSAA samples where blades intersect.
    let changed = errors.iter().filter(|&&error| error > 2).count() as f64 / errors.len() as f64;
    assert!(
        mean < 0.01 && changed < 0.001,
        "cache changed the rendered image: {changed} channels differ by >2"
    );
    assert!(cached_stats.emitted_instances.iter().sum::<u32>() > 100);
}

fn generated_instances(app: &App) -> Vec<Vec<[u32; 8]>> {
    use bevy::render::render_resource::{CommandEncoderDescriptor, MapMode, PollType};
    let world = app.sub_app(RenderApp).world();
    let device = world.resource::<RenderDevice>();
    let queue = world.resource::<RenderQueue>();
    let buffers = world.resource::<VegetationBuffers>();
    let bytes = DRAW_ARGS_SIZE + buffers.procedural_instances.size();
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("candidate comparison instance multisets"),
        size: bytes,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(&buffers.args, 0, &readback, 0, DRAW_ARGS_SIZE);
    encoder.copy_buffer_to_buffer(
        &buffers.procedural_instances,
        0,
        &readback,
        DRAW_ARGS_SIZE,
        buffers.procedural_instances.size(),
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    receiver.recv().unwrap().unwrap();
    let result = {
        let mapped = readback.slice(..).get_mapped_range();
        let args: &[u32] = bytemuck::cast_slice(&mapped[..DRAW_ARGS_SIZE as usize]);
        let instances: &[[u32; 8]] = bytemuck::cast_slice(&mapped[DRAW_ARGS_SIZE as usize..]);
        (0..4)
            .map(|bin| {
                let start = args[bin * 5 + 4] as usize;
                let count = args[bin * 5 + 1] as usize;
                let mut records = instances[start..start + count].to_vec();
                records.sort_unstable();
                records
            })
            .collect()
    };
    readback.unmap();
    result
}

#[test]
#[ignore = "requires a native GPU; run when changing topology or geometry morphs"]
fn paired_lod_boundary_matches_rendered_shape_and_lighting() {
    let mut app = test_app();
    settled_pixels(&mut app);
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .profile_mode = VegetationProfileMode::DrawFrozen;
    app.world_mut()
        .resource_mut::<VegetationBladePreparation>()
        .enabled = false;
    // Freeze placement, then exercise the real draw shader and both actual index ranges with
    // identical roots. This detects a raster/shading seam, not just matching curve endpoints.
    let scene = vegetation::fixtures::reference_scene();
    let mut species = scene
        .catalog
        .species
        .iter()
        .find(|s| matches!(s.topology, TopologyProfile::Ribbon(_)))
        .unwrap()
        .clone();
    species.bounds.minimum_height = 0.8;
    species.bounds.maximum_height = 0.8;
    species.bounds.minimum_half_width = 0.025;
    species.bounds.maximum_half_width = 0.025;
    species.height.pair_below_height = 0.8;
    if let TopologyProfile::Ribbon(ref mut profile) = species.topology {
        profile.high_section_count = 5;
        profile.low_section_count = 2;
        profile.blades_per_render_unit = 2;
        profile.longitudinal_power = 0.92;
    }
    let packed = pack_species(&species, [12.0, 12.0]);
    for bands in [VegetationBladeBands::Off, VegetationBladeBands::Medium] {
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .blade_bands = bands;
        for overhead in [false, true] {
            {
                let world = app.world_mut();
                let mut cameras =
                    world.query_filtered::<(&mut Msaa, &mut Transform), With<Camera3d>>();
                for (mut msaa, mut transform) in cameras.iter_mut(world) {
                    *msaa = Msaa::Off;
                    *transform = if overhead {
                        Transform::from_xyz(0.0, 4.0, 0.01).looking_at(Vec3::ZERO, Vec3::Y)
                    } else {
                        Transform::from_xyz(0.0, 1.2, 4.0)
                            .looking_at(Vec3::new(0.0, 0.2, 0.0), Vec3::Y)
                    };
                }
                let mut wind = world.resource_mut::<VegetationWind>();
                wind.enabled = true;
                wind.phase_seconds = 1.73;
            }
            for density in [255u32, 166] {
                let mut captures = Vec::new();
                for low in [false, true] {
                    let instances = (0..16u32)
                        .map(|i| ProceduralInstanceGpu {
                            root_clump: [
                                (i % 4) as f32 * 0.45 - 0.7,
                                0.0,
                                (i / 4) as f32 * 0.45 - 0.7,
                                f32::from_bits(24_000 | ((i * 4_000) << 16)),
                            ],
                            geometry: [
                                32_767,
                                if low { 1 << 31 } else { 0 },
                                0,
                                12_345 + i * 37 | (density << 24),
                            ],
                        })
                        .collect::<Vec<_>>();
                    let render_world = app.sub_app(RenderApp).world();
                    let buffers = render_world.resource::<VegetationBuffers>();
                    let queue = render_world.resource::<RenderQueue>();
                    queue.write_buffer(&buffers.species, 0, bytemuck::bytes_of(&packed));
                    queue.write_buffer(
                        &buffers.procedural_instances,
                        0,
                        bytemuck::cast_slice(&instances),
                    );
                    let mut args = [0u32; 20];
                    args[0] = if low {
                        SPLIT_LOW_INDEX_COUNT
                    } else {
                        SPLIT_HIGH_INDEX_COUNT
                    };
                    args[1] = instances.len() as u32;
                    args[2] = if low {
                        SPLIT_LOW_FIRST_INDEX
                    } else {
                        SPLIT_HIGH_FIRST_INDEX
                    };
                    queue.write_buffer(&buffers.args, 0, bytemuck::cast_slice(&args));
                    captures.push(settled_pixels(&mut app));
                }
                let errors = captures[0]
                    .iter()
                    .zip(&captures[1])
                    .map(|(a, b)| a.abs_diff(*b))
                    .collect::<Vec<_>>();
                let mean = errors.iter().map(|&e| f64::from(e)).sum::<f64>() / errors.len() as f64;
                let changed = errors.iter().filter(|&&e| e > 2).count();
                eprintln!(
                    "paired LOD raster boundary: bands={bands:?}, overhead={overhead}, density={density}, mean byte error={mean:.6}, channels over 2={changed}"
                );
                assert!(
                    mean < 0.08 && changed < 200,
                    "high/low raster seam: mean={mean}, changed={changed}"
                );
                let first = &captures[0][..4];
                assert!(
                    captures[0].chunks_exact(4).filter(|p| *p != first).count() > 500,
                    "must render visible grass"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires a native GPU; validates shape inspection against production placement"]
fn shape_inspection_preserves_production_population_and_density() {
    let mut app = test_app();
    settled_pixels(&mut app);
    let flatten = |bins: Vec<Vec<[u32; 8]>>| {
        let mut records = bins.into_iter().flatten().collect::<Vec<_>>();
        records.sort_unstable();
        records
    };
    let production = flatten(generated_instances(&app));
    assert!(production.len() > 100);
    assert!(
        production.iter().any(|r| r[5] >> 31 != 0),
        "exercise promotion from low bins"
    );
    assert!(
        production.iter().any(|r| r[5] >> 31 == 0),
        "exercise high bins"
    );
    let mut images = Vec::new();
    for mode in [
        VegetationShapeInspection::Current,
        VegetationShapeInspection::Full,
        VegetationShapeInspection::Low,
        VegetationShapeInspection::Morph,
        VegetationShapeInspection::Cause,
    ] {
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .shape_inspection = mode;
        images.push(settled_pixels(&mut app));
        let bins = generated_instances(&app);
        assert!(bins[1].is_empty() && bins[3].is_empty());
        assert_eq!(
            flatten(bins),
            production,
            "{mode:?} changed roots, seeds, species, density or original morph"
        );
        assert_eq!(snapshot(&app).capacity_dropped_instances, [0; 4]);
        // Preparation and fallback must honor the same override, including the cached low shoulder.
        app.world_mut()
            .resource_mut::<VegetationBladePreparation>()
            .enabled = false;
        let fallback = settled_pixels(&mut app);
        let prepared = images.last().unwrap();
        let errors = prepared
            .iter()
            .zip(&fallback)
            .map(|(a, b)| a.abs_diff(*b))
            .collect::<Vec<_>>();
        let mean = errors.iter().map(|&v| f64::from(v)).sum::<f64>() / errors.len() as f64;
        assert!(mean < 0.01, "{mode:?} preparation mismatch: {mean}");
        app.world_mut()
            .resource_mut::<VegetationBladePreparation>()
            .enabled = true;
    }
    assert_ne!(images[0], images[1], "full override had no effect");
    assert_ne!(images[1], images[2], "low override had no effect");
    assert_ne!(images[3], images[4], "cause and morph views must differ");
    {
        let mut settings = app.world_mut().resource_mut::<VegetationDebugSettings>();
        settings.shape_inspection = VegetationShapeInspection::Full;
        settings.inspection_disable_opening = true;
    }
    let closed = settled_pixels(&mut app);
    assert_eq!(flatten(generated_instances(&app)), production);
    assert_ne!(closed, images[1], "opening isolation had no effect");
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .shape_inspection = VegetationShapeInspection::Off;
    settled_pixels(&mut app);
    assert_eq!(
        flatten(generated_instances(&app)),
        production,
        "returning to production changed placement"
    );
}
