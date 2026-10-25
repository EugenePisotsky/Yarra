//! Grass on deferred views (docs/DEFERRED_LIGHTING.md). Blades write their surface into the
//! G-buffer in a pass of their own before Bevy's prepasses, so the terrain under them fails the
//! depth test instead of being shaded; then, among the lighting kernels, one full-screen pass
//! lights each of their pixels once (`shaders/vegetation/deferred.wesl`).
use super::{
    buffers::VegetationBuffers,
    gpu_types::shader_defs,
    pipelines::{DrawOutput, VegetationPipelineKey, VegetationPipelines},
    temporal,
};

use atmosphere::environment::{DeferredLighting, EnvironmentSurfaceGpu, begin_lighting_pass};

use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::{
        Core3dSystems, core_3d::CORE_3D_DEPTH_FORMAT, prepass::ViewPrepassTextures,
        schedule::Core3d,
    },
    pbr::{MeshPipelineViewLayoutKey, MeshPipelineViewLayouts, MeshViewBindGroup},
    prelude::*,
    render::{
        Render, RenderSystems,
        camera::ExtractedCamera,
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
            CachedRenderPipelineId, Canonical, ColorTargetState, CompareFunction,
            DepthStencilState, FragmentState, IndexFormat, LoadOp, Operations, PipelineCache,
            RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline,
            RenderPipelineDescriptor, ShaderStages, Specializer, SpecializerKey, StoreOp,
            TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
            Variants, VertexState, binding_types::texture_2d,
        },
        renderer::{RenderContext, RenderDevice, ViewQuery},
        texture::{CachedTexture, TextureCache},
        view::{ExtractedView, ViewDepthStencilTexture, ViewTarget},
    },
    shader::ShaderDefVal,
};

const LIGHTING_SHADER_PATH: &str = "shaders/vegetation/deferred.wesl";

/// The grass's own G-buffer texel beside Bevy's (`shaders/vegetation/draw/gbuffer.wesl`).
pub(super) const GRASS_TEXTURE_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// A deferred view's grass pipelines, queued each frame.
#[derive(Component)]
pub(super) struct ViewPipelines {
    gbuffer: CachedRenderPipelineId,
    lighting: CachedRenderPipelineId,
    // The G-buffer pipeline writes motion (MetalFX Temporal).
    motion: bool,
}

/// The grass texture of a deferred view, and the lighting kernel's bind group for it.
#[derive(Component)]
struct GrassTexture {
    texture: CachedTexture,
    group: BindGroup,
}

#[derive(Resource)]
pub(super) struct GrassLighting {
    variants: Variants<RenderPipeline, LightingSpecializer>,
    texture_layout: BindGroupLayoutDescriptor,
    // For the G-buffer pass's view slot, which binds nothing.
    unused_view: Option<BindGroup>,
}

impl FromWorld for GrassLighting {
    fn from_world(world: &mut World) -> Self {
        let shader = world.resource::<AssetServer>().load(LIGHTING_SHADER_PATH);
        let texture_layout = BindGroupLayoutDescriptor::new(
            "grass lighting texture",
            &BindGroupLayoutEntries::single(
                ShaderStages::FRAGMENT,
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
        );
        let mut defs = shader_defs();
        defs.extend([
            "SHADOW_FILTER_METHOD_HARDWARE_2X2".into(),
            "ENVIRONMENT_SURFACE".into(),
            ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 2),
            "DEPTH_PREPASS".into(),
            "DEFERRED_PREPASS".into(),
        ]);
        let descriptor = RenderPipelineDescriptor {
            label: Some("grass deferred lighting".into()),
            // The view's layout is selected per view by the specializer.
            layout: Vec::new(),
            vertex: VertexState {
                shader: shader.clone(),
                entry_point: Some("vertex".into()),
                shader_defs: defs.clone(),
                ..default()
            },
            fragment: Some(FragmentState {
                shader,
                entry_point: Some("fragment".into()),
                shader_defs: defs,
                // Set per view by the specializer.
                targets: Vec::new(),
                ..default()
            }),
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                // Reverse Z: the triangle at 0 is in front of every surface and behind none of
                // the sky.
                depth_compare: Some(CompareFunction::Less),
                stencil: default(),
                bias: default(),
            }),
            ..default()
        };
        let specializer = LightingSpecializer {
            view_layouts: world.resource::<MeshPipelineViewLayouts>().clone(),
            draw_layout: world.resource::<VegetationPipelines>().draw_layout.clone(),
            texture_layout: texture_layout.clone(),
        };

        Self {
            variants: Variants::new(specializer, descriptor),
            texture_layout,
            unused_view: None,
        }
    }
}

/// The view bind group slot of the G-buffer pass, which holds the G-buffer as attachments and
/// so cannot bind the view's group, which includes it.
pub(super) fn unused_view_layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new("unused view", &[])
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, SpecializerKey)]
struct LightingKey {
    view_layout_bits: u32,
    target_format: TextureFormat,
}

struct LightingSpecializer {
    view_layouts: MeshPipelineViewLayouts,
    draw_layout: BindGroupLayoutDescriptor,
    texture_layout: BindGroupLayoutDescriptor,
}

impl Specializer<RenderPipeline> for LightingSpecializer {
    type Key = LightingKey;

    fn specialize(
        &self,
        key: Self::Key,
        descriptor: &mut RenderPipelineDescriptor,
    ) -> Result<Canonical<Self::Key>, BevyError> {
        let view_key = MeshPipelineViewLayoutKey::from_bits_retain(key.view_layout_bits);
        descriptor.layout = vec![
            self.view_layouts.get_view_layout(view_key).main_layout,
            self.draw_layout.clone(),
            atmosphere::environment::surface_layout(),
            self.texture_layout.clone(),
        ];
        let fragment = descriptor.fragment.as_mut().unwrap();
        fragment.targets = vec![Some(ColorTargetState::from(key.target_format))];

        if view_key.contains(MeshPipelineViewLayoutKey::ATMOSPHERE) {
            fragment.shader_defs.push("ATMOSPHERE".into());
            descriptor.vertex.shader_defs.push("ATMOSPHERE".into());
        }

        Ok(key)
    }
}

pub(super) fn install(app: &mut SubApp) {
    app.add_systems(
        Render,
        prepare_textures.in_set(RenderSystems::PrepareBindGroups),
    )
    .add_systems(
        Core3d,
        (
            draw_gbuffer.before(Core3dSystems::Prepass),
            light.in_set(DeferredLighting),
        ),
    );
}

/// The G-buffer and lighting pipelines for a deferred view (`draw::queue`).
pub(super) fn specialize(
    cache: &PipelineCache,
    pipelines: &mut VegetationPipelines,
    lighting: &mut GrassLighting,
    view: &ExtractedView,
    view_layout_bits: u32,
    motion: bool,
) -> Result<ViewPipelines, BevyError> {
    let gbuffer = pipelines.draw_variants.specialize(
        cache,
        VegetationPipelineKey {
            output: DrawOutput::GBuffer { motion },
            environment: true,
        },
    )?;
    let lighting = lighting.variants.specialize(
        cache,
        LightingKey {
            view_layout_bits,
            target_format: view.target_format,
        },
    )?;
    Ok(ViewPipelines {
        gbuffer,
        lighting,
        motion,
    })
}

fn prepare_textures(
    mut commands: Commands,
    views: Query<(Entity, &ViewPrepassTextures), With<ViewPipelines>>,
    mut lighting: ResMut<GrassLighting>,
    mut textures: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
) {
    if lighting.unused_view.is_none() {
        lighting.unused_view = Some(device.create_bind_group(
            "unused view",
            &cache.get_bind_group_layout(&unused_view_layout()),
            &[],
        ));
    }

    for (entity, prepass) in &views {
        let texture = textures.get(
            &device,
            TextureDescriptor {
                label: Some("grass g-buffer"),
                size: prepass.size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: GRASS_TEXTURE_FORMAT,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        let group = device.create_bind_group(
            "grass lighting texture",
            &cache.get_bind_group_layout(&lighting.texture_layout),
            &BindGroupEntries::single(&texture.default_view),
        );
        commands
            .entity(entity)
            .insert(GrassTexture { texture, group });
    }
}

/// What the grass's deferred passes read of a view.
type GrassView = (
    &'static ExtractedCamera,
    &'static ViewPipelines,
    &'static GrassTexture,
    Option<&'static MainPassResolutionOverride>,
);

/// The bind groups and draw resources both grass passes need, when grass is drawn.
#[derive(bevy::ecs::system::SystemParam)]
struct Grass<'w> {
    buffers: Res<'w, VegetationBuffers>,
    environment: Option<Res<'w, EnvironmentSurfaceGpu>>,
    cache: Res<'w, PipelineCache>,
}

fn draw_gbuffer(
    view: ViewQuery<(GrassView, &ViewPrepassTextures, &ViewDepthStencilTexture)>,
    grass: Grass,
    lighting: Res<GrassLighting>,
    temporal: Res<temporal::State>,
    mut ctx: RenderContext,
) {
    let ((camera, pipelines, texture, resolution_override), prepass, depth) = view.into_inner();
    let (Some(environment), Some(pipeline), Some(unused_view), Some(deferred)) = (
        &grass.environment,
        grass.cache.get_render_pipeline(pipelines.gbuffer),
        &lighting.unused_view,
        &prepass.deferred,
    ) else {
        return;
    };

    let motion = temporal.group().zip(prepass.motion_vectors.as_ref());

    if !grass.buffers.active || (pipelines.motion && motion.is_none()) {
        return;
    }

    let mut color_attachments = vec![
        Some(deferred.get_attachment()),
        Some(RenderPassColorAttachment {
            view: &texture.texture.default_view,
            resolve_target: None,
            depth_slice: None,
            ops: Operations {
                load: LoadOp::Clear(default()),
                store: StoreOp::Store,
            },
        }),
    ];
    let motion = motion.filter(|_| pipelines.motion);

    if let Some((_, motion_vectors)) = motion {
        color_attachments.push(Some(motion_vectors.get_attachment()));
    }

    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("grass g-buffer"),
        color_attachments: &color_attachments,
        depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    if let Some(viewport) =
        Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution_override)
    {
        pass.set_camera_viewport(&viewport);
    }

    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, unused_view, &[]);
    pass.set_bind_group(1, &grass.buffers.draw_bind_group, &[]);
    pass.set_bind_group(2, &environment.0, &[]);

    if let Some((group, _)) = motion {
        pass.set_bind_group(3, group, &[]);
    }

    pass.set_index_buffer(
        grass.buffers.topology_indices.slice(..),
        IndexFormat::Uint16,
    );

    for offset in [0, 20] {
        pass.draw_indexed_indirect(&grass.buffers.args, offset);
    }
}

fn light(
    view: ViewQuery<(
        GrassView,
        &ViewTarget,
        &ViewDepthStencilTexture,
        &MeshViewBindGroup,
    )>,
    grass: Grass,
    mut ctx: RenderContext,
) {
    let ((camera, pipelines, texture, resolution_override), target, depth, view_group) =
        view.into_inner();
    let (Some(environment), Some(pipeline)) = (
        &grass.environment,
        grass.cache.get_render_pipeline(pipelines.lighting),
    ) else {
        return;
    };

    if !grass.buffers.active {
        return;
    }

    let mut pass = begin_lighting_pass(
        &mut ctx,
        "grass deferred lighting",
        camera,
        target,
        depth,
        resolution_override,
    );
    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, &view_group.main, &view_group.main_offsets);
    pass.set_bind_group(1, &grass.buffers.draw_bind_group, &[]);
    pass.set_bind_group(2, &environment.0, &[]);
    pass.set_bind_group(3, &texture.group, &[]);
    pass.draw(0..3, 0..1);
}
