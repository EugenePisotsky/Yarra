pub(crate) mod linear;
#[cfg(all(feature = "metalfx", any(target_os = "macos", target_os = "ios")))]
mod metalfx;

#[cfg(not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))))]
use bevy::render::{renderer::RenderDevice, texture::GpuImage};
#[cfg(all(feature = "metalfx", any(target_os = "macos", target_os = "ios")))]
pub(crate) use metalfx::{Spatial, support as metalfx_support};

#[cfg(not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))))]
pub(crate) fn metalfx_support(_: &RenderDevice) -> Result<(), String> {
    Err("MetalFX unavailable on this platform/build".into())
}
#[cfg(not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))))]
pub(crate) struct Spatial;
#[cfg(not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))))]
impl Spatial {
    pub fn new(_: &RenderDevice, _: &GpuImage, _: &GpuImage) -> Result<Self, String> {
        Err("MetalFX backend not compiled".into())
    }
    pub fn encode(
        &mut self,
        _: &RenderDevice,
        _: &GpuImage,
        _: &GpuImage,
    ) -> Result<[wgpu::CommandBuffer; 2], String> {
        Err("MetalFX backend not compiled".into())
    }
}

#[cfg(all(feature = "metalfx", any(target_os = "macos", target_os = "ios")))]
mod temporal;
#[cfg(all(feature = "metalfx", any(target_os = "macos", target_os = "ios")))]
pub(crate) use temporal::{Temporal, support as temporal_support};
#[cfg(not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))))]
pub(crate) fn temporal_support(_: &bevy::render::renderer::RenderDevice) -> Result<(), String> {
    Err("MetalFX Temporal unavailable on this platform/build".into())
}
#[cfg(not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))))]
pub(crate) struct Temporal;
#[cfg(not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))))]
impl Temporal {
    pub fn set_timing(&mut self, _enabled: bool) {}
    pub fn new(
        _: &bevy::render::renderer::RenderDevice,
        _: bevy::math::UVec2,
        _: bevy::math::UVec2,
    ) -> Result<Self, String> {
        Err("MetalFX Temporal unavailable".into())
    }
    pub fn encode(
        &mut self,
        _: &bevy::render::renderer::RenderDevice,
        _: crate::temporal::Inputs<'_>,
    ) -> Result<[wgpu::CommandBuffer; 2], String> {
        Err("MetalFX Temporal unavailable".into())
    }
}
