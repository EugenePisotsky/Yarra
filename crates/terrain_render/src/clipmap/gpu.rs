//! The clipmap's textures, its material and the uploads that fill them.
//!
//! Five textures, all written in place by the render world: heights (R32Float, one layer per
//! level, read with `textureLoad`), normals (Rg16Float x and z, filtered), control (R32Uint, the
//! ground's three heaviest surfaces per sample at twice the heights' density,
//! `world::terrain_control`), the leaves' soft layer (R16Float depth over the firm ground, level
//! 0 only, filtered: what feet press, `world::soft_layer`) and a one-row state texture
//! (Rgba32Uint) with the frame's eye, render origin, windows and node residency. The
//! state is a texture rather than material data so the material, and every patch drawing it,
//! stays unchanged from frame to frame. Uploads move to the render world during extraction, so
//! the state a frame draws with is the one its patches were selected with.
use super::{ClipmapLayout, DETAIL_LEVELS, LodRanges, MORPH_SHARE, NodeWindow, PatchKey};
use crate::{TerrainSurfaceLayer, storage_bytes};

use bevy::{
    asset::AssetId,
    asset::RenderAssetUsages,
    image::ImageAddressMode,
    image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    pbr::{Material, MaterialPlugin},
    prelude::*,
    reflect::TypePath,
    render::storage::ShaderBuffer,
    render::{
        ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems, render_asset::RenderAssets,
        render_resource::*, renderer::RenderQueue, texture::GpuImage,
    },
    shader::ShaderRef,
};

use std::collections::HashMap;
use world::TerrainNodeKey;

pub(crate) const CLIPMAP_VERTEX_SHADER: &str = "shaders/terrain/clipmap_vertex.wesl";
pub(crate) const CLIPMAP_SHADER: &str = "shaders/terrain/clipmap.wesl";

/// Levels the textures hold at most.
pub const MAX_LEVELS: u8 = 8;

/// State texels: eye, origin, a window per level, each level's residency rows, then the wear
/// slots' cells.
const STATE_WINDOWS: u32 = 2;

const STATE_RESIDENCY: u32 = STATE_WINDOWS + MAX_LEVELS as u32;

/// Residency texels per level: 32 rows of up to 32 nodes, four rows a texel.
const RESIDENCY_TEXELS: u32 = 8;

const STATE_WEAR: u32 = STATE_RESIDENCY + MAX_LEVELS as u32 * RESIDENCY_TEXELS;
const STATE_WIDTH: u32 = STATE_WEAR + WEAR_SLOTS as u32;

/// Road wear near the camera: the cells of a `WEAR_WINDOW`² window, one atlas layer per toroidal
/// slot (`terrain/clipmap.wesl`).
pub const WEAR_WINDOW: i32 = 5;

pub const WEAR_SLOTS: usize = (WEAR_WINDOW * WEAR_WINDOW) as usize;
const WEAR_SIDE: u32 = world::WEAR_RESOLUTION as u32;

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
    /// Range of the first detail level on soft ground (0: none).
    detail: Vec4,
}

/// One of the world's ground surfaces, by texture array layer.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct ClipmapSurface {
    /// Metres per repetition, anti-tiling (0/1), normal strength, normal Y sign.
    pub texture: Vec4,
    /// Roughness minimum and maximum; whether grass covers it so fully that it is shaded by its
    /// field colour alone, without textures (0/1); whether roads wear ground bare to it (0/1).
    pub material: Vec4,
    /// The grass field's linear colour and how fully it covers the surface.
    pub grass: Vec4,
    /// x: how much of the macro albedo variation it takes (`world::TerrainSurface::macro_strength`);
    /// y: whether it is soft and takes prints (0/1, `world::TerrainSurface::soft_depth`).
    pub shade: Vec4,
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
                shade: Vec4::new(
                    surface.macro_strength,
                    f32::from(surface.soft_depth > 0.0),
                    0.0,
                    0.0,
                ),
            };
        }

        let (base, normal, macro_uri) = texture_set.runtime_uris();
        Self {
            surfaces: table,
            base_color: crate::ground::load_repeat_image(asset_server, base, true),
            normal_material: crate::ground::load_repeat_image(asset_server, normal, false),
            macro_variation: crate::ground::load_repeat_image(asset_server, macro_uri, false),
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
    #[texture(11, dimension = "2d_array")]
    #[sampler(12)]
    wear: Option<Handle<Image>>,
    #[texture(13, dimension = "2d_array")]
    #[sampler(15)]
    print_relief: Option<Handle<Image>>,
    #[texture(14, sample_type = "u_int")]
    print_windows: Option<Handle<Image>>,
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

    // No prepass: MetalFX motion comes from the main pass's depth and the camera.
    fn enable_prepass() -> bool {
        false
    }
}

/// The clipmap textures of one terrain and the material drawing them.
pub struct ClipmapTextures {
    pub layout: ClipmapLayout,
    pub material: Handle<ClipmapTerrainMaterial>,
    surface_table: Vec<ClipmapSurface>,
    surfaces: Handle<ShaderBuffer>,
    heights: Handle<Image>,
    normals: Handle<Image>,
    control: Handle<Image>,
    soft: Handle<Image>,
    wear: Handle<Image>,
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
        let toroidal = || {
            ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                ..default()
            })
        };
        let normals = image(side, side, layers, TextureFormat::Rg16Float, toroidal());
        let soft = image(side, side, 1, TextureFormat::R16Float, toroidal());
        let control_side = control_layout(&layout).side;
        let control = image(
            control_side,
            control_side,
            layers,
            TextureFormat::R32Uint,
            ImageSampler::nearest(),
        );
        let wear = image(
            WEAR_SIDE,
            WEAR_SIDE,
            WEAR_SLOTS as u32,
            TextureFormat::Rgba8Unorm,
            ImageSampler::linear(),
        );
        let state = image(
            STATE_WIDTH,
            1,
            1,
            TextureFormat::Rgba32Uint,
            ImageSampler::nearest(),
        );
        let leaf = layout.spacing(0) as f32;
        let surfaces = buffers.add(ShaderBuffer::new(
            storage_bytes(&ground.surfaces),
            RenderAssetUsages::RENDER_WORLD,
        ));
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
                detail: Vec4::new(ranges.detail as f32, 0.0, 0.0, 0.0),
            },
            heights: Some(heights.clone()),
            normals: Some(normals.clone()),
            state: Some(state.clone()),
            control: Some(control.clone()),
            surfaces: surfaces.clone(),
            base_color: Some(ground.base_color),
            normal_material: Some(ground.normal_material),
            macro_variation: Some(ground.macro_variation),
            wear: Some(wear.clone()),
            print_relief: Some(crate::prints::relief_handle()),
            print_windows: Some(crate::prints::windows_handle()),
        });
        Self {
            layout,
            material,
            surface_table: ground.surfaces,
            surfaces,
            heights,
            normals,
            control,
            soft,
            wear,
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

impl ClipmapTextures {
    /// The soft layer the prints lower, when these textures draw the world.
    pub fn print_ground(&self) -> crate::PrintGround {
        crate::PrintGround {
            soft: self.soft.clone(),
            spacing: self.layout.spacing(0),
            side: self.layout.side,
        }
    }

    /// Marks the surfaces roads wear ground bare to (the layers of their pages' worn slots), which
    /// near the camera follow the fine wear field (`terrain/clipmap.wesl`). True when one was new.
    pub fn mark_bare_ground(
        &mut self,
        layers: impl IntoIterator<Item = u16>,
        buffers: &mut Assets<ShaderBuffer>,
    ) -> bool {
        let mut changed = false;

        for layer in layers {
            if let Some(s) = self.surface_table.get_mut(usize::from(layer))
                && s.material.w < 0.5
            {
                s.material.w = 1.0;
                changed = true;
            }
        }

        if changed && let Some(mut buffer) = buffers.get_mut(&self.surfaces) {
            *buffer = ShaderBuffer::new(
                storage_bytes(&self.surface_table),
                RenderAssetUsages::RENDER_WORLD,
            );
        }

        changed
    }
}

/// One node's samples for its level's block: `intervals + 1` per side, the last row and column
/// landing in the next blocks (the same world samples their own nodes hold). A part left `None`
/// keeps what the block holds.
pub struct NodeBlock {
    pub key: TerrainNodeKey,
    /// Heights, and unit normals' x and z as half floats.
    pub relief: Option<(Vec<f32>, Vec<[u16; 2]>)>,
    /// A leaf's soft layer depths in metres as half floats, zero where it has none.
    pub soft: Option<Vec<u16>>,
    /// Packed control samples, `2 × intervals + 1` per side (`world::CONTROL_SAMPLES`).
    pub control: Option<Vec<u32>>,
}

impl NodeBlock {
    /// A block that rewrites nothing yet.
    pub fn new(key: TerrainNodeKey) -> Self {
        Self {
            key,
            relief: None,
            soft: None,
            control: None,
        }
    }

    pub fn with_relief(mut self, heights: Vec<f32>, normals: &[[f32; 3]]) -> Self {
        let normals = normals
            .iter()
            .map(|n| {
                [
                    half::f16::from_f32(n[0]).to_bits(),
                    half::f16::from_f32(n[2]).to_bits(),
                ]
            })
            .collect();
        self.relief = Some((heights, normals));
        self
    }

    pub fn with_control(mut self, control: Vec<u32>) -> Self {
        self.control = Some(control);
        self
    }

    /// A node's heights and normals, for a clipmap of `intervals` per node.
    pub fn relief_of(node: &world::TerrainNode, intervals: u32) -> Result<Self, String> {
        let field = node
            .heightfield
            .as_ref()
            .ok_or_else(|| format!("terrain node {:?} holds no heights", node.key))?;

        if u32::from(field.resolution) != intervals + 1 {
            return Err(format!(
                "terrain node {:?} is not {}² samples",
                node.key,
                intervals + 1
            ));
        }

        let n = usize::from(field.resolution);
        let normals: Vec<_> = (0..n * n).map(|i| field.normal_at(i % n, i / n)).collect();
        let mut block = Self::new(node.key).with_relief(field.heights.clone(), &normals);

        if node.key.level == 0 {
            let depth = |i: usize| node.soft_depths.get(i).map_or(0.0, |&d| f32::from(d));
            block.soft = Some(
                (0..n * n)
                    .map(|i| half::f16::from_f32(depth(i) * world::TERRAIN_HEIGHT_STEP).to_bits())
                    .collect(),
            );
        }

        Ok(block)
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
    /// The cell whose wear each atlas slot holds.
    pub wear: &'a [Option<world::CellCoord>; WEAR_SLOTS],
}

/// Main-world uploads for the next extraction, a queue per clipmap (by its heights texture),
/// so the streamed terrain and an editor preview each fill their own textures.
#[derive(Resource, Default)]
pub struct ClipmapUploads {
    queues: HashMap<AssetId<Image>, Queue>,
    pub uploaded_blocks: u64,
    /// Leaves whose soft layer was written since the prints last took them: the soft texture
    /// and the leaf's world rect (metres), for the prints to resolve again.
    soft_changes: Vec<(AssetId<Image>, [f64; 4])>,
}

/// One clipmap's uploads.
struct Queue {
    target: Target,
    blocks: Vec<NodeBlock>,
    wear: Vec<(u32, Vec<u8>)>,
    state: Option<Vec<u32>>,
}

impl Queue {
    fn new(target: Target) -> Self {
        Self {
            target,
            blocks: Vec::new(),
            wear: Vec::new(),
            state: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Target {
    heights: AssetId<Image>,
    normals: AssetId<Image>,
    control: AssetId<Image>,
    soft: AssetId<Image>,
    wear: AssetId<Image>,
    state: AssetId<Image>,
    layout: ClipmapLayout,
}

impl ClipmapUploads {
    /// Uploads to these textures start; anything queued for them before is dropped.
    pub fn target(&mut self, textures: &ClipmapTextures) {
        let target = Target {
            heights: textures.heights.id(),
            normals: textures.normals.id(),
            control: textures.control.id(),
            soft: textures.soft.id(),
            wear: textures.wear.id(),
            state: textures.state.id(),
            layout: textures.layout,
        };
        self.queues.insert(target.heights, Queue::new(target));
    }

    /// Uploads to these textures stop.
    pub fn forget(&mut self, textures: &ClipmapTextures) {
        self.queues.remove(&textures.heights.id());
    }

    fn queue(&mut self, textures: &ClipmapTextures) -> Option<&mut Queue> {
        self.queues.get_mut(&textures.heights.id())
    }

    pub fn push(&mut self, textures: &ClipmapTextures, block: NodeBlock) {
        let key = block.key;
        let soft = block.soft.is_some();

        if let Some(queue) = self.queue(textures) {
            queue.blocks.push(block);
            self.uploaded_blocks += 1;

            if soft {
                let cell = textures.layout.spacing(0) * f64::from(textures.layout.intervals);
                let (x, z) = (f64::from(key.x) * cell, f64::from(key.z) * cell);
                let rect = [x, z, x + cell, z + cell];
                self.soft_changes.push((textures.soft.id(), rect));
            }
        }
    }

    /// The leaves whose soft layer changed since last taken.
    pub(crate) fn take_soft_changes(&mut self) -> Vec<(AssetId<Image>, [f64; 4])> {
        std::mem::take(&mut self.soft_changes)
    }

    /// A cell's wear field (`world::TerrainWear`, at full resolution) into atlas slot `slot`.
    pub fn push_wear(
        &mut self,
        textures: &ClipmapTextures,
        slot: usize,
        wear: &world::TerrainWear,
    ) {
        debug_assert_eq!(u32::from(wear.resolution), WEAR_SIDE);

        if let Some(queue) = self.queue(textures) {
            let texels = wear.texels.iter().flatten().copied().collect();
            queue.wear.push((slot as u32, texels));
        }
    }

    pub fn set_frame(&mut self, textures: &ClipmapTextures, frame: &ClipmapFrame) {
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

        for (slot, cell) in frame.wear.iter().enumerate() {
            if let Some(cell) = cell {
                let at = (STATE_WEAR as usize + slot) * 4;
                state[at..at + 3].copy_from_slice(&[cell.x as u32, cell.z as u32, 1]);
            }
        }

        if let Some(queue) = self.queue(textures) {
            queue.state = Some(state);
        }
    }
}

/// The render world's queues, kept until their textures exist.
#[derive(Resource, Default)]
struct RenderUploads(HashMap<AssetId<Image>, Queue>);

pub(crate) fn install(app: &mut App) {
    app.init_resource::<ClipmapUploads>()
        .add_plugins(MaterialPlugin::<ClipmapTerrainMaterial>::default())
        .add_systems(Update, apply_macro_variation);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<RenderUploads>()
            .add_systems(ExtractSchedule, extract)
            .add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

/// The macro-variation switch reaches materials made before and after it changed.
fn apply_macro_variation(
    variation: Res<crate::TerrainMacroVariation>,
    mut materials: ResMut<Assets<ClipmapTerrainMaterial>>,
) {
    let enabled = variation.shader_enabled();
    let stale: Vec<_> = materials
        .iter()
        .filter(|(_, material)| material.settings.ground.y != enabled)
        .map(|(id, _)| id)
        .collect();

    for id in stale {
        if let Some(mut material) = materials.get_mut(id) {
            material.settings.ground.y = enabled;
        }
    }
}

fn extract(mut main: ResMut<MainWorld>, mut render: ResMut<RenderUploads>) {
    let Some(mut uploads) = main.get_resource_mut::<ClipmapUploads>() else {
        return;
    };

    render.0.retain(|id, _| uploads.queues.contains_key(id));

    for (id, queue) in &mut uploads.queues {
        let pending = render
            .0
            .entry(*id)
            .or_insert_with(|| Queue::new(queue.target));

        if pending.target != queue.target {
            *pending = Queue::new(queue.target);
        }

        pending.blocks.append(&mut queue.blocks);
        pending.wear.append(&mut queue.wear);

        if let Some(state) = queue.state.take() {
            pending.state = Some(state);
        }
    }
}

fn upload(
    mut uploads: ResMut<RenderUploads>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
) {
    for pending in uploads.0.values_mut() {
        upload_one(pending, &images, &queue);
    }
}

fn upload_one(uploads: &mut Queue, images: &RenderAssets<GpuImage>, queue: &RenderQueue) {
    let target = uploads.target;

    // Images appear a frame or more after they are added: keep everything until then.
    let (Some(heights), Some(normals), Some(control), Some(soft), Some(wear), Some(state)) = (
        images.get(target.heights),
        images.get(target.normals),
        images.get(target.control),
        images.get(target.soft),
        images.get(target.wear),
        images.get(target.state),
    ) else {
        return;
    };

    for (slot, texels) in uploads.wear.drain(..) {
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &wear.texture,
                mip_level: 0,
                origin: Origin3d {
                    x: 0,
                    y: 0,
                    z: slot,
                },
                aspect: TextureAspect::All,
            },
            &texels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WEAR_SIDE * 4),
                rows_per_image: Some(WEAR_SIDE),
            },
            Extent3d {
                width: WEAR_SIDE,
                height: WEAR_SIDE,
                depth_or_array_layers: 1,
            },
        );
    }

    let layout = target.layout;
    let control_layout = control_layout(&layout);

    for block in uploads.blocks.drain(..) {
        let origin = layout.block_origin(block.key);
        let layer = u32::from(block.key.level);

        if let Some((h, n)) = &block.relief {
            write_block(queue, heights, &layout, origin, layer, bytemuck_f32(h));
            write_block(queue, normals, &layout, origin, layer, bytemuck_u16(n));
        }

        if let Some(depths) = &block.soft {
            let bytes = depths.iter().flat_map(|d| d.to_le_bytes()).collect();
            write_block(queue, soft, &layout, origin, 0, bytes);
        }

        if let Some(samples) = &block.control {
            write_block(
                queue,
                control,
                &control_layout,
                control_layout.block_origin(block.key),
                layer,
                samples.iter().flat_map(|v| v.to_le_bytes()).collect(),
            );
        }
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

/// Mesh tag of a patch, as `terrain/clipmap_vertex.wesl` unpacks it: its level counted from the
/// finest detail level, and its place among patches of its size.
pub fn patch_tag(key: PatchKey) -> u32 {
    let level = u32::from(key.node.level) + u32::from(DETAIL_LEVELS) - u32::from(key.detail);
    let [x, z] = key.grid();
    debug_assert!(level < 16 && (-8192..8192).contains(&x) && (-8192..8192).contains(&z));
    (level << 28) | ((x as u32 & 0x3fff) << 14) | (z as u32 & 0x3fff)
}

/// Where a patch stands for a render origin at `origin` metres: its mesh's sample indices
/// scaled to its spacing.
pub fn patch_transform(layout: &ClipmapLayout, key: PatchKey, origin: [f64; 2]) -> Transform {
    let size = layout.patch_metres(key);
    let spacing = layout.patch_spacing(key) as f32;
    let [x, z] = key.grid();
    Transform::from_xyz(
        (f64::from(x) * size - origin[0]) as f32,
        0.0,
        (f64::from(z) * size - origin[1]) as f32,
    )
    .with_scale(Vec3::new(spacing, 1.0, spacing))
}

/// The wear atlas slot of a cell in the window around the camera, as
/// `terrain/clipmap_common.wesl` finds it.
pub fn wear_slot(cell: world::CellCoord) -> usize {
    let n = WEAR_WINDOW;
    (cell.x.rem_euclid(n) + n * cell.z.rem_euclid(n)) as usize
}

/// The patch every node draws: `intervals + 1` vertices per side at their sample indices, and
/// the diagonal from each quad's (x, z) to (x + 1, z + 1), as the coarser grid has it.
pub fn patch_mesh(layout: &ClipmapLayout) -> Mesh {
    let n = layout.intervals + 1;
    let positions: Vec<[f32; 3]> = (0..n * n)
        .map(|i| [(i % n) as f32, 0.0, (i / n) as f32])
        .collect();
    // Quads in 8×8 blocks, row by row within each: consecutive rows then share vertices a
    // small post-transform cache still holds.
    const BLOCK: u32 = 8;
    let end = n - 1;
    let mut indices = Vec::with_capacity((end * end * 6) as usize);

    for block_z in (0..end).step_by(BLOCK as usize) {
        for block_x in (0..end).step_by(BLOCK as usize) {
            for z in block_z..(block_z + BLOCK).min(end) {
                for x in block_x..(block_x + BLOCK).min(end) {
                    let [a, b, c, d] = [
                        z * n + x,
                        (z + 1) * n + x,
                        (z + 1) * n + x + 1,
                        z * n + x + 1,
                    ];
                    indices.extend([a, b, c, a, c, d]);
                }
            }
        }
    }

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
    fn patch_tags_carry_negative_nodes_and_detail_parts() {
        let node = |level, x, z| TerrainNodeKey {
            space: world::WorldSpaceId(1),
            level,
            x,
            z,
        };
        let tag = patch_tag(PatchKey::node(node(5, -5, 4)));
        assert_eq!(tag >> 28, 5 + u32::from(DETAIL_LEVELS));
        assert_eq!(((tag << 4) as i32) >> 18, -5);
        assert_eq!(((tag << 18) as i32) >> 18, 4);
        let part = PatchKey {
            node: node(0, -1, 2),
            detail: 2,
            part: [3, 1],
        };
        let tag = patch_tag(part);
        assert_eq!(tag >> 28, u32::from(DETAIL_LEVELS) - 2);
        assert_eq!(((tag << 4) as i32) >> 18, -4 + 3);
        assert_eq!(((tag << 18) as i32) >> 18, 8 + 1);
    }

    /// The shaders keep their own copies of the state texture's layout.
    #[test]
    fn shader_constants_match_the_state_layout() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/shaders/terrain/clipmap_common.wesl"
        ));
        let value = |name: &str| -> i64 {
            let line = source
                .lines()
                .find(|l| l.starts_with(&format!("const {name}: i32 = ")))
                .unwrap_or_else(|| panic!("{name} is not declared"));
            line.rsplit(' ')
                .next()
                .unwrap()
                .trim_end_matches(';')
                .parse()
                .unwrap()
        };
        assert_eq!(value("STATE_WINDOWS"), i64::from(STATE_WINDOWS));
        assert_eq!(value("STATE_RESIDENCY"), i64::from(STATE_RESIDENCY));
        assert_eq!(value("RESIDENCY_TEXELS"), i64::from(RESIDENCY_TEXELS));
        assert_eq!(value("STATE_WEAR"), i64::from(STATE_WEAR));
        assert_eq!(value("WEAR_WINDOW"), i64::from(WEAR_WINDOW));
        assert_eq!(value("DETAIL_LEVELS"), i64::from(DETAIL_LEVELS));
    }

    #[test]
    fn leaves_upload_their_soft_layer_in_metres_and_clear_it_without_one() {
        let key = TerrainNodeKey::leaf(world::WorldSpaceId(1), world::CellCoord { x: 0, z: 0 });
        let field = world::TerrainHeightfield::from_heights(3, &[1.0; 9], 0.0, 2.0, 32.0).unwrap();
        let firm = world::TerrainNode::leaf(key, &field, 3).unwrap();
        let layer = world::SoftLayer {
            depths: vec![410; 9],
            slopes: vec![[0.0; 2]; 9],
        };
        let covered = firm.clone().with_soft_layer(layer).unwrap();
        let depth =
            |node: &world::TerrainNode| NodeBlock::relief_of(node, 2).unwrap().soft.unwrap()[4];
        let metres = half::f16::from_bits(depth(&covered)).to_f32();
        assert!((metres - 0.4).abs() < 0.001, "{metres}");
        assert_eq!(
            depth(&firm),
            0,
            "a leaf without one clears what its texels held"
        );
    }

    #[test]
    fn wrapped_blocks_split_into_spans_that_cover_them_once() {
        assert_eq!(spans(0, 33, 1024).collect::<Vec<_>>(), [(0, 0, 33)]);
        assert_eq!(
            spans(992, 33, 1024).collect::<Vec<_>>(),
            [(992, 0, 32), (0, 32, 1)]
        );
    }
}
