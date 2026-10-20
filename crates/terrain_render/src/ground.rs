//! What the ground is drawn with besides the clipmap: the world's surfaces by texture layer,
//! their textures, and the macro-variation switch.
use bevy::{
    image::{
        ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
        ImageSamplerDescriptor,
    },
    prelude::*,
};

use world::TerrainSurface;

/// A ground surface, the texture array layer it is drawn from, and its grass.
#[derive(Clone, Debug)]
pub struct TerrainSurfaceLayer {
    pub surface: TerrainSurface,
    pub layer: u16,
    /// The grass on this surface (`vegetation::VegetationCatalog::cover`): its field's linear
    /// colour and how fully it covers the ground, 0..1. Ground under dense grass is shaded by
    /// the field's colour.
    pub grass: [f32; 4],
}

/// Whether the ground's large-scale colour variation is drawn (F1 compares it off).
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

/// A ground texture, repeated and filtered for oblique views.
pub(crate) fn load_repeat_image(
    asset_server: &AssetServer,
    uri: &str,
    is_srgb: bool,
) -> Handle<Image> {
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
