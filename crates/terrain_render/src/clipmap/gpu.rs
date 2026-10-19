//! The clipmap's textures, its material and the uploads that fill them.
//!
//! Four textures, all written in place by the render world: heights (R32Float, one layer per
//! level, read with `textureLoad`), normals (Rg16Float x and z, filtered), control (R32Uint, the
//! ground's three heaviest surfaces per sample at twice the heights' density,
//! `world::terrain_control`) and a one-row state texture (Rgba32Uint) with the frame's eye,
//! render origin, windows and node residency. The
//! state is a texture rather than material data so the material, and every patch drawing it,
//! stays unchanged from frame to frame. Uploads move to the render world during extraction, so
//! the state a frame draws with is the one its patches were selected with.
use super::{ClipmapLayout, LodRanges, MORPH_SHARE, NodeWindow};
use crate::*;
use bevy::{
    asset::AssetId,
    image::ImageAddressMode,
    render::{
        ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems, render_asset::RenderAssets,
        render_resource::*, renderer::RenderQueue, texture::GpuImage,
    },
};
use world::TerrainNodeKey;

pub(crate) const CLIPMAP_VERTEX_SHADER: &str = "shaders/terrain_clipmap_vertex.wesl";
pub(crate) const CLIPMAP_SHADER: &str = "shaders/terrain_clipmap.wesl";
/// Levels the textures hold at most.
pub const MAX_LEVELS: u8 = 8;
/// State texels: eye, origin, a window per level, then each level's residency rows.
const STATE_WINDOWS: u32 = 2;
const STATE_RESIDENCY: u32 = STATE_WINDOWS + MAX_LEVELS as u32;
/// Residency texels per level: 32 rows of up to 32 nodes, four rows a texel.
const RESIDENCY_TEXELS: u32 = 8;
const STATE_WIDTH: u32 = STATE_RESIDENCY + MAX_LEVELS as u32 * RESIDENCY_TEXELS;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct ClipmapSettings {
    /// Texels per side, levels, sample intervals per node, metres between leaf samples.
    layout: Vec4,
    /// Leaf range, where morphing starts and ends as shares of a level's range, normal LOD bias.
    ranges: Vec4,
    /// Macro variation's three scales in metres and its albedo strength.
    macro_scales: Vec4,
    /// Macro contrast, whether macro variation is on, control texels per side.
    ground: Vec4,
}

/// One of the world's ground surfaces, by texture array layer.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct ClipmapSurface {
    /// Metres per repetition, anti-tiling (0/1), normal strength, normal Y sign.
    pub texture: Vec4,
    /// Roughness minimum and maximum; whether grass covers it so fully that it is shaded by its
    /// field colour alone, without textures (0/1).
    pub material: Vec4,
    /// The grass field's linear colour and how fully it covers the surface.
    pub grass: Vec4,
}

/// Grass covering this much of a surface hides its ground: it shades by the field colour.
pub const COVERED_GROUND: f32 = 0.7;

/// The world's ground surfaces and textures for the clipmap material.
pub struct ClipmapGround {
    pub surfaces: Vec<ClipmapSurface>,
    pub base_color: Handle<Image>,
    pub normal_material: Handle<Image>,
    pub macro_variation: Handle<Image>,
    pub macro_scales: [f32; 3],
    pub macro_contrast: f32,
    pub macro_albedo_strength: f32,
}

impl ClipmapGround {
    /// From a texture set and its surfaces with their layers and grass.
    pub fn new(
        asset_server: &AssetServer,
        profile: &world::TerrainProfile,
        texture_set: &world::TerrainTextureSet,
        surfaces: &[TerrainSurfaceLayer],
    ) -> Self {
        let layers = surfaces
            .iter()
            .map(|s| usize::from(s.layer) + 1)
            .max()
            .unwrap_or(1);
        let mut table = vec![ClipmapSurface::default(); layers.max(1)];
        for s in surfaces {
            let surface = &s.surface;
            table[usize::from(s.layer)] = ClipmapSurface {
                texture: Vec4::new(
                    surface.tile_size.max(0.001),
                    f32::from(surface.anti_tiling),
                    surface.normal_strength,
                    surface.normal_y_sign,
                ),
                material: Vec4::new(
                    surface.roughness_min,
                    surface.roughness_max,
                    f32::from(s.grass[3] >= COVERED_GROUND),
                    0.0,
                ),
                grass: Vec4::from_array(s.grass),
            };
        }
        let (base, normal, macro_uri) = texture_set.runtime_uris();
        Self {
            surfaces: table,
            base_color: crate::page_material::load_repeat_image(asset_server, base, true),
            normal_material: crate::page_material::load_repeat_image(asset_server, normal, false),
            macro_variation: crate::page_material::load_repeat_image(
                asset_server,
                macro_uri,
                false,
            ),
            macro_scales: profile.macro_scales,
            macro_contrast: profile.macro_contrast,
            macro_albedo_strength: profile.macro_albedo_strength,
        }
    }
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct ClipmapTerrainMaterial {
    // Fragment-only storage, as in every terrain material: Metal has few vertex buffer slots.
    #[storage(120, read_only, visibility(fragment))]
    pub(crate) environment: Handle<ShaderBuffer>,
    #[texture(121)]
    #[sampler(122)]
    pub(crate) cloud_shadows: Option<Handle<Image>>,
    #[texture(123, sample_type = "float", filterable = false)]
    pub(crate) rain_shelter: Option<Handle<Image>>,
    #[texture(124)]
    #[sampler(125)]
    pub(crate) forest_shadow: Option<Handle<Image>>,
    #[texture(126)]
    #[sampler(127)]
    pub(crate) terrain_shadow: Option<Handle<Image>>,
    #[uniform(0)]
    settings: ClipmapSettings,
    #[texture(1, dimension = "2d_array", sample_type = "float", filterable = false)]
    heights: Option<Handle<Image>>,
    #[texture(2, dimension = "2d_array")]
    #[sampler(3)]
    normals: Option<Handle<Image>>,
    #[texture(4, sample_type = "u_int")]
    state: Option<Handle<Image>>,
    #[texture(5, dimension = "2d_array", sample_type = "u_int")]
    control: Option<Handle<Image>>,
    #[storage(6, read_only, visibility(fragment))]
    surfaces: Handle<ShaderBuffer>,
    #[texture(7, dimension = "2d_array")]
    #[sampler(8)]
    base_color: Option<Handle<Image>>,
    #[texture(9, dimension = "2d_array")]
    normal_material: Option<Handle<Image>>,
    #[texture(10)]
    macro_variation: Option<Handle<Image>>,
}

impl Material for ClipmapTerrainMaterial {
    fn vertex_shader() -> ShaderRef {
        CLIPMAP_VERTEX_SHADER.into()
    }
    // Shadow cascades draw the same morphed patches.
    fn prepass_vertex_shader() -> ShaderRef {
        CLIPMAP_VERTEX_SHADER.into()
    }
    fn fragment_shader() -> ShaderRef {
        CLIPMAP_SHADER.into()
    }
    // As the composite terrain: MetalFX motion comes from the main pass's depth and the camera.
    fn enable_prepass() -> bool {
        false
    }
}

/// The clipmap textures of one terrain and the material drawing them.
pub struct ClipmapTextures {
    pub layout: ClipmapLayout,
    pub material: Handle<ClipmapTerrainMaterial>,
    heights: Handle<Image>,
    normals: Handle<Image>,
    control: Handle<Image>,
    state: Handle<Image>,
}

impl ClipmapTextures {
    pub fn new(
        layout: ClipmapLayout,
        ranges: LodRanges,
        ground: ClipmapGround,
        images: &mut Assets<Image>,
        buffers: &mut Assets<ShaderBuffer>,
        materials: &mut Assets<ClipmapTerrainMaterial>,
    ) -> Self {
        assert!(layout.levels <= MAX_LEVELS && layout.nodes_per_side() <= 32);
        assert!(layout.intervals.is_power_of_two());
        let mut image = |width, height, layers, format, sampler| {
            let mut image = Image::new_uninit(
                Extent3d {
                    width,
                    height,
                    depth_or_array_layers: layers,
                },
                TextureDimension::D2,
                format,
                RenderAssetUsages::RENDER_WORLD,
            );
            if layers > 1 {
                image.texture_view_descriptor = Some(TextureViewDescriptor {
                    dimension: Some(TextureViewDimension::D2Array),
                    ..default()
                });
            }
            image.sampler = sampler;
            images.add(image)
        };
        let side = layout.side;
        // A single-level clipmap still binds arrays: give it a second, unused layer.
        let layers = u32::from(layout.levels).max(2);
        let heights = image(
            side,
            side,
            layers,
            TextureFormat::R32Float,
            ImageSampler::nearest(),
        );
        // Toroidal: bilinear filtering wraps across the texture edge as the window does.
        let normals = image(
            side,
            side,
            layers,
            TextureFormat::Rg16Float,
            ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                ..default()
            }),
        );
        let control_side = control_layout(&layout).side;
        let control = image(
            control_side,
            control_side,
            layers,
            TextureFormat::R32Uint,
            ImageSampler::nearest(),
        );
        let state = image(
            STATE_WIDTH,
            1,
            1,
            TextureFormat::Rgba32Uint,
            ImageSampler::nearest(),
        );
        let leaf = layout.spacing(0) as f32;
        let material = materials.add(ClipmapTerrainMaterial {
            environment: atmosphere::environment::fallback_parameters(),
            cloud_shadows: None,
            rain_shelter: None,
            forest_shadow: None,
            terrain_shadow: None,
            settings: ClipmapSettings {
                layout: Vec4::new(
                    side as f32,
                    f32::from(layout.levels),
                    layout.intervals as f32,
                    leaf,
                ),
                // Morphing completes a little before the range so float error at a level's
                // edge never leaves its outer vertices short of the coarser grid.
                ranges: Vec4::new(ranges.leaf as f32, 1.0 - MORPH_SHARE as f32, 0.98, 0.0),
                macro_scales: Vec4::new(
                    ground.macro_scales[0],
                    ground.macro_scales[1],
                    ground.macro_scales[2],
                    ground.macro_albedo_strength,
                ),
                ground: Vec4::new(ground.macro_contrast, 1.0, control_side as f32, 0.0),
            },
            heights: Some(heights.clone()),
            normals: Some(normals.clone()),
            state: Some(state.clone()),
            control: Some(control.clone()),
            surfaces: buffers.add(ShaderBuffer::new(
                crate::storage_bytes(&ground.surfaces),
                RenderAssetUsages::RENDER_WORLD,
            )),
            base_color: Some(ground.base_color),
            normal_material: Some(ground.normal_material),
            macro_variation: Some(ground.macro_variation),
        });
        Self {
            layout,
            material,
            heights,
            normals,
            control,
            state,
        }
    }
}

/// The control texture's layout: twice the samples of the heights' per node.
fn control_layout(layout: &ClipmapLayout) -> ClipmapLayout {
    ClipmapLayout {
        side: layout.side * 2,
        intervals: layout.intervals * 2,
        ..*layout
    }
}

/// One node's samples for its level's block: `intervals + 1` per side, the last row and column
/// landing in the next blocks (the same world samples their own nodes hold).
pub struct NodeBlock {
    pub key: TerrainNodeKey,
    pub heights: Vec<f32>,
    /// Unit normals' x and z, as half floats.
    pub normals: Vec<[u16; 2]>,
    /// Packed control samples, `2 × intervals + 1` per side (`world::CONTROL_SAMPLES`).
    pub control: Vec<u32>,
}

impl NodeBlock {
    pub fn new(
        key: TerrainNodeKey,
        heights: Vec<f32>,
        normals: &[[f32; 3]],
        control: Vec<u32>,
    ) -> Self {
        let normals = normals
            .iter()
            .map(|n| {
                [
                    half::f16::from_f32(n[0]).to_bits(),
                    half::f16::from_f32(n[2]).to_bits(),
                ]
            })
            .collect();
        Self {
            key,
            heights,
            normals,
            control,
        }
    }
}

/// Each level's resident nodes, as rows of bits by toroidal slot: the shaders' residency.
#[derive(Clone, Debug, Default)]
pub struct Residency(Vec<[u32; 32]>);

impl Residency {
    pub fn new(levels: u8) -> Self {
        Self(vec![[0; 32]; usize::from(levels)])
    }
    pub fn set(&mut self, layout: &ClipmapLayout, key: TerrainNodeKey, resident: bool) {
        let n = layout.nodes_per_side();
        let (row, bit) = (key.z.rem_euclid(n) as usize, 1_u32 << key.x.rem_euclid(n));
        let word = &mut self.0[usize::from(key.level)][row];
        if resident {
            *word |= bit;
        } else {
            *word &= !bit;
        }
    }
}

/// What the shaders read of the frame: the eye in render space, the render origin in leaf
/// samples, and each level's window and resident nodes.
pub struct ClipmapFrame<'a> {
    pub eye: Vec3,
    pub origin_samples: [i64; 2],
    pub windows: &'a [NodeWindow],
    pub residency: &'a Residency,
}

/// Main-world uploads for the next extraction.
#[derive(Resource, Default)]
pub struct ClipmapUploads {
    target: Option<Target>,
    blocks: Vec<NodeBlock>,
    state: Option<Vec<u32>>,
    pub uploaded_blocks: u64,
}

#[derive(Clone, Copy)]
struct Target {
    heights: AssetId<Image>,
    normals: AssetId<Image>,
    control: AssetId<Image>,
    state: AssetId<Image>,
    layout: ClipmapLayout,
}

impl ClipmapUploads {
    /// Uploads go to these textures from now on; anything queued for others is dropped.
    pub fn target(&mut self, textures: &ClipmapTextures) {
        self.target = Some(Target {
            heights: textures.heights.id(),
            normals: textures.normals.id(),
            control: textures.control.id(),
            state: textures.state.id(),
            layout: textures.layout,
        });
        self.blocks.clear();
        self.state = None;
    }
    pub fn clear(&mut self) {
        self.target = None;
        self.blocks.clear();
        self.state = None;
    }
    pub fn push(&mut self, block: NodeBlock) {
        self.uploaded_blocks += 1;
        self.blocks.push(block);
    }
    pub fn set_frame(&mut self, frame: &ClipmapFrame) {
        let mut state = vec![0_u32; STATE_WIDTH as usize * 4];
        state[..3].copy_from_slice(&frame.eye.to_array().map(f32::to_bits));
        state[4] = frame.origin_samples[0] as i32 as u32;
        state[5] = frame.origin_samples[1] as i32 as u32;
        for window in frame.windows {
            let at = (STATE_WINDOWS + u32::from(window.level)) as usize * 4;
            state[at] = window.min[0] as u32;
            state[at + 1] = window.min[1] as u32;
            state[at + 2] = window.size as u32;
        }
        for (level, rows) in frame.residency.0.iter().enumerate() {
            let at = (STATE_RESIDENCY + level as u32 * RESIDENCY_TEXELS) as usize * 4;
            state[at..at + 32].copy_from_slice(rows);
        }
        self.state = Some(state);
    }
}

#[derive(Resource, Default)]
struct RenderUploads {
    target: Option<Target>,
    blocks: Vec<NodeBlock>,
    state: Option<Vec<u32>>,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<ClipmapUploads>()
        .add_plugins(MaterialPlugin::<ClipmapTerrainMaterial>::default());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<RenderUploads>()
            .add_systems(ExtractSchedule, extract)
            .add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

fn extract(mut main: ResMut<MainWorld>, mut render: ResMut<RenderUploads>) {
    let Some(mut uploads) = main.get_resource_mut::<ClipmapUploads>() else {
        return;
    };
    if render.target.map(|t| t.heights) != uploads.target.map(|t| t.heights) {
        *render = RenderUploads {
            target: uploads.target,
            ..default()
        };
    }
    render.blocks.append(&mut uploads.blocks);
    if let Some(state) = uploads.state.take() {
        render.state = Some(state);
    }
}

fn upload(
    mut uploads: ResMut<RenderUploads>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
) {
    let Some(target) = uploads.target else {
        return;
    };
    // Images appear a frame or more after they are added: keep everything until then.
    let (Some(heights), Some(normals), Some(control), Some(state)) = (
        images.get(target.heights),
        images.get(target.normals),
        images.get(target.control),
        images.get(target.state),
    ) else {
        return;
    };
    let layout = target.layout;
    let control_layout = control_layout(&layout);
    for block in uploads.blocks.drain(..) {
        let origin = layout.block_origin(block.key);
        let layer = u32::from(block.key.level);
        write_block(
            &queue,
            heights,
            &layout,
            origin,
            layer,
            bytemuck_f32(&block.heights),
        );
        write_block(
            &queue,
            normals,
            &layout,
            origin,
            layer,
            bytemuck_u16(&block.normals),
        );
        write_block(
            &queue,
            control,
            &control_layout,
            control_layout.block_origin(block.key),
            layer,
            block.control.iter().flat_map(|v| v.to_le_bytes()).collect(),
        );
    }
    if let Some(words) = uploads.state.take() {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &state.texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &bytes,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(STATE_WIDTH * 16),
                rows_per_image: Some(1),
            },
            Extent3d {
                width: STATE_WIDTH,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }
}

fn bytemuck_f32(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn bytemuck_u16(values: &[[u16; 2]]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|[a, b]| [a.to_le_bytes(), b.to_le_bytes()])
        .flatten()
        .collect()
}

/// Spans of `n` texels from `start` in a toroidal axis of `side`: (texel, first sample, count).
fn spans(start: u32, n: u32, side: u32) -> impl Iterator<Item = (u32, u32, u32)> {
    let first = n.min(side - start);
    [(start, 0, first), (0, first, n - first)]
        .into_iter()
        .filter(|s| s.2 > 0)
}

/// A node's `(intervals + 1)²` texels from its block origin, split where they wrap around.
fn write_block(
    queue: &RenderQueue,
    image: &GpuImage,
    layout: &ClipmapLayout,
    origin: [u32; 2],
    layer: u32,
    data: Vec<u8>,
) {
    let n = layout.intervals + 1;
    let texel = data.len() as u32 / (n * n);
    for (tz, sz, rows) in spans(origin[1], n, layout.side) {
        for (tx, sx, columns) in spans(origin[0], n, layout.side) {
            queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &image.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: tx,
                        y: tz,
                        z: layer,
                    },
                    aspect: TextureAspect::All,
                },
                &data,
                TexelCopyBufferLayout {
                    offset: u64::from((sz * n + sx) * texel),
                    bytes_per_row: Some(n * texel),
                    rows_per_image: Some(n),
                },
                Extent3d {
                    width: columns,
                    height: rows,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
}

/// The patch every node draws: `intervals + 1` vertices per side at their sample indices, and
/// the diagonal from each quad's (x, z) to (x + 1, z + 1), as the coarser grid has it.
pub fn patch_mesh(layout: &ClipmapLayout) -> Mesh {
    let n = layout.intervals + 1;
    let positions: Vec<[f32; 3]> = (0..n * n)
        .map(|i| [(i % n) as f32, 0.0, (i / n) as f32])
        .collect();
    let indices = crate::lod::mesh::stitch_indices(n as u16, crate::lod::StitchEdges(0))
        .expect("clipmap patch grid");
    let mut mesh = Mesh::new(
        bevy::render::render_resource::PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(bevy::mesh::Indices::U32(indices));
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_blocks_split_into_spans_that_cover_them_once() {
        assert_eq!(spans(0, 33, 1024).collect::<Vec<_>>(), [(0, 0, 33)]);
        assert_eq!(
            spans(992, 33, 1024).collect::<Vec<_>>(),
            [(992, 0, 32), (0, 32, 1)]
        );
    }
}
