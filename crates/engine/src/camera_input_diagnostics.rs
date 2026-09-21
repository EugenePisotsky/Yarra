//! Optional observation of input actually consumed by the gameplay camera.
use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    prelude::*,
};
use std::time::Instant;

/// Insert only while tracing. This does not change input routing or smoothing.
#[derive(Resource, Default, Clone, Debug)]
pub struct CameraInputDiagnostics {
    pub sequence: u64,
    pub read_at: Option<Instant>,
    pub wheel_events: usize,
    pub pixel_delta: Vec2,
    pub line_delta: Vec2,
    pub clipped_events: usize,
    pub enabled: bool,
    pub pointer_blocked: bool,
    pub startup_guard: bool,
    pub dt_secs: f32,
    pub requested_orbit: f32,
    pub applied_orbit: f32,
    pub yaw_before: f32,
    pub yaw_after: f32,
    pub target_yaw: f32,
}

impl CameraInputDiagnostics {
    pub(crate) fn record_wheel(&mut self, event: &MouseWheel, limit: f32) {
        self.wheel_events += 1;
        let delta = Vec2::new(event.x, event.y);
        match event.unit {
            MouseScrollUnit::Pixel => self.pixel_delta += delta,
            MouseScrollUnit::Line => self.line_delta += delta,
        }
        self.clipped_events += usize::from(delta.abs().max_element() > limit);
    }
}
