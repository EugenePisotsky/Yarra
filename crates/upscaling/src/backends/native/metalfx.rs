//! All native API access is contained here. Wgpu owns submission and texture lifetime.
use bevy::render::{renderer::RenderDevice, texture::GpuImage};
use objc2::{
    Message,
    rc::{Retained, autoreleasepool},
    runtime::{AnyClass, ProtocolObject},
};
use objc2_metal::{MTLPixelFormat, MTLResource, MTLStorageMode, MTLTexture};
use objc2_metal_fx::{
    MTLFXSpatialScaler, MTLFXSpatialScalerBase, MTLFXSpatialScalerColorProcessingMode,
    MTLFXSpatialScalerDescriptor,
};

pub(crate) fn support(device: &RenderDevice) -> Result<(), String> {
    if AnyClass::get(c"MTLFXSpatialScalerDescriptor").is_none() {
        return Err("MetalFX is unavailable on this OS".into());
    }
    // SAFETY: read-only capability query; neither the device nor its resources are destroyed.
    unsafe {
        let metal = device
            .wgpu_device()
            .as_hal::<wgpu::hal::api::Metal>()
            .ok_or("Renderer is not Metal")?;
        MTLFXSpatialScalerDescriptor::supportsDevice(metal.raw_device())
            .then_some(())
            .ok_or("Device does not support MetalFX Spatial".into())
    }
}
pub(crate) struct Spatial {
    scaler: Retained<ProtocolObject<dyn MTLFXSpatialScaler>>,
    _output: Retained<ProtocolObject<dyn MTLTexture>>,
    initialized: bool,
}
// SAFETY: MetalFX objects may be used on render threads. All native mutation is
// private and requires &mut Spatial; render-world ResMut serializes encoding.
// These retained objects have no main-thread affinity and expose no shared mutation.
unsafe impl Send for Spatial {}
unsafe impl Sync for Spatial {}

impl Spatial {
    pub fn new(device: &RenderDevice, input: &GpuImage, output: &GpuImage) -> Result<Self, String> {
        support(device)?;
        // SAFETY: native handles are guarded by wgpu, used only to create retained
        // scaler/view objects on the same device. No resource is destroyed or submitted here.
        autoreleasepool(|_| unsafe {
            let metal_device = device
                .wgpu_device()
                .as_hal::<wgpu::hal::api::Metal>()
                .ok_or("Renderer is not Metal")?
                .raw_device()
                .clone();
            // Retain each object separately: as_hal texture guards share a device
            // snatch lock and must not be held simultaneously (recursive read lock).
            let src = input
                .texture
                .as_hal::<wgpu::hal::api::Metal>()
                .ok_or("Input is not a Metal texture")?
                .raw_handle()
                .retain();
            let dst = output
                .texture
                .as_hal::<wgpu::hal::api::Metal>()
                .ok_or("Output is not a Metal texture")?
                .raw_handle()
                .retain();
            // Match Bevy's sRGB output view over the storage-capable RGBA8 allocation.
            let output_view = dst
                .newTextureViewWithPixelFormat(MTLPixelFormat::RGBA8Unorm_sRGB)
                .ok_or("Unable to create output sRGB view")?;
            let descriptor = MTLFXSpatialScalerDescriptor::new();
            descriptor.setInputWidth(input.texture.width() as usize);
            descriptor.setInputHeight(input.texture.height() as usize);
            descriptor.setOutputWidth(output.texture.width() as usize);
            descriptor.setOutputHeight(output.texture.height() as usize);
            descriptor.setColorTextureFormat(src.pixelFormat());
            descriptor.setOutputTextureFormat(output_view.pixelFormat());
            descriptor.setColorProcessingMode(MTLFXSpatialScalerColorProcessingMode::Perceptual);
            let scaler = descriptor
                .newSpatialScalerWithDevice(&metal_device)
                .ok_or("MetalFX rejected the texture dimensions or formats")?;
            if !src.usage().contains(scaler.colorTextureUsage())
                || !output_view.usage().contains(scaler.outputTextureUsage())
            {
                return Err("Textures do not satisfy MetalFX usage requirements".into());
            }
            if output_view.storageMode() != MTLStorageMode::Private {
                return Err("MetalFX output must use private GPU storage".into());
            }
            scaler.setColorTexture(Some(&src));
            scaler.setOutputTexture(Some(&output_view));
            scaler.setInputContentWidth(input.texture.width() as usize);
            scaler.setInputContentHeight(input.texture.height() as usize);
            Ok(Self {
                scaler,
                _output: output_view,
                initialized: false,
            })
        })
    }
    pub fn encode(
        &mut self,
        device: &RenderDevice,
        input: &GpuImage,
        output: &GpuImage,
    ) -> Result<[wgpu::CommandBuffer; 2], String> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("MetalFX resource preparation"),
        });
        if !self.initialized {
            // Register initialization with wgpu. Otherwise a later sampled read can
            // clear the native-written image because native writes bypass its tracker.
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("initialize upscale output"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output.texture_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        encoder.transition_resources(
            std::iter::empty(),
            [
                wgpu::TextureTransition {
                    texture: &*input.texture,
                    selector: None,
                    state: wgpu::TextureUses::RESOURCE,
                },
                wgpu::TextureTransition {
                    texture: &*output.texture,
                    selector: None,
                    state: wgpu::TextureUses::STORAGE_READ_WRITE,
                },
            ]
            .into_iter(),
        );
        let prepared = encoder.finish();
        // Wgpu 29 forbids mixing raw and wgpu recording in one encoder. Bevy queues
        // these two buffers in order, after the scene and before UI, on its own queue.
        let mut native = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("MetalFX Spatial"),
        });
        // SAFETY: this is a fresh encoder used exclusively for native encoding.
        // Never finish/commit the underlying Metal buffer. Textures are tracked by
        // the preceding buffer, retained by the scaler, and use Metal hazard tracking.
        // Later wgpu reads transition the tracked storage usage back to sampled usage.
        autoreleasepool(|_| unsafe {
            native.as_hal_mut::<wgpu::hal::api::Metal, _, _>(|raw| {
                let buffer = raw
                    .and_then(|e| e.raw_command_buffer())
                    .ok_or("Metal command buffer unavailable")?;
                self.scaler.encodeToCommandBuffer(buffer);
                Ok::<_, String>(())
            })
        })?;
        self.initialized = true;
        Ok([prepared, native.finish()])
    }
}
