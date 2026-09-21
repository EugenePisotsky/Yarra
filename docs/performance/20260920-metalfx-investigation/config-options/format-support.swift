import Foundation
import Metal
import MetalFX

let device = MTLCreateSystemDefaultDevice()!
let variant = CommandLine.arguments[1]
let d = MTLFXTemporalScalerDescriptor()
d.inputWidth = 3456
d.inputHeight = 1942
d.outputWidth = 3456
d.outputHeight = 1942
d.colorTextureFormat = variant == "packed-input" || variant == "packed-both" ? .rg11b10Float : .rgba16Float
d.outputTextureFormat = variant == "packed-output" || variant == "packed-both" ? .rg11b10Float : .rgba16Float
d.depthTextureFormat = .depth32Float
d.motionTextureFormat = .rg16Float
d.isAutoExposureEnabled = true
d.requiresSynchronousInitialization = true
d.isInputContentPropertiesEnabled = true
d.inputContentMinScale = 2
d.inputContentMaxScale = 2
let scaler = d.makeTemporalScaler(device: device)
let result: [String: Any] = ["case": variant, "device": device.name,
    "os": ProcessInfo.processInfo.operatingSystemVersionString,
    "created": scaler != nil,
    "color_format": d.colorTextureFormat.rawValue,
    "output_format": d.outputTextureFormat.rawValue]
print(String(data: try! JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]), encoding: .utf8)!)
