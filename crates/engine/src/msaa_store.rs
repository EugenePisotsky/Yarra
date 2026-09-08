//! Resolve-only color storage for the game's forward cameras, retaining Bevy's
//! pass ordering and draw phases. Depth is deliberately still stored.

use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::{
        Core3d,
        core_3d::{AlphaMask3d, Opaque3d, Transparent3d, main_opaque_pass_3d},
        oit::OrderIndependentTransparencySettings,
        skybox::{SkyboxBindGroup, SkyboxPipelineId},
    },
    ecs::{schedule::SystemWithAccess, system::IntoSystem},
    light::VolumetricFog,
    pbr::{GpuAtmosphereSettings, Transmissive3d, wireframe::Wireframe3d},
    prelude::*,
    render::{
        RenderApp,
        camera::ExtractedCamera,
        diagnostic::RecordDiagnostics,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_phase::{ViewBinnedRenderPhases, ViewSortedRenderPhases},
        render_resource::{PipelineCache, RenderPassDescriptor, StoreOp},
        renderer::{RenderContext, ViewQuery},
        view::{ExtractedView, ViewDepthTexture, ViewTarget, ViewUniformOffset},
    },
};

/// `Automatic` permits discarding MSAA color after the opaque pass when the
/// built-in later consumers are absent. Custom passes that load multisample
/// color must set this to `Preserve`. Cameras without this component preserve it.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, ExtractComponent)]
pub enum MsaaColorStorePolicy {
    #[default]
    Preserve,
    Automatic,
}

pub struct MsaaColorStorePlugin;

impl Plugin for MsaaColorStorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<MsaaColorStorePolicy>::default());
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        let mut schedules = render_app.world_mut().resource_mut::<Schedules>();
        let schedule = schedules
            .get_mut(Core3d)
            .expect("Core3d plugin is required");
        // Replace the uninitialized system in its existing graph slot. Removing
        // and re-adding it would lose ordering edges from transmission, atmosphere,
        // deferred lighting and third-party plugins attached to Bevy's system set.
        // Revisit this adapter when updating Bevy; it is intentionally 0.19-specific.
        assert!(!schedule.graph().systems.is_initialized());
        let original_type = IntoSystem::into_system(main_opaque_pass_3d).system_type();
        let matches: Vec<_> = schedule
            .graph()
            .systems
            .iter()
            .filter(|(_, system, _)| system.system_type() == original_type)
            .map(|(key, _, _)| key)
            .collect();
        assert_eq!(matches.len(), 1, "expected one Bevy opaque pass");
        *schedule.graph_mut().systems.get_mut(matches[0]).unwrap() =
            SystemWithAccess::new(Box::new(IntoSystem::into_system(opaque_pass)));
    }
}

fn can_discard_color(
    world: &World,
    view_entity: Entity,
    view: &ExtractedView,
    target: &ViewTarget,
    policy: Option<&MsaaColorStorePolicy>,
    targets: &Query<(Entity, &ViewTarget)>,
) -> bool {
    if policy != Some(&MsaaColorStorePolicy::Automatic) {
        return false;
    }
    let Some(sampled) = target.sampled_main_texture() else {
        return false; // Never discard single-sample color: it is the final image.
    };
    // Cameras with the same target can share their MSAA attachment. Keep it even
    // when the other camera has no draws this frame, since it may load/resolve it.
    if targets.iter().any(|(entity, other)| {
        entity != view_entity
            && other
                .sampled_main_texture()
                .is_some_and(|texture| texture.id() == sampled.id())
    }) {
        return false;
    }
    if world.get::<GpuAtmosphereSettings>(view_entity).is_some()
        || world
            .get::<OrderIndependentTransparencySettings>(view_entity)
            .is_some()
        || world.get::<VolumetricFog>(view_entity).is_some()
    {
        return false;
    }
    let retained = &view.retained_view_entity;
    // Gizmos also use Transparent3d. Missing phase resources/entries are treated
    // conservatively, rather than assuming a partially configured view is empty.
    let transparent_empty = world
        .get_resource::<ViewSortedRenderPhases<Transparent3d>>()
        .and_then(|phases| phases.get(retained))
        .is_some_and(|phase| phase.items.is_empty());
    let transmissive_empty = world
        .get_resource::<ViewSortedRenderPhases<Transmissive3d>>()
        .and_then(|phases| phases.get(retained))
        .is_some_and(|phase| phase.items.is_empty());
    let wireframe_empty = world
        .get_resource::<ViewBinnedRenderPhases<Wireframe3d>>()
        .is_none_or(|phases| phases.get(retained).is_none_or(|phase| phase.is_empty()));
    transparent_empty && transmissive_empty && wireframe_empty
}

// Adapted from Bevy 0.19.1 main_opaque_pass_3d (MIT; see third_party/BEVY-MIT.txt).
// Draw ordering, viewport, skybox, depth and diagnostic spans match upstream.
fn opaque_pass(
    world: &World,
    view: ViewQuery<(
        &ExtractedCamera,
        &ExtractedView,
        &ViewTarget,
        &ViewDepthTexture,
        Option<&SkyboxPipelineId>,
        Option<&SkyboxBindGroup>,
        &ViewUniformOffset,
        Option<&MainPassResolutionOverride>,
        Option<&MsaaColorStorePolicy>,
    )>,
    opaque_phases: Res<ViewBinnedRenderPhases<Opaque3d>>,
    alpha_mask_phases: Res<ViewBinnedRenderPhases<AlphaMask3d>>,
    pipeline_cache: Res<PipelineCache>,
    targets: Query<(Entity, &ViewTarget)>,
    mut ctx: RenderContext,
) {
    let view_entity = view.entity();
    let (
        camera,
        extracted_view,
        target,
        depth,
        skybox_pipeline,
        skybox_bind_group,
        view_uniform_offset,
        resolution_override,
        policy,
    ) = view.into_inner();
    let (Some(opaque_phase), Some(alpha_mask_phase)) = (
        opaque_phases.get(&extracted_view.retained_view_entity),
        alpha_mask_phases.get(&extracted_view.retained_view_entity),
    ) else {
        return;
    };
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let discard = can_discard_color(world, view_entity, extracted_view, target, policy, &targets);
    let mut color = target.get_color_attachment();
    if discard && color.resolve_target.is_some() {
        // Discard affects only the multisampled attachment. The resolve target
        // is still written and is consumed by UI/postprocessing/presentation.
        color.ops.store = StoreOp::Discard;
    }
    let label = if discard {
        "main_opaque_pass_3d_resolve_only"
    } else {
        "main_opaque_pass_3d"
    };
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(color)],
        depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    let span = diagnostics.pass_span(&mut pass, "main_opaque_pass_3d");
    if let Some(viewport) =
        Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution_override)
    {
        pass.set_camera_viewport(&viewport);
    }
    if !opaque_phase.is_empty()
        && let Err(err) = opaque_phase.render(&mut pass, world, view_entity)
    {
        error!("Error encountered while rendering the opaque phase {err:?}");
    }
    if !alpha_mask_phase.is_empty()
        && let Err(err) = alpha_mask_phase.render(&mut pass, world, view_entity)
    {
        error!("Error encountered while rendering the alpha mask phase {err:?}");
    }
    if let (Some(skybox_pipeline), Some(SkyboxBindGroup(skybox_bind_group))) =
        (skybox_pipeline, skybox_bind_group)
        && let Some(pipeline) = pipeline_cache.get_render_pipeline(skybox_pipeline.0)
    {
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(
            0,
            &skybox_bind_group.0,
            &[view_uniform_offset.offset, skybox_bind_group.1],
        );
        pass.draw(0..3, 0..1);
    }
    span.end(&mut pass);
}

#[cfg(test)]
mod gpu_tests;
