//! Native scene/material transition regression, deliberately without temporal AA.
use super::*;
use bevy::{
    app::PluginsState,
    asset::AssetPlugin,
    camera::RenderTarget,
    gltf::GltfAssetLabel,
    pbr::{
        PreparedMaterial, RenderMaterialInstances, SpecializedMaterialPipelineCache,
        SpecializedPrepassMaterialPipelineCache,
    },
    render::{
        RenderPlugin,
        erased_render_asset::ErasedRenderAssets,
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{PipelineCache, TextureFormat},
        sync_world::MainEntity,
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Resource, Clone, Default)]
struct Probe(Arc<Mutex<Snapshot>>);
#[derive(Default, Debug)]
struct Snapshot {
    wanted: Vec<(Entity, String)>,
    ready: Vec<String>,
    missing: Vec<String>,
}

fn track_meshes(meshes: Query<(Entity, Option<&Name>), With<Mesh3d>>, probe: Res<Probe>) {
    probe.0.lock().unwrap().wanted = meshes
        .iter()
        .map(|(e, name)| (e, name.map_or_else(|| format!("{e:?}"), |n| n.to_string())))
        .collect();
}

fn track_draws(
    probe: Res<Probe>,
    instances: Res<RenderMaterialInstances>,
    materials: Res<ErasedRenderAssets<PreparedMaterial>>,
    specialized: Res<SpecializedMaterialPipelineCache>,
    prepass: Res<SpecializedPrepassMaterialPipelineCache>,
    pipelines: Res<PipelineCache>,
) {
    let mut state = probe.0.lock().unwrap();
    state.ready.clear();
    state.missing.clear();
    for (e, name) in state.wanted.clone() {
        let e = MainEntity::from(e);
        let material_ready = instances
            .instances
            .get(&e)
            .is_some_and(|m| materials.get(m.asset_id).is_some());
        let pipeline_ready = specialized
            .values()
            .filter_map(|v| v.get(&e))
            .any(|p| pipelines.get_render_pipeline(*p).is_some());
        let prepass_ready = prepass
            .values()
            .filter_map(|v| v.get(&e))
            .any(|(_, p, _)| pipelines.get_render_pipeline(*p).is_some());
        if material_ready && pipeline_ready && prepass_ready {
            state.ready.push(name);
        } else {
            state.missing.push(format!(
                "{name}: material={material_ready} pipeline={pipeline_ready} prepass={prepass_ready}"
            ));
        }
    }
}

#[test]
#[ignore = "requires native GPU and locally imported Forest Tree Starter Kit"]
fn imported_tree_lod_materials_remain_drawable() {
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
                synchronous_pipeline_compilation: false,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<PipelinedRenderingPlugin>(),
    )
    .add_plugins((atmosphere::WorldEnvironmentPlugin::game(), TreeWindPlugin))
    .add_systems(Last, track_meshes);
    let probe = Probe::default();
    app.insert_resource(probe.clone());
    app.sub_app_mut(RenderApp)
        .insert_resource(probe.clone())
        .add_systems(Render, track_draws.in_set(RenderSystems::Cleanup));
    let deadline = Instant::now() + Duration::from_secs(60);
    while app.plugins_state() != PluginsState::Ready {
        assert!(Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            512,
            512,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    app.world_mut().spawn((
        Camera3d::default(),
        RenderTarget::Image(target.into()),
        Transform::from_xyz(0., 9., 28.).looking_at(Vec3::new(0., 8., 0.), Vec3::Y),
        Msaa::Off,
        bevy::core_pipeline::prepass::DepthPrepass,
        bevy::core_pipeline::prepass::MotionVectorPrepass,
    ));
    let variants: Vec<Handle<WorldAsset>> = (0..4)
        .map(|i| {
            app.world()
                .resource::<AssetServer>()
                .load(GltfAssetLabel::Scene(0).from_asset(format!(
                "local/forest_tree_starter_kit/runtime/tree_07/summer/tree_07_summer_lod{i}.gltf"
            )))
        })
        .collect();
    let root = app
        .world_mut()
        .spawn((WorldAssetRoot(variants[0].clone()), Transform::default()))
        .id();
    let mut stable = 0;
    while stable < 20 {
        assert!(
            Instant::now() < deadline,
            "warmup timeout: {:?}",
            probe.0.lock().unwrap()
        );
        app.update();
        let state = probe.0.lock().unwrap();
        stable = if state.ready.len() == 2 && state.missing.is_empty() {
            stable + 1
        } else {
            0
        };
        drop(state);
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut gaps = Vec::new();
    for lod in [1, 2, 3, 0, 1, 2, 3, 0] {
        app.world_mut().get_mut::<WorldAssetRoot>(root).unwrap().0 = variants[lod].clone();
        for frame in 0..16 {
            app.update();
            let state = probe.0.lock().unwrap();
            if state.ready.len() != 2 || !state.missing.is_empty() {
                gaps.push(format!("lod={lod} frame={frame}: {state:?}"));
            }
            drop(state);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    assert!(
        gaps.is_empty(),
        "LOD draw gaps without Temporal:\n{}",
        gaps.join("\n")
    );
}
