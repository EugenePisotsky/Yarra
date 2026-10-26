//! Explicit GPU tests: compile and draw the clipmap material with real Bevy imports, on a one-node
//! patch as the editor's preview draws it, and fill the ground albedo map from it.
use super::*;

use atmosphere::{environment::EnvironmentAssets, ground_albedo::GroundAlbedo};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::{
    app::PluginsState,
    asset::RenderAssetUsages,
    camera::RenderTarget,
    core_pipeline::prepass::DepthPrepass,
    render::{
        RenderApp, RenderPlugin,
        extract_resource::ExtractResourcePlugin,
        gpu_readback::{Readback, ReadbackComplete},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{
            CachedPipelineState, Extent3d, PipelineCache, PipelineDescriptor, TextureDimension,
            TextureFormat, TextureUsages,
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

/// The ground albedo map as read back.
#[derive(Resource, Default, Clone)]
struct AlbedoTexels(Arc<Mutex<Vec<u8>>>);

#[derive(Resource)]
struct TestPatch(ClipmapPatch);

/// Where the camera looks down from onto the 4 m patch.
const EYE: Vec3 = Vec3::new(2.0, 5.0, 2.1);

#[test]
#[ignore = "requires a native GPU adapter; run explicitly on a development machine"]
fn clipmap_material_compiles_and_renders() {
    let mut app = renderer();
    start(&mut app);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
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

    // Macro variation changes at draw time; a stationary view must keep its pixels across
    // frames.
    for variant in 0..2 {
        if variant == 1 {
            *app.world_mut().resource_mut::<TerrainMacroVariation>() =
                TerrainMacroVariation::Disabled;
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

/// The renderer with the terrain's plugin and the test patch under a camera, not yet started.
fn renderer() -> App {
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
    app
}

/// Runs the plugins' startup to the first frame.
fn start(app: &mut App) {
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
}

#[test]
#[ignore = "requires a native GPU adapter; run explicitly on a development machine"]
fn the_ground_albedo_map_takes_the_surfaces_colours() {
    let mut app = renderer();
    app.add_plugins(ExtractResourcePlugin::<EnvironmentAssets>::default())
        .init_resource::<AlbedoTexels>()
        .add_systems(Startup, environment.after(setup));
    start(&mut app);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let read = |app: &App| {
        let world = app.world();
        world
            .get_resource::<GroundAlbedo>()
            .is_some_and(|a| a.filled)
            && !world
                .resource::<AlbedoTexels>()
                .0
                .lock()
                .unwrap()
                .is_empty()
    };

    // Startup places the map on the first frame.
    while !read(&app) {
        assert!(
            std::time::Instant::now() < deadline,
            "the map was not filled"
        );
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Some frames on, so the readback holds a filled map.
    for _ in 0..5 {
        app.update();
    }

    let texels = app
        .world()
        .resource::<AlbedoTexels>()
        .0
        .lock()
        .unwrap()
        .clone();
    let side = atmosphere::ground_albedo::SIZE as usize;

    // The near layer's corner lies 128 m back of the patch's: texel 64 holds its first 2 m.
    let at = (64 * side + 64) * 8;
    let texel: Vec<f32> = texels[at..at + 8]
        .chunks(2)
        .map(|h| half::f16::from_le_bytes([h[0], h[1]]).to_f32())
        .collect();

    // The surfaces' textures have no mips: their colour is the four texels at their centre.
    let centre = |layer: usize| {
        let texel = |x: usize, y: usize| {
            [
                40 + (x * 11 + layer * 21) % 180,
                40 + (y * 9 + layer * 33) % 180,
                100 + (x * y + layer * 29) % 140,
            ]
        };
        let corners = [texel(7, 7), texel(8, 7), texel(7, 8), texel(8, 8)];
        [0, 1, 2].map(|c| corners.iter().map(|t| t[c] as f32).sum::<f32>() / 4.0 / 255.0)
    };
    let (a, b) = (centre(0), centre(1));

    for c in 0..3 {
        let expected = 0.6 * a[c] + 0.4 * b[c];
        assert!(
            (texel[c] - expected).abs() < 0.005,
            "channel {c}: {} against {expected}",
            texel[c]
        );
    }

    assert_eq!(
        texel[3], 0.0,
        "without a sun the ground takes none of its beam"
    );
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
        shade: Vec4::X,
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

    // Under a soft layer, so the shaders' pressed-ground path runs too.
    let node = world::TerrainNode::leaf(key, &field, 33)
        .unwrap()
        .with_soft_layer(world::SoftLayer {
            depths: vec![256; n * n],
            slopes: vec![[0.0; 2]; n * n],
        })
        .unwrap();
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

/// The environment's inputs as the atmosphere makes them, with the ground albedo map readable,
/// centred on the patch and filled from it.
fn environment(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    patch: Res<TestPatch>,
    texels: Res<AlbedoTexels>,
) {
    commands.insert_resource(patch.0.textures().ground_albedo_source());
    let mut map = GroundAlbedo::image();
    map.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let map = images.add(map);
    let open = images.add(Image::default());
    commands.insert_resource(EnvironmentAssets {
        parameters: atmosphere::environment::fallback_parameters(),
        shadows: open.clone(),
        noise: open.clone(),
        cloud_shape: open.clone(),
        cloud_detail: open.clone(),
        shelter: open.clone(),
        forest_shadow: open.clone(),
        terrain_shadow: open.clone(),
        terrain_heights: open.clone(),
        ground_albedo: map.clone(),
        drift: open.clone(),
        mist: open.clone(),
        shore: open,
    });
    let mut albedo = GroundAlbedo::default();
    albedo.follow(Vec2::ZERO, [0.0; 2]);
    commands.insert_resource(albedo);
    let texels = texels.0.clone();
    commands
        .spawn(Readback::texture(map))
        .observe(move |event: On<ReadbackComplete>| {
            *texels.lock().unwrap() = event.data.clone();
        });
}
