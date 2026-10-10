//! Explicit GPU smoke test: compile and draw every terrain diagnostic with real Bevy imports.
use super::*;
use bevy::image::ImageAddressMode;
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
        if consecutive_ready >= 5 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Repeat rates, macro variation and MSAA change at draw time; a stationary view must
    // keep its pixels across frames, including on negative coordinates.
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
        let first = settled_pixels(&mut app);
        assert!(
            first.as_chunks::<4>().0.iter().any(|p| p[0] != p[1]),
            "test must render coloured terrain"
        );
        assert_eq!(
            first,
            settled_pixels(&mut app),
            "variant {variant} is not stable"
        );
    }
}

fn settled_pixels(app: &mut App) -> Vec<u8> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut settled = 0;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "terrain readback timed out"
        );
        app.update();
        let pending = app
            .sub_app(RenderApp)
            .world()
            .resource::<PipelineCache>()
            .waiting_pipelines()
            .count();
        settled = if pending == 0 { settled + 1 } else { 0 };
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
            weights: weights.clone(),
            base_color_array: array.clone(),
            normal_material_array: array.clone(),
            macro_variation: weights.clone(),
            ..TerrainMaterial::for_tests(TerrainMaterialUniform {
                chunk_minimum: Vec2::splat(-2.0),
                chunk_extent: Vec2::splat(4.0),
                surface_layers: Vec4::new(0.0, 1.0, 2.0, 0.0),
                tile_sizes: Vec4::ONE,
                normal_settings: Vec4::ONE,
                roughness_ranges: Vec4::new(0.1, 0.9, 0.1, 0.9),
                macro_scales: Vec4::ONE,
                macro_settings: Vec4::ONE,
            })
        });
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_xyz((i % 2) as f32 * 1.8 - 0.9, 0.0, (i / 2) as f32 * 1.8 - 0.9),
        ));
    }
}
