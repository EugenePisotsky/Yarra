// Reconstruction only, using the empty example's constant color, zero depth/motion,
// dimensions, formats, auto exposure and jitter sequence. No Bevy/wgpu or window.
// CPU completion waits are intentional here to isolate the native command buffer;
// production rendering does not wait on the CPU this way.
import Foundation
import Metal
import MetalFX

let device = MTLCreateSystemDefaultDevice()!
let queue = device.makeCommandQueue()!
let inputWidth = 1728, inputHeight = 971, outputWidth = 3456, outputHeight = 1942
let packed = CommandLine.arguments.contains("--packed")
let colorFormat: MTLPixelFormat = packed ? .rg11b10Float : .rgba16Float
let descriptor = MTLFXTemporalScalerDescriptor()
descriptor.inputWidth = outputWidth
descriptor.inputHeight = outputHeight
descriptor.outputWidth = outputWidth
descriptor.outputHeight = outputHeight
descriptor.colorTextureFormat = colorFormat
descriptor.depthTextureFormat = .depth32Float
descriptor.motionTextureFormat = .rg16Float
descriptor.outputTextureFormat = colorFormat
descriptor.isAutoExposureEnabled = true
descriptor.requiresSynchronousInitialization = true
descriptor.isInputContentPropertiesEnabled = true
descriptor.inputContentMinScale = 2
descriptor.inputContentMaxScale = 2
let scaler = descriptor.makeTemporalScaler(device: device)!

func texture(_ format: MTLPixelFormat, _ usage: MTLTextureUsage) -> MTLTexture {
    let d = MTLTextureDescriptor.texture2DDescriptor(
        pixelFormat: format, width: outputWidth, height: outputHeight, mipmapped: false)
    d.storageMode = .private
    d.usage = usage.union(.renderTarget)
    return device.makeTexture(descriptor: d)!
}
let color = texture(colorFormat, scaler.colorTextureUsage.union(.shaderWrite))
let depth = texture(.depth32Float, scaler.depthTextureUsage)
let motion = texture(.rg16Float, scaler.motionTextureUsage)
let output = texture(colorFormat, scaler.outputTextureUsage)
let initialize = queue.makeCommandBuffer()!
let pass = MTLRenderPassDescriptor()
pass.colorAttachments[0].texture = color
pass.colorAttachments[0].loadAction = .clear
pass.colorAttachments[0].storeAction = .store
func linear(_ x: Double) -> Double { pow((x + 0.055) / 1.055, 2.4) }
pass.colorAttachments[0].clearColor = MTLClearColorMake(linear(0.2), linear(0.25), linear(0.3), 1)
pass.colorAttachments[1].texture = motion
pass.colorAttachments[1].loadAction = .clear
pass.colorAttachments[1].storeAction = .store
pass.colorAttachments[1].clearColor = MTLClearColorMake(0, 0, 0, 0)
pass.depthAttachment.texture = depth
pass.depthAttachment.loadAction = .clear
pass.depthAttachment.storeAction = .store
pass.depthAttachment.clearDepth = 0
initialize.makeRenderCommandEncoder(descriptor: pass)!.endEncoding()
initialize.commit()
initialize.waitUntilCompleted()
precondition(initialize.status == .completed)

scaler.colorTexture = color
scaler.depthTexture = depth
scaler.motionTexture = motion
scaler.outputTexture = output
scaler.inputContentWidth = inputWidth
scaler.inputContentHeight = inputHeight
scaler.isDepthReversed = true
scaler.motionVectorScaleX = -Float(inputWidth)
scaler.motionVectorScaleY = -Float(inputHeight)
scaler.preExposure = 1
func halton(_ number: Int, _ base: Int) -> Float {
    var n = number, fraction: Float = 1, result: Float = 0
    while n > 0 {
        fraction /= Float(base)
        result += fraction * Float(n % base)
        n /= base
    }
    return result
}
func emit(_ event: [String: Any]) {
    print(String(data: try! JSONSerialization.data(withJSONObject: event, options: [.sortedKeys]), encoding: .utf8)!)
    fflush(stdout)
}
emit(["event": "setup", "packed_hdr": packed, "device": device.name, "os": ProcessInfo.processInfo.operatingSystemVersionString,
      "input": [inputWidth, inputHeight], "output": [outputWidth, outputHeight], "target_fps": 120])
let start = ProcessInfo.processInfo.systemUptime
var times = [Double](), frame = 0, measureStart: Double? = nil
while ProcessInfo.processInfo.systemUptime - start < 16 {
    let due = start + Double(frame) / 120
    let remaining = due - ProcessInfo.processInfo.systemUptime
    if remaining > 0 { Thread.sleep(forTimeInterval: remaining) }
    let measuring = ProcessInfo.processInfo.systemUptime - start >= 8
    if measuring && measureStart == nil {
        measureStart = ProcessInfo.processInfo.systemUptime
        emit(["event": "measure_start", "unix_ms": Date().timeIntervalSince1970 * 1000])
    }
    autoreleasepool {
        let buffer = queue.makeCommandBuffer()!
        buffer.label = "Empty MetalFX reconstruction only"
        scaler.reset = frame == 0
        scaler.jitterOffsetX = -(halton(frame % 32 + 1, 2) - 0.5)
        scaler.jitterOffsetY = -(halton(frame % 32 + 1, 3) - 0.5)
        scaler.encode(commandBuffer: buffer)
        buffer.commit()
        buffer.waitUntilCompleted()
        precondition(buffer.status == .completed, "MetalFX failed: \(String(describing: buffer.error))")
        if measuring { times.append((buffer.gpuEndTime - buffer.gpuStartTime) * 1000) }
    }
    frame += 1
}
times.sort()
emit(["event": "complete", "unix_ms": Date().timeIntervalSince1970 * 1000,
      "samples": times.count, "median_gpu_ms": times[times.count / 2],
      "p95_gpu_ms": times[Int(Double(times.count) * 0.95)],
      "frames": frame, "measured_fps": Double(times.count) / (ProcessInfo.processInfo.systemUptime - measureStart!)])
