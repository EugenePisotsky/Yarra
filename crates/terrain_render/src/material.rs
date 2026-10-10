//! The page terrain material. Drawn directly, it shades the first two of a page's surfaces
//! blended by a page-local weight map, with macro variation, anti-tiling and canopy shading.
use bevy::{
    mesh::MeshVertexBufferLayoutRef,
    pbr::{Material, MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    },
    render::storage::ShaderBuffer,
    shader::ShaderRef,
};

pub(crate) const TERRAIN_SHADER: &str = "shaders/terrain_material.wesl";

pub(crate) fn apply_macro_variation(
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

    pub fn toggled(self) -> Self {
        match self {
            Self::Disabled => Self::Enabled,
            Self::Enabled => Self::Disabled,
        }
    }

    pub(crate) fn shader_enabled(self) -> f32 {
        match self {
            Self::Disabled => 0.0,
            Self::Enabled => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub(crate) struct TerrainMaterialUniform {
    pub(crate) chunk_minimum: Vec2,
    pub(crate) chunk_extent: Vec2,
    /// Array layers for slots 0 and 1, surface count, then prepared albedo period.
    pub(crate) surface_layers: Vec4,
    /// Metres per texture repetition for slots 0 and 1.
    pub(crate) tile_sizes: Vec4,
    /// Slot 0 sign/strength followed by slot 1 sign/strength.
    pub(crate) normal_settings: Vec4,
    /// Slot 0 minimum/maximum followed by slot 1 minimum/maximum.
    pub(crate) roughness_ranges: Vec4,
    /// Small, medium and large scale in metres, followed by albedo strength.
    pub(crate) macro_scales: Vec4,
    /// Contrast, macro enabled, then anti-tiling flags for slots 0 and 1.
    pub(crate) macro_settings: Vec4,
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
    pub(crate) environment: Handle<ShaderBuffer>,
    #[texture(121)]
    #[sampler(122)]
    pub(crate) cloud_shadows: Option<Handle<Image>>,
    #[texture(123, sample_type = "float", filterable = false)]
    pub(crate) rain_shelter: Option<Handle<Image>>,
    #[texture(124)]
    #[sampler(125)]
    pub(crate) forest_shadow: Option<Handle<Image>>,
    pub shading_mode: TerrainShadingMode,
    #[uniform(0)]
    pub(crate) settings: TerrainMaterialUniform,
    #[texture(1)]
    #[sampler(2)]
    pub(crate) weights: Handle<Image>,
    #[texture(3, dimension = "2d_array")]
    #[sampler(4)]
    pub(crate) base_color_array: Handle<Image>,
    #[texture(5, dimension = "2d_array")]
    pub(crate) normal_material_array: Handle<Image>,
    #[texture(6)]
    pub(crate) macro_variation: Handle<Image>,
    #[uniform(11)]
    pub(crate) canopy_shading: TerrainCanopyShading,
    #[uniform(8)]
    pub(crate) canopy_bounds: Vec4,
    #[texture(9)]
    #[sampler(10)]
    pub(crate) canopy_coverage: Option<Handle<Image>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub(crate) struct TerrainCanopyShading {
    pub(crate) appearance: Vec4,
    pub(crate) shape: Vec4,
    pub(crate) distance: Vec4,
    pub(crate) origin: Vec4,
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

#[cfg(test)]
impl TerrainMaterial {
    /// Default handles and the environment's fallback inputs around `settings`, for tests.
    pub(crate) fn for_tests(settings: TerrainMaterialUniform) -> Self {
        Self {
            source_only: false,
            environment: atmosphere::environment::fallback_parameters(),
            cloud_shadows: None,
            rain_shelter: None,
            forest_shadow: None,
            shading_mode: TerrainShadingMode::Production,
            weights: default(),
            base_color_array: default(),
            normal_material_array: default(),
            macro_variation: default(),
            canopy_bounds: Vec4::ZERO,
            canopy_shading: default(),
            canopy_coverage: None,
            settings,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerrainMaterialKey {
    canopy: bool,
    shading: TerrainShadingMode,
}

impl From<&TerrainMaterial> for TerrainMaterialKey {
    fn from(material: &TerrainMaterial) -> Self {
        Self {
            shading: material.shading_mode,
            canopy: material.canopy_coverage.is_some(),
        }
    }
}

impl Material for TerrainMaterial {
    // Terrain does not move: MetalFX Temporal's motion for it comes from the final depth and
    // the camera (`upscaling::temporal::CompleteTemporalMotion`), and the main pass writes its
    // depth, so it is not drawn twice.
    fn enable_prepass() -> bool {
        false
    }

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
