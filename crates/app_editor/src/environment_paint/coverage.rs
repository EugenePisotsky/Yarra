//! Editor-only raw coverage overlay. It shares the resident terrain mesh, owns only a small
//! mask texture/material, and never changes the compiled ground or persistent paint data.
use super::*;
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat},
    shader::ShaderRef,
};
use engine::{StreamedTerrainSurface, WorldOrigin};
use std::collections::HashMap;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(super) struct CoverageMaterial {
    #[uniform(0)]
    bounds: Vec4,
    #[uniform(1)]
    grid: Vec4,
    #[texture(2)]
    #[sampler(3)]
    mask: Handle<Image>,
}
impl Material for CoverageMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/environment_coverage.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
    fn depth_bias(&self) -> f32 {
        5.0
    }
}
#[derive(Component)]
pub(super) struct CoverageOverlay;
struct Tile {
    entity: Entity,
    material: Handle<CoverageMaterial>,
    image: Handle<Image>,
    signature: ([u8; 32], CellCoord, LayerId),
}
#[derive(Resource, Default)]
pub(super) struct CoverageOverlays {
    tiles: HashMap<Entity, Tile>,
}
#[derive(SystemParam)]
pub(super) struct CoverageSource<'w, 's> {
    project: Res<'w, ProjectEditorStore>,
    dense: Res<'w, DenseDomainWorkingSets>,
    origin: Res<'w, WorldOrigin>,
    workspace: Res<'w, State<EditorWorkspace>>,
    mode: Res<'w, PreviewModeState>,
    tools: Res<'w, EditorToolRegistry>,
    terrain: Query<
        'w,
        's,
        (
            Entity,
            &'static StreamedTerrainSurface,
            &'static Mesh3d,
            &'static Transform,
        ),
        Without<CoverageOverlay>,
    >,
}
pub(super) fn update_coverage(
    source: CoverageSource,
    mut paint: ResMut<EnvironmentPaintState>,
    mut owned: ResMut<CoverageOverlays>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<CoverageMaterial>>,
    mut commands: Commands,
) {
    let active = paint.show_coverage
        && *source.workspace.get() == EditorWorkspace::World
        && source.mode.active() == Some(EditorPreviewMode::Authoring)
        && source
            .tools
            .active(EditorWorkspace::World)
            .is_some_and(|tool| tool.id == ENVIRONMENT_TOOL.id);
    paint.coverage_visible = active;
    let definition = source
        .origin
        .space()
        .and_then(|space| source.dense.definition(space));
    let selected = paint.selected;
    let mut retained = std::collections::HashSet::new();
    if let (true, Some(definition), Some(layer)) = (active, definition, selected)
        && definition.layers.iter().any(|l| l.id == layer)
    {
        for (entity, surface, mesh, transform) in &source.terrain {
            if surface.key.space != definition.space {
                continue;
            }
            let local = source
                .dense
                .environment_record(definition.space, surface.key.cell);
            let loaded = source
                .project
                .environment_snapshot()
                .filter(|s| {
                    s.definition.space == definition.space
                        && s.definition.mask_resolution == definition.mask_resolution
                })
                .and_then(|s| s.coverage.cells.iter().find(|c| c.cell == surface.key.cell));
            let tiles = local
                .map(|r| r.tiles.as_slice())
                .or_else(|| loaded.map(|r| r.tiles.as_slice()));
            let Some(tiles) = tiles else {
                continue;
            }; // An unloaded cell must not look like empty coverage.
            let resolution = usize::from(definition.mask_resolution);
            let samples = tiles
                .iter()
                .find(|tile| tile.layer == layer)
                .map(|tile| tile.samples.as_slice());
            if samples.is_some_and(|samples| samples.len() != resolution * resolution) {
                continue;
            }
            retained.insert(entity);
            let signature = (
                *blake3::hash(samples.unwrap_or(&[])).as_bytes(),
                source.origin.cell(),
                layer,
            );
            if owned
                .tiles
                .get(&entity)
                .is_some_and(|old| old.signature == signature)
            {
                continue;
            }
            if let Some(old) = owned.tiles.remove(&entity) {
                commands.entity(old.entity).despawn();
                materials.remove(old.material.id());
                images.remove(old.image.id());
            }
            let mut image = Image::new(
                Extent3d {
                    width: resolution as u32,
                    height: resolution as u32,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                samples.map_or_else(|| vec![0; resolution * resolution], |s| s.to_vec()),
                TextureFormat::R8Unorm,
                RenderAssetUsages::default(),
            );
            image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                ..default()
            });
            let image = images.add(image);
            let minimum = surface.key.cell.origin(surface.cell_size);
            let origin = source.origin.cell().origin(surface.cell_size);
            let material = materials.add(CoverageMaterial {
                bounds: Vec4::new(
                    (minimum[0] - origin[0]) as f32,
                    (minimum[1] - origin[1]) as f32,
                    1.0 / surface.cell_size,
                    1.0 / surface.cell_size,
                ),
                grid: Vec4::new(
                    (resolution - 1) as f32 / resolution as f32,
                    0.5 / resolution as f32,
                    0.0,
                    0.0,
                ),
                mask: image.clone(),
            });
            let mut transform = *transform;
            transform.translation.y += 0.015;
            let overlay = commands
                .spawn((
                    mesh.clone(),
                    MeshMaterial3d(material.clone()),
                    transform,
                    bevy::light::NotShadowCaster,
                    bevy::light::NotShadowReceiver,
                    CoverageOverlay,
                    Name::new("Selected layer coverage"),
                ))
                .id();
            owned.tiles.insert(
                entity,
                Tile {
                    entity: overlay,
                    material,
                    image,
                    signature,
                },
            );
        }
    }
    owned.tiles.retain(|entity, tile| {
        if retained.contains(entity) {
            true
        } else {
            commands.entity(tile.entity).despawn();
            materials.remove(tile.material.id());
            images.remove(tile.image.id());
            false
        }
    });
}
