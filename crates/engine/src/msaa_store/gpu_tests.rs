use super::*;
use bevy::{
    app::PluginsState,
    camera::{RenderTarget, visibility::RenderLayers},
    ecs::system::RunSystemOnce,
    render::{
        RenderPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{
            CachedPipelineState, Extent3d, TextureDimension, TextureFormat, TextureUsages,
        },
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::sync::{Arc, Mutex};

#[derive(Resource, Default)]
struct ProbeGizmos(bool);

type PixelPair = Arc<Mutex<[(u64, Vec<u8>); 2]>>;

#[test]
#[ignore = "requires a native GPU adapter"]
fn resolve_only_matches_stored_color_and_preserves_later_consumers() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
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
    .add_plugins(MsaaColorStorePlugin)
    .init_resource::<ProbeGizmos>()
    .add_systems(Update, |probe: Res<ProbeGizmos>, mut gizmos: Gizmos| {
        if probe.0 {
            gizmos.line(
                Vec3::new(-1.2, 0.8, 0.4),
                Vec3::new(1.2, -0.5, 0.4),
                Color::srgb(0.1, 1.0, 0.2),
            );
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    while app.plugins_state() != PluginsState::Ready {
        assert!(
            std::time::Instant::now() < deadline,
            "renderer startup timed out"
        );
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let pixels: PixelPair = default();
    let mut targets = Vec::new();
    let mut cameras = Vec::new();
    let pose = Transform::from_xyz(0.0, 1.3, 4.0).looking_at(Vec3::ZERO, Vec3::Y);
    for (i, policy) in [
        MsaaColorStorePolicy::Preserve,
        MsaaColorStorePolicy::Automatic,
    ]
    .into_iter()
    .enumerate()
    {
        let mut image = Image::new_target_texture(256, 192, TextureFormat::Rgba8UnormSrgb, None);
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera::default(),
                RenderTarget::Image(target.clone().into()),
                Msaa::Sample4,
                policy,
                pose,
            ))
            .id();
        // UI renders into resolved color after the opaque pass.
        app.world_mut().spawn((
            Node {
                width: px(31),
                height: px(19),
                ..default()
            },
            BackgroundColor(Color::srgb(0.8, 0.2, 0.1)),
            UiTargetCamera(camera),
        ));
        let result = pixels.clone();
        app.world_mut()
            .spawn(Readback::texture(target.clone()))
            .observe(move |event: On<ReadbackComplete>| {
                let mut pair = result.lock().unwrap();
                pair[i].0 += 1;
                pair[i].1 = event.data.clone();
            });
        targets.push(target);
        cameras.push(camera);
    }
    let cube = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(1.3, 1.3, 1.0));
    let blue = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::srgb(0.12, 0.35, 0.8),
            unlit: true,
            ..default()
        });
    app.world_mut().spawn((
        Mesh3d(cube.clone()),
        MeshMaterial3d(blue),
        Transform::from_xyz(-0.45, 0.0, 0.0).with_rotation(Quat::from_rotation_y(0.3)),
    ));
    let mut checker_data = Vec::new();
    for y in 0..8 {
        for x in 0..8 {
            checker_data.extend_from_slice(&[
                240,
                240,
                140,
                if (x + y) % 2 == 0 { 255 } else { 0 },
            ]);
        }
    }
    let checker = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new(
            Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            checker_data,
            TextureFormat::Rgba8UnormSrgb,
            default(),
        ));
    let cutout = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color_texture: Some(checker),
            alpha_mode: AlphaMode::Mask(0.5),
            unlit: true,
            ..default()
        });
    app.world_mut().spawn((
        Mesh3d(cube.clone()),
        MeshMaterial3d(cutout),
        Transform::from_xyz(0.75, 0.0, -0.6),
    ));
    let translucent = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::srgba(0.9, 0.1, 0.4, 0.45),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        });
    let transparent = app
        .world_mut()
        .spawn((
            Mesh3d(cube),
            MeshMaterial3d(translucent.clone()),
            Transform::from_xyz(0.3, 0.0, 0.7),
            Visibility::Hidden,
        ))
        .id();

    let opaque = compare(&mut app, &pixels, "opaque, alpha cutout and UI", true);
    assert!(
        opaque.chunks_exact(4).map(|p| p[2]).max().unwrap() > 100,
        "test must draw visible geometry"
    );
    // The shared graph slot keeps Bevy's public before/after anchors functional.
    let render_world = app.sub_app_mut(RenderApp).world_mut();
    let schedule = render_world.resource::<Schedules>().get(Core3d).unwrap();
    let systems: Vec<_> = schedule
        .systems()
        .unwrap()
        .map(|(_, s)| s.system_type())
        .collect();
    let opaque_type = IntoSystem::into_system(opaque_pass).system_type();
    let transparent_type =
        IntoSystem::into_system(bevy::core_pipeline::core_3d::main_transparent_pass_3d)
            .system_type();
    let transmission_type =
        IntoSystem::into_system(bevy::pbr::main_transmissive_pass_3d).system_type();
    let opaque_index = systems.iter().position(|t| *t == opaque_type).unwrap();
    let transparent_index = systems.iter().position(|t| *t == transparent_type).unwrap();
    let transmission_index = systems
        .iter()
        .position(|t| *t == transmission_type)
        .unwrap();
    assert!(opaque_index < transmission_index && transmission_index < transparent_index);

    for &camera in &cameras {
        app.world_mut()
            .entity_mut(camera)
            .insert(bevy::core_pipeline::prepass::DepthPrepass);
    }
    compare(&mut app, &pixels, "depth prepass", true);
    for &camera in &cameras {
        app.world_mut()
            .entity_mut(camera)
            .remove::<bevy::core_pipeline::prepass::DepthPrepass>();
    }

    app.world_mut()
        .entity_mut(transparent)
        .insert(Visibility::Visible);
    let with_transparency = compare(&mut app, &pixels, "transparency", false);
    assert_ne!(
        opaque, with_transparency,
        "transparent probe must affect the image"
    );
    {
        let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
        let mut material = materials.get_mut(&translucent).unwrap();
        material.alpha_mode = AlphaMode::Opaque;
        material.specular_transmission = 0.6;
        material.unlit = false;
    }
    compare(&mut app, &pixels, "screen-space transmission", false);
    app.world_mut()
        .entity_mut(transparent)
        .insert(Visibility::Hidden);
    app.world_mut().resource_mut::<ProbeGizmos>().0 = true;
    let with_gizmos = compare(&mut app, &pixels, "gizmos", false);
    assert_ne!(opaque, with_gizmos, "gizmo probe must affect the image");
    app.world_mut().resource_mut::<ProbeGizmos>().0 = false;
    for &camera in &cameras {
        app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = Some(Viewport {
            physical_position: UVec2::splat(16),
            physical_size: UVec2::new(224, 160),
            ..default()
        });
    }
    compare(&mut app, &pixels, "sub-viewport", true);
    for &camera in &cameras {
        app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = None;
        app.world_mut().entity_mut(camera).insert(Msaa::Off);
    }
    compare(&mut app, &pixels, "single sample", false);
    for &camera in &cameras {
        app.world_mut().entity_mut(camera).insert(Msaa::Sample4);
    }
    for target in &targets {
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut(target)
            .unwrap()
            .resize(Extent3d {
                width: 512,
                height: 256,
                depth_or_array_layers: 1,
            });
    }
    compare(&mut app, &pixels, "resized 4x target", true);
    // Empty later cameras still load/resolve shared multisample color.
    for (target, policy) in targets.iter().zip([
        MsaaColorStorePolicy::Preserve,
        MsaaColorStorePolicy::Automatic,
    ]) {
        app.world_mut().spawn((
            Camera3d::default(),
            Camera {
                order: 1,
                clear_color: ClearColorConfig::None,
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            Msaa::Sample4,
            policy,
            pose,
            RenderLayers::layer(31),
        ));
    }
    compare(
        &mut app,
        &pixels,
        "shared target and empty later camera",
        false,
    );
}

fn compare(app: &mut App, pixels: &PixelPair, name: &str, expected_discard: bool) -> Vec<u8> {
    let previous = {
        let pair = pixels.lock().unwrap();
        [pair[0].0, pair[1].0]
    };
    // Settle extraction, pipeline creation, and asynchronous readback after edits.
    for _ in 0..24 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    let decisions = app
        .sub_app_mut(RenderApp)
        .world_mut()
        .run_system_once(
            |world: &World,
             views: Query<(Entity, &ExtractedView, &ViewTarget, &MsaaColorStorePolicy)>,
             targets: Query<(Entity, &ViewTarget)>| {
                views
                    .iter()
                    .map(|(entity, view, target, policy)| {
                        (
                            *policy,
                            can_discard_color(world, entity, view, target, Some(policy), &targets),
                        )
                    })
                    .collect::<Vec<_>>()
            },
        )
        .unwrap();
    assert!(decisions.len() >= 2, "{name}: missing render views");
    for (policy, discard) in decisions {
        assert_eq!(
            discard,
            policy == MsaaColorStorePolicy::Automatic && expected_discard,
            "{name}: store decision"
        );
    }
    let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
    for pipeline in cache.pipelines() {
        if let CachedPipelineState::Err(error) = &pipeline.state {
            panic!("{name}: pipeline error {error}");
        }
    }
    let pair = pixels.lock().unwrap();
    assert!(
        (0..2).all(|i| pair[i].0 > previous[i] + 3 && !pair[i].1.is_empty()),
        "{name}: missing fresh readback"
    );
    assert!(
        pair[0].1 == pair[1].1,
        "{name}: resolve-only changed the final image ({} differing bytes)",
        pair[0]
            .1
            .iter()
            .zip(&pair[1].1)
            .filter(|(a, b)| a != b)
            .count()
    );
    eprintln!("MSAA_STORE {name}: identical pixels, discard={expected_discard}");
    pair[0].1.clone()
}
