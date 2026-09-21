//! Compare the same final grass view reached while stationary and while strafing.
use super::*;
use upscaling::temporal::{TemporalDebug, TemporalFrame, TemporalView};

#[test]
#[ignore = "requires native MetalFX; bounded grass reconstruction image comparison"]
fn temporal_grass_detail_during_strafe() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("YARRA_TEMPORAL_IMAGES")
            .expect("set YARRA_TEMPORAL_IMAGES to the diagnostic output directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    let mut app = test_app();
    app.update();
    let camera = app
        .world_mut()
        .query_filtered::<Entity, With<Camera3d>>()
        .single(app.world())
        .unwrap();
    let endpoint = Transform::from_xyz(12., 2.8, 18.).looking_at(Vec3::new(12., 0.5, 8.), Vec3::Y);
    app.world_mut().entity_mut(camera).insert((
        endpoint,
        TemporalView {
            input_size: UVec2::splat(
                std::env::var("YARRA_TEMPORAL_INPUT_PIXELS")
                    .map_or(128, |v| v.parse::<u32>().unwrap().clamp(86, 256)),
            ),
            reset_epoch: 0,
            debug: TemporalDebug::Off,
        },
    ));
    app.world_mut().resource_mut::<VegetationWind>().enabled = false;
    settled_pixels(&mut app);
    for prepared in [true, false] {
        app.world_mut()
            .resource_mut::<VegetationBladePreparation>()
            .enabled = prepared;
        *app.world_mut().get_mut::<Transform>(camera).unwrap() = endpoint;
        settled_pixels(&mut app);
        save_pixels(&mut app, &directory.join(format!("static-{prepared}.ppm")));
        let stationary_instances = generated_instances(&app);
        // At 60 Hz this represents a 6 m/s camera strafe. Finish at precisely the
        // same pose as the stationary reference, with no wind or lighting changes.
        for frame in 0..48 {
            let mut pose = endpoint;
            pose.translation.x -= (47 - frame) as f32 * 0.1;
            *app.world_mut().get_mut::<Transform>(camera).unwrap() = pose;
            if frame == 0 {
                app.world_mut()
                    .get_mut::<TemporalView>(camera)
                    .unwrap()
                    .reset_epoch += 1;
            }
            app.update();
            if frame > 0 {
                let world = app.sub_app_mut(RenderApp).world_mut();
                let temporal = world.query::<&TemporalFrame>().single(world).unwrap();
                assert!(!temporal.reset, "ordinary strafe reset the history");
                assert_ne!(temporal.jitter, Vec2::ZERO, "native Temporal fell back");
            }
        }
        save_pixels(&mut app, &directory.join(format!("strafe-{prepared}.ppm")));
        assert_eq!(
            stationary_instances,
            generated_instances(&app),
            "camera path changed the final grass population"
        );
    }
}

fn save_pixels(app: &mut App, path: &std::path::Path) {
    use bevy::{
        camera::RenderTarget,
        render::{render_asset::RenderAssets, texture::GpuImage},
    };
    let handle = app
        .world_mut()
        .query_filtered::<&RenderTarget, With<Camera3d>>()
        .single(app.world())
        .unwrap();
    let RenderTarget::Image(handle) = handle else {
        panic!("expected image target")
    };
    let id = handle.handle.id();
    let world = app.sub_app(RenderApp).world();
    let image = world.resource::<RenderAssets<GpuImage>>().get(id).unwrap();
    let bytes = read_temporal_texture(world, &image.texture);
    let mut ppm = b"P6\n256 256\n255\n".to_vec();
    for pixel in bytes.chunks_exact(4) {
        ppm.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
    }
    std::fs::write(path, ppm).unwrap();
    println!("TEMPORAL_GRASS_IMAGE {}", path.display());
}
