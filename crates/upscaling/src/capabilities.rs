use crate::UpscaleMethod;
use bevy::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BackendCapability {
    pub method: UpscaleMethod,
    pub supported: bool,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct UpscalingCapabilities {
    pub backends: Vec<BackendCapability>,
}
impl UpscalingCapabilities {
    pub(crate) fn detect(device: &bevy::render::renderer::RenderDevice) -> Self {
        let support = crate::backends::metalfx_support(device);
        let temporal = crate::backends::temporal_support(device);
        Self {
            backends: vec![
                BackendCapability {
                    method: UpscaleMethod::MetalFxTemporal,
                    supported: temporal.is_ok(),
                    reason: temporal.err(),
                },
                BackendCapability {
                    method: UpscaleMethod::Linear,
                    supported: true,
                    reason: None,
                },
                BackendCapability {
                    method: UpscaleMethod::MetalFxSpatial,
                    supported: support.is_ok(),
                    reason: support.err(),
                },
            ],
        }
    }
    pub fn select(&self, requested: UpscaleMethod) -> (UpscaleMethod, Option<String>) {
        if requested == UpscaleMethod::Linear {
            return (UpscaleMethod::Linear, None);
        }
        let candidate = self.backends.iter().find(|c| {
            c.method
                == if requested == UpscaleMethod::MetalFxTemporal {
                    requested
                } else {
                    UpscaleMethod::MetalFxSpatial
                }
        });
        match candidate {
            Some(c) if c.supported => (c.method, None),
            Some(c) => (UpscaleMethod::Linear, c.reason.clone()),
            None => (
                UpscaleMethod::Linear,
                Some("Backend capabilities not ready".into()),
            ),
        }
    }
}
