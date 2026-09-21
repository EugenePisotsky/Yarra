use super::*;
use bevy::{
    app::PluginsState,
    camera::{ClearColorConfig, RenderTarget},
    post_process::bloom::Bloom,
    render::{
        RenderPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        pipelined_rendering::PipelinedRenderingPlugin,
        view::ColorGrading,
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[test]
#[ignore = "native GPU: compares direct output to Bevy tone map + blit, including HDR bloom/grades/resize"]
fn direct_output_matches_standard_postprocessing() {
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
    .add_plugins(crate::UpscalingPlugin);
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let pixels = Arc::new(Mutex::new([Vec::<u8>::new(), Vec::<u8>::new()]));
    let mut cameras = Vec::new();
    let mut targets = Vec::new();
    for index in 0..2 {
        let mut image = Image::new_target_texture(64, 64, TextureFormat::Rgba8UnormSrgb, None);
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let output = pixels.clone();
        app.world_mut()
            .spawn(Readback::texture(target.clone()))
            .observe(move |event: On<ReadbackComplete>| {
                output.lock().unwrap()[index] = event.data.clone();
            });
        let mut camera = app.world_mut().spawn((
            Camera3d::default(),
            Camera {
                order: index as isize,
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            Msaa::Off,
            TemporalView {
                input_size: UVec2::splat(32),
                reset_epoch: 0,
                debug: crate::temporal::TemporalDebug::Bypass,
            },
            Bloom::default(),
        ));
        if index == 1 {
            camera.insert(DirectTonemapOutput);
        }
        cameras.push(camera.id());
        targets.push(target);
    }
    for case in 0..4 {
        let (colour, tone) = match case {
            0 => (Color::linear_rgb(8., 1.2, 0.25), Tonemapping::TonyMcMapface),
            1 => (
                Color::linear_rgb(0.003, 0.012, 0.07),
                Tonemapping::AcesFitted,
            ),
            _ => (Color::linear_rgb(0.4, 0.7, 0.2), Tonemapping::AgX),
        };
        for &camera in &cameras {
            app.world_mut()
                .get_mut::<Camera>(camera)
                .unwrap()
                .clear_color = ClearColorConfig::Custom(colour);
            let mut grade = ColorGrading::default();
            if case > 0 {
                grade.global.exposure = 0.4;
                grade.global.hue = 0.07;
                grade.global.temperature = 0.02;
                grade.midtones.saturation = 0.7;
            }
            app.world_mut()
                .entity_mut(camera)
                .insert((tone, grade, DebandDither::Enabled));
        }
        if case == 2 {
            for target in &targets {
                app.world_mut()
                    .resource_mut::<Assets<Image>>()
                    .get_mut(target)
                    .unwrap()
                    .resize(Extent3d {
                        width: 64,
                        height: 33,
                        depth_or_array_layers: 1,
                    });
            }
        }
        if case == 3 {
            app.world_mut()
                .entity_mut(cameras[1])
                .remove::<DirectTonemapOutput>();
        }
        *pixels.lock().unwrap() = default();
        let mut previous = [Vec::new(), Vec::new()];
        let mut stable = 0;
        for frame in 0.. {
            assert!(Instant::now() < deadline, "output comparison timed out");
            app.update();
            let render = app.sub_app(RenderApp).world();
            for pipeline in render.resource::<PipelineCache>().pipelines() {
                if let CachedPipelineState::Err(e) = &pipeline.state
                    && !matches!(
                        e,
                        bevy::shader::ShaderCacheError::ShaderNotLoaded(_)
                            | bevy::shader::ShaderCacheError::ShaderImportNotYetAvailable
                    )
                {
                    panic!("output pipeline: {e}");
                }
            }
            let result = pixels.lock().unwrap().clone();
            let expected_len = 64 * (if case < 2 { 64 } else { 33 }) * 4;
            if result.iter().all(|v| v.len() == expected_len) && result == previous {
                stable += 1;
            } else {
                stable = 0;
            }
            previous = result;
            if frame >= 20 && stable >= 5 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let render = app.sub_app_mut(RenderApp).world_mut();
        assert_eq!(
            render.query::<&Ready>().iter(render).count(),
            usize::from(case < 3),
            "fused path activation/fallback"
        );
        let [reference, optimized] = previous;
        assert!(
            reference.chunks_exact(4).any(|p| p[0..3] != [0, 0, 0]),
            "no scene colour"
        );
        let max_error = reference
            .iter()
            .zip(&optimized)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        eprintln!("output case={case} max_channel_error={max_error}/255");
        assert!(max_error <= 1, "direct output changed colour/grade/bloom");
    }
}
