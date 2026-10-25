//! Deferred lighting (docs/DEFERRED_LIGHTING.md): surfaces on deferred views write a G-buffer in
//! the prepass (`shaders/lighting/gbuffer.wesl`), and before the main pass draws the forward
//! surfaces each lighting model's kernel lights its own pixels once, in `DeferredLighting`. This
//! module owns the kernel of the models the shared lighting serves
//! (`shaders/lighting/deferred.wesl`: terrain, objects, tree crowns and bark), which branches on
//! each pixel's model; other models' renderers own theirs.
//!
//! Bevy's deferred lighting pass is skipped: it runs Bevy's lighting, and it places pixels by the
//! whole view, which is wrong when the main pass draws a region of it (MetalFX Temporal). Its
//! pipeline layout is kept: the pass is specialised from it, so the view's shader definitions and
//! bind groups match the materials', with the environment group in place of Bevy's third group.
use super::surface::{EnvironmentSurfaceGpu, EnvironmentSurfaceLayout};

use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::{
        Core3dSystems, core_3d::main_opaque_pass_3d,
        deferred::copy_lighting_id::copy_deferred_lighting_id, prepass::DeferredPrepass,
        schedule::Core3d,
    },
    ecs::schedule::{SystemKey, SystemWithAccess},
    pbr::{
        MeshPipelineKey, MeshViewBindGroup, ViewKeyCache,
        deferred::{DeferredLightingLayout, SkipDeferredLighting},
    },
    platform::collections::HashMap,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        camera::ExtractedCamera,
        render_phase::TrackedRenderPass,
        render_resource::*,
        renderer::{RenderContext, ViewQuery},
        view::{ExtractedView, ViewDepthStencilTexture, ViewTarget},
    },
    shader::ShaderDefVal,
};

const SHADER: &str = "shaders/lighting/deferred.wesl";

/// Where other models' lighting kernels draw on deferred views: after the shared lighting's
/// kernel, whose pass is the first on the view's target and clears it, and before the main pass.
/// Each kernel covers the view with `begin_lighting_pass` and discards other models' pixels, so
/// their order among themselves does not matter.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeferredLighting;

/// The lighting pipeline per view key, specialised once each.
#[derive(Resource, Default)]
struct Pipelines(HashMap<MeshPipelineKey, CachedRenderPipelineId>);

#[derive(Component)]
struct Pipeline(CachedRenderPipelineId);

pub(super) fn install(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .init_resource::<Pipelines>()
        .add_systems(
            Render,
            skip_bevy_lighting.in_set(RenderSystems::CreateViews),
        )
        .add_systems(Render, prepare.in_set(RenderSystems::Prepare))
        .configure_sets(
            Core3d,
            DeferredLighting
                .in_set(Core3dSystems::MainPass)
                .before(main_opaque_pass_3d),
        )
        .add_systems(
            Core3d,
            draw.in_set(Core3dSystems::MainPass)
                .before(DeferredLighting),
        );
}

/// Bevy copies each pixel's lighting-pass id into a depth texture for its own lighting pass,
/// which this one replaces: the copy, a pass over the whole window, becomes a no-op in its slot,
/// so ordering edges attached to it stay valid.
pub(super) fn finish(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    let mut schedules = render.world_mut().resource_mut::<Schedules>();
    let schedule = schedules
        .get_mut(Core3d)
        .expect("Core3d plugin is required");
    assert!(!schedule.graph().systems.is_initialized());
    let copy = IntoSystem::into_system(copy_deferred_lighting_id).system_type();
    let keys: Vec<SystemKey> = schedule
        .graph()
        .systems
        .iter()
        .filter(|(_, system, _)| system.system_type() == copy)
        .map(|(key, _, _)| key)
        .collect();
    assert_eq!(
        keys.len(),
        1,
        "Bevy 0.20 schedules its lighting-id copy once"
    );
    *schedule.graph_mut().systems.get_mut(keys[0]).unwrap() =
        SystemWithAccess::new(Box::new(IntoSystem::into_system(skip_lighting_id_copy)));
}

fn skip_lighting_id_copy() {}

fn skip_bevy_lighting(
    mut commands: Commands,
    views: Query<Entity, (With<DeferredPrepass>, Without<SkipDeferredLighting>)>,
) {
    for view in &views {
        commands.entity(view).insert(SkipDeferredLighting);
    }
}

#[allow(clippy::too_many_arguments)] // Independent render-world inputs of one pipeline.
fn prepare(
    mut commands: Commands,
    views: Query<(Entity, &ExtractedView), With<DeferredPrepass>>,
    view_keys: Res<ViewKeyCache>,
    bevy_layout: Option<Res<DeferredLightingLayout>>,
    environment: Res<EnvironmentSurfaceLayout>,
    server: Res<AssetServer>,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<Pipelines>,
) {
    let Some(bevy_layout) = bevy_layout else {
        return;
    };

    for (entity, view) in &views {
        let Some(&key) = view_keys.get(&view.retained_view_entity) else {
            continue;
        };
        let id = *pipelines.0.entry(key).or_insert_with(|| {
            cache.queue_render_pipeline(descriptor(&bevy_layout, key, &environment.0, &server))
        });
        commands.entity(entity).insert(Pipeline(id));
    }
}

/// Bevy's deferred lighting pipeline for the view key, with our shader, the environment group,
/// and a depth test against the view's depth that passes wherever a surface was drawn.
fn descriptor(
    bevy_layout: &DeferredLightingLayout,
    key: MeshPipelineKey,
    environment: &BindGroupLayoutDescriptor,
    server: &AssetServer,
) -> RenderPipelineDescriptor {
    let mut descriptor = bevy_layout.specialize(key);
    let shader = server.load(SHADER);

    // Our lighting has no clearcoat; the environment group is the material group the lighting's
    // modules bind (`shaders/lighting/surface.wesl`).
    let mut defs: Vec<ShaderDefVal> = descriptor
        .vertex
        .shader_defs
        .drain(..)
        .filter(|def| *def != ShaderDefVal::from("STANDARD_MATERIAL_CLEARCOAT"))
        .collect();
    defs.push(ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 2));
    descriptor.label = Some("deferred lighting".into());
    descriptor.layout[2] = environment.clone();
    descriptor.vertex.shader = shader.clone();
    descriptor.vertex.shader_defs = defs.clone();
    descriptor.vertex.entry_point = Some("vertex".into());
    let fragment = descriptor
        .fragment
        .as_mut()
        .expect("Bevy's deferred lighting has a fragment stage");
    fragment.shader = shader;
    fragment.shader_defs = defs;
    fragment.entry_point = Some("fragment".into());
    descriptor.depth_stencil = Some(DepthStencilState {
        format: bevy::core_pipeline::core_3d::CORE_3D_DEPTH_FORMAT,
        depth_write_enabled: Some(false),
        // Reverse Z: the triangle at 0 is in front of every surface and behind none of the sky.
        depth_compare: Some(CompareFunction::Less),
        stencil: StencilState::default(),
        bias: DepthBiasState::default(),
    });
    descriptor
}

/// What the pass reads of a deferred view.
type LitView = (
    &'static ExtractedCamera,
    &'static MeshViewBindGroup,
    &'static ViewTarget,
    &'static ViewDepthStencilTexture,
    &'static Pipeline,
    Option<&'static MainPassResolutionOverride>,
);

fn draw(
    view: ViewQuery<LitView>,
    environment: Option<Res<EnvironmentSurfaceGpu>>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (camera, view_bind_group, target, depth, pipeline, resolution_override) = view.into_inner();

    // Opened whether or not the kernel can draw yet: the target's first pass clears it, and it
    // must be this one, ahead of the kernels in `DeferredLighting`, which load what it leaves.
    let mut pass = begin_lighting_pass(
        &mut ctx,
        "deferred lighting",
        camera,
        target,
        depth,
        resolution_override,
    );
    let (Some(environment), Some(pipeline)) = (&environment, cache.get_render_pipeline(pipeline.0))
    else {
        return;
    };

    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, &view_bind_group.main, &view_bind_group.main_offsets);
    pass.set_bind_group(1, &view_bind_group.binding_array, &[]);
    pass.set_bind_group(2, &environment.0, &[]);
    pass.draw(0..3, 0..1);
}

/// A lighting kernel's pass over the main pass's region of the view: it writes the HDR target and
/// tests against the view's depth without writing it, so a triangle drawn at the far plane with
/// depth compare Less reaches every surface's pixel and none of the sky's.
pub fn begin_lighting_pass<'a>(
    ctx: &'a mut RenderContext,
    label: &'static str,
    camera: &ExtractedCamera,
    target: &'a ViewTarget,
    depth: &'a ViewDepthStencilTexture,
    resolution_override: Option<&MainPassResolutionOverride>,
) -> TrackedRenderPass<'a> {
    let mut depth_attachment = depth.get_attachment(StoreOp::Store);
    depth_attachment.depth_ops = None;
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(target.get_color_attachment())],
        depth_stencil_attachment: Some(depth_attachment),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    if let Some(viewport) =
        Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution_override)
    {
        pass.set_camera_viewport(&viewport);
    }

    pass
}
