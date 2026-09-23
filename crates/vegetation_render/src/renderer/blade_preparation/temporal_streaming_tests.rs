//! Residency churn must not discard unrelated pixels' temporal history.
use super::*;
use bevy::render::view::ViewDepthTexture;
use upscaling::temporal::{TemporalDebug, TemporalFrame, TemporalMotionTarget, TemporalView};

#[test]
#[ignore = "requires native MetalFX; streams offscreen pages while checking history and motion"]
fn temporal_offscreen_grass_streaming_preserves_history() {
    let mut app = test_app();
    app.update();
    let camera = app
        .world_mut()
        .query_filtered::<Entity, With<Camera3d>>()
        .single(app.world())
        .unwrap();
    app.world_mut().entity_mut(camera).insert(TemporalView {
        input_size: UVec2::splat(128),
        reset_epoch: 0,
        debug: TemporalDebug::Off,
    });
    {
        let mut c = app.world_mut().get_mut::<Camera3d>(camera).unwrap();
        c.depth_texture_usages =
            (TextureUsages::from(c.depth_texture_usages) | TextureUsages::COPY_SRC).into();
    }
    app.world_mut().resource_mut::<VegetationWind>().enabled = false;
    let baseline = app
        .world()
        .resource::<VegetationSceneState>()
        .scene()
        .clone();
    let mut resets = 0;
    for prepared in [true, false] {
        app.world_mut()
            .resource_mut::<VegetationBladePreparation>()
            .enabled = prepared;
        app.world_mut().resource_mut::<VegetationWind>().enabled = false;
        settled_pixels(&mut app);
        assert!(snapshot(&app).emitted_instances.iter().sum::<u32>() > 100);
        for step in 0..12 {
            // Loading a far page at the front also changes packed source indices.
            // No visible page, seed, species, or ground sample changes.
            let mut scene = baseline.clone();
            if step % 2 == 0 {
                let mut offscreen = scene.pages[0].clone();
                offscreen.origin_xz[0] -= 4096.;
                scene.pages.insert(0, offscreen);
            }
            app.world_mut()
                .resource_mut::<VegetationSceneState>()
                .replace(scene)
                .unwrap();
            if step >= 4 {
                app.world_mut()
                    .get_mut::<Transform>(camera)
                    .unwrap()
                    .translation
                    .x += 0.025;
            }
            if step >= 8 {
                let mut wind = app.world_mut().resource_mut::<VegetationWind>();
                wind.enabled = true;
                wind.phase_seconds += 1. / 60.;
            }
            app.update();
            let world = app.sub_app_mut(RenderApp).world_mut();
            let (frame, motion) = world
                .query::<(&TemporalFrame, &TemporalMotionTarget)>()
                .single(world)
                .unwrap();
            assert_ne!(
                frame.jitter,
                Vec2::ZERO,
                "requires native Temporal, not fallback"
            );
            resets += usize::from(frame.reset);
            let motion = read_temporal_texture(world, &motion.texture);
            let speed = (0..128)
                .flat_map(|y| (0..128).map(move |x| (y * 256 + x) * 4))
                .map(|i| {
                    let v = Vec2::new(
                        temporal_half(&motion[i..i + 2]),
                        temporal_half(&motion[i + 2..i + 4]),
                    );
                    assert!(v.is_finite());
                    v.length()
                })
                .fold(0_f32, f32::max);
            println!(
                "GRASS_STREAM_HISTORY prepared={prepared} step={step} reset={} max_motion_uv={speed:.6}",
                frame.reset
            );
            if !frame.reset {
                if step < 4 {
                    assert!(
                        speed < 0.0001,
                        "stationary roots acquired motion after repacking"
                    );
                } else {
                    assert!(speed > 0.0001, "streaming lost camera/wind motion");
                }
                if (4..8).contains(&step) {
                    assert_camera_motion(world);
                }
            }
        }
        app.world_mut().resource_mut::<VegetationWind>().enabled = false;
        settled_pixels(&mut app);
        // Empty map edges and the grass toggle are local visibility changes too.
        for empty_scene in [true, false] {
            for enabled in [false, true, false, true] {
                if empty_scene {
                    let mut scene = baseline.clone();
                    if !enabled {
                        scene.pages.clear();
                    }
                    app.world_mut()
                        .resource_mut::<VegetationSceneState>()
                        .replace(scene)
                        .unwrap();
                } else {
                    app.world_mut()
                        .resource_mut::<VegetationDebugSettings>()
                        .profile_mode = if enabled {
                        VegetationProfileMode::Full
                    } else {
                        VegetationProfileMode::Disabled
                    };
                }
                app.world_mut()
                    .get_mut::<Transform>(camera)
                    .unwrap()
                    .translation
                    .x += 0.025;
                app.update();
                let world = app.sub_app_mut(RenderApp).world_mut();
                assert!(
                    !world.query::<&TemporalFrame>().single(world).unwrap().reset,
                    "grass visibility changes must retain view history"
                );
                if enabled {
                    assert_camera_motion(world);
                }
            }
        }
    }
    assert_eq!(
        resets, 0,
        "offscreen residency updates reset the whole view"
    );
    // Genuine camera cuts must retain their explicit invalidation behavior.
    app.world_mut()
        .get_mut::<TemporalView>(camera)
        .unwrap()
        .reset_epoch += 1;
    app.update();
    let world = app.sub_app_mut(RenderApp).world_mut();
    assert!(world.query::<&TemporalFrame>().single(world).unwrap().reset);
    app.update();
    let mut edited = baseline.clone();
    edited.catalog.species[0].material.root_color[0] *= 0.5;
    app.world_mut()
        .resource_mut::<VegetationSceneState>()
        .replace(edited)
        .unwrap();
    app.update();
    let world = app.sub_app_mut(RenderApp).world_mut();
    assert!(
        world.query::<&TemporalFrame>().single(world).unwrap().reset,
        "catalog edits still invalidate old species/material history"
    );
    app.update();
    let world = app.sub_app_mut(RenderApp).world_mut();
    assert!(!world.query::<&TemporalFrame>().single(world).unwrap().reset);
}

fn assert_camera_motion(world: &mut World) {
    let (frame, motion, depth) = world
        .query::<(&TemporalFrame, &TemporalMotionTarget, &ViewDepthTexture)>()
        .single(world)
        .unwrap();
    let motion = read_temporal_texture(world, &motion.texture);
    let depth = read_temporal_texture(world, &depth.texture);
    let previous_from_raster =
        frame.previous_clip_from_world * frame.raster_clip_from_world.inverse();
    let mut errors = Vec::new();
    for y in 1..127 {
        for x in 1..127 {
            let i = (y * 256 + x) * 4;
            let d = f32::from_le_bytes(depth[i..i + 4].try_into().unwrap());
            if d <= 0. {
                continue;
            }
            let pixel = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let uv = pixel / 128.;
            let previous = previous_from_raster * Vec4::new(uv.x * 2. - 1., 1. - uv.y * 2., d, 1.);
            let previous_uv = previous.xy() / previous.w * Vec2::new(0.5, -0.5) + Vec2::splat(0.5);
            let expected = (pixel + frame.jitter) / 128. - previous_uv;
            let actual = Vec2::new(
                temporal_half(&motion[i..i + 2]),
                temporal_half(&motion[i + 2..i + 4]),
            );
            errors.push((actual - expected).length() * 128.);
        }
    }
    assert!(errors.len() > 100, "probe must contain visible grass");
    errors.sort_by(f32::total_cmp);
    let p95 = errors[errors.len() * 95 / 100];
    println!(
        "GRASS_STREAM_REPROJECTION pixels={} p95_error_px={p95:.6}",
        errors.len()
    );
    assert!(
        p95 < 0.1,
        "repacking or re-enabling broke camera motion: {p95}px"
    );
}
