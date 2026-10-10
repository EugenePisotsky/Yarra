pub(crate) mod linear;

// MetalFX scalers on Apple platforms with the `metalfx` feature; elsewhere stand-ins that refuse
// to start, so callers fall back to the linear scaler.
#[cfg(all(feature = "metalfx", any(target_os = "macos", target_os = "ios")))]
mod native {
    mod metalfx;
    mod temporal;
    pub(crate) use metalfx::{Spatial, support as metalfx_support};
    pub(crate) use temporal::{Temporal, support as temporal_support};
}
#[cfg(not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))))]
mod native {
    use bevy::{
        math::UVec2,
        render::{renderer::RenderDevice, texture::GpuImage},
    };

    pub(crate) fn metalfx_support(_: &RenderDevice) -> Result<(), String> {
        Err("MetalFX unavailable on this platform/build".into())
    }
    pub(crate) struct Spatial;
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

    pub(crate) fn temporal_support(_: &RenderDevice) -> Result<(), String> {
        Err("MetalFX Temporal unavailable on this platform/build".into())
    }
    pub(crate) struct Temporal;
    impl Temporal {
        pub(crate) fn set_timing(&mut self, _enabled: bool) {}
        pub fn new(_: &RenderDevice, _: UVec2, _: UVec2) -> Result<Self, String> {
            Err("MetalFX Temporal unavailable".into())
        }
        pub fn encode(
            &mut self,
            _: &RenderDevice,
            _: crate::temporal::Inputs<'_>,
        ) -> Result<[wgpu::CommandBuffer; 2], String> {
            Err("MetalFX Temporal unavailable".into())
        }
    }
}
pub(crate) use native::{Spatial, Temporal, metalfx_support, temporal_support};
