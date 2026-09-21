use crate::UpscaleMethod;
use bevy::prelude::*;

/// Temporal reconstruction needs a different render hook and inputs from spatial scaling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpscaleStage {
    AfterTonemapping,
    BeforePostProcessing,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpscaleRequirements {
    pub stage: UpscaleStage,
    pub antialiased_color: bool,
    pub depth: bool,
    pub motion_vectors: bool,
    pub jitter: bool,
    pub history: bool,
}
pub const SPATIAL_REQUIREMENTS: UpscaleRequirements = UpscaleRequirements {
    stage: UpscaleStage::AfterTonemapping,
    antialiased_color: true,
    depth: false,
    motion_vectors: false,
    jitter: false,
    history: false,
};
pub const TEMPORAL_REQUIREMENTS: UpscaleRequirements = UpscaleRequirements {
    stage: UpscaleStage::BeforePostProcessing,
    antialiased_color: false,
    depth: true,
    motion_vectors: true,
    jitter: true,
    history: true,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendCapability {
    pub method: UpscaleMethod,
    pub supported: bool,
    pub reason: Option<String>,
    pub requirements: UpscaleRequirements,
}
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct UpscalingCapabilities {
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
                    requirements: TEMPORAL_REQUIREMENTS,
                },
                BackendCapability {
                    method: UpscaleMethod::Linear,
                    supported: true,
                    reason: None,
                    requirements: SPATIAL_REQUIREMENTS,
                },
                BackendCapability {
                    method: UpscaleMethod::MetalFxSpatial,
                    supported: support.is_ok(),
                    reason: support.err(),
                    requirements: SPATIAL_REQUIREMENTS,
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
