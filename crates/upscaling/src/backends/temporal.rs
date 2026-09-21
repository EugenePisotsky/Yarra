//! MetalFX owns history; Bevy/wgpu owns all resources and submission.
use crate::temporal::Inputs;
use bevy::{prelude::*, render::renderer::RenderDevice};
use objc2::{
    Message,
    rc::{Retained, autoreleasepool},
    runtime::{AnyClass, ProtocolObject},
};
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLPixelFormat, MTLTexture};
use objc2_metal_fx::{MTLFXTemporalScaler, MTLFXTemporalScalerBase, MTLFXTemporalScalerDescriptor};

pub(crate) fn support(device: &RenderDevice) -> Result<(), String> {
    if AnyClass::get(c"MTLFXTemporalScalerDescriptor").is_none() {
        return Err("MetalFX Temporal unavailable on this OS".into());
    }
    // SAFETY: guarded device, read-only capability query.
    unsafe {
        let metal = device
            .wgpu_device()
            .as_hal::<wgpu::hal::api::Metal>()
            .ok_or("Renderer is not Metal")?;
        MTLFXTemporalScalerDescriptor::supportsDevice(metal.raw_device())
            .then_some(())
            .ok_or("Device does not support MetalFX Temporal".into())
    }
}
pub(crate) struct Temporal {
    scaler: Retained<ProtocolObject<dyn MTLFXTemporalScaler>>,
    initialized: Vec<bevy::render::render_resource::TextureId>,
    exposure: bevy::render::render_resource::Texture,
    timing: bool,
    frame: u64,
    pending_timing:
        std::collections::VecDeque<(u64, bool, Retained<ProtocolObject<dyn MTLCommandBuffer>>)>,
}
// SAFETY: private native mutation requires &mut self and is serialized in RenderWorld.
unsafe impl Send for Temporal {}
unsafe impl Sync for Temporal {}
impl Temporal {
    pub fn new(device: &RenderDevice, input: UVec2, output: UVec2) -> Result<Self, String> {
        support(device)?;
        // SAFETY: creates a retained scaler on the same device as our guarded textures.
        autoreleasepool(|_| unsafe {
            let metal = device
                .wgpu_device()
                .as_hal::<wgpu::hal::api::Metal>()
                .ok_or("Renderer is not Metal")?
                .raw_device()
                .clone();
            let d = MTLFXTemporalScalerDescriptor::new();
            d.setInputWidth(output.x as usize);
            d.setInputHeight(output.y as usize);
            d.setOutputWidth(output.x as usize);
            d.setOutputHeight(output.y as usize);
            d.setColorTextureFormat(MTLPixelFormat::RGBA16Float);
            d.setDepthTextureFormat(MTLPixelFormat::Depth32Float);
            d.setMotionTextureFormat(MTLPixelFormat::RG16Float);
            d.setOutputTextureFormat(MTLPixelFormat::RGBA16Float);
            // Scene HDR is already camera-exposed (including custom grass). A second
            // auto exposure gives MetalFX a different brightness than our tone mapper.
            d.setAutoExposureEnabled(false);
            d.setRequiresSynchronousInitialization(true);
            d.setInputContentPropertiesEnabled(true);
            let scale = (output.as_vec2() / input.as_vec2()).max_element();
            d.setInputContentMinScale(scale);
            d.setInputContentMaxScale(scale);
            let scaler = d
                .newTemporalScalerWithDevice(&metal)
                .ok_or("MetalFX rejected temporal dimensions/formats")?;
            let exposure = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("temporal unit exposure"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            Ok(Self {
                scaler,
                exposure,
                initialized: Vec::with_capacity(2),
                timing: false,
                frame: 0,
                pending_timing: default(),
            })
        })
    }
    pub fn set_timing(&mut self, enabled: bool) {
        self.timing = enabled;
    }

    pub fn encode(
        &mut self,
        device: &RenderDevice,
        i: Inputs<'_>,
    ) -> Result<[wgpu::CommandBuffer; 2], String> {
        // SAFETY: retain handles separately; holding two as_hal guards deadlocks wgpu's snatch lock.
        // No resource destruction or queue submission occurs through the native API.
        autoreleasepool(|_| unsafe {
            self.frame = self.frame.wrapping_add(1);
            while let Some((frame, reset, b)) = self.pending_timing.front() {
                if b.status() == MTLCommandBufferStatus::Error {
                    warn!("METALFX_TIMING frame={frame} command_buffer_failed=true");
                } else if b.status() == MTLCommandBufferStatus::Completed {
                    // Native command-buffer elapsed time can include dependency stalls.
                    // It is a cross-check, not an additive pass cost or a whole-frame total.
                    warn!(
                        "METALFX_TIMING frame={frame} reset={reset} gpu_ms={:.3} completed_after_frames={}",
                        (b.GPUEndTime() - b.GPUStartTime()) * 1000.,
                        self.frame.wrapping_sub(*frame)
                    );
                } else {
                    break;
                }
                self.pending_timing.pop_front();
            }
            let retain = |t: &bevy::render::render_resource::Texture| {
                t.as_hal::<wgpu::hal::api::Metal>()
                    .map(|t| t.raw_handle().retain())
                    .ok_or_else(|| "Temporal texture is not Metal".to_string())
            };
            let color = retain(i.color)?;
            let depth = retain(i.depth)?;
            let motion = retain(i.motion)?;
            let output = retain(i.output)?;
            let exposure = retain(&self.exposure)?;
            if !color.usage().contains(self.scaler.colorTextureUsage())
                || !depth.usage().contains(self.scaler.depthTextureUsage())
                || !motion.usage().contains(self.scaler.motionTextureUsage())
                || !output.usage().contains(self.scaler.outputTextureUsage())
            {
                return Err("Temporal texture usage requirements not satisfied".into());
            }
            self.scaler.setColorTexture(Some(&color));
            self.scaler.setDepthTexture(Some(&depth));
            self.scaler.setMotionTexture(Some(&motion));
            self.scaler.setOutputTexture(Some(&output));
            self.scaler.setExposureTexture(Some(&exposure));
            self.scaler.setInputContentWidth(i.size.x as usize);
            self.scaler.setInputContentHeight(i.size.y as usize);
            self.scaler.setJitterOffsetX(-i.jitter.x);
            self.scaler.setJitterOffsetY(-i.jitter.y);
            // Shared motion convention: current UV minus previous UV, excluding jitter.
            self.scaler.setMotionVectorScaleX(-(i.size.x as f32));
            self.scaler.setMotionVectorScaleY(-(i.size.y as f32));
            self.scaler.setDepthReversed(true);
            self.scaler.setPreExposure(1.0);
            self.scaler.setReset(i.reset);
            let mut prep = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("temporal resources"),
            });
            // Register each ping-pong allocation once before bypassing the wgpu writer.
            let initialize = !self.initialized.contains(&i.output.id());
            if initialize {
                let _pass = prep.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("initialize temporal output"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: i.output_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
            if self.initialized.is_empty() {
                let view = self.exposure.create_view(&Default::default());
                let _pass = prep.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("initialize temporal unit exposure"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
            prep.transition_resources(
                std::iter::empty(),
                [
                    wgpu::TextureTransition {
                        texture: &*self.exposure,
                        selector: None,
                        state: wgpu::TextureUses::RESOURCE,
                    },
                    wgpu::TextureTransition {
                        texture: &**i.color,
                        selector: None,
                        state: wgpu::TextureUses::RESOURCE,
                    },
                    wgpu::TextureTransition {
                        texture: &**i.depth,
                        selector: None,
                        state: wgpu::TextureUses::RESOURCE,
                    },
                    wgpu::TextureTransition {
                        texture: &**i.motion,
                        selector: None,
                        state: wgpu::TextureUses::RESOURCE,
                    },
                    wgpu::TextureTransition {
                        texture: &**i.output,
                        selector: None,
                        state: wgpu::TextureUses::STORAGE_READ_WRITE,
                    },
                ]
                .into_iter(),
            );
            let mut native = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("MetalFX Temporal"),
            });
            // A fresh encoder must be exclusively native (wgpu29 forbids mixing encoders).
            native.as_hal_mut::<wgpu::hal::api::Metal, _, _>(|raw| {
                let buffer = raw
                    .and_then(|e| e.raw_command_buffer())
                    .ok_or("Metal command buffer unavailable")?;
                self.scaler.encodeToCommandBuffer(buffer);
                if self.timing && self.frame.is_multiple_of(120) && self.pending_timing.len() < 4 {
                    self.pending_timing
                        .push_back((self.frame, i.reset, buffer.retain()));
                }
                Ok::<_, String>(())
            })?;
            if initialize {
                if self.initialized.len() == 2 {
                    self.initialized.remove(0);
                }
                self.initialized.push(i.output.id());
            }
            Ok([prep.finish(), native.finish()])
        })
    }
}
