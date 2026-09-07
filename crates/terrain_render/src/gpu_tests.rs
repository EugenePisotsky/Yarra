//! Explicit GPU smoke test: compile and draw every terrain diagnostic with real Bevy imports.
use super::*;
use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    core_pipeline::prepass::DepthPrepass,
    render::{
        RenderApp, RenderPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::sync::{Arc, Mutex};

#[derive(Resource, Default, Clone)]
struct Pixels(Arc<Mutex<Vec<u8>>>);

#[test]
#[ignore = "requires a native GPU adapter; run explicitly on a development machine"]
fn terrain_variants_compile_and_render() {
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
    .add_plugins(TerrainRenderPlugin)
    .init_resource::<Pixels>()
    .add_systems(Startup, setup);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(
            std::time::Instant::now() < deadline,
            "renderer startup timed out"
        );
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let shader: Handle<Shader> = app.world().resource::<AssetServer>().load(TERRAIN_SHADER);
    let mut consecutive_ready = 0;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "terrain pipelines timed out"
        );
        app.update();
        let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
        let mut ready = 0;
        for pipeline in cache.pipelines() {
            if let PipelineDescriptor::RenderPipelineDescriptor(descriptor) = &pipeline.descriptor
                && descriptor
                    .fragment
                    .as_ref()
                    .is_some_and(|f| f.shader == shader)
            {
                match &pipeline.state {
                    CachedPipelineState::Err(error) => panic!("terrain pipeline: {error}"),
                    CachedPipelineState::Ok(_) => ready += 1,
                    _ => (),
                }
            }
        }
        consecutive_ready = if ready >= 4 { consecutive_ready + 1 } else { 0 };
        // Keep drawing after all variants compile to also exercise their bind groups.
        if consecutive_ready >= 5
            && app.world().resource::<TerrainCacheStats>().snapshot().ready == 4
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Changing macro strength still happens at draw time; changing lattice inputs rebuilds.
    // Compare after each edit to exercise negative coordinates and different repeat rates.
    for variant in 0..3 {
        if variant == 2 {
            let world = app.world_mut();
            let mut cameras = world.query_filtered::<&mut Msaa, With<Camera3d>>();
            for mut msaa in cameras.iter_mut(world) {
                *msaa = Msaa::Sample4;
            }
        }
        if variant > 0 {
            for (_, material) in app
                .world_mut()
                .resource_mut::<Assets<TerrainMaterial>>()
                .iter_mut()
            {
                material.settings.tile_sizes.x *= 0.7;
                material.settings.macro_settings.y = if variant == 1 { 0.0 } else { 1.0 };
            }
        }
        app.world_mut()
            .resource_mut::<TerrainCacheSettings>()
            .enabled = true;
        let cached = settled_pixels(&mut app, true);
        assert!(
            cached.chunks_exact(4).any(|p| p[0] != p[1]),
            "test must render coloured terrain"
        );
        let builds = app
            .world()
            .resource::<TerrainCacheStats>()
            .snapshot()
            .builds;
        let _ = settled_pixels(&mut app, true);
        assert_eq!(
            app.world()
                .resource::<TerrainCacheStats>()
                .snapshot()
                .builds,
            builds,
            "stationary tables must not rebuild"
        );
        app.world_mut()
            .resource_mut::<TerrainCacheSettings>()
            .enabled = false;
        let reference = settled_pixels(&mut app, false);
        assert_eq!(cached.len(), reference.len());
        let errors: Vec<_> = cached
            .iter()
            .zip(&reference)
            .map(|(a, b)| a.abs_diff(*b))
            .collect();
        let maximum = errors.iter().copied().max().unwrap();
        let mean = errors.iter().map(|&e| f64::from(e)).sum::<f64>() / errors.len() as f64;
        eprintln!(
            "terrain cache/reference variant {variant}: max byte error={maximum}, mean={mean:.6}"
        );
        assert!(
            maximum <= 2 && mean < 0.05,
            "cache changed terrain: max={maximum}, mean={mean}"
        );
        assert_eq!(
            app.world().resource::<TerrainCacheStats>().snapshot().bytes,
            0
        );
    }
    exercise_prepared_material(&mut app);
}

fn exercise_prepared_material(app: &mut App) {
    let albedo = load_repeat_image(
        app.world().resource::<AssetServer>(),
        "local/terrain/temperate_meadow/runtime/universal/base_color_array.ktx2",
        true,
    );
    let macro_image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![191, 191, 191, 255],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::default(),
        ));
    for (_, material) in app
        .world_mut()
        .resource_mut::<Assets<TerrainMaterial>>()
        .iter_mut()
    {
        material.source_base_color_array = albedo.clone();
        material.macro_variation = macro_image.clone();
        material.settings.macro_scales = Vec4::new(8.0, 32.0, 128.0, 0.2);
    }
    // Only the lit and surface-unlit variants are eligible; flat/one-texture stay diagnostic.
    app.world_mut()
        .resource_mut::<TerrainPreparedSettings>()
        .enabled = true;
    settle_prepared(app, 2);
    let before = app.world().resource::<TerrainPreparedStats>().snapshot();
    assert_eq!(before.pages, 2);
    assert_eq!(before.builds, 2);
    let (material, control) = app
        .world()
        .resource::<Assets<TerrainMaterial>>()
        .iter()
        .find(|(_, m)| m.prepared)
        .map(|(id, m)| (id, m.weights.clone()))
        .unwrap();
    let controls = Arc::new(Mutex::new(Vec::<u8>::new()));
    let readback = controls.clone();
    let observer = app
        .world_mut()
        .spawn(Readback::texture(control.clone()))
        .observe(move |event: On<ReadbackComplete>| {
            *readback.lock().unwrap() = event.data.clone();
        })
        .id();
    settle_prepared(app, 2);
    let data = controls.lock().unwrap();
    assert!(
        data.len() >= 272 * 1280,
        "prepared control readback must complete: {} bytes",
        data.len()
    );
    // Readback rows include 256-byte alignment. Check every real texel, including the halo.
    for row in data.chunks_exact(1280).take(272) {
        for pixel in row[..272 * 4].chunks_exact(4) {
            let decoded = (u16::from(pixel[2]) * 256 + u16::from(pixel[3])) as f32 / 65535.0;
            assert!(
                (decoded - 191.0 / 255.0).abs() < 0.00004,
                "incorrect packed macro: {decoded}"
            );
        }
    }
    drop(data);
    app.world_mut().despawn(observer);
    // Draw-time controls must not cause texture rebuilds.
    for (_, m) in app
        .world_mut()
        .resource_mut::<Assets<TerrainMaterial>>()
        .iter_mut()
    {
        m.settings.macro_scales.w = 0.4;
        m.settings.macro_settings.x = 0.6;
        m.settings.macro_settings.y = 0.0;
    }
    settle_prepared(app, 2);
    assert_eq!(
        app.world()
            .resource::<TerrainPreparedStats>()
            .snapshot()
            .builds,
        before.builds
    );
    app.world_mut()
        .resource_mut::<TerrainPreparedSettings>()
        .enabled = false;
    settle_prepared(app, 0);
    assert!(
        app.world()
            .resource::<Assets<TerrainMaterial>>()
            .iter()
            .all(|(_, m)| m.weights == m.source_weights
                && m.base_color_array == m.source_base_color_array)
    );
    app.world_mut()
        .resource_mut::<TerrainPreparedSettings>()
        .enabled = true;
    settle_prepared(app, 2);
    assert_eq!(
        app.world()
            .resource::<TerrainPreparedStats>()
            .snapshot()
            .builds,
        before.builds,
        "toggle must reuse resident controls"
    );
    // Editing a source image invalidates both pages, even though its asset ID is unchanged.
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .get_mut(&macro_image)
        .unwrap()
        .data
        .as_mut()
        .unwrap()[0] = 64;
    settle_prepared(app, 2);
    assert_eq!(
        app.world()
            .resource::<TerrainPreparedStats>()
            .snapshot()
            .builds,
        before.builds + 2
    );
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&control)
            .is_none(),
        "stale control must be reclaimed"
    );
    app.world_mut()
        .resource_mut::<Assets<TerrainMaterial>>()
        .remove(material);
    settle_prepared(app, 1);
    assert_eq!(
        app.world()
            .resource::<TerrainPreparedStats>()
            .snapshot()
            .pages,
        1
    );
    app.world_mut()
        .resource_mut::<TerrainPreparedSettings>()
        .max_bytes = 0;
    settle_prepared(app, 0);
    assert_eq!(
        app.world()
            .resource::<TerrainPreparedStats>()
            .snapshot()
            .bytes,
        0
    );
    eprintln!(
        "prepared terrain: lit/unlit 4x MSAA, packed macro, reuse, toggle, source edit, eviction and budget passed"
    );
}

fn settle_prepared(app: &mut App, active: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    let mut settled = 0;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "prepared terrain timed out: {:?}",
            app.world().resource::<TerrainPreparedStats>().snapshot()
        );
        app.update();
        let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
        for pipeline in cache.pipelines() {
            if let CachedPipelineState::Err(error) = &pipeline.state {
                panic!("prepared pipeline: {error}");
            }
        }
        let ready = app
            .world()
            .resource::<TerrainPreparedStats>()
            .snapshot()
            .active
            == active
            && cache.waiting_pipelines().count() == 0;
        settled = if ready { settled + 1 } else { 0 };
        if settled >= 20 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn settled_pixels(app: &mut App, cached: bool) -> Vec<u8> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut settled = 0;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "terrain readback timed out"
        );
        app.update();
        let stats = app.world().resource::<TerrainCacheStats>().snapshot();
        let pending = app
            .sub_app(RenderApp)
            .world()
            .resource::<PipelineCache>()
            .waiting_pipelines()
            .count();
        let ready = pending == 0
            && if cached {
                stats.ready == 4
            } else {
                stats.tables == 0
            };
        settled = if ready { settled + 1 } else { 0 };
        if settled >= 20 {
            let pixels = app.world().resource::<Pixels>().0.lock().unwrap().clone();
            if !pixels.is_empty() {
                return pixels;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    pixels: Res<Pixels>,
) {
    let mut target = Image::new_target_texture(128, 128, TextureFormat::Bgra8UnormSrgb, None);
    target.texture_descriptor.usage |= bevy::render::render_resource::TextureUsages::COPY_SRC;
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
        Transform::from_xyz(0.0, 5.0, 0.1).looking_at(Vec3::ZERO, Vec3::Y),
        Msaa::Off,
        DepthPrepass,
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_rotation(Quat::from_rotation_x(-1.0)),
    ));
    let make_image = |layers| {
        let data: Vec<_> = (0..layers)
            .flat_map(|layer| {
                (0..256).flat_map(move |i| {
                    let x = i % 16;
                    let y = i / 16;
                    [
                        (40 + (x * 11 + layer * 21) % 180) as u8,
                        (40 + (y * 9 + layer * 33) % 180) as u8,
                        (100 + (x * y + layer * 29) % 140) as u8,
                        220,
                    ]
                })
            })
            .collect();
        let mut image = Image::new(
            Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: layers,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::default(),
        );
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            ..default()
        });
        image
    };
    let weights = images.add(make_image(1));
    let array = images.add(make_image(2));
    let mut plane = Plane3d::default().mesh().size(1.5, 1.5).build();
    plane.generate_tangents().unwrap();
    let mesh = meshes.add(plane);
    for (i, mode) in [
        TerrainShadingMode::Production,
        TerrainShadingMode::SurfaceUnlit,
        TerrainShadingMode::SingleTexture,
        TerrainShadingMode::Flat,
    ]
    .into_iter()
    .enumerate()
    {
        let material = materials.add(TerrainMaterial {
            shading_mode: mode,
            stochastic_cached: false,
            prepared: false,
            source_weights: weights.clone(),
            source_base_color_array: array.clone(),
            stochastic_cache: Handle::default(),
            settings: TerrainMaterialUniform {
                cache_origins: Vec4::ZERO,
                cache_size: UVec4::ZERO,
                chunk_minimum: Vec2::splat(-2.0),
                chunk_extent: Vec2::splat(4.0),
                surface_layers: Vec4::new(0.0, 1.0, 2.0, 0.0),
                tile_sizes: Vec4::ONE,
                normal_settings: Vec4::ONE,
                roughness_ranges: Vec4::new(0.1, 0.9, 0.1, 0.9),
                macro_scales: Vec4::ONE,
                macro_settings: Vec4::ONE,
            },
            weights: weights.clone(),
            base_color_array: array.clone(),
            normal_material_array: array.clone(),
            macro_variation: weights.clone(),
        });
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_xyz((i % 2) as f32 * 1.8 - 0.9, 0.0, (i / 2) as f32 * 1.8 - 0.9),
        ));
    }
}
