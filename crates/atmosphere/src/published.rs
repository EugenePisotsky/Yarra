//! What the world-derived maps (valley mist, shore, forest and terrain shadows) share: the
//! published map with its revision, and their half-float images.
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

/// A map published to the shaders, or none. The revision changes with it, so its image is
/// rewritten only then (`environment::publish`).
#[derive(Debug)]
pub struct Published<T> {
    map: Option<T>,
    revision: u64,
}

impl<T> Default for Published<T> {
    fn default() -> Self {
        Self {
            map: None,
            revision: 0,
        }
    }
}

impl<T> Published<T> {
    pub fn publish(&mut self, map: T) {
        self.map = Some(map);
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn disable(&mut self) {
        if self.map.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn map(&self) -> Option<&T> {
        self.map.as_ref()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// RGBA texels as half floats, the format of every map.
pub(crate) fn half_floats(texels: &[[f32; 4]]) -> Vec<u8> {
    let bits: Vec<u16> = texels
        .iter()
        .flatten()
        .map(|&v| half::f16::from_f32(v).to_bits())
        .collect();
    bytemuck::cast_slice(&bits).to_vec()
}

/// Maps are sampled linearly and never repeat.
pub(crate) fn clamped_linear() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    }
}

/// One texel of `texel` until a map replaces it.
pub(crate) fn placeholder(texel: [f32; 4]) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        half_floats(&[texel]),
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(clamped_linear());
    image
}

/// Resizes `image` to a map's `size` and `texels`. The size changes with the world, so Bevy
/// makes a new texture.
pub(crate) fn fill(image: &mut Image, size: UVec2, texels: &[[f32; 4]]) {
    image.texture_descriptor.size = Extent3d {
        width: size.x,
        height: size.y,
        depth_or_array_layers: 1,
    };
    image.data = Some(half_floats(texels));
}
