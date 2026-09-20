//! Lit, world-projected ground composites. A material tile may cover many mesh LODs.
use super::*;
use world::{TERRAIN_COMPOSITE_MIPS, TerrainComposite, TerrainMaterialKey};

pub const COMPOSITE_SHADER: &str = "shaders/terrain_composite.wgsl";
pub mod atlas;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct DetailProjection {
    /// Origin cell XZ, table mask, enabled.
    origin: IVec4,
    /// Cell size and material root level.
    world: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
#[bind_group_data(CompositePipelineKey)]
pub struct TerrainCompositeMaterial {
    #[storage(120, read_only)]
    pub(super) cloud_parameters: Handle<ShaderBuffer>,
    #[texture(121)]
    #[sampler(122)]
    pub(super) cloud_shadows: Option<Handle<Image>>,
    pub key: Option<TerrainMaterialKey>,
    pub shading_mode: TerrainShadingMode,
    /// Diagnostic shader bypass only; retains near-source residency for a fair A/B.
    pub near_disabled: bool,
    /// Render-relative minimum XZ, extent, then whether baked maps are present.
    #[uniform(0)]
    projection: Vec4,
    #[texture(1)]
    #[sampler(2)]
    color: Option<Handle<Image>>,
    #[texture(3)]
    response: Option<Handle<Image>>,
    #[uniform(4)]
    detail_projection: DetailProjection,
    #[texture(5, dimension = "2d_array")]
    detail_color: Option<Handle<Image>>,
    #[texture(6, dimension = "2d_array")]
    detail_response: Option<Handle<Image>>,
    #[storage(7, read_only)]
    detail_table: Handle<ShaderBuffer>,
    #[texture(8, dimension = "2d_array")]
    near_weights: Option<Handle<Image>>,
    #[texture(9, dimension = "2d_array")]
    near_canopy: Option<Handle<Image>>,
    #[storage(10, read_only)]
    near_table: Handle<ShaderBuffer>,
    #[uniform(11)]
    near_settings: Vec4,
    #[texture(12, dimension = "2d_array")]
    #[sampler(15)]
    near_base: Option<Handle<Image>>,
    #[texture(13, dimension = "2d_array")]
    near_normal: Option<Handle<Image>>,
    #[texture(14)]
    near_macro: Option<Handle<Image>>,
    #[texture(16, dimension = "2d_array")]
    near_prepared: Option<Handle<Image>>,
}

impl Default for TerrainCompositeMaterial {
    fn default() -> Self {
        Self {
            cloud_parameters: atmosphere::clouds::fallback_parameters(),
            cloud_shadows: None,
            key: None,
            shading_mode: Default::default(),
            near_disabled: false,
            projection: Default::default(),
            color: None,
            response: None,
            detail_projection: Default::default(),
            detail_color: None,
            detail_response: None,
            detail_table: Default::default(),
            near_weights: None,
            near_canopy: None,
            near_table: Default::default(),
            near_settings: Default::default(),
            near_base: None,
            near_normal: None,
            near_macro: None,
            near_prepared: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompositePipelineKey {
    shading: TerrainShadingMode,
    near_disabled: bool,
}
impl From<&TerrainCompositeMaterial> for CompositePipelineKey {
    fn from(material: &TerrainCompositeMaterial) -> Self {
        Self {
            shading: material.shading_mode,
            near_disabled: material.near_disabled,
        }
    }
}

impl TerrainCompositeMaterial {
    /// A committed authoring revision must not sample old detail/near control tiles.
    /// The newly baked coarse material remains valid while those caches refill.
    pub fn clear_streamed_inputs(&mut self) {
        self.clear_near();
        self.detail_color = None;
        self.detail_response = None;
        self.detail_table = default();
        self.detail_projection.origin.w = 0;
    }
    pub fn from_composite(
        tile: TerrainComposite,
        origin: CellCoord,
        cell_size: f32,
        images: &mut Assets<Image>,
    ) -> Result<Self, String> {
        tile.validate().map_err(|e| e.to_string())?;
        if !cell_size.is_finite() || cell_size <= 0. {
            return Err("invalid composite cell size".into());
        }
        let mut color = Vec::with_capacity(TerrainComposite::gpu_bytes() / 2);
        let mut response = Vec::with_capacity(TerrainComposite::gpu_bytes() / 2);
        for mip in tile.mips {
            color.extend(mip.color);
            response.extend(mip.response);
        }
        let mut material = Self {
            key: Some(tile.key),
            color: Some(images.add(composite_image(color, true))),
            response: Some(images.add(composite_image(response, false))),
            ..default()
        };
        material.set_origin(origin, cell_size);
        Ok(material)
    }

    pub fn set_origin(&mut self, origin: CellCoord, cell_size: f32) {
        let Some(key) = self.key else {
            return;
        };
        let span = 1_i64 << key.0.level;
        self.detail_projection.origin.x = origin.x;
        self.detail_projection.origin.y = origin.z;
        self.detail_projection.world = Vec4::new(cell_size, key.0.level as f32, 0., 0.);
        self.projection = Vec4::new(
            ((i64::from(key.0.x) * span - i64::from(origin.x)) as f64 * f64::from(cell_size))
                as f32,
            ((i64::from(key.0.z) * span - i64::from(origin.z)) as f64 * f64::from(cell_size))
                as f32,
            (span as f64 * f64::from(cell_size)) as f32,
            1.,
        );
    }

    pub(crate) fn clear_near(&mut self) {
        self.near_weights = None;
        self.near_canopy = None;
        self.near_table = default();
        self.near_base = None;
        self.near_normal = None;
        self.near_macro = None;
        self.near_prepared = None;
        self.near_settings = Vec4::ZERO;
    }
    pub(crate) fn has_near(&self, atlas: &crate::near::gpu::NearAtlas) -> bool {
        self.near_table == atlas.table
    }
    pub(crate) fn set_near(&mut self, atlas: &crate::near::gpu::NearAtlas) {
        self.near_weights = Some(atlas.weights.clone());
        self.near_canopy = Some(atlas.canopy.clone());
        self.near_table = atlas.table.clone();
        self.near_base = Some(atlas.pack.base.clone());
        self.near_normal = Some(atlas.pack.normal.clone());
        self.near_macro = Some(atlas.pack.macro_image.clone());
        self.near_prepared = atlas.pack.prepared.clone();
        self.near_settings = Vec4::new(crate::near::NEAR_START, crate::near::NEAR_END, 1., 0.);
    }

    pub fn set_detail_atlas(&mut self, atlas: &atlas::DetailAtlas) {
        assert!(atlas.ready());
        self.detail_color = Some(atlas.color.clone());
        self.detail_response = Some(atlas.response.clone());
        self.detail_table = atlas.table.clone();
        self.detail_projection.origin.z = (atlas::TABLE_SIZE - 1) as i32;
        self.detail_projection.origin.w = 1;
    }
}

fn composite_image(data: Vec<u8>, srgb: bool) -> Image {
    let size = TerrainComposite::mip_size(0) as u32;
    let mut image = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        // Release the main-world pixel copy after extraction. The material owns residency.
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = TERRAIN_COMPOSITE_MIPS as u32;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        lod_max_clamp: (TERRAIN_COMPOSITE_MIPS - 1) as f32,
        ..default()
    });
    image
}

impl Material for TerrainCompositeMaterial {
    fn fragment_shader() -> ShaderRef {
        COMPOSITE_SHADER.into()
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(fragment) = descriptor.fragment.as_mut() {
            let define = match key.bind_group_data.shading {
                TerrainShadingMode::Production => None,
                TerrainShadingMode::SurfaceUnlit => Some("TERRAIN_SURFACE_UNLIT"),
                TerrainShadingMode::SingleTexture => Some("TERRAIN_SINGLE_TEXTURE"),
                TerrainShadingMode::Flat => Some("TERRAIN_FLAT"),
            };
            if let Some(define) = define {
                fragment.shader_defs.push(define.into());
            }
            if key.bind_group_data.near_disabled {
                fragment.shader_defs.push("TERRAIN_NEAR_DISABLED".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
