pub mod composite;
pub mod lod;
pub mod near;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef};
use bevy::{
    asset::RenderAssetUsages,
    image::{
        ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
        ImageSamplerDescriptor,
    },
    pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, Extent3d, PrimitiveTopology, RenderPipelineDescriptor, ShaderType,
        SpecializedMeshPipelineError, TextureDimension, TextureFormat,
    },
    render::storage::ShaderBuffer,
    shader::ShaderRef,
};
pub use composite::TerrainCompositeMaterial;
use world::{
    CellCoord, TerrainHeightfield, TerrainProfile, TerrainSurface, TerrainSurfaceId,
    TerrainTextureSet, TerrainWeightPage,
};

mod stochastic_cache;
pub use stochastic_cache::{TerrainCacheSettings, TerrainCacheStats};
mod prepared;
pub use prepared::{TerrainPreparedSettings, TerrainPreparedStats};

/// Schedule terrain material edits before this set to prepare their current inputs.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainMaterialPreparation;

const TERRAIN_SHADER: &str = "shaders/terrain_material.wgsl";

/// Installs the small, page-oriented terrain material renderer.
///
/// The world database and cooker support up to eight authored surface slots.
/// This first GPU implementation intentionally specializes in one or two
/// surfaces so its cost remains visible and understandable while the editor is
/// still being built.
pub struct TerrainRenderPlugin;

impl Plugin for TerrainRenderPlugin {
    fn build(&self, app: &mut App) {
        stochastic_cache::install(app);
        prepared::install(app);
        composite::atlas::install(app);
        near::install(app);
        app.init_resource::<TerrainMacroVariation>()
            .add_systems(Startup, atmosphere::clouds::init_fallback)
            .add_systems(PostUpdate, sync_cloud_inputs)
            .add_plugins(MaterialPlugin::<TerrainMaterial>::default())
            .add_plugins(MaterialPlugin::<TerrainCompositeMaterial>::default())
            .add_systems(
                Update,
                (toggle_macro_variation, apply_macro_variation).chain(),
            );
    }
}

fn toggle_macro_variation(
    keys: Res<ButtonInput<KeyCode>>,
    mut variation: ResMut<TerrainMacroVariation>,
) {
    if !keys.just_pressed(KeyCode::KeyV) {
        return;
    }

    *variation = variation.toggled();
    info!("terrain macro variation: {}", variation.label());
}

fn apply_macro_variation(
    variation: Res<TerrainMacroVariation>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    if !variation.is_changed() {
        return;
    }
    for (_, material) in materials.iter_mut() {
        material.settings.macro_settings.y = variation.shader_enabled();
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerrainMacroVariation {
    Disabled,
    #[default]
    Enabled,
}

impl TerrainMacroVariation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "off",
            Self::Enabled => "on",
        }
    }

    fn toggled(self) -> Self {
        match self {
            Self::Disabled => Self::Enabled,
            Self::Enabled => Self::Disabled,
        }
    }

    fn shader_enabled(self) -> f32 {
        match self {
            Self::Disabled => 0.0,
            Self::Enabled => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub struct TerrainMaterialUniform {
    chunk_minimum: Vec2,
    chunk_extent: Vec2,
    /// Array layers for slots 0 and 1, surface count, then prepared albedo period.
    surface_layers: Vec4,
    /// Metres per texture repetition for slots 0 and 1.
    tile_sizes: Vec4,
    /// Slot 0 sign/strength followed by slot 1 sign/strength.
    normal_settings: Vec4,
    /// Slot 0 minimum/maximum followed by slot 1 minimum/maximum.
    roughness_ranges: Vec4,
    /// Small, medium and large scale in metres, followed by albedo strength.
    macro_scales: Vec4,
    /// Contrast, macro enabled, then anti-tiling flags for slots 0 and 1.
    macro_settings: Vec4,
    /// Integer lattice origins for the two anti-tiling lookup layers.
    cache_origins: Vec4,
    cache_size: UVec4,
}

/// Separate compiled fragment variants for measuring terrain costs without changing geometry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TerrainShadingMode {
    #[default]
    Production,
    SurfaceUnlit,
    SingleTexture,
    Flat,
}

impl TerrainShadingMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::SurfaceUnlit => "surface unlit",
            Self::SingleTexture => "one texture",
            Self::Flat => "flat unlit",
        }
    }
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
#[bind_group_data(TerrainMaterialKey)]
pub struct TerrainMaterial {
    /// Input carrier for hierarchy shading; owns no drawn mesh or prepared control cache.
    pub source_only: bool,
    // Shading-only buffers must not consume vertex slots in the motion prepass.
    // On Metal, that can collide with the vertex buffer's reserved binding.
    #[storage(120, read_only, visibility(fragment))]
    cloud_parameters: Handle<ShaderBuffer>,
    #[texture(121)]
    #[sampler(122)]
    cloud_shadows: Option<Handle<Image>>,
    pub shading_mode: TerrainShadingMode,
    stochastic_cached: bool,
    prepared: bool,
    prepared_albedo: bool,
    source_weights: Handle<Image>,
    source_base_color_array: Handle<Image>,
    #[uniform(0)]
    settings: TerrainMaterialUniform,
    #[texture(1)]
    #[sampler(2)]
    weights: Handle<Image>,
    #[texture(3, dimension = "2d_array")]
    #[sampler(4)]
    base_color_array: Handle<Image>,
    #[texture(5, dimension = "2d_array")]
    normal_material_array: Handle<Image>,
    #[texture(6)]
    macro_variation: Handle<Image>,
    #[storage(7, read_only, visibility(fragment))]
    stochastic_cache: Handle<ShaderBuffer>,
    #[uniform(11)]
    canopy_shading: TerrainCanopyShading,
    #[uniform(8)]
    canopy_bounds: Vec4,
    #[texture(9)]
    #[sampler(10)]
    canopy_coverage: Option<Handle<Image>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct TerrainCanopyShading {
    appearance: Vec4,
    shape: Vec4,
    distance: Vec4,
    origin: Vec4,
}
impl From<[[f32; 4]; 4]> for TerrainCanopyShading {
    fn from(v: [[f32; 4]; 4]) -> Self {
        Self {
            appearance: v[0].into(),
            shape: v[1].into(),
            distance: v[2].into(),
            origin: v[3].into(),
        }
    }
}
impl TerrainMaterial {
    pub fn set_canopy_shading(&mut self, packed: [[f32; 4]; 4]) {
        self.canopy_shading = packed.into();
    }

    /// Static grass cover in render-space XZ; visibility is independent of render LOD/wind.
    pub fn set_canopy_coverage(&mut self, coverage: Option<Handle<Image>>, bounds: Vec4) {
        self.canopy_coverage = coverage;
        self.canopy_bounds = bounds;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerrainMaterialKey {
    canopy: bool,
    shading: TerrainShadingMode,
    stochastic_cached: bool,
    prepared: bool,
    prepared_albedo: bool,
}

impl From<&TerrainMaterial> for TerrainMaterialKey {
    fn from(material: &TerrainMaterial) -> Self {
        Self {
            shading: material.shading_mode,
            canopy: material.canopy_coverage.is_some(),
            stochastic_cached: material.stochastic_cached,
            prepared: material.prepared,
            prepared_albedo: material.prepared_albedo,
        }
    }
}

impl Material for TerrainMaterial {
    fn fragment_shader() -> ShaderRef {
        TERRAIN_SHADER.into()
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.canopy
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("TERRAIN_CANOPY".into());
        }
        if key.bind_group_data.prepared
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("TERRAIN_PREPARED".into());
        }
        if key.bind_group_data.prepared_albedo {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("TERRAIN_PREPARED_ALBEDO".into());
            }
        } else if key.bind_group_data.stochastic_cached
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment
                .shader_defs
                .push("TERRAIN_STOCHASTIC_CACHED".into());
        }
        let define = match key.bind_group_data.shading {
            TerrainShadingMode::Production => None,
            TerrainShadingMode::SurfaceUnlit => Some("TERRAIN_SURFACE_UNLIT"),
            TerrainShadingMode::SingleTexture => Some("TERRAIN_SINGLE_TEXTURE"),
            TerrainShadingMode::Flat => Some("TERRAIN_FLAT"),
        };
        if let (Some(fragment), Some(define)) = (descriptor.fragment.as_mut(), define) {
            fragment.shader_defs.push(define.into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct TerrainSurfaceLayer {
    pub surface: TerrainSurface,
    pub layer: u16,
}

pub struct PreparedTerrainMaterial {
    pub material: Handle<TerrainMaterial>,
    /// Page-local generated image. Remove it when the page leaves residency.
    pub weight_image: Handle<Image>,
}

pub struct PrepareTerrainMaterialContext<'a> {
    pub asset_server: &'a AssetServer,
    pub images: &'a mut Assets<Image>,
    pub materials: &'a mut Assets<TerrainMaterial>,
    pub cell: CellCoord,
    /// The logical cell represented by render-space origin.
    pub origin_cell: CellCoord,
    pub cell_size: f32,
    pub page_surfaces: &'a [TerrainSurfaceId],
    pub weight_pages: &'a [TerrainWeightPage],
    pub profile: &'a TerrainProfile,
    pub texture_set: &'a TerrainTextureSet,
    /// Must be in the exact order stored by `page_surfaces`.
    pub surfaces: &'a [TerrainSurfaceLayer],
    pub macro_variation: TerrainMacroVariation,
}

pub fn prepare_terrain_material(
    context: PrepareTerrainMaterialContext<'_>,
) -> Result<PreparedTerrainMaterial, String> {
    if !(1..=2).contains(&context.surfaces.len())
        || context.page_surfaces.len() != context.surfaces.len()
    {
        return Err(format!(
            "the initial terrain renderer supports one or two surfaces, got {}",
            context.surfaces.len()
        ));
    }
    if context.cell_size <= 0.0 || !context.cell_size.is_finite() {
        return Err("terrain cell size must be finite and positive".into());
    }

    let weight_image = context.images.add(make_weight_image(
        context.page_surfaces,
        context.weight_pages,
    )?);
    let (base_color_uri, normal_material_uri, macro_variation_uri) =
        context.texture_set.runtime_uris();
    let base_color_array = load_repeat_image(context.asset_server, base_color_uri, true);
    let normal_material_array = load_repeat_image(context.asset_server, normal_material_uri, false);
    let macro_variation = load_repeat_image(context.asset_server, macro_variation_uri, false);

    let first = &context.surfaces[0];
    let second = context.surfaces.get(1).unwrap_or(first);
    let material = context.materials.add(TerrainMaterial {
        source_only: false,
        cloud_parameters: atmosphere::clouds::fallback_parameters(),
        cloud_shadows: None,
        shading_mode: TerrainShadingMode::Production,
        stochastic_cached: false,
        prepared: false,
        prepared_albedo: false,
        source_weights: weight_image.clone(),
        source_base_color_array: base_color_array.clone(),
        stochastic_cache: Handle::default(),
        canopy_bounds: Vec4::ZERO,
        canopy_shading: Default::default(),
        canopy_coverage: None,
        settings: TerrainMaterialUniform {
            cache_origins: Vec4::ZERO,
            cache_size: UVec4::ZERO,
            chunk_minimum: Vec2::new(
                (i64::from(context.cell.x) - i64::from(context.origin_cell.x)) as f32
                    * context.cell_size,
                (i64::from(context.cell.z) - i64::from(context.origin_cell.z)) as f32
                    * context.cell_size,
            ),
            chunk_extent: Vec2::splat(context.cell_size),
            surface_layers: Vec4::new(
                first.layer as f32,
                second.layer as f32,
                context.surfaces.len() as f32,
                0.0,
            ),
            tile_sizes: Vec4::new(first.surface.tile_size, second.surface.tile_size, 0.0, 0.0),
            normal_settings: Vec4::new(
                first.surface.normal_y_sign,
                first.surface.normal_strength,
                second.surface.normal_y_sign,
                second.surface.normal_strength,
            ),
            roughness_ranges: Vec4::new(
                first.surface.roughness_min,
                first.surface.roughness_max,
                second.surface.roughness_min,
                second.surface.roughness_max,
            ),
            macro_scales: Vec4::new(
                context.profile.macro_scales[0],
                context.profile.macro_scales[1],
                context.profile.macro_scales[2],
                context.profile.macro_albedo_strength,
            ),
            macro_settings: Vec4::new(
                context.profile.macro_contrast,
                context.macro_variation.shader_enabled(),
                f32::from(first.surface.anti_tiling),
                f32::from(second.surface.anti_tiling),
            ),
        },
        weights: weight_image.clone(),
        base_color_array,
        normal_material_array,
        macro_variation,
    });
    Ok(PreparedTerrainMaterial {
        material,
        weight_image,
    })
}

fn make_weight_image(
    surfaces: &[TerrainSurfaceId],
    weight_pages: &[TerrainWeightPage],
) -> Result<Image, String> {
    let (resolution, rgba) = if surfaces.len() == 1 {
        (1_u32, vec![255, 0, 0, 0])
    } else {
        let Some(weights) = weight_pages.first() else {
            return Err("blended terrain page has no weight map".into());
        };
        let expected = usize::from(weights.resolution).pow(2) * 4;
        if weights.resolution < 2 || weights.rgba.len() != expected {
            return Err("terrain weight map has invalid dimensions".into());
        }
        (u32::from(weights.resolution), weights.rgba.clone())
    };
    let mut image = Image::new(
        Extent3d {
            width: resolution,
            height: resolution,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    Ok(image)
}

/// Builds the page-local raster mesh from the same height samples used by CPU surface queries.
///
/// X/Z positions are centred around the page entity. Heights remain absolute world-space Y so the
/// entity only needs render-origin translation in X/Z.
pub fn build_heightfield_mesh(
    heightfield: &TerrainHeightfield,
    cell_size: f32,
) -> Result<Mesh, String> {
    heightfield.validate().map_err(|error| error.to_string())?;
    if !cell_size.is_finite() || cell_size <= 0.0 {
        return Err("terrain cell size must be finite and positive".into());
    }

    let resolution = usize::from(heightfield.resolution);
    let intervals = (resolution - 1) as f32;
    let mut positions = Vec::with_capacity(resolution * resolution);
    let mut normals = Vec::with_capacity(resolution * resolution);
    let mut uvs = Vec::with_capacity(resolution * resolution);
    for z in 0..resolution {
        for x in 0..resolution {
            let u = x as f32 / intervals;
            let v = z as f32 / intervals;
            positions.push([
                u * cell_size - cell_size * 0.5,
                heightfield.height_at(x, z),
                v * cell_size - cell_size * 0.5,
            ]);
            normals.push(heightfield.normal_at(x, z));
            uvs.push([u, v]);
        }
    }

    let mut indices = Vec::with_capacity((resolution - 1) * (resolution - 1) * 6);
    for z in 0..resolution - 1 {
        for x in 0..resolution - 1 {
            let lower_left = (z * resolution + x) as u32;
            let lower_right = lower_left + 1;
            let upper_left = lower_left + resolution as u32;
            let upper_right = upper_left + 1;
            indices.extend_from_slice(&[
                lower_left,
                upper_left,
                upper_right,
                lower_left,
                upper_right,
                lower_right,
            ]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh.generate_tangents()
        .map_err(|error| format!("could not generate terrain tangents: {error}"))?;
    Ok(mesh)
}

fn load_repeat_image(asset_server: &AssetServer, uri: &str, is_srgb: bool) -> Handle<Image> {
    asset_server
        .load_builder()
        .with_settings(move |settings: &mut ImageLoaderSettings| {
            settings.is_srgb = is_srgb;
            settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                mipmap_filter: ImageFilterMode::Linear,
                anisotropy_clamp: 8,
                ..default()
            });
        })
        .load(uri.to_owned())
}

#[cfg(test)]
mod gpu_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heightfield_mesh_has_expected_topology_and_tangents() {
        let heightfield = TerrainHeightfield::from_heights(
            3,
            &[0.0, 0.5, 1.0, 0.0, 0.5, 1.0, 0.0, 0.5, 1.0],
            0.0,
            1.0,
            8.0,
        )
        .unwrap();
        let mesh = build_heightfield_mesh(&heightfield, 8.0).unwrap();

        assert_eq!(mesh.count_vertices(), 9);
        assert_eq!(mesh.indices().unwrap().len(), 24);
        assert!(mesh.attribute(Mesh::ATTRIBUTE_TANGENT).is_some());
    }
}

fn sync_cloud_inputs(
    clouds: Option<Res<atmosphere::clouds::CloudAssets>>,
    mut near: ResMut<Assets<TerrainMaterial>>,
    mut far: ResMut<Assets<TerrainCompositeMaterial>>,
) {
    let Some(clouds) = clouds else {
        return;
    };
    let ids: Vec<_> = near
        .iter()
        .filter(|(_, m)| m.cloud_parameters != clouds.parameters)
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        let mut m = near.get_mut(id).unwrap();
        m.cloud_parameters = clouds.parameters.clone();
        m.cloud_shadows = Some(clouds.shadows.clone());
    }
    let ids: Vec<_> = far
        .iter()
        .filter(|(_, m)| m.cloud_parameters != clouds.parameters)
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        let mut m = far.get_mut(id).unwrap();
        m.cloud_parameters = clouds.parameters.clone();
        m.cloud_shadows = Some(clouds.shadows.clone());
    }
}
