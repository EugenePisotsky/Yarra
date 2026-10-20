//! Explicit GPU smoke test: compile and draw the clipmap material with real Bevy imports, on a
//! one-node patch as the editor's preview draws it.
use super::*;

use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::{
    app::PluginsState,
    asset::RenderAssetUsages,
    camera::RenderTarget,
    core_pipeline::prepass::DepthPrepass,
    render::{
        RenderApp, RenderPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{
            CachedPipelineState, Extent3d, PipelineCache, PipelineDescriptor, TextureDimension,
            TextureFormat,
        },
        storage::ShaderBuffer,
    },
    window::ExitCondition,
    winit::WinitPlugin,
};

use clipmap::{
    ClipmapGround, ClipmapPatch, ClipmapSurface, ClipmapTerrainMaterial, ClipmapUploads,
};
use std::sync::{Arc, Mutex};

#[derive(Resource, Default, Clone)]
struct Pixels(Arc<Mutex<Vec<u8>>>);

#[derive(Resource)]
struct TestPatch(ClipmapPatch);

/// Where the camera looks down from onto the 4 m patch.
const EYE: Vec3 = Vec3::new(2.0, 5.0, 2.1);

#[test]
#[ignore = "requires a native GPU adapter; run explicitly on a development machine"]
fn clipmap_material_compiles_and_renders() {
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
    .add_systems(Startup, setup)
    .add_systems(Update, follow);
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
    let shader: Handle<Shader> = app
        .world()
        .resource::<AssetServer>()
        .load("shaders/terrain/clipmap.wesl");
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

        consecutive_ready = if ready >= 1 { consecutive_ready + 1 } else { 0 };

        // Keep drawing after all variants compile to also exercise their bind groups.
        if consecutive_ready >= 5 {
            break;
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Macro variation and MSAA change at draw time; a stationary view must keep its pixels
    // across frames.
    for variant in 0..3 {
        if variant == 1 {
            *app.world_mut().resource_mut::<TerrainMacroVariation>() =
                TerrainMacroVariation::Disabled;
        }

        if variant == 2 {
            let world = app.world_mut();
            let mut cameras = world.query_filtered::<&mut Msaa, With<Camera3d>>();

            for mut msaa in cameras.iter_mut(world) {
                *msaa = Msaa::Sample4;
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

/// The patch's frame follows the camera, as the preview's does.
fn follow(patch: Res<TestPatch>, mut uploads: ResMut<ClipmapUploads>) {
    patch.0.follow(EYE, &mut uploads);
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut materials: ResMut<Assets<ClipmapTerrainMaterial>>,
    mut uploads: ResMut<ClipmapUploads>,
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
        Transform::from_translation(EYE).looking_at(Vec3::new(2.0, 0.0, 2.0), Vec3::Y),
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
    let surface = ClipmapSurface {
        texture: Vec4::new(1.0, 1.0, 1.0, 1.0),
        material: Vec4::new(0.3, 0.9, 0.0, 0.0),
        grass: Vec4::ZERO,
    };
    let ground = ClipmapGround {
        surfaces: vec![surface; 2],
        base_color: images.add(make_image(2)),
        normal_material: images.add(make_image(2)),
        macro_variation: images.add(make_image(1)),
        macro_scales: [8.0, 32.0, 128.0],
        macro_contrast: 1.0,
        macro_albedo_strength: 0.5,
    };

    // Gentle relief, two surfaces mixed.
    let n = 33;
    let heights: Vec<f32> = (0..n * n)
        .map(|i| 0.1 * ((i % n) as f32 * 0.4).sin() * ((i / n) as f32 * 0.3).cos())
        .collect();
    let field = world::TerrainHeightfield::from_heights(33, &heights, -0.1, 0.1, 4.0).unwrap();
    let key = world::TerrainNodeKey::leaf(world::WorldSpaceId(1), world::CellCoord::ZERO);
    let node = world::TerrainNode::leaf(key, &field, 33).unwrap();
    let control = vec![
        world::pack_control([(0, 0.6), (1, 0.4), (0, 0.0)]);
        world::CONTROL_SAMPLES * world::CONTROL_SAMPLES
    ];
    let patch = ClipmapPatch::new(
        &node,
        4.0,
        control,
        None,
        ground,
        &mut uploads,
        clipmap::PatchAssets {
            images: &mut images,
            buffers: &mut buffers,
            materials: &mut materials,
        },
    )
    .unwrap();
    commands.spawn(patch.bundle(&mut meshes));
    commands.insert_resource(TestPatch(patch));
}
