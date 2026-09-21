use super::*;
use world::{TerrainCompositeMip, TerrainNodeKey, WorldSpaceId};

fn tile() -> TerrainComposite {
    TerrainComposite {
        key: TerrainMaterialKey(TerrainNodeKey::leaf(
            WorldSpaceId(1),
            CellCoord { x: -1, z: 0 },
        )),
        fingerprint: [7; 32],
        mips: (0..TERRAIN_COMPOSITE_MIPS)
            .map(|l| {
                let size = TerrainComposite::mip_size(l);
                TerrainCompositeMip {
                    color: (0..size * size)
                        .flat_map(|i| [80 + (i % size * 140 / size) as u8, 50, 20, 255])
                        .collect(),
                    response: [128, 128, 220, 255].repeat(size * size),
                }
            })
            .collect(),
    }
}

#[test]
fn composite_maps_keep_mips_formats_and_relative_precision() {
    let mut images = Assets::default();
    let source = tile();
    let material =
        TerrainCompositeMaterial::from_composite(source.clone(), CellCoord::ZERO, 32., &mut images)
            .unwrap();
    let color = images.get(material.color.as_ref().unwrap()).unwrap();
    let response = images.get(material.response.as_ref().unwrap()).unwrap();
    assert_eq!(
        color.texture_descriptor.format,
        TextureFormat::Rgba8UnormSrgb
    );
    assert_eq!(
        response.texture_descriptor.format,
        TextureFormat::Rgba8Unorm
    );
    assert_eq!(color.texture_descriptor.mip_level_count, 3);
    assert_eq!(
        color.data.as_ref().unwrap().len() * 2,
        TerrainComposite::gpu_bytes()
    );
    assert_eq!(
        *color.data.as_ref().unwrap(),
        source
            .mips
            .iter()
            .flat_map(|m| m.color.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(color.asset_usage, RenderAssetUsages::RENDER_WORLD);
    let mut source = tile();
    source.key.0.x = -2_000_000_000;
    let mut material = TerrainCompositeMaterial::from_composite(
        source,
        CellCoord {
            x: -2_000_000_001,
            z: 0,
        },
        32.,
        &mut images,
    )
    .unwrap();
    assert_eq!(material.projection, Vec4::new(32., 0., 32., 1.));
    material.set_origin(
        CellCoord {
            x: -1_999_999_999,
            z: 2,
        },
        32.,
    );
    assert_eq!(material.projection, Vec4::new(-32., -64., 32., 1.));
}

#[derive(Resource, Default, Clone)]
struct Pixels(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

#[test]
#[ignore = "requires a native GPU; checks actual material sampling, light and rebasing"]
fn composite_draws_rebases_and_responds_to_light() {
    use bevy::{
        app::PluginsState,
        camera::RenderTarget,
        render::{
            RenderApp, RenderPlugin,
            gpu_readback::{Readback, ReadbackComplete},
            pipelined_rendering::PipelinedRenderingPlugin,
            render_resource::{CachedPipelineState, PipelineCache, TextureUsages},
        },
        window::ExitCondition,
        winit::WinitPlugin,
    };
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
    .insert_resource(ClearColor(Color::BLACK));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let mut target = Image::new_target_texture(128, 128, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.world_mut()
        .spawn(Readback::texture(target.clone()))
        .observe(|event: On<ReadbackComplete>, pixels: Res<Pixels>| {
            *pixels.0.lock().unwrap() = event.data.clone();
        });
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            RenderTarget::Image(target.into()),
            Transform::from_xyz(-16., 25., 35.).looking_at(Vec3::new(-16., 0., 16.), Vec3::Y),
            Msaa::Off,
            bevy::core_pipeline::prepass::DepthPrepass,
        ))
        .id();
    let light = app
        .world_mut()
        .spawn((
            DirectionalLight {
                illuminance: 20000.,
                ..default()
            },
            Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -1.1, -0.4, 0.)),
        ))
        .id();
    let mut coarse = tile();
    coarse.key.0.level = 2;
    let material = TerrainCompositeMaterial::from_composite(
        coarse,
        CellCoord::ZERO,
        32.,
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
    )
    .unwrap();
    let material = app
        .world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .add(material);
    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[0., 0., 0.], [32., 0., 0.], [0., 0., 32.], [32., 0., 32.]],
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 1., 0.]; 4])
    .with_inserted_indices(Indices::U32(vec![0, 2, 1, 1, 2, 3]));
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    let ground = app
        .world_mut()
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(-32., 0., 0.),
        ))
        .id();
    let settle = |app: &mut App| -> Vec<u8> {
        let mut previous = Vec::new();
        let mut stable = 0;
        let mut frames = 0;
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "composite render timeout"
            );
            app.update();
            for pipeline in app
                .sub_app(RenderApp)
                .world()
                .resource::<PipelineCache>()
                .pipelines()
            {
                if let CachedPipelineState::Err(e) = &pipeline.state {
                    panic!("composite pipeline: {e}");
                }
            }
            let pixels = app.world().resource::<Pixels>().0.lock().unwrap().clone();
            assert!(
                !pixels
                    .chunks_exact(4)
                    .any(|p| p[2] > p[0].saturating_add(10)),
                "a reused texture slot leaked unrelated blue ground"
            );
            let colored = pixels
                .chunks_exact(4)
                .filter(|p| p[0] > p[1].saturating_add(5) && p[1] > p[2])
                .count();
            stable = if colored > 2000 && pixels == previous {
                stable + 1
            } else {
                0
            };
            previous = pixels;
            frames += 1;
            if stable >= 5 && frames > 15 {
                return previous;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let coarse_pixels = settle(&mut app);
    // Temporal upscaling adds another view binding. Fragment-only material
    // buffers must not collide with Metal's vertex/array-size buffer slots.
    app.world_mut()
        .entity_mut(camera)
        .insert(bevy::core_pipeline::prepass::MotionVectorPrepass);
    assert_eq!(coarse_pixels, settle(&mut app));
    let hub = app.world().resource::<atlas::CompositeUploadHub>().clone();
    let atlas = app
        .world_mut()
        .resource_scope(|w, mut images: Mut<Assets<Image>>| {
            atlas::DetailAtlas::new(
                &mut images,
                &mut w.resource_mut::<Assets<ShaderBuffer>>(),
                &hub,
            )
        });
    while !atlas.ready() {
        assert!(std::time::Instant::now() < deadline);
        app.update();
    }
    app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&material)
        .unwrap()
        .set_detail_atlas(&atlas);
    let empty = settle(&mut app);
    assert_eq!(
        coarse_pixels, empty,
        "empty detail cache must draw its coarse fallback"
    );
    let mut fine = tile();
    for mip in &mut fine.mips {
        mip.color = [220, 50, 20, 255].repeat(mip.color.len() / 4);
    }
    let fine_key = fine.key;
    // Force a collision at a distant negative key. The shader must probe past it.
    let collision_key = (-4096..-2)
        .map(|x| TerrainMaterialKey(TerrainNodeKey { x, ..fine_key.0 }))
        .find(|k| atlas::hash(k.0) == atlas::hash(fine_key.0))
        .unwrap();
    let mut collision = fine.clone();
    collision.key = collision_key;
    atlas.submit(
        atlas::table([(collision_key, 1, 1.), (fine_key, 0, 0.)]),
        vec![(1, collision), (0, fine.clone())],
    );
    let zero = settle(&mut app);
    assert_eq!(
        zero, coarse_pixels,
        "an uploaded tile with zero availability must not pop in"
    );
    atlas.submit(
        atlas::table([(collision_key, 1, 1.), (fine_key, 0, 0.5)]),
        vec![],
    );
    let half = settle(&mut app);
    atlas.submit(
        atlas::table([(collision_key, 1, 1.), (fine_key, 0, 1.)]),
        vec![],
    );
    let before = settle(&mut app);
    let red = |p: &[u8]| p.chunks_exact(4).map(|c| c[0] as u64).sum::<u64>();
    assert!(
        red(&zero) < red(&half) && red(&half) < red(&before),
        "fine shading must blend on the same unchanged mesh"
    );
    assert_eq!(
        atlas.uploads(),
        2,
        "fading and stationary rendering must not re-upload tiles"
    );
    let origin = CellCoord { x: -9, z: 7 };
    let shift = Vec3::new(origin.x as f32 * 32., 0., origin.z as f32 * 32.);
    for entity in [ground, camera] {
        app.world_mut()
            .get_mut::<Transform>(entity)
            .unwrap()
            .translation -= shift;
    }
    app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&material)
        .unwrap()
        .set_origin(origin, 32.);
    let rebased = settle(&mut app);
    let errors: Vec<_> = before
        .iter()
        .zip(&rebased)
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    assert!(
        *errors.iter().max().unwrap() <= 2,
        "rebasing shifted the material image"
    );
    // Reusing a physical slot updates pixels and addressing in one transaction.
    // The new tile is outside this mesh, so the old region must return to its parent.
    let mut other = fine.clone();
    other.key.0.x = -2;
    for mip in &mut other.mips {
        mip.color = [20, 50, 220, 255].repeat(mip.color.len() / 4);
    }
    atlas.submit(atlas::table([(other.key, 0, 1.)]), vec![(0, other)]);
    let reused = settle(&mut app);
    assert!(
        reused
            .iter()
            .zip(&coarse_pixels)
            .all(|(a, b)| a.abs_diff(*b) <= 2)
    );
    atlas.submit(atlas::table([(fine_key, 0, 1.)]), vec![(0, fine)]);
    let _ = settle(&mut app);
    app.world_mut()
        .get_mut::<DirectionalLight>(light)
        .unwrap()
        .illuminance = 2000.;
    let darker = settle(&mut app);
    let brightness = |p: &[u8]| {
        p.chunks_exact(4)
            .map(|c| c[0] as u64 + c[1] as u64 + c[2] as u64)
            .sum::<u64>()
    };
    assert!(
        brightness(&darker) * 10 < brightness(&rebased) * 9,
        "lighting must remain dynamic"
    );
}
