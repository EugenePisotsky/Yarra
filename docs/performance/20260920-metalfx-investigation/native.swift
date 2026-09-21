// Synthetic reconstruction-only diagnostic. Run without a game or GPU capture.
// CPU waits intentionally isolate native command-buffer elapsed time here;
// the game's MetalFX integration does not use these waits.
// Flat inputs do not represent real scene content or validate image quality.
import Metal
import MetalFX
import Foundation
let device = MTLCreateSystemDefaultDevice()!
let queue = device.makeCommandQueue()!
func texture(_ format: MTLPixelFormat, _ w: Int, _ h: Int, _ usage: MTLTextureUsage) -> MTLTexture {
 let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: format, width: w, height: h, mipmapped: false)
 d.storageMode = .private; d.usage = usage.union(.renderTarget)
 return device.makeTexture(descriptor: d)!
}
print("device=\(device.name) os=\(ProcessInfo.processInfo.operatingSystemVersionString)")
for (label, iw, ih, ow, oh, compact, autoExposure) in [
 ("subrect-auto-A",1728,971,3456,1942,false,true),
 ("compact-auto-A",1728,971,3456,1942,true,true),
 ("compact-manual",1728,971,3456,1942,true,false),
 ("compact-auto-B",1728,971,3456,1942,true,true),
 ("subrect-auto-B",1728,971,3456,1942,false,true),
 ("1440p-subrect",1280,720,2560,1440,false,true),
 ("1440p-compact",1280,720,2560,1440,true,true)
] {
 let w=compact ? iw : ow, h=compact ? ih : oh
 let d=MTLFXTemporalScalerDescriptor()
 d.inputWidth=w; d.inputHeight=h; d.outputWidth=ow; d.outputHeight=oh
 d.colorTextureFormat = .rgba16Float; d.depthTextureFormat = .depth32Float
 d.motionTextureFormat = .rg16Float; d.outputTextureFormat = .rgba16Float
 d.isAutoExposureEnabled=autoExposure; d.requiresSynchronousInitialization=true
 d.isInputContentPropertiesEnabled = !compact
 if !compact { d.inputContentMinScale=2; d.inputContentMaxScale=2 }
 let s=d.makeTemporalScaler(device: device)!
 let color=texture(.rgba16Float,w,h,s.colorTextureUsage)
 let depth=texture(.depth32Float,w,h,s.depthTextureUsage)
 let motion=texture(.rg16Float,w,h,s.motionTextureUsage)
 let output=texture(.rgba16Float,ow,oh,s.outputTextureUsage)
 let initBuffer=queue.makeCommandBuffer()!
 let pass=MTLRenderPassDescriptor()
 pass.colorAttachments[0].texture=color; pass.colorAttachments[0].loadAction = .clear; pass.colorAttachments[0].storeAction = .store
 pass.colorAttachments[0].clearColor=MTLClearColorMake(0.3,0.5,0.8,1)
 pass.colorAttachments[1].texture=motion; pass.colorAttachments[1].loadAction = .clear; pass.colorAttachments[1].storeAction = .store
 pass.depthAttachment.texture=depth; pass.depthAttachment.loadAction = .clear; pass.depthAttachment.storeAction = .store; pass.depthAttachment.clearDepth=0.3
 initBuffer.makeRenderCommandEncoder(descriptor: pass)!.endEncoding();initBuffer.commit();initBuffer.waitUntilCompleted()
 s.colorTexture=color; s.depthTexture=depth; s.motionTexture=motion; s.outputTexture=output
 s.inputContentWidth=iw; s.inputContentHeight=ih; s.isDepthReversed=true
 s.motionVectorScaleX = -Float(iw); s.motionVectorScaleY = -Float(ih); s.preExposure=1
 var times=[Double]()
 for frame in 0..<240 {
  autoreleasepool {
   let b=queue.makeCommandBuffer()!
   s.reset=frame == 0; s.jitterOffsetX=0.25; s.jitterOffsetY = -0.25
   s.encode(commandBuffer:b); b.commit();b.waitUntilCompleted()
   if frame>=60 {times.append((b.gpuEndTime-b.gpuStartTime)*1000)}
  }
 }
 times.sort()
 print("case=\(label) samples=\(times.count) median_gpu_ms=\(times[times.count/2]) p95=\(times[Int(Double(times.count)*0.95)])")
}
