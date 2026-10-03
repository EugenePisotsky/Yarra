//! Compare actual colour coverage with/without the passes required by Temporal.
//! Compute-only wind tests cannot catch separately compiled vertex depth drift.
use super::*;
use bevy::{
    app::PluginsState,
    asset::AssetPlugin,
    camera::RenderTarget,
    core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass},
    gltf::GltfAssetLabel,
    render::{
        RenderPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{CachedPipelineState, PipelineCache, TextureFormat, TextureUsages},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Pixels = Arc<Mutex<[(u64, Vec<u8>); 2]>>;

#[test]
#[ignore = "requires native GPU and local dead-tree/spruce bundles"]
fn wind_depth_prepass_preserves_colour_coverage() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(tree_gltf_plugin())
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
    .add_plugins((atmosphere::WorldEnvironmentPlugin::game(), TreeWindPlugin));
    let deadline = Instant::now() + Duration::from_secs(90);
    while app.plugins_state() != PluginsState::Ready {
        assert!(Instant::now() < deadline, "renderer startup timed out");
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let pixels: Pixels = default();
    for i in 0..2 {
        let mut image = Image::new_target_texture(512, 512, TextureFormat::Rgba8UnormSrgb, None);
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let mut camera = app.world_mut().spawn((
            Camera3d::default(),
            RenderTarget::Image(target.clone().into()),
            Transform::from_xyz(7., 9., 11.).looking_at(Vec3::new(0., 7., 0.), Vec3::Y),
            Msaa::Off,
        ));
        if i == 1 {
            camera.insert((DepthPrepass, MotionVectorPrepass));
        }
        let result = pixels.clone();
        app.world_mut().spawn(Readback::texture(target)).observe(
            move |event: On<ReadbackComplete>| {
                let mut pair = result.lock().unwrap();
                pair[i].0 += 1;
                pair[i].1 = event.data.clone();
            },
        );
    }
    // Freeze transport at several known phases while keeping the full strong
    // deformation. Both cameras must receive exactly the same animated pose.
    let mut wind = VegetationWind::default();
    wind.externally_driven = true;
    wind.gustiness = 1.;
    app.insert_resource(wind);
    let mut root = None;
    for path in [
        "local/yarra_dead_trees/runtime/dead_spreading/dead_spreading_lod0.gltf",
        "local/yarra_spruces/runtime/spruce_forest/spruce_forest_lod0.gltf",
    ] {
        if let Some(entity) = root.take() {
            app.world_mut().despawn(entity);
        }
        let scene = app
            .world()
            .resource::<AssetServer>()
            .load(GltfAssetLabel::Scene(0).from_asset(path));
        root = Some(
            app.world_mut()
                .spawn((WorldAssetRoot(scene), Transform::default()))
                .id(),
        );
        for (strength, phase) in [(0., 0.), (1.7, 7.), (1.7, 19.)] {
            let mut wind = app.world_mut().resource_mut::<VegetationWind>();
            wind.strength = strength;
            wind.set_phase_seconds(phase);
            let before = pixels.lock().unwrap()[0].0;
            // Wait for scene/material preparation, then settle GPU readbacks.
            let mut ready = 0;
            while ready < 24 {
                assert!(
                    Instant::now() < deadline,
                    "scene/readback timed out: {path}"
                );
                app.update();
                let pipeline_ready = app.sub_app(RenderApp).world().resource::<PipelineCache>();
                for p in pipeline_ready.pipelines() {
                    if let CachedPipelineState::Err(error) = &p.state {
                        panic!("tree pipeline error: {error}");
                    }
                }
                let mesh_count = app.world_mut().query::<&Mesh3d>().iter(app.world()).count();
                ready = if mesh_count == 2 { ready + 1 } else { 0 };
                std::thread::sleep(Duration::from_millis(12));
            }
            let pair = pixels.lock().unwrap();
            assert!(
                pair.iter()
                    .all(|(n, p)| *n > before + 3 && p.len() == 512 * 512 * 4)
            );
            let a = &pair[0].1;
            let b = &pair[1].1;
            assert!(
                a.chunks_exact(4).filter(|p| p[0] > 10 || p[1] > 10).count() > 1000,
                "test must draw a visible tree"
            );
            // Allow tiny rounding at silhouettes, not black missing facets.
            let damaged = a
                .chunks_exact(4)
                .zip(b.chunks_exact(4))
                .filter(|(x, y)| x[..3].iter().zip(&y[..3]).any(|(x, y)| x.abs_diff(*y) > 8))
                .count();
            eprintln!(
                "WIND_DEPTH {path} strength={strength} phase={phase}: {damaged} differing pixels"
            );
            assert!(
                damaged < 32,
                "depth prepass changed colour coverage: {damaged} pixels, {path}, strength={strength}, phase={phase}"
            );
        }
    }
}
